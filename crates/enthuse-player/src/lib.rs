//! Playback for Enthuse: the [`Player`] trait and its libmpv implementation.
//!
//! The trait is intentionally small so another backend (for example libVLC)
//! can be added later. Playlist and auto-next logic do not live here; see
//! `enthuse-core`. The player draws subtitles itself, into the video frame.

pub mod gl;
mod mpv;
pub mod thumbnail;

pub use mpv::MpvPlayer;

use std::ffi::{CStr, c_void};
use std::fmt;
use std::sync::Arc;

/// Error from a player backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerError(pub String);

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlayerError {}

pub type Result<T> = std::result::Result<T, PlayerError>;

/// Kind of a media track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
}

/// A track in the current file, in our own model rather than a backend's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Backend track id, unique per kind within the current file.
    pub id: i64,
    pub kind: TrackKind,
    /// Language code such as `eng`, if known.
    pub language: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    /// Loaded from a separate file (e.g. an `.srt` next to the video).
    pub external: bool,
}

impl Track {
    /// A short human-readable label, e.g. `English (SDH) [external]` or
    /// `Track 3`.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(lang) = &self.language {
            parts.push(language_name(lang).unwrap_or(lang).to_owned());
        }
        if let Some(title) = &self.title {
            if parts.is_empty() {
                parts.push(title.clone());
            } else {
                parts.push(format!("({title})"));
            }
        }
        if parts.is_empty() {
            parts.push(format!("Track {}", self.id));
        }
        if self.external {
            parts.push("[external]".to_owned());
        }
        parts.join(" ")
    }
}

/// Short summary of the selected subtitle and audio tracks for the player's
/// menu button, e.g. `English, Danish audio`. The audio part is left out
/// when the file has only one audio track.
pub fn track_summary(tracks: &[Track]) -> String {
    let subtitle = tracks
        .iter()
        .find(|t| t.kind == TrackKind::Subtitle && t.selected)
        .map_or_else(|| "Subtitles off".to_owned(), short_label);
    let audio: Vec<_> = tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .collect();
    match audio.iter().find(|t| t.selected) {
        Some(a) if audio.len() > 1 => format!("{subtitle}, {} audio", short_label(a)),
        _ => subtitle,
    }
}

/// The language name, else the title, else `Track N`.
fn short_label(track: &Track) -> String {
    track
        .language
        .as_deref()
        .map(|l| language_name(l).unwrap_or(l).to_owned())
        .or_else(|| track.title.clone())
        .unwrap_or_else(|| format!("Track {}", track.id))
}

/// English names for common ISO 639 codes; others are shown as-is.
fn language_name(code: &str) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase().as_str() {
        "en" | "eng" => "English",
        "da" | "dan" => "Danish",
        "de" | "deu" | "ger" => "German",
        "es" | "spa" => "Spanish",
        "fi" | "fin" => "Finnish",
        "fr" | "fra" | "fre" => "French",
        "it" | "ita" => "Italian",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "nl" | "nld" | "dut" => "Dutch",
        "no" | "nor" | "nb" | "nob" => "Norwegian",
        "pl" | "pol" => "Polish",
        "pt" | "por" => "Portuguese",
        "ru" | "rus" => "Russian",
        "sv" | "swe" => "Swedish",
        "zh" | "zho" | "chi" => "Chinese",
        _ => return None,
    })
}

/// Why playback of a file ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// Reached the end of the file. Auto-next reacts only to this.
    EndOfFile,
    /// Stopped by the app (stop, or another file was opened).
    Stopped,
    /// The file could not be played.
    Error(String),
}

/// Something that happened in the player. Delivered through
/// [`Player::poll_event`].
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// A file was opened and its duration and tracks are known.
    FileLoaded,
    /// Playback of the current file ended.
    Ended(EndReason),
    /// Current position in seconds, or `None` if unknown.
    PositionChanged(Option<f64>),
    /// Duration in seconds, or `None` if unknown (e.g. live streams).
    DurationChanged(Option<f64>),
    PauseChanged(bool),
    /// The list of tracks or the selected tracks changed.
    TracksChanged,
    /// Display aspect ratio (width / height) of the video, or `None` if
    /// there is no video.
    AspectChanged(Option<f64>),
    /// Volume in percent.
    VolumeChanged(f64),
    MuteChanged(bool),
    /// Playback speed, 1.0 is normal.
    SpeedChanged(f64),
    /// Playback is waiting for data (e.g. a slow network share).
    BufferingChanged(bool),
    /// Position in seconds up to which the file is read ahead, if known.
    BufferedChanged(Option<f64>),
}

/// Called from any thread when events are waiting. It must not call into
/// the player; it should only schedule [`Player::poll_event`] on the UI
/// thread.
pub type WakeupFn = Arc<dyn Fn() + Send + Sync>;

