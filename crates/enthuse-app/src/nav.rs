//! Moves between the browser and the player, keeps resume positions and
//! watched state up to date, and plays the next file in the folder when one
//! ends (auto-next).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::ended::{ended_early, interrupted};
use enthuse_core::folder::{display_title, episode_label};
use enthuse_core::next::next_video;
use enthuse_core::resume::{SaveSchedule, start_position};
use enthuse_core::source::{FileStamp, SourcePath};
use enthuse_core::storage::Store;
use enthuse_core::up_next::{UpNext, UpNextState};
use enthuse_player::{EndReason, Player, PlayerEvent};
use slint::{ComponentHandle, Weak};

use crate::browser::{Browser, SourceRef};
use crate::playback::Playback;
use crate::thumbs::{Thumbs, thumb_id};
use crate::{AppWindow, PlayerModel, bg};

pub const PAGE_BROWSER: i32 = 0;
pub const PAGE_PLAYER: i32 = 1;

/// The file playing now, when it came from a source (not a bare URL).
struct Current {
    source: SourceRef,
    path: SourcePath,
    duration: Option<f64>,
    /// Last position the player reported; it reads as unknown once the
    /// file ends.
    position: f64,
    /// Size and time of the file when it was opened, once looked up, to
    /// tell a share that dropped from a real end of file (#57).
    stamp: Option<FileStamp>,
    /// The file could no longer be read; Retry resumes at `position`.
    lost: bool,
    schedule: SaveSchedule,
    up_next: UpNext,
    /// The player opened the file. An error before that means it could not
    /// be played at all: the browser shows again with a toast.
    loaded: bool,
}

