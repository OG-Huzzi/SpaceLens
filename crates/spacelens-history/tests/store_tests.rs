//! Integration tests for the SQLite history store (`spacelens_history::store`).
//!
//! Each test owns a `tempfile` directory and fixed run ids for determinism;
//! run timestamps are offsets from `UNIX_EPOCH` so ordering is exact and
//! independent of the wall clock. Paths are opaque POSIX-style strings —
//! the store treats them as data.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use spacelens_classifier::{Category, Subcategory};
use spacelens_engine::{EntryKind, FsEntry};
use spacelens_history::{
    BuildError, ClassificationRef, ConfigFingerprint, HistoryStore, ObservedEntry, ObservedKind,
    QueryLimits, RetentionPolicy, RunCounts, RunId, RunRecord, RunStatus, Snapshot,
    SnapshotBuilder, StoreError,
};
use spacelens_identity::{
    ContentRef, DuplicateStatus, MemberRef, ObjectRef, Relationship, RelationshipKind,
    RelationshipReport, RelationshipStats, StorageAccounting, Undetermined,
};

/// A verified content identity for round-trip tests (any hex string).
const CONTENT_ONE: &str = "cafe0011cafe0011cafe0011cafe0011cafe0011cafe0011cafe0011cafe0011";
/// A verified content identity observed on two different paths in two runs.
const CONTENT_SHARED: &str =
    "abc123def456abc123def456abc123def456abc123def456abc123def456abc123def";

/// Deterministic run start: `UNIX_EPOCH + 1_000_000 + offset` seconds.
fn started_at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000 + offset)
}

/// Deterministic entry modification time, distinct from all run starts.
fn entry_time() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(2_000_000)
}

fn run_record(id: &str, offset: u64, status: RunStatus, roots: &[&str]) -> RunRecord {
    RunRecord {
        run_id: RunId(id.to_string()),
        started_at: started_at(offset),
        completed_at: None,
        roots: roots.iter().map(PathBuf::from).collect(),
        platform: "test-platform".to_string(),
        config: ConfigFingerprint::current(),
        status,
        counts: RunCounts::default(),
    }
}

