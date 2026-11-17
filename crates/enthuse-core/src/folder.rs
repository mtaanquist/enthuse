//! What the folder browser shows about a folder: a badge per video, a
//! summary line, the primary action ("Resume", "Play next") and episode
//! labels parsed from file names. Pure logic over a listing and the stored
//! watch states, so the UI only displays the result.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::natsort::natural_cmp;
use crate::resume::{Marker, marker};
use crate::source::{Entry, EntryKind, SourcePath};
use crate::storage::WatchState;
use crate::time::format_minutes;

/// An episode number parsed from a file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Episode {
    pub season: Option<u32>,
    pub episode: u32,
}

impl Episode {
    /// Short form: `S02E05`, or `E06` when the season is not known.
    pub fn code(&self) -> String {
        match self.season {
            Some(s) => format!("S{s:02}E{:02}", self.episode),
            None => format!("E{:02}", self.episode),
        }
    }
}

/// Finds an episode number in a file name: `S02E05`, `s2e5`, `2x05`,
/// `E06` or `Episode 6`. Years and resolutions (`1080p`, `x264`) are not
/// mistaken for episodes.
pub fn parse_episode(name: &str) -> Option<Episode> {
    let lower = name.to_ascii_lowercase();
    let b = lower.as_bytes();
    let is_word_start = |i: usize| i == 0 || !b[i - 1].is_ascii_alphanumeric();
    let digits_at = |i: usize| -> (Option<u32>, usize) {
        let end = (i..b.len())
            .find(|&j| !b[j].is_ascii_digit())
            .unwrap_or(b.len());
        if end == i || end - i > 4 {
            return (None, end);
        }
        (lower[i..end].parse().ok(), end)
    };

    // S02E05, s2e5, S02.E05
    for i in 0..b.len() {
        if b[i] == b's' && is_word_start(i) {
            let (season, end) = digits_at(i + 1);
            let mut j = end;
            if j < b.len() && (b[j] == b'.' || b[j] == b' ' || b[j] == b'_') {
                j += 1;
            }
            if let Some(season) = season
                && j < b.len()
                && b[j] == b'e'
                && let (Some(episode), _) = digits_at(j + 1)
            {
                return Some(Episode {
                    season: Some(season),
                    episode,
                });
            }
        }
    }
    // 2x05 (not x264, which has no season digits before it)
    for i in 1..b.len() {
        if b[i] == b'x' && b[i - 1].is_ascii_digit() {
            let start = (0..i)
                .rev()
                .find(|&j| !b[j].is_ascii_digit())
                .map_or(0, |j| j + 1);
            if !is_word_start(start) || i - start > 2 {
                continue;
            }
            let (episode, end) = digits_at(i + 1);
            let ends_word = end == b.len() || !b[end].is_ascii_alphanumeric();
            if let (Some(episode), true) = (episode, ends_word)
                && end - (i + 1) >= 2
            {
                return Some(Episode {
                    season: lower[start..i].parse().ok(),
                    episode,
                });
            }
        }
    }
    // Episode 6, Ep 6, Ep.06, E06
    for prefix in ["episode", "ep", "e"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(prefix) {
            let i = from + pos;
            from = i + 1;
            if !is_word_start(i) {
                continue;
            }
            let mut j = i + prefix.len();
            // A bare "E" needs its digits right after it ("E06"), or titles
            // like "Formula E 2023" would count.
            if prefix != "e" {
                while j < b.len() && (b[j] == b' ' || b[j] == b'.' || b[j] == b'_') {
                    j += 1;
                }
            }
            let (episode, end) = digits_at(j);
            let ends_word = end == b.len() || !b[end].is_ascii_alphanumeric();
            // Four digits from 1900 on are a year ("Wall-E 2008"), not an episode.
            let year = end - j == 4 && episode.is_some_and(|n| (1900..2100).contains(&n));
            if let (Some(episode), true, false) = (episode, ends_word, year) {
                return Some(Episode {
                    season: None,
                    episode,
                });
            }
        }
    }
    None
}

