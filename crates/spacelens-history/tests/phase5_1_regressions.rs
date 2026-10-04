//! Phase 5.1 regression tests — every defect from the Phase 5.1 audit,
//! written to compile against BOTH the pre-repair and post-repair APIs
//! (inputs flow through the stable `FsEntry`/`SnapshotBuilder` surface and
//! assertions target change counts, event kinds, paths, and raw SQLite
//! columns), so the pre-repair failures are reproducible evidence
//! (Objective 9) and the post-repair passes are durable guards.
//!
//! Findings covered:
//! - F1  `file_id_hi` dropped by the history model, builder, and SQLite.
//! - F2  `Modified` emitted without same-object proof.
//! - F3  hard-link alias path deletion swallowed by object survival.
//! - F4  fabricated pairwise move mappings for multi-path objects.
//! - F5  lossy (`to_string_lossy`) path persistence corrupting non-UTF-8
//!   paths.
//! - F6  scope comparison ignoring Windows case-insensitivity.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::Connection;
use spacelens_engine::{EntryKind, FsEntry};
use spacelens_history::{
    compare, path_covers, CompareOptions, ConfigFingerprint, EventEvidence, EventKind,
    HistoryStore, QueryLimits, RunCounts, RunId, RunRecord, RunSnapshot, RunStatus, Snapshot,
    SnapshotBuilder,
};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn run_record(id: &str, roots: &[&str]) -> RunRecord {
    RunRecord {
        run_id: RunId(id.to_string()),
        started_at: UNIX_EPOCH,
        completed_at: Some(UNIX_EPOCH + Duration::from_secs(60)),
        roots: roots.iter().map(PathBuf::from).collect(),
        platform: "test/test".into(),
        config: ConfigFingerprint::current(),
        status: RunStatus::Completed,
        counts: RunCounts::default(),
    }
}

fn fs_entry(path: &str, size: u64, object: Option<(u64, u64)>, hi: Option<u64>) -> FsEntry {
    let (device, inode) = match object {
        Some((d, i)) => (Some(d), Some(i)),
        None => (None, None),
    };
    FsEntry {
        id: 0,
        parent_id: None,
        path: PathBuf::from(path),
        kind: EntryKind::File,
        size,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device,
        inode,
        file_id_hi: hi,
        hidden: false,
        error: None,
    }
}

fn snap(run_id: &str, entries: Vec<FsEntry>, content: &[(&str, &str)]) -> Snapshot {
    let mut builder = SnapshotBuilder::new();
    for e in &entries {
        builder.push_entry(e, None);
    }
    for (p, c) in content {
        builder.set_content(Path::new(p), c.to_string());
    }
    builder.build(RunId(run_id.to_string())).unwrap()
}

/// Compare two completed runs over `/scope-a`.
fn cmp(
    from: (Vec<FsEntry>, Vec<(&str, &str)>),
    to: (Vec<FsEntry>, Vec<(&str, &str)>),
) -> spacelens_history::ChangeSet {
    let a = RunSnapshot::new(
        run_record("reg-a", &["/scope-a"]),
        snap("reg-a", from.0, &from.1),
    );
    let b = RunSnapshot::new(
        run_record("reg-b", &["/scope-a"]),
        snap("reg-b", to.0, &to.1),
    );
    compare(&a, &b, &CompareOptions::default()).unwrap()
}

