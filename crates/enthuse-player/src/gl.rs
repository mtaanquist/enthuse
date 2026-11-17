//! OpenGL rendering of mpv video through mpv's render API.
//!
//! The app owns the OpenGL context (Slint's, in our case). Each frame it asks
//! [`GlRenderContext::render`] to draw the current video frame into whatever
//! framebuffer is bound, and then draws its own UI on top. This is what lets
//! controls and overlays sit on top of the video.
//!
//! All methods must be called on the thread that owns the GL context, with
//! that context current.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::Arc;

use libmpv2::Mpv;
use libmpv2_sys as sys;

use crate::{GetProcAddress, PlayerError, VideoRenderer};

/// Error from the mpv render API, holding mpv's error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderError(pub c_int);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SAFETY: mpv_error_string accepts any error code and returns a
        // pointer to a static, NUL-terminated string.
        let msg = unsafe { CStr::from_ptr(sys::mpv_error_string(self.0)) };
        write!(f, "mpv render error {}: {}", self.0, msg.to_string_lossy())
    }
}

impl std::error::Error for RenderError {}

type UpdateCallback = Box<dyn Fn() + Send + 'static>;

/// An mpv render context bound to the app's OpenGL context.
///
/// Holds a reference to the mpv core so the core always outlives the render
/// context, as mpv requires.
pub(crate) struct GlRenderContext {
    ctx: *mut sys::mpv_render_context,
    // Double-boxed so the pointer handed to mpv stays thin and stable.
    update_callback: Option<Box<UpdateCallback>>,
    // Dropped after `ctx` is freed (see Drop).
    _mpv: Arc<Mpv>,
}

impl GlRenderContext {
    /// Creates a render context for `mpv` using the current OpenGL context.
    ///
    /// `get_proc_address` only needs to be valid for the duration of this
    /// call: mpv resolves all GL functions while creating the context.
    ///
    /// # Safety
    ///
    /// The OpenGL context that `get_proc_address` belongs to must be current
    /// on this thread, and every later call on the returned value (including
    /// drop) must happen on this thread with that context current.
    pub(crate) unsafe fn new(
        mpv: Arc<Mpv>,
        get_proc_address: GetProcAddress<'_>,
    ) -> Result<Self, RenderError> {
        unsafe extern "C" fn trampoline(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
            // SAFETY: `ctx` is the `&GetProcAddress` passed below, which lives
            // until mpv_render_context_create returns, and mpv only calls this
            // during that call. `name` is a NUL-terminated string from mpv.
            unsafe {
                let gpa = &*(ctx as *const GetProcAddress<'_>);
                gpa(CStr::from_ptr(name)) as *mut c_void
            }
        }

        let mut init = sys::mpv_opengl_init_params {
            get_proc_address: Some(trampoline),
            get_proc_address_ctx: &get_proc_address as *const GetProcAddress<'_> as *mut c_void,
        };
        let api_type = sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void;
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: api_type,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut ctx = ptr::null_mut();
        // SAFETY: `mpv.ctx` is a live handle kept alive by the Arc; `params`
        // is a correctly terminated array whose pointers outlive the call.
        let err = unsafe {
            sys::mpv_render_context_create(&mut ctx, mpv.ctx.as_ptr(), params.as_mut_ptr())
        };
        if err < 0 {
            return Err(RenderError(err));
        }
        Ok(Self {
            ctx,
            update_callback: None,
            _mpv: mpv,
        })
    }

    /// Sets a callback that mpv invokes, from any thread, when a new frame
    /// should be drawn. The callback must not call into mpv; it should only
    /// schedule a redraw on the GL thread.
    pub fn set_update_callback(&mut self, callback: impl Fn() + Send + 'static) {
        unsafe extern "C" fn trampoline(ctx: *mut c_void) {
            // SAFETY: `ctx` points to the boxed callback owned by `self`,
            // which is only replaced or dropped after mpv stops using it.
            unsafe { (*(ctx as *const UpdateCallback))() }
        }

        let boxed: Box<UpdateCallback> = Box::new(Box::new(callback));
        let raw = &*boxed as *const UpdateCallback as *mut c_void;
        // SAFETY: `self.ctx` is valid. mpv switches to the new callback
        // before this returns, so the old one can be dropped afterwards.
        unsafe { sys::mpv_render_context_set_update_callback(self.ctx, Some(trampoline), raw) };
        self.update_callback = Some(boxed);
    }

    /// Lets mpv process pending updates. Returns true if a new video frame
    /// is ready to be rendered.
    pub fn update(&self) -> bool {
        // SAFETY: `self.ctx` is valid and we are on the GL thread.
        let flags = unsafe { sys::mpv_render_context_update(self.ctx) };
        flags & sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64 != 0
    }

    /// Draws the current video frame into framebuffer `fbo` (0 for the
    /// default framebuffer) of `width` x `height` pixels. Set `flip_y` when
    /// rendering to the default framebuffer.
    pub fn render(
        &self,
        fbo: i32,
        width: i32,
        height: i32,
        flip_y: bool,
    ) -> Result<(), RenderError> {
        let mut target = sys::mpv_opengl_fbo {
            fbo,
            w: width,
            h: height,
            internal_format: 0,
        };
        let mut flip: c_int = flip_y.into();
        // By default mpv sleeps inside this call until the frame's display
        // time. That stalls our UI thread for most of each video frame, so
        // window moves and input lag while playing. Slint presents the frame
        // right away instead, and report_swap keeps mpv's timing informed.
        let mut block: c_int = 0;
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: &mut target as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: &mut flip as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                data: &mut block as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        // SAFETY: `self.ctx` is valid, we are on the GL thread with the
        // context current, and `params` outlives the call.
        let err = unsafe { sys::mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
        if err < 0 {
            Err(RenderError(err))
        } else {
            Ok(())
        }
    }

    /// Tells mpv that the frame was presented, which helps frame timing.
    pub fn report_swap(&self) {
        // SAFETY: `self.ctx` is valid.
        unsafe { sys::mpv_render_context_report_swap(self.ctx) };
    }
}

impl Drop for GlRenderContext {
    fn drop(&mut self) {
        // SAFETY: `self.ctx` is valid and freed exactly once. Freeing also
        // unregisters the update callback, so it can be dropped after this.
        // The mpv core (`_mpv`) is dropped after this body runs.
        unsafe { sys::mpv_render_context_free(self.ctx) };
    }
}

impl From<RenderError> for PlayerError {
    fn from(err: RenderError) -> Self {
        PlayerError(err.to_string())
    }
}

impl VideoRenderer for GlRenderContext {
    fn set_update_callback(&mut self, callback: Box<dyn Fn() + Send>) {
        GlRenderContext::set_update_callback(self, callback);
    }

    fn render(&self, fbo: i32, width: i32, height: i32) -> crate::Result<()> {
        // mpv draws bottom-up for both the window and texture FBOs (as the
        // render spike's self-test showed), so always flip.
        self.update();
        Ok(GlRenderContext::render(self, fbo, width, height, true)?)
    }

    fn report_swap(&self) {
        GlRenderContext::report_swap(self);
    }
}
