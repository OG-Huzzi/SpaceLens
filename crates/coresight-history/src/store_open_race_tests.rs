//! Crate-internal concurrency and file-identity tests for the store-open
//! sequence (Phase 6.5.3).
//!
//! Three properties are proven here, and every one of them is about the
//! FILE and the DECISION, not the in-memory model:
//!
//! 1. **Two openers of the same older store apply each migration once.**
//!    Each migration step takes SQLite's own cross-process write lock
//!    (`BEGIN IMMEDIATE`) and re-reads the recorded version INSIDE it, so a
//!    loser observes the winner's committed step and skips it instead of
//!    applying a stale plan and failing on a duplicate `ADD COLUMN`.
//! 2. **A writer that changes the store between the integrity check and the
//!    first mutation cannot make an opener act on stale facts.** The
//!    version the opener decides from is the one it reads at decision time,
//!    and the plan it applies is re-derived under the write lock.
//! 3. **A quarantine relocation acts only on the file whose corruption was
//!    observed.** A healthy database substituted at the store path in the
//!    window between the read-only close and the rename is never moved, and
//!    the error never claims the damaged file was preserved.
//!
//! Interleavings are driven by [`Barrier`]s and the crate-internal
//! [`OpenSeam`] callbacks — never by sleeps. Every fixture is a real SQLite
//! store in a temporary directory, built with the crate's OWN migration SQL
//! (so it is a genuine store at that version, never a hand-copied schema).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use rusqlite::Connection;

use super::{HistoryStore, OpenSeam, StoreError, HISTORY_SCHEMA_VERSION};
use crate::store_safety::Hazard;

// No ordering GUARD (a channel timeout) is needed: every wait in this module
// is either a `Barrier` (both threads must arrive) or an inline seam callback
// (the writer acts synchronously on the opener's thread). A `Barrier` cannot
// expire, so no timeout is ever part of a passing run.

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Build a REAL store at `target` by running this crate's own migration SQL
/// for every version up to `target`, in order, each atomic with its version
/// bump — exactly the sequence `apply_migrations` performs, but driven
/// independently of the function under test (so a bug in the tested
/// function cannot manufacture a passing fixture).
fn build_store_at(db: &Path, target: u32) {
    assert!(
        (1..=HISTORY_SCHEMA_VERSION).contains(&target),
        "target version {target} is outside 1..={HISTORY_SCHEMA_VERSION}"
    );
    let mut conn = coresight_core::db::open_connection(db).expect("fixture: open");
    coresight_core::db::apply_bootstrap(&conn).expect("fixture: v1 bootstrap");
    if target >= 2 {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(super::MIGRATION_V2).unwrap();
        tx.execute("UPDATE schema_version SET version = 2", [])
            .unwrap();
        tx.commit().unwrap();
    }
    if target >= 3 {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(super::MIGRATION_V3).unwrap();
        super::retag_legacy_roots(&tx).unwrap();
        tx.execute("UPDATE schema_version SET version = 3", [])
            .unwrap();
        tx.commit().unwrap();
    }
    if target >= 4 {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(super::MIGRATION_V4).unwrap();
        tx.execute("UPDATE schema_version SET version = 4", [])
            .unwrap();
        tx.commit().unwrap();
    }
    if target >= 5 {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(super::MIGRATION_V5).unwrap();
        tx.execute("UPDATE schema_version SET version = 5", [])
            .unwrap();
        tx.commit().unwrap();
    }
    if target >= 6 {
        let tx = conn.transaction().unwrap();
        tx.execute_batch("PRAGMA defer_foreign_keys=ON;").unwrap();
        tx.execute_batch(super::MIGRATION_V6).unwrap();
        super::rekey_legacy_application_ids(&tx).unwrap();
        tx.execute("UPDATE schema_version SET version = 6", [])
            .unwrap();
        tx.commit().unwrap();
    }
    conn.close().unwrap();
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(suffix);
    path.parent().unwrap().join(name)
}

