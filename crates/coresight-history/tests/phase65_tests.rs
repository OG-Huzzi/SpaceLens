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
//! Every fixture is a real SQLite database in a temporary directory. No real
//! user database, home-directory path, or platform data is ever touched.

use std::fs;
use std::path::{Path, PathBuf};

use coresight_history::store::HISTORY_SCHEMA_VERSION;
use coresight_history::store_safety::Hazard;
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

/// (6) A backup failure at EVERY stage prevents the migration and leaves the
/// source exactly as it was. Each injected hazard stands for a real fault.
#[test]
fn a_backup_failure_prevents_migration_and_preserves_the_source() {
    for (label, hazard) in [
        ("copy", Hazard::BackupCopy),
        ("verify", Hazard::BackupVerify),
        ("publish", Hazard::BackupPublish),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hazard.db");
        build_legacy_v2(&db);

        let before = fs::read(&db).unwrap();
        let err = HistoryStore::open_with_hazard(&db, hazard)
            .expect_err(&format!("a {label} failure must abort the open"));
        assert!(
            matches!(err, StoreError::BackupFailed { .. }),
            "{label}: expected BackupFailed, got {err:?}"
        );

        assert_eq!(
            schema_version_of(&db),
            2,
            "{label}: a failed backup must NOT migrate the store"
        );
        assert_eq!(
            fs::read(&db).unwrap(),
            before,
            "{label}: a failed backup must leave the source byte-identical"
        );
        // Nothing was published under the backup name, and the temporary
        // artifact this attempt owned was cleaned up.
        assert!(
            backup_artifacts(&db).is_empty(),
            "{label}: a failed attempt must publish nothing"
        );
        assert!(
            !db.with_extension("backup-in-progress").exists()
                && !with_raw_suffix(&db, ".backup-in-progress").exists(),
            "{label}: the temporary artifact must be cleaned up"
        );
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

/// The backup is SQLite-consistent, not a file copy: data that exists ONLY
/// in an uncheckpointed write-ahead log must appear in the backup.
///
/// This is the specific failure a raw `fs::copy` of the main database file
/// would produce: the main file is a valid, older image and the newest
/// committed rows live in `-wal`. Copying only the main file would publish a
/// backup that silently omits committed history.
#[test]
fn a_backup_captures_data_held_only_in_the_uncheckpointed_wal() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("wal.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (1);
             CREATE TABLE keeper(marker TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute("UPDATE schema_version SET version = 2", [])
            .unwrap();
        conn.execute("INSERT INTO keeper VALUES ('committed-row')", [])
            .unwrap();
        // Leaking the connection leaves the WAL un-checkpointed, so the row
        // above exists ONLY in the -wal file.
        std::mem::forget(conn);
    }
    let wal = with_raw_suffix(&db, "-wal");
    assert!(
        wal.exists(),
        "the fixture must really have an uncheckpointed WAL"
    );

    let src = Connection::open(&db).unwrap();
    let published = coresight_history::store_safety::verified_pre_migration_backup(
        &src,
        &db,
        Some(2),
        Hazard::None,
    )
    .expect("a WAL-mode store must still back up");
    drop(src);

    let b = Connection::open(&published).unwrap();
    let rows: i64 = b
        .query_row(
            "SELECT COUNT(*) FROM keeper WHERE marker = 'committed-row'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 1,
        "the backup must include rows that exist only in the uncheckpointed WAL"
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

/// (12) A relocation failure is rolled back: the store is restored to its
/// original location and the error does NOT claim a successful quarantine.
#[test]
fn a_relocation_failure_restores_the_store_and_reports_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("relocate-fail.db");
    build_corrupt_store(&db);
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open_with_hazard(&db, Hazard::QuarantineRelocate)
        .expect_err("an injected relocation failure must abort the open");

    match &err {
        StoreError::Corrupt { detail, .. } => {
            assert!(
                detail.contains("could NOT be moved aside"),
                "a failed quarantine must be reported truthfully: {detail}"
            );
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
    assert!(
        db.exists(),
        "the store must be restored to its original location"
    );
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "restoration must be byte-identical"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "a rolled-back quarantine must not leave an empty directory behind"
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
