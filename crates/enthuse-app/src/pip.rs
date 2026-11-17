//! Picture-in-picture: the main window turns into a small borderless,
//! always-on-top player and back. The video stays in the same window, so
//! mpv's render context never has to move (that stalls playback, see #52).
//! Wayland may ignore always-on-top and does not let apps place windows;
//! PiP still works there, just not on top and not moved to a corner.

use std::cell::Cell;
use std::rc::Rc;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;
use slint::{ComponentHandle, LogicalSize, PhysicalPosition, PhysicalSize, Weak};

use crate::AppWindow;

/// PiP size when entering, in logical pixels (16:9).
const PIP_SIZE: LogicalSize = LogicalSize {
    width: 480.0,
    height: 270.0,
};
/// Gap between the PiP window and the screen edges, in logical pixels.
const PIP_MARGIN: f32 = 24.0;

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
}

impl Pip {
    pub fn new(main: &AppWindow) -> Rc<Self> {
        let this = Rc::new(Self {
            main: main.as_weak(),
            saved: Cell::new(None),
        });

        // Moving and resizing go through the window system where needed
        // (Wayland does not let apps place windows). Elsewhere the window is
        // moved by us: the OS move loop on Windows blocks the event loop,
        // which made the video stutter while dragging.
        let w = main.as_weak();
        main.on_pip_start_move(move || {
            if let Some(w) = w.upgrade() {
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_window();
                });
                release_pointer(&w);
            }
        });
        let w = main.as_weak();
        main.on_pip_move_by(move |dx, dy| {
            if let Some(w) = w.upgrade() {
                let window = w.window();
                let scale = window.scale_factor();
                let pos = window.position();
                window.set_position(PhysicalPosition::new(
                    pos.x + (dx * scale).round() as i32,
                    pos.y + (dy * scale).round() as i32,
                ));
            }
        });
        let w = main.as_weak();
        main.on_pip_start_resize(move |direction| {
            if let Some(w) = w.upgrade() {
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_resize_window(resize_direction(direction));
                });
                release_pointer(&w);
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
        let wayland = is_wayland(&m);
        m.set_pip_os_move(wayland);
        m.set_pip_active(true);
        window.set_size(PIP_SIZE);
        if !wayland && let Some(pos) = corner_position(&m) {
            window.set_position(pos);
        }
    }

    /// Restores the main window as it was before PiP.
    pub fn exit(&self) {
        let Some(saved) = self.saved.take() else {
            return;
        };
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
}

/// Bottom-right corner of the monitor the window is on, for a window of
/// [`PIP_SIZE`].
fn corner_position(window: &AppWindow) -> Option<PhysicalPosition> {
    let scale = window.window().scale_factor();
    let monitor = window
        .window()
        .with_winit_window(|ww| ww.current_monitor())
        .flatten()?;
    let (origin, size) = (monitor.position(), monitor.size());
    let w = (PIP_SIZE.width * scale) as i32;
    let h = (PIP_SIZE.height * scale) as i32;
    let margin = (PIP_MARGIN * scale) as i32;
    Some(PhysicalPosition::new(
        origin.x + size.width as i32 - w - margin,
        origin.y + size.height as i32 - h - margin,
    ))
}

/// The window system takes over the pointer for a move or resize and eats
/// the button release, so Slint would keep the pressed area grabbed and send
/// it the next press too (e.g. every later resize went to the first edge).
/// Release it ourselves, outside the window so no click is registered.
fn release_pointer(window: &AppWindow) {
    let w = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
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

/// Wayland does not let apps place their windows, so moving must go
/// through the compositor there.
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
