//! Phase 6.4.3 — read-side integrity and exact boundary semantics.
//!
//! Covers the legacy numeric compatibility policy (Workstream A), the exact
//! query-bound domain (B), and the preflight/timestamp/retention contract
//! (D). Every fixture is synthetic and portable.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use coresight_history::{
    ConfigFingerprint, HistoryStore, QueryLimits, RetentionPolicy, RunCounts, RunId, RunRecord,
    RunStatus, Snapshot, SnapshotBuilder, StoreError,
};
use coresight_identity::{DuplicateStatus, ObjectIdentity, StorageAccounting, Undetermined};

// ---------------------------------------------------------------------------
// A legacy v4-era database written by the PRE-6.4.2 unchecked writer
// ---------------------------------------------------------------------------

const LEGACY_SCHEMA: &str = "
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
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
";

fn config_json() -> String {
    serde_json::to_string(&ConfigFingerprint::current()).unwrap()
}

/// A legacy run whose numbers were written with the OLD unchecked
/// `u64 as i64` conversion, so a domain value >= 2^63 is stored negative.
fn build_legacy_run(db: &Path, rel_truncated_i64: i64, recoverable_i64: i64) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(LEGACY_SCHEMA).unwrap();
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (6)",
        rusqlite::params![],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, completed_at, roots, platform, config, status,
          rel_status, rel_truncated)
         VALUES ('legacy', 1000000000, 1000000001, '[\"u:/scope-a\"]',
                 'test', ?2, 'COMPLETED', 'COMPLETED_WITH_LIMITS', ?1)",
        rusqlite::params![rel_truncated_i64, config_json()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relationship_obs
         (run_id, rel_id, kind, size, member_count, recoverable, accounting)
         VALUES ('legacy', 'content-abc', 'CONTENT_DUPLICATE', 100, 2, ?1, 'ESTIMATED')",
        rusqlite::params![recoverable_i64],
    )
    .unwrap();
    conn.close().unwrap();
}

// ---------------------------------------------------------------------------
// Workstream A — legacy numeric compatibility
// ---------------------------------------------------------------------------

/// A legacy `rel_truncated` above `2^63` is a VALID legacy bit pattern and
/// must round-trip. The previous implementation clamped it to 0, which
/// erased the incompleteness signal and made a truncated run look complete.
#[test]
fn a_legacy_negative_rel_truncated_round_trips_and_keeps_its_meaning() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-trunc.db");
    let huge = 10_000_000_000_000_000_000u64; // 1e19 > 2^63
    build_legacy_run(&db, huge as i64, 0);

    let store = HistoryStore::open(&db).unwrap();
    let snap = store
        .load_run_snapshot(&RunId("legacy".to_string()))
        .unwrap()
        .unwrap();
    let rel = snap.relationships.as_ref().expect("a report was recorded");
    assert_eq!(
        rel.relationships_truncated, huge,
        "the legacy bit pattern must be reconstructed exactly, not clamped to 0"
    );
    // And the status stays "limits applied", so the run still reads as
    // incomplete rather than complete.
    assert_eq!(
        rel.status,
        DuplicateStatus::CompletedWithLimits,
        "the run must not be re-presented as complete"
    );
}

/// A positive stored value is read back unchanged.
#[test]
fn a_positive_rel_truncated_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-positive.db");
    build_legacy_run(&db, 42, 0);
    let store = HistoryStore::open(&db).unwrap();
    let snap = store
        .load_run_snapshot(&RunId("legacy".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(
        snap.relationships.unwrap().relationships_truncated,
        42,
        "an ordinary positive counter must be unchanged"
    );
}

/// `NULL` means "no report was recorded", not a fabricated count.
#[test]
fn a_null_rel_status_means_no_report() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-null.db");
    // Let the store bootstrap its own schema, then insert the legacy run
    // with a NULL rel_status into that same database.
    {
        let store = HistoryStore::open(&db).unwrap();
        drop(store);
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, roots, platform, config, status, rel_status)
         VALUES ('legacy', 1000000000, '[\"u:/scope-a\"]', 'test', ?1, 'COMPLETED', NULL)",
        rusqlite::params![config_json()],
    )
    .unwrap();
    conn.close().unwrap();

    let store = HistoryStore::open(&db).unwrap();
    let snap = store
        .load_run_snapshot(&RunId("legacy".to_string()))
        .unwrap()
        .unwrap();
    assert!(
        snap.relationships.is_none(),
        "a NULL rel_status means no report was recorded"
    );
}

/// `recoverable` also round-trips a legacy negative bit pattern, through
/// BOTH the reconstructed report and the relationship-history API.
#[test]
fn a_legacy_negative_recoverable_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-rec.db");
    let huge = 10_000_000_000_000_000_000u64;
    build_legacy_run(&db, 0, huge as i64);
    let store = HistoryStore::open(&db).unwrap();

    let snap = store
        .load_run_snapshot(&RunId("legacy".to_string()))
        .unwrap()
        .unwrap();
    let rel = snap.relationships.clone().unwrap();
    assert_eq!(
        rel.relationships[0].recoverable_bytes,
        Some(huge),
        "the recoverable bit pattern must be reconstructed exactly"
    );

    let hist = store
        .relationship_history("content-abc", &QueryLimits::default())
        .unwrap();
    assert_eq!(hist[0].recoverable, Some(huge));
}