fn events_of_kind(
    cs: &spacelens_history::ChangeSet,
    kind: EventKind,
) -> Vec<spacelens_history::ChangeEvent> {
    cs.events
        .iter()
        .filter(|e| e.kind == kind)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// F1 — full Phase 3.2 Windows identity (file_id_hi) must survive
// ---------------------------------------------------------------------------

#[test]
fn f1_snapshot_builder_preserves_file_id_hi_in_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("history.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let record = run_record("f1-run", &["/scope-a"]);
    store.begin_run(&record).unwrap();
    // A ReFS-class entry: 128-bit file id, high bits proven non-zero.
    let entry = fs_entry(
        "/scope-a/refs.bin",
        10,
        Some((1, 5)),
        Some(0x00AA_BBCC_DD00_1122),
    );
    let snapshot = snap("f1-run", vec![entry], &[]);
    store.commit_run(&record, &snapshot, None).unwrap();

    // Raw column introspection: the schema must carry the high bits, and
    // the committed value must equal the proven component.
    let conn = Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('observations')")
        .unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    let has_hi = cols.iter().any(|c| c == "file_id_hi");
    assert!(
        has_hi,
        "observations must persist file_id_hi (got columns {cols:?})"
    );
    let stored: Option<i64> = conn
        .query_row(
            "SELECT file_id_hi FROM observations WHERE run_id = 'f1-run'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        stored,
        Some(0x00AA_BBCC_DD00_1122_i64),
        "the proven file id high bits must round-trip through SQLite"
    );
}

#[test]
fn f1_wide_file_id_participates_in_identity_comparison() {
    // Same path, same (device, inode), DIFFERENT proven high bits:
    // two different objects (128-bit ids differ) — a replacement, not
    // silence.
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 10, Some((1, 5)), Some(7))],
            vec![],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 10, Some((1, 5)), Some(9))],
            vec![],
        ),
    );
    assert_eq!(
        cs.counts.object_identity_changed, 1,
        "differing wide file ids prove different objects: {cs:?}"
    );
}

#[test]
fn f1_mixed_wide_id_provability_is_unknown_never_equal() {
    // One side proved wide bits, the other did not: identity is UNKNOWN —
    // neither a replacement claim nor a modification claim may be made.
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 10, Some((1, 5)), Some(7))],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 10, Some((1, 5)), None)],
            vec![("/scope-a/f.bin", HASH_B)],
        ),
    );
    assert_eq!(
        cs.counts.modified, 0,
        "unknown identity never proves modification: {cs:?}"
    );
    assert_eq!(
        cs.counts.object_identity_changed, 0,
        "mixed provability never proves replacement: {cs:?}"
    );
}

#[test]
fn f1_mixed_wide_id_provability_blocks_move_continuity() {
    // Object observed at A with wide bits in from-run, and at B without
    // them in to-run: continuity is UNKNOWN — no move claim; honest
    // path-level facts only.
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/a.bin", 10, Some((1, 5)), Some(7))],
            vec![],
        ),
        (
            vec![fs_entry("/scope-a/b.bin", 10, Some((1, 5)), None)],
            vec![],
        ),
    );
    assert_eq!(
        cs.counts.moved, 0,
        "unprovable continuity must not claim a move: {cs:?}"
    );
    assert_eq!(cs.counts.renamed, 0);
    assert_eq!(cs.counts.deleted, 1);
    assert_eq!(cs.counts.created, 1);
}

// ---------------------------------------------------------------------------
// F2 — Modified requires proven same-object continuity
// ---------------------------------------------------------------------------

#[test]
fn f2_no_modified_without_any_object_proof() {
    // Both sides unprovable: content difference is historical uncertainty,
    // never a modification of "the same object".
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 50, None, None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 50, None, None)],
            vec![("/scope-a/f.bin", HASH_B)],
        ),
    );
    assert_eq!(
        cs.counts.modified, 0,
        "no object proof — no Modified: {cs:?}"
    );
}

#[test]
fn f2_no_modified_when_only_one_side_proves_the_object() {
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 50, None, None)],
            vec![("/scope-a/f.bin", HASH_B)],
        ),
    );
    assert_eq!(
        cs.counts.modified, 0,
        "one-sided identity is unknown: {cs:?}"
    );
    assert_eq!(cs.counts.object_identity_changed, 0);
}

#[test]
fn f2_same_object_two_verified_differing_hashes_is_modified() {
    // The one legitimate Modified: proven equal object + two verified
    // hashes + they differ — and the event must carry the same-object
    // proof in its evidence.
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_B)],
        ),
    );
    assert_eq!(cs.counts.modified, 1);
    let m = &events_of_kind(&cs, EventKind::Modified)[0];
    assert!(m.evidence.contains(&EventEvidence::ObjectIdentityEqual));
    assert!(m
        .evidence
        .contains(&EventEvidence::ContentIdentityDiffering));
}

