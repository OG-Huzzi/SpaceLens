//! Crate-internal failure-path tests for the store-open sequence
//! (Phase 6.5 / 6.5.2).
//!
//! These tests drive faults production cannot provoke on demand — a disk
//! that cannot be written, a rename that fails mid-quarantine, an integrity
//! check that answers with nothing. The seams they use ([`Hazard`],
//! [`PostBootstrapFault`], [`HistoryStore::open_impl`],
//! [`HistoryStore::open_impl_observed`]) are CRATE-INTERNAL (Phase 6.5.2,
//! Workstream A): no public caller can name a hazard, force a quarantine on
//! a healthy store, or reach the low-level backup/quarantine helpers. The
//! public entry point is `HistoryStore::open`, and the public-API tests
//! live in `tests/phase65_tests.rs`.
//!
//! Every fixture is a real SQLite database in a temporary directory. No
//! real user database, home-directory path, or platform data is ever
//! touched.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{HistoryStore, OpenStage, StoreError, HISTORY_SCHEMA_VERSION};
use crate::store_safety::{verified_pre_migration_backup, Hazard, PostBootstrapFault};

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

fn with_raw_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(suffix);
    path.parent().unwrap().join(name)
}

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

/// A healthy store already at the current version: nothing to migrate, so
/// no backup is involved and the only variable is the gate under test.
fn build_current_store(db: &Path) {
    let store = HistoryStore::open(db).expect("fixture: a fresh store must open");
    drop(store);
}

/// A corrupt store with BOTH sidecars present as real files, so a
/// relocation failure has already-moved artifacts to roll back.
/// Returns `(wal, wal_bytes, shm)`. The `-shm` is a derived index that
/// SQLite's read path rebuilds when a `-wal` is present, so its BYTES are
/// not part of the contract — only its survival at the original path.
fn build_corrupt_store_with_sidecars(db: &Path) -> (PathBuf, Vec<u8>, PathBuf) {
    build_corrupt_store(db);
    let wal = with_raw_suffix(db, "-wal");
    let shm = with_raw_suffix(db, "-shm");
    fs::write(&wal, b"uncheckpointed wal frames").unwrap();
    fs::write(&shm, b"shm index state").unwrap();
    let wal_bytes = fs::read(&wal).unwrap();
    (wal, wal_bytes, shm)
}

// ---------------------------------------------------------------------------
// Pre-migration backup (Phase 6.5)
// ---------------------------------------------------------------------------

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
        let err = HistoryStore::open_impl(&db, hazard)
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
        let leftovers: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains("backup-in-progress"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "{label}: the temporary artifact must be cleaned up: {leftovers:?}"
        );
    }
}

/// (6.5.2, Workstream B) A backup VERIFICATION whose check cannot establish
/// integrity — here because it returned no rows at all — must never publish
/// the artifact, and the open must abort with the source unchanged. This is
/// the backup gate of the fail-closed rule.
#[test]
fn an_empty_backup_verification_result_never_publishes_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("verify-empty.db");
    build_legacy_v2(&db);
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open_impl(&db, Hazard::BackupVerifyReportsNoRows)
        .expect_err("an unverifiable backup must abort the open");
    match &err {
        StoreError::BackupFailed { step, detail } => {
            assert_eq!(step, "verify");
            assert!(
                detail.contains("integrity could not be established"),
                "the failure must say what could not be established: {detail}"
            );
        }
        other => panic!("expected BackupFailed, got {other:?}"),
    }

    assert!(
        backup_artifacts(&db).is_empty(),
        "an unverifiable backup must not be published under the backup name"
    );
    let leftovers: Vec<PathBuf> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains("backup-in-progress"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "the failed attempt's temporary artifact must be cleaned up: {leftovers:?}"
    );
    assert_eq!(schema_version_of(&db), 2, "the store must not be migrated");
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the source must be byte-identical"
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
    let published = verified_pre_migration_backup(&src, &db, Some(2), Hazard::None)
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
// Quarantine (Phase 6.5 / 6.5.1)
// ---------------------------------------------------------------------------

/// (12) A relocation failure is rolled back: the store is restored to its
/// original location and the error does NOT claim a successful quarantine.
#[test]
fn a_relocation_failure_restores_the_store_and_reports_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("relocate-fail.db");
    build_corrupt_store(&db);
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open_impl(&db, Hazard::QuarantineRelocate)
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