fn fs_entry(path: &str, kind: EntryKind, size: u64, object: Option<(u64, u64)>) -> FsEntry {
    let (device, inode) = match object {
        Some((d, i)) => (Some(d), Some(i)),
        None => (None, None),
    };
    FsEntry {
        id: 0,
        parent_id: None,
        path: PathBuf::from(path),
        kind,
        size,
        allocated_size: None,
        modified: Some(entry_time()),
        created: None,
        accessed: None,
        changed: None,
        device,
        inode,
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

fn empty_snapshot(run_id: RunId) -> Snapshot {
    SnapshotBuilder::new().build(run_id).unwrap()
}

fn snapshot_of(run_id: RunId, entries: &[FsEntry]) -> Snapshot {
    let mut builder = SnapshotBuilder::new();
    for entry in entries {
        builder.push_entry(entry, None);
    }
    builder.build(run_id).unwrap()
}

/// Begin and commit one `Completed` run holding exactly `entries`.
fn commit_completed_run(
    store: &mut HistoryStore,
    id: &str,
    offset: u64,
    roots: &[&str],
    entries: &[FsEntry],
) -> RunRecord {
    let record = run_record(id, offset, RunStatus::Completed, roots);
    store.begin_run(&record).unwrap();
    let snapshot = snapshot_of(record.run_id.clone(), entries);
    store.commit_run(&record, &snapshot, None).unwrap();
    record
}

/// Commit five completed runs, oldest `started_at` first; returns run ids
/// in creation order (index 0 = oldest, index 4 = newest).
fn commit_five_oldest_first(store: &mut HistoryStore, prefix: &str) -> Vec<RunId> {
    let mut ids = Vec::new();
    for n in 1..=5u64 {
        let record = commit_completed_run(store, &format!("{prefix}-{n}"), n, &["/scope-a"], &[]);
        ids.push(record.run_id);
    }
    ids
}

#[test]
fn run_lifecycle_start_complete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let record = run_record("test-run-a", 1, RunStatus::Completed, &["/scope-a"]);
    store.begin_run(&record).unwrap();

    let file1 = fs_entry("/scope-a/file1.bin", EntryKind::File, 1024, Some((1, 42)));
    let file2 = fs_entry("/scope-a/file2.bin", EntryKind::File, 2048, Some((1, 43)));
    let subdir = fs_entry("/scope-a/sub", EntryKind::Dir, 0, Some((1, 100)));
    let mut builder = SnapshotBuilder::new();
    builder
        .push_entry(
            &file1,
            Some(ClassificationRef::from_parts(Category::Documents, None)),
        )
        .push_entry(
            &file2,
            Some(ClassificationRef::from_parts(
                Category::Archives,
                Some(Subcategory::Archive),
            )),
        )
        .push_entry(&subdir, None);
    builder.set_content(Path::new("/scope-a/file1.bin"), CONTENT_ONE.to_string());
    let snapshot = builder.build(record.run_id.clone()).unwrap();

    store.commit_run(&record, &snapshot, None).unwrap();

    let run = store.get_run(&record.run_id).unwrap().expect("run row");
    assert_eq!(run.run_id, record.run_id);
    assert_eq!(run.status, RunStatus::Completed);
    assert_eq!(run.completed_at, Some(started_at(1)));

    let loaded = store
        .load_run_snapshot(&record.run_id)
        .unwrap()
        .expect("committed snapshot");
    assert_eq!(loaded.snapshot.run_id, record.run_id);
    let expected = vec![
        ObservedEntry {
            path: PathBuf::from("/scope-a/file1.bin"),
            kind: ObservedKind::File,
            size: Some(1024),
            object: Some((1, 42)),
            modified: Some(entry_time()),
            classification: Some(ClassificationRef::from_parts(Category::Documents, None)),
            content_sha256: Some(CONTENT_ONE.to_string()),
            observation_error: None,
        },
        ObservedEntry {
            path: PathBuf::from("/scope-a/file2.bin"),
            kind: ObservedKind::File,
            size: Some(2048),
            object: Some((1, 43)),
            modified: Some(entry_time()),
            classification: Some(ClassificationRef::from_parts(
                Category::Archives,
                Some(Subcategory::Archive),
            )),
            content_sha256: None,
            observation_error: None,
        },
        ObservedEntry {
            path: PathBuf::from("/scope-a/sub"),
            kind: ObservedKind::Dir,
            size: None,
            object: Some((1, 100)),
            modified: Some(entry_time()),
            classification: None,
            content_sha256: None,
            observation_error: None,
        },
    ];
    assert_eq!(loaded.snapshot.entries, expected);
}

#[test]
fn commit_requires_begun_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let record = run_record("never-begun", 1, RunStatus::Completed, &["/scope-a"]);
    let snapshot = empty_snapshot(record.run_id.clone());

    match store.commit_run(&record, &snapshot, None) {
        Err(StoreError::AlreadyCommitted(id)) => assert_eq!(id, record.run_id),
        other => panic!("expected AlreadyCommitted, got {other:?}"),
    }
    assert!(store.get_run(&record.run_id).unwrap().is_none());
}

#[test]
fn double_commit_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let record = run_record("twice-run", 1, RunStatus::Completed, &["/scope-a"]);
    let snapshot = snapshot_of(
        record.run_id.clone(),
        &[fs_entry(
            "/scope-a/one.bin",
            EntryKind::File,
            5,
            Some((1, 7)),
        )],
    );

    store.begin_run(&record).unwrap();
    store.commit_run(&record, &snapshot, None).unwrap();

    match store.commit_run(&record, &snapshot, None) {
        Err(StoreError::AlreadyCommitted(id)) => assert_eq!(id, record.run_id),
        other => panic!("expected AlreadyCommitted, got {other:?}"),
    }

    // The first commit's data is intact.
    let run = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Completed);
    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    assert_eq!(loaded.snapshot.entries.len(), 1);
    assert_eq!(
        loaded.snapshot.entries[0].path,
        PathBuf::from("/scope-a/one.bin")
    );
    assert_eq!(loaded.snapshot.entries[0].size, Some(5));
}