#[test]
fn f2_same_object_one_hash_only_is_never_modified() {
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![],
        ),
    );
    assert_eq!(
        cs.counts.modified, 0,
        "one hash cannot prove a change: {cs:?}"
    );
}

#[test]
fn f2_same_object_equal_hashes_emit_nothing() {
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
    );
    assert!(cs.events.is_empty(), "{cs:?}");
}

#[test]
fn f2_different_object_differing_hashes_is_replacement_not_modified() {
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 10)), None)],
            vec![("/scope-a/f.bin", HASH_A)],
        ),
        (
            vec![fs_entry("/scope-a/f.bin", 50, Some((1, 11)), None)],
            vec![("/scope-a/f.bin", HASH_B)],
        ),
    );
    assert_eq!(cs.counts.object_identity_changed, 1);
    assert_eq!(cs.counts.modified, 0);
}

// ---------------------------------------------------------------------------
// F3 — path deletion is independent of object survival
// ---------------------------------------------------------------------------

#[test]
fn f3_alias_path_deleted_while_object_survives_through_other_alias() {
    // Run A: A.txt and B.txt are hard links to object X.
    // Run B: only A.txt remains. The PATH B.txt was removed — that is a
    // path-level deletion even though object X survives.
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/A.txt", 10, Some((9, 9)), None),
                fs_entry("/scope-a/B.txt", 10, Some((9, 9)), None),
            ],
            vec![],
        ),
        (
            vec![fs_entry("/scope-a/A.txt", 10, Some((9, 9)), None)],
            vec![],
        ),
    );
    let deleted = events_of_kind(&cs, EventKind::Deleted);
    assert_eq!(deleted.len(), 1, "exactly one deleted path event: {cs:?}");
    assert_eq!(deleted[0].path, PathBuf::from("/scope-a/B.txt"));
    assert_eq!(cs.counts.created, 0);
    assert_eq!(cs.counts.moved, 0);
    // The surviving alias produced no churn.
    assert_eq!(cs.counts.size_changed, 0);
}

#[test]
fn f3_all_aliases_removed_each_path_deleted() {
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/A.txt", 10, Some((9, 9)), None),
                fs_entry("/scope-a/B.txt", 10, Some((9, 9)), None),
            ],
            vec![],
        ),
        (vec![], vec![]),
    );
    let deleted = events_of_kind(&cs, EventKind::Deleted);
    let mut paths: Vec<PathBuf> = deleted.iter().map(|e| e.path.clone()).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("/scope-a/A.txt"),
            PathBuf::from("/scope-a/B.txt")
        ],
        "every removed alias path receives its path-level deletion: {cs:?}"
    );
    assert_eq!(cs.counts.created, 0);
}

#[test]
fn f3_one_alias_survives_another_moves_away() {
    // A.txt and B.txt alias object X in from-run; in to-run A.txt remains
    // and C.txt is a NEW alias. B.txt was removed; C.txt was added.
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/A.txt", 10, Some((9, 9)), None),
                fs_entry("/scope-a/B.txt", 10, Some((9, 9)), None),
            ],
            vec![],
        ),
        (
            vec![
                fs_entry("/scope-a/A.txt", 10, Some((9, 9)), None),
                fs_entry("/scope-a/C.txt", 10, Some((9, 9)), None),
            ],
            vec![],
        ),
    );
    let deleted = events_of_kind(&cs, EventKind::Deleted);
    assert_eq!(deleted.len(), 1, "{cs:?}");
    assert_eq!(deleted[0].path, PathBuf::from("/scope-a/B.txt"));
    let created = events_of_kind(&cs, EventKind::Created);
    assert_eq!(created.len(), 1, "{cs:?}");
    assert_eq!(created[0].path, PathBuf::from("/scope-a/C.txt"));
    // Object continuity is carried as evidence, not as a fabricated move.
    assert!(created[0]
        .evidence
        .contains(&EventEvidence::ObjectIdentityEqual));
    assert_eq!(cs.counts.moved, 0);
    assert_eq!(cs.counts.renamed, 0);
}