/// A store whose main file is unreadable as a database (SQLite answers with
/// its own "file is not a database" code), with no sidecars.
fn build_corrupt_store(db: &Path) {
    build_store_at(db, 2);
    let mut bytes = fs::read(db).unwrap();
    for b in bytes.iter_mut().take(32) {
        *b = 0x00;
    }
    fs::write(db, &bytes).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(with_suffix(db, suffix));
    }
}

fn schema_version_of(db: &Path) -> u32 {
    let conn = Connection::open(db).unwrap();
    conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap()
}

fn integrity_ok(db: &Path) -> bool {
    let conn = Connection::open(db).unwrap();
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    result == "ok"
}

fn schema_version_row_count(db: &Path) -> i64 {
    let conn = Connection::open(db).unwrap();
    conn.query_row("SELECT count(*) FROM schema_version", [], |r| r.get(0))
        .unwrap()
}

/// How many schema objects carry exactly this name (a duplicate `CREATE`
/// that slipped past the version gate would show up here).
fn named_objects(db: &Path, name: &str) -> i64 {
    let conn = Connection::open(db).unwrap();
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name = ?1",
        [name],
        |r| r.get(0),
    )
    .unwrap()
}

/// How many times `table` declares `column` (a duplicate `ADD COLUMN` would
/// show up here as 2).
fn column_count(db: &Path, table: &str, column: &str) -> i64 {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    names.iter().filter(|n| *n == column).count() as i64
}

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

/// A published backup is a VERIFIED, structurally intact copy that
/// identifies the schema version it protects.
fn assert_backup_is_verified(backup: &Path, protects_version: u32) {
    let conn = Connection::open(backup).expect("a published backup must open");
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        integrity, "ok",
        "a published backup must pass its own integrity check: {backup:?}"
    );
    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, protects_version,
        "the backup must identify the schema version it protects: {backup:?}"
    );
}

// ---------------------------------------------------------------------------
// Part A — concurrent opens of the same store
// ---------------------------------------------------------------------------

/// Two processes opening the same genuine v1 store at the same time: each
/// migration is applied exactly once, no opener applies a stale plan, and
/// the store ends valid at v6. A loser (if any) may only fail closed on a
/// lock or backup error — never on a duplicate schema change.
#[test]
fn two_concurrent_opens_of_a_genuine_v1_store_apply_each_migration_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v1.db");
    build_store_at(&db, 1);
    assert_eq!(schema_version_of(&db), 1);

    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let db = db.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                HistoryStore::open(&db)
            })
        })
        .collect();

    let mut succeeded = 0;
    for handle in handles {
        match handle.join().expect("thread must not panic") {
            Ok(store) => {
                succeeded += 1;
                drop(store);
            }
            Err(e) => assert!(
                matches!(e, StoreError::BackupFailed { .. } | StoreError::Sqlite(_)),
                "a losing concurrent opener may only fail closed on a lock/backup error, \
                 got {e:?}"
            ),
        }
    }
    assert!(succeeded >= 1, "at least one concurrent open must succeed");

    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(integrity_ok(&db), "the store must be intact after the race");
    assert_eq!(
        schema_version_row_count(&db),
        1,
        "the v1 bootstrap must install exactly one version row"
    );
    // No duplicate schema change: each migrated object exists exactly once.
    assert_eq!(named_objects(&db, "scan_runs"), 1);
    assert_eq!(named_objects(&db, "app_snapshot_apps"), 1);
    assert_eq!(
        column_count(&db, "observations", "file_id_hi"),
        1,
        "the v3 column must be added exactly once"
    );
    assert_eq!(
        column_count(&db, "scan_runs", "rel_status"),
        1,
        "the v4 column must be added exactly once"
    );

    // Any backup this race published is a verified copy naming the
    // pre-migration state it protects.
    for backup in backup_artifacts(&db) {
        let conn = Connection::open(&backup).unwrap();
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok", "a published backup must be intact");
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(
            (1..HISTORY_SCHEMA_VERSION).contains(&version),
            "a published backup must record the pre-migration version it protects, got {version}"
        );
    }
}

