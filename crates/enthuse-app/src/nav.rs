//! Moves between the browser and the player, keeps resume positions and
//! watched state up to date, and plays the next file in the folder when one
//! ends (auto-next).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::next::next_video;
use enthuse_core::resume::{SaveSchedule, start_position};
use enthuse_core::source::SourcePath;
use enthuse_core::storage::Store;
use enthuse_player::{EndReason, Player, PlayerEvent};
use slint::{ComponentHandle, Weak};

use crate::browser::{Browser, SourceRef};
use crate::playback::Playback;
use crate::{AppWindow, bg};

pub const PAGE_BROWSER: i32 = 0;
pub const PAGE_PLAYER: i32 = 1;

/// The file playing now, when it came from a source (not a bare URL).
struct Current {
    source: SourceRef,
    path: SourcePath,
    duration: Option<f64>,
    schedule: SaveSchedule,
}

pub struct Nav {
    window: Weak<AppWindow>,
    player: Arc<dyn Player>,
    playback: Rc<Playback>,
    browser: Rc<Browser>,
    store: Rc<Store>,
    current: RefCell<Option<Current>>,
    on_leave_player: RefCell<Option<Box<dyn Fn()>>>,
    /// Bumped whenever the user or auto-next starts or stops a file, so a
    /// slow next-file lookup does not override what happened meanwhile.
    generation: Cell<u64>,
}

impl Nav {
    pub fn new(
        window: &AppWindow,
        player: Arc<dyn Player>,
        playback: Rc<Playback>,
        browser: Rc<Browser>,
        store: Rc<Store>,
    ) -> Rc<Self> {
        let this = Rc::new(Self {
            window: window.as_weak(),
            player,
            playback,
            browser,
            store,
            current: RefCell::new(None),
            on_leave_player: RefCell::new(None),
            generation: Cell::new(0),
        });

        let weak = Rc::downgrade(&this);
        this.playback.set_listener(Box::new(move |event| {
            if let Some(nav) = weak.upgrade() {
                nav.on_event(event);
            }
        }));
        let weak = Rc::downgrade(&this);
        this.browser.set_play_handler(Box::new(move |source, path| {
            if let Some(nav) = weak.upgrade() {
                nav.play(source, path);
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

    /// Plays `path` from `source`, resuming where it was left off.
    pub fn play(&self, source: SourceRef, path: SourcePath) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        self.save_position();
        self.generation.set(self.generation.get() + 1);
        match source.source.playable(&path) {
            Ok(url) => {
                let state = self.store.watch_state(source.id, &path).ok().flatten();
                let start = start_position(state.as_ref());
                self.playback.open(&url, path.name().unwrap_or(&url), start);
                *self.current.borrow_mut() = Some(Current {
                    source,
                    path,
                    duration: state.and_then(|s| s.duration),
                    schedule: SaveSchedule::new(start),
                });
                w.set_page(PAGE_PLAYER);
            }
            Err(err) => {
                self.show_browser();
                w.set_browser_error(format!("Cannot play this file: {err}").into());
            }
        }
    }

    /// Runs when playback stops and the browser shows, e.g. to close PiP.
    pub fn set_on_leave_player(&self, f: Box<dyn Fn()>) {
        *self.on_leave_player.borrow_mut() = Some(f);
    }

    /// Saves the position, stops playback and returns to the browser.
    pub fn show_browser(&self) {
        self.save_position();
        if let Some(f) = self.on_leave_player.borrow().as_ref() {
            f();
        }
        self.generation.set(self.generation.get() + 1);
        *self.current.borrow_mut() = None;
        let _ = self.player.stop();
        if let Some(w) = self.window.upgrade() {
            w.set_page(PAGE_BROWSER);
        }
        // Update the watched markers.
        self.browser.refresh();
    }

    /// Saves the position of the current file, if any. Call before exiting.
    pub fn save_position(&self) {
        let Some(position) = self.player.position() else {
            return;
        };
        if let Some(current) = self.current.borrow_mut().as_mut() {
            current.schedule.saved(position);
            self.store_position(current, position);
        }
    }

    fn store_position(&self, current: &Current, position: f64) {
        if let Err(err) =
            self.store
                .save_position(current.source.id, &current.path, position, current.duration)
        {
            eprintln!("enthuse: cannot save position: {err}");
        }
    }

    fn on_event(self: &Rc<Self>, event: &PlayerEvent) {
        match event {
            PlayerEvent::DurationChanged(Some(d)) => {
                if let Some(current) = self.current.borrow_mut().as_mut() {
                    current.duration = Some(*d);
                }
            }
            PlayerEvent::PositionChanged(Some(p)) => {
                let mut current = self.current.borrow_mut();
                if let Some(current) = current.as_mut()
                    && current.schedule.due(*p)
                {
                    self.store_position(current, *p);
                }
            }
            PlayerEvent::PauseChanged(true) => self.save_position(),
            PlayerEvent::Ended(EndReason::EndOfFile) => self.finished(),
            _ => {}
        }
    }

    /// The current file played to its end: mark it watched and go on to the
    /// next one in its folder.
    fn finished(self: &Rc<Self>) {
        let Some(current) = self.current.borrow_mut().take() else {
            self.show_browser();
            return;
        };
        if let Err(err) = self
            .store
            .set_watched(current.source.id, &current.path, true)
        {
            eprintln!("enthuse: cannot mark watched: {err}");
        }
        self.play_next(current.source, current.path);
    }

    /// Opens the next video after `path` in its folder, or returns to the
    /// browser at the end of the folder. The folder is listed again off the
    /// UI thread, so files added or removed meanwhile are handled.
    fn play_next(self: &Rc<Self>, source: SourceRef, path: SourcePath) {
        let this = Rc::downgrade(self);
        let generation = self.generation.get();
        let src = source.source.clone();
        bg::spawn(
            move || next_video(&*src, &path),
            move |next| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if this.generation.get() != generation {
                    return; // The user moved on while the folder was listed.
                }
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
