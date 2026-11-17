//! Draws mpv video underneath the Slint UI using Slint's rendering notifier.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_player::{Player, TrackKind, VideoRenderer};
use glow::HasContext;
use slint::{ComponentHandle, GraphicsAPI, RenderingState, Rgba8Pixel, SharedPixelBuffer};

/// Lets a caller grab the next frame exactly as it was drawn to the window,
/// read back from the window's framebuffer after Slint finished drawing.
/// Used by the self-test; `take_snapshot` alone renders to a texture instead.
#[derive(Clone, Default)]
pub struct ScreenCapture(Rc<RefCell<CaptureState>>);

#[derive(Default)]
enum CaptureState {
    #[default]
    Idle,
    Requested,
    Done(SharedPixelBuffer<Rgba8Pixel>),
}

impl ScreenCapture {
    /// Asks for the next rendered frame to be captured.
    pub fn request(&self) {
        *self.0.borrow_mut() = CaptureState::Requested;
    }

    /// Returns the captured frame once it is available.
    pub fn take(&self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let mut state = self.0.borrow_mut();
        match std::mem::take(&mut *state) {
            CaptureState::Done(buf) => Some(buf),
            other => {
                *state = other;
                None
            }
        }
    }

    fn wanted(&self) -> bool {
        matches!(*self.0.borrow(), CaptureState::Requested)
    }
}

/// The video output shared by the app's windows (main window and PiP).
///
/// mpv allows one render context at a time, so only the active window owns
/// one. Moving the video to another window frees the render context in the
/// old window (with its GL context current) before the new window creates
/// its own; playback itself continues.
#[derive(Clone)]
pub struct VideoOutput(Rc<RefCell<Output>>);

struct Output {
    player: Arc<dyn Player>,
    /// The window that should show the video.
    active: usize,
    /// The window that owns the render context right now, if any.
    holder: Option<usize>,
    /// Requests a redraw of each attached window, by id.
    redraw: Vec<Rc<dyn Fn()>>,
    /// Runs when the first render context exists.
    on_ready: Option<Box<dyn FnOnce()>>,
    /// Runs when the video arrived in the window `move_to` asked for.
    on_moved: Option<Box<dyn FnOnce()>>,
    /// Video track to select again once the new render context exists.
    restore_video: Option<i64>,
}

/// Per-window GL state, owned by that window's rendering notifier.
struct WindowVideo {
    gl: glow::Context,
    renderer: Option<Box<dyn VideoRenderer>>,
}

impl VideoOutput {
    /// `on_ready` runs once the first mpv render context exists; load media
    /// only after that, or mpv fails to open its video output.
    pub fn new(player: Arc<dyn Player>, on_ready: impl FnOnce() + 'static) -> Self {
        Self(Rc::new(RefCell::new(Output {
            player,
            active: 0,
            holder: None,
            redraw: Vec::new(),
            on_ready: Some(Box::new(on_ready)),
            on_moved: None,
            restore_video: None,
        })))
    }