/// Two processes opening the same supported INTERMEDIATE store (v4). Same
/// invariant as the v1 case, but the pending steps are the `ALTER TABLE`
/// steps a stale plan would fail on with a duplicate column.
#[test]
fn two_concurrent_opens_of_an_intermediate_store_apply_each_migration_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v4.db");
    build_store_at(&db, 4);
    assert_eq!(schema_version_of(&db), 4);

    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let db = db.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                HistoryStore::open(&db)
            })
        })
        .collect();

    for handle in handles {
        match handle.join().expect("thread must not panic") {
            Ok(store) => drop(store),
            Err(e) => assert!(
                matches!(e, StoreError::BackupFailed { .. } | StoreError::Sqlite(_)),
                "a losing concurrent opener may only fail closed, got {e:?}"
            ),
        }
    }

    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(integrity_ok(&db));
    assert_eq!(named_objects(&db, "app_snapshot_apps"), 1);
    assert_eq!(
        column_count(&db, "app_snapshot_apps", "id_encoding"),
        1,
        "the v6 column must be added exactly once"
    );
    for backup in backup_artifacts(&db) {
        let conn = Connection::open(&backup).unwrap();
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(
            (4..HISTORY_SCHEMA_VERSION).contains(&version),
            "got {version}"
        );
    }
}

/// Two openers that have BOTH passed their mutation gate and taken their
/// verified backup, held together by a barrier at the exact moment before
/// the first mutation. Nothing can serialize them before this point, so the
/// only thing that can keep each migration single is the per-step write
/// lock + re-read. Both must succeed, and the store must carry exactly one
/// application of every step.
#[test]
fn two_openers_that_reach_the_mutation_gate_together_apply_one_plan() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("gate.db");
    build_store_at(&db, 3);

    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let db = db.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                HistoryStore::open_impl_seamed(&db, Hazard::None, &mut |_| {}, &mut |stage| {
                    if stage == OpenSeam::MutationBeforeFirstWrite {
                        barrier.wait();
                    }
                })
            })
        })
        .collect();

    for handle in handles {
        handle
            .join()
            .expect("thread must not panic")
            .expect("both openers must succeed once their plan is serialized");
    }

    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(integrity_ok(&db));
    assert_eq!(named_objects(&db, "scan_runs"), 1);
    assert_eq!(
        column_count(&db, "scan_runs", "rel_status"),
        1,
        "the v4 column must be added exactly once"
    );
    assert_eq!(column_count(&db, "app_snapshot_apps", "id_encoding"), 1);

    // Both openers observed v3, so both took a v3 backup; each one must be a
    // verified copy of that state.
    let backups = backup_artifacts(&db);
    assert!(
        !backups.is_empty(),
        "a migrating concurrent open must publish its verified backup"
    );
    for backup in backups {
        assert_backup_is_verified(&backup, 3);
    }
}

/// A writer commits a schema upgrade in the middle of this opener's
/// read-only... no: in the middle of its MUTATION GATE's inspection, after
/// the integrity rows are read and before the recorded version is. The
/// opener must decide from the version that is CURRENT at decision time —
/// it observes v6 and does not fight it.
#[test]
fn a_version_change_between_integrity_and_recognition_is_observed_not_fought() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mid-inspect.db");
    build_store_at(&db, 3);

    let mut writer_migrated = false;
    let store = HistoryStore::open_impl_seamed(&db, Hazard::None, &mut |_| {}, &mut |stage| {
        if stage == OpenSeam::MutationAfterIntegrity {
            // Another authorized opener completes the migration while
            // THIS opener is still gathering its facts. The version this
            // opener is about to read is the writer's committed one.
            let writer =
                HistoryStore::open(&db).expect("the concurrent opener must migrate the store");
            drop(writer);
            writer_migrated = true;
        }
    })
    .expect("the opener must observe the committed version, not fight it");
    drop(store);
    assert!(writer_migrated, "the seam must have fired");

    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(integrity_ok(&db));
    assert_eq!(named_objects(&db, "app_snapshot_apps"), 1);
    assert_eq!(column_count(&db, "scan_runs", "rel_status"), 1);
    // The opener decided `Current` from the writer's committed version, so
    // IT took no backup; the writer's own v3 backup is the one published.
    for backup in backup_artifacts(&db) {
        assert_backup_is_verified(&backup, 3);
    }
}

