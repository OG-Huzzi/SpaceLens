//! Phase 6.4.4 — numeric preflight closure and relationship-metadata
//! consistency.
//!
//! Workstream A: every caller-controlled unsigned value that reaches an
//! SQLite INTEGER column is checked BEFORE the write transaction opens,
//! with typed errors naming the owning table and column.
//!
//! Workstream C: the `rel_status` / `rel_truncated` pair has one explicit
//! nullability contract; combinations no writer can produce are rejected,
//! and every legitimate historical state is preserved.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use coresight_history::{
    ConfigFingerprint, HistoryStore, ObservedEntry, ObservedKind, RunCounts, RunId, RunRecord,
    RunStatus, Snapshot, StoreError,
};
use coresight_identity::{AliasSet, DuplicateStatus, StorageAccounting, Undetermined};
use std::time::SystemTime;

fn config_json() -> String {
    serde_json::to_string(&ConfigFingerprint::current()).unwrap()
}

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

fn empty_snapshot(run_id: RunId) -> Snapshot {
    Snapshot {
        run_id,
        entries: Vec::new(),
    }
}

fn far_future() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(32503680000)
}

fn entry(size: Option<u64>, modified: Option<SystemTime>) -> ObservedEntry {
    entry_at("/scope-a/x", size, modified)
}

fn entry_at(path: &str, size: Option<u64>, modified: Option<SystemTime>) -> ObservedEntry {
    ObservedEntry {
        path: PathBuf::from(path),
        kind: ObservedKind::File,
        size,
        object: None,
        modified,
        classification: None,
        content_sha256: None,
        observation_error: None,
    }
}

// ---------------------------------------------------------------------------
// Workstream A — numeric preflight
// ---------------------------------------------------------------------------

