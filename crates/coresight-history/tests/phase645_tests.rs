//! Phase 6.4.5 — timestamp error semantics & legacy history compatibility.
//!
//! Workstream A: an unrepresentable CALLER timestamp is typed caller input
//! (`StoreError::InvalidTimestamp`), naming the owning table/column and the
//! run, and is rejected BEFORE the write transaction opens.
//!
//! Workstream B: persisted timestamp decoding follows one documented
//! compatibility policy — non-negative values round-trip exactly, and a
//! negative value is a legitimate legacy bit pattern (the pre-6.4.2
//! writer's unchecked `u128 as i64` wrap) reconstructed faithfully rather
//! than clamped to a plausible-looking epoch.
//!
//! Workstream C: a genuine pre-v4 (v2) database carrying
//! `relationship_obs` / `relationship_members` rows migrates through the
//! real chain to v6; the rows are retained, the v4 metadata columns stay
//! NULL, and the reader neither manufactures a relationship report nor
//! loses the historical facts (they stay queryable via
//! `relationship_history`).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use coresight_history::{
    store::HISTORY_SCHEMA_VERSION, ConfigFingerprint, HistoryStore, ObservedEntry, ObservedKind,
    QueryLimits, RunCounts, RunId, RunRecord, RunStatus, Snapshot, StoreError,
};
use coresight_identity::{DuplicateStatus, StorageAccounting, Undetermined};
use rusqlite::{params, Connection};

// ---------------------------------------------------------------------------
// shared fixtures
// ---------------------------------------------------------------------------

fn run_record(id: &str) -> RunRecord {
    RunRecord {
        run_id: RunId(id.to_string()),
        started_at: UNIX_EPOCH + Duration::from_secs(1_000_000),
        completed_at: Some(UNIX_EPOCH + Duration::from_secs(1_000_001)),
        roots: vec![PathBuf::from("/scope-a")],
        platform: "test".to_string(),
        config: ConfigFingerprint::current(),
        status: RunStatus::Completed,
        counts: RunCounts::default(),
    }
}

/// A time past the store's signed 64-bit nanosecond ceiling (~2262 CE).
fn far_future() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(32503680000)
}

/// A far-future time the pre-6.4.2 writer stored lossily but recoverably:
/// above the `i64` nanosecond ceiling (so it wrapped NEGATIVE), yet below
/// 2^64 nanoseconds (~year 2554) — so the stored 64 bits are the whole
/// value and the faithful reconstruction is the EXACT original time.
fn recoverable_far_future() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(13_000_000_000)
}

fn nanos_after_epoch(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH)
        .expect("decoded timestamps in these tests are all post-epoch")
        .as_nanos()
}

