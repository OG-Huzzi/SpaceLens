//! SQLite bootstrap — validates the storage approach only.
//!
//! Creates the minimal Phase-0 schema (schema version + drives + scans) with
//! forward-only migration semantics from `docs/DATABASE.md`. Full schema
//! arrives in Phase 1/5.

use rusqlite::{Connection, Result};

/// Current schema version. Migrations are forward-only and numbered.
pub const SCHEMA_VERSION: u32 = 1;

/// Forward-only migrations. Index i holds migration i+1.
const MIGRATIONS: [&str; 1] = ["
    CREATE TABLE schema_version (version INTEGER NOT NULL);
    INSERT INTO schema_version (version) VALUES (1);
    CREATE TABLE drives (
        id            TEXT PRIMARY KEY,
        label         TEXT NOT NULL,
        fs_type       TEXT NOT NULL,
        kind          TEXT NOT NULL,
        capacity      INTEGER NOT NULL,
        first_seen_at TEXT NOT NULL,
        seen_last_at  TEXT NOT NULL,
        offline       INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE scans (
        id          TEXT PRIMARY KEY,
        drive_id    TEXT NOT NULL REFERENCES drives(id),
        started_at  TEXT NOT NULL,
        finished_at TEXT,
        status      TEXT NOT NULL,
        root        TEXT NOT NULL,
        file_count  INTEGER NOT NULL DEFAULT 0,
        dir_count   INTEGER NOT NULL DEFAULT 0,
        bytes       INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_scans_drive ON scans(drive_id);
"];

/// Opens (or creates) the database and applies pending migrations.
pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    let current: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap_or(0);
    for (i, migration) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as u32;
        if version > current {
            conn.execute_batch(migration)?;
        }
    }
    Ok(conn)
}

/// Returns the schema version recorded in an opened database.
pub fn schema_version(conn: &Connection) -> Result<u32> {
    conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_creates_schema_version_1() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("spacelens.db")).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn reopen_is_idempotent_and_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spacelens.db");
        {
            let conn = open(&path).unwrap();
            conn.execute(
                "INSERT INTO drives (id, label, fs_type, kind, capacity, first_seen_at, seen_last_at, offline)
                 VALUES ('d1', 'System', 'NTFS', 'internal', 512000000000, '2026-09-07T00:00:00Z', '2026-09-07T00:00:00Z', 0)",
                [],
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM drives", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn wal_and_foreign_keys_are_on() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("spacelens.db")).unwrap();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }
}
