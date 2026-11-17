//! Wires the player to the playback screen: forwards UI actions to the
//! player and reflects player events in the UI.

use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::time::format_time;
use enthuse_player::{EndReason, Player, PlayerEvent, TrackKind};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::{AppWindow, TrackItem};

/// Track id the UI uses for "subtitles off".
const SUBTITLES_OFF: i32 = -1;

pub struct Playback {
    player: Arc<dyn Player>,
    window: Weak<AppWindow>,
}

impl Playback {
    /// Connects `window`'s playback callbacks to `player` and starts
    /// forwarding player events to the window.
    pub fn new(window: &AppWindow, player: Arc<dyn Player>) -> Rc<Self> {
        let this = Rc::new(Self {
            player: player.clone(),
            window: window.as_weak(),
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

        let p = player.clone();
        window.on_toggle_pause(move || {
            let _ = p.set_paused(!p.is_paused());
        });
        let p = player.clone();
        window.on_seek(move |t| {
            let _ = p.seek(t as f64);
        });
        let p = player.clone();
        window.on_seek_relative(move |delta| {
            let target = p.position().unwrap_or(0.0) + delta as f64;
            let target = match p.duration() {
                Some(d) => target.min(d),
                None => target,
            };
            let _ = p.seek(target.max(0.0));
        });
        let p = player;
        window.on_select_subtitle(move |id| {
            let id = (id != SUBTITLES_OFF).then_some(id as i64);
            let _ = p.select_track(TrackKind::Subtitle, id);
        });
        this
    }

    /// Opens `url` and shows `title` on the playback screen.
    pub fn open(&self, url: &str, title: &str, start: Option<f64>) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        w.set_video_title(title.into());
        w.set_message(SharedString::new());
        w.set_position(0.0);
        w.set_duration(0.0);
        w.set_position_text(format_time(0.0).into());
        w.set_duration_text(format_time(0.0).into());
        if let Err(err) = self.player.open(url, start) {
            w.set_message(format!("Cannot open this file: {err}").into());
        }
    }

    fn drain_events(&self) {
        while let Some(event) = self.player.poll_event() {
            self.handle(event);
        }
    }

    fn handle(&self, event: PlayerEvent) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        match event {
            PlayerEvent::FileLoaded => {
                w.set_message(SharedString::new());
                self.refresh_tracks(&w);
            }
            PlayerEvent::PositionChanged(pos) => {
                let pos = pos.unwrap_or(0.0);
                w.set_position(pos as f32);
                w.set_position_text(format_time(pos).into());
            }
            PlayerEvent::DurationChanged(d) => {
                let d = d.unwrap_or(0.0);
                w.set_duration(d as f32);
                w.set_duration_text(format_time(d).into());
            }
            PlayerEvent::PauseChanged(paused) => w.set_paused(paused),
            PlayerEvent::TracksChanged => self.refresh_tracks(&w),
            // Auto-next (#7) takes over here.
            PlayerEvent::Ended(EndReason::EndOfFile) => w.set_message("Finished".into()),
            PlayerEvent::Ended(EndReason::Error(err)) => {
                w.set_message(format!("Cannot play this file: {err}").into());
            }
            PlayerEvent::Ended(EndReason::Stopped) => {}
        }
    }

    fn refresh_tracks(&self, w: &AppWindow) {
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
