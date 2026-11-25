//! [`Source`] for files that are not in any library: a file or folder that
//! was dropped on the window or opened from the command line (#60).
//!
//! Its paths are the files' absolute paths, so watch state of such files is
//! keyed by the full path and survives opening the same file again:
//!
//! - Linux: `/home/me/a.mkv` is `home/me/a.mkv`.
//! - Windows: `C:\Movies\a.mkv` is `C:/Movies/a.mkv`, and
//!   `\\nas\share\a.mkv` is `UNC/nas/share/a.mkv` (a drive is always `X:`,
//!   so `UNC` cannot be mistaken for one).

use std::path::{Component, Path, PathBuf};

use super::local::{list_dir, playable_file, stamp_file};
use super::{Entry, FileStamp, Source, SourceError, SourcePath};

/// Display name of the [`Computer`] source.
pub const COMPUTER_NAME: &str = "This computer";

/// Every file the OS can reach, by absolute path.
#[derive(Debug, Clone, Copy, Default)]
pub struct Computer;

impl Computer {
    /// The source path of an absolute OS path. `..` is resolved without
    /// touching the disk. `None` for a relative path, a name that is not
    /// valid UTF-8, or a Windows device path (`\\.\...`).
    pub fn path_of(os: &Path) -> Option<SourcePath> {
        if !os.is_absolute() {
            return None;
        }
        let mut segments: Vec<String> = Vec::new();
        // The drive or share, which `..` cannot leave.
        let mut fixed = 0;
        for component in os.components() {
            match component {
                Component::Prefix(prefix) => {
                    segments.extend(platform::prefix_segments(prefix.kind())?);
                    fixed = segments.len();
                }
                Component::RootDir | Component::CurDir => {}
                Component::ParentDir => {
                    if segments.len() > fixed {
                        segments.pop();
                    }
                }
                Component::Normal(name) => segments.push(name.to_str()?.to_owned()),
            }
        }
        segments
            .iter()
            .try_fold(SourcePath::root(), |path, s| path.join(s).ok())
    }

    /// The OS path of a source path, or `None` if it names no place on this
    /// OS (e.g. the root on Windows, which has no single top folder).
    pub fn os_path(path: &SourcePath) -> Option<PathBuf> {
        platform::os_path(path)
    }
}

/// Where `path` is inside the folder `root`, or `None` if it is outside.
/// Both must be absolute. Compares the way the OS does: ignoring case on
/// Windows.
pub fn relative_to(root: &Path, path: &Path) -> Option<SourcePath> {
    let root = Computer::path_of(root)?;
    let path = Computer::path_of(path)?;
    let mut rest = path.segments();
    for segment in root.segments() {
        if !platform::same_name(segment, rest.next()?) {
            return None;
        }
    }
    rest.try_fold(SourcePath::root(), |p, s| p.join(s).ok())
}

impl Source for Computer {
    fn display_name(&self) -> String {
        COMPUTER_NAME.to_owned()
    }

    fn list(&self, dir: &SourcePath) -> Result<Vec<Entry>, SourceError> {
        let os = Computer::os_path(dir).ok_or_else(|| SourceError::NotFound(dir.clone()))?;
        list_dir(&os, dir)
    }

    fn playable(&self, file: &SourcePath) -> Result<String, SourceError> {
        let os = Computer::os_path(file).ok_or_else(|| SourceError::NotFound(file.clone()))?;
        playable_file(&os, file)
    }

