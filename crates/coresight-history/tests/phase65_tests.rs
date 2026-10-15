//! Phase 6.5 — SQLite pre-migration backup & corrupt-store quarantine.
//!
//! Two properties are proven here, and both are about the FILE, not the
//! in-memory model:
//!
//! 1. **A forward migration never begins without a verified backup.**
//!    [`HistoryStore::open`] inspects an existing store read-only, and only
//!    when the recorded schema is genuinely older does it complete and
//!    publish a backup through SQLite's Online Backup API. A backup that
//!    cannot be created, verified, or published aborts the open, so the
//!    store is never migrated unprotected.
//!
//! 2. **A confirmed-corrupt store is preserved, never replaced.**
//!    Corruption established by `PRAGMA integrity_check` (or by SQLite's own
//!    "this file is not a database" codes) relocates the database and its
//!    `-wal`/`-shm` sidecars into a collision-safe quarantine. `open` still
//!    returns an error naming that location. No empty store is created in
//!    its place and no repair is attempted. An error that is NOT confirmed
//!    corruption (permission, lock, I/O) must leave the files alone.
//!
//! Phase 6.5.1 closes four gaps in those properties, each proven below:
//!
//! - **Concurrent backup attempts are isolated** (`store_safety` unit
//!   tests): every attempt reserves its OWN temporary and destination
//!   names, so a second attempt can neither adopt nor delete a first
//!   attempt's in-flight artifact. Under the old scheme both used the same
//!   temporary path and the second deleted the first's file mid-copy.
//! - **A file this build cannot recognize is refused without being
//!   written** (`UnrecognizedSchema`): a foreign SQLite database, unusable
//!   `schema_version` metadata, a recorded version of 0, or a recorded
//!   version whose required structure is absent (the tables AND signature
//!   columns, Phase 6.5.2) never reaches the mutating bootstrap. Before, a
//!   foreign database was called "current" (it records no version), and
//!   the bootstrap ADDED CoreSight tables to it. The database file and its
//!   `-wal` sidecar are left byte-identical and no sidecar is deleted; the
//!   `-shm` (SQLite's derived WAL index, which the read path rebuilds
//!   whenever a `-wal` is present) is checked for survival, not for bytes —
//!   it holds no data.
//! - **Post-bootstrap corruption is quarantined too**: corruption
//!   established on the already-open connection took a path that returned
//!   the error WITHOUT moving the store aside, so a reliably corrupt store
//!   stayed at a path every future open keeps failing on. (Proven by the
//!   crate-internal failure-path tests in `src/store_hazard_tests.rs`.)
//! - **A failed rollback reports honestly**: a relocation failure after
//!   real moves are already made restores what it can; if the MAIN file
//!   cannot be restored it stays quarantined and the error says exactly
//!   which artifact is where instead of claiming the store was moved or
//!   that everything is back where it was. (Proven by the crate-internal
//!   failure-path tests.)
//!
//! Phase 6.5.2 adds two more, also proven below:
//!
//! - **The recognition contract is the real per-version structure**: a
//!   store recording a version that is missing the tables or signature
//!   columns that version's migrations guarantee is refused with its bytes
//!   preserved — never bootstrapped, migrated, backed up, or quarantined.
//! - **No public API can manufacture corruption evidence**: the hazard
//!   seams are crate-internal now (no `#[doc(hidden)] pub`, no test
//!   feature); this test crate imports nothing from `store_safety`, and
//!   the only public entry point is [`HistoryStore::open`].
//!
//! Every fixture is a real SQLite database in a temporary directory. No real
//! user database, home-directory path, or platform data is ever touched.

use std::fs;
use std::path::{Path, PathBuf};

use coresight_history::store::HISTORY_SCHEMA_VERSION;
use coresight_history::{HistoryStore, QueryLimits, StoreError};
use rusqlite::Connection;