#[test]
fn f3_single_alias_move_remains_a_move() {
    // Guard: the provable 1:1 relocation stays a move (never delete+create
    // churn).
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/old/cat.jpg", 99, Some((1, 7)), None)],
            vec![],
        ),
        (
            vec![fs_entry("/scope-a/new/cat.jpg", 99, Some((1, 7)), None)],
            vec![],
        ),
    );
    assert_eq!(cs.counts.moved, 1);
    assert_eq!(cs.counts.deleted, 0);
    assert_eq!(cs.counts.created, 0);
    let mv = &events_of_kind(&cs, EventKind::Moved)[0];
    assert_eq!(
        mv.previous_path,
        Some(PathBuf::from("/scope-a/old/cat.jpg"))
    );
}

#[test]
fn f3_deletion_invariant_every_deleted_path_absent_in_to() {
    // Global invariant across a mixed scenario.
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/keep.bin", 10, Some((1, 1)), None),
                fs_entry("/scope-a/gone-alias.bin", 10, Some((1, 2)), None),
                fs_entry("/scope-a/twin-alias.bin", 10, Some((1, 2)), None),
                fs_entry("/scope-a/no-id.bin", 10, None, None),
            ],
            vec![],
        ),
        (
            vec![
                fs_entry("/scope-a/keep.bin", 10, Some((1, 1)), None),
                fs_entry("/scope-a/fresh.bin", 10, Some((1, 9)), None),
            ],
            vec![],
        ),
    );
    for e in events_of_kind(&cs, EventKind::Deleted) {
        assert!(e.path != *"/scope-a/keep.bin");
        assert!(e.path != *"/scope-a/fresh.bin");
    }
    let deleted_paths: Vec<PathBuf> = events_of_kind(&cs, EventKind::Deleted)
        .iter()
        .map(|e| e.path.clone())
        .collect();
    assert!(deleted_paths.contains(&PathBuf::from("/scope-a/gone-alias.bin")));
    assert!(deleted_paths.contains(&PathBuf::from("/scope-a/twin-alias.bin")));
    assert!(deleted_paths.contains(&PathBuf::from("/scope-a/no-id.bin")));
}

// ---------------------------------------------------------------------------
// F4 — multi-path object relocations never fabricate pairings
// ---------------------------------------------------------------------------

#[test]
fn f4_two_old_two_new_no_fabricated_pairings() {
    // A, B → C, D for one object: exact pairing is unprovable.
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/A.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/B.bin", 10, Some((5, 5)), None),
            ],
            vec![],
        ),
        (
            vec![
                fs_entry("/scope-a/C.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/D.bin", 10, Some((5, 5)), None),
            ],
            vec![],
        ),
    );
    assert_eq!(cs.counts.moved, 0, "no fabricated pairwise mapping: {cs:?}");
    assert_eq!(cs.counts.renamed, 0);
    assert_eq!(cs.counts.deleted, 2, "old alias paths removed: {cs:?}");
    assert_eq!(cs.counts.created, 2);
    // No event may carry a previous-path pairing.
    assert!(
        cs.events.iter().all(|e| e.previous_path.is_none()),
        "no old→new pairing may be claimed: {cs:?}"
    );
    // Object continuity survives on the created events as evidence.
    for e in events_of_kind(&cs, EventKind::Created) {
        assert!(e.evidence.contains(&EventEvidence::ObjectIdentityEqual));
    }
}

#[test]
fn f4_two_old_one_new_no_fabricated_mapping() {
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/A.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/B.bin", 10, Some((5, 5)), None),
            ],
            vec![],
        ),
        (
            vec![fs_entry("/scope-a/C.bin", 10, Some((5, 5)), None)],
            vec![],
        ),
    );
    assert_eq!(cs.counts.moved, 0, "{cs:?}");
    assert_eq!(cs.counts.deleted, 2);
    assert_eq!(cs.counts.created, 1);
    assert!(cs.events.iter().all(|e| e.previous_path.is_none()));
}