/// A function that resolves OpenGL function pointers by name, like
/// `eglGetProcAddress`.
pub type GetProcAddress<'a> = &'a dyn Fn(&CStr) -> *const c_void;

/// Draws video into the app's OpenGL context. Lives on the GL thread.
pub trait VideoRenderer {
    /// Sets a callback that is called, from any thread, when a new frame
    /// should be drawn. It must only schedule a redraw.
    fn set_update_callback(&mut self, callback: Box<dyn Fn() + Send>);

    /// Draws the current frame into framebuffer `fbo` (0 for the window)
    /// of `width` x `height` pixels.
    fn render(&self, fbo: i32, width: i32, height: i32) -> Result<()>;

    /// Tells the backend that the frame was presented, for frame timing.
    fn report_swap(&self);
}

/// A media player backend.
pub trait Player: Send + Sync {
    /// Opens `url` (a local path or URL), optionally starting at `start`
    /// seconds, and starts playing. Ends the current file with
    /// [`EndReason::Stopped`].
    fn open(&self, url: &str, start: Option<f64>) -> Result<()>;
    /// Stops playback of the current file.
    fn stop(&self) -> Result<()>;
    fn set_paused(&self, paused: bool) -> Result<()>;
    fn is_paused(&self) -> bool;
    /// Seeks to `seconds` from the start of the file.
    fn seek(&self, seconds: f64) -> Result<()>;
    fn position(&self) -> Option<f64>;
    fn duration(&self) -> Option<f64>;
    /// Volume in percent, 0 to 100.
    fn volume(&self) -> f64;
    fn set_volume(&self, percent: f64) -> Result<()>;
    fn set_muted(&self, muted: bool) -> Result<()>;
    /// Playback speed, 1.0 is normal.
    fn set_speed(&self, speed: f64) -> Result<()>;
    /// All tracks of the current file.
    fn tracks(&self) -> Vec<Track>;
    /// Selects a track of `kind` by id, or turns that kind off with `None`.
    fn select_track(&self, kind: TrackKind, id: Option<i64>) -> Result<()>;
    /// Backend-specific tuning, e.g. mpv options.
    fn set_option(&self, key: &str, value: &str) -> Result<()>;

    /// Returns the next pending event without blocking, or `None`.
    fn poll_event(&self) -> Option<PlayerEvent>;
    /// Sets the callback that signals pending events. Replaces any earlier one.
    ///
    /// It may run on any thread, and runs once right away in case events are
    /// already queued. After each wakeup, drain [`Player::poll_event`] until
    /// it returns `None`; the next wakeup may not come before that.
    fn set_wakeup(&self, wakeup: WakeupFn);

    /// Creates a renderer that draws into the current OpenGL context.
    /// Create it before opening a file, or the backend may fail to set up
    /// video output.
    ///
    /// # Safety
    ///
    /// The OpenGL context `get_proc_address` belongs to must be current on
    /// this thread. The renderer must only be used, and dropped, on this
    /// thread while that context is current.
    unsafe fn create_renderer(
        &self,
        get_proc_address: GetProcAddress<'_>,
    ) -> Result<Box<dyn VideoRenderer>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(language: Option<&str>, title: Option<&str>, external: bool) -> Track {
        Track {
            id: 3,
            kind: TrackKind::Subtitle,
            language: language.map(Into::into),
            title: title.map(Into::into),
            codec: None,
            selected: false,
            external,
        }
    }

    #[test]
    fn labels() {
        assert_eq!(track(Some("eng"), None, false).label(), "English");
        assert_eq!(
            track(Some("dan"), Some("SDH"), false).label(),
            "Danish (SDH)"
        );
        assert_eq!(track(None, Some("Commentary"), false).label(), "Commentary");
        assert_eq!(track(None, None, false).label(), "Track 3");
        assert_eq!(track(Some("xyz"), None, true).label(), "xyz [external]");
    }

    #[test]
    fn summaries() {
        let t = |kind, id, lang: &str, selected| Track {
            id,
            kind,
            language: Some(lang.into()),
            title: None,
            codec: None,
            selected,
            external: false,
        };
        let sub = |lang, selected| t(TrackKind::Subtitle, 1, lang, selected);
        let audio = |id, lang, selected| t(TrackKind::Audio, id, lang, selected);
        assert_eq!(
            track_summary(&[
                sub("eng", true),
                audio(1, "eng", false),
                audio(2, "dan", true)
            ]),
            "English, Danish audio"
        );
        assert_eq!(
            track_summary(&[sub("eng", false), audio(1, "eng", true)]),
            "Subtitles off"
        );
        assert_eq!(track_summary(&[]), "Subtitles off");
    }
}
