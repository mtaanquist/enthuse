//! Wires the player to the playback screen: forwards UI actions to the
//! player and reflects player events in the UI.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::time::format_time;
use enthuse_player::{EndReason, Player, PlayerEvent, TrackKind};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::{AppWindow, BrowserWindow, TrackItem};

/// Gets every player event after the playback screen has handled it.
pub type EventListener = Box<dyn Fn(&PlayerEvent)>;

/// Track id the UI uses for "subtitles off".
const SUBTITLES_OFF: i32 = -1;

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
        let p = player;
        window.on_select_subtitle(move |id| {
            let id = (id != SUBTITLES_OFF).then_some(id as i64);
            let _ = p.select_track(TrackKind::Subtitle, id);
        });
        this
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

    /// Opens `url` and shows `title` on the playback screen.
    pub fn open(&self, url: &str, title: &str, start: Option<f64>) {
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
        }
    }

    pub fn set_listener(&self, listener: EventListener) {
        *self.listener.borrow_mut() = Some(listener);
    }

    fn drain_events(&self) {
        while let Some(event) = self.player.poll_event() {
            self.handle(&event);
            if let Some(listener) = self.listener.borrow().as_ref() {
                listener(&event);
            }
        }
    }

    fn handle(&self, event: &PlayerEvent) {
        match event {
            PlayerEvent::FileLoaded => {
                each_view!(self, w => w.set_message(SharedString::new()));
                self.refresh_tracks();
            }
            PlayerEvent::PositionChanged(pos) => {
                let pos = pos.unwrap_or(0.0);
                let text: SharedString = format_time(pos).into();
                each_view!(self, w => {
                    w.set_position(pos as f32);
                    w.set_position_text(text.clone());
                });
            }
            PlayerEvent::DurationChanged(d) => {
                let d = d.unwrap_or(0.0);
                let text: SharedString = format_time(d).into();
                each_view!(self, w => {
                    w.set_duration(d as f32);
                    w.set_duration_text(text.clone());
                });
            }
            PlayerEvent::PauseChanged(paused) => each_view!(self, w => w.set_paused(*paused)),
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
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let tracks: Vec<_> = self
            .player
            .tracks()
            .into_iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .collect();
        let none_selected = !tracks.iter().any(|t| t.selected);
        let mut items = vec![TrackItem {
            id: SUBTITLES_OFF,
            label: "Off".into(),
            selected: none_selected,
        }];
        items.extend(tracks.iter().map(|t| TrackItem {
            id: t.id as i32,
            label: t.label().into(),
            selected: t.selected,
        }));
        w.set_subtitle_tracks(ModelRc::new(VecModel::from(items)));
    }
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
