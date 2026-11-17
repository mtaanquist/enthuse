//! Moves between the browser and the player, and plays the next file in the
//! folder when one ends (auto-next).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::next::next_video;
use enthuse_core::source::{Source, SourcePath};
use enthuse_player::Player;
use slint::{ComponentHandle, Weak};

use crate::playback::Playback;
use crate::{AppWindow, bg};

pub const PAGE_BROWSER: i32 = 0;
pub const PAGE_PLAYER: i32 = 1;

pub struct Nav {
    window: Weak<AppWindow>,
    player: Arc<dyn Player>,
    playback: Rc<Playback>,
    /// The file playing now, if it came from a source (not a bare URL).
    current: RefCell<Option<(Arc<dyn Source>, SourcePath)>>,
}

impl Nav {
    pub fn new(window: &AppWindow, player: Arc<dyn Player>, playback: Rc<Playback>) -> Rc<Self> {
        let this = Rc::new(Self {
            window: window.as_weak(),
            player,
            playback,
            current: RefCell::new(None),
        });
        let weak = Rc::downgrade(&this);
        this.playback.set_end_of_file_handler(Box::new(move || {
            if let Some(nav) = weak.upgrade() {
                nav.play_next();
            }
        }));
        let weak = Rc::downgrade(&this);
        window.on_back(move || {
            if let Some(nav) = weak.upgrade() {
                nav.show_browser();
            }
        });
        this
    }

    /// Plays `path` from `source` on the player page.
    pub fn play(&self, source: Arc<dyn Source>, path: SourcePath) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        match source.playable(&path) {
            Ok(url) => {
                self.playback.open(&url, path.name().unwrap_or(&url), None);
                *self.current.borrow_mut() = Some((source, path));
                w.set_page(PAGE_PLAYER);
            }
            Err(err) => {
                self.show_browser();
                w.set_browser_error(format!("Cannot play this file: {err}").into());
            }
        }
    }

    /// Stops playback and returns to the folder browser.
    pub fn show_browser(&self) {
        let _ = self.player.stop();
        *self.current.borrow_mut() = None;
        if let Some(w) = self.window.upgrade() {
            w.set_page(PAGE_BROWSER);
        }
    }

    /// Opens the next video in the current file's folder, or returns to the
    /// browser at the end of the folder. The folder is listed again off the
    /// UI thread, so files added or removed meanwhile are handled.
    fn play_next(self: &Rc<Self>) {
        let Some((source, path)) = self.current.borrow().clone() else {
            self.show_browser();
            return;
        };
        let this = Rc::downgrade(self);
        let src = source.clone();
        bg::spawn(
            move || next_video(&*src, &path),
            move |next| {
                let Some(this) = this.upgrade() else { return };
                match next {
                    Ok(Some(next)) => this.play(source, next),
                    Ok(None) => this.show_browser(),
                    Err(err) => {
                        this.show_browser();
                        if let Some(w) = this.window.upgrade() {
                            w.set_browser_error(format!("Cannot find the next file: {err}").into());
                        }
                    }
                }
            },
        );
    }
}