/// The exact expression every pre-6.4.2 writer used to store a timestamp:
/// `duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)`.
fn legacy_stored_nanos(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn entry(size: Option<u64>, modified: Option<SystemTime>) -> ObservedEntry {
    ObservedEntry {
        path: PathBuf::from("/scope-a/tamper.bin"),
        kind: ObservedKind::File,
        size,
        object: None,
        modified,
        classification: None,
        content_sha256: None,
        observation_error: None,
    }
}

fn base_relationship() -> coresight_identity::Relationship {
    coresight_identity::Relationship {
        id: "content-xyz".to_string(),
        kind: coresight_identity::RelationshipKind::ContentDuplicate,
        size: 10,
        member_count: 2,
        members: Vec::new(),
        distinct_objects: Some(2),
        evidence: Vec::new(),
        content: Some(coresight_identity::ContentRef {
            sha256_hex: "deadbeef".to_string(),
            algorithm: "sha256".to_string(),
        }),
        object: None,
        alias_sets: Vec::new(),
        logical_duplicate_bytes: 10,
        recoverable_bytes: Some(10),
        accounting: StorageAccounting::Exact,
        detail_truncated: false,
    }
}

fn report_with(
    relationships: Vec<coresight_identity::Relationship>,
) -> coresight_identity::RelationshipReport {
    coresight_identity::RelationshipReport {
        status: DuplicateStatus::CompletedWithLimits,
        relationships,
        relationships_truncated: 0,
        undetermined: Undetermined {
            failed: 0,
            not_examined: 0,
            failed_by_reason: Vec::new(),
            detail: Vec::new(),
            detail_truncated: 0,
        },
        stats: coresight_identity::RelationshipStats::default(),
        started_at: UNIX_EPOCH,
        finished_at: UNIX_EPOCH,
    }
}

/// Dump the committed rows of a database, for direct state comparison.
fn db_dump(db: &Path) -> String {
    let conn = Connection::open(db).unwrap();
    let mut out = String::new();
    for sql in [
        "SELECT run_id, started_at, completed_at, status, entries_examined, files, dirs, \
         links, other_entries, bytes, observation_errors, candidates_untracked, \
         hash_failures, rel_status, rel_truncated FROM scan_runs ORDER BY run_id",
        "SELECT run_id, path, kind, size, modified FROM observations ORDER BY run_id, path",
        "SELECT run_id, rel_id, kind, size, member_count, recoverable, accounting \
         FROM relationship_obs ORDER BY run_id, rel_id",
        "SELECT run_id, rel_id, path FROM relationship_members ORDER BY run_id, rel_id, path",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let cols = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                let mut line = String::new();
                for i in 0..cols {
                    if i > 0 {
                        line.push('|');
                    }
                    let v: Option<String> = r.get(i).ok();
                    line.push_str(&v.unwrap_or_else(|| "<NULL>".to_string()));
                }
                Ok(line)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        out.push_str(sql);
        out.push_str(" =>\n");
        for row in rows {
            out.push_str("  ");
            out.push_str(&row);
            out.push('\n');
        }
    }
    conn.close().unwrap();
    out
}

/// Directly tamper with a stored column (the only way legacy bit patterns
/// and corrupted values can be produced on a current-schema database).
fn tamper<P: rusqlite::Params>(db: &Path, sql: &str, values: P) {
    let conn = Connection::open(db).unwrap();
    conn.execute(sql, values).unwrap();
    conn.close().unwrap();
}

// ---------------------------------------------------------------------------
// Workstream A — typed caller-input errors for timestamps
// ---------------------------------------------------------------------------

/// An unrepresentable CALLER `started_at` is typed as caller input
/// (`InvalidTimestamp`), names `scan_runs.started_at` and the run, and is
/// rejected BEFORE any row exists — `begin_run` never opens its
/// transaction for a value it cannot store.
#[test]
fn unrepresentable_started_at_is_caller_input_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("started.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let mut record = run_record("a-started-run");
    record.started_at = far_future();

    let err = store
        .begin_run(&record)
        .expect_err("an unrepresentable started_at must be rejected");
    match err {
        StoreError::InvalidTimestamp {
            table,
            column,
            run_id,
            nanos,
        } => {
            assert_eq!(table, "scan_runs", "the field's owning table");
            assert_eq!(column, "started_at", "the actual column");
            assert_eq!(run_id.as_deref(), Some("a-started-run"), "run context");
            assert_eq!(nanos, nanos_after_epoch(far_future()), "timestamp context");
        }
        other => panic!("expected InvalidTimestamp for caller input, got {other:?}"),
    }

    // Before the transaction: no row at all, nothing to roll back.
    assert!(store.get_run(&record.run_id).unwrap().is_none());
    assert!(store.list_runs(&QueryLimits::default()).unwrap().is_empty());
}

/// An unrepresentable CALLER `completed_at` is typed as caller input,
/// names `scan_runs.completed_at` and the run, and the rejection happens
/// BEFORE the write transaction: a previously committed run and the
/// in-flight run row both survive byte-for-byte.
#[test]
fn unrepresentable_completed_at_is_caller_input_before_the_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("completed.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // A previously committed run whose state must be preserved.
    let first = run_record("a-completed-first");
    store.begin_run(&first).unwrap();
    store
        .commit_run(
            &first,
            &Snapshot {
                run_id: first.run_id.clone(),
                entries: Vec::new(),
            },
            None,
        )
        .unwrap();

    // An in-flight run whose row must survive the failed commit untouched.
    let second = run_record("a-completed-second");
    store.begin_run(&second).unwrap();
    let before = db_dump(&db);

    let mut terminal = second.clone();
    terminal.completed_at = Some(far_future());
    let err = store
        .commit_run(
            &terminal,
            &Snapshot {
                run_id: second.run_id.clone(),
                entries: Vec::new(),
            },
            None,
        )
        .expect_err("an unrepresentable completed_at must be rejected");
    match err {
        StoreError::InvalidTimestamp {
            table,
            column,
            run_id,
            nanos,
        } => {
            assert_eq!(table, "scan_runs", "the field's owning table");
            assert_eq!(column, "completed_at", "the actual column");
            assert_eq!(run_id.as_deref(), Some("a-completed-second"), "run context");
            assert_eq!(nanos, nanos_after_epoch(far_future()), "timestamp context");
        }
        other => panic!("expected InvalidTimestamp for caller input, got {other:?}"),
    }

    // Not one row changed: the committed run and the RUNNING row are intact.
    assert_eq!(
        db_dump(&db),
        before,
        "a rejected commit must not alter any persisted row"
    );
    assert_eq!(
        store.get_run(&second.run_id).unwrap().unwrap().status,
        RunStatus::Running,
        "the in-flight run must remain RUNNING"
    );
    assert_eq!(
        store.get_run(&first.run_id).unwrap().unwrap().status,
        RunStatus::Completed,
        "the previously committed run must be preserved"
    );
}

/// An unrepresentable CALLER observation `modified` is typed as caller
/// input, names `observations.modified` (not `scan_runs`) and the run, and
/// the rejection happens BEFORE the write transaction.
#[test]
fn unrepresentable_observation_modified_is_caller_input_before_the_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("modified.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // A previously committed run with a real observation to preserve.
    let first = run_record("a-modified-first");
    store.begin_run(&first).unwrap();
    let good = Snapshot {
        run_id: first.run_id.clone(),
        entries: vec![ObservedEntry {
            path: PathBuf::from("/scope-a/real.bin"),
            kind: ObservedKind::File,
            size: Some(10),
            object: None,
            modified: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            classification: None,
            content_sha256: None,
            observation_error: None,
        }],
    };
    store.commit_run(&first, &good, None).unwrap();

    // An in-flight run whose row must survive the failed commit untouched.
    let second = run_record("a-modified-second");
    store.begin_run(&second).unwrap();
    let before = db_dump(&db);

    let bad = Snapshot {
        run_id: second.run_id.clone(),
        entries: vec![entry(Some(11), Some(far_future()))],
    };
    let err = store
        .commit_run(&second, &bad, None)
        .expect_err("an unrepresentable observation timestamp must be rejected");
    match err {
        StoreError::InvalidTimestamp {
            table,
            column,
            run_id,
            nanos,
        } => {
            assert_eq!(table, "observations", "the field's owning table");
            assert_eq!(column, "modified", "the actual column");
            assert_eq!(run_id.as_deref(), Some("a-modified-second"), "run context");
            assert_eq!(nanos, nanos_after_epoch(far_future()), "timestamp context");
        }
        other => panic!("expected InvalidTimestamp for caller input, got {other:?}"),
    }

    // Not one row changed.
    assert_eq!(
        db_dump(&db),
        before,
        "a rejected commit must not alter any persisted row"
    );
    assert_eq!(
        store.get_run(&second.run_id).unwrap().unwrap().status,
        RunStatus::Running,
        "the in-flight run must remain RUNNING"
    );
    let reloaded = store.load_run_snapshot(&first.run_id).unwrap().unwrap();
    assert_eq!(
        reloaded.snapshot.entries.len(),
        1,
        "prior observations intact"
    );
}

// ---------------------------------------------------------------------------
// Workstream B — persisted timestamp decoding
// ---------------------------------------------------------------------------

/// The compatibility policy for every stored timestamp value, proven by
/// direct SQL tampering with each column, across every read path:
///
/// - `0` is the documented pre-epoch convention (every writer maps a
///   pre-epoch time to `0`) and reads as the epoch.
/// - a positive value is the exact time for every writer and round-trips.
/// - a NEGATIVE value is a legitimate legacy bit pattern — the
///   pre-6.4.2 writer's unchecked `u128 as i64` wrap of a far-future
///   time — and is reconstructed as the stored bits read unsigned,
///   exactly recovering the original for any time below ~year 2554.
#[test]
fn stored_timestamps_follow_the_documented_compatibility_policy() {
    let dir = tempfile::tempdir().unwrap();
    // Legacy-wrapped year-3000 bits, derived exactly as the old writer did.
    let legacy_wrap = legacy_stored_nanos(far_future());
    let cases: [(i64, &str); 6] = [
        (-1, "one below zero (legacy-wrapped / tampered)"),
        (i64::MIN, "signed minimum (legacy-wrapped / tampered)"),
        (legacy_wrap, "legacy-wrapped year-3000 time"),
        (0, "pre-epoch convention"),
        (1_700_000_000_000_000_000, "ordinary time"),
        (i64::MAX, "signed maximum (~2262 CE)"),
    ];
    for (i, (value, label)) in cases.iter().enumerate() {
        let db = dir.path().join(format!("ts-matrix-{i}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record(&format!("ts-matrix-run-{i}"));
        store.begin_run(&record).unwrap();
        let snap = Snapshot {
            run_id: record.run_id.clone(),
            entries: vec![entry(Some(10), Some(UNIX_EPOCH + Duration::from_secs(1)))],
        };
        store
            .commit_run(
                &record,
                &snap,
                Some(&report_with(vec![base_relationship()])),
            )
            .unwrap();

        tamper(
            &db,
            "UPDATE scan_runs SET started_at = ?1, completed_at = ?1 WHERE run_id = ?2",
            params![*value, record.run_id.0],
        );
        tamper(
            &db,
            "UPDATE observations SET modified = ?1 WHERE run_id = ?2",
            params![*value, record.run_id.0],
        );

        // The policy, stated independently of the implementation: the read
        // is the epoch plus the stored bits read as an UNSIGNED nanosecond
        // count. (A Windows SystemTime is 100-nanosecond FILETIME-based, so
        // a sub-100ns remainder truncates on every conversion — the same
        // granularity every timestamp read on that platform has always
        // had, positive values included. Constructing the expected value
        // through the same public arithmetic keeps the assertion exact on
        // every platform while still failing under the old clamp, which
        // would read every negative as the epoch.)
        let expected = UNIX_EPOCH + Duration::from_nanos(*value as u64);

        // Run read paths: get_run and list_runs.
        let run = store.get_run(&record.run_id).unwrap().unwrap();
        assert_eq!(
            run.started_at, expected,
            "{label}: started_at must decode as the stored bits read unsigned"
        );
        assert_eq!(
            run.completed_at,
            Some(expected),
            "{label}: completed_at must decode as the stored bits read unsigned"
        );
        let listed = store.list_runs(&QueryLimits::default()).unwrap();
        assert_eq!(
            listed[0].started_at, expected,
            "{label}: list_runs must decode the same way"
        );

        // Observation read path: load_run_snapshot.
        let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
        assert_eq!(
            loaded.snapshot.entries[0].modified,
            Some(expected),
            "{label}: observations.modified must decode as the stored bits read unsigned"
        );

        // History read paths.
        let points = store
            .history_for_path(Path::new("/scope-a/tamper.bin"), &QueryLimits::default())
            .unwrap();
        assert_eq!(
            points[0].started_at,
            Some(expected),
            "{label}: history_for_path must decode the run timestamp the same way"
        );
        let rel_points = store
            .relationship_history("content-xyz", &QueryLimits::default())
            .unwrap();
        assert_eq!(
            rel_points[0].started_at,
            Some(expected),
            "{label}: relationship_history must decode the run timestamp the same way"
        );
    }
}

/// A negative value is NOT clamped to the epoch (the previous reader's
/// silent conversion into the plausible date 1970-01-01) and NOT rejected
/// as corruption: a legacy-wrapped value and a tampered value are
/// indistinguishable in the stored format, so the store applies the one
/// reading that preserves every historical value and documents the
/// ambiguity rather than claiming to detect it.
#[test]
fn negative_stored_timestamps_are_reconstructed_not_clamped_or_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("negative.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("negative-run");
    store.begin_run(&record).unwrap();
    store
        .commit_run(
            &record,
            &Snapshot {
                run_id: record.run_id.clone(),
                entries: Vec::new(),
            },
            None,
        )
        .unwrap();

    tamper(
        &db,
        "UPDATE scan_runs SET started_at = -1 WHERE run_id = ?1",
        params![record.run_id.0],
    );
    let run = store.get_run(&record.run_id).unwrap().unwrap();
    // Not the epoch: the clamp is gone.
    assert_ne!(
        run.started_at, UNIX_EPOCH,
        "a negative must not read as the epoch"
    );
    // The faithful reading: the stored bits as an unsigned nanosecond count.
    assert_eq!(run.started_at, UNIX_EPOCH + Duration::from_nanos(u64::MAX));
    let decoded = nanos_after_epoch(run.started_at);
    assert!(
        decoded >= u64::MAX as u128 - 99 && decoded <= u64::MAX as u128,
        "the reconstruction is exact to within the platform's SystemTime \
         granularity (Windows truncates sub-100ns remainders)"
    );
    // And it is not rejected: the format cannot distinguish this value
    // from a legacy wrapped one, so no corruption is claimed.
    assert!(store.load_run_snapshot(&record.run_id).is_ok());
}

/// A stored value that NO supported writer could ever produce (a
/// non-integer in a timestamp column) fails CLOSED: the read errors
/// instead of returning a plausible date. Such a value cannot arise from
/// any writer generation, and SQLite's type system refuses the integer
/// read rather than silently converting it — so no date is invented and
/// no corruption is masked.
#[test]
fn non_integer_stored_timestamps_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("non-integer.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("non-integer-run");
    store.begin_run(&record).unwrap();
    store
        .commit_run(
            &record,
            &Snapshot {
                run_id: record.run_id.clone(),
                entries: vec![entry(Some(10), Some(UNIX_EPOCH + Duration::from_secs(1)))],
            },
            None,
        )
        .unwrap();

    // TEXT in a run timestamp column.
    tamper(
        &db,
        "UPDATE scan_runs SET started_at = 'not-a-time' WHERE run_id = ?1",
        params![record.run_id.0],
    );
    assert!(
        store.get_run(&record.run_id).is_err(),
        "a TEXT timestamp must fail closed, never return a plausible date"
    );
    assert!(
        store.list_runs(&QueryLimits::default()).is_err(),
        "every run read path must fail closed on a TEXT timestamp"
    );

    // REAL in run timestamp columns.
    tamper(
        &db,
        "UPDATE scan_runs SET started_at = 1.5, completed_at = 1.5 WHERE run_id = ?1",
        params![record.run_id.0],
    );
    assert!(
        store.get_run(&record.run_id).is_err(),
        "a REAL timestamp must fail closed, never return a plausible date"
    );

    // TEXT in an observation timestamp column.
    tamper(
        &db,
        "UPDATE observations SET modified = 'not-a-time' WHERE run_id = ?1",
        params![record.run_id.0],
    );
    assert!(
        store.load_run_snapshot(&record.run_id).is_err(),
        "a TEXT observation timestamp must fail closed"
    );
}

/// A legacy-wrapped timestamp (what a pre-6.4.2 writer stored for a
/// far-future time) survives the REAL migration chain v2 → v6 and reads
/// back as the exact original time — the legacy representation is
/// preserved, not rejected and not clamped. The chosen time is above the
/// `i64` nanosecond ceiling (so it wrapped negative) yet below 2^64
/// nanoseconds (~year 2554), so the stored bits carry the whole value.
#[test]
fn legacy_wrapped_timestamps_survive_the_real_migration_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-wrap.db");

    // A genuine v2 database whose timestamps were written by the legacy
    // (unchecked) writer — a far-future run start, terminal time and
    // observation mtime, each wrapped negative exactly as that writer did.
    let original = recoverable_far_future();
    let stored = legacy_stored_nanos(original);
    assert!(stored < 0, "the legacy writer wrapped this time negative");
    build_legacy_relational_v2(&db, stored, Some(stored), Some(stored));

    let store = HistoryStore::open(&db).unwrap();
    assert_eq!(
        current_version(&db),
        HISTORY_SCHEMA_VERSION,
        "the legacy store must migrate all the way to v6"
    );

    let run = store
        .get_run(&RunId("legacy-rel-run".to_string()))
        .unwrap()
        .expect("the legacy run must survive migration");
    assert_eq!(
        run.started_at, original,
        "the legacy wrapped start must reconstruct to the exact original time"
    );
    assert_eq!(run.completed_at, Some(original), "completed_at likewise");

    let loaded = store
        .load_run_snapshot(&RunId("legacy-rel-run".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.snapshot.entries[0].modified,
        Some(original),
        "the legacy wrapped mtime must reconstruct to the exact original time"
    );

    let points = store
        .relationship_history("content-deadbeef", &QueryLimits::default())
        .unwrap();
    assert_eq!(points[0].started_at, Some(original));
}

// ---------------------------------------------------------------------------
// Workstream C — legacy relationship rows and NULL metadata
// ---------------------------------------------------------------------------

/// A genuine pre-v4 database carrying `relationship_obs` and
/// `relationship_members` rows migrates through the real chain to v6:
/// the rows are retained verbatim, both v4 metadata columns remain NULL
/// (nothing is fabricated), `load_run_snapshot` attaches no relationship
/// report, and the retained facts stay queryable through
/// `relationship_history`.
#[test]
fn legacy_relationship_rows_survive_the_migration_without_fabricated_completeness() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-rel.db");
    let started = 1_000_000_000i64; // an ordinary v2-era timestamp
    build_legacy_relational_v2(&db, started, Some(started + 500), Some(started));

    // Opened through the repository's actual migration chain (v2 → v6).
    let store = HistoryStore::open(&db).unwrap();
    assert_eq!(
        current_version(&db),
        HISTORY_SCHEMA_VERSION,
        "the legacy store must migrate all the way to v6"
    );

    // The v4 metadata columns exist and remain NULL for the legacy run:
    // the old store never recorded a relationship report.
    let (status, truncated): (Option<String>, Option<i64>) = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT rel_status, rel_truncated FROM scan_runs WHERE run_id = 'legacy-rel-run'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, None, "no relationship status was ever recorded");
    assert_eq!(truncated, None, "no truncation count was ever recorded");

    // The rows are retained — nothing dropped by the migration.
    let counts = row_counts(&db);
    assert_eq!(counts.observations, 2, "both legacy observations retained");
    assert_eq!(
        counts.relationship_obs, 1,
        "the legacy relationship row retained"
    );
    assert_eq!(
        counts.relationship_members, 2,
        "both legacy members retained"
    );

    // load_run_snapshot must NOT manufacture a report or claim
    // completeness the run never recorded.
    let loaded = store
        .load_run_snapshot(&RunId("legacy-rel-run".to_string()))
        .unwrap()
        .expect("the legacy run must survive migration");
    assert!(
        loaded.relationships.is_none(),
        "NULL metadata must never manufacture a relationship report"
    );
    // The run's observations migrated through the v3 path-tagging and
    // still decode to the original paths.
    let mut paths: Vec<String> = loaded
        .snapshot
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["/scope-a/a.bin", "/scope-a/b.bin"]);

    // The retained relationship facts remain queryable through the public
    // history query — (NULL, NULL) does not mean "no rows ever existed".
    let points = store
        .relationship_history("content-deadbeef", &QueryLimits::default())
        .unwrap();
    assert_eq!(
        points.len(),
        1,
        "the legacy relationship row stays queryable"
    );
    let point = &points[0];
    assert_eq!(point.run_id, RunId("legacy-rel-run".to_string()));
    assert_eq!(
        point.started_at,
        Some(UNIX_EPOCH + Duration::from_nanos(started as u64)),
        "the run timestamp migrates intact"
    );
    assert_eq!(point.kind, "CONTENT_DUPLICATE");
    assert_eq!(point.size, 100);
    assert_eq!(point.member_count, 2);
    assert_eq!(point.recoverable, Some(100));
    assert_eq!(point.accounting, "EXACT");
    assert_eq!(
        point.stored_member_count, 2,
        "both legacy member rows are stored for the run"
    );
}