/// A title for a file in lists outside its folder, such as "Continue
/// watching": the part of the name before the episode number
/// ("Harbor Lights" for `Harbor.Lights.S02E05.1080p.mkv`), or the file name
/// without its extension. Dots and underscores read as spaces.
pub fn display_title(file_name: &str) -> String {
    let stem = file_name
        .rsplit_once('.')
        .filter(|(stem, ext)| !stem.is_empty() && ext.len() <= 4)
        .map_or(file_name, |(stem, _)| stem);
    let spaced: String = stem
        .chars()
        .map(|c| if c == '.' || c == '_' { ' ' } else { c })
        .collect();
    if let Some(at) = episode_start(&spaced) {
        let show = spaced[..at].trim_end_matches([' ', '-', '(', '[']).trim();
        if !show.is_empty() {
            return show.to_owned();
        }
    }
    spaced.trim().to_owned()
}

/// The line under a "Continue watching" card: the episode, if the name has
/// one, and the time left ("S02E05, 31 min left"), or the resume time when
/// the duration is unknown.
pub fn continue_subtitle(file_name: &str, state: &WatchState) -> String {
    let position = state.position.unwrap_or(0.0);
    let left = state
        .duration
        .filter(|d| *d > 0.0)
        .map(|d| (d - position).max(0.0));
    let time = Badge::Resume {
        position,
        fraction: None,
        left,
    }
    .label();
    match parse_episode(file_name) {
        Some(e) => format!("{}, {time}", e.code()),
        None => time,
    }
}

/// How far along a started file is (0 to 1), or 0 when the duration is not
/// known.
pub fn progress(state: &WatchState) -> f64 {
    match (state.position, state.duration) {
        (Some(p), Some(d)) if d > 0.0 => (p / d).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Byte index where the episode number starts in `name`, if it has one.
fn episode_start(name: &str) -> Option<usize> {
    parse_episode(name)?;
    let lower = name.to_ascii_lowercase();
    // Shrink the name from the right while it still has an episode number:
    // the first word that loses it is where the number starts.
    let words: Vec<usize> = lower
        .char_indices()
        .filter(|&(i, c)| {
            c.is_ascii_alphanumeric()
                && (i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric())
        })
        .map(|(i, _)| i)
        .collect();
    words
        .iter()
        .rev()
        .find(|&&i| parse_episode(&lower[i..]).is_some())
        .copied()
}

/// The state badge on a video row.
#[derive(Debug, Clone, PartialEq)]
pub enum Badge {
    None,
    Watched,
    /// Started and not finished. `fraction` is how far along and `left`
    /// the time left, when the duration is known.
    Resume {
        position: f64,
        fraction: Option<f64>,
        left: Option<f64>,
    },
    /// Never started, in a folder where something has been played: the
    /// next files of a show being followed. A folder nobody has started
    /// shows no badges at all, so it isn't a wall of "New".
    New,
}

impl Badge {
    /// Text next to the badge: "Watched", "31 min left", "New".
    pub fn label(&self) -> String {
        match self {
            Badge::None => String::new(),
            Badge::Watched => "Watched".into(),
            Badge::Resume { left: Some(l), .. } => format!("{} left", format_minutes(*l)),
            Badge::Resume { position, .. } => {
                format!("Resume at {}", crate::time::format_time(*position))
            }
            Badge::New => "New".into(),
        }
    }
}

/// Sort orders offered in the browser. Folders always come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    NameAscending,
    NameDescending,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            SortOrder::NameAscending => SortOrder::NameDescending,
            SortOrder::NameDescending => SortOrder::NameAscending,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::NameAscending => "Sort: Name",
            SortOrder::NameDescending => "Sort: Name, Z to A",
        }
    }

    pub fn compare(self, a: &Entry, b: &Entry) -> Ordering {
        let rank = |e: &Entry| match e.kind {
            EntryKind::Folder => 0,
            EntryKind::Video => 1,
        };
        let by_name = natural_cmp(&a.name, &b.name);
        rank(a).cmp(&rank(b)).then(match self {
            SortOrder::NameAscending => by_name,
            SortOrder::NameDescending => by_name.reverse(),
        })
    }
}

/// The main button above the file list.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimaryAction {
    /// Resume the file at this index in the listing.
    Resume(usize),
    /// Start the first unwatched file at this index.
    PlayNext(usize),
    /// Everything is watched: start the first video.
    PlayFromStart(usize),
}