/// The v1 core bootstrap, verbatim — the schema a genuine legacy store has.
/// Deliberately minimal: the v2 tables are created separately.
const V1_BOOTSTRAP: &str = "
    CREATE TABLE schema_version (version INTEGER NOT NULL);
    INSERT INTO schema_version (version) VALUES (1);
    CREATE TABLE drives (
        id TEXT PRIMARY KEY, label TEXT NOT NULL, fs_type TEXT NOT NULL,
        kind TEXT NOT NULL, capacity INTEGER NOT NULL,
        first_seen_at TEXT NOT NULL, seen_last_at TEXT NOT NULL,
        offline INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE scans (
        id TEXT PRIMARY KEY,
        drive_id TEXT NOT NULL REFERENCES drives(id),
        started_at TEXT NOT NULL, finished_at TEXT, status TEXT NOT NULL,
        root TEXT NOT NULL,
        file_count INTEGER NOT NULL DEFAULT 0,
        dir_count INTEGER NOT NULL DEFAULT 0,
        bytes INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_scans_drive ON scans(drive_id);
";

/// A minimal but REAL v2 history store: the core bootstrap plus the v2
/// tables, with one committed run so the backup has representative data to
/// preserve. Written through real SQL, then closed cleanly.
fn build_legacy_v2(db: &Path) {
    let conn = Connection::open(db).unwrap();
    conn.execute_batch(V1_BOOTSTRAP).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS scan_runs (
            run_id TEXT PRIMARY KEY, started_at INTEGER NOT NULL,
            completed_at INTEGER, roots TEXT NOT NULL, platform TEXT NOT NULL,
            config TEXT NOT NULL, status TEXT NOT NULL,
            entries_examined INTEGER NOT NULL DEFAULT 0,
            files INTEGER NOT NULL DEFAULT 0, dirs INTEGER NOT NULL DEFAULT 0,
            links INTEGER NOT NULL DEFAULT 0, other_entries INTEGER NOT NULL DEFAULT 0,
            bytes INTEGER NOT NULL DEFAULT 0,
            observation_errors INTEGER NOT NULL DEFAULT 0,
            candidates_untracked INTEGER NOT NULL DEFAULT 0,
            hash_failures INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS observations (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            path TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER,
            device INTEGER, inode INTEGER, modified INTEGER, category TEXT,
            subcategory TEXT, content_sha256 TEXT, obs_error TEXT
        );
        CREATE TABLE IF NOT EXISTS relationship_obs (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            rel_id TEXT NOT NULL, kind TEXT NOT NULL, rel_status TEXT,
            rel_truncated INTEGER, member_count INTEGER, recoverable INTEGER
        );
        CREATE TABLE IF NOT EXISTS relationship_members (
            run_id TEXT NOT NULL,
            rel_id TEXT NOT NULL,
            path   TEXT NOT NULL,
            device INTEGER,
            inode  INTEGER,
            PRIMARY KEY (run_id, rel_id, path)
        );
        ",
    )
    .unwrap();
    conn.execute("UPDATE schema_version SET version = 2", [])
        .unwrap();
    let config = r#"{"observationModel":1,"classifierSchema":"spacelens.v1.classification","classifierRules":1,"hashAlgorithm":"sha256","relationshipSchema":1,"historySchema":1}"#;
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, completed_at, roots, platform, config, status)
         VALUES ('legacy-run', 1000, 2000, ?1, 'test', ?2, 'COMPLETED')",
        rusqlite::params![r#"["/scope-a"]"#, config],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations
         (run_id, path, kind, size, device, inode, content_sha256)
         VALUES ('legacy-run', '/scope-a/kept.bin', 'FILE', 10, 7, 100, NULL)",
        [],
    )
    .unwrap();
    conn.close().unwrap();
}

/// Every backup artifact sitting next to `db`.
fn backup_artifacts(db: &Path) -> Vec<PathBuf> {
    let parent = db.parent().unwrap().to_path_buf();
    let stem = db.file_name().unwrap().to_string_lossy().into_owned();
    let prefix = format!("{stem}.pre-migration-backup");
    let mut found: Vec<PathBuf> = fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    found.sort();
    found
}

/// Every quarantine directory sitting next to `db`.
fn quarantine_dirs(db: &Path) -> Vec<PathBuf> {
    let parent = db.parent().unwrap().to_path_buf();
    let stem = db.file_name().unwrap().to_string_lossy().into_owned();
    let prefix = format!("{stem}.quarantined");
    let mut found: Vec<PathBuf> = fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
        })
        .collect();
    found.sort();
    found
}

fn schema_version_of(db: &Path) -> u32 {
    let conn = Connection::open(db).unwrap();
    conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Backup: only when a migration is genuinely needed
// ---------------------------------------------------------------------------

/// (1) A brand-new database must not generate a needless backup of itself.
#[test]
fn a_new_database_creates_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("new.db");
    HistoryStore::open(&db).expect("a new store must open");
    assert!(
        backup_artifacts(&db).is_empty(),
        "a new store has no pre-migration state to protect; a backup would be noise"
    );
    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
}

