//! Per-OS locations for the database and configuration.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Directories where Enthuse keeps its data and configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    data: PathBuf,
    config: PathBuf,
    cache: PathBuf,
}

impl AppDirs {
    /// Resolves the standard directories for the current user.
    ///
    /// - Linux: `$XDG_DATA_HOME/enthuse` and `$XDG_CONFIG_HOME/enthuse`.
    /// - Windows: `%APPDATA%\Enthuse\data` and `%APPDATA%\Enthuse\config`.
    /// - Cache: `$XDG_CACHE_HOME/enthuse`, or `%LOCALAPPDATA%\Enthuse\cache`.
    ///
    /// Returns `None` if no home directory can be determined.
    pub fn from_system() -> Option<Self> {
        let dirs = ProjectDirs::from("", "", "Enthuse")?;
        Some(Self {
            data: dirs.data_dir().to_path_buf(),
            config: dirs.config_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Uses a single base directory for everything, e.g. for tests or a
    /// portable install.
    pub fn at(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        Self {
            data: base.join("data"),
            config: base.join("config"),
            cache: base.join("cache"),
        }
    }

    /// Directory for persistent data such as the database.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// Directory for user configuration.
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// Directory for data that can be rebuilt, such as thumbnails.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    /// Directory for video thumbnails.
    pub fn thumbnail_dir(&self) -> PathBuf {
        self.cache.join("thumbnails")
    }

    /// Path of the SQLite database file.
    pub fn database_path(&self) -> PathBuf {
        self.data.join("enthuse.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_base_puts_database_under_data() {
        let dirs = AppDirs::at("/tmp/enthuse-test");
        assert_eq!(
            dirs.database_path(),
            Path::new("/tmp/enthuse-test")
                .join("data")
                .join("enthuse.db")
        );
        assert_eq!(
            dirs.config_dir(),
            Path::new("/tmp/enthuse-test").join("config")
        );
        assert_eq!(
            dirs.thumbnail_dir(),
            Path::new("/tmp/enthuse-test")
                .join("cache")
                .join("thumbnails")
        );
    }

    #[test]
    fn system_dirs_are_absolute_and_named() {
        // CI runners and dev machines always have a home directory.
        let dirs = AppDirs::from_system().expect("home directory");
        assert!(dirs.data_dir().is_absolute());
        let lowered = dirs.data_dir().to_string_lossy().to_lowercase();
        assert!(lowered.contains("enthuse"), "{lowered}");
    }
}