#[test]
fn f4_one_old_two_new_no_duplicate_movement() {
    let cs = cmp(
        (
            vec![fs_entry("/scope-a/A.bin", 10, Some((5, 5)), None)],
            vec![],
        ),
        (
            vec![
                fs_entry("/scope-a/B.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/C.bin", 10, Some((5, 5)), None),
            ],
            vec![],
        ),
    );
    assert_eq!(
        cs.counts.moved, 0,
        "one old path must never map to two new paths: {cs:?}"
    );
    assert_eq!(cs.counts.renamed, 0);
    assert_eq!(cs.counts.deleted, 1);
    assert_eq!(cs.counts.created, 2);
}

#[test]
fn f4_three_old_three_new_no_fabricated_pairings() {
    let cs = cmp(
        (
            vec![
                fs_entry("/scope-a/A1.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/A2.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/A3.bin", 10, Some((5, 5)), None),
            ],
            vec![],
        ),
        (
            vec![
                fs_entry("/scope-a/B1.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/B2.bin", 10, Some((5, 5)), None),
                fs_entry("/scope-a/B3.bin", 10, Some((5, 5)), None),
            ],
            vec![],
        ),
    );
    assert_eq!(cs.counts.moved, 0, "{cs:?}");
    assert_eq!(cs.counts.renamed, 0);
    assert_eq!(cs.counts.deleted, 3);
    assert_eq!(cs.counts.created, 3);
}

// ---------------------------------------------------------------------------
// F5 — lossless path persistence
// ---------------------------------------------------------------------------

/// A path that is NOT valid UTF-8 on this platform (Unix: invalid bytes;
/// Windows: an unpaired UTF-16 surrogate, the only non-UTF-8 `OsStr`
/// representable on the platform).
#[cfg(any(unix, windows))]
fn non_utf8_path() -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(vec![b'/', b'x', 0xff, 0xfe, b'y']))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xD800,
        ]))
    }
}

#[cfg(any(unix, windows))]
#[test]
fn f5_non_utf8_path_round_trips_exactly_through_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("history.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let path = non_utf8_path();
    assert!(path.to_str().is_none(), "fixture must be non-UTF-8");

    let record = run_record("f5-run", &["/scope-a"]);
    store.begin_run(&record).unwrap();
    let mut entry = fs_entry("", 10, Some((1, 3)), None);
    entry.path = path.clone();
    let snapshot = snap("f5-run", vec![entry], &[]);
    store.commit_run(&record, &snapshot, None).unwrap();

    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    assert_eq!(
        loaded.snapshot.entries[0].path, path,
        "store(path) → load(path) must be exact for every supported path"
    );

    // The path-history lookup must also find the non-UTF-8 path.
    let points = store
        .history_for_path(&path, &QueryLimits::default())
        .unwrap();
    assert_eq!(
        points.len(),
        1,
        "history_for_path must find non-UTF-8 paths"
    );
}

#[test]
fn f5_utf8_path_variety_round_trips_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("history.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let paths: Vec<PathBuf> = vec![
        PathBuf::from("/scope-a/plain.bin"),
        PathBuf::from("/scope-a/with spaces and  tabs.bin"),
        PathBuf::from("/scope-a/üñïçø∂é-🎉.bin"),
        PathBuf::from("/scope-a/日本語/файл.bin"),
        PathBuf::from(&format!("/scope-a/{}.bin", "x".repeat(1200))),
    ];

    let record = run_record("f5-utf8-run", &["/scope-a"]);
    store.begin_run(&record).unwrap();
    let entries: Vec<FsEntry> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut e = fs_entry("", i as u64, Some((1, i as u64)), None);
            e.path = p.clone();
            e
        })
        .collect();
    let snapshot = snap("f5-utf8-run", entries, &[]);
    store.commit_run(&record, &snapshot, None).unwrap();

    let loaded = store.load_run_snapshot(&record.run_id).unwrap().unwrap();
    let mut loaded_paths: Vec<PathBuf> = loaded
        .snapshot
        .entries
        .iter()
        .map(|e| e.path.clone())
        .collect();
    loaded_paths.sort();
    let mut expected = paths;
    expected.sort();
    assert_eq!(
        loaded_paths, expected,
        "every UTF-8 path must round-trip exactly"
    );
}

