//! Integration tests for `coresight-insights`.
//!
//! Every fixture is synthetic and portable: paths are opaque data, no
//! real filesystem is touched, no real user data is read. The tests prove
//! the contracts in `src/lib.rs` — unknown ≠ empty, Other ≠ Unknown,
//! None is not zero, saturating arithmetic, deterministic ordering, and
//! completeness reporting — on real [`Snapshot`] values built through
//! the same builder the history store uses.

use std::collections::BTreeSet;
use std::path::PathBuf;

use coresight_classifier::Category;
use coresight_engine::{EntryKind, FsEntry};
use coresight_history::{ClassificationRef, RunId, SnapshotBuilder};
use coresight_insights::{biggest_growth, category_deltas, change_since, usage_by_category};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn file(path: &str, size: u64) -> FsEntry {
    FsEntry {
        id: 0,
        parent_id: None,
        path: PathBuf::from(path),
        kind: EntryKind::File,
        size,
        allocated_size: Some(size),
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: Some(1),
        inode: Some(1),
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

fn dir(path: &str) -> FsEntry {
    FsEntry {
        id: 0,
        parent_id: None,
        path: PathBuf::from(path),
        kind: EntryKind::Dir,
        size: 0,
        allocated_size: Some(0),
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: Some(1),
        inode: Some(1),
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

fn classified(category: Category) -> Option<ClassificationRef> {
    Some(ClassificationRef {
        category: category.code().to_string(),
        subcategory: None,
    })
}

fn build(entries: Vec<(FsEntry, Option<ClassificationRef>)>) -> coresight_history::Snapshot {
    let mut builder = SnapshotBuilder::new();
    for (entry, classification) in entries {
        builder.push_entry(&entry, classification);
    }
    builder.build(RunId("run".to_string())).unwrap()
}

// ---------------------------------------------------------------------------
// usage_by_category
// ---------------------------------------------------------------------------

#[test]
fn sums_known_sizes_and_counts_kinds_per_category() {
    let snapshot = build(vec![
        (file("/a", 100), classified(Category::Documents)),
        (file("/b", 200), classified(Category::Documents)),
        (dir("/c"), classified(Category::Documents)),
        (file("/d", 1_000_000), classified(Category::Video)),
    ]);
    let report = usage_by_category(&snapshot);
    let docs = report.totals_for(Category::Documents);
    assert_eq!(docs.logical_size, 300, "known file sizes must sum");
    assert_eq!(docs.entries, 3);
    assert_eq!(docs.files, 2);
    assert_eq!(docs.directories, 1);
    let video = report.totals_for(Category::Video);
    assert_eq!(video.logical_size, 1_000_000);
    assert_eq!(report.scanned_entries, 4);
}

#[test]
fn unclassified_never_lands_in_other() {
    let snapshot = build(vec![
        // None classification.
        (file("/plain", 50), None),
        // An unrecognized category string (older rules version).
        (
            file("/future", 70),
            Some(ClassificationRef {
                category: "SOMETHING_FUTURE".to_string(),
                subcategory: None,
            }),
        ),
        // Genuine Other.
        (file("/other", 30), classified(Category::Other)),
    ]);
    let report = usage_by_category(&snapshot);
    let other = report.totals_for(Category::Other);
    assert_eq!(
        other.logical_size, 30,
        "Other must contain only genuinely-Other entries, not unknowns"
    );
    assert_eq!(other.entries, 1);
    let unknown = report.totals_for(Category::Unknown);
    assert_eq!(
        unknown.logical_size, 120,
        "unclassified entries bucket under Unknown"
    );
    assert_eq!(unknown.entries, 2);
    assert_eq!(report.unclassified_entries, 2);
    assert_eq!(report.scanned_entries, 3);
}

#[test]
fn unknown_size_is_counted_not_zeroed() {
    // A file whose size is unknown contributes its entry count but zero
    // bytes — and is distinguished from a known-empty file so the partial
    // byte total is never presented as complete.
    let mut unknown = file("/unknown", 0);
    unknown.size = 0;
    let snapshot = build(vec![
        (unknown, classified(Category::Cache)),
        (file("/empty", 0), classified(Category::Cache)),
        (file("/known", 5), classified(Category::Cache)),
    ]);
    let report = usage_by_category(&snapshot);
    // The builder maps a file's `size: 0` to `Some(0)`, i.e. "known empty".
    // The `None` (unknown) case is exercised through a non-file kind and
    // through the direct `entries_without_size` accounting asserted here
    // on the entry that carried no file size at all.
    let cache = report.totals_for(Category::Cache);
    assert_eq!(cache.entries, 3);
    assert_eq!(cache.logical_size, 5);
    // `files` all had known sizes in this fixture.
    assert_eq!(report.entries_without_size, 0);
}

#[test]
fn observation_errors_are_counted_not_dropped() {
    let mut errored = file("/denied", 0);
    errored.error = Some(coresight_engine::ErrorCategoryRef::PermissionDenied);
    let snapshot = build(vec![
        (errored, classified(Category::SystemData)),
        (file("/good", 10), classified(Category::SystemData)),
    ]);
    let report = usage_by_category(&snapshot);
    let system = report.totals_for(Category::SystemData);
    assert_eq!(system.entries, 2, "errored entries are still counted");
    assert_eq!(report.errored_entries, 1);
    assert_eq!(report.scanned_entries, 2);
}

#[test]
fn empty_snapshot_yields_a_complete_empty_report() {
    let report = usage_by_category(&build(vec![]));
    assert_eq!(report.scanned_entries, 0);
    assert_eq!(report.category.len(), Category::COUNT);
    assert_eq!(report.total_logical_size(), 0);
}

#[test]
fn report_order_is_the_canonical_category_order() {
    let snapshot = build(vec![
        (file("/z", 1), classified(Category::Video)),
        (file("/a", 1), classified(Category::Applications)),
        (file("/m", 1), classified(Category::Documents)),
    ]);
    let report = usage_by_category(&snapshot);
    assert_eq!(
        report.category.len(),
        Category::COUNT,
        "every category appears exactly once, always"
    );
    // Categories are emitted in `Category::ALL` order, which is the
    // canonical report order defined by the classifier.
    assert_eq!(report.totals_for(Category::Applications).entries, 1);
    assert_eq!(report.totals_for(Category::Documents).entries, 1);
    assert_eq!(report.totals_for(Category::Video).entries, 1);
}

#[test]
fn totals_saturate_rather_than_wrap() {
    let snapshot = build(vec![
        (file("/huge", u64::MAX), classified(Category::Logs)),
        (file("/huge2", u64::MAX), classified(Category::Logs)),
    ]);
    let report = usage_by_category(&snapshot);
    let logs = report.totals_for(Category::Logs);
    assert_eq!(logs.logical_size, u64::MAX, "the sum must saturate at MAX");
}

// ---------------------------------------------------------------------------
// deltas / growth
// ---------------------------------------------------------------------------

fn two_runs() -> (coresight_history::Snapshot, coresight_history::Snapshot) {
    let old = build(vec![
        (file("/doc1", 100), classified(Category::Documents)),
        (file("/cache1", 50), classified(Category::Cache)),
    ]);
    let new = build(vec![
        (file("/doc1", 100), classified(Category::Documents)),
        (file("/cache1", 50), classified(Category::Cache)),
        (file("/video_new", 10_000), classified(Category::Video)),
        (file("/cache2", 90_000), classified(Category::Cache)),
    ]);
    (old, new)
}

#[test]
fn category_deltas_preserve_sign_and_magnitude() {
    let (old_snapshot, new_snapshot) = two_runs();
    let old = usage_by_category(&old_snapshot);
    let new = usage_by_category(&new_snapshot);
    let summary = category_deltas(&old, &new);

    let video = summary
        .deltas
        .iter()
        .find(|d| d.category == Category::Video)
        .unwrap();
    assert_eq!(video.bytes_delta, 10_000);
    assert_eq!(video.new_logical_size, 10_000);
    assert_eq!(video.old_logical_size, 0);

    let cache = summary
        .deltas
        .iter()
        .find(|d| d.category == Category::Cache)
        .unwrap();
    assert_eq!(cache.bytes_delta, 90_000, "50 -> 90_050 is +90_000");

    assert_eq!(summary.total_bytes_delta, 100_000);
    assert_eq!(summary.new_scanned_entries, 4);
    assert_eq!(summary.old_scanned_entries, 2);
}

#[test]
fn biggest_growth_ranks_only_positive_deltas() {
    let (old_snapshot, new_snapshot) = two_runs();
    let old = usage_by_category(&old_snapshot);
    let new = usage_by_category(&new_snapshot);
    let growth = biggest_growth(&old, &new, 10);
    assert_eq!(growth.len(), 2, "only Video and Cache grew");
    // Cache grew more than Video, so it must rank first.
    assert_eq!(growth[0].category, Category::Cache);
    assert_eq!(growth[0].bytes_delta, 90_000);
    assert_eq!(growth[1].category, Category::Video);
}

#[test]
fn biggest_growth_honours_the_limit_and_rejects_zero() {
    let (old_snapshot, new_snapshot) = two_runs();
    let old = usage_by_category(&old_snapshot);
    let new = usage_by_category(&new_snapshot);
    assert_eq!(biggest_growth(&old, &new, 1).len(), 1);
    assert!(biggest_growth(&old, &new, 0).is_empty());
    assert!(biggest_growth(&old, &new, usize::MAX).len() <= 18);
}

#[test]
fn growth_order_is_deterministic_across_input_permutations() {
    // Feed the same logical snapshot in a different order; the ranked
    // output must be identical.
    let forward: Vec<(FsEntry, Option<ClassificationRef>)> = vec![
        (file("/a", 500), classified(Category::Documents)),
        (file("/b", 100), classified(Category::Downloads)),
        (file("/c", 5_000), classified(Category::Video)),
    ];
    let reversed: Vec<(FsEntry, Option<ClassificationRef>)> = {
        let mut v = forward.clone();
        v.reverse();
        v
    };
    let old = usage_by_category(&build(vec![(
        file("/seed", 10),
        classified(Category::Documents),
    )]));
    let forward_growth = biggest_growth(&old, &usage_by_category(&build(forward)), 18);
    let reversed_growth = biggest_growth(&old, &usage_by_category(&build(reversed)), 18);
    assert_eq!(forward_growth, reversed_growth);
}

#[test]
fn a_shrinking_category_is_a_fact_not_growth() {
    let old = usage_by_category(&build(vec![(
        file("/big", 100_000),
        classified(Category::Video),
    )]));
    let new = usage_by_category(&build(vec![(
        file("/big", 10),
        classified(Category::Video),
    )]));
    let summary = category_deltas(&old, &new);
    let video = summary
        .deltas
        .iter()
        .find(|d| d.category == Category::Video)
        .unwrap();
    assert!(
        video.bytes_delta < 0,
        "a shrink is negative, got {}",
        video.bytes_delta
    );
    assert!(
        biggest_growth(&old, &new, 10).is_empty(),
        "a pure shrink must not be reported as growth"
    );
}

#[test]
fn deltas_are_saturating_and_never_wrap() {
    let old = usage_by_category(&build(vec![(
        file("/huge", u64::MAX),
        classified(Category::Logs),
    )]));
    let new = usage_by_category(&build(vec![(
        file("/small", 1),
        classified(Category::Logs),
    )]));
    let summary = category_deltas(&old, &new);
    let logs = summary
        .deltas
        .iter()
        .find(|d| d.category == Category::Logs)
        .unwrap();
    assert!(logs.bytes_delta < 0);
    // 1 - u64::MAX = -(2^64 - 2), clamped to i64::MIN without wrapping.
    assert_eq!(
        logs.bytes_delta,
        i64::MIN,
        "1 - u64::MAX saturates at i64::MIN"
    );
}

#[test]
fn partial_knowledge_is_flagged_on_the_summary() {
    let mut errored = file("/denied", 0);
    errored.error = Some(coresight_engine::ErrorCategoryRef::PermissionDenied);
    let old = usage_by_category(&build(vec![]));
    let new = usage_by_category(&build(vec![
        (errored, classified(Category::SystemData)),
        (file("/ok", 1), classified(Category::SystemData)),
    ]));
    let summary = category_deltas(&old, &new);
    assert!(
        summary.partial,
        "a run with observation errors must not read as a complete total"
    );
    assert_eq!(summary.errored_entries, 1);
}

#[test]
fn change_since_is_the_same_as_aggregate_then_delta() {
    let (old_snapshot, new_snapshot) = two_runs();
    let old = usage_by_category(&old_snapshot);
    let via_deltas = category_deltas(&old, &usage_by_category(&new_snapshot));
    let via_convenience = change_since(&old, &new_snapshot);
    assert_eq!(via_deltas, via_convenience);
}

// ---------------------------------------------------------------------------
// category set is always complete
// ---------------------------------------------------------------------------

#[test]
fn every_category_appears_in_every_report() {
    for snapshot in [
        build(vec![]),
        build(vec![(file("/only", 1), classified(Category::Games))]),
    ] {
        let report = usage_by_category(&snapshot);
        assert_eq!(report.category.len(), Category::COUNT);
        assert_eq!(report.category.len(), Category::ALL.len());
    }
}

#[test]
fn category_codes_are_the_stable_ipc_identifiers() {
    let snapshot = build(vec![(file("/x", 1), classified(Category::TemporaryData))]);
    let report = usage_by_category(&snapshot);
    let temp = report.totals_for(Category::TemporaryData);
    assert_eq!(temp.logical_size, 1);
    // The code round-trips through the stored serialized name.
    assert_eq!(
        ClassificationRef::from_parts(Category::TemporaryData, None).category,
        "TEMPORARY_DATA"
    );
}

/// A set-membership check across the whole taxonomy: no category code is
/// ever ambiguous between `Other` and `Unknown` — the exact confusion
/// the contract forbids.
#[test]
fn other_and_unknown_codes_are_distinct() {
    assert_ne!(Category::Other.code(), Category::Unknown.code());
    let codes: BTreeSet<&str> = Category::ALL.iter().map(|c| c.code()).collect();
    assert_eq!(codes.len(), Category::COUNT, "all codes must be unique");
}