impl PrimaryAction {
    pub fn index(&self) -> usize {
        match self {
            PrimaryAction::Resume(i)
            | PrimaryAction::PlayNext(i)
            | PrimaryAction::PlayFromStart(i) => *i,
        }
    }
}

/// Everything the browser shows for one folder.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderView {
    /// One badge per entry, in listing order (`Badge::None` for folders).
    pub badges: Vec<Badge>,
    pub videos: usize,
    pub watched: usize,
    pub folders: usize,
    /// Whether every video has an episode number, so the summary can say
    /// "episodes".
    pub all_episodes: bool,
    pub action: Option<PrimaryAction>,
}

impl FolderView {
    /// Works out badges, counts and the primary action for `entries` (in
    /// display order). With `resume` off, started files count as not
    /// started for the action, since playing them starts from the top.
    pub fn new(entries: &[Entry], states: &HashMap<SourcePath, WatchState>, resume: bool) -> Self {
        let markers: Vec<Option<Marker>> = entries
            .iter()
            .map(|e| (e.kind == EntryKind::Video).then(|| marker(states.get(&e.path))))
            .collect();
        let followed = markers
            .iter()
            .flatten()
            .any(|m| !matches!(m, Marker::Unwatched));
        let badges = markers
            .iter()
            .zip(entries)
            .map(|(m, e)| match m {
                None => Badge::None,
                Some(Marker::Watched) => Badge::Watched,
                Some(Marker::InProgress { position, fraction }) => Badge::Resume {
                    position: *position,
                    fraction: *fraction,
                    left: states
                        .get(&e.path)
                        .and_then(|s| s.duration)
                        .filter(|d| *d > 0.0)
                        .map(|d| (d - position).max(0.0)),
                },
                Some(Marker::Unwatched) if followed => Badge::New,
                Some(Marker::Unwatched) => Badge::None,
            })
            .collect();

        // The action follows the episodes' natural order, whatever order the
        // list is shown in.
        let mut videos: Vec<usize> = (0..entries.len())
            .filter(|&i| entries[i].kind == EntryKind::Video)
            .collect();
        videos.sort_by(|&a, &b| SortOrder::NameAscending.compare(&entries[a], &entries[b]));
        let watched = videos
            .iter()
            .filter(|&&i| markers[i] == Some(Marker::Watched))
            .count();
        let in_progress = |i: &&usize| matches!(markers[**i], Some(Marker::InProgress { .. }));
        let action = if let Some(&i) = videos.iter().find(in_progress).filter(|_| resume) {
            Some(PrimaryAction::Resume(i))
        } else if let Some(&i) = videos
            .iter()
            .find(|&&i| markers[i] != Some(Marker::Watched))
        {
            Some(PrimaryAction::PlayNext(i))
        } else {
            videos.first().map(|&i| PrimaryAction::PlayFromStart(i))
        };

        Self {
            badges,
            videos: videos.len(),
            watched,
            folders: entries.len() - videos.len(),
            all_episodes: !videos.is_empty()
                && videos
                    .iter()
                    .all(|&i| parse_episode(&entries[i].name).is_some()),
            action,
        }
    }

    /// "8 episodes, 4 watched", "3 folders, 12 videos", "No videos".
    pub fn summary(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if self.folders > 0 {
            parts.push(plural(self.folders, "folder", "folders"));
        }
        if self.videos > 0 {
            parts.push(if self.all_episodes {
                plural(self.videos, "episode", "episodes")
            } else {
                plural(self.videos, "video", "videos")
            });
            if self.watched > 0 {
                parts.push(if self.watched == self.videos {
                    "all watched".to_owned()
                } else {
                    format!("{} watched", self.watched)
                });
            }
        }
        if parts.is_empty() {
            return "No videos".into();
        }
        parts.join(", ")
    }