/// A writer commits a schema upgrade between this opener's recognition
/// verdict and its first mutation. The opener must NOT apply the stale plan
/// it decided from: the per-step write lock re-reads the version and skips
/// every step the writer already applied.
#[test]
fn a_version_change_before_the_first_mutation_never_applies_a_stale_plan() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("before-write.db");
    build_store_at(&db, 3);

    let mut writer_migrated = false;
    let store = HistoryStore::open_impl_seamed(&db, Hazard::None, &mut |_| {}, &mut |stage| {
        if stage == OpenSeam::MutationBeforeFirstWrite {
            // The opener has already decided "migrate from v3" and taken
            // its v3 backup. The writer finishes the whole migration
            // before the opener runs a single step.
            let writer =
                HistoryStore::open(&db).expect("the concurrent opener must migrate the store");
            drop(writer);
            writer_migrated = true;
        }
    })
    .expect("the opener must not apply a stale migration plan");
    drop(store);
    assert!(writer_migrated, "the seam must have fired");

    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(integrity_ok(&db));
    assert_eq!(named_objects(&db, "app_snapshot_apps"), 1);
    assert_eq!(
        column_count(&db, "scan_runs", "rel_status"),
        1,
        "the stale plan must never add a second rel_status column"
    );
    assert_eq!(column_count(&db, "app_snapshot_apps", "id_encoding"), 1);

    // This opener's own verified backup names the pre-migration state it
    // observed (v3), even though the other process completed the migration.
    let backups = backup_artifacts(&db);
    assert!(!backups.is_empty());
    for backup in backups {
        assert_backup_is_verified(&backup, 3);
    }
}

/// A concurrent holder of the store's write lock outlasting this opener's
/// lock wait makes the open fail closed with SQLite's own busy error — never
/// a silent success — and leaves the store VALID and recoverable: a later
/// open migrates it normally.
#[test]
fn a_concurrent_open_that_cannot_get_the_write_lock_fails_closed_and_leaves_the_store_valid() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lock.db");
    build_store_at(&db, 3);

    let mut held: Option<Connection> = None;
    let err = HistoryStore::open_impl_seamed(
        &db,
        Hazard::MutationBusyTimeout(Duration::from_millis(50)),
        &mut |_| {},
        &mut |stage| {
            if stage == OpenSeam::MutationBeforeFirstWrite {
                // Hold the store's write lock across the opener's migration
                // attempt, so its `BEGIN IMMEDIATE` cannot get in.
                let holder = Connection::open(&db).unwrap();
                holder.busy_timeout(Duration::from_secs(30)).unwrap();
                holder.execute_batch("BEGIN IMMEDIATE").unwrap();
                held = Some(holder);
            }
        },
    )
    .expect_err("a lock the opener cannot acquire must fail, not silently succeed");
    assert!(
        matches!(err, StoreError::Sqlite(_)),
        "the failure must be SQLite's own lock error, got {err:?}"
    );
    drop(held.take());

    // Nothing was migrated and nothing was damaged: the store is valid.
    assert_eq!(schema_version_of(&db), 3);
    assert!(integrity_ok(&db));
    // ...and a later open, once the lock is free, completes normally.
    let store = HistoryStore::open(&db).expect("the store must still be openable");
    drop(store);
    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    assert!(integrity_ok(&db));
}

