//! [`Player`] implemented with libmpv.

use std::sync::{Arc, Mutex};

use libmpv2::events::{Event, PropertyData};
use libmpv2::{Format, Mpv};
use libmpv2_sys as sys;

use crate::gl::GlRenderContext;
use crate::{
    EndReason, GetProcAddress, Player, PlayerError, PlayerEvent, Result, Track, TrackKind,
    VideoRenderer, WakeupFn,
};

impl From<libmpv2::Error> for PlayerError {
    fn from(err: libmpv2::Error) -> Self {
        PlayerError(format!("mpv: {err}"))
    }
}

// Ids for observed properties.
const OBS_TIME_POS: u64 = 1;
const OBS_DURATION: u64 = 2;
const OBS_PAUSE: u64 = 3;
const OBS_TRACK_COUNT: u64 = 4;
const OBS_AID: u64 = 5;
const OBS_SID: u64 = 6;
const OBS_ASPECT: u64 = 7;
const OBS_VOLUME: u64 = 8;
const OBS_MUTE: u64 = 9;
const OBS_SPEED: u64 = 10;
const OBS_BUFFERING: u64 = 11;
const OBS_CACHE_END: u64 = 12;

/// Options applied before mpv starts. Callers can override them.
const DEFAULT_OPTIONS: &[(&str, &str)] = &[
    // Draw through the render API into the app's GL context.
    ("vo", "libmpv"),
    ("hwdec", "auto-safe"),
    // End files normally so we get an end-of-file event; the app decides
    // what plays next.
    ("keep-open", "no"),
    ("idle", "yes"),
    ("input-default-bindings", "no"),
    ("terminal", "no"),
];

/// libmpv-backed [`Player`].
pub struct MpvPlayer {
    mpv: Arc<Mpv>,
    wakeup: Arc<Mutex<Option<WakeupFn>>>,
}

impl MpvPlayer {
    /// Creates a player with the default options plus `options`, which take
    /// precedence (e.g. `("vo", "null")` for headless tests).
    pub fn new(options: &[(&str, &str)]) -> Result<Self> {
        let mut mpv = Mpv::with_initializer(|init| {
            for (k, v) in DEFAULT_OPTIONS {
                if !options.iter().any(|(ok, _)| ok == k) {
                    init.set_option(k, *v)?;
                }
            }
            for (k, v) in options {
                init.set_option(k, *v)?;
            }
            Ok(())
        })?;

        mpv.observe_property("time-pos", Format::Double, OBS_TIME_POS)?;
        mpv.observe_property("duration", Format::Double, OBS_DURATION)?;
        mpv.observe_property("pause", Format::Flag, OBS_PAUSE)?;
        mpv.observe_property("track-list/count", Format::Int64, OBS_TRACK_COUNT)?;
        mpv.observe_property("aid", Format::String, OBS_AID)?;
        mpv.observe_property("sid", Format::String, OBS_SID)?;
        mpv.observe_property("video-params/aspect", Format::Double, OBS_ASPECT)?;
        mpv.observe_property("volume", Format::Double, OBS_VOLUME)?;
        mpv.observe_property("mute", Format::Flag, OBS_MUTE)?;
        mpv.observe_property("speed", Format::Double, OBS_SPEED)?;
        mpv.observe_property("paused-for-cache", Format::Flag, OBS_BUFFERING)?;
        mpv.observe_property("demuxer-cache-time", Format::Double, OBS_CACHE_END)?;

        let wakeup: Arc<Mutex<Option<WakeupFn>>> = Arc::default();
        let slot = wakeup.clone();
        mpv.set_wakeup_callback(move || {
            // Clone out of the lock so the callback never runs while held.
            let f = slot.lock().ok().and_then(|g| g.clone());
            if let Some(f) = f {
                f();
            }
        });

        Ok(Self {
            mpv: Arc::new(mpv),
            wakeup,
        })
    }

    fn track_prop<T: libmpv2::GetData>(&self, i: i64, name: &str) -> Option<T> {
        self.mpv
            .get_property(&format!("track-list/{i}/{name}"))
            .ok()
    }
}

fn kind_property(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "vid",
        TrackKind::Audio => "aid",
        TrackKind::Subtitle => "sid",
    }
}

