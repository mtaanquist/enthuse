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
    ])
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrations_are_valid() {
        super::migrations().validate().unwrap();
    }
}