/// A migrating open publishes exactly one verified backup, and that backup
/// identifies the schema version it is there to protect. This is the
/// deterministic counterpart to the race assertions above.
#[test]
fn a_published_backup_records_the_version_it_protects() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("backup.db");
    build_store_at(&db, 3);

    let store = HistoryStore::open(&db).expect("the v3 store must migrate");
    drop(store);

    assert_eq!(schema_version_of(&db), HISTORY_SCHEMA_VERSION);
    let backups = backup_artifacts(&db);
    assert_eq!(
        backups.len(),
        1,
        "an uncontended migrating open publishes exactly one backup: {backups:?}"
    );
    assert_backup_is_verified(&backups[0], 3);
}

// ---------------------------------------------------------------------------
// Part B — quarantine is bound to the observed file
// ---------------------------------------------------------------------------

/// A genuinely corrupt store is quarantined with its identity proven, which
/// requires the read-only handle to be CLOSED before the rename: on Windows
/// an open database file cannot be renamed, so a relocation attempted while
/// the handle were alive would fail here. This is the close-before-rename
/// evidence that spans all three CI platforms.
#[test]
fn a_corrupt_store_is_quarantined_only_after_every_handle_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("closed.db");
    build_corrupt_store(&db);

    let err = HistoryStore::open(&db).expect_err("a corrupt store must be refused");
    assert!(
        matches!(err, StoreError::Corrupt { .. }),
        "expected Corrupt, got {err:?}"
    );

    let dirs = quarantine_dirs(&db);
    assert_eq!(dirs.len(), 1, "the store must be quarantined: {dirs:?}");
    assert!(
        dirs[0].join("closed.db").exists(),
        "the damaged file must be preserved inside the quarantine directory"
    );
    assert!(
        err.to_string()
            .contains("was preserved (not deleted, not repaired)"),
        "a real quarantine is reported as such: {err}"
    );
}

/// The load-bearing 6.5.3 case: a HEALTHY database substituted at the store
/// path in the window between the read-only close and the relocation. The
/// substitute must NOT be moved, and the error must not claim the observed
/// damaged file was preserved.
#[test]
fn a_substituted_file_is_never_quarantined_as_another_files_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("substitute.db");
    let damaged = dir.path().join("damaged-original.db");
    build_corrupt_store(&db);

    // A healthy store, elsewhere, to drop in at the unattended path.
    let healthy_dir = dir.path().join("healthy");
    fs::create_dir(&healthy_dir).unwrap();
    let healthy = healthy_dir.join("healthy.db");
    drop(HistoryStore::open(&healthy).expect("fixture: build a healthy store"));
    let healthy_bytes = fs::read(&healthy).unwrap();

    let mut substituted = false;
    let err = HistoryStore::open_impl_seamed(&db, Hazard::None, &mut |_| {}, &mut |stage| {
        if stage == OpenSeam::AfterReadOnlyClose {
            // The read-only handle is closed and no rename has run: the
            // path is unattended. Replace the damaged file with a
            // HEALTHY one, exactly as an ordinary writer (a restore, a
            // backup tool, a second process) could.
            fs::rename(&db, &damaged).unwrap();
            fs::write(&db, &healthy_bytes).unwrap();
            substituted = true;
        }
    })
    .expect_err("corruption WAS observed, so the open must still report it");
    assert!(substituted, "the seam must have fired");

    let message = err.to_string();
    assert!(
        message.contains("database integrity check failed on open:"),
        "the corruption evidence must be preserved: {message}"
    );
    assert!(
        message.contains("NOT quarantined") && message.contains("NOTHING was moved"),
        "the refusal must say nothing was moved: {message}"
    );
    assert!(
        !message.contains("was preserved (not deleted, not repaired)"),
        "the error must NOT claim the observed damaged file was preserved: {message}"
    );

    // The healthy substitute is untouched and still a valid store, and no
    // quarantine directory was ever created for it.
    assert_eq!(
        fs::read(&db).unwrap(),
        healthy_bytes,
        "the substituted file must be byte-identical"
    );
    assert!(integrity_ok(&db), "the substitute must still be healthy");
    assert!(
        quarantine_dirs(&db).is_empty(),
        "a file that never failed the integrity check must not be quarantined"
    );
    // The genuinely damaged file is exactly where the test put it.
    assert!(damaged.exists());
}