/// (21) Corruption established AFTER the bootstrap writes — on the
/// already-open connection — is quarantined exactly like preflight
/// corruption; never merely refused with the store left at a path every
/// future open keeps failing on.
#[test]
fn post_bootstrap_corruption_is_quarantined_like_preflight_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("late-corrupt.db");
    build_current_store(&db);

    let err = HistoryStore::open_impl(
        &db,
        Hazard::PostBootstrapIntegrity(PostBootstrapFault::ReportsCorruption),
    )
    .expect_err("injected post-bootstrap corruption must abort the open");

    assert!(
        matches!(err, StoreError::Corrupt { .. }),
        "expected a typed Corrupt error, got {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("database integrity check failed on open:"),
        "the established prefix must be preserved: {message}"
    );
    assert!(
        message.contains("injected post-bootstrap corruption"),
        "the evidence must name the injected corruption: {message}"
    );

    let dirs = quarantine_dirs(&db);
    assert_eq!(dirs.len(), 1, "the store must be quarantined: {dirs:?}");
    assert!(
        dirs[0].join("late-corrupt.db").exists(),
        "the store file must be preserved inside the quarantine directory"
    );
    assert!(
        message.contains(&dirs[0].display().to_string()),
        "the error must name the quarantine location: {message}"
    );
    assert!(
        !db.exists(),
        "no replacement store may be left at the original path"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "a store already at the current version is not backed up"
    );
}

/// (22) A post-bootstrap check that cannot be READ for a non-corruption
/// reason preserves the store: nothing may be quarantined on an
/// environment failure.
#[test]
fn a_post_bootstrap_io_failure_preserves_the_store_without_quarantine() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("late-io.db");
    build_current_store(&db);

    let err = HistoryStore::open_impl(
        &db,
        Hazard::PostBootstrapIntegrity(PostBootstrapFault::UnreadableIoError),
    )
    .expect_err("an unreadable (I/O) result must abort the open");
    assert!(
        !matches!(err, StoreError::Corrupt { .. }),
        "an I/O failure is not corruption and must not be reported as it: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("injected unreadable integrity result (I/O)"),
        "the environment failure must be surfaced as-is: {err}"
    );

    assert!(
        db.exists(),
        "an environment failure must not move the store"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "nothing may be quarantined for a non-corruption failure"
    );
    assert!(backup_artifacts(&db).is_empty());
    let store = HistoryStore::open(&db).expect("the store must still be readable");
    drop(store);
}

/// (23) A post-bootstrap result that cannot be READ with SQLite's own
/// corruption code is positive corruption evidence: it is quarantined
/// rather than smoothed into "healthy" or reported without a quarantine.
#[test]
fn a_post_bootstrap_unreadable_corruption_result_is_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("late-unreadable.db");
    build_current_store(&db);

    let err = HistoryStore::open_impl(
        &db,
        Hazard::PostBootstrapIntegrity(PostBootstrapFault::UnreadableCorruption),
    )
    .expect_err("an unreadable (corruption) result must abort the open");
    match &err {
        StoreError::Corrupt { detail, .. } => assert!(
            detail.contains("injected unreadable integrity result (corruption)"),
            "{detail}"
        ),
        other => panic!("expected Corrupt, got {other:?}"),
    }

    let dirs = quarantine_dirs(&db);
    assert_eq!(
        dirs.len(),
        1,
        "corruption established from an unreadable result must quarantine: {dirs:?}"
    );
    assert!(dirs[0].join("late-unreadable.db").exists());
}

/// (6.5.2, Workstream B) The POST-BOOTSTRAP integrity check returning NO
/// rows fails closed exactly like the other two decision sites: a typed
/// failure to establish integrity (no corruption code, no quarantine), with
/// the store preserved and still usable.
#[test]
fn an_empty_post_bootstrap_integrity_result_fails_closed_without_quarantine() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("late-empty-rows.db");
    build_current_store(&db);

    let err = HistoryStore::open_impl(
        &db,
        Hazard::PostBootstrapIntegrity(PostBootstrapFault::ReportsNoRows),
    )
    .expect_err("an empty post-bootstrap result must not be read as healthy");
    assert!(
        !matches!(err, StoreError::Corrupt { .. }),
        "an empty result is not evidence of damage: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("integrity could not be established"),
        "the error must say what could not be established: {err}"
    );

    assert!(db.exists(), "the store must not be moved");
    assert!(
        quarantine_dirs(&db).is_empty(),
        "nothing may be quarantined on a decision that was never made"
    );
    assert!(backup_artifacts(&db).is_empty());
    let store = HistoryStore::open(&db).expect("the store must still be readable");
    drop(store);
}

