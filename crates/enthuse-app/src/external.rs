//! Files and folders from outside the app (#60), dropped on a window or
//! given on the command line. A file plays right away; a folder opens in the
//! browser without becoming a library. Either way, if a library holds it,
//! it opens there.
//!
//! Dropping needs the window system's support: winit 0.30 has it on Windows
//! and X11, not on Wayland, where only the command line works.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use enthuse_core::natsort::natural_cmp;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
use slint::{ComponentHandle, Weak};

use crate::browser::Browser;
use crate::nav::{Nav, PAGE_PLAYER};
use crate::pip::Pip;
use crate::{AppWindow, Section};

pub struct Opener {
    window: Weak<AppWindow>,
    browser: Rc<Browser>,
    nav: Rc<Nav>,
    pip: Rc<Pip>,
    /// Paths of a drop still arriving: winit sends one event per file.
    dropped: RefCell<Vec<PathBuf>>,
}

impl Opener {
    pub fn new(window: &AppWindow, browser: Rc<Browser>, nav: Rc<Nav>, pip: Rc<Pip>) -> Rc<Self> {
        let this = Rc::new(Self {
            window: window.as_weak(),
            browser,
            nav,
            pip,
            dropped: Default::default(),
        });
        // The main window (also when it is the PiP player) and the browser
        // window PiP opens.
        this.watch_drops(window.window());
        this.watch_drops(this.pip.side().window());
        this
    }

    fn watch_drops(self: &Rc<Self>, window: &slint::Window) {
        let weak = Rc::downgrade(self);
        window.on_winit_window_event(move |_, event| {
            if let WindowEvent::DroppedFile(path) = event
                && let Some(this) = weak.upgrade()
            {
                this.dropped(path.clone());
            }
            EventResult::Propagate
        });
    }

    /// Collects the files of one drop, then opens them together once the
    /// events for the rest have been handled.
    fn dropped(self: &Rc<Self>, path: PathBuf) {
        let first = {
            let mut dropped = self.dropped.borrow_mut();
            dropped.push(path);
            dropped.len() == 1
        };
        if first {
            let weak = Rc::downgrade(self);
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(this) = weak.upgrade() {
                    let paths = this.dropped.take();
                    this.open(&paths);
                }
            });
        }
    }

    /// Opens what was dropped or given: the first folder if there is one,
    /// or else the first file by name, so auto-next goes on from there.
    pub fn open(&self, paths: &[PathBuf]) {
        if let Some(folder) = paths.iter().find(|p| p.is_dir()) {
            self.open_folder(folder);
        } else if let Some(file) = first_by_name(paths) {
            self.browser.play_external(file);
        }
    }

    fn open_folder(&self, folder: &Path) {
        if self.pip.is_active() {
            // The video keeps playing; the browser window shows the folder.
            self.pip.side().set_section(Section::Browse);
        } else if let Some(w) = self.window.upgrade() {
            if w.get_page() == PAGE_PLAYER {
                // Like going back: saves the position and stops.
                self.nav.show_browser();
            }
            w.set_section(Section::Browse);
        }
        self.browser.open_external(folder);
    }
}

/// The path whose file name comes first in natural order.
fn first_by_name(paths: &[PathBuf]) -> Option<&PathBuf> {
    let name = |p: &PathBuf| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    paths.iter().min_by(|a, b| natural_cmp(&name(a), &name(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_file_in_natural_order() {
        let paths: Vec<PathBuf> = ["x/Ep 10.mkv", "y/Ep 9.mkv", "z/Ep 11.mkv"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(first_by_name(&paths), Some(&PathBuf::from("y/Ep 9.mkv")));
        assert_eq!(first_by_name(&[]), None);
    }
}