    /// Button text for the primary action, using episode numbers when the
    /// file names have them.
    pub fn action_label(&self, entries: &[Entry]) -> Option<String> {
        let action = self.action.as_ref()?;
        let episode = entries
            .get(action.index())
            .and_then(|e| parse_episode(&e.name));
        Some(match (action, episode) {
            (PrimaryAction::Resume(_), Some(ep)) => format!("Resume episode {}", ep.episode),
            (PrimaryAction::Resume(_), None) => "Resume".into(),
            (PrimaryAction::PlayNext(_), Some(ep)) => format!("Play next: {}", ep.code()),
            (PrimaryAction::PlayNext(_), None) => "Play next".into(),
            (PrimaryAction::PlayFromStart(_), _) => "Play from start".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(season: Option<u32>, episode: u32) -> Option<Episode> {
        Some(Episode { season, episode })
    }

    #[test]
    fn parses_episode_numbers() {
        assert_eq!(
            parse_episode("Harbor.Lights.S02E05.1080p.mkv"),
            ep(Some(2), 5)
        );
        assert_eq!(parse_episode("show s1e12 x264.mkv"), ep(Some(1), 12));
        assert_eq!(parse_episode("Show S03.E07.mkv"), ep(Some(3), 7));
        assert_eq!(parse_episode("Show 2x05 Title.avi"), ep(Some(2), 5));
        assert_eq!(parse_episode("E06 Lantern Rock.mkv"), ep(None, 6));
        assert_eq!(parse_episode("Episode 6.mp4"), ep(None, 6));
        assert_eq!(parse_episode("Ep.03 - Title.mp4"), ep(None, 3));
        assert_eq!(parse_episode("Movie (2020) 1080p x264.mkv"), None);
        assert_eq!(parse_episode("Mexico 1986.mkv"), None);
        assert_eq!(parse_episode("Some Film.mkv"), None);
        assert_eq!(parse_episode("Series Finale.mkv"), None);
        assert_eq!(parse_episode("Wall-E 2008.mkv"), None);
        assert_eq!(parse_episode("Formula E 2023 Round 05.mkv"), None);
        assert_eq!(parse_episode("Episode 2008.mkv"), None);
        assert_eq!(parse_episode("E 06.mkv"), None);
        assert_eq!(
            Episode {
                season: Some(2),
                episode: 5
            }
            .code(),
            "S02E05"
        );
        assert_eq!(
            Episode {
                season: None,
                episode: 6
            }
            .code(),
            "E06"
        );
    }

    #[test]
    fn display_titles() {
        assert_eq!(
            display_title("Harbor.Lights.S02E05.1080p.mkv"),
            "Harbor Lights"
        );
        assert_eq!(
            display_title("Harbor Lights - S02E05 - The Long Tide.mkv"),
            "Harbor Lights"
        );
        assert_eq!(
            display_title("The Quiet Orbit (2021).mkv"),
            "The Quiet Orbit (2021)"
        );
        assert_eq!(display_title("E06 Lantern Rock.mkv"), "E06 Lantern Rock");
        assert_eq!(display_title("Field_Notes_2x03.mp4"), "Field Notes");
        assert_eq!(display_title("no extension"), "no extension");
    }

    #[test]
    fn continue_subtitles() {
        let state = |position, duration| WatchState {
            position: Some(position),
            duration,
            watched: false,
        };
        assert_eq!(
            continue_subtitle("Harbor.Lights.S02E05.mkv", &state(600.0, Some(2460.0))),
            "S02E05, 31 min left"
        );
        assert_eq!(
            continue_subtitle("Some Film (2019).mkv", &state(65.0, None)),
            "Resume at 1:05"
        );
        assert_eq!(progress(&state(600.0, Some(2400.0))), 0.25);
        assert_eq!(progress(&state(600.0, None)), 0.0);
    }

    fn video(name: &str) -> Entry {
        Entry {
            name: name.into(),
            path: SourcePath::parse(name).unwrap(),
            kind: EntryKind::Video,
        }
    }

    fn folder(name: &str) -> Entry {
        Entry {
            name: name.into(),
            path: SourcePath::parse(name).unwrap(),
            kind: EntryKind::Folder,
        }
    }

    fn state(position: f64, duration: f64, watched: bool) -> WatchState {
        WatchState {
            position: Some(position),
            duration: Some(duration),
            watched,
        }
    }

    fn season() -> Vec<Entry> {
        (1..=8).map(|i| video(&format!("S02E{i:02}.mkv"))).collect()
    }

    #[test]
    fn untouched_folder_has_no_badges_and_plays_first() {
        let entries = season();
        let view = FolderView::new(&entries, &HashMap::new(), true);
        assert!(view.badges.iter().all(|b| *b == Badge::None));
        assert_eq!(view.action, Some(PrimaryAction::PlayNext(0)));
        assert_eq!(view.action_label(&entries).unwrap(), "Play next: S02E01");
        assert_eq!(view.summary(), "8 episodes");
    }

    #[test]
    fn followed_show_resumes_and_marks_new() {
        let entries = season();
        let mut states = HashMap::new();
        for e in &entries[..4] {
            states.insert(e.path.clone(), state(2600.0, 2640.0, true));
        }
        states.insert(entries[4].path.clone(), state(780.0, 2640.0, false));
        let view = FolderView::new(&entries, &states, true);
        assert_eq!(view.badges[0], Badge::Watched);
        assert_eq!(view.badges[4].label(), "31 min left");
        assert_eq!(view.badges[5], Badge::New);
        assert_eq!(view.action, Some(PrimaryAction::Resume(4)));
        assert_eq!(view.action_label(&entries).unwrap(), "Resume episode 5");
        assert_eq!(view.summary(), "8 episodes, 4 watched");

        // With resume off the started file plays from the top: "Play next".
        let view = FolderView::new(&entries, &states, false);
        assert_eq!(view.action, Some(PrimaryAction::PlayNext(4)));
        assert_eq!(view.action_label(&entries).unwrap(), "Play next: S02E05");
    }

    #[test]
    fn action_follows_natural_order_when_sorted_descending() {
        let mut entries = season();
        entries.sort_by(|a, b| SortOrder::NameDescending.compare(a, b));
        let view = FolderView::new(&entries, &HashMap::new(), true);
        // S02E01 is last in the list but still the one to play next.
        assert_eq!(view.action, Some(PrimaryAction::PlayNext(7)));
        assert_eq!(view.action_label(&entries).unwrap(), "Play next: S02E01");
    }

    #[test]
    fn all_watched_plays_from_start() {
        let entries = vec![folder("Extras"), video("a.mkv"), video("b.mkv")];
        let states = entries[1..]
            .iter()
            .map(|e| (e.path.clone(), state(10.0, 10.0, true)))
            .collect();
        let view = FolderView::new(&entries, &states, true);
        assert_eq!(view.badges[0], Badge::None);
        assert_eq!(view.action, Some(PrimaryAction::PlayFromStart(1)));
        assert_eq!(view.action_label(&entries).unwrap(), "Play from start");
        assert_eq!(view.summary(), "1 folder, 2 videos, all watched");
    }

    #[test]
    fn folders_only_have_no_action() {
        let entries = vec![folder("Season 1"), folder("Season 2")];
        let view = FolderView::new(&entries, &HashMap::new(), true);
        assert_eq!(view.action, None);
        assert_eq!(view.action_label(&entries), None);
        assert_eq!(view.summary(), "2 folders");
        assert_eq!(
            FolderView::new(&[], &HashMap::new(), true).summary(),
            "No videos"
        );
    }

    #[test]
    fn resume_without_duration_shows_position() {
        let entries = vec![video("film.mkv")];
        let states = HashMap::from([(
            entries[0].path.clone(),
            WatchState {
                position: Some(754.0),
                duration: None,
                watched: false,
            },
        )]);
        let view = FolderView::new(&entries, &states, true);
        assert_eq!(view.badges[0].label(), "Resume at 12:34");
        assert_eq!(view.action_label(&entries).unwrap(), "Resume");
    }

    #[test]
    fn sorts_folders_first_both_ways() {
        let mut entries = [video("E10.mkv"), folder("b"), video("E9.mkv"), folder("a")];
        entries.sort_by(|a, b| SortOrder::NameAscending.compare(a, b));
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "E9.mkv", "E10.mkv"]);
        entries.sort_by(|a, b| SortOrder::NameDescending.compare(a, b));
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "E10.mkv", "E9.mkv"]);
        assert_eq!(SortOrder::default().next().next(), SortOrder::NameAscending);
    }
}
