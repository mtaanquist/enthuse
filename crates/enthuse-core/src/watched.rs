//! When a file counts as watched (#41): after a share of it has played,
//! when little enough is left, or whichever of the two comes first.
//! Reaching the end always counts; the app marks that separately.

/// Which test the [`WatchedRule`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WatchedMode {
    /// After `fraction` of the file has played.
    #[default]
    Percent,
    /// When at most `remaining` seconds are left.
    Remaining,
    /// Whichever of the two comes first.
    FirstOf,
}

impl WatchedMode {
    pub const ALL: [WatchedMode; 3] = [Self::Percent, Self::Remaining, Self::FirstOf];

    /// The value stored in the settings table.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Percent => "percent",
            Self::Remaining => "remaining",
            Self::FirstOf => "first",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }

    /// Position in [`Self::ALL`], for a segmented control.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

/// The time-left test never counts before this share of the file is left,
/// so "2 min left" does not mark a 3-minute clip watched after one minute.
/// Files over 8 minutes are not affected at 2 minutes.
pub const MAX_REMAINING_SHARE: f64 = 0.25;

/// Percentages offered in Settings.
pub const FRACTION_STEPS: [f64; 9] = [0.5, 0.6, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95, 0.98];
/// Times left offered in Settings, in seconds.
pub const REMAINING_STEPS: [f64; 10] = [
    30.0, 60.0, 90.0, 120.0, 180.0, 240.0, 300.0, 420.0, 600.0, 900.0,
];

/// The user's watched rule. Both values are kept whatever the mode, so
/// switching modes back and forth does not lose them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchedRule {
    pub mode: WatchedMode,
    /// Share of the file played, 0 to 1.
    pub fraction: f64,
    /// Seconds left.
    pub remaining: f64,
}

impl Default for WatchedRule {
    fn default() -> Self {
        Self {
            mode: WatchedMode::Percent,
            fraction: 0.9,
            remaining: 120.0,
        }
    }
}

impl WatchedRule {
    /// A rule that counts after `fraction` of the file.
    pub fn percent(fraction: f64) -> Self {
        Self {
            mode: WatchedMode::Percent,
            fraction,
            ..Self::default()
        }
    }

    /// Whether `position` of a file of `duration` counts as watched. Never
    /// without a known duration.
    pub fn reached(&self, position: f64, duration: Option<f64>) -> bool {
        let Some(d) = duration.filter(|d| *d > 0.0) else {
            return false;
        };
        let by_share = position / d >= self.fraction;
        let left = self.remaining.min(d * MAX_REMAINING_SHARE);
        let by_time = d - position <= left;
        match self.mode {
            WatchedMode::Percent => by_share,
            WatchedMode::Remaining => by_time,
            WatchedMode::FirstOf => by_share || by_time,
        }
    }

    /// One line for Settings, e.g. "At 90% or 2 min left, whichever comes first".
    pub fn summary(&self) -> String {
        let percent = percent_label(self.fraction);
        let left = remaining_label(self.remaining);
        match self.mode {
            WatchedMode::Percent => format!("After {percent} has played"),
            WatchedMode::Remaining => format!("When {left} or less is left"),
            WatchedMode::FirstOf => {
                format!("After {percent}, or with {left} left, whichever comes first")
            }
        }
    }
}

/// Whether `position` of `duration` counts as watched under `rule`.
pub fn reaches_threshold(position: f64, duration: Option<f64>, rule: &WatchedRule) -> bool {
    rule.reached(position, duration)
}

/// The value next to `current` in `steps`, up (`dir > 0`) or down. A value
/// between steps goes to the nearest step in that direction; the ends stay.
pub fn step(steps: &[f64], current: f64, dir: i32) -> f64 {
    const EPS: f64 = 1e-9;
    let found = if dir > 0 {
        steps.iter().find(|s| **s > current + EPS)
    } else {
        steps.iter().rev().find(|s| **s < current - EPS)
    };
    found.copied().unwrap_or(current)
}

/// "90%".
pub fn percent_label(fraction: f64) -> String {
    format!("{}%", (fraction * 100.0).round() as i64)
}

