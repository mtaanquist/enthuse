//! Video thumbnails: cache keys, the cache on disk and the queue of files
//! waiting for a frame grab. The grabbing itself is the player's job.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::source::FileStamp;

/// Thumbnail width in pixels; the height follows the video.
pub const WIDTH: u32 = 256;
/// Most the cache may hold before the oldest thumbnails go.
pub const CACHE_LIMIT: u64 = 200 * 1024 * 1024;

/// A cache key for the file the player opens as `playable`. A changed size
/// or modification time gives a new key, so a replaced file gets a new
/// thumbnail. Stable across runs and Rust versions (FNV-1a, 64 bit).
pub fn cache_key(playable: &str, stamp: Option<FileStamp>) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let stamp = stamp.unwrap_or_default();
    let (size, modified) = (stamp.size.to_le_bytes(), stamp.modified.to_le_bytes());
    let bytes = playable
        .as_bytes()
        .iter()
        .chain(&[0])
        .chain(&size)
        .chain(&modified);
    let hash = bytes.fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME));
    format!("{hash:016x}")
}

/// What the cache has for a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The thumbnail, ready to show.
    Ready(PathBuf),
    /// An earlier grab failed (an empty or broken file); don't try again.
    Failed,
    Missing,
}

/// Thumbnails on disk: `<key>.jpg`, or `<key>.failed` when no frame could
/// be grabbed.
#[derive(Debug, Clone)]
pub struct ThumbCache {
    dir: PathBuf,
}

