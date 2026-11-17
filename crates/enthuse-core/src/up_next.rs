//! The "Up next" countdown at the end of a file (docs/ui.md, `UpNextCard`).
//!
//! The card shows once the file is in its last [`WINDOW`] seconds and a next
//! file exists, then counts down [`COUNTDOWN`] seconds of playback (so it
//! pauses with the video) and asks for the next file to play. Seeking back
//! out of the window hides it and restarts the countdown next time. Cancel
//! hides it for the rest of the file.

/// How close to the end the card appears, in seconds.
pub const WINDOW: f64 = 20.0;
/// How long it counts down before the next file plays, in seconds.
pub const COUNTDOWN: f64 = 10.0;

/// What the screen should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpNextState {
    Hidden,
    /// Show the card with this many whole seconds left.
    Counting(u32),
    /// The countdown ran out: play the next file.
    Due,
}

/// Countdown state for one file. Make a new one for every file.
#[derive(Debug, Default, Clone)]
pub struct UpNext {
    /// Position at which the countdown started.
    start: Option<f64>,
    cancelled: bool,
    fired: bool,
}

impl UpNext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hides the card for the rest of this file. Playback then stops at the
    /// end instead of going on to the next file.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.start = None;
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Updates the countdown for the current `position`. `duration` is the
    /// file's length if known; `has_next` says whether a next file exists.
    /// Returns [`UpNextState::Due`] once per file.
    pub fn update(&mut self, position: f64, duration: Option<f64>, has_next: bool) -> UpNextState {
        let Some(duration) = duration.filter(|d| d.is_finite() && *d > WINDOW) else {
            // Unknown length, or a file shorter than the window: auto-next
            // at the end is enough.
            return UpNextState::Hidden;
        };
        if self.cancelled || self.fired || !has_next || !position.is_finite() {
            return UpNextState::Hidden;
        }
        if duration - position > WINDOW {
            self.start = None;
            return UpNextState::Hidden;
        }
        // Start, or start again after a seek back within the window.
        let start = match self.start {
            Some(s) if s <= position => s,
            _ => position,
        };
        self.start = Some(start);
        let left = COUNTDOWN - (position - start);
        if left <= 0.0 {
            self.fired = true;
            self.start = None;
            return UpNextState::Due;
        }
        UpNextState::Counting(left.ceil() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: Option<f64> = Some(600.0);

    #[test]
    fn hidden_before_the_window() {
        let mut u = UpNext::new();
        assert_eq!(u.update(100.0, LEN, true), UpNextState::Hidden);
        assert_eq!(u.update(579.9, LEN, true), UpNextState::Hidden);
    }

    #[test]
    fn counts_down_then_fires_once() {
        let mut u = UpNext::new();
        assert_eq!(u.update(581.0, LEN, true), UpNextState::Counting(10));
        assert_eq!(u.update(583.5, LEN, true), UpNextState::Counting(8));
        assert_eq!(u.update(590.9, LEN, true), UpNextState::Counting(1));
        assert_eq!(u.update(591.0, LEN, true), UpNextState::Due);
        assert_eq!(u.update(592.0, LEN, true), UpNextState::Hidden);
    }

    #[test]
    fn needs_a_next_file_and_a_known_length() {
        let mut u = UpNext::new();
        assert_eq!(u.update(590.0, LEN, false), UpNextState::Hidden);
        assert_eq!(u.update(590.0, None, true), UpNextState::Hidden);
    }

    #[test]
    fn short_files_get_no_card() {
        let mut u = UpNext::new();
        assert_eq!(u.update(5.0, Some(15.0), true), UpNextState::Hidden);
        assert_eq!(u.update(5.0, Some(WINDOW), true), UpNextState::Hidden);
    }

    #[test]
    fn cancel_hides_for_the_rest_of_the_file() {
        let mut u = UpNext::new();
        assert_eq!(u.update(585.0, LEN, true), UpNextState::Counting(10));
        u.cancel();
        assert!(u.is_cancelled());
        assert_eq!(u.update(586.0, LEN, true), UpNextState::Hidden);
        assert_eq!(u.update(599.0, LEN, true), UpNextState::Hidden);
    }

    #[test]
    fn seeking_out_of_the_window_resets() {
        let mut u = UpNext::new();
        assert_eq!(u.update(581.0, LEN, true), UpNextState::Counting(10));
        assert_eq!(u.update(586.0, LEN, true), UpNextState::Counting(5));
        assert_eq!(u.update(300.0, LEN, true), UpNextState::Hidden);
        assert_eq!(u.update(582.0, LEN, true), UpNextState::Counting(10));
    }

    #[test]
    fn seeking_back_within_the_window_restarts() {
        let mut u = UpNext::new();
        assert_eq!(u.update(586.0, LEN, true), UpNextState::Counting(10));
        assert_eq!(u.update(590.0, LEN, true), UpNextState::Counting(6));
        assert_eq!(u.update(583.0, LEN, true), UpNextState::Counting(10));
    }

    #[test]
    fn a_seek_past_the_countdown_fires() {
        let mut u = UpNext::new();
        assert_eq!(u.update(581.0, LEN, true), UpNextState::Counting(10));
        assert_eq!(u.update(598.0, LEN, true), UpNextState::Due);
    }
}
