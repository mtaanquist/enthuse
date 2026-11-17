//! The Recent screen: recently played files, watched or not, most recent
//! first. Pure logic; [`crate::storage::Store::recently_played`] loads them.

use crate::folder::{Badge, display_title, episode_label};
use crate::source::SourcePath;
use crate::storage::WatchState;

/// Rows on the Recent screen.
pub const RECENT_LIMIT: usize = 50;

/// How a recently played file reads: the show (or cleaned-up file name)
/// and the episode, e.g. "Harbor Lights, S02E06 Lantern Rock".
pub fn title(file_name: &str) -> String {
    let show = display_title(file_name);
    let episode = episode_label(file_name);
    if episode.is_empty() {
        show
    } else if show.is_empty() || episode.starts_with(&show) {
        // No show name before the episode, e.g. "S01E02.mkv".
        episode
    } else {
        format!("{show}, {episode}")
    }
}

/// The badge for a played file: watched, or how much is left.
pub fn badge(state: &WatchState) -> Badge {
    if state.watched {
        return Badge::Watched;
    }
    match state.position.filter(|p| *p > 0.0) {
        Some(position) => {
            let duration = state.duration.filter(|d| *d > 0.0);
            Badge::Resume {
                position,
                fraction: duration.map(|d| (position / d).clamp(0.0, 1.0)),
                left: duration.map(|d| (d - position).max(0.0)),
            }
        }
        None => Badge::None,
    }
}

/// When, relative to `now` (both Unix seconds): "Just now", "5 min ago",
/// "3 h ago", "Yesterday", "4 days ago", "3 weeks ago", "5 months ago",
/// "2 years ago". A time in the future (a changed clock) is "Just now".
pub fn ago(played_at: i64, now: i64) -> String {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    let d = (now - played_at).max(0);
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match d {
        _ if d < MIN => "Just now".into(),
        _ if d < HOUR => format!("{} min ago", d / MIN),
        _ if d < DAY => format!("{} h ago", d / HOUR),
        _ if d < 2 * DAY => "Yesterday".into(),
        _ if d < 14 * DAY => plural(d / DAY, "day"),
        _ if d < 60 * DAY => plural(d / (7 * DAY), "week"),
        _ if d < 365 * DAY => plural(d / (30 * DAY), "month"),
        _ => plural(d / (365 * DAY), "year"),
    }
}

/// The line under the title: where the file is and when it was played,
/// e.g. "Harbor Lights / Season 2, 3 h ago".
pub fn detail(source_name: &str, path: &SourcePath, played_at: i64, now: i64) -> String {
    let folder = path.parent().unwrap_or_else(SourcePath::root);
    let place = std::iter::once(source_name)
        .chain(folder.segments())
        .collect::<Vec<_>>()
        .join(" / ");
    format!("{place}, {}", ago(played_at, now))
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
    fn titles() {
        assert_eq!(
            title("Harbor Lights - S02E06 - Lantern Rock.mkv"),
            "Harbor Lights, S02E06 Lantern Rock"
        );
        assert_eq!(title("The.Big.Film.2019.mkv"), "The Big Film 2019");
        assert_eq!(title("S01E02.mkv"), "S01E02");
    }

    #[test]
    fn badges() {
        assert_eq!(badge(&state(None, None, true)), Badge::Watched);
        assert_eq!(badge(&state(Some(100.0), Some(400.0), true)), Badge::Watched);
        assert_eq!(
            badge(&state(Some(100.0), Some(400.0), false)),
            Badge::Resume {
                position: 100.0,
                fraction: Some(0.25),
                left: Some(300.0)
            }
        );
        assert_eq!(
            badge(&state(Some(100.0), None, false)),
            Badge::Resume {
                position: 100.0,
                fraction: None,
                left: None
            }
        );
        assert_eq!(badge(&state(None, Some(400.0), false)), Badge::None);
    }

    #[test]
    fn relative_times() {
        let now = 1_000_000_000;
        assert_eq!(ago(now, now), "Just now");
        assert_eq!(ago(now + 100, now), "Just now");
        assert_eq!(ago(now - 59, now), "Just now");
        assert_eq!(ago(now - 5 * 60, now), "5 min ago");
        assert_eq!(ago(now - 3 * 3600, now), "3 h ago");
        assert_eq!(ago(now - 30 * 3600, now), "Yesterday");
        assert_eq!(ago(now - 4 * 86400, now), "4 days ago");
        assert_eq!(ago(now - 7 * 86400, now), "7 days ago");
        assert_eq!(ago(now - 15 * 86400, now), "2 weeks ago");
        assert_eq!(ago(now - 90 * 86400, now), "3 months ago");
        assert_eq!(ago(now - 400 * 86400, now), "1 year ago");
    }

    #[test]
    fn details() {
        let p = SourcePath::parse("Season 2/E05.mkv").unwrap();
        assert_eq!(
            detail("Harbor Lights", &p, 0, 3 * 3600),
            "Harbor Lights / Season 2, 3 h ago"
        );
        let top = SourcePath::parse("Film.mkv").unwrap();
        assert_eq!(detail("Movies", &top, 0, 0), "Movies, Just now");
    }
}