pub struct Nav {
    window: Weak<AppWindow>,
    player: Arc<dyn Player>,
    playback: Rc<Playback>,
    browser: Rc<Browser>,
    store: Rc<Store>,
    current: RefCell<Option<Current>>,
    /// The file after the current one in its folder, once looked up.
    next: RefCell<Option<SourcePath>>,
    on_leave_player: RefCell<Option<Box<dyn Fn()>>>,
    /// Bumped whenever the user or auto-next starts or stops a file, so a
    /// slow next-file lookup does not override what happened meanwhile.
    generation: Cell<u64>,
    thumbs: RefCell<Option<Rc<Thumbs>>>,
    /// The auto-next setting, kept here so position updates (one per
    /// frame) don't read the database. See [`Nav::reload_settings`].
    auto_next: Cell<bool>,
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
            next: RefCell::new(None),
            on_leave_player: RefCell::new(None),
            generation: Cell::new(0),
            thumbs: RefCell::new(None),
            auto_next: Cell::new(true),
        });
        this.reload_settings();

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
        window.global::<PlayerModel>().on_next(move || {
            if let Some(nav) = weak.upgrade() {
                nav.skip_next();
            }
        });
        let weak = Rc::downgrade(&this);
        window.global::<PlayerModel>().on_up_next_play(move || {
            if let Some(nav) = weak.upgrade() {
                nav.play_up_next();
            }
        });
        let weak = Rc::downgrade(&this);
        window.global::<PlayerModel>().on_up_next_cancel(move || {
            if let Some(nav) = weak.upgrade() {
                if let Some(current) = nav.current.borrow_mut().as_mut() {
                    current.up_next.cancel();
                }
                nav.show_up_next(UpNextState::Hidden);
            }
        });
        // Replaces Playback's own Retry, so a file from a source resumes
        // and its next button works again.
        let weak = Rc::downgrade(&this);
        window.global::<PlayerModel>().on_retry(move || {
            if let Some(nav) = weak.upgrade() {
                nav.retry();
            }
        });
        let weak = Rc::downgrade(&this);
        window.on_back(move || {
            if let Some(nav) = weak.upgrade() {
                nav.show_browser();
            }
        });
        this
    }

    /// Plays `path` from `source`, resuming where it was left off.
    pub fn play(self: &Rc<Self>, source: SourceRef, path: SourcePath) {
        self.play_from(source, path, None);
    }

    /// Plays `path` from `source` at `start`, or by default where it was
    /// left off (if resume is on for its folder).
    fn play_from(self: &Rc<Self>, source: SourceRef, path: SourcePath, start: Option<f64>) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        self.save_position();
        self.generation.set(self.generation.get() + 1);
        match source.source.playable(&path) {
            Ok(url) => {
                let state = self.store.watch_state(source.id, &path).ok().flatten();
                let rule = self.store.watched_rule().unwrap_or_default();
                let folder = path.parent().unwrap_or_else(SourcePath::root);
                let resume = self
                    .store
                    .playback_settings(source.id, &folder)
                    .map_or(true, |s| s.resume);
                let start = match start {
                    Some(start) => Some(start),
                    None if resume => start_position(state.as_ref(), &rule),
                    None => None,
                };
                if let Err(err) = self.playback.open(&url, path.name().unwrap_or(&url), start) {
                    self.show_browser();
                    self.browser.show_error(&cannot_play(&path, &err));
                    return;
                }
                *self.next.borrow_mut() = None;
                self.show_up_next(UpNextState::Hidden);
                self.find_next(&source, &path);
                self.find_stamp(&source, &path);
                *self.current.borrow_mut() = Some(Current {
                    source,
                    path,
                    duration: state.and_then(|s| s.duration),
                    position: start.unwrap_or(0.0),
                    stamp: None,
                    lost: false,
                    schedule: SaveSchedule::new(start),
                    up_next: UpNext::new(),
                    loaded: false,
                });
                w.set_page(PAGE_PLAYER);
            }
            Err(err) => {
                self.show_browser();
                self.browser
                    .show_error(&cannot_play(&path, &err.to_string()));
            }
        }
    }

    /// Looks up, off the UI thread, whether a file follows `path` in its
    /// folder, and enables the next button if so.
    fn find_next(self: &Rc<Self>, source: &SourceRef, path: &SourcePath) {
        let this = Rc::downgrade(self);
        let generation = self.generation.get();
        let (src, path) = (source.source.clone(), path.clone());
        let source = source.clone();
        bg::spawn(
            move || next_video(&*src, &path),
            move |next| {
                if let Some(this) = this.upgrade()
                    && this.generation.get() == generation
                {
                    let next = next.ok().flatten();
                    this.playback.set_has_next(next.is_some());
                    if let Some(next) = &next {
                        this.want_thumbnail(&source, next);
                    }
                    *this.next.borrow_mut() = next;
                }
            },
        );
    }

    /// Looks up, off the UI thread, the size and time of the file that just
    /// opened, to compare with if it ends early.
    fn find_stamp(self: &Rc<Self>, source: &SourceRef, path: &SourcePath) {
        let this = Rc::downgrade(self);
        let generation = self.generation.get();
        let (src, path) = (source.source.clone(), path.clone());
        bg::spawn(
            move || src.stamp(&path),
            move |stamp| {
                if let Some(this) = this.upgrade()
                    && this.generation.get() == generation
                    && let Some(current) = this.current.borrow_mut().as_mut()
                {
                    current.stamp = stamp;
                }
            },
        );
    }

    /// Shows thumbnails from `thumbs` on the Up next card.
    pub fn set_thumbs(self: &Rc<Self>, thumbs: &Rc<Thumbs>) {
        let weak = Rc::downgrade(self);
        thumbs.on_ready(move |id, image| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let source = this.current.borrow().as_ref().map(|c| c.source.id);
            let is_next = match (source, this.next.borrow().as_ref()) {
                (Some(source), Some(next)) => thumb_id(source, next) == id,
                _ => false,
            };
            if is_next && let Some(w) = this.window.upgrade() {
                w.global::<PlayerModel>()
                    .set_up_next_thumbnail(image.clone());
            }
        });
        *self.thumbs.borrow_mut() = Some(thumbs.clone());
    }

    /// Asks for the next file's thumbnail for the Up next card, and shows
    /// the one there is so far (or none).
    fn want_thumbnail(&self, source: &SourceRef, next: &SourcePath) {
        let Some(thumbs) = self.thumbs.borrow().clone() else {
            return;
        };
        let location = self
            .browser
            .sources()
            .into_iter()
            .find(|(s, _)| s.id == source.id)
            .map(|(_, location)| location)
            .unwrap_or_default();
        thumbs.want(source.id, &source.source, &location, next);
        if let Some(w) = self.window.upgrade() {
            w.global::<PlayerModel>()
                .set_up_next_thumbnail(thumbs.image(&thumb_id(source.id, next)));
        }
    }

    /// Retry after a playback error.
    fn retry(self: &Rc<Self>) {
        let current = self.current.borrow().as_ref().map(|c| {
            let start = c.lost.then_some(c.position).filter(|p| *p > 0.0);
            (
                c.source.clone(),
                c.path.clone(),
                c.up_next.is_cancelled(),
                start,
            )
        });
        match current {
            Some((source, path, cancelled, start)) => {
                // A file that was cut off goes on where it stopped.
                self.play_from(source, path, start);
                // Cancel holds for the file, also after a retry.
                if cancelled && let Some(c) = self.current.borrow_mut().as_mut() {
                    c.up_next.cancel();
                }
            }
            None => self.playback.retry(),
        }
    }

    /// The next button: saves the position and plays the next file.
    fn skip_next(self: &Rc<Self>) {
        self.save_position();
        let Some(current) = self
            .current
            .borrow()
            .as_ref()
            .map(|c| (c.source.clone(), c.path.clone()))
        else {
            return;
        };
        self.generation.set(self.generation.get() + 1);
        self.play_next(current.0, current.1);
    }

    /// Runs when playback stops and the browser shows, e.g. to close PiP.
    pub fn set_on_leave_player(&self, f: Box<dyn Fn()>) {
        *self.on_leave_player.borrow_mut() = Some(f);
    }

    /// Saves the position, stops playback and returns to the browser.
    pub fn show_browser(self: &Rc<Self>) {
        self.save_position();
        if let Some(f) = self.on_leave_player.borrow().as_ref() {
            f();
        }
        self.generation.set(self.generation.get() + 1);
        *self.current.borrow_mut() = None;
        self.show_up_next(UpNextState::Hidden);
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
            self.store_position(current, position, true);
        }
    }

    /// Saves `position`. A `final_save` (pause, stop, exit) marks the file
    /// watched if it is past the threshold; periodic saves never do, so a
    /// seek past the end and back keeps the resume point.
    fn store_position(&self, current: &Current, position: f64, final_save: bool) {
        let (id, path, duration) = (current.source.id, &current.path, current.duration);
        let result = if final_save {
            self.store.save_position(id, path, position, duration)
        } else {
            self.store.save_progress(id, path, position, duration)
        };
        match result {
            // Home's "Continue watching" shows the new position.
            Ok(()) if final_save => self.browser.changed(),
            Ok(()) => {}
            Err(err) => eprintln!("enthuse: cannot save position: {err}"),
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
                // No card in PiP, and the countdown waits until PiP ends.
                // None either with auto-next off: the file just ends.
                let pip = self.window.upgrade().is_some_and(|w| w.get_pip_active());
                let has_next = !pip && self.auto_next() && self.next.borrow().is_some();
                let up_next = {
                    let mut current = self.current.borrow_mut();
                    let Some(current) = current.as_mut() else {
                        return;
                    };
                    current.position = *p;
                    if current.schedule.due(*p) {
                        self.store_position(current, *p, false);
                    }
                    current.up_next.update(*p, current.duration, has_next)
                };
                self.show_up_next(up_next);
            }
            PlayerEvent::FileLoaded => {
                if let Some(current) = self.current.borrow_mut().as_mut() {
                    current.loaded = true;
                }
            }
            PlayerEvent::Ended(EndReason::Error(err)) => {
                // Failed to open (missing, unreadable): back to where the
                // user was, with the reason. Errors mid-file keep the
                // player's overlay with Retry and Back.
                let path = self
                    .current
                    .borrow()
                    .as_ref()
                    .filter(|c| !c.loaded)
                    .map(|c| c.path.clone());
                if let Some(path) = path {
                    self.show_browser();
                    self.browser.show_error(&cannot_play(&path, err));
                }
            }
            PlayerEvent::PauseChanged(true) => self.save_position(),
            PlayerEvent::Ended(EndReason::EndOfFile) => self.finished(),
            _ => {}
        }
    }

    fn auto_next(&self) -> bool {
        self.auto_next.get()
    }

    /// Picks up changed settings, e.g. auto-next turned off in Settings.
    pub fn reload_settings(&self) {
        self.auto_next.set(self.store.auto_next().unwrap_or(true));
    }

    /// The player reached the end of the data. Well before the duration,
    /// that can be a share that dropped (#57): check that the file is still
    /// there before treating it as watched and going on.
    fn finished(self: &Rc<Self>) {
        let check = self.current.borrow().as_ref().and_then(|c| {
            ended_early(Some(c.position), c.duration)
                .then(|| (c.source.source.clone(), c.path.clone(), c.stamp))
        });
        let Some((src, path, before)) = check else {
            self.complete();
            return;
        };
        // Can take a while on a share that is gone.
        self.playback.set_buffering(true);
        let this = Rc::downgrade(self);
        let generation = self.generation.get();
        bg::spawn(
            move || src.stamp(&path),
            move |now| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if this.generation.get() != generation {
                    return; // The user moved on meanwhile.
                }
                this.playback.set_buffering(false);
                if interrupted(before, now) {
                    this.lost_file();
                } else {
                    this.complete();
                }
            },
        );
    }

    /// The file was cut off: keep its resume point, don't mark it watched
    /// and don't go on into the same share. Retry resumes it once the share
    /// is back; Back returns to the browser.
    fn lost_file(self: &Rc<Self>) {
        if let Some(current) = self.current.borrow_mut().as_mut() {
            current.lost = true;
            self.store_position(current, current.position, false);
        }
        self.show_up_next(UpNextState::Hidden);
        self.playback.set_has_next(false);
        self.playback
            .show_message("Lost access to this file. Check the network share, then Retry.");
    }

    /// The current file played to its end: mark it watched and go on to the
    /// next one in its folder, unless auto-next is off.
    fn complete(self: &Rc<Self>) {
        let Some(current) = self.current.borrow_mut().take() else {
            self.show_browser();
            return;
        };
        self.mark_watched(&current.source, &current.path);
        // Cancelling "Up next" turns auto-next off for this file.
        if current.up_next.is_cancelled() || !self.auto_next() {
            self.show_browser();
        } else {
            self.play_next(current.source, current.path);
        }
    }

    fn mark_watched(&self, source: &SourceRef, path: &SourcePath) {
        if let Err(err) = self.store.set_watched(source.id, path, true) {
            eprintln!("enthuse: cannot mark watched: {err}");
        }
    }

    /// Shows, updates or hides the up next card. When the countdown runs
    /// out, plays the next file. In PiP there is no card, and the next file
    /// starts at the end as before.
    fn show_up_next(self: &Rc<Self>, state: UpNextState) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let model = w.global::<PlayerModel>();
        match state {
            UpNextState::Counting(seconds) if !w.get_pip_active() => {
                let name = self
                    .next
                    .borrow()
                    .as_ref()
                    .and_then(|n| n.name().map(str::to_owned))
                    .unwrap_or_default();
                let label = episode_label(&name);
                let title = if label.is_empty() {
                    display_title(&name)
                } else {
                    label
                };
                model.set_up_next_title(title.into());
                model.set_up_next_seconds(seconds as i32);
                model.set_up_next_visible(true);
            }
            UpNextState::Due if !w.get_pip_active() => {
                model.set_up_next_visible(false);
                self.play_up_next();
            }
            _ => model.set_up_next_visible(false),
        }
    }

    /// "Play now", or the countdown ran out: the current file counts as
    /// watched and the next one starts.
    fn play_up_next(self: &Rc<Self>) {
        let Some(next) = self.next.borrow().clone() else {
            return;
        };
        let Some(current) = self
            .current
            .borrow()
            .as_ref()
            .map(|c| (c.source.clone(), c.path.clone()))
        else {
            return;
        };
        self.save_position();
        self.mark_watched(&current.0, &current.1);
        self.play(current.0, next);
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
                        this.browser
                            .show_error(&format!("Cannot find the next file: {err}"));
                    }
                }
            },
        );
    }
}

/// The toast for a file that would not play: "Couldn't play S02E05: <why>",
/// with the episode when the name has one, else the title.
fn cannot_play(path: &SourcePath, reason: &str) -> String {
    let name = path.name().unwrap_or_default();
    let label = episode_label(name);
    let what = if label.is_empty() {
        display_title(name)
    } else {
        label
    };
    format!("Couldn't play {what}: {reason}")
}