/// (2) An already-current schema must not generate a needless backup.
#[test]
fn a_current_schema_creates_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("current.db");
    {
        let store = HistoryStore::open(&db).unwrap();
        drop(store);
    }
    // Reopen: nothing to migrate, so nothing to back up.
    let store = HistoryStore::open(&db).expect("a current store must reopen");
    drop(store);
    assert!(
        backup_artifacts(&db).is_empty(),
        "a store already at the current version must not be backed up"
    );
}

/// (3) An older valid schema takes a verified, readable backup before the
/// migration runs — and (4) that backup keeps the ORIGINAL version and the
/// ORIGINAL data after the source has migrated.
#[test]
fn an_older_schema_is_backed_up_before_it_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    build_legacy_v2(&db);
    assert_eq!(schema_version_of(&db), 2, "fixture starts at v2");

    HistoryStore::open(&db).expect("a v2 store must migrate to current");
    assert_eq!(
        schema_version_of(&db),
        HISTORY_SCHEMA_VERSION,
        "the source migrated"
    );

    let backups = backup_artifacts(&db);
    assert_eq!(
        backups.len(),
        1,
        "exactly one pre-migration backup must exist: {backups:?}"
    );
    let backup = &backups[0];

    // (4) The backup is a READABLE store at the PRE-migration version...
    let bconn = Connection::open(backup).unwrap();
    let backed_up_version: u32 = bconn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        backed_up_version, 2,
        "the backup must retain the version the source HAD, not the one it reached"
    );
    // ...and it passes its own integrity check.
    let problems: Vec<String> = {
        let mut stmt = bconn.prepare("PRAGMA integrity_check").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(problems, vec!["ok".to_string()]);

    // ...and it carries representative historical data.
    let rows: i64 = bconn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE path = '/scope-a/kept.bin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1, "the backup must contain the legacy observation");
    drop(bconn);

    // The migrated source still has the data too (migration did not lose it).
    // MIGRATION_V3 re-tags every legacy observation path with `u:`/`l:`.
    let sconn = Connection::open(&db).unwrap();
    let after: i64 = sconn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE path = 'u:/scope-a/kept.bin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, 1, "the migrated source kept its row");
}

/// (5) A second migration attempt cannot overwrite an existing backup: each
/// attempt publishes a distinct, collision-safe name.
#[test]
fn a_second_backup_cannot_overwrite_an_existing_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("repeat.db");

    // First migration produces backup #1.
    build_legacy_v2(&db);
    HistoryStore::open(&db).unwrap();
    let first = backup_artifacts(&db);
    assert_eq!(first.len(), 1);
    let first_bytes = fs::read(&first[0]).unwrap();

    // Force a second genuine migration by rewriting the source's recorded
    // version back to 2. The open is expected to FAIL (the v2->v3 ALTER
    // cannot re-run on an already-v6 store), but the naming contract is what
    // is under test: the attempt must reserve a NEW name rather than
    // touching the existing backup.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute("UPDATE schema_version SET version = 2", [])
            .unwrap();
    }
    let _ = HistoryStore::open(&db);

    let backups = backup_artifacts(&db);
    assert_eq!(
        backups.len(),
        2,
        "the second attempt must publish a NEW name, never overwrite: {backups:?}"
    );
    assert_ne!(backups[0], backups[1]);
    assert_eq!(
        fs::read(&backups[0]).unwrap(),
        first_bytes,
        "the first backup must be byte-identical after a second attempt"
    );
    // Both are real, readable stores.
    for b in &backups {
        let c = Connection::open(b).unwrap();
        let v: u32 = c
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2, "each backup retains the version the source had");
    }
}

fn with_raw_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(suffix);
    path.parent().unwrap().join(name)
}

