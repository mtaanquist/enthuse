//! Wires the player to the playback screen: forwards UI actions to the
//! player and reflects player events in the UI.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use enthuse_core::controls::{SPEEDS, remaining_label, speed_label, step_volume};
use enthuse_core::folder::{display_title, parse_episode};
use enthuse_core::time::format_time;
use enthuse_player::{EndReason, Player, PlayerEvent, TrackKind, track_summary};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, VecModel, Weak};

use crate::{AppWindow, BrowserWindow, PlayerModel, TrackItem};

/// Height of the bottom controls (scrubber and transport bar) in logical
/// pixels, which subtitles move above while the controls show.
const CONTROLS_HEIGHT: f32 = 136.0;

/// How long the controls stay up without input while playing.
const HIDE_AFTER: Duration = Duration::from_secs(3);

/// Gets every player event after the playback screen has handled it.
pub type EventListener = Box<dyn Fn(&PlayerEvent)>;

/// Track id the UI uses for "subtitles off".
const TRACK_OFF: i32 = -1;

/// Runs `$body` with `$w` bound to each window that shows what is playing:
/// the main window, and the browser window used while PiP is on. Both have
/// the same now-playing properties.
macro_rules! each_view {
    ($self:ident, $w:ident => $body:expr) => {{
        if let Some($w) = $self.window.upgrade() {
            $body;
        }
        let side = $self.side.borrow().as_ref().and_then(|s| s.upgrade());
        if let Some($w) = side {
            $body;
        }
    }};
}

/// Wires a window's playback controls to the player.
macro_rules! wire_controls {
    ($window:expr, $player:expr) => {{
        let p = $player.clone();
        $window.on_toggle_pause(move || {
            let _ = p.set_paused(!p.is_paused());
        });
        let p = $player.clone();
        $window.on_seek(move |t| {
            let _ = p.seek(t as f64);
        });
        let p = $player.clone();
        $window.on_seek_relative(move |delta| seek_relative(&*p, delta as f64));
    }};
}

pub struct Playback {
    player: Arc<dyn Player>,
    window: Weak<AppWindow>,
    /// The browser window, which shows the PiP video's controls.
    side: RefCell<Option<Weak<BrowserWindow>>>,
    listener: RefCell<Option<EventListener>>,
    /// What was opened last and where it was, for Retry.
    last: RefCell<Option<(String, String)>>,
    position: Cell<f64>,
    duration: Cell<f64>,
    muted: Cell<bool>,
    volume: Cell<f64>,
    /// Hides the controls after [`HIDE_AFTER`] without input.
    hide_timer: Timer,
    /// Off for the self-test, which checks the controls are drawn.
    auto_hide: Cell<bool>,
}

