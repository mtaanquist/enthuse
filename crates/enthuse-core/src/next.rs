//! Choosing what plays after the current file: the next video in the same
//! folder, in the same order the browser shows.

use crate::natsort::natural_cmp;
use crate::source::{EntryKind, Source, SourceError, SourcePath};

/// Returns the video that follows `current` in its folder, or `None` at the
/// end of the folder.
///
/// The folder is listed again rather than relying on an earlier listing, so
/// files added or removed during playback are taken into account. `current`
/// itself does not need to exist any more.
pub fn next_video(
    source: &dyn Source,
    current: &SourcePath,
) -> Result<Option<SourcePath>, SourceError> {
    let Some(name) = current.name() else {
        return Ok(None); // The root is not a file.
    };
    let folder = current.parent().unwrap_or_default();
    let entries = source.list(&folder)?;
    Ok(entries
        .into_iter()
        .filter(|e| e.kind == EntryKind::Video)
        .find(|e| natural_cmp(&e.name, name).is_gt())
        .map(|e| e.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Entry, entry_order};
    use std::collections::HashMap;

    /// In-memory source: folder path -> entries.
    struct FakeSource(HashMap<String, Vec<(&'static str, EntryKind)>>);

    impl FakeSource {
        fn with(folder: &str, entries: &[(&'static str, EntryKind)]) -> Self {
            Self(HashMap::from([(folder.to_owned(), entries.to_vec())]))
        }
    }

    impl Source for FakeSource {
        fn display_name(&self) -> String {
            "fake".into()
        }

        fn list(&self, dir: &SourcePath) -> Result<Vec<Entry>, SourceError> {
            let items = self
                .0
                .get(dir.as_str())
                .ok_or_else(|| SourceError::NotFound(dir.clone()))?;
            let mut entries: Vec<Entry> = items
                .iter()
                .map(|(name, kind)| Entry {
                    name: (*name).to_owned(),
                    path: dir.join(name).unwrap(),
                    kind: *kind,
                })
                .collect();
            entries.sort_by(entry_order);
            Ok(entries)
        }

        fn playable(&self, file: &SourcePath) -> Result<String, SourceError> {
            Ok(file.to_string())
        }
    }

    use EntryKind::{Folder, Video};

    fn next(src: &FakeSource, current: &str) -> Option<String> {
        next_video(src, &SourcePath::parse(current).unwrap())
            .unwrap()
            .map(|p| p.as_str().to_owned())
    }

    #[test]
    fn picks_next_in_natural_order() {
        let src = FakeSource::with(
            "Show",
            &[
                ("Ep 10.mkv", Video),
                ("Ep 2.mkv", Video),
                ("Ep 1.mkv", Video),
                ("Ep 9.mkv", Video),
            ],
        );
        assert_eq!(
            next(&src, "Show/Ep 1.mkv").as_deref(),
            Some("Show/Ep 2.mkv")
        );
        assert_eq!(
            next(&src, "Show/Ep 2.mkv").as_deref(),
            Some("Show/Ep 9.mkv")
        );
        assert_eq!(
            next(&src, "Show/Ep 9.mkv").as_deref(),
            Some("Show/Ep 10.mkv")
        );
    }

    #[test]
    fn none_after_last_file() {
        let src = FakeSource::with("Show", &[("Ep 1.mkv", Video), ("Ep 2.mkv", Video)]);
        assert_eq!(next(&src, "Show/Ep 2.mkv"), None);
    }

    #[test]
    fn skips_folders() {
        let src = FakeSource::with(
            "Show",
            &[
                ("Ep 1.mkv", Video),
                ("Extras", Folder),
                ("Zzz", Folder),
                ("Ep 2.mkv", Video),
            ],
        );
        assert_eq!(
            next(&src, "Show/Ep 1.mkv").as_deref(),
            Some("Show/Ep 2.mkv")
        );
        assert_eq!(next(&src, "Show/Ep 2.mkv"), None);
    }

    #[test]
    fn current_file_deleted_during_playback() {
        let src = FakeSource::with("Show", &[("Ep 1.mkv", Video), ("Ep 3.mkv", Video)]);
        assert_eq!(
            next(&src, "Show/Ep 2.mkv").as_deref(),
            Some("Show/Ep 3.mkv")
        );
    }

    #[test]
    fn mixed_case_and_season_codes() {
        let src = FakeSource::with(
            "",
            &[
                ("show s01e10.mkv", Video),
                ("Show S01E02.mkv", Video),
                ("SHOW S01E03.MKV", Video),
            ],
        );
        assert_eq!(
            next(&src, "Show S01E02.mkv").as_deref(),
            Some("SHOW S01E03.MKV")
        );
        assert_eq!(
            next(&src, "SHOW S01E03.MKV").as_deref(),
            Some("show s01e10.mkv")
        );
    }

    #[test]
    fn folder_listing_errors_propagate() {
        let src = FakeSource::with("Other", &[]);
        let current = SourcePath::parse("Gone/Ep 1.mkv").unwrap();
        assert!(matches!(
            next_video(&src, &current),
            Err(SourceError::NotFound(_))
        ));
    }

    #[test]
    fn root_is_not_a_file() {
        let src = FakeSource::with("", &[("a.mkv", Video)]);
        assert_eq!(next_video(&src, &SourcePath::root()).unwrap(), None);
    }

    #[test]
    fn works_with_local_fs() {
        let dir = std::env::temp_dir().join(format!("enthuse-next-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["Ep 1.mkv", "Ep 10.mkv", "Ep 2.mkv", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let src = crate::source::LocalFs::new(&dir);
        let after = |p: &str| next_video(&src, &SourcePath::parse(p).unwrap()).unwrap();
        assert_eq!(after("Ep 2.mkv").unwrap().as_str(), "Ep 10.mkv");
        assert_eq!(after("Ep 10.mkv"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