/// The Phase 6.4.4 nullability contract remains intact on a store that
/// went through the REAL v2 → v6 migration with legacy relationship rows
/// present: both mixed-nullability combinations (which no writer can
/// produce) are still rejected as inconsistent metadata, while the
/// legacy (NULL, NULL) case above stays accepted.
#[test]
fn metadata_nullability_checks_remain_intact_after_the_migration() {
    let dir = tempfile::tempdir().unwrap();
    let started = 1_000_000_000i64;

    // status present, counter absent — impossible for any writer.
    let db = dir.path().join("legacy-mixed-a.db");
    let store = open_migrated_legacy_store(&db, started);
    tamper(
        &db,
        "UPDATE scan_runs SET rel_status = 'COMPLETED', rel_truncated = NULL
         WHERE run_id = 'legacy-rel-run'",
        params![],
    );
    let err = store
        .load_run_snapshot(&RunId("legacy-rel-run".to_string()))
        .expect_err("an inconsistent pair must be rejected");
    match &err {
        StoreError::Corrupt {
            table,
            column,
            run_id,
            ..
        } => {
            assert_eq!(*table, "scan_runs");
            assert_eq!(*column, "rel_truncated", "the missing field is reported");
            assert_eq!(run_id.as_deref(), Some("legacy-rel-run"));
        }
        other => panic!("expected typed corruption, got {other:?}"),
    }

    // status absent, counter present — likewise impossible.
    let db2 = dir.path().join("legacy-mixed-b.db");
    let store2 = open_migrated_legacy_store(&db2, started);
    tamper(
        &db2,
        "UPDATE scan_runs SET rel_status = NULL, rel_truncated = 5
         WHERE run_id = 'legacy-rel-run'",
        params![],
    );
    let err = store2
        .load_run_snapshot(&RunId("legacy-rel-run".to_string()))
        .expect_err("an inconsistent pair must be rejected");
    match &err {
        StoreError::Corrupt {
            table,
            column,
            run_id,
            ..
        } => {
            assert_eq!(*table, "scan_runs");
            assert_eq!(*column, "rel_status", "the missing field is reported");
            assert_eq!(run_id.as_deref(), Some("legacy-rel-run"));
        }
        other => panic!("expected typed corruption, got {other:?}"),
    }
}