/// (7) The source keeps its pre-migration data and version when a migration
/// itself fails, and the verified backup taken beforehand is RETAINED.
///
/// The migration failure is provoked the way a real one fails, WITHOUT
/// destroying the data the backup is supposed to protect: `MIGRATION_V3`
/// begins with `ALTER TABLE observations ADD COLUMN file_id_hi`, so a store
/// that already carries that column makes the step fail inside its
/// transaction.
#[test]
fn a_failed_migration_keeps_the_pre_migration_state_and_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("migfail.db");
    build_legacy_v2(&db);

    // Provoke the v2->v3 ALTER failure while leaving every legacy row intact.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("ALTER TABLE observations ADD COLUMN file_id_hi INTEGER;")
            .unwrap();
    }

    let err = HistoryStore::open(&db).expect_err("a broken migration must fail");
    assert!(
        !matches!(err, StoreError::BackupFailed { .. }),
        "the backup itself succeeded; the failure is the migration: {err:?}"
    );

    // The backup taken BEFORE the migration survives the failure.
    let backups = backup_artifacts(&db);
    assert_eq!(
        backups.len(),
        1,
        "a verified backup must be retained after a migration failure: {backups:?}"
    );
    let bconn = Connection::open(&backups[0]).unwrap();
    let v: u32 = bconn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 2, "the retained backup is the pre-migration state");
    let rows: i64 = bconn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE path = '/scope-a/kept.bin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1, "the retained backup still has its data");
    drop(bconn);

    // The transactional migration rolled back: the source is still v2 and
    // still holds its rows.
    assert_eq!(
        schema_version_of(&db),
        2,
        "a failed migration must roll its version bump back"
    );
    let sconn = Connection::open(&db).unwrap();
    let still: i64 = sconn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE path = '/scope-a/kept.bin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        still, 1,
        "a failed migration must leave the pre-migration rows intact"
    );
}

// ---------------------------------------------------------------------------
// Quarantine
// ---------------------------------------------------------------------------

/// Build a store whose bytes are genuinely unreadable as a database, so
/// SQLite reports its own "file is not a database" code.
fn build_corrupt_store(db: &Path) {
    build_legacy_v2(db);
    // Corrupt the 16-byte header page so SQLite cannot parse it.
    let mut bytes = fs::read(db).unwrap();
    for b in bytes.iter_mut().take(32) {
        *b = 0x00;
    }
    fs::write(db, &bytes).unwrap();
    // Remove any sidecar so the corruption is read from the main file.
    let _ = fs::remove_file(with_raw_suffix(db, "-wal"));
    let _ = fs::remove_file(with_raw_suffix(db, "-shm"));
}

/// (8) A confirmed-corrupt store returns a typed error AND is quarantined.
#[test]
fn a_corrupt_store_is_quarantined_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("corrupt.db");
    build_corrupt_store(&db);

    let err = HistoryStore::open(&db).expect_err("a corrupt store must not open");
    assert!(
        matches!(err, StoreError::Corrupt { .. }),
        "expected a typed Corrupt error, got {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("integrity check failed"),
        "the error must describe the corruption: {message}"
    );

    let dirs = quarantine_dirs(&db);
    assert_eq!(dirs.len(), 1, "the store must be quarantined: {dirs:?}");
    assert!(
        dirs[0].join("corrupt.db").exists(),
        "the damaged file must be preserved inside the quarantine directory, not deleted"
    );
    assert!(
        message.contains(&dirs[0].display().to_string()),
        "the error must tell the caller where the quarantined store is: {message}"
    );
}

/// (10) The original path must NOT be silently replaced by a new empty store.
#[test]
fn a_quarantined_store_is_not_replaced_by_an_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("corrupt.db");
    build_corrupt_store(&db);

    HistoryStore::open(&db).expect_err("a corrupt store must not open");

    assert!(
        !db.exists(),
        "the corrupt path must be empty — NOT repopulated with a fresh store"
    );
    let quarantined = quarantine_dirs(&db)[0].clone();
    assert!(quarantined.join("corrupt.db").exists());
    // Nothing anywhere claims the data was recovered or repaired.
    let err = HistoryStore::open(&db);
    // Opening the now-absent path legitimately CREATES a fresh store; what
    // must not happen is that it inherits the corrupt one. Prove the fresh
    // store is genuinely new by showing it holds none of the legacy rows.
    let fresh = err.expect("a fresh store may be created once the path is free");
    let runs = fresh.list_runs(&QueryLimits::default()).unwrap();
    assert!(
        runs.is_empty(),
        "a fresh store must contain none of the quarantined history"
    );
}