#[test]
fn snapshot_rejects_duplicate_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let record = run_record("dup-run", 1, RunStatus::Completed, &["/scope-a"]);
    store.begin_run(&record).unwrap();

    let twin1 = fs_entry("/scope-a/twin.bin", EntryKind::File, 10, Some((1, 50)));
    let twin2 = fs_entry("/scope-a/twin.bin", EntryKind::File, 20, Some((1, 51)));
    let mut builder = SnapshotBuilder::new();
    builder.push_entry(&twin1, None).push_entry(&twin2, None);

    match builder.build(record.run_id.clone()) {
        Err(BuildError::DuplicatePath(p)) => assert_eq!(p, PathBuf::from("/scope-a/twin.bin")),
        other => panic!("expected DuplicatePath, got {other:?}"),
    }

    // The store file is unaffected: the run row is untouched by the failed build.
    let run = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(store.list_runs(&QueryLimits::default()).unwrap().len(), 1);
}

#[test]
fn cancel_and_fail_statuses_persist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let cancelled = run_record("run-cancelled", 1, RunStatus::Running, &["/scope-a"]);
    store.begin_run(&cancelled).unwrap();
    store
        .abandon_run(&cancelled.run_id, RunStatus::Cancelled)
        .unwrap();

    let failed = run_record("run-failed", 2, RunStatus::Running, &["/scope-a"]);
    store.begin_run(&failed).unwrap();
    store
        .abandon_run(&failed.run_id, RunStatus::Failed)
        .unwrap();

    let got_cancelled = store.get_run(&cancelled.run_id).unwrap().unwrap();
    assert_eq!(got_cancelled.status, RunStatus::Cancelled);
    let got_failed = store.get_run(&failed.run_id).unwrap().unwrap();
    assert_eq!(got_failed.status, RunStatus::Failed);

    // No observations were committed, so the loaded snapshots are empty.
    let loaded_cancelled = store.load_run_snapshot(&cancelled.run_id).unwrap().unwrap();
    assert!(loaded_cancelled.snapshot.entries.is_empty());
    let loaded_failed = store.load_run_snapshot(&failed.run_id).unwrap().unwrap();
    assert!(loaded_failed.snapshot.entries.is_empty());
}

#[test]
fn crash_recovery_marks_running_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");

    let orphan = run_record("crashed-run", 1, RunStatus::Running, &["/scope-a"]);
    {
        let mut store = HistoryStore::open(&path).unwrap();
        store.begin_run(&orphan).unwrap();
        // Drop without committing — the "crash".
    }

    let mut store = HistoryStore::open(&path).unwrap();
    let recovered = store.get_run(&orphan.run_id).unwrap().unwrap();
    assert_eq!(recovered.status, RunStatus::Failed);

    // A new run on the recovered store works end to end.
    let record = run_record("post-recovery-run", 2, RunStatus::Completed, &["/scope-a"]);
    store.begin_run(&record).unwrap();
    let snapshot = snapshot_of(
        record.run_id.clone(),
        &[fs_entry(
            "/scope-a/alive.bin",
            EntryKind::File,
            9,
            Some((1, 70)),
        )],
    );
    store.commit_run(&record, &snapshot, None).unwrap();
    let done = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(done.status, RunStatus::Completed);
    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    assert_eq!(loaded.snapshot.entries.len(), 1);
}