// ---------------------------------------------------------------------------
// F6 — platform-aware scope (path_covers) comparison semantics
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn f6_windows_scope_comparison_is_case_insensitive() {
    assert!(
        path_covers(Path::new(r"C:\Users"), Path::new(r"c:\users\Huzzi")),
        "Windows-style components must compare case-insensitively"
    );
    assert!(path_covers(
        Path::new(r"C:\Users"),
        Path::new(r"C:\USERS\Huzzi")
    ));
    assert!(path_covers(
        Path::new(r"C:\Users\Huzzi"),
        Path::new(r"C:\Users\Huzzi\Documents")
    ));
}

#[cfg(windows)]
#[test]
fn f6_windows_run_scope_recognizes_case_variants() {
    let run = run_record("f6-run", &[r"C:\Users"]);
    assert!(run.covers(&[PathBuf::from(r"c:\Users\Huzzi")]));
    assert!(run.covers(&[PathBuf::from(r"C:\USERS")]));
}

#[cfg(unix)]
#[test]
fn f6_unix_scope_comparison_stays_case_sensitive() {
    assert!(!path_covers(Path::new("/Data"), Path::new("/data/x")));
    assert!(path_covers(Path::new("/Data"), Path::new("/Data/x")));
    assert!(!path_covers(Path::new("/DATA"), Path::new("/Data")));
}

#[test]
fn f6_component_wise_comparison_never_string_prefix_matches() {
    // C:\A must NOT cover C:\AB (no string-prefix false match), everywhere
    // (on Unix these parse as single relative components, which also
    // differ).
    assert!(!path_covers(Path::new(r"C:\A"), Path::new(r"C:\AB")));
    assert!(path_covers(Path::new(r"C:\A"), Path::new(r"C:\A\child")));
    // Direction matters.
    assert!(!path_covers(Path::new(r"C:\A\child"), Path::new(r"C:\A")));
}

// ---------------------------------------------------------------------------
// Objective 11 — global invariants over a mixed adversarial scenario
// ---------------------------------------------------------------------------

