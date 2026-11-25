//! Telling a real end of file from a file that was cut off (#57).
//!
//! When a network share drops mid-file, reads fail and the player sees the
//! end of the data, which it reports as a normal end of file. Taking that
//! at face value would mark a half-watched file watched, drop its resume
//! point and start the next file on the same dead share.
//!
//! So an end of file well before the duration is checked: if the file can
//! no longer be reached, or it changed, playback was interrupted. If it is
//! still there as it was, the end was real and the duration was wrong (some
//! files have broken headers).

use crate::source::FileStamp;

/// An end of file more than this many seconds before the duration is
/// checked. Real ends report a position within a frame or two of the end.
pub const EARLY_END_MARGIN: f64 = 5.0;

/// Whether an end of file at `position` is too early for a file of
/// `duration` and should be checked with [`interrupted`]. Without a known
/// position and duration there is nothing to go on, so it is not.
pub fn ended_early(position: Option<f64>, duration: Option<f64>) -> bool {
    match (position, duration) {
        (Some(p), Some(d)) if d.is_finite() && d > 0.0 => p < d - EARLY_END_MARGIN,
        _ => false,
    }
}

/// Whether a file that ended early was cut off: it can't be reached now
/// (`now` is `None`), or its size or time changed since it was opened.
/// `before` is `None` if it wasn't known when the file was opened.
pub fn interrupted(before: Option<FileStamp>, now: Option<FileStamp>) -> bool {
    match (before, now) {
        (_, None) => true,
        (Some(before), Some(now)) => before != now,
        (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(size: u64) -> Option<FileStamp> {
        Some(FileStamp { size, modified: 1 })
    }

    #[test]
    fn ends_near_the_duration_are_real() {
        assert!(!ended_early(Some(59.96), Some(60.0)));
        assert!(!ended_early(Some(55.5), Some(60.0)));
        assert!(ended_early(Some(11.9), Some(60.0)));
        assert!(ended_early(Some(0.0), Some(3600.0)));
    }

    #[test]
    fn unknown_position_or_duration_is_not_early() {
        assert!(!ended_early(None, Some(60.0)));
        assert!(!ended_early(Some(10.0), None));
        assert!(!ended_early(Some(10.0), Some(0.0)));
        assert!(!ended_early(Some(10.0), Some(f64::INFINITY)));
    }

    #[test]
    fn gone_or_changed_files_were_interrupted() {
        assert!(interrupted(stamp(100), None));
        assert!(interrupted(None, None));
        assert!(interrupted(stamp(100), stamp(20)));
        assert!(!interrupted(stamp(100), stamp(100)));
        assert!(!interrupted(None, stamp(100)));
    }
}