impl Playback {
    /// Connects `window`'s playback callbacks to `player` and starts
    /// forwarding player events to the window.
    pub fn new(window: &AppWindow, player: Arc<dyn Player>) -> Rc<Self> {
        let this = Rc::new(Self {
            player: player.clone(),
            window: window.as_weak(),
            side: RefCell::new(None),
            listener: RefCell::new(None),
            last: RefCell::new(None),
            position: Cell::new(0.0),
            duration: Cell::new(0.0),
            muted: Cell::new(false),
            volume: Cell::new(100.0),
            hide_timer: Timer::default(),
            auto_hide: Cell::new(true),
        });

        // mpv signals pending events from its own thread; drain them on the
        // UI thread. The weak handle avoids keeping the window alive.
        let weak = window.as_weak();
        player.set_wakeup(Arc::new(move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak.upgrade() {
                    w.invoke_player_events_pending();
                }
            });
        }));
        {
            let this = this.clone();
            window.on_player_events_pending(move || this.drain_events());
        }

        wire_controls!(window, player);
        let model = window.global::<PlayerModel>();
        model.set_speeds(ModelRc::new(VecModel::from(
            SPEEDS.iter().map(|s| *s as f32).collect::<Vec<_>>(),
        )));
        let weak = Rc::downgrade(&this);
        model.on_activity(move || {
            if let Some(this) = weak.upgrade() {
                this.show_controls();
            }
        });
        let weak = Rc::downgrade(&this);
        model.on_toggle_controls(move || {
            if let Some(this) = weak.upgrade() {
                this.toggle_controls();
            }
        });
        let p = player.clone();
        model.on_set_volume(move |v| {
            let _ = p.set_muted(false);
            let _ = p.set_volume(v as f64);
        });
        let weak = Rc::downgrade(&this);
        model.on_step_volume(move |delta| {
            if let Some(this) = weak.upgrade() {
                let _ = this.player.set_muted(false);
                let _ = this
                    .player
                    .set_volume(step_volume(this.volume.get(), delta as f64));
            }
        });
        let weak = Rc::downgrade(&this);
        model.on_toggle_mute(move || {
            if let Some(this) = weak.upgrade() {
                let _ = this.player.set_muted(!this.muted.get());
            }
        });
        let p = player.clone();
        model.on_set_speed(move |speed| {
            let _ = p.set_speed(speed as f64);
        });
        let p = player.clone();
        model.on_select_subtitle(move |id| {
            let id = (id != TRACK_OFF).then_some(id as i64);
            let _ = p.select_track(TrackKind::Subtitle, id);
        });
        let p = player.clone();
        model.on_select_audio(move |id| {
            let _ = p.select_track(TrackKind::Audio, Some(id as i64));
        });
        let weak = window.as_weak();
        model.on_toggle_fullscreen(move || {
            if let Some(w) = weak.upgrade() {
                set_fullscreen(&w, !w.window().is_fullscreen());
            }
        });
        let weak = Rc::downgrade(&this);
        model.on_resized(move || {
            if let Some(this) = weak.upgrade()
                && let Some(w) = this.window.upgrade()
            {
                // Full screen can also end from outside (the window
                // manager, a system shortcut); keep the button in step.
                w.global::<PlayerModel>()
                    .set_fullscreen(w.window().is_fullscreen());
                // The room the controls take changes with the height.
                if w.global::<PlayerModel>().get_controls_visible() {
                    this.set_controls_visible(&w, true);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        model.on_retry(move || {
            if let Some(this) = weak.upgrade() {
                this.retry();
            }
        });
        this
    }

    /// Keeps the controls up for good; for the self-test.
    pub fn disable_auto_hide(&self) {
        self.auto_hide.set(false);
        self.hide_timer.stop();
        if let Some(w) = self.window.upgrade() {
            self.set_controls_visible(&w, true);
        }
    }

    /// Shows the controls and starts counting down to hiding them again.
    fn show_controls(self: &Rc<Self>) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        self.set_controls_visible(&w, true);
        self.schedule_hide();
    }

    fn toggle_controls(self: &Rc<Self>) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let model = w.global::<PlayerModel>();
        if model.get_controls_visible() && self.auto_hide.get() && !w.get_paused() {
            self.hide_timer.stop();
            self.set_controls_visible(&w, false);
        } else {
            self.show_controls();
        }
    }

    fn schedule_hide(self: &Rc<Self>) {
        if !self.auto_hide.get() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.hide_timer
            .start(slint::TimerMode::SingleShot, HIDE_AFTER, move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let Some(w) = this.window.upgrade() else {
                    return;
                };
                let model = w.global::<PlayerModel>();
                // Keep them while paused, in a menu or under the pointer.
                if w.get_paused() || model.get_menu() != 0 || model.get_pointer_on_controls() {
                    this.schedule_hide();
                } else {
                    this.set_controls_visible(&w, false);
                }
            });
    }

    /// Shows or hides the controls, and moves subtitles above the bottom
    /// controls while they show, so they are not covered. Outside PiP only.
    fn set_controls_visible(&self, w: &AppWindow, visible: bool) {
        w.global::<PlayerModel>().set_controls_visible(visible);
        let height = w
            .window()
            .size()
            .to_logical(w.window().scale_factor())
            .height;
        // PiP has no bottom controls to make room for.
        let pos = if visible && height > 0.0 && !w.get_pip_active() {
            (100.0 - CONTROLS_HEIGHT / height * 100.0).clamp(50.0, 100.0)
        } else {
            100.0
        };
        let _ = self.player.set_option("sub-pos", &format!("{pos:.0}"));
    }

    /// Opens the last file again where it stopped, after an error. Files
    /// from a source go through `Nav` instead, which resumes them.
    pub fn retry(self: &Rc<Self>) {
        let Some((url, title)) = self.last.borrow().clone() else {
            return;
        };
        let start = Some(self.position.get()).filter(|p| *p > 0.0);
        let _ = self.open(&url, &title, start);
    }

    /// Leaves full screen, e.g. when going back to the browser.
    pub fn leave_fullscreen(&self) {
        if let Some(w) = self.window.upgrade()
            && w.window().is_fullscreen()
        {
            set_fullscreen(&w, false);
        }
    }

    /// Shows the playing video's state and controls in `side` too.
    pub fn attach_side(&self, side: &BrowserWindow) {
        wire_controls!(side, self.player);
        if let Some(w) = self.window.upgrade() {
            side.set_video_title(w.get_video_title());
            side.set_message(w.get_message());
            side.set_paused(w.get_paused());
            side.set_position(w.get_position());
            side.set_duration(w.get_duration());
            side.set_position_text(w.get_position_text());
            side.set_duration_text(w.get_duration_text());
        }
        *self.side.borrow_mut() = Some(side.as_weak());
    }

    /// Opens `url` and shows `title` (a file name) on the playback screen.
    /// Opens `url`. An error the player reports at once shows on the
    /// overlay and is returned too, so a caller can go back instead.
    pub fn open(self: &Rc<Self>, url: &str, title: &str, start: Option<f64>) -> Result<(), String> {
        *self.last.borrow_mut() = Some((url.to_owned(), title.to_owned()));
        self.position.set(start.unwrap_or(0.0));
        if let Some(w) = self.window.upgrade() {
            let model = w.global::<PlayerModel>();
            model.set_subtitle(
                parse_episode(title)
                    .map(|e| e.code())
                    .unwrap_or_default()
                    .into(),
            );
            model.set_buffered(0.0);
            model.set_remaining_text(remaining_label(0.0, 0.0).into());
            model.set_has_next(false);
            model.set_pointer_on_controls(false);
        }
        self.show_controls();
        let title = display_title(title);
        let title = title.as_str();
        each_view!(self, w => {
            w.set_video_title(title.into());
            w.set_message(SharedString::new());
            w.set_position(0.0);
            w.set_duration(0.0);
            w.set_position_text(format_time(0.0).into());
            w.set_duration_text(format_time(0.0).into());
        });
        if let Err(err) = self.player.open(url, start) {
            let message: SharedString = format!("Cannot open this file: {err}").into();
            each_view!(self, w => w.set_message(message.clone()));
            return Err(err.to_string());
        }
        Ok(())
    }

    pub fn set_listener(&self, listener: EventListener) {
        *self.listener.borrow_mut() = Some(listener);
    }

    fn drain_events(self: &Rc<Self>) {
        while let Some(event) = self.player.poll_event() {
            self.handle(&event);
            if let Some(listener) = self.listener.borrow().as_ref() {
                listener(&event);
            }
        }
    }

    fn with_model(&self, f: impl FnOnce(PlayerModel<'_>)) {
        if let Some(w) = self.window.upgrade() {
            f(w.global::<PlayerModel>());
        }
    }

    fn update_remaining(&self) {
        let text = remaining_label(self.position.get(), self.duration.get());
        self.with_model(|m| m.set_remaining_text(text.into()));
    }

    /// Tells the screen whether a next file exists, for the next button.
    pub fn set_has_next(&self, has_next: bool) {
        self.with_model(|m| m.set_has_next(has_next));
    }

    fn handle(self: &Rc<Self>, event: &PlayerEvent) {
        match event {
            PlayerEvent::FileLoaded => {
                each_view!(self, w => w.set_message(SharedString::new()));
                self.refresh_tracks();
            }
            PlayerEvent::PositionChanged(pos) => {
                // Keep the last known position for Retry: it reads as
                // unknown once a file fails.
                if let Some(p) = pos {
                    self.position.set(*p);
                }
                let pos = pos.unwrap_or(0.0);
                self.update_remaining();
                let text: SharedString = format_time(pos).into();
                each_view!(self, w => {
                    w.set_position(pos as f32);
                    w.set_position_text(text.clone());
                });
            }
            PlayerEvent::DurationChanged(d) => {
                let d = d.unwrap_or(0.0);
                self.duration.set(d);
                self.update_remaining();
                let text: SharedString = format_time(d).into();
                each_view!(self, w => {
                    w.set_duration(d as f32);
                    w.set_duration_text(text.clone());
                });
            }
            PlayerEvent::PauseChanged(paused) => {
                each_view!(self, w => w.set_paused(*paused));
                // Pausing shows the controls; playing again hides them later.
                self.show_controls();
            }
            PlayerEvent::VolumeChanged(v) => {
                self.volume.set(*v);
                self.with_model(|m| m.set_volume(*v as f32));
            }
            PlayerEvent::MuteChanged(m) => {
                self.muted.set(*m);
                self.with_model(|model| model.set_muted(*m));
            }
            PlayerEvent::SpeedChanged(speed) => self.with_model(|m| {
                m.set_speed(*speed as f32);
                m.set_speed_label(speed_label(*speed).into());
            }),
            PlayerEvent::BufferingChanged(b) => self.with_model(|m| m.set_buffering(*b)),
            PlayerEvent::BufferedChanged(t) => {
                self.with_model(|m| m.set_buffered(t.unwrap_or(0.0) as f32))
            }
            PlayerEvent::TracksChanged => self.refresh_tracks(),
            PlayerEvent::AspectChanged(a) => {
                if let Some(w) = self.window.upgrade() {
                    w.set_video_aspect(a.unwrap_or(0.0) as f32);
                }
            }
            // With a listener, auto-next takes over.
            PlayerEvent::Ended(EndReason::EndOfFile) => {
                if self.listener.borrow().is_none() {
                    each_view!(self, w => w.set_message("Finished".into()));
                }
            }
            PlayerEvent::Ended(EndReason::Error(err)) => {
                let message: SharedString = format!("Cannot play this file: {err}").into();
                each_view!(self, w => w.set_message(message.clone()));
            }
            PlayerEvent::Ended(EndReason::Stopped) => {}
        }
    }

    fn refresh_tracks(&self) {
        let tracks = self.player.tracks();
        let items = |kind: TrackKind| {
            tracks
                .iter()
                .filter(|t| t.kind == kind)
                .map(|t| TrackItem {
                    id: t.id as i32,
                    label: t.label().into(),
                    selected: t.selected,
                })
                .collect::<Vec<_>>()
        };
        let mut subtitles = vec![TrackItem {
            id: TRACK_OFF,
            label: "Off".into(),
            selected: !tracks
                .iter()
                .any(|t| t.kind == TrackKind::Subtitle && t.selected),
        }];
        subtitles.extend(items(TrackKind::Subtitle));
        let audio = items(TrackKind::Audio);
        let summary = track_summary(&tracks);
        self.with_model(|m| {
            m.set_subtitle_tracks(ModelRc::new(VecModel::from(subtitles)));
            m.set_audio_tracks(ModelRc::new(VecModel::from(audio)));
            m.set_tracks_label(summary.into());
        });
    }
}

fn set_fullscreen(w: &AppWindow, fullscreen: bool) {
    w.window().set_fullscreen(fullscreen);
    w.global::<PlayerModel>().set_fullscreen(fullscreen);
}

/// Seeks `delta` seconds from the current position, within the video.
fn seek_relative(player: &dyn Player, delta: f64) {
    let target = player.position().unwrap_or(0.0) + delta;
    let target = match player.duration() {
        Some(d) => target.min(d),
        None => target,
    };
    let _ = player.seek(target.max(0.0));
}
