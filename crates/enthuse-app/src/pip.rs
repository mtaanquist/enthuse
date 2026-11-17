//! Picture-in-picture: a separate borderless, always-on-top window that
//! shows the same player's video. Wayland may ignore always-on-top; the
//! window still works there, just not on top.

use std::cell::Cell;
use std::rc::Rc;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::ResizeDirection;
use slint::{CloseRequestResponse, ComponentHandle, Weak};

use crate::video::VideoOutput;
use crate::{AppWindow, PipWindow};

/// The main window's id in the [`VideoOutput`] (it is attached first).
pub const MAIN_WINDOW: usize = 0;

pub struct Pip {
    main: Weak<AppWindow>,
    window: PipWindow,
    output: VideoOutput,
    id: usize,
    active: Cell<bool>,
}

impl Pip {
    pub fn new(
        main: &AppWindow,
        output: VideoOutput,
    ) -> Result<Rc<Self>, Box<dyn std::error::Error>> {
        let window = PipWindow::new()?;
        let id = output.attach(&window, None)?;
        let this = Rc::new(Self {
            main: main.as_weak(),
            window,
            output,
            id,
            active: Cell::new(false),
        });

        // Moving and resizing go through the window system, which also works
        // on Wayland where apps cannot place their own windows.
        let w = this.window.as_weak();
        this.window.on_start_move(move || {
            if let Some(w) = w.upgrade() {
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_window();
                });
            }
        });
        let w = this.window.as_weak();
        this.window.on_start_resize(move || {
            if let Some(w) = w.upgrade() {
                w.window().with_winit_window(|ww| {
                    let _ = ww.drag_resize_window(ResizeDirection::SouthEast);
                });
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.on_exit_pip(move || {
            if let Some(pip) = weak.upgrade() {
                pip.exit();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.window().on_close_requested(move || {
            if let Some(pip) = weak.upgrade() {
                pip.exit();
            }
            // Hidden once the video is back in the main window.
            CloseRequestResponse::KeepWindowShown
        });
        // Closing the main window quits, even while PiP is open.
        main.window().on_close_requested(|| {
            let _ = slint::quit_event_loop();
            CloseRequestResponse::HideWindow
        });
        let weak = Rc::downgrade(&this);
        main.on_toggle_pip(move || {
            if let Some(pip) = weak.upgrade() {
                if pip.active.get() {
                    pip.exit();
                } else {
                    pip.enter();
                }
            }
        });
        Ok(this)
    }

    /// Shows the PiP window and moves the video there.
    pub fn enter(&self) {
        if self.active.replace(true) {
            return;
        }
        if let Some(m) = self.main.upgrade() {
            m.set_pip_active(true);
        }
        if let Err(err) = self.window.show() {
            eprintln!("enthuse: cannot open picture-in-picture: {err}");
            self.active.set(false);
            if let Some(m) = self.main.upgrade() {
                m.set_pip_active(false);
            }
            return;
        }
        self.output.move_to(self.id, || {});
    }

    /// Moves the video back to the main window, then hides the PiP window.
    pub fn exit(&self) {
        if !self.active.replace(false) {
            return;
        }
        if let Some(m) = self.main.upgrade() {
            m.set_pip_active(false);
            // The video only moves back once the main window draws, which a
            // minimized window does not do.
            if m.window().is_minimized() {
                m.window().set_minimized(false);
            }
        }
        let pip = self.window.as_weak();
        self.output.move_to(MAIN_WINDOW, move || {
            if let Some(p) = pip.upgrade() {
                let _ = p.hide();
            }
        });
    }
}