/// (24) A relocation failure AFTER real moves were made restores every
/// MOVED artifact byte-identically and reports truthfully that the store
/// was NOT moved. (Test 12 covers failing before anything moved; this
/// covers the rollback with a real move list — the main file and the
/// `-wal` had already moved when the `-shm` relocation failed.)
#[test]
fn a_sidecar_relocation_failure_restores_everything_that_moved() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sidecar-fail.db");
    let (wal, wal_bytes, shm) = build_corrupt_store_with_sidecars(&db);
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open_impl(&db, Hazard::QuarantineRelocateSidecar)
        .expect_err("an injected sidecar relocation failure must abort the open");
    match &err {
        StoreError::Corrupt { detail, .. } => {
            assert!(
                detail.contains("could NOT be moved aside"),
                "a fully rolled-back quarantine must say the store was NOT moved: {detail}"
            );
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }

    assert!(db.exists(), "the main store must be restored");
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the restored main store must be byte-identical"
    );
    assert_eq!(
        fs::read(&wal).unwrap(),
        wal_bytes,
        "the -wal must be restored byte-identically"
    );
    assert!(
        shm.exists(),
        "the -shm must be back at its original path — the rollback of the \
         relocations must never lose a sidecar (SQLite may rebuild this \
         derived index while reading; its bytes are not the contract)"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "a fully rolled-back quarantine must not leave a directory behind"
    );
    assert!(backup_artifacts(&db).is_empty());
}

/// (25) When the rollback itself fails for the MAIN database, the error
/// reports a partial state honestly: the main file stays inside the
/// quarantine directory, the sidecars are back where they were, and the
/// message claims neither a successful quarantine nor that everything is
/// unchanged.
#[test]
fn an_unrestorable_main_file_is_reported_honestly_and_stays_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("restore-fail.db");
    let (wal, wal_bytes, shm) = build_corrupt_store_with_sidecars(&db);
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open_impl(&db, Hazard::QuarantineRestore)
        .expect_err("an injected restore failure must abort the open");
    let detail = match &err {
        StoreError::Corrupt { detail, .. } => detail.clone(),
        other => panic!("expected Corrupt, got {other:?}"),
    };

    assert!(
        detail.contains("could NOT be quarantined cleanly"),
        "the partial failure must say the quarantine was not clean: {detail}"
    );
    assert!(
        detail.contains("NOT all at their original locations"),
        "the partial failure must not claim the files are where they were: {detail}"
    );
    assert!(
        !detail.contains("could NOT be moved aside"),
        "the partial-state report must not be downgraded to the fully-restored wording: {detail}"
    );
    assert!(
        !detail.contains("was preserved (not deleted, not repaired) under"),
        "no successful quarantine may be claimed: {detail}"
    );

    // The main file could not be restored: it remains quarantined, and the
    // error says exactly where.
    assert!(
        !db.exists(),
        "the main file did not return to its original path"
    );
    let dirs = quarantine_dirs(&db);
    assert_eq!(
        dirs.len(),
        1,
        "the quarantine directory must remain: {dirs:?}"
    );
    let quarantined = dirs[0].join("restore-fail.db");
    assert!(
        quarantined.exists(),
        "the main file must remain inside the quarantine directory"
    );
    assert_eq!(
        fs::read(&quarantined).unwrap(),
        before,
        "the quarantined file must be byte-identical to what was moved"
    );
    assert!(
        detail.contains(&quarantined.display().to_string()),
        "the error must name where the unrestorable artifact is: {detail}"
    );

    // The sidecars WERE restored to their original path.
    assert_eq!(
        fs::read(&wal).unwrap(),
        wal_bytes,
        "the -wal must have returned to its original path"
    );
    assert!(
        shm.exists(),
        "the -shm must have returned to its original path (SQLite may have \
         rebuilt this derived index while reading; it must never be lost)"
    );
    assert!(backup_artifacts(&db).is_empty());
}