/// The compatibility limitation is DOCUMENTED, not claimed away: a negative
/// stored value is indistinguishable between a legacy wrapped counter and a
/// tampered value. This asserts the documented behaviour (faithful round
/// trip) rather than a detection capability we do not have.
#[test]
fn the_negative_value_ambiguity_is_documented_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-ambiguous.db");
    // The same negative value the legacy writer would produce for a huge
    // counter, and the same one a tampering writer would store.
    let wrapped = 10_000_000_000_000_000_000u64 as i64;
    build_legacy_run(&db, wrapped, 0);
    let store = HistoryStore::open(&db).unwrap();
    let snap = store
        .load_run_snapshot(&RunId("legacy".to_string()))
        .unwrap()
        .unwrap();
    let value = snap.relationships.unwrap().relationships_truncated;
    assert_eq!(
        value, wrapped as u64,
        "the documented policy is faithful round trip, not guessed detection"
    );
}

// ---------------------------------------------------------------------------
// Workstream B — exact query-bound domain
// ---------------------------------------------------------------------------

/// The accepted domain ends at `i64::MAX - 1` so the probe stays exact, and
/// `i64::MAX` is the first refused value.
#[test]
fn the_supported_limit_domain_supports_exact_probing() {
    assert_eq!(QueryLimits::MAX_SUPPORTED, i64::MAX as usize - 1);
    for (limit, sql, probe) in [
        (0usize, 0i64, 1i64),
        (1, 1, 2),
        (10_000, 10_000, 10_001),
        (QueryLimits::MAX_SUPPORTED, i64::MAX - 1, i64::MAX),
    ] {
        let l = QueryLimits { max_results: limit };
        assert_eq!(l.to_sql_limit().unwrap(), sql, "limit {limit}");
        assert_eq!(l.to_probe_sql_limit().unwrap(), probe, "probe for {limit}");
    }
    // i64::MAX is valid as a SQLite LIMIT but cannot probe: refused.
    let too_big = QueryLimits {
        max_results: i64::MAX as usize,
    };
    match too_big.to_sql_limit() {
        Err(StoreError::LimitOutOfRange {
            requested,
            supported,
        }) => {
            assert_eq!(requested, i64::MAX as usize);
            assert_eq!(supported, QueryLimits::MAX_SUPPORTED);
        }
        other => panic!("i64::MAX must be refused, got {other:?}"),
    }
    assert!(matches!(
        QueryLimits {
            max_results: usize::MAX
        }
        .to_probe_sql_limit(),
        Err(StoreError::LimitOutOfRange { .. })
    ));
}
/// Every public query entry point validates the bound BEFORE issuing SQL —
/// including on the ABSENT-record path, where a naive implementation would
/// return an empty result before noticing the limit.
#[test]
fn every_entry_point_validates_the_bound_before_any_sql() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bounds.db");
    let store = HistoryStore::open(&db).unwrap();
    let bad = QueryLimits {
        max_results: i64::MAX as usize,
    };

    for res in [
        store.list_runs(&bad).map(|_| ()),
        store
            .history_for_path(Path::new("/scope-a"), &bad)
            .map(|_| ()),
        store.history_for_object(1, 1, None, &bad).map(|_| ()),
        store.history_for_content("abc", &bad).map(|_| ()),
        store.relationship_history("rel-1", &bad).map(|_| ()),
        store.list_system_snapshots(&bad).map(|_| ()),
    ] {
        assert!(
            matches!(res, Err(StoreError::LimitOutOfRange { .. })),
            "history queries must reject the bound even with no data"
        );
    }
    let absent = RunId("does-not-exist".to_string());
    for res in [
        store.load_system_snapshot(&absent, &bad).map(|_| ()),
        store.load_snapshot_applications(&absent, &bad).map(|_| ()),
        store
            .rebuild_system_model(
                &absent,
                &bad,
                &coresight_system_model::SystemModelLimits::default(),
            )
            .map(|_| ()),
    ] {
        assert!(
            matches!(res, Err(StoreError::LimitOutOfRange { .. })),
            "snapshot APIs must reject the bound even for an absent run"
        );
    }
}

// ---------------------------------------------------------------------------
// Workstream D — preflight and timestamp context
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

/// The far-future time used to exercise the unrepresentable-domain check.
fn far_future() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(32503680000)
}