/// The Phase 6.4.3 faithful-round-trip policy for a legacy NEGATIVE
/// `rel_truncated` still holds on a store that went through the real
/// v2 → v6 migration: the wrapped bit pattern reconstructs exactly.
#[test]
fn legacy_negative_counter_round_trips_after_the_migration() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-neg-counter.db");
    let started = 1_000_000_000i64;
    let store = open_migrated_legacy_store(&db, started);
    let huge = 10_000_000_000_000_000_000u64;
    tamper(
        &db,
        "UPDATE scan_runs SET rel_status = 'COMPLETED_WITH_LIMITS', rel_truncated = ?1
         WHERE run_id = 'legacy-rel-run'",
        params![huge as i64],
    );
    let loaded = store
        .load_run_snapshot(&RunId("legacy-rel-run".to_string()))
        .unwrap()
        .unwrap();
    let report = loaded
        .relationships
        .expect("a present pair attaches a report");
    assert_eq!(
        report.relationships_truncated, huge,
        "the legacy wrapped bit pattern must be reconstructed exactly"
    );
}

// ---------------------------------------------------------------------------
// legacy v2 fixtures (historical — do not "fix" to match live migrations)
// ---------------------------------------------------------------------------

/// The Phase 5 v2 schema, verbatim — what a genuine pre-v4 database
/// looked like on disk before any of the v3–v6 migrations.
const V2_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS scan_runs (
    run_id        TEXT PRIMARY KEY,
    started_at    INTEGER NOT NULL,
    completed_at  INTEGER,
    roots         TEXT NOT NULL,
    platform      TEXT NOT NULL,
    config        TEXT NOT NULL,
    status        TEXT NOT NULL,
    entries_examined INTEGER NOT NULL DEFAULT 0,
    files         INTEGER NOT NULL DEFAULT 0,
    dirs          INTEGER NOT NULL DEFAULT 0,
    links         INTEGER NOT NULL DEFAULT 0,
    other_entries INTEGER NOT NULL DEFAULT 0,
    bytes         INTEGER NOT NULL DEFAULT 0,
    observation_errors INTEGER NOT NULL DEFAULT 0,
    candidates_untracked INTEGER NOT NULL DEFAULT 0,
    hash_failures INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS observations (
    run_id    TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    path      TEXT NOT NULL,
    kind      TEXT NOT NULL,
    size      INTEGER,
    device    INTEGER,
    inode     INTEGER,
    modified  INTEGER,
    category  TEXT,
    subcategory TEXT,
    content_sha256 TEXT,
    obs_error TEXT,
    PRIMARY KEY (run_id, path)
);
CREATE INDEX IF NOT EXISTS idx_obs_path ON observations(path);
CREATE INDEX IF NOT EXISTS idx_obs_object ON observations(device, inode);
CREATE INDEX IF NOT EXISTS idx_obs_content ON observations(content_sha256);
CREATE TABLE IF NOT EXISTS relationship_obs (
    run_id    TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    rel_id    TEXT NOT NULL,
    kind      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    member_count INTEGER NOT NULL,
    recoverable INTEGER,
    accounting TEXT NOT NULL,
    PRIMARY KEY (run_id, rel_id)
);
CREATE TABLE IF NOT EXISTS relationship_members (
    run_id TEXT NOT NULL,
    rel_id TEXT NOT NULL,
    path   TEXT NOT NULL,
    device INTEGER,
    inode  INTEGER,
    PRIMARY KEY (run_id, rel_id, path)
);
CREATE INDEX IF NOT EXISTS idx_rel_members_path ON relationship_members(path);
";

/// Minimal core v1 bootstrap, matching `coresight_core::db`'s schema.
const V1_BOOTSTRAP: &str = "
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
";

/// The full config fingerprint JSON a genuine v2 store wrote (serde of
/// the v2 `ConfigFingerprint` — the current reader decodes strictly, and
/// a real v2 database always carried this shape).
const V2_CONFIG: &str = r#"{"observationModel":1,"classifierSchema":"spacelens.v1.classification","classifierRules":1,"hashAlgorithm":"sha256","relationshipSchema":1,"historySchema":1}"#;

/// Build a genuine v2 database: one committed run, two content-identical
/// observations, one `relationship_obs` row and its two
/// `relationship_members` rows — exactly what a pre-v4 store that had
/// run the relationship layer contained (the metadata columns did not
/// exist yet, so no report status could have been recorded).
fn build_legacy_relational_v2(
    db: &Path,
    started_at: i64,
    completed_at: Option<i64>,
    modified: Option<i64>,
) {
    let conn = Connection::open(db).unwrap();
    conn.execute_batch(V1_BOOTSTRAP).unwrap();
    conn.execute_batch(V2_SCHEMA).unwrap();
    conn.execute("UPDATE schema_version SET version = 2", [])
        .unwrap();
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, completed_at, roots, platform, config, status)
         VALUES ('legacy-rel-run', ?1, ?2, '[\"/scope-a\"]', 'test', ?3, 'COMPLETED')",
        params![started_at, completed_at, V2_CONFIG],
    )
    .unwrap();
    for path in ["/scope-a/a.bin", "/scope-a/b.bin"] {
        conn.execute(
            "INSERT INTO observations
             (run_id, path, kind, size, device, inode, modified, content_sha256)
             VALUES ('legacy-rel-run', ?1, 'FILE', 100, 7, 100, ?2, 'deadbeef')",
            params![path, modified],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO relationship_obs
         (run_id, rel_id, kind, size, member_count, recoverable, accounting)
         VALUES ('legacy-rel-run', 'content-deadbeef', 'CONTENT_DUPLICATE', 100, 2, 100, 'EXACT')",
        [],
    )
    .unwrap();
    for path in ["/scope-a/a.bin", "/scope-a/b.bin"] {
        conn.execute(
            "INSERT INTO relationship_members (run_id, rel_id, path, device, inode)
             VALUES ('legacy-rel-run', 'content-deadbeef', ?1, 7, 100)",
            params![path],
        )
        .unwrap();
    }
    conn.close().unwrap();
}

/// Build the genuine v2 fixture and open it through the repository's
/// actual migration chain (v2 → v6), returning the live store.
fn open_migrated_legacy_store(db: &Path, started_at: i64) -> HistoryStore {
    build_legacy_relational_v2(db, started_at, Some(started_at + 500), Some(started_at));
    let store = HistoryStore::open(db).unwrap();
    assert_eq!(
        current_version(db),
        HISTORY_SCHEMA_VERSION,
        "the legacy store must migrate all the way to v6"
    );
    store
}

fn current_version(db: &Path) -> u32 {
    Connection::open(db)
        .unwrap()
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap()
}

struct RowCounts {
    observations: i64,
    relationship_obs: i64,
    relationship_members: i64,
}

fn row_counts(db: &Path) -> RowCounts {
    let conn = Connection::open(db).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    let counts = RowCounts {
        observations: count("observations"),
        relationship_obs: count("relationship_obs"),
        relationship_members: count("relationship_members"),
    };
    conn.close().unwrap();
    counts
}