/// (9) SQLite sidecars present at the time of quarantine are preserved with
/// the database, so uncheckpointed WAL frames are never silently dropped.
#[test]
fn quarantine_preserves_wal_and_shm_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sidecar.db");
    build_corrupt_store(&db);

    // Write real sidecar files next to the corrupt store. Their CONTENT is
    // irrelevant: what is under test is that quarantine RELOCATES them
    // rather than leaving them behind (which would silently discard
    // uncheckpointed WAL state) or deleting them.
    let wal = with_raw_suffix(&db, "-wal");
    let shm = with_raw_suffix(&db, "-shm");
    fs::write(&wal, b"unchecked wal frames").unwrap();
    fs::write(&shm, b"shm index").unwrap();
    let wal_bytes = fs::read(&wal).unwrap();

    HistoryStore::open(&db).expect_err("a corrupt store must not open");

    let quarantined = quarantine_dirs(&db)[0].clone();
    assert!(
        quarantined.join("sidecar.db").exists(),
        "the main file is preserved"
    );
    assert!(
        quarantined.join("sidecar.db-wal").exists(),
        "the -wal sidecar must be preserved, not silently discarded"
    );
    assert_eq!(
        fs::read(quarantined.join("sidecar.db-wal")).unwrap(),
        wal_bytes,
        "the -wal sidecar must be preserved byte-for-byte"
    );
    assert!(
        quarantined.join("sidecar.db-shm").exists(),
        "the -shm sidecar must be preserved"
    );
    // Nothing is left behind at the original location.
    assert!(!db.exists(), "the original database path is left empty");
    assert!(!wal.exists(), "no sidecar may be left at the original path");
    assert!(!shm.exists(), "no sidecar may be left at the original path");
}

/// (11) A quarantine naming collision cannot overwrite an existing
/// quarantined artifact: a second attempt gets its own directory.
#[test]
fn a_quarantine_collision_cannot_overwrite_existing_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("collide.db");

    build_corrupt_store(&db);
    HistoryStore::open(&db).expect_err("first corruption is quarantined");
    let first = quarantine_dirs(&db);
    assert_eq!(first.len(), 1);
    let first_artifact = first[0].join("collide.db");
    let first_bytes = fs::read(&first_artifact).unwrap();

    // Re-create a corrupt store at the same path and corrupt it again: the
    // quarantine directory name is now taken, so the second attempt must
    // use a fresh one and leave the first artifact untouched.
    build_corrupt_store(&db);
    HistoryStore::open(&db).expect_err("second corruption is quarantined");

    let dirs = quarantine_dirs(&db);
    assert_eq!(
        dirs.len(),
        2,
        "a collision must yield a second directory: {dirs:?}"
    );
    assert_eq!(
        fs::read(&first_artifact).unwrap(),
        first_bytes,
        "the earlier quarantined artifact must be untouched"
    );
}

/// (13) An error that is NOT confirmed corruption must NOT quarantine: a
/// directory where the database should be is an environment problem, and
/// nothing may be moved or deleted.
#[test]
fn a_non_corruption_open_failure_does_not_quarantine() {
    let dir = tempfile::tempdir().unwrap();
    // A directory at the store path: SQLite cannot open it as a database,
    // but it is not a corrupt DATABASE either — it is a wrong-path
    // environment problem. Quarantining (i.e. renaming a user directory)
    // would be wrong, so nothing may move.
    let db = dir.path().join("not-a-file.db");
    fs::create_dir(&db).unwrap();
    fs::write(db.join("important.txt"), b"user data").unwrap();

    let err = HistoryStore::open(&db).expect_err("opening a directory must fail");
    assert!(
        !matches!(err, StoreError::Corrupt { .. })
            && !matches!(err, StoreError::QuarantineFailed { .. }),
        "a non-corruption failure must not be reported as corruption: {err:?}"
    );
    assert!(
        db.join("important.txt").exists(),
        "nothing may be moved or deleted for a non-corruption failure"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "no quarantine directory may be created for a non-corruption failure"
    );
}

/// (14) A valid store whose schema is NEWER than supported is refused and
/// left completely untouched — not migrated, not backed up, not quarantined.
#[test]
fn a_newer_schema_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("future.db");
    build_legacy_v2(&db);
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE schema_version SET version = ?1",
            rusqlite::params![HISTORY_SCHEMA_VERSION + 1],
        )
        .unwrap();
    }
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open(&db).expect_err("a newer schema must be refused");
    match err {
        StoreError::SchemaTooNew { found, supported } => {
            assert_eq!(found, HISTORY_SCHEMA_VERSION + 1);
            assert_eq!(supported, HISTORY_SCHEMA_VERSION);
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "a newer store must be left byte-identical"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "a newer store must NOT be backed up — this build does not own it"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "a newer store must NOT be quarantined — it is not corrupt"
    );
}