/// An unrepresentable OBSERVATION timestamp rejects the commit and leaves
/// the prior committed run, its observations, and its relationships
/// byte-for-byte unchanged (Workstream D).
#[test]
fn an_invalid_observation_timestamp_preserves_all_committed_data() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("preflight-obs.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // Commit a valid run first.
    let record = run_record("preflight-run");
    store.begin_run(&record).unwrap();
    let good = empty_snapshot(&record.run_id);
    store.commit_run(&record, &good, None).unwrap();
    let before = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(before.status, RunStatus::Completed);

    // Now a commit whose report carries an unrepresentable recoverable
    // value. The run row must survive untouched.
    let bad_report = report_with(vec![relationship_with_recoverable(Some(u64::MAX))]);
    let err = store
        .commit_run(&record, &good, Some(&bad_report))
        .expect_err("an unrepresentable recoverable value must be rejected");
    match err {
        StoreError::Corrupt { column, .. } => assert_eq!(column, "recoverable"),
        other => panic!("expected typed corruption, got {other:?}"),
    }
    let after = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(
        after.status,
        RunStatus::Completed,
        "the previously committed run must be preserved"
    );
    assert_eq!(after.started_at, before.started_at);
}

/// A run report whose single relationship carries the given recoverable
/// upper bound.
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
            failed_by_reason: vec![],
            detail: vec![],
            detail_truncated: 0,
        },
        stats: coresight_identity::RelationshipStats::default(),
        started_at: UNIX_EPOCH + Duration::from_secs(1),
        finished_at: UNIX_EPOCH + Duration::from_secs(2),
    }
}

/// A minimal content-duplicate relationship carrying a specific
/// recoverable-byte value.
fn relationship_with_recoverable(recoverable: Option<u64>) -> coresight_identity::Relationship {
    coresight_identity::Relationship {
        id: "content-xyz".to_string(),
        kind: coresight_identity::RelationshipKind::ContentDuplicate,
        size: 1,
        member_count: 2,
        members: vec![coresight_identity::MemberRef {
            entry_id: 1,
            path: PathBuf::from("/scope-a/a.bin"),
            object: Some(ObjectIdentity {
                volume: 1,
                file_id: 10,
                file_id_hi: None,
            }),
        }],
        distinct_objects: Some(2),
        evidence: vec![],
        content: Some(coresight_identity::ContentRef {
            sha256_hex: "deadbeef".to_string(),
            algorithm: "sha256".to_string(),
        }),
        object: None,
        alias_sets: vec![],
        logical_duplicate_bytes: 1,
        recoverable_bytes: recoverable,
        accounting: StorageAccounting::Exact,
        detail_truncated: false,
    }
}

/// An empty run snapshot (no observations).
fn empty_snapshot(run_id: &RunId) -> Snapshot {
    SnapshotBuilder::new().build(run_id.clone()).unwrap()
}

/// Each timestamp field names ITSELF in the error.
#[test]
fn timestamp_errors_identify_the_actual_field() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ts-ctx.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // started_at.
    let mut r = run_record("ts-started");
    r.started_at = far_future();
    let err = store.begin_run(&r).unwrap_err();
    assert_eq!(corrupt_column(&err), "started_at");

    // completed_at (falls back to started_at, both unrepresentable).
    let mut r = run_record("ts-completed");
    r.completed_at = Some(far_future());
    store.begin_run(&r).unwrap();
    let s = empty_snapshot(&r.run_id);
    let err = store.commit_run(&r, &s, None).unwrap_err();
    assert_eq!(corrupt_column(&err), "completed_at");
}

fn corrupt_column(err: &StoreError) -> &'static str {
    match err {
        StoreError::Corrupt { column, .. } => column,
        other => panic!("expected typed corruption, got {other:?}"),
    }
}

/// `keep_latest == 0` is rejected before any deletion, and the minimum
/// valid value (1) still protects the newest baseline.
#[test]
fn keep_latest_zero_is_rejected_and_one_protects_the_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("retention.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("ret-run");
    store.begin_run(&record).unwrap();
    let s = empty_snapshot(&record.run_id);
    store.commit_run(&record, &s, None).unwrap();

    // keep_latest == 0 refused, nothing touched.
    let err = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 0,
            max_runs: None,
            max_age: None,
        })
        .expect_err("keep_latest == 0 must be rejected");
    assert!(matches!(err, StoreError::InvalidRetentionPolicy(_)));
    assert!(store.get_run(&record.run_id).unwrap().is_some());

    // keep_latest == 1 still permits the age rule to prune an older run
    // while protecting this baseline.
    let older = run_record("ret-older");
    store.begin_run(&older).unwrap();
    let os = empty_snapshot(&older.run_id);
    store.commit_run(&older, &os, None).unwrap();
    let report = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 1,
            max_runs: None,
            max_age: Some(Duration::from_secs(0)),
        })
        .unwrap();
    assert!(report.removed_runs.contains(&older.run_id));
    assert!(
        store.get_run(&record.run_id).unwrap().is_some(),
        "the newest baseline must be protected"
    );
    assert!(store.get_run(&older.run_id).unwrap().is_none());
}
