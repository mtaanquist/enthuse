//! Small pieces of player control logic: speeds and volume steps.

/// Playback speeds offered in the speed menu.
pub const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

/// How much the up and down keys change the volume, in percent.
pub const VOLUME_STEP: f64 = 5.0;

/// Label for a speed: `1x`, `1.25x`, `0.5x`.
pub fn speed_label(speed: f64) -> String {
    let rounded = (speed * 100.0).round() / 100.0;
    format!("{rounded}x")
}

/// The volume after a step of `delta` percent, kept within 0 to 100.
pub fn step_volume(volume: f64, delta: f64) -> f64 {
    (volume + delta).clamp(0.0, 100.0)
}

/// Remaining time as shown at the right end of the scrubber: `-31:56`.
pub fn remaining_label(position: f64, duration: f64) -> String {
    format!(
        "-{}",
        crate::time::format_time((duration - position).max(0.0))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_labels() {
        assert_eq!(speed_label(1.0), "1x");
        assert_eq!(speed_label(1.25), "1.25x");
        assert_eq!(speed_label(0.5), "0.5x");
        assert_eq!(speed_label(1.2500001), "1.25x");
    }

    #[test]
    fn volume_steps_stay_in_range() {
        assert_eq!(step_volume(50.0, VOLUME_STEP), 55.0);
        assert_eq!(step_volume(98.0, VOLUME_STEP), 100.0);
        assert_eq!(step_volume(2.0, -VOLUME_STEP), 0.0);
    }

    #[test]
    fn remaining() {
        assert_eq!(remaining_label(724.0, 2640.0), "-31:56");
        assert_eq!(remaining_label(50.0, 40.0), "-0:00");
    }
}
