//! [`Source`] for anything the OS exposes as a folder: local disks, Linux
//! mounts (fstab/CIFS, GVfs, KIO FUSE) and Windows UNC paths such as
//! `\\nas\share` that Windows can already reach.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{
    Entry, EntryKind, FileStamp, Source, SourceError, SourcePath, entry_order, is_video_file,
};

/// A folder on a filesystem the OS can read.
#[derive(Debug, Clone)]
pub struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
    /// Creates a source rooted at `root`. The folder is not checked here;
    /// errors surface when it is listed, since network shares come and go.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The OS path for `path` inside this source.
    pub fn os_path(&self, path: &SourcePath) -> PathBuf {
        let mut p = self.root.clone();
        p.extend(path.segments());
        p
    }
}

fn map_err(path: &SourcePath, err: io::Error) -> SourceError {
    match err.kind() {
        io::ErrorKind::NotFound => SourceError::NotFound(path.clone()),
        io::ErrorKind::PermissionDenied => SourceError::PermissionDenied(path.clone()),
        io::ErrorKind::NotADirectory => SourceError::NotAFolder(path.clone()),
        _ => SourceError::Io(path.clone(), err),
    }
}

impl Source for LocalFs {
    fn display_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    fn list(&self, dir: &SourcePath) -> Result<Vec<Entry>, SourceError> {
        list_dir(&self.os_path(dir), dir)
    }

    fn playable(&self, file: &SourcePath) -> Result<String, SourceError> {
        playable_file(&self.os_path(file), file)
    }

    fn stamp(&self, file: &SourcePath) -> Option<FileStamp> {
        stamp_file(&self.os_path(file))
    }
}

/// Lists the folder `os_dir`, which is `dir` inside its source.
pub(super) fn list_dir(os_dir: &Path, dir: &SourcePath) -> Result<Vec<Entry>, SourceError> {
    let meta = fs::metadata(os_dir).map_err(|e| map_err(dir, e))?;
    if !meta.is_dir() {
        return Err(SourceError::NotAFolder(dir.clone()));
    }

    let mut entries = Vec::new();
    for item in fs::read_dir(os_dir).map_err(|e| map_err(dir, e))? {
        // A single unreadable entry should not hide the rest of the folder.
        let Ok(item) = item else { continue };
        // Names that are not valid UTF-8 cannot be stored as SourcePaths.
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        // Follow symlinks; skip broken ones.
        let Ok(meta) = fs::metadata(item.path()) else {
            continue;
        };
        if platform::is_hidden(&meta) {
            continue;
        }
        let kind = if meta.is_dir() {
            EntryKind::Folder
        } else if meta.is_file() && is_video_file(&name) {
            EntryKind::Video
        } else {
            continue;
        };
        let Ok(path) = dir.join(&name) else { continue };
        entries.push(Entry { name, path, kind });
    }
    entries.sort_by(entry_order);
    Ok(entries)
}

/// The player URL for the file `os_path`, which is `file` inside its source.
pub(super) fn playable_file(os_path: &Path, file: &SourcePath) -> Result<String, SourceError> {
    let meta = fs::metadata(os_path).map_err(|e| map_err(file, e))?;
    if !meta.is_file() {
        return Err(SourceError::NotFound(file.clone()));
    }
    Ok(os_path.to_string_lossy().into_owned())
}

pub(super) fn stamp_file(os_path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(os_path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some(FileStamp {
        size: meta.len(),
        modified,
    })
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

    /// Hidden or system files (e.g. `$RECYCLE.BIN`) are not shown.
    pub fn is_hidden(meta: &std::fs::Metadata) -> bool {
        meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    /// On Unix, hidden means a leading dot, which is checked by name.
    pub fn is_hidden(_meta: &std::fs::Metadata) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "enthuse-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, rel: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }

        fn dir(&self, rel: &str) {
            fs::create_dir_all(self.0.join(rel)).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn stamps_files() {
        let t = TempDir::new("stamp");
        t.file("a.mkv");
        let src = LocalFs::new(&t.0);
        let stamp = src.stamp(&SourcePath::parse("a.mkv").unwrap()).unwrap();
        assert_eq!(stamp.size, 1);
        assert!(stamp.modified > 0);
        assert_eq!(src.stamp(&SourcePath::parse("missing.mkv").unwrap()), None);
    }

    #[test]
    fn lists_folders_then_videos_naturally_sorted() {
        let t = TempDir::new("list");
        t.file("Ep 10.mkv");
        t.file("Ep 9.MP4");
        t.file("Ep 1.avi");
        t.file("Ep 1.srt");
        t.file("cover.jpg");
        t.file(".hidden.mkv");
        t.dir("Season 10");
        t.dir("Season 2");
        t.dir(".git");

        let src = LocalFs::new(&t.0);
        let entries = src.list(&SourcePath::root()).unwrap();
        assert_eq!(
            names(&entries),
            ["Season 2", "Season 10", "Ep 1.avi", "Ep 9.MP4", "Ep 10.mkv"]
        );
        assert_eq!(entries[0].kind, EntryKind::Folder);
        assert_eq!(entries[2].kind, EntryKind::Video);
        assert_eq!(entries[2].path.as_str(), "Ep 1.avi");
    }

    #[test]
    fn lists_subfolder_and_resolves_playable_path() {
        let t = TempDir::new("sub");
        t.file("TV/Show/S01E01.mkv");
        let src = LocalFs::new(&t.0);

        let show = SourcePath::parse("TV/Show").unwrap();
        let entries = src.list(&show).unwrap();
        assert_eq!(names(&entries), ["S01E01.mkv"]);
        assert_eq!(entries[0].path.as_str(), "TV/Show/S01E01.mkv");

        let playable = src.playable(&entries[0].path).unwrap();
        assert_eq!(
            PathBuf::from(playable),
            t.0.join("TV").join("Show").join("S01E01.mkv")
        );
    }

    #[test]
    fn errors_for_missing_and_non_folder_paths() {
        let t = TempDir::new("err");
        t.file("movie.mkv");
        let src = LocalFs::new(&t.0);

        let missing = SourcePath::parse("nope").unwrap();
        assert!(matches!(src.list(&missing), Err(SourceError::NotFound(_))));
        assert!(matches!(
            src.playable(&missing),
            Err(SourceError::NotFound(_))
        ));

        let file = SourcePath::parse("movie.mkv").unwrap();
        assert!(matches!(src.list(&file), Err(SourceError::NotAFolder(_))));

        let offline = LocalFs::new(t.0.join("unplugged-drive"));
        assert!(matches!(
            offline.list(&SourcePath::root()),
            Err(SourceError::NotFound(_))
        ));
    }

    #[test]
    fn folder_is_not_playable() {
        let t = TempDir::new("dirplay");
        t.dir("Folder.mkv");
        let src = LocalFs::new(&t.0);
        let p = SourcePath::parse("Folder.mkv").unwrap();
        assert!(src.playable(&p).is_err());
        // Listed as a folder even though the name looks like a video.
        let entries = src.list(&SourcePath::root()).unwrap();
        assert_eq!(entries[0].kind, EntryKind::Folder);
    }

    #[test]
    fn display_name_is_last_component() {
        assert_eq!(LocalFs::new("/media/Movies").display_name(), "Movies");
    }
}
