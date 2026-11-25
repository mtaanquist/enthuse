//! Picture-in-picture: the main window turns into a small borderless,
//! always-on-top player and back. The video stays in the same window, so
//! mpv's render context never has to move (that stalls playback, see #52).
//! Meanwhile the browser opens in a window of its own where the main
//! window was, so it looks as if only the video popped out.
//! Wayland may ignore always-on-top and does not let apps place windows;
//! PiP still works there, just not on top and not moved to a corner.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use enthuse_core::pip::{Edges, Orientation, Rect, anchor, fit_area, resize, user_resized};
use enthuse_core::storage::Store;
use enthuse_player::Player;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;
use slint::{
    CloseRequestResponse, ComponentHandle, PhysicalPosition, PhysicalSize, Timer, TimerMode, Weak,
};

use crate::browser::{Browser, View};
use crate::pointer;
use crate::{AppWindow, BrowserModel, BrowserWindow, KeyFocus, PlayerModel};

/// PiP size when entering until the user picks one, in logical pixels; a
/// video of another shape gets the same area.
const PIP_SIZE: (f32, f32) = (560.0, 315.0);
/// Smallest PiP size, in logical pixels. Keep in step with app.slint.
const PIP_MIN: (f32, f32) = (192.0, 108.0);
/// Gap between the PiP window and the screen edges, in logical pixels.
const PIP_MARGIN: f32 = 48.0;
/// How often a move or resize follows the pointer.
const DRAG_TICK: Duration = Duration::from_millis(8);
/// Subtitle scale in PiP, so they stay readable in the small window.
const PIP_SUB_SCALE: &str = "1.7";

/// A window's geometry, to hand over between the main and browser windows.
#[derive(Clone, Copy)]
struct Saved {
    size: PhysicalSize,
    position: PhysicalPosition,
    maximized: bool,
    fullscreen: bool,
}

pub struct Pip {
    main: Weak<AppWindow>,
    /// Shows the browser while the main window is the PiP player.
    side: BrowserWindow,
    browser: Rc<Browser>,
    player: Arc<dyn Player>,
    /// Remembers the PiP size the user chose per orientation.
    store: Rc<Store>,
    /// The aspect ratio the window was last fitted to.
    shown_aspect: Cell<Option<f64>>,
    /// The window size we last set or saw the user choose. A different
    /// size later means the user resized it some other way (the window
    /// system on Wayland, Snap, keyboard), which is remembered too.
    expected: Cell<Option<(i32, i32)>>,
    /// Set while PiP is on.
    saved: Cell<Option<Saved>>,
    /// Follows the pointer during a move or resize we do ourselves.
    drag: RefCell<Option<Timer>>,
}

