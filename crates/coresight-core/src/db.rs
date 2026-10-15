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

/// Open (or create) the database file and write NOTHING to it.
///
/// The returned connection is the one a validating caller is expected to
/// inspect and — only if the inspection authorizes it — hand to
/// [`apply_bootstrap`]. Validating and mutating through the SAME connection
/// is what makes the sequence race-safe (Phase 6.5.2): a database swapped
/// in at the path after an inspection can never be initialized or changed
/// by a caller that stayed with its own connection, because a handle pins
/// the file it was opened on (and on Windows an open database file cannot
/// be replaced at all).
///
/// Opening a path that does not exist CREATES an empty file, exactly as
/// [`open`] always has — that is what creating a brand-new store means.
pub fn open_connection(path: &std::path::Path) -> Result<Connection> {
    Connection::open(path)
}

/// Apply the write-side bootstrap to an ALREADY-OPEN connection: WAL, ON
/// foreign keys, and the v1 schema when no version is recorded yet.
///
/// This is the mutating half of [`open`], split out so a validating caller
/// can run its checks on the very connection that will be written. Callers
/// MUST have validated the connection first (integrity plus recognition of
/// a store this build could have written): the bootstrap adds tables to
/// whatever database it is pointed at, which is only ever correct for a
/// database the caller owns.
pub fn apply_bootstrap(conn: &Connection) -> Result<()> {
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
    Ok(())
}

/// Opens (or creates) the database and applies pending migrations.
pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = open_connection(path)?;
    apply_bootstrap(&conn)?;
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
        let conn = open(&dir.path().join("coresight.db")).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn reopen_is_idempotent_and_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coresight.db");
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
        let conn = open(&dir.path().join("coresight.db")).unwrap();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    /// (Phase 6.5.2, Workstream D) `open_connection` is the no-write half:
    /// it must leave the file byte-identical until `apply_bootstrap` is
    /// explicitly called on the SAME connection, and that call must
    /// complete the bootstrap in place without losing pre-existing data.
    #[test]
    fn open_connection_writes_nothing_until_the_bootstrap_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coresight.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE marker (x INTEGER);
                 INSERT INTO marker VALUES (7);",
            )
            .unwrap();
        }
        let before = std::fs::read(&path).unwrap();

        {
            let conn = open_connection(&path).unwrap();
            let x: i64 = conn
                .query_row("SELECT x FROM marker", [], |r| r.get(0))
                .unwrap();
            assert_eq!(x, 7);
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "opening a connection must not write anything to the database"
        );

        let conn = open_connection(&path).unwrap();
        apply_bootstrap(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);
        let x: i64 = conn
            .query_row("SELECT x FROM marker", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, 7, "the bootstrap must not lose pre-existing data");
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");
    }
}