#[test]
fn latest_run_for_scope_skips_partial_and_respects_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let run1 = run_record("scope-run-1", 1, RunStatus::Completed, &["/scope-a"]);
    store.begin_run(&run1).unwrap();
    store
        .commit_run(&run1, &empty_snapshot(run1.run_id.clone()), None)
        .unwrap();

    // A newer cancelled run: partial by definition, never a baseline.
    let run2 = run_record("scope-run-2", 2, RunStatus::Running, &["/scope-a"]);
    store.begin_run(&run2).unwrap();
    store
        .abandon_run(&run2.run_id, RunStatus::Cancelled)
        .unwrap();

    // The newest run: completed with limits, still full-scope.
    let run3 = run_record(
        "scope-run-3",
        3,
        RunStatus::CompletedWithLimits,
        &["/scope-a"],
    );
    store.begin_run(&run3).unwrap();
    store
        .commit_run(&run3, &empty_snapshot(run3.run_id.clone()), None)
        .unwrap();

    let latest = store
        .latest_run_for_scope(&[PathBuf::from("/scope-a")])
        .unwrap()
        .expect("a full-scope baseline exists");
    assert_eq!(latest.run_id, run3.run_id);
    assert_eq!(latest.status, RunStatus::CompletedWithLimits);

    // Unrelated scope: not covered by any run's roots.
    assert!(store
        .latest_run_for_scope(&[PathBuf::from("/other")])
        .unwrap()
        .is_none());
}

#[test]
fn history_for_path_across_runs_ordered_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let older = commit_completed_run(
        &mut store,
        "path-run-old",
        1,
        &["/scope-a"],
        &[fs_entry(
            "/scope-a/keep.bin",
            EntryKind::File,
            100,
            Some((1, 10)),
        )],
    );
    let newer = commit_completed_run(
        &mut store,
        "path-run-new",
        2,
        &["/scope-a"],
        &[fs_entry(
            "/scope-a/keep.bin",
            EntryKind::File,
            250,
            Some((1, 10)),
        )],
    );

    let points = store
        .history_for_path(Path::new("/scope-a/keep.bin"), &QueryLimits::default())
        .unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].run_id, newer.run_id);
    assert_eq!(points[0].size, Some(250));
    assert_eq!(points[0].object, Some((1, 10)));
    assert_eq!(points[1].run_id, older.run_id);
    assert_eq!(points[1].size, Some(100));
    assert_eq!(points[1].object, Some((1, 10)));
}

#[test]
fn history_for_object_and_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    // Run one: alpha.bin carries object (1, 42) and the shared content.
    let run_one = run_record("obj-run-one", 1, RunStatus::Completed, &["/scope-a"]);
    store.begin_run(&run_one).unwrap();
    let alpha = fs_entry("/scope-a/alpha.bin", EntryKind::File, 700, Some((1, 42)));
    let mut one = SnapshotBuilder::new();
    one.push_entry(&alpha, None);
    one.set_content(Path::new("/scope-a/alpha.bin"), CONTENT_SHARED.to_string());
    let snap_one = one.build(run_one.run_id.clone()).unwrap();
    store.commit_run(&run_one, &snap_one, None).unwrap();

    // Run two: a different path AND a different object, same verified content.
    let run_two = run_record("obj-run-two", 2, RunStatus::Completed, &["/scope-b"]);
    store.begin_run(&run_two).unwrap();
    let beta = fs_entry("/scope-b/beta.bin", EntryKind::File, 700, Some((1, 43)));
    let mut two = SnapshotBuilder::new();
    two.push_entry(&beta, None);
    two.set_content(Path::new("/scope-b/beta.bin"), CONTENT_SHARED.to_string());
    let snap_two = two.build(run_two.run_id.clone()).unwrap();
    store.commit_run(&run_two, &snap_two, None).unwrap();

    // By proven object identity: exactly the one entry.
    let by_object = store
        .history_for_object(1, 42, &QueryLimits::default())
        .unwrap();
    assert_eq!(by_object.len(), 1);
    assert_eq!(by_object[0].run_id, run_one.run_id);
    assert_eq!(by_object[0].object, Some((1, 42)));
    assert_eq!(by_object[0].content_sha256.as_deref(), Some(CONTENT_SHARED));

    // By content identity: both observations (one per run).
    let by_content = store
        .history_for_content(CONTENT_SHARED, &QueryLimits::default())
        .unwrap();
    assert_eq!(by_content.len(), 2, "both runs observed this content");
    let mut seen_runs: Vec<String> = by_content.iter().map(|p| p.run_id.0.clone()).collect();
    seen_runs.sort();
    assert_eq!(seen_runs, vec!["obj-run-one", "obj-run-two"]);

    // The two distinct paths carrying that content, confirmed per run.
    let loaded_one = store.load_run_snapshot(&run_one.run_id).unwrap().unwrap();
    assert_eq!(
        loaded_one.snapshot.entries[0].path,
        PathBuf::from("/scope-a/alpha.bin")
    );
    assert_eq!(
        loaded_one.snapshot.entries[0].content_sha256.as_deref(),
        Some(CONTENT_SHARED)
    );
    let loaded_two = store.load_run_snapshot(&run_two.run_id).unwrap().unwrap();
    assert_eq!(
        loaded_two.snapshot.entries[0].path,
        PathBuf::from("/scope-b/beta.bin")
    );
    assert_eq!(
        loaded_two.snapshot.entries[0].content_sha256.as_deref(),
        Some(CONTENT_SHARED)
    );
}

