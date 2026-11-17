//! Persistent storage: saved sources, resume positions and watched state,
//! in SQLite.

mod migrations;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::settings::{self, FolderOverrides, PlaybackSettings};
use crate::source::SourcePath;
use crate::watched::{WatchedMode, WatchedRule, reaches_threshold};

/// The watched rule's share of the file (see [`Store::watched_rule`]).
const KEY_WATCHED_THRESHOLD: &str = "watched_threshold";
const KEY_WATCHED_MODE: &str = "watched_mode";
const KEY_WATCHED_REMAINING: &str = "watched_remaining";
const KEY_AUTO_NEXT: &str = "auto_next";
const KEY_RESUME: &str = "resume";
const KEY_NETWORK_THUMBNAILS: &str = "network_thumbnails";

/// Storage error.
#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
    /// A stored path could not be parsed.
    Corrupt(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "database error: {e}"),
            Self::Migration(e) => write!(f, "database migration failed: {e}"),
            Self::Corrupt(e) => write!(f, "invalid data in database: {e}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Migration(e) => Some(e),
            Self::Corrupt(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<rusqlite_migration::Error> for StorageError {
    fn from(e: rusqlite_migration::Error) -> Self {
        Self::Migration(e)
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Id of a saved source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub i64);

/// A saved source, as stored. `kind` says which [`Source`](crate::source::Source)
/// implementation to build (e.g. `"local"`), `location` how to reach it
/// (e.g. a folder path or URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSource {
    pub id: SourceId,
    pub kind: String,
    pub name: String,
    pub location: String,
}

/// Playback state of one file.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchState {
    /// Last saved position in seconds, if any.
    pub position: Option<f64>,
    /// Duration in seconds, if known.
    pub duration: Option<f64>,
    pub watched: bool,
}

impl WatchState {
    /// Started but not finished: there is a position and it is not watched.
    pub fn in_progress(&self) -> bool {
        !self.watched && self.position.is_some_and(|p| p > 0.0)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The Enthuse database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it to
    /// the latest schema.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            // Let SQLite report the real error if this fails.
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    /// An empty in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrations::migrations().to_latest(&mut conn)?;
        Ok(Self { conn })
    }

    // Sources

    /// Saves a source, or returns the existing one with the same kind and
    /// location (renaming it to `name`).
    pub fn add_source(&self, kind: &str, name: &str, location: &str) -> Result<SourceId> {
        let id = self.conn.query_row(
            "INSERT INTO sources (kind, name, location, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (kind, location) DO UPDATE SET name = excluded.name
             RETURNING id",
            params![kind, name, location, now()],
            |r| r.get(0),
        )?;
        Ok(SourceId(id))
    }

    /// All saved sources, oldest first.
    pub fn sources(&self) -> Result<Vec<SavedSource>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, name, location FROM sources ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |r| {
            Ok(SavedSource {
                id: SourceId(r.get(0)?),
                kind: r.get(1)?,
                name: r.get(2)?,
                location: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Removes a source and all its watch state.
    pub fn remove_source(&self, id: SourceId) -> Result<()> {
        self.conn
            .execute("DELETE FROM sources WHERE id = ?1", [id.0])?;
        Ok(())
    }

    // Watch state

    /// The state of one file, or `None` if it was never played.
    pub fn watch_state(&self, source: SourceId, path: &SourcePath) -> Result<Option<WatchState>> {
        Ok(self
            .conn
            .query_row(
                "SELECT position, duration, watched FROM watch_state
                 WHERE source_id = ?1 AND path = ?2",
                params![source.0, path.as_str()],
                |r| {
                    Ok(WatchState {
                        position: r.get(0)?,
                        duration: r.get(1)?,
                        watched: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Files that were started and not finished, most recently played
    /// first, for "Continue watching". At most `limit`.
    pub fn continue_watching(
        &self,
        limit: usize,
    ) -> Result<Vec<(SourceId, SourcePath, WatchState)>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, path, position, duration, watched FROM watch_state
             WHERE watched = 0 AND position > 0
             ORDER BY updated_at DESC, rowid DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((
                SourceId(r.get(0)?),
                r.get::<_, String>(1)?,
                WatchState {
                    position: r.get(2)?,
                    duration: r.get(3)?,
                    watched: r.get(4)?,
                },
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (source, path, state) = row?;
            let path =
                SourcePath::parse(&path).map_err(|e| StorageError::Corrupt(e.to_string()))?;
            out.push((source, path, state));
        }
        Ok(out)
    }

    /// Files that were played or marked watched, most recent first, with
    /// when (Unix seconds), for the Recent screen. At most `limit`.
    pub fn recently_played(
        &self,
        limit: usize,
    ) -> Result<Vec<(SourceId, SourcePath, WatchState, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, path, position, duration, watched, updated_at FROM watch_state
             WHERE watched = 1 OR position > 0
             ORDER BY updated_at DESC, rowid DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((
                SourceId(r.get(0)?),
                r.get::<_, String>(1)?,
                WatchState {
                    position: r.get(2)?,
                    duration: r.get(3)?,
                    watched: r.get(4)?,
                },
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (source, path, state, at) = row?;
            let path =
                SourcePath::parse(&path).map_err(|e| StorageError::Corrupt(e.to_string()))?;
            out.push((source, path, state, at));
        }
        Ok(out)
    }

    /// States of all played files directly inside `folder`, keyed by path,
    /// for showing markers in the browser.
    pub fn folder_states(
        &self,
        source: SourceId,
        folder: &SourcePath,
    ) -> Result<HashMap<SourcePath, WatchState>> {
        // Paths are stored '/'-separated, so a folder's direct children are
        // "folder/name" without a further '/'. Filter in Rust to keep LIKE
        // escaping out of the picture.
        let prefix = if folder.is_root() {
            String::new()
        } else {
            format!("{}/", folder.as_str())
        };
        let mut stmt = self.conn.prepare(
            "SELECT path, position, duration, watched FROM watch_state
             WHERE source_id = ?1 AND substr(path, 1, ?2) = ?3",
        )?;
        let rows = stmt.query_map(
            params![source.0, prefix.chars().count() as i64, prefix],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    WatchState {
                        position: r.get(1)?,
                        duration: r.get(2)?,
                        watched: r.get(3)?,
                    },
                ))
            },
        )?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, state) = row?;
            if path[prefix.len()..].contains('/') {
                continue; // Deeper than a direct child.
            }
            let path =
                SourcePath::parse(&path).map_err(|e| StorageError::Corrupt(e.to_string()))?;
            out.insert(path, state);
        }
        Ok(out)
    }

    /// Saves the playback position of a file when playback stops or pauses.
    /// Marks it watched if the position meets the watched rule.
    pub fn save_position(
        &self,
        source: SourceId,
        path: &SourcePath,
        position: f64,
        duration: Option<f64>,
    ) -> Result<()> {
        let watched = reaches_threshold(position, duration, &self.watched_rule()?);
        self.write_position(source, path, position, duration, watched)
    }

    /// Saves the position during playback. Never marks the file watched, so
    /// seeking past the threshold and back does not lose the resume point.
    pub fn save_progress(
        &self,
        source: SourceId,
        path: &SourcePath,
        position: f64,
        duration: Option<f64>,
    ) -> Result<()> {
        self.write_position(source, path, position, duration, false)
    }

    fn write_position(
        &self,
        source: SourceId,
        path: &SourcePath,
        position: f64,
        duration: Option<f64>,
        watched: bool,
    ) -> Result<()> {
        let position = position.max(0.0);
        self.conn.execute(
            "INSERT INTO watch_state (source_id, path, position, duration, watched, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (source_id, path) DO UPDATE SET
                position = excluded.position,
                duration = COALESCE(excluded.duration, duration),
                watched = watched OR excluded.watched,
                updated_at = excluded.updated_at",
            params![source.0, path.as_str(), position, duration, watched, now()],
        )?;
        Ok(())
    }

    /// Marks a file watched or unwatched. Either way the saved position is
    /// cleared, so the next play starts from the beginning.
    pub fn set_watched(&self, source: SourceId, path: &SourcePath, watched: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO watch_state (source_id, path, position, duration, watched, updated_at)
             VALUES (?1, ?2, NULL, NULL, ?3, ?4)
             ON CONFLICT (source_id, path) DO UPDATE SET
                position = NULL, watched = excluded.watched, updated_at = excluded.updated_at",
            params![source.0, path.as_str(), watched, now()],
        )?;
        Ok(())
    }

    // Settings

    /// When a file counts as watched (#41).
    pub fn watched_rule(&self) -> Result<WatchedRule> {
        let mut rule = WatchedRule::default();
        if let Some(mode) = self
            .setting(KEY_WATCHED_MODE)?
            .as_deref()
            .and_then(WatchedMode::parse)
        {
            rule.mode = mode;
        }
        if let Some(f) = self
            .setting(KEY_WATCHED_THRESHOLD)?
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| (0.0..=1.0).contains(v))
        {
            rule.fraction = f;
        }
        if let Some(r) = self
            .setting(KEY_WATCHED_REMAINING)?
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
        {
            rule.remaining = r;
        }
        Ok(rule)
    }

    pub fn set_watched_rule(&self, rule: &WatchedRule) -> Result<()> {
        // All three keys or none.
        let tx = self.conn.unchecked_transaction()?;
        self.set_setting(KEY_WATCHED_MODE, rule.mode.as_str())?;
        self.set_setting(
            KEY_WATCHED_THRESHOLD,
            &rule.fraction.clamp(0.0, 1.0).to_string(),
        )?;
        let remaining = if rule.remaining.is_finite() {
            rule.remaining.max(0.0)
        } else {
            0.0
        };
        self.set_setting(KEY_WATCHED_REMAINING, &remaining.to_string())?;
        tx.commit()?;
        Ok(())
    }

    /// Whether the next file in the folder starts when one ends. On unless
    /// turned off.
    pub fn auto_next(&self) -> Result<bool> {
        Ok(self.setting(KEY_AUTO_NEXT)?.is_none_or(|v| v != "0"))
    }

    pub fn set_auto_next(&self, on: bool) -> Result<()> {
        self.set_setting(KEY_AUTO_NEXT, if on { "1" } else { "0" })
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Whether to make thumbnails for files on network shares. On unless
    /// turned off, since grabbing a frame reads part of each file.
    pub fn network_thumbnails(&self) -> Result<bool> {
        Ok(self
            .setting(KEY_NETWORK_THUMBNAILS)?
            .is_none_or(|v| v != "0"))
    }

    pub fn set_network_thumbnails(&self, on: bool) -> Result<()> {
        self.set_setting(KEY_NETWORK_THUMBNAILS, if on { "1" } else { "0" })
    }

    /// Global playback settings, used where no folder overrides them.
    pub fn default_playback_settings(&self) -> Result<PlaybackSettings> {
        let mut s = PlaybackSettings::default();
        if let Some(v) = self.setting(KEY_RESUME)? {
            s.resume = v != "0";
        }
        Ok(s)
    }

    pub fn set_default_resume(&self, resume: bool) -> Result<()> {
        self.set_setting(KEY_RESUME, if resume { "1" } else { "0" })
    }

    /// The overrides stored for folders of `source`.
    pub fn folder_overrides(
        &self,
        source: SourceId,
    ) -> Result<HashMap<SourcePath, FolderOverrides>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, resume FROM folder_settings WHERE source_id = ?1")?;
        let rows = stmt.query_map([source.0], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<bool>>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, resume) = row?;
            let path =
                SourcePath::parse(&path).map_err(|e| StorageError::Corrupt(e.to_string()))?;
            out.insert(path, FolderOverrides { resume });
        }
        Ok(out)
    }

    /// Every folder override of every source, by source and path.
    pub fn all_folder_overrides(&self) -> Result<Vec<(SourceId, SourcePath, FolderOverrides)>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, path, resume FROM folder_settings ORDER BY source_id, path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<bool>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, path, resume) = row?;
            let path =
                SourcePath::parse(&path).map_err(|e| StorageError::Corrupt(e.to_string()))?;
            out.push((SourceId(id), path, FolderOverrides { resume }));
        }
        Ok(out)
    }

    /// Sets `folder`'s own resume setting; `None` follows the parent again.
    pub fn set_folder_resume(
        &self,
        source: SourceId,
        folder: &SourcePath,
        resume: Option<bool>,
    ) -> Result<()> {
        let o = FolderOverrides { resume };
        if o.is_empty() {
            self.conn.execute(
                "DELETE FROM folder_settings WHERE source_id = ?1 AND path = ?2",
                params![source.0, folder.as_str()],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO folder_settings (source_id, path, resume) VALUES (?1, ?2, ?3)
                 ON CONFLICT (source_id, path) DO UPDATE SET resume = excluded.resume",
                params![source.0, folder.as_str(), o.resume],
            )?;
        }
        Ok(())
    }

    /// The settings that apply to files in `folder`.
    pub fn playback_settings(
        &self,
        source: SourceId,
        folder: &SourcePath,
    ) -> Result<PlaybackSettings> {
        Ok(settings::resolve(
            self.default_playback_settings()?,
            folder,
            &self.folder_overrides(source)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> SourcePath {
        SourcePath::parse(s).unwrap()
    }

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn continue_watching_lists_started_files_newest_first() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        s.save_progress(src, &p("a.mkv"), 100.0, Some(1000.0))
            .unwrap();
        s.save_progress(src, &p("b.mkv"), 200.0, Some(1000.0))
            .unwrap();
        s.save_position(src, &p("done.mkv"), 990.0, Some(1000.0))
            .unwrap();
        s.save_progress(src, &p("zero.mkv"), 0.0, Some(1000.0))
            .unwrap();
        // Played again later: moves to the front. Times are in seconds, so
        // set them by hand rather than sleeping.
        s.save_progress(src, &p("a.mkv"), 150.0, Some(1000.0))
            .unwrap();
        s.conn
            .execute(
                "UPDATE watch_state SET updated_at = 100 WHERE path = 'b.mkv'",
                [],
            )
            .unwrap();
        s.conn
            .execute(
                "UPDATE watch_state SET updated_at = 200 WHERE path = 'a.mkv'",
                [],
            )
            .unwrap();
        let list = s.continue_watching(10).unwrap();
        let names: Vec<_> = list.iter().map(|(_, path, _)| path.as_str()).collect();
        assert_eq!(names, ["a.mkv", "b.mkv"]);
        assert_eq!(list[0].2.position, Some(150.0));
        assert_eq!(s.continue_watching(1).unwrap().len(), 1);
        // Removing the source drops its files.
        s.remove_source(src).unwrap();
        assert!(s.continue_watching(10).unwrap().is_empty());
    }

    #[test]
    fn progress_saves_never_mark_watched() {
        let s = Store::open_in_memory().unwrap();
        let src = s.add_source("local", "M", "/m").unwrap();
        let f = SourcePath::parse("a.mkv").unwrap();
        s.save_progress(src, &f, 950.0, Some(1000.0)).unwrap();
        assert!(!s.watch_state(src, &f).unwrap().unwrap().watched);
        s.save_position(src, &f, 950.0, Some(1000.0)).unwrap();
        assert!(s.watch_state(src, &f).unwrap().unwrap().watched);
    }

    #[test]
    fn sources_roundtrip_and_dedupe() {
        let s = store();
        let a = s.add_source("local", "Movies", "/media/movies").unwrap();
        let b = s.add_source("local", "TV", r"\\nas\tv").unwrap();
        let a2 = s.add_source("local", "Films", "/media/movies").unwrap();
        assert_eq!(a, a2);
        assert_ne!(a, b);

        let all = s.sources().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "Films");
        assert_eq!(all[1].location, r"\\nas\tv");

        s.remove_source(a).unwrap();
        assert_eq!(s.sources().unwrap().len(), 1);
    }

    #[test]
    fn removing_source_removes_its_state() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        s.save_position(src, &p("a.mkv"), 10.0, Some(100.0))
            .unwrap();
        s.remove_source(src).unwrap();
        let src2 = s.add_source("local", "M", "/m").unwrap();
        assert_eq!(s.watch_state(src2, &p("a.mkv")).unwrap(), None);
    }

    #[test]
    fn position_saves_and_marks_watched_at_threshold() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        let f = p("Show/Ep 1.mkv");
        assert_eq!(s.watch_state(src, &f).unwrap(), None);

        s.save_position(src, &f, 60.0, Some(1000.0)).unwrap();
        let st = s.watch_state(src, &f).unwrap().unwrap();
        assert_eq!(st.position, Some(60.0));
        assert!(!st.watched);
        assert!(st.in_progress());

        s.save_position(src, &f, 900.0, Some(1000.0)).unwrap();
        let st = s.watch_state(src, &f).unwrap().unwrap();
        assert!(st.watched);
        assert!(!st.in_progress());

        // Seeking back later does not unmark it.
        s.save_position(src, &f, 5.0, None).unwrap();
        let st = s.watch_state(src, &f).unwrap().unwrap();
        assert!(st.watched);
        assert_eq!(
            st.duration,
            Some(1000.0),
            "unknown duration keeps the old one"
        );
    }

    #[test]
    fn set_watched_clears_position() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        let f = p("a.mkv");
        s.save_position(src, &f, 300.0, Some(1000.0)).unwrap();
        s.set_watched(src, &f, true).unwrap();
        let st = s.watch_state(src, &f).unwrap().unwrap();
        assert!(st.watched);
        assert_eq!(st.position, None);

        s.set_watched(src, &f, false).unwrap();
        assert!(!s.watch_state(src, &f).unwrap().unwrap().watched);
        // Marking an unplayed file works too.
        s.set_watched(src, &p("b.mkv"), true).unwrap();
        assert!(s.watch_state(src, &p("b.mkv")).unwrap().unwrap().watched);
    }

    #[test]
    fn folder_states_only_direct_children() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        let other = s.add_source("local", "O", "/o").unwrap();
        s.save_position(src, &p("TV/a.mkv"), 1.0, None).unwrap();
        s.save_position(src, &p("TV/Season 1/b.mkv"), 1.0, None)
            .unwrap();
        s.save_position(src, &p("TV2/c.mkv"), 1.0, None).unwrap();
        s.save_position(src, &p("root.mkv"), 1.0, None).unwrap();
        s.save_position(other, &p("TV/d.mkv"), 1.0, None).unwrap();

        let tv = s.folder_states(src, &p("TV")).unwrap();
        assert_eq!(
            tv.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            ["TV/a.mkv"]
        );

        let root = s.folder_states(src, &SourcePath::root()).unwrap();
        assert_eq!(
            root.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            ["root.mkv"]
        );
    }

    #[test]
    fn non_ascii_paths() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        s.save_position(src, &p("Ødland/Æble 1.mkv"), 1.0, None)
            .unwrap();
        let st = s.folder_states(src, &p("Ødland")).unwrap();
        assert!(st.contains_key(&p("Ødland/Æble 1.mkv")));
    }

    #[test]
    fn watched_rule_setting() {
        let s = store();
        assert_eq!(s.watched_rule().unwrap(), WatchedRule::default());
        let rule = WatchedRule {
            mode: WatchedMode::FirstOf,
            fraction: 0.95,
            remaining: 90.0,
        };
        s.set_watched_rule(&rule).unwrap();
        assert_eq!(s.watched_rule().unwrap(), rule);
        s.set_watched_rule(&WatchedRule {
            fraction: 5.0,
            remaining: -3.0,
            ..rule
        })
        .unwrap();
        let read = s.watched_rule().unwrap();
        assert_eq!((read.fraction, read.remaining), (1.0, 0.0));
        // A value from an older build, or edited by hand, falls back.
        s.set_setting(KEY_WATCHED_MODE, "bogus").unwrap();
        assert_eq!(s.watched_rule().unwrap().mode, WatchedMode::Percent);

        let src = s.add_source("local", "M", "/m").unwrap();
        s.set_watched_rule(&WatchedRule::percent(0.5)).unwrap();
        s.save_position(src, &p("a.mkv"), 50.0, Some(100.0))
            .unwrap();
        assert!(s.watch_state(src, &p("a.mkv")).unwrap().unwrap().watched);
        // Time left: 3 hours with a minute to go counts.
        s.set_watched_rule(&WatchedRule {
            mode: WatchedMode::Remaining,
            fraction: 0.9,
            remaining: 120.0,
        })
        .unwrap();
        s.save_position(src, &p("b.mkv"), 10_740.0, Some(10_800.0))
            .unwrap();
        assert!(s.watch_state(src, &p("b.mkv")).unwrap().unwrap().watched);
        s.save_position(src, &p("c.mkv"), 9_720.0, Some(10_800.0))
            .unwrap();
        assert!(!s.watch_state(src, &p("c.mkv")).unwrap().unwrap().watched);
    }

    #[test]
    fn recently_played_includes_watched_newest_first() {
        let s = store();
        let src = s.add_source("local", "M", "/m").unwrap();
        s.save_progress(src, &p("a.mkv"), 30.0, Some(100.0)).unwrap();
        s.set_watched(src, &p("b.mkv"), true).unwrap();
        s.save_progress(src, &p("c.mkv"), 10.0, Some(100.0)).unwrap();
        // Marked unwatched again, with no position: not listed.
        s.set_watched(src, &p("c.mkv"), false).unwrap();
        let recent: Vec<_> = s
            .recently_played(10)
            .unwrap()
            .into_iter()
            .map(|(_, path, state, _)| (path, state.watched))
            .collect();
        assert_eq!(recent, vec![(p("b.mkv"), true), (p("a.mkv"), false)]);
        assert_eq!(s.recently_played(1).unwrap().len(), 1);
    }

    #[test]
    fn auto_next_default_on() {
        let s = store();
        assert!(s.auto_next().unwrap());
        s.set_auto_next(false).unwrap();
        assert!(!s.auto_next().unwrap());
    }

    #[test]
    fn lists_all_folder_overrides() {
        let s = store();
        let a = s.add_source("local", "A", "/a").unwrap();
        let b = s.add_source("local", "B", "/b").unwrap();
        s.set_folder_resume(b, &p("X"), Some(true)).unwrap();
        s.set_folder_resume(a, &p("Y"), Some(false)).unwrap();
        let all = s.all_folder_overrides().unwrap();
        assert_eq!(
            all,
            vec![
                (
                    a,
                    p("Y"),
                    FolderOverrides {
                        resume: Some(false)
                    }
                ),
                (b, p("X"), FolderOverrides { resume: Some(true) }),
            ]
        );
    }

    #[test]
    fn resume_settings_per_folder() {
        let s = store();
        let id = s.add_source("local", "Media", "/media").unwrap();
        let other = s.add_source("local", "Other", "/other").unwrap();
        assert!(s.playback_settings(id, &p("Shows/A")).unwrap().resume);

        s.set_folder_resume(id, &p("Shows"), Some(false)).unwrap();
        assert!(!s.playback_settings(id, &p("Shows/A")).unwrap().resume);
        assert!(s.playback_settings(id, &p("Movies")).unwrap().resume);
        // Overrides belong to their source.
        assert!(s.playback_settings(other, &p("Shows/A")).unwrap().resume);

        s.set_default_resume(false).unwrap();
        assert!(!s.playback_settings(id, &p("Movies")).unwrap().resume);
        s.set_folder_resume(id, &p("Shows/A"), Some(true)).unwrap();
        assert!(s.playback_settings(id, &p("Shows/A/S01")).unwrap().resume);

        s.set_folder_resume(id, &p("Shows/A"), None).unwrap();
        s.set_folder_resume(id, &p("Shows"), None).unwrap();
        assert!(s.folder_overrides(id).unwrap().is_empty());

        s.set_folder_resume(other, &SourcePath::root(), Some(false))
            .unwrap();
        s.remove_source(other).unwrap();
        let count: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM folder_settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn network_thumbnails_default_on() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.network_thumbnails().unwrap());
        s.set_network_thumbnails(false).unwrap();
        assert!(!s.network_thumbnails().unwrap());
        s.set_network_thumbnails(true).unwrap();
        assert!(s.network_thumbnails().unwrap());
    }

    #[test]
    fn reopening_file_database_keeps_data() {
        let dir = std::env::temp_dir().join(format!("enthuse-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = dir.join("nested").join("enthuse.db");
        {
            let s = Store::open(&db).unwrap();
            let src = s.add_source("local", "M", "/m").unwrap();
            s.save_position(src, &p("a.mkv"), 42.0, None).unwrap();
        }
        let s = Store::open(&db).unwrap();
        let src = s.sources().unwrap()[0].id;
        assert_eq!(
            s.watch_state(src, &p("a.mkv")).unwrap().unwrap().position,
            Some(42.0)
        );
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
