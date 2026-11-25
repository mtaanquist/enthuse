//! Database schema, as numbered migrations.
//!
//! Never edit a migration that has been merged to `main`: add a new one.

use rusqlite_migration::{M, Migrations};

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        // 1: sources, per-file watch state and settings.
        M::up(
            "CREATE TABLE sources (
                id         INTEGER PRIMARY KEY,
                kind       TEXT NOT NULL,
                name       TEXT NOT NULL,
                location   TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE (kind, location)
            );
            CREATE TABLE watch_state (
                source_id  INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
                path       TEXT NOT NULL,
                position   REAL,
                duration   REAL,
                watched    INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (source_id, path)
            );
            CREATE TABLE settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        ),
        // 2: per-folder playback settings. NULL means "same as the parent".
        M::up(
            "CREATE TABLE folder_settings (
                source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
                path      TEXT NOT NULL,
                resume    INTEGER,
                PRIMARY KEY (source_id, path)
            );",
        ),
        // 3: libraries (#59): favourites listed first, the user's order
        // (NULL: in the order added), libraries left off Home, and
        // favourite folders inside them.
        M::up(
            "ALTER TABLE sources ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE sources ADD COLUMN sort_order INTEGER;
            ALTER TABLE sources ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
            CREATE TABLE favorite_folders (
                source_id  INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
                path       TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (source_id, path)
            );",
        ),
    ])
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrations_are_valid() {
        super::migrations().validate().unwrap();
    }
}
