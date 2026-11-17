//! Resume and watched decisions: where to start a file, when to save the
//! position, and which marker the browser shows. Pure logic; the app does
//! the storing.

use crate::storage::{WatchState, reaches_threshold};

/// How far playback must move (in seconds) before the position is saved
/// again. Pausing, stopping and leaving the file save regardless.
pub const SAVE_INTERVAL_SECS: f64 = 10.0;

/// Positions closer to the start than this are not worth resuming.
pub const MIN_RESUME_SECS: f64 = 5.0;

/// Where to start playing a file with saved state `state`: the saved
/// position, unless it is barely started or past the watched `threshold`,
/// in which case the beginning (`None`). A watched file that was started
/// again resumes too; marking a file watched clears its position.
pub fn start_position(state: Option<&WatchState>, threshold: f64) -> Option<f64> {
    let state = state?;
    let position = state.position.filter(|p| *p >= MIN_RESUME_SECS)?;
    if reaches_threshold(position, state.duration, threshold) {
        return None;
    }
    Some(position)
}

/// Decides when the periodic position save is due.
#[derive(Debug, Clone)]
pub struct SaveSchedule {
    last_saved: f64,
}

impl SaveSchedule {
    /// Starts counting from `start`, where playback begins.
    pub fn new(start: Option<f64>) -> Self {
        Self {
            last_saved: start.unwrap_or(0.0),
        }
    }

    /// Whether to save at `position`. Returns true once playback has moved
    /// [`SAVE_INTERVAL_SECS`] from the last save, in either direction (so a
    /// seek counts), and then counts from `position`.
    pub fn due(&mut self, position: f64) -> bool {
        if (position - self.last_saved).abs() >= SAVE_INTERVAL_SECS {
            self.last_saved = position;
            true
        } else {
            false
        }
    }

    /// Records a save made for another reason (pause, stop).
    pub fn saved(&mut self, position: f64) {
        self.last_saved = position;
    }
}

/// What the browser shows next to a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Marker {
    Unwatched,
    /// Started, not finished. `fraction` is how far along, if the duration
    /// is known.
    InProgress {
        position: f64,
        fraction: Option<f64>,
    },
    Watched,
}

pub fn marker(state: Option<&WatchState>) -> Marker {
    match state {
        Some(s) if s.watched => Marker::Watched,
        Some(s) if s.in_progress() => {
            let position = s.position.unwrap_or(0.0);
            let fraction = s
                .duration
                .filter(|d| *d > 0.0)
                .map(|d| (position / d).clamp(0.0, 1.0));
            Marker::InProgress { position, fraction }
        }
        _ => Marker::Unwatched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(position: Option<f64>, duration: Option<f64>, watched: bool) -> WatchState {
        WatchState {
            position,
            duration,
            watched,
        }
    }

    #[test]
    fn resume_decisions() {
        let start = |s: WatchState| start_position(Some(&s), 0.9);
        assert_eq!(start_position(None, 0.9), None);
        assert_eq!(start(state(Some(600.0), Some(1200.0), false)), Some(600.0));
        // Watched but started again: resume that.
        assert_eq!(start(state(Some(600.0), Some(1200.0), true)), Some(600.0));
        // Watched with no position (set_watched clears it): start over.
        assert_eq!(start(state(None, Some(1200.0), true)), None);
        // Past the threshold: start over.
        assert_eq!(start(state(Some(1150.0), Some(1200.0), false)), None);
        // Barely started: start over.
        assert_eq!(start(state(Some(2.0), Some(1200.0), false)), None);
        assert_eq!(start(state(None, None, false)), None);
    }

    #[test]
    fn saves_every_interval_and_after_seeks() {
        let mut s = SaveSchedule::new(None);
        assert!(!s.due(1.0));
        assert!(!s.due(9.9));
        assert!(s.due(10.0));
        assert!(!s.due(15.0));
        assert!(s.due(20.5));
        // Seek backwards far enough also saves.
        assert!(s.due(3.0));
        assert!(!s.due(4.0));
    }

    #[test]
    fn schedule_counts_from_resume_point_and_manual_saves() {
        let mut s = SaveSchedule::new(Some(600.0));
        assert!(!s.due(605.0));
        assert!(s.due(610.0));
        s.saved(612.0);
        assert!(!s.due(620.0));
        assert!(s.due(622.0));
    }

    #[test]
    fn markers() {
        assert_eq!(marker(None), Marker::Unwatched);
        assert_eq!(
            marker(Some(&state(Some(300.0), Some(1200.0), false))),
            Marker::InProgress {
                position: 300.0,
                fraction: Some(0.25)
            }
        );
        assert_eq!(
            marker(Some(&state(Some(300.0), None, false))),
            Marker::InProgress {
                position: 300.0,
                fraction: None
            }
        );
        assert_eq!(
            marker(Some(&state(None, Some(1200.0), true))),
            Marker::Watched
        );
        assert_eq!(marker(Some(&state(None, None, false))), Marker::Unwatched);
    }
}