/// Maps mpv's `track-list/N/type` to our kind.
fn track_kind(mpv_type: &str) -> Option<TrackKind> {
    match mpv_type {
        "video" => Some(TrackKind::Video),
        "audio" => Some(TrackKind::Audio),
        "sub" => Some(TrackKind::Subtitle),
        _ => None,
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

/// Translates an mpv event into ours. Returns `None` for events we ignore.
fn translate(event: &Event<'_>) -> Option<PlayerEvent> {
    match event {
        Event::FileLoaded => Some(PlayerEvent::FileLoaded),
        Event::EndFile(reason) => Some(PlayerEvent::Ended(end_reason(*reason))),
        Event::PropertyChange {
            change,
            reply_userdata,
            ..
        } => match (*reply_userdata, change) {
            (OBS_TIME_POS, d) => Some(PlayerEvent::PositionChanged(as_seconds(d))),
            (OBS_DURATION, d) => Some(PlayerEvent::DurationChanged(as_seconds(d))),
            (OBS_PAUSE, PropertyData::Flag(p)) => Some(PlayerEvent::PauseChanged(*p)),
            (OBS_TRACK_COUNT | OBS_AID | OBS_SID, _) => Some(PlayerEvent::TracksChanged),
            (OBS_ASPECT, PropertyData::Double(a)) if a.is_finite() && *a > 0.0 => {
                Some(PlayerEvent::AspectChanged(Some(*a)))
            }
            (OBS_ASPECT, _) => Some(PlayerEvent::AspectChanged(None)),
            (OBS_VOLUME, PropertyData::Double(v)) => Some(PlayerEvent::VolumeChanged(*v)),
            (OBS_MUTE, PropertyData::Flag(m)) => Some(PlayerEvent::MuteChanged(*m)),
            (OBS_SPEED, PropertyData::Double(s)) => Some(PlayerEvent::SpeedChanged(*s)),
            (OBS_BUFFERING, PropertyData::Flag(b)) => Some(PlayerEvent::BufferingChanged(*b)),
            // Unavailable (e.g. no file) reads as "not buffering".
            (OBS_BUFFERING, _) => Some(PlayerEvent::BufferingChanged(false)),
            (OBS_CACHE_END, d) => Some(PlayerEvent::BufferedChanged(as_seconds(d))),
            _ => None,
        },
        _ => None,
    }
}

fn as_seconds(data: &PropertyData<'_>) -> Option<f64> {
    match data {
        PropertyData::Double(d) if d.is_finite() => Some(*d),
        _ => None,
    }
}

/// Readable text for a libmpv error, e.g. "loading failed" instead of
/// "Raw(-13)".
fn describe(err: &libmpv2::Error) -> String {
    match err {
        libmpv2::Error::Raw(code) => {
            // SAFETY: mpv_error_string accepts any code and returns a pointer
            // to a static, NUL-terminated string.
            let text = unsafe { std::ffi::CStr::from_ptr(sys::mpv_error_string(*code)) };
            text.to_string_lossy().into_owned()
        }
        other => other.to_string(),
    }
}

fn end_reason(reason: sys::mpv_end_file_reason) -> EndReason {
    match reason {
        sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF => EndReason::EndOfFile,
        sys::mpv_end_file_reason_MPV_END_FILE_REASON_ERROR => {
            EndReason::Error("playback failed".into())
        }
        // STOP, QUIT and REDIRECT all mean the app (or mpv) moved on.
        _ => EndReason::Stopped,
    }
}

impl Player for MpvPlayer {
    fn open(&self, url: &str, start: Option<f64>) -> Result<()> {
        match start {
            Some(s) if s > 0.0 => {
                // loadfile <url> <flags> <index> <options>; index -1 is
                // required before options since mpv 0.38 and ignored before.
                let opt = format!("start={s}");
                self.mpv
                    .command("loadfile", &[url, "replace", "-1", &opt])
                    .or_else(|_| self.mpv.command("loadfile", &[url, "replace", &opt]))?;
            }
            _ => self.mpv.command("loadfile", &[url, "replace"])?,
        }
        self.set_paused(false)
    }

    fn stop(&self) -> Result<()> {
        Ok(self.mpv.command("stop", &[])?)
    }

    fn set_paused(&self, paused: bool) -> Result<()> {
        Ok(self.mpv.set_property("pause", paused)?)
    }

    fn is_paused(&self) -> bool {
        self.mpv.get_property("pause").unwrap_or(false)
    }

    fn seek(&self, seconds: f64) -> Result<()> {
        Ok(self
            .mpv
            .command("seek", &[&seconds.max(0.0).to_string(), "absolute"])?)
    }

    fn position(&self) -> Option<f64> {
        self.mpv.get_property("time-pos").ok()
    }

    fn duration(&self) -> Option<f64> {
        self.mpv.get_property("duration").ok()
    }

    fn volume(&self) -> f64 {
        self.mpv.get_property("volume").unwrap_or(100.0)
    }

    fn set_volume(&self, percent: f64) -> Result<()> {
        Ok(self.mpv.set_property("volume", percent.clamp(0.0, 100.0))?)
    }

    fn set_muted(&self, muted: bool) -> Result<()> {
        Ok(self.mpv.set_property("mute", muted)?)
    }

    fn set_speed(&self, speed: f64) -> Result<()> {
        Ok(self.mpv.set_property("speed", speed.clamp(0.25, 4.0))?)
    }

    fn tracks(&self) -> Vec<Track> {
        let count: i64 = self.mpv.get_property("track-list/count").unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let kind = track_kind(&self.track_prop::<String>(i, "type")?)?;
                Some(Track {
                    id: self.track_prop(i, "id")?,
                    kind,
                    language: non_empty(self.track_prop(i, "lang")),
                    title: non_empty(self.track_prop(i, "title")),
                    codec: non_empty(self.track_prop(i, "codec")),
                    selected: self.track_prop(i, "selected").unwrap_or(false),
                    external: self.track_prop(i, "external").unwrap_or(false),
                })
            })
            .collect()
    }

    fn select_track(&self, kind: TrackKind, id: Option<i64>) -> Result<()> {
        let value = id.map_or_else(|| "no".to_owned(), |id| id.to_string());
        Ok(self.mpv.set_property(kind_property(kind), value)?)
    }

    fn set_option(&self, key: &str, value: &str) -> Result<()> {
        Ok(self.mpv.set_property(key, value)?)
    }

    fn poll_event(&self) -> Option<PlayerEvent> {
        loop {
            match self.mpv.wait_event(0.0)? {
                Ok(event) => {
                    if let Some(e) = translate(&event) {
                        return Some(e);
                    }
                }
                // libmpv2 reports an end-file with an error code as Err.
                // Other events carry no error, since we make no async calls.
                Err(err) => return Some(PlayerEvent::Ended(EndReason::Error(describe(&err)))),
            }
        }
    }

    fn set_wakeup(&self, wakeup: WakeupFn) {
        if let Ok(mut slot) = self.wakeup.lock() {
            *slot = Some(wakeup.clone());
        }
        // mpv calls the wakeup callback only once until the queue is read
        // again. Events queued before this (e.g. the initial property values)
        // would otherwise never be noticed, and no later wakeup would come.
        wakeup();
    }

    unsafe fn create_renderer(
        &self,
        get_proc_address: GetProcAddress<'_>,
    ) -> Result<Box<dyn VideoRenderer>> {
        // SAFETY: forwarded from the caller's contract.
        let ctx = unsafe { GlRenderContext::new(self.mpv.clone(), get_proc_address)? };
        Ok(Box::new(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_raw_errors() {
        assert_eq!(
            describe(&libmpv2::Error::Raw(
                sys::mpv_error_MPV_ERROR_LOADING_FAILED
            )),
            "loading failed"
        );
    }

    #[test]
    fn end_reasons() {
        assert_eq!(
            end_reason(sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF),
            EndReason::EndOfFile
        );
        assert_eq!(
            end_reason(sys::mpv_end_file_reason_MPV_END_FILE_REASON_STOP),
            EndReason::Stopped
        );
        assert_eq!(
            end_reason(sys::mpv_end_file_reason_MPV_END_FILE_REASON_QUIT),
            EndReason::Stopped
        );
        assert!(matches!(
            end_reason(sys::mpv_end_file_reason_MPV_END_FILE_REASON_ERROR),
            EndReason::Error(_)
        ));
    }

    #[test]
    fn translates_events() {
        let prop = |id, change| Event::PropertyChange {
            name: "x",
            change,
            reply_userdata: id,
        };
        assert_eq!(translate(&Event::FileLoaded), Some(PlayerEvent::FileLoaded));
        assert_eq!(
            translate(&Event::EndFile(
                sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF
            )),
            Some(PlayerEvent::Ended(EndReason::EndOfFile))
        );
        assert_eq!(
            translate(&prop(OBS_TIME_POS, PropertyData::Double(12.5))),
            Some(PlayerEvent::PositionChanged(Some(12.5)))
        );
        assert_eq!(
            translate(&prop(OBS_TIME_POS, PropertyData::None)),
            Some(PlayerEvent::PositionChanged(None))
        );
        assert_eq!(
            translate(&prop(OBS_DURATION, PropertyData::Double(f64::NAN))),
            Some(PlayerEvent::DurationChanged(None))
        );
        assert_eq!(
            translate(&prop(OBS_PAUSE, PropertyData::Flag(true))),
            Some(PlayerEvent::PauseChanged(true))
        );
        assert_eq!(
            translate(&prop(OBS_SID, PropertyData::Str("2"))),
            Some(PlayerEvent::TracksChanged)
        );
        assert_eq!(
            translate(&prop(OBS_ASPECT, PropertyData::Double(16.0 / 9.0))),
            Some(PlayerEvent::AspectChanged(Some(16.0 / 9.0)))
        );
        assert_eq!(
            translate(&prop(OBS_ASPECT, PropertyData::None)),
            Some(PlayerEvent::AspectChanged(None))
        );
        assert_eq!(
            translate(&prop(OBS_MUTE, PropertyData::Flag(true))),
            Some(PlayerEvent::MuteChanged(true))
        );
        assert_eq!(
            translate(&prop(OBS_BUFFERING, PropertyData::None)),
            Some(PlayerEvent::BufferingChanged(false))
        );
        assert_eq!(
            translate(&prop(OBS_CACHE_END, PropertyData::Double(80.0))),
            Some(PlayerEvent::BufferedChanged(Some(80.0)))
        );
        assert_eq!(translate(&prop(99, PropertyData::Int64(1))), None);
        assert_eq!(translate(&Event::Seek), None);
    }

    #[test]
    fn track_kinds() {
        assert_eq!(track_kind("video"), Some(TrackKind::Video));
        assert_eq!(track_kind("audio"), Some(TrackKind::Audio));
        assert_eq!(track_kind("sub"), Some(TrackKind::Subtitle));
        assert_eq!(track_kind("attachment"), None);
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("eng".into())), Some("eng".into()));
    }
}
