//! Picture-in-picture: the main window turns into a small borderless,
//! always-on-top player and back. The video stays in the same window, so
//! mpv's render context never has to move (that stalls playback, see #52).
//! Wayland may ignore always-on-top and does not let apps place windows;
//! PiP still works there, just not on top and not moved to a corner.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize, Timer, TimerMode, Weak};

use crate::AppWindow;
use crate::pip_geometry::{Edges, Rect, fit_aspect, resize};
use crate::pointer;

/// PiP size when entering, in logical pixels; the video's ratio is fitted
/// into it.
const PIP_SIZE: (f32, f32) = (480.0, 270.0);
/// Smallest PiP size, in logical pixels. Keep in step with app.slint.
const PIP_MIN: (f32, f32) = (192.0, 108.0);
/// Gap between the PiP window and the screen edges, in logical pixels.
const PIP_MARGIN: f32 = 24.0;
/// How often a move or resize follows the pointer.
const DRAG_TICK: Duration = Duration::from_millis(8);

/// The main window's geometry before PiP, restored on exit.
#[derive(Clone, Copy)]
struct Saved {
    size: PhysicalSize,
    position: PhysicalPosition,
    maximized: bool,
    fullscreen: bool,
}

pub struct Pip {
    main: Weak<AppWindow>,
    saved: Cell<Option<Saved>>,
    /// Follows the pointer during a move or resize we do ourselves.
    drag: RefCell<Option<Timer>>,
}

impl Pip {
    pub fn new(main: &AppWindow) -> Rc<Self> {
        let this = Rc::new(Self {
            main: main.as_weak(),
            saved: Cell::new(None),
            drag: RefCell::new(None),
        });

        let weak = Rc::downgrade(&this);
        main.on_pip_start_move(move |x, y| {
            if let Some(pip) = weak.upgrade() {
                pip.start_drag(None, x, y);
            }
        });
        let weak = Rc::downgrade(&this);
        main.on_pip_start_resize(move |direction, x, y| {
            if let Some(pip) = weak.upgrade() {
                pip.start_drag(Some(direction), x, y);
            }
        });
        let weak = Rc::downgrade(&this);
        main.on_video_aspect_changed(move || {
            if let Some(pip) = weak.upgrade() {
                pip.fit_video();
            }
        });
        let weak = Rc::downgrade(&this);
        main.on_toggle_pip(move || {
            if let Some(pip) = weak.upgrade() {
                if pip.saved.get().is_some() {
                    pip.exit();
                } else {
                    pip.enter();
                }
            }
        });
        this
    }

    /// Turns the main window into the PiP player.
    pub fn enter(&self) {
        let Some(m) = self.main.upgrade() else {
            return;
        };
        if self.saved.get().is_some() {
            return;
        }
        let window = m.window();
        let saved = Saved {
            size: window.size(),
            position: window.position(),
            maximized: window.is_maximized(),
            fullscreen: window.is_fullscreen(),
        };
        self.saved.set(Some(saved));
        if saved.fullscreen {
            window.set_fullscreen(false);
        }
        if saved.maximized {
            window.set_maximized(false);
        }
        m.set_pip_active(true);

        let scale = window.scale_factor();
        let mut rect = Rect {
            x: 0,
            y: 0,
            w: (PIP_SIZE.0 * scale).round() as i32,
            h: (PIP_SIZE.1 * scale).round() as i32,
        };
        if let Some(a) = aspect(&m) {
            rect = fit_aspect(rect, a, None);
        }
        let placed = !is_wayland(&m)
            && monitor(&m).is_some_and(|screen| {
                let margin = (PIP_MARGIN * scale) as i32;
                rect.x = screen.x + screen.w - rect.w - margin;
                rect.y = screen.y + screen.h - rect.h - margin;
                true
            });
        if placed {
            set_rect(&m, rect);
        } else {
            window.set_size(PhysicalSize::new(rect.w as u32, rect.h as u32));
        }
    }

    /// Restores the main window as it was before PiP.
    pub fn exit(&self) {
        let Some(saved) = self.saved.take() else {
            return;
        };
        self.drag.borrow_mut().take();
        let Some(m) = self.main.upgrade() else {
            return;
        };
        let window = m.window();
        m.set_pip_active(false);
        window.set_size(saved.size);
        window.set_position(saved.position);
        if saved.maximized {
            window.set_maximized(true);
        }
        if saved.fullscreen {
            window.set_fullscreen(true);
        }
    }

    /// Fits the PiP window to a new video's aspect ratio, never taller.
    fn fit_video(&self) {
        if self.saved.get().is_none() || self.drag.borrow().is_some() {
            return;
        }
        let Some(m) = self.main.upgrade() else {
            return;
        };
        let Some(a) = aspect(&m) else {
            return;
        };
        let current = window_rect(&m);
        let fitted = fit_aspect(current, a, monitor(&m));
        if fitted != current {
            set_rect(&m, fitted);
        }
    }