    /// Draws the video under `component`'s Slint items when this window is
    /// active. Returns the window's id for [`Self::move_to`]; the first
    /// window attached (id 0) starts out active. `capture`, if given, can
    /// grab what this window drew.
    pub fn attach<C: ComponentHandle + 'static>(
        &self,
        component: &C,
        capture: Option<ScreenCapture>,
    ) -> Result<usize, slint::SetRenderingNotifierError> {
        let weak = component.as_weak();
        let redraw: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(c) = weak.upgrade() {
                c.window().request_redraw();
            }
        });
        let id = {
            let mut out = self.0.borrow_mut();
            out.redraw.push(redraw);
            out.redraw.len() - 1
        };

        let output = self.clone();
        let weak = component.as_weak();
        let mut state: Option<WindowVideo> = None;
        component
            .window()
            .set_rendering_notifier(move |rendering_state, graphics_api| match rendering_state {
                RenderingState::RenderingSetup => {
                    let GraphicsAPI::NativeOpenGL { get_proc_address } = graphics_api else {
                        eprintln!("video: renderer is not OpenGL, video cannot be drawn");
                        return;
                    };
                    // SAFETY: Slint calls the notifier on the UI thread with its
                    // GL context current, and all later calls happen there too.
                    let gl = unsafe {
                        glow::Context::from_loader_function_cstr(|name| get_proc_address(name))
                    };
                    state = Some(WindowVideo { gl, renderer: None });
                }
                RenderingState::BeforeRendering => {
                    let (Some(st), Some(c)) = (state.as_mut(), weak.upgrade()) else {
                        return;
                    };
                    let GraphicsAPI::NativeOpenGL { get_proc_address } = graphics_api else {
                        return;
                    };
                    output.sync(id, st, get_proc_address, &weak);
                    let Some(renderer) = &st.renderer else { return };
                    let size = c.window().size();
                    // SAFETY: GL context is current (see RenderingSetup).
                    unsafe {
                        let saved = GlState::save(&st.gl);
                        let fbo = saved.framebuffer;
                        GlState::reset_for_mpv(&st.gl);
                        // Slint renders to the window (FBO 0) or, for snapshots, to
                        // a texture FBO; the renderer handles orientation for both.
                        if let Err(err) =
                            renderer.render(fbo, size.width as i32, size.height as i32)
                        {
                            eprintln!("video: {err}");
                        }
                        saved.restore(&st.gl);
                    }
                }
                RenderingState::AfterRendering => {
                    let Some(st) = &state else { return };
                    let Some(renderer) = &st.renderer else { return };
                    renderer.report_swap();
                    if let Some(capture) = &capture
                        && capture.wanted()
                        && let Some(c) = weak.upgrade()
                    {
                        let size = c.window().size();
                        // SAFETY: GL context is current (see RenderingSetup).
                        let frame = unsafe { read_framebuffer(&st.gl, size.width, size.height) };
                        *capture.0.borrow_mut() = CaptureState::Done(frame);
                    }
                }
                RenderingState::RenderingTeardown => {
                    // Free the render context while the GL context is still current.
                    if let Some(st) = state.as_mut() {
                        output.release(id, st);
                    }
                    state = None;
                }
                _ => {}
            })?;
        Ok(id)
    }

    /// Moves the video to window `id`. `then` runs once that window draws it.
    pub fn move_to(&self, id: usize, then: impl FnOnce() + 'static) {
        let (old, new) = {
            let mut out = self.0.borrow_mut();
            if out.active == id {
                drop(out);
                then();
                return;
            }
            // Freeing a render context while video plays makes mpv fail the
            // video output, which ends a file without audio. Turn video off
            // first and back on in the new window; playback carries on.
            if out.holder.is_some() && out.restore_video.is_none() {
                let video = out
                    .player
                    .tracks()
                    .into_iter()
                    .find(|t| t.kind == TrackKind::Video && t.selected);
                if let Some(track) = video {
                    out.restore_video = Some(track.id);
                    let _ = out.player.select_track(TrackKind::Video, None);
                }
            }
            out.active = id;
            out.on_moved = Some(Box::new(then));
            let old = out.holder.and_then(|h| out.redraw.get(h).cloned());
            (old, out.redraw.get(id).cloned())
        };
        // The old window frees its render context on its next frame, then
        // asks the new one to draw.
        for redraw in [old, new].into_iter().flatten() {
            redraw();
        }
    }

    /// Frees or creates this window's render context as needed.
    fn sync<C: ComponentHandle + 'static>(
        &self,
        id: usize,
        st: &mut WindowVideo,
        get_proc_address: &dyn Fn(&std::ffi::CStr) -> *const std::ffi::c_void,
        weak: &slint::Weak<C>,
    ) {
        let (active, holder) = {
            let out = self.0.borrow();
            (out.active, out.holder)
        };
        if st.renderer.is_some() && active != id {
            self.release(id, st);
            return;
        }
        if st.renderer.is_some() {
            // Still the active window, e.g. PiP was toggled on and off before
            // this window drew a frame: finish any pending move here.
            self.finish_move();
            return;
        }
        if active != id || holder.is_some() {
            return;
        }

        let player = self.0.borrow().player.clone();
        // SAFETY: called from BeforeRendering with this window's GL context
        // current; the renderer is only used and dropped there as well.
        match unsafe { player.create_renderer(get_proc_address) } {
            Ok(mut renderer) => {
                let weak = weak.clone();
                renderer.set_update_callback(Box::new(move || {
                    let weak = weak.clone();
                    // Called on an mpv thread: hop to the UI thread to redraw.
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(c) = weak.upgrade() {
                            c.window().request_redraw();
                        }
                    });
                }));
                st.renderer = Some(renderer);
                self.0.borrow_mut().holder = Some(id);
                self.finish_move();
            }
            Err(err) => eprintln!("video: {err}"),
        }
    }

    /// Turns video back on and runs the pending ready / moved callbacks.
    /// They run after the current frame, not inside it: they may load media
    /// or hide a window, and hiding destroys that window's GL context
    /// (making it current for teardown) while this frame still draws.
    fn finish_move(&self) {
        let (ready, moved) = {
            let mut out = self.0.borrow_mut();
            if let Some(track) = out.restore_video.take() {
                let _ = out.player.select_track(TrackKind::Video, Some(track));
            }
            (out.on_ready.take(), out.on_moved.take())
        };
        if ready.is_none() && moved.is_none() {
            return;
        }
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(f) = ready {
                f();
            }
            if let Some(f) = moved {
                f();
            }
        });
    }

    /// Frees this window's render context and lets the active window
    /// create its own.
    fn release(&self, id: usize, st: &mut WindowVideo) {
        if st.renderer.take().is_none() {
            return;
        }
        let redraw = {
            let mut out = self.0.borrow_mut();
            if out.holder == Some(id) {
                out.holder = None;
            }
            out.redraw.get(out.active).cloned()
        };
        if let Some(redraw) = redraw {
            redraw();
        }
    }
}

