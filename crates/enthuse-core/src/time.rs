//! Formatting of playback times.

/// Formats seconds as `m:ss`, or `h:mm:ss` from one hour. Negative or
/// non-finite values show as `0:00`.
pub fn format_time(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds.floor() as u64
    } else {
        0
    };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Formats a length for labels, rounded to whole minutes: `44 min`,
/// `1 h 12 min`, `2 h`. Anything under a minute but above zero shows as
/// `1 min`, so a label never says `0 min` while time is left.
pub fn format_minutes(seconds: f64) -> String {
    let minutes = if seconds.is_finite() && seconds > 0.0 {
        ((seconds / 60.0).round() as u64).max(1)
    } else {
        0
    };
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::{format_minutes, format_time};

    #[test]
    fn formats_minutes() {
        assert_eq!(format_minutes(0.0), "0 min");
        assert_eq!(format_minutes(f64::NAN), "0 min");
        assert_eq!(format_minutes(10.0), "1 min");
        assert_eq!(format_minutes(44.0 * 60.0 + 20.0), "44 min");
        assert_eq!(format_minutes(3600.0), "1 h");
        assert_eq!(format_minutes(72.0 * 60.0), "1 h 12 min");
    }

    #[test]
    fn formats() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(5.9), "0:05");
        assert_eq!(format_time(65.0), "1:05");
        assert_eq!(format_time(3599.0), "59:59");
        assert_eq!(format_time(3600.0), "1:00:00");
        assert_eq!(format_time(36_061.0), "10:01:01");
        assert_eq!(format_time(-3.0), "0:00");
        assert_eq!(format_time(f64::NAN), "0:00");
    }
}