#[test]
fn query_limits_truncate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    for n in 1..=5u64 {
        commit_completed_run(
            &mut store,
            &format!("limit-run-{n}"),
            n,
            &["/scope-a"],
            &[fs_entry(
                "/scope-a/moved.bin",
                EntryKind::File,
                n * 100,
                Some((1, n)),
            )],
        );
    }

    let limited = store
        .history_for_path(
            Path::new("/scope-a/moved.bin"),
            &QueryLimits { max_results: 3 },
        )
        .unwrap();
    assert_eq!(limited.len(), 3);
    // Truncation keeps the newest observations.
    assert_eq!(limited[0].run_id, RunId("limit-run-5".into()));
    assert_eq!(limited[0].size, Some(500));
    assert_eq!(limited[2].run_id, RunId("limit-run-3".into()));
    assert_eq!(limited[2].size, Some(300));
}

#[test]
fn retention_keeps_latest_baseline_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let ids = commit_five_oldest_first(&mut store, "retain-run");

    let report = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 2,
            max_runs: None,
            max_age: None,
        })
        .unwrap();

    let mut removed: Vec<String> = report.removed_runs.iter().map(|r| r.0.clone()).collect();
    removed.sort();
    assert_eq!(
        removed,
        vec!["retain-run-1", "retain-run-2", "retain-run-3"],
        "the three runs older than the kept baseline must be removed"
    );
    assert_eq!(report.kept_runs, 2);

    let remaining: Vec<RunId> = store
        .list_runs(&QueryLimits::default())
        .unwrap()
        .into_iter()
        .map(|r| r.run_id)
        .collect();
    // Newest first: exactly the two newest runs remain.
    assert_eq!(remaining, vec![ids[4].clone(), ids[3].clone()]);
}

#[test]
fn retention_never_removes_the_newest_when_max_runs_is_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    commit_five_oldest_first(&mut store, "retain-run");

    let report = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 2,
            max_runs: Some(1),
            max_age: None,
        })
        .unwrap();

    // keep_latest wins over max_runs: the newest two baselines stay.
    let mut removed: Vec<String> = report.removed_runs.iter().map(|r| r.0.clone()).collect();
    removed.sort();
    assert_eq!(
        removed,
        vec!["retain-run-1", "retain-run-2", "retain-run-3"],
        "removals hit only runs older than the kept baselines"
    );
    assert_eq!(report.kept_runs, 2);

    let remaining: Vec<String> = store
        .list_runs(&QueryLimits::default())
        .unwrap()
        .into_iter()
        .map(|r| r.run_id.0)
        .collect();
    assert_eq!(remaining, vec!["retain-run-5", "retain-run-4"]);
}