/// The bits of GL state mpv may change that Slint's renderer relies on.
struct GlState {
    framebuffer: i32,
    program: i32,
    vertex_array: i32,
    array_buffer: i32,
    active_texture: i32,
    texture_2d: i32,
    viewport: [i32; 4],
    blend: bool,
    scissor: bool,
    stencil: bool,
    depth: bool,
    cull_face: bool,
    color_mask: [bool; 4],
    unpack_alignment: i32,
    pixel_unpack_buffer: i32,
}

impl GlState {
    /// # Safety
    /// The GL context behind `gl` must be current.
    unsafe fn save(gl: &glow::Context) -> Self {
        // SAFETY: caller guarantees a current context; these are plain queries.
        unsafe {
            let mut viewport = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            Self {
                framebuffer: gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
                program: gl.get_parameter_i32(glow::CURRENT_PROGRAM),
                vertex_array: gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING),
                array_buffer: gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING),
                active_texture: gl.get_parameter_i32(glow::ACTIVE_TEXTURE),
                texture_2d: gl.get_parameter_i32(glow::TEXTURE_BINDING_2D),
                viewport,
                blend: gl.is_enabled(glow::BLEND),
                scissor: gl.is_enabled(glow::SCISSOR_TEST),
                stencil: gl.is_enabled(glow::STENCIL_TEST),
                depth: gl.is_enabled(glow::DEPTH_TEST),
                cull_face: gl.is_enabled(glow::CULL_FACE),
                color_mask: {
                    let mut m = [0; 4];
                    gl.get_parameter_i32_slice(glow::COLOR_WRITEMASK, &mut m);
                    m.map(|v| v != 0)
                },
                unpack_alignment: gl.get_parameter_i32(glow::UNPACK_ALIGNMENT),
                pixel_unpack_buffer: gl.get_parameter_i32(glow::PIXEL_UNPACK_BUFFER_BINDING),
            }
        }
    }

    /// Puts GL into the default state mpv's renderer expects. Slint's
    /// renderer may leave stencil or scissor tests on, which can hide the
    /// video entirely.
    ///
    /// # Safety
    /// The GL context behind `gl` must be current.
    unsafe fn reset_for_mpv(gl: &glow::Context) {
        // SAFETY: caller guarantees a current context.
        unsafe {
            for cap in [
                glow::BLEND,
                glow::SCISSOR_TEST,
                glow::STENCIL_TEST,
                glow::DEPTH_TEST,
                glow::CULL_FACE,
            ] {
                gl.disable(cap);
            }
            gl.color_mask(true, true, true, true);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.use_program(None);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.active_texture(glow::TEXTURE0);
        }
    }

    /// # Safety
    /// The GL context behind `gl` must be current.
    unsafe fn restore(&self, gl: &glow::Context) {
        fn name<T>(id: i32, make: fn(std::num::NonZeroU32) -> T) -> Option<T> {
            std::num::NonZeroU32::new(id as u32).map(make)
        }
        // SAFETY: caller guarantees a current context; ids come from queries
        // on the same context.
        unsafe {
            gl.bind_framebuffer(
                glow::FRAMEBUFFER,
                name(self.framebuffer, glow::NativeFramebuffer),
            );
            gl.use_program(name(self.program, glow::NativeProgram));
            gl.bind_vertex_array(name(self.vertex_array, glow::NativeVertexArray));
            gl.bind_buffer(
                glow::ARRAY_BUFFER,
                name(self.array_buffer, glow::NativeBuffer),
            );
            gl.active_texture(self.active_texture as u32);
            gl.bind_texture(glow::TEXTURE_2D, name(self.texture_2d, glow::NativeTexture));
            let [x, y, w, h] = self.viewport;
            gl.viewport(x, y, w, h);
            set_enabled(gl, glow::BLEND, self.blend);
            set_enabled(gl, glow::SCISSOR_TEST, self.scissor);
            set_enabled(gl, glow::STENCIL_TEST, self.stencil);
            set_enabled(gl, glow::DEPTH_TEST, self.depth);
            set_enabled(gl, glow::CULL_FACE, self.cull_face);
            let [r, g, b, a] = self.color_mask;
            gl.color_mask(r, g, b, a);
            gl.bind_buffer(
                glow::PIXEL_UNPACK_BUFFER,
                name(self.pixel_unpack_buffer, glow::NativeBuffer),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, self.unpack_alignment);
        }
    }
}

/// # Safety
/// The GL context behind `gl` must be current.
unsafe fn set_enabled(gl: &glow::Context, cap: u32, on: bool) {
    // SAFETY: caller guarantees a current context.
    unsafe {
        if on {
            gl.enable(cap);
        } else {
            gl.disable(cap);
        }
    }
}

/// Reads the bound framebuffer into a top-down RGBA buffer.
///
/// # Safety
/// The GL context behind `gl` must be current.
unsafe fn read_framebuffer(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let row = width as usize * 4;
    let mut bottom_up = vec![0u8; row * height as usize];
    // SAFETY: caller guarantees a current context; the buffer is large
    // enough for width x height RGBA pixels with pack alignment 4.
    unsafe {
        let draw = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        gl.bind_framebuffer(
            glow::READ_FRAMEBUFFER,
            std::num::NonZeroU32::new(draw as u32).map(glow::NativeFramebuffer),
        );
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut bottom_up)),
        );
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let out = buf.make_mut_bytes();
    for (y, src) in bottom_up.chunks_exact(row).rev().enumerate() {
        out[y * row..(y + 1) * row].copy_from_slice(src);
    }
    buf
}