#[test]
fn invariants_hold_over_mixed_scenario() {
    // One comparison exercising every event family at once.
    let cs = cmp(
        (
            vec![
                // modified: same object, two hashes, differ
                fs_entry("/scope-a/mod.bin", 10, Some((1, 1)), None),
                // replaced: same path, different object
                fs_entry("/scope-a/repl.bin", 10, Some((1, 2)), None),
                // alias pair, one removed, one kept
                fs_entry("/scope-a/keep-alias.bin", 10, Some((1, 3)), None),
                fs_entry("/scope-a/gone-alias.bin", 10, Some((1, 3)), None),
                // relocated 1:1
                fs_entry("/scope-a/old-mv.bin", 10, Some((1, 4)), None),
                // ambiguous relocation 2→2
                fs_entry("/scope-a/amb1.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/amb2.bin", 10, Some((1, 5)), None),
                // unprovable identity, content changed
                fs_entry("/scope-a/unknown.bin", 10, None, None),
                // size change, same object
                fs_entry("/scope-a/grow.bin", 10, Some((1, 6)), None),
            ],
            vec![
                ("/scope-a/mod.bin", HASH_A),
                ("/scope-a/repl.bin", HASH_A),
                ("/scope-a/unknown.bin", HASH_A),
            ],
        ),
        (
            vec![
                fs_entry("/scope-a/mod.bin", 10, Some((1, 1)), None),
                fs_entry("/scope-a/repl.bin", 10, Some((9, 9)), None),
                fs_entry("/scope-a/keep-alias.bin", 10, Some((1, 3)), None),
                fs_entry("/scope-a/new-mv.bin", 10, Some((1, 4)), None),
                fs_entry("/scope-a/amb-n1.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/amb-n2.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/unknown.bin", 10, None, None),
                fs_entry("/scope-a/grow.bin", 900, Some((1, 6)), None),
                fs_entry("/scope-a/fresh.bin", 10, Some((1, 7)), None),
            ],
            vec![
                ("/scope-a/mod.bin", HASH_B),
                ("/scope-a/repl.bin", HASH_B),
                ("/scope-a/unknown.bin", HASH_B),
            ],
        ),
    );

    // Modified ⇔ proven-equal object + both hashes verified + differing.
    for e in events_of_kind(&cs, EventKind::Modified) {
        assert!(e.evidence.contains(&EventEvidence::ObjectIdentityEqual));
        assert!(e
            .evidence
            .contains(&EventEvidence::ContentIdentityDiffering));
    }
    assert_eq!(cs.counts.modified, 1);

    // Deleted ⇔ path in from-run and absent in to-run.
    let from_paths: Vec<PathBuf> = vec![
        "/scope-a/mod.bin",
        "/scope-a/repl.bin",
        "/scope-a/keep-alias.bin",
        "/scope-a/gone-alias.bin",
        "/scope-a/old-mv.bin",
        "/scope-a/amb1.bin",
        "/scope-a/amb2.bin",
        "/scope-a/unknown.bin",
        "/scope-a/grow.bin",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    let to_paths: Vec<PathBuf> = vec![
        "/scope-a/mod.bin",
        "/scope-a/repl.bin",
        "/scope-a/keep-alias.bin",
        "/scope-a/new-mv.bin",
        "/scope-a/amb-n1.bin",
        "/scope-a/amb-n2.bin",
        "/scope-a/unknown.bin",
        "/scope-a/grow.bin",
        "/scope-a/fresh.bin",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    for e in events_of_kind(&cs, EventKind::Deleted) {
        assert!(
            from_paths.contains(&e.path),
            "deleted path must come from from-run"
        );
        assert!(
            !to_paths.contains(&e.path),
            "deleted path must be absent in to-run"
        );
    }

    // Moves: at most one event per old path and per new path (1:1 only).
    let mut previous_paths: Vec<PathBuf> = events_of_kind(&cs, EventKind::Moved)
        .iter()
        .chain(events_of_kind(&cs, EventKind::Renamed).iter())
        .filter_map(|e| e.previous_path.clone())
        .collect();
    previous_paths.sort();
    let mut previous_dedup = previous_paths.clone();
    previous_dedup.dedup();
    assert_eq!(previous_paths, previous_dedup, "no old path may move twice");

    // Every event carries non-empty canonical evidence.
    for e in &cs.events {
        assert!(!e.evidence.is_empty());
        let mut sorted = e.evidence.clone();
        sorted.sort();
        assert_eq!(e.evidence, sorted);
    }

    // Determinism: recomputation is identical.
    let cs2 = cmp(
        (
            vec![
                fs_entry("/scope-a/mod.bin", 10, Some((1, 1)), None),
                fs_entry("/scope-a/repl.bin", 10, Some((1, 2)), None),
                fs_entry("/scope-a/keep-alias.bin", 10, Some((1, 3)), None),
                fs_entry("/scope-a/gone-alias.bin", 10, Some((1, 3)), None),
                fs_entry("/scope-a/old-mv.bin", 10, Some((1, 4)), None),
                fs_entry("/scope-a/amb1.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/amb2.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/unknown.bin", 10, None, None),
                fs_entry("/scope-a/grow.bin", 10, Some((1, 6)), None),
            ],
            vec![
                ("/scope-a/mod.bin", HASH_A),
                ("/scope-a/repl.bin", HASH_A),
                ("/scope-a/unknown.bin", HASH_A),
            ],
        ),
        (
            vec![
                fs_entry("/scope-a/mod.bin", 10, Some((1, 1)), None),
                fs_entry("/scope-a/repl.bin", 10, Some((9, 9)), None),
                fs_entry("/scope-a/keep-alias.bin", 10, Some((1, 3)), None),
                fs_entry("/scope-a/new-mv.bin", 10, Some((1, 4)), None),
                fs_entry("/scope-a/amb-n1.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/amb-n2.bin", 10, Some((1, 5)), None),
                fs_entry("/scope-a/unknown.bin", 10, None, None),
                fs_entry("/scope-a/grow.bin", 900, Some((1, 6)), None),
                fs_entry("/scope-a/fresh.bin", 10, Some((1, 7)), None),
            ],
            vec![
                ("/scope-a/mod.bin", HASH_B),
                ("/scope-a/repl.bin", HASH_B),
                ("/scope-a/unknown.bin", HASH_B),
            ],
        ),
    );
    assert_eq!(cs, cs2);
}
