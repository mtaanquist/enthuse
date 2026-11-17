//! Draws mpv video underneath the Slint UI using Slint's rendering notifier.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_player::gl::GlRenderContext;
use enthuse_player::libmpv2::Mpv;
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

/// Installs a rendering notifier on `window` that draws the current mpv
/// frame before Slint draws its own items.
///
/// `on_ready` runs once the mpv render context exists; load media only after
/// that, or mpv fails to open its video output.
pub fn attach<C: ComponentHandle + 'static>(
    component: &C,
    mpv: Arc<Mpv>,
    on_ready: impl FnOnce() + 'static,
    capture: ScreenCapture,
) -> Result<(), slint::SetRenderingNotifierError> {
    let mut on_ready = Some(on_ready);
    let weak = component.as_weak();
    let mut state: Option<(GlRenderContext, glow::Context)> = None;

    component
        .window()
        .set_rendering_notifier(move |rendering_state, graphics_api| match rendering_state {
            RenderingState::RenderingSetup => {
                let GraphicsAPI::NativeOpenGL { get_proc_address } = graphics_api else {
                    eprintln!("video: renderer is not OpenGL, video cannot be drawn");
                    return;
                };
                // SAFETY: Slint calls the notifier on the UI thread with its GL
                // context current, and all later calls (render, drop on
                // teardown) happen there as well.
                let gl = unsafe {
                    glow::Context::from_loader_function_cstr(|name| get_proc_address(name))
                };
                // SAFETY: as above.
                match unsafe { GlRenderContext::new(mpv.clone(), get_proc_address) } {
                    Ok(mut ctx) => {
                        let weak = weak.clone();
                        ctx.set_update_callback(move || {
                            let weak = weak.clone();
                            // Called on an mpv thread: hop to the UI thread to redraw.
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(c) = weak.upgrade() {
                                    c.window().request_redraw();
                                }
                            });
                        });
                        state = Some((ctx, gl));
                        if let Some(f) = on_ready.take() {
                            f();
                        }
                    }
                    Err(err) => eprintln!("video: {err}"),
                }
            }
            RenderingState::BeforeRendering => {
                let (Some((ctx, gl)), Some(c)) = (&state, weak.upgrade()) else {
                    return;
                };
                ctx.update();
                let size = c.window().size();
                // SAFETY: GL context is current (see RenderingSetup).
                unsafe {
                    let saved = GlState::save(gl);
                    let fbo = saved.framebuffer;
                    // Slint renders to the window (FBO 0) or, for snapshots, to a
                    // texture FBO. Both use GL's bottom-up row order, so mpv
                    // always needs Y flipped.
                    if let Err(err) = ctx.render(fbo, size.width as i32, size.height as i32, true) {
                        eprintln!("video: {err}");
                    }
                    saved.restore(gl);
                }
            }
            RenderingState::AfterRendering => {
                let Some((ctx, gl)) = &state else { return };
                ctx.report_swap();
                if capture.wanted()
                    && let Some(c) = weak.upgrade()
                {
                    let size = c.window().size();
                    // SAFETY: GL context is current (see RenderingSetup).
                    let frame = unsafe { read_framebuffer(gl, size.width, size.height) };
                    *capture.0.borrow_mut() = CaptureState::Done(frame);
                }
            }
            RenderingState::RenderingTeardown => {
                // Free the render context while the GL context is still current.
                state = None;
            }
            _ => {}
        })
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
    unpack_alignment: i32,
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
                unpack_alignment: gl.get_parameter_i32(glow::UNPACK_ALIGNMENT),
            }
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