/// A per-field check applied to a `RunCounts` value.
type CountField = (&'static str, fn(&mut RunCounts));

/// Every one of the nine `RunCounts` fields is checked independently: a
/// `u64::MAX` in any of them is rejected at preflight as caller input,
/// naming the exact column.
#[test]
fn every_run_count_field_is_checked_before_the_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let field_cases: [CountField; 9] = [
        ("entries_examined", |c| c.entries_examined = u64::MAX),
        ("files", |c| c.files = u64::MAX),
        ("dirs", |c| c.dirs = u64::MAX),
        ("links", |c| c.links = u64::MAX),
        ("other_entries", |c| c.other_entries = u64::MAX),
        ("bytes", |c| c.bytes = u64::MAX),
        ("observation_errors", |c| c.observation_errors = u64::MAX),
        ("candidates_untracked", |c| {
            c.candidates_untracked = u64::MAX
        }),
        ("hash_failures", |c| c.hash_failures = u64::MAX),
    ];
    for (i, (column, set)) in field_cases.iter().enumerate() {
        let db = dir.path().join(format!("count-{i}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let mut record = run_record(&format!("count-run-{i}"));
        set(&mut record.counts);
        store.begin_run(&record).unwrap();
        let err = store
            .commit_run(&record, &empty_snapshot(record.run_id.clone()), None)
            .expect_err("an unrepresentable count must be rejected");
        match &err {
            StoreError::InvalidRunCount {
                table,
                column: col,
                value,
            } => {
                assert_eq!(*table, "scan_runs", "{column}: wrong table");
                assert_eq!(col, column, "{column}: wrong column");
                assert_eq!(*value, u64::MAX, "{column}: wrong value");
            }
            other => panic!("{column}: expected InvalidRunCount, got {other:?}"),
        }
        // No partial commit: the run must still be RUNNING, never Completed.
        assert_eq!(
            store.get_run(&record.run_id).unwrap().unwrap().status,
            RunStatus::Running,
            "{column}: the run must remain RUNNING, not partially committed"
        );
    }
}

/// Valid boundary values are ACCEPTED: `0`, `i64::MAX`, and `None` for the
/// optional size all round-trip correctly.
#[test]
fn valid_numeric_boundaries_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("valid.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let mut record = run_record("valid-run");
    // i64::MAX in every counter is still representable.
    record.counts.entries_examined = i64::MAX as u64;
    record.counts.bytes = i64::MAX as u64;
    store.begin_run(&record).unwrap();

    // `None` size, `Some(0)`, and `Some(i64::MAX)` are all valid. Each
    // entry needs a distinct path (the observations PK is per-path).
    let snap = Snapshot {
        run_id: record.run_id.clone(),
        entries: vec![
            entry_at("/scope-a/none", None, None),
            entry_at("/scope-a/zero", Some(0), None),
            entry_at("/scope-a/max", Some(i64::MAX as u64), None),
        ],
    };
    store.commit_run(&record, &snap, None).unwrap();
    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    assert_eq!(loaded.run.counts.entries_examined, i64::MAX as u64);
    // The store loads observations in canonical PATH order (documented), so
    // assert the value per path and that the order is canonical.
    let by_path: std::collections::BTreeMap<String, Option<u64>> = loaded
        .snapshot
        .entries
        .iter()
        .map(|e| (e.path.to_string_lossy().to_string(), e.size))
        .collect();
    let expected: std::collections::BTreeMap<String, Option<u64>> = [
        ("/scope-a/none".to_string(), None),
        ("/scope-a/zero".to_string(), Some(0)),
        ("/scope-a/max".to_string(), Some(i64::MAX as u64)),
    ]
    .into_iter()
    .collect();
    assert_eq!(by_path, expected, "every boundary value must round-trip");
    let paths: Vec<String> = loaded
        .snapshot
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "observations load in canonical path order");
    let rows = store
        .list_runs(&coresight_history::QueryLimits::default())
        .unwrap();
    assert_eq!(rows.len(), 1);
}

/// Each relationship numeric field is checked independently: `size`,
/// `member_count`, `recoverable_bytes`, and `relationships_truncated`.
#[test]
fn every_relationship_numeric_field_is_checked() {
    let dir = tempfile::tempdir().unwrap();

    // relationship size.
    {
        let db = dir.path().join("rel-size.db");
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record("rel-size-run");
        store.begin_run(&record).unwrap();
        let mut rel = base_relationship();
        rel.size = u64::MAX;
        let err = store
            .commit_run(
                &record,
                &empty_snapshot(record.run_id.clone()),
                Some(&report_with(vec![rel])),
            )
            .expect_err("an unrepresentable relationship size must be rejected");
        assert!(matches!(
            &err,
            StoreError::InvalidRunCount { table, column, value }
                if *table == "relationship_obs" && *column == "size" && *value == u64::MAX
        ));
    }
    // member_count.
    {
        let db = dir.path().join("rel-count.db");
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record("rel-count-run");
        store.begin_run(&record).unwrap();
        let mut rel = base_relationship();
        rel.member_count = u64::MAX;
        let err = store
            .commit_run(
                &record,
                &empty_snapshot(record.run_id.clone()),
                Some(&report_with(vec![rel])),
            )
            .expect_err("an unrepresentable member count must be rejected");
        assert!(matches!(
            &err,
            StoreError::InvalidRunCount { table, column, value }
                if *table == "relationship_obs" && *column == "member_count" && *value == u64::MAX
        ));
    }
    // recoverable_bytes.
    {
        let db = dir.path().join("rel-rec.db");
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record("rel-rec-run");
        store.begin_run(&record).unwrap();
        let mut rel = base_relationship();
        rel.recoverable_bytes = Some(u64::MAX);
        let err = store
            .commit_run(
                &record,
                &empty_snapshot(record.run_id.clone()),
                Some(&report_with(vec![rel])),
            )
            .expect_err("an unrepresentable recoverable value must be rejected");
        assert!(matches!(
            &err,
            StoreError::InvalidRunCount { table, column, value }
                if *table == "relationship_obs" && *column == "recoverable" && *value == u64::MAX
        ));
    }
    // relationships_truncated.
    {
        let db = dir.path().join("rel-trunc.db");
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record("rel-trunc-run");
        store.begin_run(&record).unwrap();
        let mut report = report_with(vec![base_relationship()]);
        report.relationships_truncated = u64::MAX;
        let err = store
            .commit_run(
                &record,
                &empty_snapshot(record.run_id.clone()),
                Some(&report),
            )
            .expect_err("an unrepresentable truncation counter must be rejected");
        assert!(matches!(
            &err,
            StoreError::InvalidRunCount { table, column, value }
                if *table == "scan_runs" && *column == "rel_truncated" && *value == u64::MAX
        ));
    }
}

/// `ObservedEntry.size` is checked as caller input.
#[test]
fn an_unrepresentable_observation_size_is_caller_input() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("obs-size.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("obs-size-run");
    store.begin_run(&record).unwrap();
    let snap = Snapshot {
        run_id: record.run_id.clone(),
        entries: vec![entry(Some(u64::MAX), None)],
    };
    let err = store
        .commit_run(&record, &snap, None)
        .expect_err("an unrepresentable observation size must be rejected");
    assert!(matches!(
        &err,
        StoreError::InvalidRunCount { table, column, value }
            if *table == "observations" && *column == "size" && *value == u64::MAX
    ));
    // Nothing partial.
    assert_eq!(
        store.get_run(&record.run_id).unwrap().unwrap().status,
        RunStatus::Running
    );
    assert!(store
        .load_run_snapshot(&record.run_id)
        .unwrap()
        .unwrap()
        .snapshot
        .entries
        .is_empty());
}

/// The observation `modified` timestamp must be rejected with the OWNING
/// table named (`observations.modified`), not `scan_runs`. Phase 6.4.5
/// completes the contract: the error is typed CALLER INPUT
/// (`InvalidTimestamp`), not persisted corruption.
#[test]
fn an_observation_timestamp_error_names_its_owning_table() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("obs-ts.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("obs-ts-run");
    store.begin_run(&record).unwrap();
    let snap = Snapshot {
        run_id: record.run_id.clone(),
        entries: vec![entry(Some(1), Some(far_future()))],
    };
    let err = store
        .commit_run(&record, &snap, None)
        .expect_err("an unrepresentable observation timestamp must be rejected");
    match &err {
        StoreError::InvalidTimestamp {
            table,
            column,
            run_id,
            nanos,
        } => {
            assert_eq!(*table, "observations");
            assert_eq!(*column, "modified");
            assert_eq!(run_id.as_deref(), Some("obs-ts-run"));
            assert_eq!(
                *nanos,
                far_future().duration_since(UNIX_EPOCH).unwrap().as_nanos()
            );
        }
        other => panic!("expected InvalidTimestamp for caller input, got {other:?}"),
    }
    assert!(store
        .load_run_snapshot(&record.run_id)
        .unwrap()
        .unwrap()
        .snapshot
        .entries
        .is_empty());
}

/// `begin_run` validates `started_at` BEFORE opening its transaction:
/// no row is created and no partial state exists. The rejection is typed
/// caller input (`InvalidTimestamp`), not persisted corruption (Phase
/// 6.4.5).
#[test]
fn begin_run_validates_started_at_before_its_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("begin.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let mut record = run_record("begin-run");
    record.started_at = far_future();
    let err = store
        .begin_run(&record)
        .expect_err("an unrepresentable started_at must be rejected");
    match &err {
        StoreError::InvalidTimestamp {
            table,
            column,
            run_id,
            ..
        } => {
            assert_eq!(*table, "scan_runs");
            assert_eq!(*column, "started_at");
            assert_eq!(run_id.as_deref(), Some("begin-run"));
        }
        other => panic!("expected InvalidTimestamp for caller input, got {other:?}"),
    }
    // No row at all.
    assert!(store.get_run(&record.run_id).unwrap().is_none());
    assert!(store
        .list_runs(&coresight_history::QueryLimits::default())
        .unwrap()
        .is_empty());
}

/// Full-width object identity still round-trips: `u64::MAX` components are
/// intentional bit patterns and must NOT be range-checked.
#[test]
fn full_width_identity_is_not_range_checked() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("identity.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("identity-run");
    store.begin_run(&record).unwrap();
    // A snapshot whose entry carries a u64::MAX identity. The size must
    // stay representable so the identity (not the size) is what is tested.
    let snap = Snapshot {
        run_id: record.run_id.clone(),
        entries: vec![ObservedEntry {
            path: PathBuf::from("/scope-a/wide"),
            kind: ObservedKind::File,
            size: Some(1),
            object: Some(coresight_history::ObjectId {
                device: u64::MAX,
                inode: u64::MAX,
                // The wide-identity high bits are the clearest u64::MAX case.
                file_id_hi: Some(u64::MAX),
            }),
            modified: None,
            classification: None,
            content_sha256: None,
            observation_error: None,
        }],
    };
    store.commit_run(&record, &snap, None).unwrap();
    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    let object = loaded.snapshot.entries[0].object.unwrap();
    assert_eq!(object.device, u64::MAX);
    assert_eq!(object.inode, u64::MAX);
    assert_eq!(object.file_id_hi, Some(u64::MAX));
}

// ---------------------------------------------------------------------------
// Workstream C — relationship status/counter metadata
// ---------------------------------------------------------------------------

/// A minimal content-duplicate relationship.
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
        alias_sets: Vec::<AliasSet>::new(),
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

/// The store's own schema, used to build raw fixture rows.
const RAW_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS scan_runs (
    run_id TEXT PRIMARY KEY,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    roots TEXT NOT NULL,
    platform TEXT NOT NULL,
    config TEXT NOT NULL,
    status TEXT NOT NULL,
    entries_examined INTEGER NOT NULL DEFAULT 0,
    files INTEGER NOT NULL DEFAULT 0,
    dirs INTEGER NOT NULL DEFAULT 0,
    links INTEGER NOT NULL DEFAULT 0,
    other_entries INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    observation_errors INTEGER NOT NULL DEFAULT 0,
    candidates_untracked INTEGER NOT NULL DEFAULT 0,
    hash_failures INTEGER NOT NULL DEFAULT 0,
    rel_status TEXT,
    rel_truncated INTEGER
);
CREATE TABLE IF NOT EXISTS observations (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    path TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER,
    device INTEGER, inode INTEGER, file_id_hi INTEGER, modified INTEGER,
    category TEXT, subcategory TEXT, content_sha256 TEXT, obs_error TEXT,
    PRIMARY KEY (run_id, path)
);
CREATE TABLE IF NOT EXISTS relationship_obs (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    rel_id TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER NOT NULL,
    member_count INTEGER NOT NULL, recoverable INTEGER, accounting TEXT NOT NULL,
    PRIMARY KEY (run_id, rel_id)
);
CREATE TABLE IF NOT EXISTS relationship_members (
    run_id TEXT NOT NULL, rel_id TEXT NOT NULL, path TEXT NOT NULL,
    device INTEGER, inode INTEGER, file_id_hi INTEGER,
    PRIMARY KEY (run_id, rel_id, path)
);
";

/// Insert one raw run row with an explicit (status, counter) pair.
fn insert_raw_run(db: &Path, run_id: &str, rel_status: Option<&str>, rel_truncated: Option<i64>) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(RAW_SCHEMA).unwrap();
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (6)",
        rusqlite::params![],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, roots, platform, config, status, rel_status, rel_truncated)
         VALUES (?1, 1000000000, '[\"u:/scope-a\"]', 'test', ?2, 'COMPLETED', ?3, ?4)",
        rusqlite::params![run_id, config_json(), rel_status, rel_truncated],
    )
    .unwrap();
    conn.close().unwrap();
}