// ---------------------------------------------------------------------------
// Recognition (Phase 6.5.1, Workstream B): only a recognizable store is
// ever touched
// ---------------------------------------------------------------------------

/// (15) A foreign SQLite database — objects but no `schema_version` table —
/// is refused and NOTHING durable about it changes: no CoreSight tables
/// added, no backup, no quarantine, the database file and its `-wal`
/// byte-identical, no sidecar deleted. (SQLite may rebuild the `-shm` WAL
/// index while reading a file that carries a `-wal`; it is a derived cache
/// and holds no data.)
#[test]
fn a_foreign_sqlite_database_is_refused_and_left_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("foreign.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             INSERT INTO users (id, name) VALUES (1, 'someone');",
        )
        .unwrap();
    }
    // Stale sidecars next to it must not be deleted, and the `-wal`
    // (uncheckpointed frames) must be byte-identical. The `-shm` is an
    // index SQLite derives from the `-wal` and may rebuild it while
    // reading, so it is checked for survival, never for bytes.
    let wal = with_raw_suffix(&db, "-wal");
    let shm = with_raw_suffix(&db, "-shm");
    fs::write(&wal, b"foreign wal bytes").unwrap();
    fs::write(&shm, b"foreign shm bytes").unwrap();
    let before = fs::read(&db).unwrap();
    let wal_before = fs::read(&wal).unwrap();

    let err = HistoryStore::open(&db).expect_err("a foreign database must be refused");
    match &err {
        StoreError::UnrecognizedSchema { detail } => assert!(
            detail.contains("no schema_version"),
            "the refusal must say what made the file unrecognizable: {detail}"
        ),
        other => panic!("expected UnrecognizedSchema, got {other:?}"),
    }

    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the foreign file must be byte-identical after the refusal"
    );
    assert_eq!(
        fs::read(&wal).unwrap(),
        wal_before,
        "the -wal must be untouched"
    );
    assert!(
        shm.exists(),
        "the refusal must not delete the -shm sidecar (SQLite may rebuild it \
         while reading; it must never be lost)"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "an unrecognized file must not be backed up"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "an unrecognized file is not corrupt and must not be quarantined"
    );

    // The decisive regression: no CoreSight table may have been ADDED.
    let conn = Connection::open(&db).unwrap();
    let added: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN
             ('schema_version', 'drives', 'scans', 'scan_runs', 'observations')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        added, 0,
        "the mutating bootstrap must never run on a foreign database"
    );
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "the foreign data must be intact");
}

/// (16) A foreign database that HAS a `schema_version` table (a common
/// name) is refused when the core tables of its recorded version are
/// absent: a version table alone is not recognizability.
#[test]
fn a_foreign_database_with_a_schema_version_table_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("foreign-versioned.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (2);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
    }
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open(&db).expect_err("a foreign versioned database must be refused");
    match &err {
        StoreError::UnrecognizedSchema { detail } => {
            assert!(
                detail.contains("missing core tables"),
                "the refusal must name the missing core tables: {detail}"
            );
            assert!(
                detail.contains("scan_runs") && detail.contains("observations"),
                "the refusal must say WHICH core tables are missing: {detail}"
            );
        }
        other => panic!("expected UnrecognizedSchema, got {other:?}"),
    }
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the file must be byte-identical"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "a refused file must not be backed up — it is not this build's store"
    );
    assert!(quarantine_dirs(&db).is_empty(), "it is not corrupt either");
}

