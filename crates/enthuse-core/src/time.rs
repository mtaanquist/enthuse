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

#[cfg(test)]
mod tests {
    use super::format_time;

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