/// Status absent, counter absent: no report was recorded.
#[test]
fn metadata_status_absent_counter_absent_means_no_report() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("both-null.db");
    insert_raw_run(&db, "run", None, None);
    let store = HistoryStore::open(&db).unwrap();
    let loaded = store
        .load_run_snapshot(&RunId("run".to_string()))
        .unwrap()
        .unwrap();
    assert!(
        loaded.relationships.is_none(),
        "both NULL means no relationship report was recorded"
    );
}

/// Status present, counter present (including zero): a report WAS recorded,
/// and a recorded zero-truncation report stays distinct from no report.
#[test]
fn metadata_status_present_counter_present_records_the_report() {
    let dir = tempfile::tempdir().unwrap();
    for (i, (status, count)) in [
        ("COMPLETED", 0i64),
        ("COMPLETED_WITH_LIMITS", 7),
        ("COMPLETED", 1),
    ]
    .iter()
    .enumerate()
    {
        let db = dir.path().join(format!("both-set-{i}.db"));
        insert_raw_run(&db, "run", Some(status), Some(*count));
        let store = HistoryStore::open(&db).unwrap();
        let loaded = store
            .load_run_snapshot(&RunId("run".to_string()))
            .unwrap()
            .unwrap();
        let report = loaded
            .relationships
            .as_ref()
            .expect("a present pair must attach a report");
        assert_eq!(report.relationships_truncated, *count as u64);
        // The status decoded back to what was stored.
        let expected = match *status {
            "COMPLETED" => DuplicateStatus::Completed,
            _ => DuplicateStatus::CompletedWithLimits,
        };
        assert_eq!(report.status, expected);
    }
}