    fn stamp(&self, file: &SourcePath) -> Option<FileStamp> {
        stamp_file(&Computer::os_path(file)?)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::path::{PathBuf, Prefix};

    use crate::source::SourcePath;

    const UNC: &str = "UNC";

    pub fn prefix_segments(prefix: Prefix<'_>) -> Option<Vec<String>> {
        match prefix {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                Some(vec![format!("{}:", (d as char).to_ascii_uppercase())])
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => Some(vec![
                UNC.to_owned(),
                server.to_str()?.to_owned(),
                share.to_str()?.to_owned(),
            ]),
            Prefix::DeviceNS(_) | Prefix::Verbatim(_) => None,
        }
    }

    fn is_drive(segment: &str) -> bool {
        let b = segment.as_bytes();
        b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    }

    pub fn os_path(path: &SourcePath) -> Option<PathBuf> {
        let mut segments = path.segments();
        let first = segments.next()?;
        let mut os = if first == UNC {
            let (server, share) = (segments.next()?, segments.next()?);
            PathBuf::from(format!(r"\\{server}\{share}\"))
        } else if is_drive(first) {
            PathBuf::from(format!(r"{first}\"))
        } else {
            return None;
        };
        os.extend(segments);
        Some(os)
    }

    pub fn same_name(a: &str, b: &str) -> bool {
        a.to_lowercase() == b.to_lowercase()
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use std::path::{PathBuf, Prefix};

    use crate::source::SourcePath;

    /// Only Windows paths have a prefix.
    pub fn prefix_segments(_prefix: Prefix<'_>) -> Option<Vec<String>> {
        None
    }

    pub fn os_path(path: &SourcePath) -> Option<PathBuf> {
        let mut os = PathBuf::from("/");
        os.extend(path.segments());
        Some(os)
    }

    pub fn same_name(a: &str, b: &str) -> bool {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::EntryKind;

    fn path_of(s: &str) -> Option<String> {
        Computer::path_of(Path::new(s)).map(|p| p.as_str().to_owned())
    }

    #[test]
    fn relative_paths_have_no_place() {
        assert_eq!(path_of("Movies/a.mkv"), None);
        assert_eq!(path_of(""), None);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_paths_round_trip() {
        assert_eq!(path_of("/home/me/a.mkv").unwrap(), "home/me/a.mkv");
        assert_eq!(path_of("/").unwrap(), "");
        assert_eq!(path_of("/srv/./tv/../a.mkv").unwrap(), "srv/a.mkv");
        assert_eq!(path_of("/../a.mkv").unwrap(), "a.mkv");
        let p = SourcePath::parse("home/me/a.mkv").unwrap();
        assert_eq!(
            Computer::os_path(&p).unwrap(),
            PathBuf::from("/home/me/a.mkv")
        );
        assert_eq!(
            Computer::os_path(&SourcePath::root()).unwrap(),
            PathBuf::from("/")
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_relative_to_is_case_sensitive() {
        let rel = |root: &str, path: &str| {
            relative_to(Path::new(root), Path::new(path)).map(|p| p.as_str().to_owned())
        };
        assert_eq!(rel("/srv/tv", "/srv/tv/Show/a.mkv").unwrap(), "Show/a.mkv");
        assert_eq!(rel("/srv/tv", "/srv/tv").unwrap(), "");
        assert_eq!(rel("/srv/tv", "/srv/TV/a.mkv"), None);
        assert_eq!(rel("/srv/tv", "/srv/tvshows/a.mkv"), None);
        assert_eq!(rel("/", "/srv/a.mkv").unwrap(), "srv/a.mkv");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_paths_round_trip() {
        assert_eq!(path_of(r"C:\Movies\a.mkv").unwrap(), "C:/Movies/a.mkv");
        assert_eq!(path_of(r"d:\a.mkv").unwrap(), "D:/a.mkv");
        assert_eq!(path_of(r"C:\").unwrap(), "C:");
        assert_eq!(path_of(r"C:\TV\..\..\a.mkv").unwrap(), "C:/a.mkv");
        assert_eq!(
            path_of(r"\\nas\media\TV\a.mkv").unwrap(),
            "UNC/nas/media/TV/a.mkv"
        );
        assert_eq!(path_of(r"\\?\C:\a.mkv").unwrap(), "C:/a.mkv");
        assert_eq!(
            path_of(r"\\?\UNC\nas\media\a.mkv").unwrap(),
            "UNC/nas/media/a.mkv"
        );
        assert_eq!(path_of(r"\\.\COM1"), None);

        let os = |s: &str| Computer::os_path(&SourcePath::parse(s).unwrap());
        assert_eq!(
            os("C:/Movies/a.mkv").unwrap(),
            PathBuf::from(r"C:\Movies\a.mkv")
        );
        assert_eq!(os("C:").unwrap(), PathBuf::from(r"C:\"));
        assert_eq!(
            os("UNC/nas/media/a.mkv").unwrap(),
            PathBuf::from(r"\\nas\media\a.mkv")
        );
        assert_eq!(os("UNC/nas"), None);
        assert_eq!(os(""), None);
        assert_eq!(os("Movies/a.mkv"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_relative_to_ignores_case() {
        let rel = |root: &str, path: &str| {
            relative_to(Path::new(root), Path::new(path)).map(|p| p.as_str().to_owned())
        };
        assert_eq!(rel(r"C:\TV", r"c:\tv\Show\a.mkv").unwrap(), "Show/a.mkv");
        assert_eq!(rel(r"\\NAS\Media", r"\\nas\media\a.mkv").unwrap(), "a.mkv");
        assert_eq!(rel(r"C:\TV", r"D:\TV\a.mkv"), None);
        assert_eq!(rel(r"C:\", r"C:\TV\a.mkv").unwrap(), "TV/a.mkv");
    }

    #[test]
    fn lists_and_plays_by_absolute_path() {
        let dir =
            std::env::temp_dir().join(format!("enthuse-test-computer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Season 1")).unwrap();
        std::fs::write(dir.join("Season 1").join("E01.mkv"), b"x").unwrap();

        let folder = Computer::path_of(&dir.join("Season 1")).unwrap();
        let entries = Computer.list(&folder).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, EntryKind::Video);
        assert_eq!(entries[0].path, folder.join("E01.mkv").unwrap());
        assert_eq!(
            PathBuf::from(Computer.playable(&entries[0].path).unwrap()),
            dir.join("Season 1").join("E01.mkv")
        );
        assert!(Computer.stamp(&entries[0].path).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
