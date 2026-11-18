//! Browsable media sources.
//!
//! Every place media can come from (a local folder, a mounted or UNC network
//! share, later a built-in SMB client or Plex) implements [`Source`]. Paths
//! inside a source are [`SourcePath`]s: relative, `/`-separated and
//! platform-independent, so they can be stored as keys for watch state.

mod local;
mod path;

pub use local::LocalFs;
pub use path::{InvalidPath, SourcePath};

use std::cmp::Ordering;
use std::fmt;

use crate::natsort::natural_cmp;

/// File extensions (lowercase, without dot) treated as video files.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "3gp", "asf", "avi", "divx", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts",
    "ogv", "rm", "rmvb", "ts", "vob", "webm", "wmv",
];

/// Returns true if `name` has one of the [`VIDEO_EXTENSIONS`].
pub fn is_video_file(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && VIDEO_EXTENSIONS.iter().any(|v| ext.eq_ignore_ascii_case(v))
    })
}

/// Whether a source location is a network share: a UNC path
/// (`\\server\share`, or `//server/share`) or an `smb://` URL.
pub fn is_network_location(location: &str) -> bool {
    location.starts_with(r"\\") || location.starts_with("//") || location.starts_with("smb://")
}

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Folder,
    Video,
}

/// One item in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Display name (the last path segment).
    pub name: String,
    /// Path of the entry inside its source.
    pub path: SourcePath,
    pub kind: EntryKind,
}

/// Orders entries as the browser shows them: folders first, then videos,
/// each in natural order.
pub fn entry_order(a: &Entry, b: &Entry) -> Ordering {
    let rank = |e: &Entry| match e.kind {
        EntryKind::Folder => 0,
        EntryKind::Video => 1,
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| natural_cmp(&a.name, &b.name))
}

/// Size and modification time of a file, to tell when it changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FileStamp {
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: u64,
}

/// Why a source operation failed.
#[derive(Debug)]
pub enum SourceError {
    /// The path does not exist.
    NotFound(SourcePath),
    /// The path exists but is not a folder.
    NotAFolder(SourcePath),
    /// Access was denied, e.g. missing share credentials.
    PermissionDenied(SourcePath),
    /// Any other I/O failure, e.g. a share that is offline.
    Io(SourcePath, std::io::Error),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "'{p}' was not found"),
            Self::NotAFolder(p) => write!(f, "'{p}' is not a folder"),
            Self::PermissionDenied(p) => write!(f, "access to '{p}' was denied"),
            Self::Io(p, e) => write!(f, "cannot read '{p}': {e}"),
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, e) => Some(e),
            _ => None,
        }
    }
}

/// A place to browse for media.
///
/// Listing can be slow on network shares, so callers should run it off the
/// UI thread. Implementations must be `Send + Sync` to allow that.
pub trait Source: Send + Sync {
    /// Human-readable name, e.g. the folder or share name.
    fn display_name(&self) -> String;

    /// Lists folders and video files directly inside `dir`, sorted with
    /// [`entry_order`]. Other files and hidden entries are left out.
    fn list(&self, dir: &SourcePath) -> Result<Vec<Entry>, SourceError>;

    /// What to hand to the player to open `file`: a local path or a URL.
    fn playable(&self, file: &SourcePath) -> Result<String, SourceError>;

    /// Size and modification time of `file`, when the source knows them.
    /// Can be slow on network shares, like listing.
    fn stamp(&self, _file: &SourcePath) -> Option<FileStamp> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_paths_are_network_shares() {
        assert!(is_network_location(r"\\nas\media"));
        assert!(is_network_location("//nas/media"));
        assert!(is_network_location("smb://nas/media"));
        assert!(!is_network_location(r"D:\Movies"));
        assert!(!is_network_location("/mnt/media"));
    }

    #[test]
    fn video_extension_detection() {
        assert!(is_video_file("movie.mkv"));
        assert!(is_video_file("MOVIE.MKV"));
        assert!(is_video_file("show.s01e01.mp4"));
        assert!(!is_video_file("notes.txt"));
        assert!(!is_video_file("subs.srt"));
        assert!(!is_video_file("mkv"));
        assert!(!is_video_file(".mkv"));
        assert!(!is_video_file("archive.mkv.part"));
    }

    #[test]
    fn folders_before_videos_in_natural_order() {
        let e = |name: &str, kind| Entry {
            name: name.into(),
            path: SourcePath::root().join(name).unwrap(),
            kind,
        };
        let mut v = [
            e("Ep 10.mkv", EntryKind::Video),
            e("Season 2", EntryKind::Folder),
            e("Ep 9.mkv", EntryKind::Video),
            e("Season 10", EntryKind::Folder),
        ];
        v.sort_by(entry_order);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Season 2", "Season 10", "Ep 9.mkv", "Ep 10.mkv"]);
    }
}