/// Status present, counter absent: no writer produces this, so it is
/// rejected with the offending column and run named.
#[test]
fn metadata_status_present_counter_absent_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mixed-a.db");
    insert_raw_run(&db, "run", Some("COMPLETED"), None);
    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_run_snapshot(&RunId("run".to_string()))
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
            assert_eq!(run_id.as_deref(), Some("run"));
        }
        other => panic!("expected typed corruption, got {other:?}"),
    }
}

/// Status absent, counter present: likewise impossible for any writer.
#[test]
fn metadata_status_absent_counter_present_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mixed-b.db");
    insert_raw_run(&db, "run", None, Some(5));
    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_run_snapshot(&RunId("run".to_string()))
        .expect_err("an inconsistent pair must be rejected");
    match &err {
        StoreError::Corrupt { table, column, .. } => {
            assert_eq!(*table, "scan_runs");
            assert_eq!(*column, "rel_status", "the missing field is reported");
        }
        other => panic!("expected typed corruption, got {other:?}"),
    }
}

/// A legacy NEGATIVE counter (a valid pre-6.4.2 wrapped bit pattern) is
/// still reconstructed faithfully when paired with a status — the 6.4.3
/// compatibility policy is preserved, and the missing-field inference is
/// NOT applied because the counter IS present.
#[test]
fn a_legacy_negative_counter_still_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-neg.db");
    let huge = 10_000_000_000_000_000_000u64;
    insert_raw_run(&db, "run", Some("COMPLETED_WITH_LIMITS"), Some(huge as i64));
    let store = HistoryStore::open(&db).unwrap();
    let loaded = store
        .load_run_snapshot(&RunId("run".to_string()))
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