/// (6.5.2, Workstream B) The PREFLIGHT integrity check returning NO rows
/// establishes nothing: the open must abort WITHOUT quarantining — an empty
/// result is neither health nor damage — and the store must open normally
/// afterwards.
#[test]
fn an_empty_preflight_integrity_result_fails_closed_without_quarantine() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty-rows.db");
    build_current_store(&db);
    let before = fs::read(&db).unwrap();

    let err = HistoryStore::open_impl(&db, Hazard::IntegrityReportsNoRows)
        .expect_err("an empty integrity result must not be read as healthy");
    assert!(
        !matches!(err, StoreError::Corrupt { .. }),
        "an empty result is not evidence of damage: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("integrity could not be established"),
        "the error must say what could not be established: {err}"
    );

    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "the store must be byte-untouched"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "nothing may be quarantined on a decision that was never made"
    );
    assert!(backup_artifacts(&db).is_empty());

    // The store itself is healthy: a normal open still works.
    drop(HistoryStore::open(&db).expect("the store must still open normally"));
}

// ---------------------------------------------------------------------------
// The inspection-to-mutation boundary (Phase 6.5.2, Workstream D)
// ---------------------------------------------------------------------------

/// A database SUBSTITUTED at the store path between the read-only gate and
/// the mutation gate is validated by the mutation gate — on the very
/// connection that would write it — before anything is written. Here the
/// substitute is a foreign SQLite database; the open refuses it and it
/// comes out byte-identical, with no CoreSight table added, no WAL mode
/// set, no backup and no quarantine. The genuine store survives the swap at
/// the path the test moved it to.
#[test]
fn a_database_swapped_in_after_the_read_only_gate_is_gated_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swap.db");
    let moved = dir.path().join("original-moved.db");
    let introduced = dir.path().join("introduced.db");

    // A genuine current store at the path, and a foreign database (a
    // readable SQLite file with no CoreSight structure) waiting to be
    // swapped in at exactly the boundary between the two gates.
    build_current_store(&db);
    {
        let conn = Connection::open(&introduced).unwrap();
        conn.execute_batch(
            "CREATE TABLE keep (x INTEGER);
             INSERT INTO keep VALUES (1);",
        )
        .unwrap();
    }
    let introduced_bytes = fs::read(&introduced).unwrap();

    let mut swaps = 0;
    let err = HistoryStore::open_impl_observed(&db, Hazard::None, &mut |stage| {
        // The stage enum has exactly this variant; the `let` is the
        // explicit "this is the boundary" statement.
        let OpenStage::PreflightPassed(_) = stage;
        // No connection is held here: the read-only gate has closed and
        // the mutating connection does not exist yet. That is exactly
        // the boundary the threat model names.
        fs::rename(&db, &moved).unwrap();
        fs::rename(&introduced, &db).unwrap();
        swaps += 1;
    })
    .expect_err("the database introduced after the read-only gate must be refused");
    assert_eq!(swaps, 1, "the boundary observer must run exactly once");

    // The introduced database was refused WITHOUT being written:
    // byte-identical, no CoreSight table added, still in rollback journal
    // mode (the bootstrap, which sets WAL, never ran).
    assert!(
        matches!(err, StoreError::UnrecognizedSchema { .. }),
        "expected UnrecognizedSchema, got {err:?}"
    );
    assert_eq!(
        fs::read(&db).unwrap(),
        introduced_bytes,
        "the introduced database must be byte-identical after the refusal"
    );
    {
        let conn = Connection::open(&db).unwrap();
        let core_tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN \
                 ('schema_version', 'drives', 'scans', 'scan_runs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(core_tables, 0, "no CoreSight table may be added to it");
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            journal.to_lowercase(),
            "delete",
            "the bootstrap (which sets WAL) must never have run on the introduced file"
        );
    }

    // The genuine store is still exactly what it was, at the path the
    // boundary moved it to, and no artifact was created for either file.
    assert!(
        moved.exists(),
        "the genuine store must still exist where the boundary moved it"
    );
    assert_eq!(
        schema_version_of(&moved),
        HISTORY_SCHEMA_VERSION,
        "the genuine store must be untouched at its moved path"
    );
    assert!(
        backup_artifacts(&db).is_empty(),
        "no backup may be published for a refused open"
    );
    assert!(
        quarantine_dirs(&db).is_empty(),
        "a refusal at the mutation gate must not quarantine"
    );
}