/// "30 s", "1.5 min", "2 min".
pub fn remaining_label(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 60.0 {
        return format!("{} s", seconds.round() as i64);
    }
    let minutes = (seconds / 30.0).round() / 2.0;
    if minutes.fract() == 0.0 {
        format!("{} min", minutes as i64)
    } else {
        format!("{minutes:.1} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOURS_3: f64 = 3.0 * 3600.0;
    const EPISODE: f64 = 22.0 * 60.0;

    fn rule(mode: WatchedMode) -> WatchedRule {
        WatchedRule {
            mode,
            fraction: 0.9,
            remaining: 120.0,
        }
    }

    #[test]
    fn unknown_or_zero_duration_never_counts() {
        for mode in WatchedMode::ALL {
            assert!(!rule(mode).reached(100.0, None));
            assert!(!rule(mode).reached(100.0, Some(0.0)));
        }
    }

    #[test]
    fn percent() {
        let r = rule(WatchedMode::Percent);
        assert!(r.reached(90.0, Some(100.0)));
        assert!(!r.reached(89.9, Some(100.0)));
        // 3 hours: 90% leaves 18 minutes, which is what #41 is about.
        assert!(r.reached(HOURS_3 - 18.0 * 60.0, Some(HOURS_3)));
    }

    #[test]
    fn time_left_on_long_and_normal_files() {
        let r = rule(WatchedMode::Remaining);
        assert!(!r.reached(HOURS_3 - 18.0 * 60.0, Some(HOURS_3)));
        assert!(!r.reached(HOURS_3 - 121.0, Some(HOURS_3)));
        assert!(r.reached(HOURS_3 - 120.0, Some(HOURS_3)));
        assert!(r.reached(EPISODE - 100.0, Some(EPISODE)));
        assert!(!r.reached(EPISODE - 200.0, Some(EPISODE)));
    }

    #[test]
    fn time_left_on_short_files_needs_three_quarters() {
        let r = rule(WatchedMode::Remaining);
        // 3 minutes: 2 min left would be after one minute; it waits for 45 s left.
        assert!(!r.reached(60.0, Some(180.0)));
        assert!(!r.reached(134.0, Some(180.0)));
        assert!(r.reached(135.0, Some(180.0)));
        // 8 minutes and longer: the full 2 minutes apply.
        assert!(r.reached(360.0, Some(480.0)));
    }

    #[test]
    fn whichever_comes_first() {
        let r = rule(WatchedMode::FirstOf);
        // Short episode: 90% comes first.
        assert!(r.reached(EPISODE * 0.9, Some(EPISODE)));
        // 3 hours: 90% also comes first, at 18 minutes left.
        assert!(r.reached(HOURS_3 * 0.9, Some(HOURS_3)));
        // With 98%, 2 min left wins on a 22-minute episode.
        let r = WatchedRule {
            fraction: 0.98,
            ..r
        };
        assert!(r.reached(EPISODE - 120.0, Some(EPISODE)));
        assert!(!r.reached(EPISODE - 121.0, Some(EPISODE)));
    }

    #[test]
    fn modes_round_trip() {
        for (i, mode) in WatchedMode::ALL.into_iter().enumerate() {
            assert_eq!(WatchedMode::parse(mode.as_str()), Some(mode));
            assert_eq!(mode.index(), i);
            assert_eq!(WatchedMode::from_index(i), mode);
        }
        assert_eq!(WatchedMode::parse("nope"), None);
        assert_eq!(WatchedMode::from_index(9), WatchedMode::Percent);
    }

    #[test]
    fn steps_move_and_stop_at_the_ends() {
        assert_eq!(step(&FRACTION_STEPS, 0.9, 1), 0.95);
        assert_eq!(step(&FRACTION_STEPS, 0.9, -1), 0.85);
        assert_eq!(step(&FRACTION_STEPS, 0.98, 1), 0.98);
        assert_eq!(step(&FRACTION_STEPS, 0.5, -1), 0.5);
        // Between steps (e.g. stored by hand): to the nearest one that way.
        assert_eq!(step(&FRACTION_STEPS, 0.93, 1), 0.95);
        assert_eq!(step(&FRACTION_STEPS, 0.93, -1), 0.9);
        assert_eq!(step(&REMAINING_STEPS, 120.0, 1), 180.0);
    }

    #[test]
    fn labels() {
        assert_eq!(percent_label(0.9), "90%");
        assert_eq!(percent_label(0.75), "75%");
        assert_eq!(remaining_label(30.0), "30 s");
        assert_eq!(remaining_label(90.0), "1.5 min");
        assert_eq!(remaining_label(120.0), "2 min");
        assert_eq!(rule(WatchedMode::Percent).summary(), "After 90% has played");
        assert_eq!(
            rule(WatchedMode::Remaining).summary(),
            "When 2 min or less is left"
        );
    }
}