/// (17) `schema_version` metadata that does not hold exactly one
/// non-negative integer is refused — never coerced, never defaulted to 0.
#[test]
fn unusable_version_metadata_is_refused_not_guessed() {
    for (label, version_ddl) in [
        (
            "empty table",
            "CREATE TABLE schema_version (version INTEGER NOT NULL);",
        ),
        (
            "two rows",
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (3);
             INSERT INTO schema_version VALUES (4);",
        ),
        (
            "non-integer",
            "CREATE TABLE schema_version (version TEXT NOT NULL);
             INSERT INTO schema_version VALUES ('six');",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("unusable.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(version_ddl).unwrap();
            conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);")
                .unwrap();
        }
        let before = fs::read(&db).unwrap();

        let err = HistoryStore::open(&db)
            .expect_err(&format!("{label}: unusable metadata must be refused"));
        assert!(
            matches!(err, StoreError::UnrecognizedSchema { .. }),
            "{label}: expected UnrecognizedSchema, got {err:?}"
        );
        assert_eq!(fs::read(&db).unwrap(), before, "{label}: byte-identical");
        assert!(
            backup_artifacts(&db).is_empty(),
            "{label}: no backup may be taken"
        );
        assert!(
            quarantine_dirs(&db).is_empty(),
            "{label}: no quarantine may be created"
        );
    }
}

/// (18) A store explicitly recording version 0 is refused: no CoreSight
/// build has ever written version 0. (The old code treated it as "needs
/// migration", took a backup, and then died inside the bootstrap, leaving
/// a published backup of a store it could not migrate.)
#[test]
fn a_store_recording_version_zero_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("zero.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (0);",
        )
        .unwrap();
    }
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open(&db).expect_err("version 0 must be refused");
    match &err {
        StoreError::UnrecognizedSchema { detail } => {
            assert!(detail.contains("version 0"), "{detail}");
        }
        other => panic!("expected UnrecognizedSchema, got {other:?}"),
    }
    assert_eq!(fs::read(&db).unwrap(), before);
    assert!(backup_artifacts(&db).is_empty());
    assert!(quarantine_dirs(&db).is_empty());
}

/// (19) An EMPTY database file is initialized, not refused: it holds no
/// schema of anyone's, so the v1 bootstrap is exactly how this build
/// creates a store. Both a zero-byte file and a valid database whose
/// objects were all dropped count as empty.
#[test]
fn an_empty_database_file_is_initialized_not_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("zero-byte.db");
    fs::write(&db, b"").unwrap();

    HistoryStore::open(&db).expect("a zero-byte file must be initialized");
    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(
        backup_artifacts(&db).is_empty(),
        "an empty file has nothing to protect and must not be backed up"
    );

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("emptied.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE scratch (x INTEGER); DROP TABLE scratch;")
            .unwrap();
    }
    HistoryStore::open(&db).expect("an object-less database must be initialized");
    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(backup_artifacts(&db).is_empty());
}

/// (20) A genuine EXISTING v1 store (the Phase-0 bootstrap) is recognized,
/// backed up at version 1, and migrated. New-file bootstraps were already
/// covered; an existing v1 FILE — the store a user created before the
/// history layer existed — was not.
#[test]
fn an_existing_v1_store_is_backed_up_and_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v1.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(V1_BOOTSTRAP).unwrap();
    }

    HistoryStore::open(&db).expect("an existing v1 store must migrate to current");
    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);

    let backups = backup_artifacts(&db);
    assert_eq!(
        backups.len(),
        1,
        "a v1 store must be backed up before migrating: {backups:?}"
    );
    let bconn = Connection::open(&backups[0]).unwrap();
    let v: u32 = bconn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 1, "the backup keeps the version the source had");
    let drives: i64 = bconn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'drives'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drives, 1, "the backup is the genuine v1 bootstrap");
    drop(bconn);

    // The migrated source kept its v1 tables (drives/scans survive).
    let sconn = Connection::open(&db).unwrap();
    let drives: i64 = sconn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'drives'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drives, 1, "the source keeps its v1 tables");
}

// ---------------------------------------------------------------------------
// The full recognition contract (Phase 6.5.2, Workstream C): the
// per-version structure a real store has, not just a table name or two
// ---------------------------------------------------------------------------