impl ThumbCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the thumbnail for `key` goes.
    pub fn image_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.jpg"))
    }

    fn failed_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.failed"))
    }

    /// What there is for `key`. A thumbnail found is marked as used, so
    /// [`Self::prune`] removes the ones not looked at for longest.
    pub fn lookup(&self, key: &str) -> Lookup {
        let image = self.image_path(key);
        if image.is_file() {
            if let Ok(f) = fs::File::options().write(true).open(&image) {
                let _ = f.set_modified(SystemTime::now());
            }
            Lookup::Ready(image)
        } else if self.failed_path(key).is_file() {
            Lookup::Failed
        } else {
            Lookup::Missing
        }
    }

    /// Remembers that no thumbnail can be made for `key`.
    pub fn mark_failed(&self, key: &str) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.failed_path(key), b"")
    }

    /// Removes the thumbnails used longest ago until the cache holds at
    /// most `limit` bytes, and scratch folders a grab left behind (the app
    /// quit mid-grab). Returns how many thumbnails were removed. Failure
    /// markers take no room and stay; other files are left alone.
    pub fn prune(&self, limit: u64) -> io::Result<usize> {
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = match fs::read_dir(&self.dir) {
            Ok(dir) => dir
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let p = e.path();
                    let scratch = e.file_name().to_string_lossy().starts_with(".grab-");
                    if scratch && p.is_dir() {
                        let _ = fs::remove_dir_all(&p);
                    }
                    !scratch && p.extension().is_some_and(|x| x == "jpg")
                })
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    Some((modified, meta.len(), e.path()))
                })
                .collect(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err),
        };
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        entries.sort();
        let mut removed = 0;
        for (_, len, path) in entries {
            if total <= limit {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Files waiting for a thumbnail, by id. Two lanes: `urgent` for cards
/// shown wherever the user is (Home, Up next), then the folder being
/// browsed, in listing order so the rows on screen come first. Going to
/// another folder replaces the folder lane, so its leftovers never run.
#[derive(Debug)]
pub struct ThumbQueue<T> {
    urgent: VecDeque<(String, T)>,
    folder: VecDeque<(String, T)>,
    /// The job being worked on, so it isn't queued again meanwhile.
    running: Option<String>,
}

impl<T> Default for ThumbQueue<T> {
    fn default() -> Self {
        Self {
            urgent: VecDeque::new(),
            folder: VecDeque::new(),
            running: None,
        }
    }
}

impl<T> ThumbQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a job ahead of the folder's, unless it is already waiting there.
    pub fn push_urgent(&mut self, id: String, job: T) {
        if self.running.as_ref() == Some(&id) || self.urgent.iter().any(|(i, _)| *i == id) {
            return;
        }
        self.folder.retain(|(i, _)| *i != id);
        self.urgent.push_back((id, job));
    }

    /// Replaces the folder lane with `jobs`, in order.
    pub fn set_folder(&mut self, jobs: Vec<(String, T)>) {
        self.folder.clear();
        for (id, job) in jobs {
            let waiting = self.running.as_ref() == Some(&id)
                || self
                    .urgent
                    .iter()
                    .chain(&self.folder)
                    .any(|(i, _)| *i == id);
            if !waiting {
                self.folder.push_back((id, job));
            }
        }
    }

    /// The next job: urgent ones first. It counts as running until
    /// [`Self::finish`].
    pub fn pop(&mut self) -> Option<(String, T)> {
        let next = self.urgent.pop_front().or_else(|| self.folder.pop_front());
        self.running = next.as_ref().map(|(id, _)| id.clone());
        next
    }

    /// Drops waiting jobs, in both lanes, for which `keep` is false, e.g.
    /// files on network shares when their thumbnails are turned off.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.urgent.retain(|(_, job)| keep(job));
        self.folder.retain(|(_, job)| keep(job));
    }

    /// The running job is done.
    pub fn finish(&mut self) {
        self.running = None;
    }

    pub fn len(&self) -> usize {
        self.urgent.len() + self.folder.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn retain_drops_from_both_lanes() {
        let mut q = super::ThumbQueue::new();
        q.push_urgent("a".into(), true);
        q.push_urgent("b".into(), false);
        q.set_folder(vec![("c".into(), false), ("d".into(), true)]);
        q.retain(|network| !network);
        assert_eq!(q.pop().map(|(id, _)| id).as_deref(), Some("b"));
        assert_eq!(q.pop().map(|(id, _)| id).as_deref(), Some("c"));
        assert!(q.pop().is_none());
    }

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "enthuse-thumbs-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn keys_are_stable_and_change_with_the_file() {
        let stamp = FileStamp {
            size: 100,
            modified: 1_700_000_000,
        };
        let key = cache_key("/media/a.mkv", Some(stamp));
        assert_eq!(key.len(), 16);
        assert_eq!(key, cache_key("/media/a.mkv", Some(stamp)));
        // Fixed value: a changed hash would orphan every cached thumbnail.
        assert_eq!(cache_key("", None), "4dfa4cffd1f7979f");
        assert_ne!(key, cache_key("/media/b.mkv", Some(stamp)));
        let resized = FileStamp { size: 101, ..stamp };
        assert_ne!(key, cache_key("/media/a.mkv", Some(resized)));
        assert_ne!(key, cache_key("/media/a.mkv", None));
    }

    #[test]
    fn lookup_finds_images_and_failures() {
        let t = TempDir::new("lookup");
        let cache = ThumbCache::new(&t.0);
        assert_eq!(cache.lookup("a"), Lookup::Missing);
        fs::write(cache.image_path("a"), b"jpg").unwrap();
        assert_eq!(cache.lookup("a"), Lookup::Ready(t.0.join("a.jpg")));
        cache.mark_failed("b").unwrap();
        assert_eq!(cache.lookup("b"), Lookup::Failed);
    }

    #[test]
    fn prune_removes_the_oldest_until_under_the_limit() {
        let t = TempDir::new("prune");
        let cache = ThumbCache::new(&t.0);
        let base = SystemTime::now() - std::time::Duration::from_secs(1000);
        for (i, name) in ["old.jpg", "mid.jpg", "new.jpg"].iter().enumerate() {
            let path = t.0.join(name);
            fs::write(&path, [0u8; 10]).unwrap();
            let f = fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(base + std::time::Duration::from_secs(i as u64 * 100))
                .unwrap();
        }
        fs::write(t.0.join("notes.txt"), [0u8; 50]).unwrap();
        cache.mark_failed("broken").unwrap();
        fs::create_dir_all(t.0.join(".grab-1-2")).unwrap();
        assert_eq!(cache.prune(20).unwrap(), 1);
        assert!(!t.0.join("old.jpg").exists());
        assert!(t.0.join("mid.jpg").exists());
        assert!(t.0.join("notes.txt").exists());
        assert_eq!(cache.lookup("broken"), Lookup::Failed);
        assert!(!t.0.join(".grab-1-2").exists());
        assert_eq!(cache.prune(100).unwrap(), 0);
    }

    #[test]
    fn a_lookup_keeps_a_thumbnail_from_pruning() {
        let t = TempDir::new("touch");
        let cache = ThumbCache::new(&t.0);
        let old = SystemTime::now() - std::time::Duration::from_secs(1000);
        for (name, age) in [("a", 0), ("b", 100)] {
            let path = cache.image_path(name);
            fs::write(&path, [0u8; 10]).unwrap();
            let f = fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(old + std::time::Duration::from_secs(age))
                .unwrap();
        }
        // "a" is older, but just used.
        assert!(matches!(cache.lookup("a"), Lookup::Ready(_)));
        assert_eq!(cache.prune(10).unwrap(), 1);
        assert!(cache.image_path("a").exists());
        assert!(!cache.image_path("b").exists());
    }

    #[test]
    fn prune_of_a_missing_folder_is_fine() {
        let cache = ThumbCache::new(std::env::temp_dir().join("enthuse-no-such-cache"));
        assert_eq!(cache.prune(0).unwrap(), 0);
    }

    #[test]
    fn urgent_jobs_come_first_and_folders_replace_each_other() {
        let mut q = ThumbQueue::new();
        q.set_folder(vec![("a".into(), 1), ("b".into(), 2)]);
        q.push_urgent("c".into(), 3);
        assert_eq!(q.pop(), Some(("c".into(), 3)));
        assert_eq!(q.pop(), Some(("a".into(), 1)));
        q.set_folder(vec![("x".into(), 9)]);
        assert_eq!(q.pop(), Some(("x".into(), 9)));
        assert!(q.is_empty());
    }

    #[test]
    fn a_running_job_is_not_queued_again() {
        let mut q = ThumbQueue::new();
        q.push_urgent("a".into(), 1);
        assert_eq!(q.pop(), Some(("a".into(), 1)));
        q.push_urgent("a".into(), 1);
        q.set_folder(vec![("a".into(), 1)]);
        assert!(q.is_empty());
        q.finish();
        q.push_urgent("a".into(), 1);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn jobs_are_not_queued_twice() {
        let mut q = ThumbQueue::new();
        q.set_folder(vec![("a".into(), 1), ("a".into(), 1), ("b".into(), 2)]);
        q.push_urgent("b".into(), 2);
        q.push_urgent("b".into(), 2);
        assert_eq!(q.len(), 2);
        q.set_folder(vec![("b".into(), 2), ("c".into(), 3)]);
        assert_eq!(q.pop(), Some(("b".into(), 2)));
        assert_eq!(q.pop(), Some(("c".into(), 3)));
        assert_eq!(q.pop(), None);
    }
}