impl Pip {
    pub fn new(
        main: &AppWindow,
        browser: Rc<Browser>,
        player: Arc<dyn Player>,
        store: Rc<Store>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let side = BrowserWindow::new()?;
        {
            let weak = side.as_weak();
            side.global::<KeyFocus>().on_restore(move || {
                if let Some(w) = weak.upgrade() {
                    w.invoke_focus_keys();
                }
            });
        }
        browser.attach_side(&side);
        // The browser window stands in for the main window: closing it quits.
        side.window().on_close_requested(|| {
            let _ = slint::quit_event_loop();
            CloseRequestResponse::HideWindow
        });
        let this = Rc::new(Self {
            main: main.as_weak(),
            side,
            browser,
            player,
            store,
            shown_aspect: Cell::new(None),
            expected: Cell::new(None),
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
        // The browser window's now-playing bar: back to the video, or stop
        // as the player's own Back does.
        let weak = Rc::downgrade(&this);
        this.side.on_show_video(move || {
            if let Some(pip) = weak.upgrade() {
                pip.exit();
            }
        });
        let main_weak = main.as_weak();
        this.side.on_stop(move || {
            if let Some(m) = main_weak.upgrade() {
                m.invoke_back();
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
        Ok(this)
    }

    /// The browser window shown while PiP is on.
    /// Whether the main window is the PiP player now.
    pub fn is_active(&self) -> bool {
        self.saved.get().is_some()
    }

    pub fn side(&self) -> &BrowserWindow {
        &self.side
    }

    /// Turns the main window into the PiP player, and opens the browser
    /// in its own window where the main window was.
    pub fn enter(&self) {
        let Some(m) = self.main.upgrade() else {
            return;
        };
        if self.saved.get().is_some() {
            return;
        }
        let window = m.window();
        let saved = geometry(window);
        self.saved.set(Some(saved));

        // The browser window takes the main window's place. Place it again
        // once shown: window managers may place a new window themselves.
        let side = self.side.window();
        side.set_size(saved.size);
        side.set_position(saved.position);
        // The browser window shows the section the main window was on.
        self.side.set_section(m.get_section());
        self.side
            .global::<BrowserModel>()
            .set_view_mode(m.global::<BrowserModel>().get_view_mode());
        self.browser.set_view(View::Side(self.side.as_weak()));
        if let Err(err) = self.side.show() {
            eprintln!("enthuse: cannot open the browser window: {err}");
            self.browser.set_view(View::Main(m.as_weak()));
        } else if saved.maximized || saved.fullscreen {
            side.set_maximized(true);
        } else {
            side.set_position(saved.position);
            let weak = self.side.as_weak();
            Timer::single_shot(Duration::from_millis(50), move || {
                if let Some(w) = weak.upgrade() {
                    w.window().set_position(saved.position);
                }
            });
        }

        if saved.fullscreen {
            window.set_fullscreen(false);
        }
        if saved.maximized {
            window.set_maximized(false);
        }
        m.set_pip_active(true);
        let _ = self.player.set_option("sub-scale", PIP_SUB_SCALE);
        #[cfg(target_os = "windows")]
        win32::set_rounded(&m, true);

        let scale = window.scale_factor();
        let screen = monitor(&m);
        let a = aspect(&m);
        let (w, h) = self.size_for(
            (
                (PIP_SIZE.0 * scale).round() as i32,
                (PIP_SIZE.1 * scale).round() as i32,
            ),
            a,
            scale,
            screen,
        );
        self.shown_aspect.set(a);
        self.expected.set(Some((w, h)));
        let mut rect = Rect { x: 0, y: 0, w, h };
        let placed = !is_wayland(&m)
            && screen.is_some_and(|screen| {
                let margin = margin(scale);
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

    /// Turns the PiP player back into the main window, where the browser
    /// window is now, and closes the browser window.
    pub fn exit(&self) {
        let Some(saved) = self.saved.take() else {
            return;
        };
        self.drag.borrow_mut().take();
        let Some(m) = self.main.upgrade() else {
            return;
        };
        self.remember_user_resize(&m);
        self.expected.set(None);
        // Take the browser window's place, as the user left it.
        let target = if self.side.window().is_visible() {
            let mut g = geometry(self.side.window());
            g.fullscreen = saved.fullscreen;
            if g.maximized {
                // Its restored size and position are not known here.
                g.size = saved.size;
                g.position = saved.position;
            }
            g
        } else {
            saved
        };
        let window = m.window();
        m.set_section(self.side.get_section());
        m.global::<BrowserModel>()
            .set_view_mode(self.side.global::<BrowserModel>().get_view_mode());
        m.set_pip_active(false);
        let _ = self.player.set_option("sub-scale", "1");
        #[cfg(target_os = "windows")]
        win32::set_rounded(&m, false);
        window.set_size(target.size);
        window.set_position(target.position);
        if target.maximized {
            window.set_maximized(true);
        }
        if target.fullscreen {
            window.set_fullscreen(true);
        }
        self.browser.set_view(View::Main(m.as_weak()));
        let _ = self.side.hide();
        // Show the player's controls, which also moves the subtitles above
        // them again.
        m.global::<PlayerModel>().invoke_activity();
    }

    /// Fits the PiP window to a new video's aspect ratio: the size the user
    /// chose for its orientation, else the same area as now (#96).
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
        self.remember_user_resize(&m);
        let current = window_rect(&m);
        let screen = monitor(&m);
        let size = self.size_for(
            (current.w, current.h),
            Some(a),
            m.window().scale_factor(),
            screen,
        );
        self.shown_aspect.set(Some(a));
        self.expected.set(Some(size));
        let fitted = anchor(current, size, screen);
        if fitted != current {
            set_rect(&m, fitted);
        }
    }

    /// The PiP size in physical pixels for a video with `aspect`: the size
    /// the user chose for its orientation, else the area of `like`, in the
    /// video's shape and within the screen.
    fn size_for(
        &self,
        like: (i32, i32),
        aspect: Option<f64>,
        scale: f32,
        screen: Option<Rect>,
    ) -> (i32, i32) {
        let Some(a) = aspect else {
            return like;
        };
        let like = match self.store.pip_size(Orientation::of(a)) {
            Ok(Some((w, h))) => ((w * scale).round() as i32, (h * scale).round() as i32),
            Ok(None) => like,
            Err(err) => {
                eprintln!("enthuse: cannot read the PiP size: {err}");
                like
            }
        };
        let max = screen.map(|s| {
            let margin = 2 * margin(scale);
            (s.w - margin, s.h - margin)
        });
        fit_area(like, a, min_size(scale), max)
    }

    /// Remembers `size` (physical pixels) as the user's PiP size for videos
    /// shaped like `aspect`.
    fn remember(&self, aspect: f64, size: (i32, i32), scale: f32) {
        if size.0 <= 0 || size.1 <= 0 || scale.is_nan() || scale <= 0.0 {
            return;
        }
        let logical = (size.0 as f32 / scale, size.1 as f32 / scale);
        if let Err(err) = self.store.set_pip_size(Orientation::of(aspect), logical) {
            eprintln!("enthuse: cannot save the PiP size: {err}");
        }
    }

    /// If the user resized the window some way other than our own drag,
    /// remembers its size for the video it was showing.
    fn remember_user_resize(&self, m: &AppWindow) {
        let (Some(expected), Some(a)) = (self.expected.get(), self.shown_aspect.get()) else {
            return;
        };
        let r = window_rect(m);
        if user_resized(expected, (r.w, r.h), a) {
            self.remember(a, (r.w, r.h), m.window().scale_factor());
            self.expected.set(Some((r.w, r.h)));
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
        let min = min_size(scale);
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
                let resized = edges.is_some() && last != start;
                let size = (last.w, last.h);
                Timer::single_shot(Duration::ZERO, move || {
                    if let Some(pip) = this.upgrade() {
                        pip.drag.borrow_mut().take();
                        if let (true, Some(a)) = (resized, ratio) {
                            pip.remember(a, size, scale);
                            pip.expected.set(Some(size));
                        }
                        // The video may have changed during the drag.
                        pip.fit_video();
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

fn geometry(window: &slint::Window) -> Saved {
    Saved {
        size: window.size(),
        position: window.position(),
        maximized: window.is_maximized(),
        fullscreen: window.is_fullscreen(),
    }
}

/// Gap between the PiP window and the screen edges, in physical pixels.
fn margin(scale: f32) -> i32 {
    (PIP_MARGIN * scale).round() as i32
}

/// The smallest PiP size in physical pixels.
fn min_size(scale: f32) -> (i32, i32) {
    (
        (PIP_MIN.0 * scale).round() as i32,
        (PIP_MIN.1 * scale).round() as i32,
    )
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
    use slint::ComponentHandle;
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
    };

    use crate::AppWindow;
    use enthuse_core::pip::Rect;

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

    /// Rounds the window's corners (Windows 11), or gives back the
    /// default. Older Windows ignores it and keeps square corners.
    pub fn set_rounded(window: &AppWindow, rounded: bool) {
        window.window().with_winit_window(|ww| {
            let Ok(handle) = ww.window_handle() else {
                return;
            };
            let RawWindowHandle::Win32(h) = handle.as_raw() else {
                return;
            };
            let preference = if rounded {
                DWMWCP_ROUND
            } else {
                DWMWCP_DEFAULT
            };
            // SAFETY: the handle belongs to our live window, and the
            // attribute value is a 4-byte DWM_WINDOW_CORNER_PREFERENCE.
            unsafe {
                DwmSetWindowAttribute(
                    h.hwnd.get() as _,
                    DWMWA_WINDOW_CORNER_PREFERENCE as _,
                    (&raw const preference).cast(),
                    std::mem::size_of_val(&preference) as u32,
                );
            }
        });
    }
}