/// (26) An incomplete v2 store — it records version 2, but it has only
/// `scan_runs` + `observations` and lacks the v1 bootstrap tables and half
/// the v2 tables — is refused as `UnrecognizedSchema` and left untouched:
/// no bootstrap table added, no migration, no backup, no quarantine, the
/// database and its `-wal` byte-identical, no sidecar deleted.
///
/// This is the decisive regression for the recognition contract: under the
/// Phase 6.5.1 rule (`scan_runs` + `observations` present) this file WAS
/// recognized, so the mutating bootstrap would have run on it.
#[test]
fn an_incomplete_v2_store_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("incomplete-v2.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (2);
             CREATE TABLE scan_runs (
                 run_id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, roots TEXT NOT NULL,
                 platform TEXT NOT NULL, config TEXT NOT NULL, status TEXT NOT NULL
             );
             CREATE TABLE observations (
                 run_id TEXT NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL
             );",
        )
        .unwrap();
    }
    // A stale `-wal` beside it must survive the refusal byte-identically:
    // the check must not delete a sidecar of a store it did not write. (The
    // file is in rollback journal mode, so this stray WAL is inert.)
    let wal = with_raw_suffix(&db, "-wal");
    fs::write(&wal, b"stale wal bytes").unwrap();
    let before = fs::read(&db).unwrap();
    let wal_before = fs::read(&wal).unwrap();

    let err = HistoryStore::open(&db).expect_err("an incomplete v2 store must be refused");
    match &err {
        StoreError::UnrecognizedSchema { detail } => {
            for needle in [
                "missing core tables",
                "missing table drives",
                "missing table scans",
                "missing table relationship_obs",
                "missing table relationship_members",
            ] {
                assert!(
                    detail.contains(needle),
                    "the refusal must name {needle:?}: {detail}"
                );
            }
        }
        other => panic!("expected UnrecognizedSchema, got {other:?}"),
    }

    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the refused database must be byte-identical"
    );
    assert!(wal.exists(), "no sidecar may be deleted");
    assert_eq!(
        fs::read(&wal).unwrap(),
        wal_before,
        "the -wal must be byte-identical"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "an unrecognized file must not be backed up"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "an unrecognized file is not corrupt and must not be quarantined"
    );
    assert_eq!(schema_version_of(&db), 2, "no migration may run");

    let conn = Connection::open(&db).unwrap();
    let added: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN
             ('drives', 'scans', 'relationship_obs', 'relationship_members')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(added, 0, "no CoreSight table may be added");
}

/// (27) A store that records v6 but is missing a signature COLUMN the v6
/// migration guarantees (`app_snapshot_apps.id_encoding`) is refused —
/// having every table is not enough when the recorded version promises the
/// column — and is left byte-identical: no bootstrap, no migration, no
/// backup, no quarantine.
#[test]
fn a_v6_store_missing_a_required_column_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v6-missing-column.db");
    {
        let store = HistoryStore::open(&db).expect("fixture: a fresh store must open");
        drop(store);
    }
    // Remove the v6 column: a plain schema edit, exactly the shape a
    // partial lookalike (or a damaged store) has.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("ALTER TABLE app_snapshot_apps DROP COLUMN id_encoding;")
            .unwrap();
    }
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open(&db).expect_err("an incomplete v6 store must be refused");
    match &err {
        StoreError::UnrecognizedSchema { detail } => assert!(
            detail.contains("missing column app_snapshot_apps.id_encoding"),
            "the refusal must name the missing column: {detail}"
        ),
        other => panic!("expected UnrecognizedSchema, got {other:?}"),
    }

    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the refused database must be byte-identical"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "an unrecognized file must not be backed up"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "an unrecognized file is not corrupt and must not be quarantined"
    );
    assert_eq!(
        schema_version_of(&db),
        HISTORY_SCHEMA_VERSION,
        "no migration may run and the version must not change"
    );
}

/// (28) A store that records v6 but is missing a v5 table is refused the
/// same way: the structure contract follows the recorded version through
/// every migration, not just the newest one.
#[test]
fn a_v6_store_missing_a_required_table_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v6-missing-table.db");
    {
        let store = HistoryStore::open(&db).expect("fixture: a fresh store must open");
        drop(store);
    }
    // `app_snapshot_history` is referenced by no other table, so dropping
    // it leaves a clean (if incomplete) schema.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("DROP TABLE app_snapshot_history;")
            .unwrap();
    }
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open(&db).expect_err("an incomplete v6 store must be refused");
    match &err {
        StoreError::UnrecognizedSchema { detail } => assert!(
            detail.contains("missing table app_snapshot_history"),
            "the refusal must name the missing table: {detail}"
        ),
        other => panic!("expected UnrecognizedSchema, got {other:?}"),
    }

    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the refused database must be byte-identical"
    );
    assert!(backup_artifacts(&db).is_empty(), "no backup may be taken");
    assert!(
        quarantine_dirs(&db).is_empty(),
        "nothing may be quarantined"
    );
    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
}