#[test]
fn relationship_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = HistoryStore::open(&path).unwrap();

    let record = run_record("rel-run", 1, RunStatus::Completed, &["/scope-a"]);
    store.begin_run(&record).unwrap();

    let member_a = fs_entry("/scope-a/a.bin", EntryKind::File, 10, Some((1, 10)));
    let member_b = fs_entry("/scope-a/b.bin", EntryKind::File, 10, Some((1, 11)));
    let mut builder = SnapshotBuilder::new();
    builder
        .push_entry(&member_a, None)
        .push_entry(&member_b, None);
    builder.set_content(Path::new("/scope-a/a.bin"), "deadbeef".to_string());
    builder.set_content(Path::new("/scope-a/b.bin"), "deadbeef".to_string());
    let snapshot = builder.build(record.run_id.clone()).unwrap();

    let report = RelationshipReport {
        status: DuplicateStatus::Completed,
        relationships: vec![Relationship {
            id: "content-deadbeef".to_string(),
            kind: RelationshipKind::ContentDuplicate,
            size: 10,
            member_count: 2,
            members: vec![
                MemberRef {
                    entry_id: 1,
                    path: PathBuf::from("/scope-a/a.bin"),
                    object: Some(ObjectRef {
                        volume: 1,
                        file_id: 10,
                    }),
                },
                MemberRef {
                    entry_id: 2,
                    path: PathBuf::from("/scope-a/b.bin"),
                    object: Some(ObjectRef {
                        volume: 1,
                        file_id: 11,
                    }),
                },
            ],
            distinct_objects: Some(2),
            evidence: vec![],
            content: Some(ContentRef {
                sha256_hex: "deadbeef".to_string(),
                algorithm: "sha256".to_string(),
            }),
            object: None,
            alias_sets: vec![],
            logical_duplicate_bytes: 10,
            recoverable_bytes: Some(10),
            accounting: StorageAccounting::Exact,
            detail_truncated: false,
        }],
        relationships_truncated: 0,
        undetermined: Undetermined {
            failed: 0,
            not_examined: 0,
            failed_by_reason: vec![],
            detail: vec![],
            detail_truncated: 0,
        },
        stats: RelationshipStats::default(),
        started_at: UNIX_EPOCH,
        finished_at: UNIX_EPOCH,
    };

    store.commit_run(&record, &snapshot, Some(&report)).unwrap();

    let history = store
        .relationship_history("content-deadbeef", &QueryLimits::default())
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].run_id, record.run_id);
    assert_eq!(history[0].kind, "CONTENT_DUPLICATE");
    assert_eq!(history[0].size, 10);
    assert_eq!(history[0].member_count, 2);
    assert_eq!(history[0].recoverable, Some(10));
    assert_eq!(history[0].accounting, "EXACT");
    assert_eq!(history[0].stored_member_count, 2);

    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    let relationships = loaded.relationships.expect("relationships were committed");
    assert_eq!(relationships.relationships.len(), 1);
    let rel = &relationships.relationships[0];
    assert_eq!(rel.id, "content-deadbeef");
    assert_eq!(rel.member_count, 2);
    let member_paths: Vec<String> = rel
        .members
        .iter()
        .map(|m| m.path.to_string_lossy().to_string())
        .collect();
    assert_eq!(member_paths, vec!["/scope-a/a.bin", "/scope-a/b.bin"]);
    assert_eq!(
        rel.content.as_ref().map(|c| c.sha256_hex.as_str()),
        Some("deadbeef")
    );
}

#[test]
fn paths_are_private_and_schema_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");

    let record = run_record("stable-run", 1, RunStatus::Completed, &["/scope-a"]);
    {
        let mut store = HistoryStore::open(&path).unwrap();
        store.begin_run(&record).unwrap();
        let snapshot = snapshot_of(
            record.run_id.clone(),
            &[fs_entry(
                "/scope-a/private/file.bin",
                EntryKind::File,
                3,
                Some((1, 90)),
            )],
        );
        store.commit_run(&record, &snapshot, None).unwrap();
    }

    // First reopen: no errors, schema stable, data intact.
    {
        let store = HistoryStore::open(&path).unwrap();
        let runs = store.list_runs(&QueryLimits::default()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, record.run_id);
        assert_eq!(runs[0].status, RunStatus::Completed);
        let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
        assert_eq!(loaded.snapshot.entries.len(), 1);
        assert_eq!(
            loaded.snapshot.entries[0].path,
            PathBuf::from("/scope-a/private/file.bin")
        );
    }

    // Second reopen: idempotent — still no errors, data intact.
    {
        let store = HistoryStore::open(&path).unwrap();
        assert_eq!(store.list_runs(&QueryLimits::default()).unwrap().len(), 1);
        let run = store.get_run(&record.run_id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Completed);
    }
}