    /// Moves (`direction` `None`) or resizes the window with the pointer,
    /// pressed at (`x`, `y`) in logical window coordinates.
    /// We follow the pointer ourselves where we can read it: the OS move
    /// and resize loop on Windows blocks our event loop, which made the
    /// video stutter, and our own resize can keep the aspect ratio.
    fn start_drag(self: &Rc<Self>, direction: Option<i32>, x: f32, y: f32) {
        let Some(m) = self.main.upgrade() else {
            return;
        };
        let readable = !is_wayland(&m) && pointer::read().is_some_and(|p| p.left_down);
        if !readable {
            os_drag(&m, direction);
            return;
        }
        let start = window_rect(&m);
        let scale = m.window().scale_factor();
        // Where the press happened on screen. The pointer may have moved on
        // by the time this runs, and the window should follow from the
        // press, not from here.
        let start_pointer = (
            start.x + (x * scale).round() as i32,
            start.y + (y * scale).round() as i32,
        );
        let edges = direction.map(Edges::from_direction);
        let min = (
            (PIP_MIN.0 * scale).round() as i32,
            (PIP_MIN.1 * scale).round() as i32,
        );
        let ratio = aspect(&m);
        let window = m.as_weak();
        let this = Rc::downgrade(self);
        let mut last = start;
        let mut done = false;
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, DRAG_TICK, move || {
            if done {
                return;
            }
            let (Some(w), Some(p)) = (window.upgrade(), pointer::read()) else {
                return;
            };
            if !p.left_down {
                done = true;
                // Drop the timer outside its own callback.
                let this = this.clone();
                Timer::single_shot(Duration::ZERO, move || {
                    if let Some(pip) = this.upgrade() {
                        pip.drag.borrow_mut().take();
                    }
                });
                return;
            }
            let (dx, dy) = (p.x - start_pointer.0, p.y - start_pointer.1);
            let rect = match edges {
                None => Rect {
                    x: start.x + dx,
                    y: start.y + dy,
                    ..start
                },
                Some(e) => resize(start, e, dx, dy, ratio, min.0, min.1),
            };
            if rect != last {
                set_rect(&w, rect);
                last = rect;
            }
        });
        *self.drag.borrow_mut() = Some(timer);
    }
}

/// The video's aspect ratio, if known.
fn aspect(window: &AppWindow) -> Option<f64> {
    let a = window.get_video_aspect() as f64;
    (a.is_finite() && a > 0.0).then_some(a)
}

fn window_rect(window: &AppWindow) -> Rect {
    let w = window.window();
    let (pos, size) = (w.position(), w.size());
    Rect {
        x: pos.x,
        y: pos.y,
        w: size.width as i32,
        h: size.height as i32,
    }
}

/// Moves and resizes the window in one step where the platform allows it,
/// so dragging the top or left edge does not wobble.
fn set_rect(window: &AppWindow, rect: Rect) {
    #[cfg(target_os = "windows")]
    if win32::set_rect(window, rect) {
        return;
    }
    let w = window.window();
    w.set_position(PhysicalPosition::new(rect.x, rect.y));
    w.set_size(PhysicalSize::new(
        rect.w.max(1) as u32,
        rect.h.max(1) as u32,
    ));
}

/// The monitor the window is on, in physical pixels.
fn monitor(window: &AppWindow) -> Option<Rect> {
    let monitor = window
        .window()
        .with_winit_window(|ww| ww.current_monitor())
        .flatten()?;
    let (pos, size) = (monitor.position(), monitor.size());
    Some(Rect {
        x: pos.x,
        y: pos.y,
        w: size.width as i32,
        h: size.height as i32,
    })
}

/// Moves or resizes through the window system, where we cannot follow the
/// pointer ourselves (Wayland).
fn os_drag(window: &AppWindow, direction: Option<i32>) {
    window.window().with_winit_window(|ww| {
        let _ = match direction {
            None => ww.drag_window(),
            Some(d) => ww.drag_resize_window(resize_direction(d)),
        };
    });
    release_pointer(window);
}

/// The window system takes over the pointer for a move or resize and eats
/// the button release, so Slint would keep the pressed area grabbed and send
/// it the next press too (e.g. every later resize went to the first edge).
/// Release it ourselves, outside the window so no click is registered.
fn release_pointer(window: &AppWindow) {
    let w = window.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(w) = w.upgrade() {
            let position = slint::LogicalPosition::new(-1.0, -1.0);
            w.window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                    position,
                    button: slint::platform::PointerEventButton::Left,
                });
            w.window()
                .dispatch_event(slint::platform::WindowEvent::PointerExited);
        }
    });
}

/// Wayland does not let apps place their windows or read the pointer's
/// screen position, so moving must go through the compositor there.
fn is_wayland(window: &AppWindow) -> bool {
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window
        .window()
        .with_winit_window(|ww| {
            matches!(
                ww.window_handle().map(|h| h.as_raw()),
                Ok(RawWindowHandle::Wayland(_))
            )
        })
        .unwrap_or(false)
}

/// Maps the direction from the UI (0 = north, clockwise to 7 = north-west).
fn resize_direction(direction: i32) -> ResizeDirection {
    match direction.rem_euclid(8) {
        0 => ResizeDirection::North,
        1 => ResizeDirection::NorthEast,
        2 => ResizeDirection::East,
        3 => ResizeDirection::SouthEast,
        4 => ResizeDirection::South,
        5 => ResizeDirection::SouthWest,
        6 => ResizeDirection::West,
        _ => ResizeDirection::NorthWest,
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
    };

    use crate::AppWindow;
    use crate::pip_geometry::Rect;

    /// Moves and resizes with one `SetWindowPos`. Returns false if the
    /// window handle is not available.
    pub fn set_rect(window: &AppWindow, rect: Rect) -> bool {
        window
            .window()
            .with_winit_window(|ww| {
                let Ok(handle) = ww.window_handle() else {
                    return false;
                };
                let RawWindowHandle::Win32(h) = handle.as_raw() else {
                    return false;
                };
                // SAFETY: the handle belongs to our live window, and
                // SetWindowPos is called on its own UI thread.
                unsafe {
                    SetWindowPos(
                        h.hwnd.get() as _,
                        std::ptr::null_mut(),
                        rect.x,
                        rect.y,
                        rect.w,
                        rect.h,
                        SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
                    ) != 0
                }
            })
            .unwrap_or(false)
    }
}
