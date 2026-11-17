//! The "Up next" countdown at the end of a file (docs/ui.md, `UpNextCard`).
//!
//! The card shows once the file is in its last [`WINDOW`] seconds and a next
//! file exists, then counts down [`COUNTDOWN`] seconds of playback (so it
//! pauses with the video) and asks for the next file to play. Only steps of
//! normal playback count, so a seek forward neither starts the next file nor
//! shortens the countdown. Seeking back restarts the countdown, and seeking
//! out of the window hides the card. Cancel hides it for the rest of the file.

/// How close to the end the card appears, in seconds.
pub const WINDOW: f64 = 20.0;
/// How long it counts down before the next file plays, in seconds.
pub const COUNTDOWN: f64 = 10.0;
/// Largest forward step between two updates that still counts as playback.
/// Anything bigger is a seek. Positions arrive several times a second.
const MAX_STEP: f64 = 2.0;

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
    /// Position at the last update inside the window.
    last: Option<f64>,
    /// Seconds of playback counted so far.
    elapsed: f64,
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
        self.last = None;
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
            self.last = None;
            return UpNextState::Hidden;
        }
        match self.last {
            // Entering the window, or a seek back: start over.
            None => self.elapsed = 0.0,
            Some(last) if position < last => self.elapsed = 0.0,
            Some(last) if position - last <= MAX_STEP => self.elapsed += position - last,
            // A seek forward counts as no playback.
            Some(_) => {}
        }
        self.last = Some(position);
        let left = COUNTDOWN - self.elapsed;
        if left <= 1e-9 {
            self.fired = true;
            self.last = None;
            return UpNextState::Due;
        }
        UpNextState::Counting(left.ceil() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: Option<f64> = Some(600.0);

    /// Plays from `from` to `to` in half-second steps, returning the last state.
    fn play(u: &mut UpNext, from: f64, to: f64) -> UpNextState {
        let mut state = u.update(from, LEN, true);
        let mut p = from;
        while p + 0.5 <= to + 1e-9 {
            p += 0.5;
            state = u.update(p, LEN, true);
        }
        state
    }

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
        assert_eq!(play(&mut u, 581.0, 583.5), UpNextState::Counting(8));
        assert_eq!(play(&mut u, 583.5, 590.5), UpNextState::Counting(1));
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
        assert_eq!(play(&mut u, 581.0, 586.0), UpNextState::Counting(5));
        assert_eq!(u.update(300.0, LEN, true), UpNextState::Hidden);
        assert_eq!(u.update(582.0, LEN, true), UpNextState::Counting(10));
    }

    #[test]
    fn seeking_back_within_the_window_restarts() {
        let mut u = UpNext::new();
        assert_eq!(play(&mut u, 586.0, 590.0), UpNextState::Counting(6));
        assert_eq!(u.update(583.0, LEN, true), UpNextState::Counting(10));
    }

    #[test]
    fn a_seek_forward_does_not_count() {
        let mut u = UpNext::new();
        assert_eq!(u.update(581.0, LEN, true), UpNextState::Counting(10));
        assert_eq!(u.update(591.0, LEN, true), UpNextState::Counting(10));
        assert_eq!(play(&mut u, 591.0, 594.0), UpNextState::Counting(7));
        assert_eq!(u.update(598.0, LEN, true), UpNextState::Counting(7));
    }

    #[test]
    fn paused_does_not_count() {
        let mut u = UpNext::new();
        assert_eq!(play(&mut u, 581.0, 584.0), UpNextState::Counting(7));
        assert_eq!(u.update(584.0, LEN, true), UpNextState::Counting(7));
        assert_eq!(u.update(584.0, LEN, true), UpNextState::Counting(7));
    }
}
