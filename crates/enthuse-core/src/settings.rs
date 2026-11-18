//! Playback settings with per-folder overrides. A folder uses its own
//! override if it has one, else its nearest parent's, else the global
//! default. Pure logic; [`crate::storage::Store`] loads and saves them.

use std::collections::HashMap;

use crate::source::SourcePath;

/// The settings that apply to a file, after overrides are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackSettings {
    /// Start files where they were left off.
    pub resume: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self { resume: true }
    }
}

/// A folder's own settings. `None` means "same as the parent folder".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderOverrides {
    pub resume: Option<bool>,
}

impl FolderOverrides {
    pub fn is_empty(&self) -> bool {
        self.resume.is_none()
    }
}

/// The settings for files in `folder`, given the global defaults and the
/// overrides stored for folders of the same source.
pub fn resolve(
    global: PlaybackSettings,
    folder: &SourcePath,
    overrides: &HashMap<SourcePath, FolderOverrides>,
) -> PlaybackSettings {
    let mut resume = None;
    let mut current = Some(folder.clone());
    while let Some(dir) = current {
        if let Some(o) = overrides.get(&dir) {
            resume = resume.or(o.resume);
        }
        if resume.is_some() {
            break;
        }
        current = dir.parent();
    }
    PlaybackSettings {
        resume: resume.unwrap_or(global.resume),
    }
}

/// How a folder override reads in Settings: the folder's name (the
/// source's name for its top folder), and a line with where it is and
/// what it sets, e.g. "NAS / Shows / Kids, resume off".
pub fn override_row(
    source_name: &str,
    folder: &SourcePath,
    o: &FolderOverrides,
) -> (String, String) {
    let title = folder.name().unwrap_or(source_name).to_owned();
    let place = std::iter::once(source_name)
        .chain(folder.segments())
        .collect::<Vec<_>>()
        .join(" / ");
    let what = match o.resume {
        Some(true) => "resume on",
        Some(false) => "resume off",
        None => "no own settings",
    };
    (title, format!("{place}, {what}"))
}

/// The next value when the user taps a three-way setting: inherit, then
/// off, then on, then inherit again.
pub fn cycle(value: Option<bool>) -> Option<bool> {
    match value {
        None => Some(false),
        Some(false) => Some(true),
        Some(true) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> SourcePath {
        SourcePath::parse(s).unwrap()
    }

    fn resume(v: bool) -> FolderOverrides {
        FolderOverrides { resume: Some(v) }
    }

    #[test]
    fn global_default_without_overrides() {
        let none = HashMap::new();
        let off = PlaybackSettings { resume: false };
        assert!(resolve(PlaybackSettings::default(), &p("Shows/A"), &none).resume);
        assert!(!resolve(off, &p("Shows/A"), &none).resume);
    }

    #[test]
    fn nearest_override_wins() {
        let mut o = HashMap::new();
        o.insert(SourcePath::root(), resume(false));
        o.insert(p("Shows"), resume(true));
        o.insert(p("Shows/Kids"), FolderOverrides::default());
        let g = PlaybackSettings::default();
        assert!(!resolve(g, &SourcePath::root(), &o).resume);
        assert!(!resolve(g, &p("Movies"), &o).resume);
        assert!(resolve(g, &p("Shows"), &o).resume);
        // An empty override inherits from further up.
        assert!(resolve(g, &p("Shows/Kids/S01"), &o).resume);
    }

    #[test]
    fn override_rows() {
        assert_eq!(
            override_row("NAS", &p("Shows/Kids"), &resume(false)),
            ("Kids".into(), "NAS / Shows / Kids, resume off".into())
        );
        assert_eq!(
            override_row("NAS", &SourcePath::root(), &resume(true)),
            ("NAS".into(), "NAS, resume on".into())
        );
    }

    #[test]
    fn cycles_through_three_states() {
        assert_eq!(cycle(None), Some(false));
        assert_eq!(cycle(Some(false)), Some(true));
        assert_eq!(cycle(Some(true)), None);
    }
}
