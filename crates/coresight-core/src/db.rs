//! SQLite bootstrap — validates the storage approach only.
//!
//! Creates the minimal Phase-0 schema (schema version + drives + scans) with
//! forward-only migration semantics from `docs/DATABASE.md`. Full schema
//! arrives in Phase 1/5.

use std::time::{Duration, Instant};

use rusqlite::{Connection, Result};

/// Current schema version. Migrations are forward-only and numbered.
pub const SCHEMA_VERSION: u32 = 1;

/// How long a connection opened through this module waits for a database
/// lock held by another process before SQLite reports `SQLITE_BUSY`
/// (Phase 6.5.3).
///
/// CoreSight is a single-process application, but two processes CAN open
/// the same store at the same time (a second launch, a helper, a test): one
/// migrating while the other waits its turn. Waiting instead of failing
/// immediately turns that collision into "the other process finished
/// first", which the serialized open path then observes. It does NOT make a
/// genuine deadlock recoverable, and it never reports success for a lock
/// that was not actually acquired — if the holder outlasts this bound,
/// SQLite's own `SQLITE_BUSY` is returned and the caller fails closed.
///
/// This is a connection-LOCAL setting and writes nothing to the database.
pub const OPEN_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

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
///
/// The connection carries [`OPEN_BUSY_TIMEOUT`]: a lock held by a
/// concurrent process is waited out (up to that bound) rather than failing
/// immediately. That is a connection-local setting; it still writes
/// nothing to the database.
pub fn open_connection(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // Connection-local, no database writes; the no-write guarantee of this
    // function is unaffected.
    conn.busy_timeout(OPEN_BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Switch the connection's database to WAL journaling.
///
/// SQLite explicitly REFUSES `PRAGMA journal_mode=WAL` inside a transaction,
/// so this runs OUTSIDE any transaction (Phase 6.5.3). Nothing here is a
/// transaction: a caller that later opens one does so after this pragma has
/// completed.
///
/// ## The conversion does not use the busy timeout (measured)
///
/// A journal-mode change takes the database's exclusive lock, and SQLite
/// does NOT invoke the busy handler for it: a concurrent holder makes the
/// pragma answer `SQLITE_BUSY` immediately. The connection's
/// [`OPEN_BUSY_TIMEOUT`] therefore cannot make this wait, so this function
/// retries the conversion itself within the same bound — one attempt per
/// millisecond, never an unbounded spin — and then fails closed with
/// SQLite's own error rather than looping forever or pretending the
/// conversion happened.
///
/// This WRITES to the database, which is why it belongs to
/// [`apply_bootstrap`] and never to [`open_connection`].
fn enable_wal(conn: &Connection) -> Result<()> {
    let deadline = Instant::now() + OPEN_BUSY_TIMEOUT;
    loop {
        match ensure_wal_or_wait(conn) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if !is_busy(&e) || Instant::now() >= deadline {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// One journal-mode attempt: read the current mode and, when it is not WAL,
/// convert. Any `SQLITE_BUSY` from either statement is the caller's cue to
/// wait and retry.
fn ensure_wal_or_wait(conn: &Connection) -> Result<()> {
    // Already WAL → nothing to convert and no exclusive lock to take. This
    // is the overwhelmingly common case for an existing store.
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        return Ok(());
    }
    conn.execute_batch("PRAGMA journal_mode=WAL;")
}

/// Whether an SQLite error is lock contention (`SQLITE_BUSY`) — the one
/// failure the retry loop above waits out.
fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(ffi, _) if ffi.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

/// Whether the database already carries a `schema_version` table.
///
/// The v1 decision is the table's PRESENCE, never a coerced version
/// number: a database that already has the table is never initialized over,
/// whatever it records. Unusable metadata inside such a table is the
/// caller's validation gate's business (a validated caller refuses it), not
/// something this helper corrects.
fn schema_version_table_exists(conn: &Connection) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    Ok(count > 0)
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
///
/// ## Serialized against concurrent first opens (Phase 6.5.3)
///
/// The v1 install is a check-then-act sequence, and two processes opening
/// the same brand-new store would otherwise both see "no schema_version
/// table" and both try to create it. The decision is therefore re-made
/// INSIDE `BEGIN IMMEDIATE` — SQLite's own cross-process write lock, not a
/// process-local mutex — with the connection's busy timeout waiting out a
/// concurrent holder, so exactly one process observes "no table" and
/// installs v1; the other observes the completed install and does nothing.
/// The unlocked peek before the lock is only an optimization for the common
/// case (a store that already has its schema); it decides nothing, and a
/// store that already has the table never takes the write lock at all.
///
/// Both pragmas run OUTSIDE any transaction, as SQLite requires
/// (`journal_mode=WAL` is explicitly refused inside one).
pub fn apply_bootstrap(conn: &Connection) -> Result<()> {
    enable_wal(conn)?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    if schema_version_table_exists(conn)? {
        return Ok(());
    }

    // The serialization point. `BEGIN IMMEDIATE` acquires the write lock up
    // front (waiting per `OPEN_BUSY_TIMEOUT`) — unlike a deferred
    // transaction, it cannot fail later on an upgrade attempt after the
    // check has already run.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let installed = (|| -> Result<()> {
        // Re-made under the lock: this is the decision that counts.
        if !schema_version_table_exists(conn)? {
            // The one migration this crate ships; index 0 holds v1.
            conn.execute_batch(MIGRATIONS[0])?;
        }
        Ok(())
    })();
    match installed {
        Ok(()) => conn.execute_batch("COMMIT"),
        Err(e) => {
            // Best-effort rollback; the caller sees the original failure.
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
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

    /// (Phase 6.5.3, Workstream A) Two processes opening the same brand-new
    /// store at the same time must both succeed and end up with exactly ONE
    /// v1 install: the install decision is re-made inside `BEGIN IMMEDIATE`,
    /// so the loser of the race observes the completed install instead of
    /// trying to create the tables again (which would fail with "table
    /// schema_version already exists"). The barrier starts both threads
    /// together; EVERY interleaving must satisfy the same invariant, so the
    /// test asserts it for whichever order the scheduler produces rather
    /// than assuming one.
    #[test]
    fn concurrent_first_opens_bootstrap_exactly_once() {
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coresight.db");
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let conn = open(&path)?;
                    schema_version(&conn)
                })
            })
            .collect();
        for handle in handles {
            let version = handle
                .join()
                .unwrap()
                .expect("a concurrent first open must succeed");
            assert_eq!(version, SCHEMA_VERSION);
        }

        let conn = open(&path).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the v1 bootstrap must install exactly one row");
        let objects: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('drives', 'scans')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(objects, 2, "both v1 tables must exist exactly once");
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    /// (Phase 6.5.3) The v1 decision is the version table's PRESENCE, not a
    /// version number read and coerced: a database that already has a
    /// `schema_version` table is never initialized over, whatever that table
    /// records. The raw helper adds nothing; a validating caller (the
    /// history store's gates) is what refuses unusable metadata.
    #[test]
    fn bootstrap_never_initializes_a_database_that_has_a_version_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foreign.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 CREATE TABLE marker (x INTEGER);
                 INSERT INTO marker VALUES (7);",
            )
            .unwrap();
        }

        let conn = open(&path).unwrap();
        let version_rows: i64 = conn
            .query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            version_rows, 0,
            "a present version table must not be written by the bootstrap"
        );
        let drives: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'drives'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            drives, 0,
            "the v1 schema must not be installed over a database that has a version table"
        );
        let x: i64 = conn
            .query_row("SELECT x FROM marker", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, 7, "pre-existing data must be untouched");
    }
}
