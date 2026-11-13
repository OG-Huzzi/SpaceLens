//! Hostile inputs aimed at the file-tool planners: duplicate detection, big
//! files, the semantic finder, bulk rename, and split/merge.
//!
//! Every fixture is synthetic. No path here exists and nothing is read or
//! written. Each test names the honesty property it protects.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use coresight_filetools::{
    find_big_files, find_duplicates, plan_merge, plan_rename, plan_split, search_at,
    BigFileOptions, DuplicateOptions, DuplicateReport, MergeError, MergePart, RenameError,
    RenameInput, RenameRule, SearchKind, SearchOptions, SearchQuery, SplitError, MAX_SPLIT_PARTS,
};
use coresight_history::{
    ClassificationRef, ObjectId, ObservedEntry, ObservedKind, RunId, Snapshot,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct Entry(ObservedEntry);

impl Entry {
    fn new(path: impl Into<PathBuf>, kind: ObservedKind) -> Self {
        Entry(ObservedEntry {
            path: path.into(),
            kind,
            size: None,
            object: None,
            modified: None,
            classification: None,
            content_sha256: None,
            observation_error: None,
        })
    }

    fn file(path: impl Into<PathBuf>) -> Self {
        Entry::new(path, ObservedKind::File)
    }

    fn size(mut self, size: u64) -> Self {
        self.0.size = Some(size);
        self
    }

    fn hash(mut self, hash: &str) -> Self {
        self.0.content_sha256 = Some(hash.to_string());
        self
    }

    fn object(mut self, inode: u64) -> Self {
        self.0.object = Some(ObjectId {
            device: 1,
            inode,
            file_id_hi: None,
        });
        self
    }

    fn modified(mut self, when: SystemTime) -> Self {
        self.0.modified = Some(when);
        self
    }

    fn build(self) -> ObservedEntry {
        self.0
    }
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("run-hostile".to_string()),
        entries,
    }
}

fn now() -> SystemTime {
    // A fixed instant: no test depends on the wall clock.
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn days_before(reference: SystemTime, days: u64) -> SystemTime {
    reference - Duration::from_secs(days * 86_400)
}

/// The type exposes no `is_conclusive` flag, so completeness is derived from
/// the published counters exactly as a UI would have to.
fn duplicates_conclusive(report: &DuplicateReport) -> bool {
    report.truncated_groups == 0 && report.truncated_members == 0
}

/// The big-file equivalent: a report is conclusive only when nothing was
/// truncated and nothing was skipped for an unknown age.
fn big_files_conclusive(report: &coresight_filetools::BigFileReport) -> bool {
    report.truncated == 0 && report.unknown_age_entries == 0
}

// ---------------------------------------------------------------------------
// Duplicate finder: magnitude, contradiction, unknown
// ---------------------------------------------------------------------------

// If this regresses, three exabyte-sized copies report a wrapped, tiny
// recoverable total.
#[test]
fn duplicate_recoverable_bytes_saturate_at_u64_max() {
    let snapshot = snapshot(vec![
        Entry::file("/d/a")
            .size(u64::MAX)
            .hash("sha-a")
            .object(1)
            .build(),
        Entry::file("/d/b")
            .size(u64::MAX)
            .hash("sha-a")
            .object(2)
            .build(),
        Entry::file("/d/c")
            .size(u64::MAX)
            .hash("sha-a")
            .object(3)
            .build(),
    ]);
    let report = find_duplicates(&snapshot, &DuplicateOptions::default());

    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].size, u64::MAX);
    assert_eq!(
        report.groups[0].recoverable_bytes,
        u64::MAX,
        "size * (distinct - 1) must saturate, never wrap"
    );
    assert_eq!(report.total_recoverable_bytes, u64::MAX);
    assert!(duplicates_conclusive(&report));
}

// If this regresses, the duplicate size floor is off by one at u64::MAX and a
// maximum-size file is either dropped or wrongly grouped.
#[test]
fn duplicate_min_size_boundary_is_exact_at_u64_max() {
    let snapshot = snapshot(vec![
        Entry::file("/d/a")
            .size(u64::MAX - 1)
            .hash("sha-near-max")
            .object(1)
            .build(),
        Entry::file("/d/b")
            .size(u64::MAX - 1)
            .hash("sha-near-max")
            .object(2)
            .build(),
    ]);

    let below_limit = find_duplicates(
        &snapshot,
        &DuplicateOptions {
            max_groups: 100,
            min_size_bytes: u64::MAX,
            max_members_per_group: 256,
        },
    );
    assert!(below_limit.groups.is_empty());
    assert_eq!(
        below_limit.unverified_entries, 0,
        "out of scope is not unverified"
    );

    let at_limit = find_duplicates(
        &snapshot,
        &DuplicateOptions {
            max_groups: 100,
            min_size_bytes: u64::MAX - 1,
            max_members_per_group: 256,
        },
    );
    assert_eq!(at_limit.groups.len(), 1);
    assert_eq!(at_limit.groups[0].size, u64::MAX - 1);
    assert_eq!(at_limit.groups[0].recoverable_bytes, u64::MAX - 1);
}

// If this regresses, hard-link aliases of one object are counted as freeable
// space the user does not actually have.
#[test]
fn hardlink_aliases_contribute_no_recoverable_bytes() {
    let snapshot = snapshot(vec![
        Entry::file("/d/a")
            .size(100)
            .hash("sha-a")
            .object(7)
            .build(),
        Entry::file("/d/b")
            .size(100)
            .hash("sha-a")
            .object(7)
            .build(),
    ]);
    let report = find_duplicates(&snapshot, &DuplicateOptions::default());

    assert_eq!(report.groups.len(), 1, "the relationship is still visible");
    assert_eq!(report.groups[0].recoverable_bytes, 0);
    assert!(!report.groups[0].members[0].is_hardlink_alias);
    assert!(report.groups[0].members[1].is_hardlink_alias);
}

// If this regresses, two entries with the same hash but different sizes are
// merged into one group and the larger size is presented as shared truth.
#[test]
fn one_hash_with_two_different_sizes_is_never_merged() {
    let snapshot = snapshot(vec![
        Entry::file("/d/a")
            .size(10)
            .hash("sha-dup")
            .object(1)
            .build(),
        Entry::file("/d/b")
            .size(20)
            .hash("sha-dup")
            .object(2)
            .build(),
    ]);
    let report = find_duplicates(&snapshot, &DuplicateOptions::default());

    assert_eq!(
        report.groups.len(),
        0,
        "inconsistent evidence is not a group"
    );
    assert_eq!(report.total_groups, 0);
    assert_eq!(report.total_recoverable_bytes, 0);
    assert_eq!(report.unverified_entries, 0);
    assert_eq!(report.unknown_size_entries, 0);
}

// If this regresses, an unhashed file or a hashed-but-unsized file is treated
// as "not a duplicate" and disappears from the accounting instead of being
// counted as unknown.
#[test]
fn unverified_and_unknown_size_entries_are_counted_not_grouped() {
    let snapshot = snapshot(vec![
        Entry::file("/d/no-hash").size(10).object(1).build(),
        Entry::file("/d/hash-no-size")
            .hash("sha-a")
            .object(2)
            .build(),
        Entry::file("/d/single")
            .size(10)
            .hash("sha-b")
            .object(3)
            .build(),
        Entry::file("/d/also-no-hash").size(10).object(4).build(),
    ]);
    let report = find_duplicates(&snapshot, &DuplicateOptions::default());

    assert_eq!(report.unverified_entries, 2);
    assert_eq!(report.unknown_size_entries, 1);
    assert_eq!(report.groups.len(), 0);
    assert_eq!(report.total_recoverable_bytes, 0);
}

// If this regresses, a capped report hides groups and the hidden bytes vanish
// from the total the user is shown.
#[test]
fn duplicate_group_truncation_is_counted_and_the_hidden_bytes_stay_visible() {
    let mut entries = Vec::new();
    for group in 0..3 {
        for member in 0..2 {
            entries.push(
                Entry::file(format!("/d/g{group}-{member}"))
                    .size(100)
                    .hash(&format!("sha-{group}"))
                    .object((group * 10 + member) as u64)
                    .build(),
            );
        }
    }
    let report = find_duplicates(
        &snapshot(entries),
        &DuplicateOptions {
            max_groups: 1,
            min_size_bytes: 1,
            max_members_per_group: 256,
        },
    );

    assert_eq!(report.total_groups, 3);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.truncated_groups, 2);
    assert_eq!(
        report.total_recoverable_bytes, 300,
        "all three groups' savings must be published even when two are hidden"
    );
    assert!(!duplicates_conclusive(&report));
}

// If this regresses, a member cap understates what the group holds.
#[test]
fn duplicate_member_truncation_is_counted_exactly() {
    let mut entries = Vec::new();
    for member in 0..5 {
        entries.push(
            Entry::file(format!("/d/m{member}"))
                .size(10)
                .hash("sha-a")
                .object(member)
                .build(),
        );
    }
    let report = find_duplicates(
        &snapshot(entries),
        &DuplicateOptions {
            max_groups: 100,
            min_size_bytes: 1,
            max_members_per_group: 2,
        },
    );

    assert_eq!(report.groups[0].members.len(), 2);
    assert_eq!(report.truncated_members, 3);
    assert_eq!(
        report.groups[0].recoverable_bytes, 40,
        "the full group (5 distinct objects) drives the savings"
    );
    assert!(!duplicates_conclusive(&report));
}

// If this regresses, a `usize::MAX` display cap is taken as an allocation
// request and the finder hangs.
#[test]
fn duplicate_usize_max_caps_do_not_allocate_or_hang() {
    let snapshot = snapshot(vec![
        Entry::file("/d/a").size(1).hash("sha-a").object(1).build(),
        Entry::file("/d/b").size(1).hash("sha-a").object(2).build(),
        Entry::file("/d/c").size(1).hash("sha-a").object(3).build(),
    ]);
    let report = find_duplicates(
        &snapshot,
        &DuplicateOptions {
            max_groups: usize::MAX,
            min_size_bytes: 1,
            max_members_per_group: usize::MAX,
        },
    );

    assert_eq!(report.groups[0].members.len(), 3);
    assert_eq!(report.truncated_groups, 0);
    assert_eq!(report.truncated_members, 0);
    assert!(duplicates_conclusive(&report));
}

// If this regresses, duplicate grouping depends on snapshot order and the same
// disk produces different reports on two runs.
#[test]
fn duplicate_report_is_byte_identical_across_permutations() {
    let entries = [
        Entry::file("/d/a").size(10).hash("sha-a").object(1).build(),
        Entry::file("/d/b").size(10).hash("sha-a").object(2).build(),
        Entry::file("/d/c").size(20).hash("sha-b").object(3).build(),
        Entry::file("/d/d").size(20).hash("sha-b").object(4).build(),
    ];
    let orders = [
        vec![
            entries[0].clone(),
            entries[1].clone(),
            entries[2].clone(),
            entries[3].clone(),
        ],
        vec![
            entries[2].clone(),
            entries[0].clone(),
            entries[3].clone(),
            entries[1].clone(),
        ],
        vec![
            entries[3].clone(),
            entries[2].clone(),
            entries[0].clone(),
            entries[1].clone(),
        ],
    ];

    let reports: Vec<String> = orders
        .iter()
        .map(|order| {
            serde_json::to_string(&find_duplicates(
                &snapshot(order.clone()),
                &DuplicateOptions::default(),
            ))
            .expect("the report serializes")
        })
        .collect();

    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
    assert!(reports[0].contains("sha-a") && reports[0].contains("sha-b"));
}

// If this regresses, an empty scan reports non-zero or panics.
#[test]
fn an_empty_snapshot_yields_an_empty_duplicate_report() {
    let report = find_duplicates(&snapshot(Vec::new()), &DuplicateOptions::default());
    assert!(report.groups.is_empty());
    assert_eq!(report.unverified_entries, 0);
    assert_eq!(report.unknown_size_entries, 0);
    assert_eq!(report.total_groups, 0);
    assert_eq!(report.total_recoverable_bytes, 0);
    assert!(duplicates_conclusive(&report));
}

// ---------------------------------------------------------------------------
// Big-file finder
// ---------------------------------------------------------------------------

// If this regresses, the age threshold wraps to a small number of seconds and
// a young file is classified as old enough to delete.
#[test]
fn big_file_age_threshold_saturates_instead_of_wrapping() {
    let min_age_days = u64::MAX / 86_400 + 2;
    let wrapped = min_age_days.wrapping_mul(86_400);
    assert!(
        wrapped < 10 * 86_400,
        "the fixture must prove a wrapping multiply would admit a ten-day-old file: {wrapped}"
    );

    let reference = now();
    let snapshot = snapshot(vec![Entry::file("/big")
        .size(1_000)
        .modified(days_before(reference, 10))
        .build()]);
    let report = find_big_files(
        &snapshot,
        reference,
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: Some(min_age_days),
            max_results: 100,
        },
    );

    assert!(
        report.candidates.is_empty(),
        "no file can be older than a saturated u64::MAX-day threshold"
    );
}

// If this regresses, a timestamp later than `now` is treated as evidence of
// age and a just-touched file is offered as old.
#[test]
fn a_future_mtime_is_never_age_evidence_for_big_files() {
    let reference = now();
    let snapshot = snapshot(vec![Entry::file("/big")
        .size(1_000)
        .modified(reference + Duration::from_secs(86_400))
        .build()]);
    let report = find_big_files(
        &snapshot,
        reference,
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: Some(0),
            max_results: 100,
        },
    );

    assert!(report.candidates.is_empty());
    // A future timestamp is unjudgeable, not "recent". It must be counted so
    // the report cannot read as conclusive.
    assert_eq!(
        report.unknown_age_entries, 1,
        "an unjudgeable timestamp must be counted as unknown age"
    );
}

// If this regresses, an entry with no timestamp is silently treated as old.
#[test]
fn an_unknown_mtime_is_counted_and_never_treated_as_old() {
    let reference = now();
    let snapshot = snapshot(vec![Entry::file("/big").size(1_000).build()]);
    let report = find_big_files(
        &snapshot,
        reference,
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: Some(1),
            max_results: 100,
        },
    );

    assert!(report.candidates.is_empty());
    assert_eq!(report.unknown_age_entries, 1);
}

// If this regresses, a future-dated big file is dropped from the age-filtered
// report without being counted anywhere, so the report reads as conclusive
// when it actually skipped an entry it could not judge.
// FIXED: find_big_files counts a future timestamp as unknown age.
#[test]
fn a_future_mtime_in_big_files_is_counted_not_silently_dropped() {
    let reference = now();
    let snapshot = snapshot(vec![Entry::file("/big/future")
        .size(1_000)
        .modified(reference + Duration::from_secs(86_400))
        .build()]);
    let report = find_big_files(
        &snapshot,
        reference,
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: Some(1),
            max_results: 100,
        },
    );

    assert!(report.candidates.is_empty());
    assert_eq!(
        report.unknown_age_entries, 1,
        "an unjudgeable timestamp must be counted in the unknown-age counter"
    );
    assert!(
        !big_files_conclusive(&report),
        "a report that skipped an entry it could not judge is not conclusive"
    );
}

// If this regresses, truncation is not counted and a bounded list is presented
// as the whole truth.
#[test]
fn big_file_truncation_is_counted_exactly() {
    let snapshot = snapshot(vec![
        Entry::file("/big/a").size(300).build(),
        Entry::file("/big/b").size(200).build(),
        Entry::file("/big/c").size(100).build(),
    ]);
    let report = find_big_files(
        &snapshot,
        now(),
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: None,
            max_results: 2,
        },
    );

    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.truncated, 1);
    assert_eq!(
        report.candidates[0].size, 300,
        "largest first, regardless of input order"
    );
}

// If this regresses, a truncated or partially-aged report is read as the whole
// truth about the disk.
#[test]
fn big_file_completeness_is_false_whenever_anything_was_skipped_or_truncated() {
    let snapshot = snapshot(vec![
        Entry::file("/big/a").size(300).build(),
        Entry::file("/big/b").size(200).build(),
    ]);

    let capped = find_big_files(
        &snapshot,
        now(),
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: None,
            max_results: 1,
        },
    );
    assert_eq!(capped.truncated, 1);
    assert!(!big_files_conclusive(&capped));

    let aged = find_big_files(
        &snapshot,
        now(),
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: Some(1),
            max_results: 100,
        },
    );
    assert_eq!(aged.unknown_age_entries, 2);
    assert!(!big_files_conclusive(&aged));

    let whole = find_big_files(
        &snapshot,
        now(),
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: None,
            max_results: 100,
        },
    );
    assert!(big_files_conclusive(&whole));
}

// If this regresses, a `usize::MAX` result cap hangs the finder.
#[test]
fn big_file_usize_max_results_does_not_allocate_or_hang() {
    let snapshot = snapshot(vec![
        Entry::file("/big/a").size(2).build(),
        Entry::file("/big/b").size(1).build(),
    ]);
    let report = find_big_files(
        &snapshot,
        now(),
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: None,
            max_results: usize::MAX,
        },
    );
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.truncated, 0);
}

// If this regresses, an unknown category code is guessed into a real category
// on the candidate row.
#[test]
fn big_file_unknown_category_stays_unknown() {
    let mut entry = Entry::file("/big/a").size(10).build();
    entry.classification = Some(ClassificationRef {
        category: "A_CATEGORY_FROM_THE_FUTURE".to_string(),
        subcategory: None,
    });
    let report = find_big_files(
        &snapshot(vec![entry]),
        now(),
        &BigFileOptions {
            min_size_bytes: 1,
            min_age_days: None,
            max_results: 100,
        },
    );

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].category, None);
}

// If this regresses, ties break on insertion order and two runs disagree.
#[test]
fn big_file_order_is_byte_identical_across_permutations() {
    let entries = [
        Entry::file("/big/b").size(100).build(),
        Entry::file("/big/a").size(100).build(),
        Entry::file("/big/c").size(200).build(),
    ];
    let orders = [
        vec![entries[0].clone(), entries[1].clone(), entries[2].clone()],
        vec![entries[2].clone(), entries[1].clone(), entries[0].clone()],
        vec![entries[1].clone(), entries[2].clone(), entries[0].clone()],
    ];

    let reports: Vec<String> = orders
        .iter()
        .map(|order| {
            serde_json::to_string(&find_big_files(
                &snapshot(order.clone()),
                now(),
                &BigFileOptions {
                    min_size_bytes: 1,
                    min_age_days: None,
                    max_results: 100,
                },
            ))
            .expect("the report serializes")
        })
        .collect();

    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
}

// ---------------------------------------------------------------------------
// Semantic finder
// ---------------------------------------------------------------------------

// If this regresses, a contradictory size range silently looks like a disk
// with no matching files, and a range error can never be told apart from a
// real empty result.
#[test]
fn a_contradictory_size_range_can_never_produce_a_hit() {
    let snapshot = snapshot(vec![Entry::file("/x/fifty").size(50).build()]);
    let query = SearchQuery {
        min_size_bytes: Some(100),
        max_size_bytes: Some(10),
        ..SearchQuery::default()
    };
    let report = search_at(&snapshot, &query, &SearchOptions::default(), now());

    assert_eq!(report.matched, 0);
    assert!(report.hits.is_empty());
    for hit in &report.hits {
        assert!(
            !hit.matched_because
                .iter()
                .any(|why| why.contains("at least")),
            "no hit may claim to satisfy an impossible lower bound"
        );
    }
    // The honest limit of the current API, recorded here rather than hidden:
    // the report has no field that distinguishes "nothing matched" from "the
    // query cannot be satisfied".
    assert_eq!(report.excluded_unknown, 0);
}

// If this regresses, a future timestamp counts as age and a fresh file is
// proposed for cleanup.
#[test]
fn a_future_mtime_is_unknown_age_for_the_finder() {
    let reference = now();
    let snapshot = snapshot(vec![Entry::file("/x/fresh")
        .size(10)
        .modified(reference + Duration::from_secs(3_600))
        .build()]);
    let query = SearchQuery {
        min_age_days: Some(0),
        ..SearchQuery::default()
    };
    let report = search_at(&snapshot, &query, &SearchOptions::default(), reference);

    assert_eq!(report.matched, 0);
    assert_eq!(
        report.excluded_unknown, 1,
        "a contradictory timestamp must be reported as unevaluable, not as a non-match"
    );
}

// If this regresses, `u64::MAX` days wraps to a short threshold and every file
// passes the age test.
#[test]
fn finder_min_age_saturates_at_u64_max() {
    let reference = now();
    let snapshot = snapshot(vec![Entry::file("/x/old")
        .size(10)
        .modified(days_before(reference, 10))
        .build()]);
    let query = SearchQuery {
        min_age_days: Some(u64::MAX),
        ..SearchQuery::default()
    };
    let report = search_at(&snapshot, &query, &SearchOptions::default(), reference);

    assert_eq!(report.matched, 0);
}

// If this regresses, an entry with no size is either dropped without a trace
// or published as if it had been measured.
#[test]
fn finder_audit_mode_counts_unknown_size_instead_of_dropping_it() {
    let snapshot = snapshot(vec![Entry::file("/x/unsized").build()]);
    let query = SearchQuery {
        min_size_bytes: Some(1),
        ..SearchQuery::default()
    };
    let report = search_at(&snapshot, &query, &SearchOptions::default(), now());

    assert_eq!(report.matched, 0);
    assert_eq!(report.excluded_unknown, 1);
    assert_eq!(report.entries_examined, 1);
}

// If this regresses, filter mode publishes a hit as if every criterion had been
// verified when one was skipped for lack of evidence.
#[test]
fn finder_filter_mode_marks_a_partial_hit_as_not_fully_evaluated() {
    let snapshot = snapshot(vec![Entry::file("/x/unsized").build()]);
    let query = SearchQuery {
        kinds: vec![SearchKind::File],
        min_size_bytes: Some(1),
        ..SearchQuery::default()
    };
    let report = search_at(
        &snapshot,
        &query,
        &SearchOptions {
            max_results: 100,
            strict_unknowns: false,
        },
        now(),
    );

    assert_eq!(report.matched, 1);
    assert!(
        !report.hits[0].fully_evaluated,
        "a hit that skipped a criterion must not claim full evaluation"
    );
    assert_eq!(report.hits[0].size, None, "unknown size must stay unknown");
}

// If this regresses, a report summed from two exabyte hits wraps.
#[test]
fn finder_total_bytes_saturate_at_u64_max() {
    let snapshot = snapshot(vec![
        Entry::file("/x/a").size(u64::MAX).build(),
        Entry::file("/x/b").size(u64::MAX).build(),
    ]);
    let query = SearchQuery {
        kinds: vec![SearchKind::File],
        ..SearchQuery::default()
    };
    let report = search_at(&snapshot, &query, &SearchOptions::default(), now());

    assert_eq!(report.matched, 2);
    assert_eq!(report.total_bytes, u64::MAX);
}

// If this regresses, a zero cap still returns rows, or a `usize::MAX` cap
// hangs.
#[test]
fn finder_result_caps_are_exact_at_both_extremes() {
    let snapshot = snapshot(vec![
        Entry::file("/x/a").size(2).build(),
        Entry::file("/x/b").size(1).build(),
    ]);
    let query = SearchQuery {
        kinds: vec![SearchKind::File],
        ..SearchQuery::default()
    };

    let none = search_at(
        &snapshot,
        &query,
        &SearchOptions {
            max_results: 0,
            strict_unknowns: true,
        },
        now(),
    );
    assert!(none.hits.is_empty());
    assert_eq!(none.matched, 2);
    assert_eq!(none.truncated, 2);

    let all = search_at(
        &snapshot,
        &query,
        &SearchOptions {
            max_results: usize::MAX,
            strict_unknowns: true,
        },
        now(),
    );
    assert_eq!(all.hits.len(), 2);
    assert_eq!(all.truncated, 0);
}

// If this regresses, hit order depends on snapshot order.
#[test]
fn finder_order_is_byte_identical_across_permutations() {
    let entries = [
        Entry::file("/x/a").size(10).build(),
        Entry::file("/x/b").size(10).build(),
        Entry::file("/x/c").size(20).build(),
    ];
    let orders = [
        vec![entries[0].clone(), entries[1].clone(), entries[2].clone()],
        vec![entries[2].clone(), entries[0].clone(), entries[1].clone()],
        vec![entries[1].clone(), entries[2].clone(), entries[0].clone()],
    ];
    let query = SearchQuery {
        kinds: vec![SearchKind::File],
        ..SearchQuery::default()
    };

    let reports: Vec<String> = orders
        .iter()
        .map(|order| {
            serde_json::to_string(&search_at(
                &snapshot(order.clone()),
                &query,
                &SearchOptions::default(),
                now(),
            ))
            .expect("the report serializes")
        })
        .collect();

    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
}

// If this regresses, an empty snapshot claims work it did not do.
#[test]
fn an_empty_snapshot_examines_nothing_and_says_so() {
    let query = SearchQuery {
        kinds: vec![SearchKind::File],
        ..SearchQuery::default()
    };
    let report = search_at(
        &snapshot(Vec::new()),
        &query,
        &SearchOptions::default(),
        now(),
    );
    assert_eq!(report.entries_examined, 0);
    assert_eq!(report.matched, 0);
    assert_eq!(report.total_bytes, 0);
}

// ---------------------------------------------------------------------------
// Bulk rename
// ---------------------------------------------------------------------------

// If this regresses, the numbering depends on input order and the same folder
// renames differently on a retry.
#[test]
fn rename_sequence_is_byte_identical_across_permutations() {
    let entries = [
        RenameInput {
            path: PathBuf::from("/d/a.txt"),
        },
        RenameInput {
            path: PathBuf::from("/d/b.txt"),
        },
        RenameInput {
            path: PathBuf::from("/d/c.txt"),
        },
    ];
    let orders = [
        vec![entries[0].clone(), entries[1].clone(), entries[2].clone()],
        vec![entries[2].clone(), entries[0].clone(), entries[1].clone()],
        vec![entries[1].clone(), entries[2].clone(), entries[0].clone()],
    ];
    let rule = RenameRule::Sequence {
        start: 1,
        padding: 3,
    };

    let plans: Vec<String> = orders
        .iter()
        .map(|order| {
            serde_json::to_string(&plan_rename(order, &rule).expect("the plan is valid"))
                .expect("the plan serializes")
        })
        .collect();

    assert_eq!(plans[0], plans[1]);
    assert_eq!(plans[1], plans[2]);
    assert!(plans[0].contains("a_001.txt"));
}

// If this regresses, the sequence wraps from u64::MAX back to 0 and a folder
// of names is numbered in an order nobody asked for.
#[test]
fn rename_sequence_saturates_at_u64_max_and_never_wraps() {
    let entries = [
        RenameInput {
            path: PathBuf::from("/d/a.txt"),
        },
        RenameInput {
            path: PathBuf::from("/d/b.txt"),
        },
    ];
    let plan = plan_rename(
        &entries,
        &RenameRule::Sequence {
            start: u64::MAX,
            padding: 1,
        },
    )
    .expect("two distinct stems cannot collide");

    let names: Vec<String> = plan
        .operations
        .iter()
        .map(|operation| {
            operation
                .to
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "a_18446744073709551615.txt".to_string(),
            "b_18446744073709551615.txt".to_string()
        ],
        "both numbers must saturate at u64::MAX, never wrap to 0"
    );
}

// If this regresses, an unbounded padding width allocates gigabytes per name.
#[test]
fn rename_padding_is_clamped_at_usize_max() {
    let plan = plan_rename(
        &[RenameInput {
            path: PathBuf::from("/d/a.txt"),
        }],
        &RenameRule::Sequence {
            start: 1,
            padding: usize::MAX,
        },
    )
    .expect("one entry is a valid plan");

    let name = plan.operations[0]
        .to
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned();
    assert!(
        name.len() < 64,
        "padding must be clamped to 32 digits, got a {}-byte name",
        name.len()
    );
    assert!(name.ends_with(".txt"));
}

// If this regresses, a rule can inject a path separator or NUL and the rename
// escapes its own directory or produces an unrepresentable name.
#[test]
fn rename_refuses_separators_and_nul_in_generated_names() {
    let entry = [RenameInput {
        path: PathBuf::from("/d/report.txt"),
    }];

    for hostile in ["../", "a/b", "a\\b", "a\0b"] {
        let error = plan_rename(&entry, &RenameRule::AddPrefix(hostile.to_string()))
            .expect_err("an injected separator must be refused");
        assert!(
            matches!(error, RenameError::InvalidName { .. }),
            "prefix {hostile:?} produced {error:?}"
        );
    }
}

// If this regresses, a rule that deletes the whole stem produces an empty name
// and the executor is pointed at the parent directory.
#[test]
fn rename_refuses_an_empty_generated_name() {
    let entry = [RenameInput {
        path: PathBuf::from("/d/report"),
    }];
    let error = plan_rename(
        &entry,
        &RenameRule::Replace {
            find: "report".to_string(),
            replace_with: String::new(),
        },
    )
    .expect_err("an empty name must be refused");
    assert!(matches!(error, RenameError::EmptyName { .. }), "{error:?}");
}

// If this regresses, a Windows device name or a trailing space is accepted as
// a target and the filesystem silently maps it onto a different file.
// FIXED: plan_rename now rejects reserved device names and names ending in a
// space or a dot.
#[test]
fn rename_never_accepts_a_windows_device_name_or_trailing_space() {
    let entry = [RenameInput {
        path: PathBuf::from("/d/a.txt"),
    }];
    let mut accepted: Vec<String> = Vec::new();
    for device in ["CON", "NUL", "PRN", "AUX", "COM1"] {
        if let Ok(plan) = plan_rename(
            &entry,
            &RenameRule::Replace {
                find: "a".to_string(),
                replace_with: device.to_string(),
            },
        ) {
            accepted.push(plan.operations[0].to.display().to_string());
        }
    }

    if let Ok(plan) = plan_rename(
        &[RenameInput {
            path: PathBuf::from("/d/report"),
        }],
        &RenameRule::AddSuffix(" ".to_string()),
    ) {
        accepted.push(plan.operations[0].to.display().to_string());
    }

    assert!(
        accepted.is_empty(),
        "device names and trailing spaces are not valid Windows targets, but were accepted: {accepted:?}"
    );
}

// If this regresses, a non-UTF-8 file name is renamed through a lossy
// conversion and points at a different file.
#[test]
fn rename_refuses_a_non_utf8_source_name() {
    let error = plan_rename(
        &[RenameInput {
            path: non_utf8_path(),
        }],
        &RenameRule::AddPrefix("x".to_string()),
    )
    .expect_err("a non-UTF-8 name cannot be transformed safely");
    assert!(
        matches!(error, RenameError::InvalidName { .. }),
        "{error:?}"
    );
}

// If this regresses, the same source supplied twice (or in two spellings that
// name one path) is renamed twice.
#[test]
fn rename_refuses_a_duplicate_source_path() {
    let entries = [
        RenameInput {
            path: PathBuf::from("/d/a.txt"),
        },
        RenameInput {
            path: PathBuf::from("/d/a.txt"),
        },
    ];
    assert!(matches!(
        plan_rename(&entries, &RenameRule::AddPrefix("x".to_string())),
        Err(RenameError::DuplicateSource { .. })
    ));
}

// If this regresses, an empty input list produces a plan that claims work.
#[test]
fn rename_of_an_empty_list_is_an_empty_plan() {
    let plan = plan_rename(&[], &RenameRule::AddPrefix("x".to_string())).expect("empty is valid");
    assert!(plan.operations.is_empty());
}

// ---------------------------------------------------------------------------
// Split / merge
// ---------------------------------------------------------------------------

// If this regresses, a one-byte part size over a huge file tries to allocate
// an unbounded part list.
#[test]
fn split_refuses_an_unbounded_part_count() {
    let error = plan_split(&PathBuf::from("/d/big.bin"), u64::MAX, 1)
        .expect_err("u64::MAX one-byte parts must be refused");
    assert_eq!(
        error,
        SplitError::TooManyParts {
            count: u64::MAX,
            max: MAX_SPLIT_PARTS
        }
    );
}

// If this regresses, splitting a maximum-size file into one part loses a byte
// to an off-by-one or a wrap.
#[test]
fn split_of_u64_max_into_one_part_is_exact() {
    let plan =
        plan_split(&PathBuf::from("/d/big.bin"), u64::MAX, u64::MAX).expect("one part is valid");

    assert_eq!(plan.part_count, 1);
    assert_eq!(plan.parts.len(), 1);
    assert_eq!(plan.parts[0].index, 1);
    assert_eq!(plan.parts[0].offset, 0);
    assert_eq!(plan.parts[0].size, u64::MAX);
    assert_eq!(plan.parts[0].path, PathBuf::from("/d/big.bin.part001"));
}

// If this regresses, the offsets of a two-part split of a maximum-size file
// wrap and the parts overlap or leave a hole.
#[test]
fn split_offsets_never_wrap_and_cover_the_whole_file() {
    let part_size = u64::MAX / 2 + 1;
    let plan =
        plan_split(&PathBuf::from("/d/big.bin"), u64::MAX, part_size).expect("two parts are valid");

    assert_eq!(plan.part_count, 2);
    assert_eq!(plan.parts[0].offset, 0);
    assert_eq!(plan.parts[0].size, part_size);
    assert_eq!(plan.parts[1].offset, part_size);
    assert_eq!(plan.parts[1].size, u64::MAX - part_size);
    assert_eq!(
        plan.parts[0].size.saturating_add(plan.parts[1].size),
        u64::MAX,
        "the parts must cover the source exactly"
    );
}

// If this regresses, a zero part size divides by zero or produces an infinite
// loop.
#[test]
fn split_refuses_zero_part_size_and_empty_file() {
    assert_eq!(
        plan_split(&PathBuf::from("/d/a.bin"), 10, 0),
        Err(SplitError::ZeroPartSize)
    );
    assert_eq!(
        plan_split(&PathBuf::from("/d/a.bin"), 0, 10),
        Err(SplitError::EmptyFile)
    );
}

// If this regresses, merging two maximum-size parts saturates the total and
// publishes a merged size that is smaller than the parts it lists.
#[test]
fn merge_rejects_an_overflowing_total_instead_of_saturating() {
    let parts = [
        MergePart {
            path: PathBuf::from("/d/a.part001"),
            expected_size: u64::MAX,
        },
        MergePart {
            path: PathBuf::from("/d/a.part002"),
            expected_size: u64::MAX,
        },
    ];
    assert_eq!(plan_merge(&parts), Err(MergeError::SizeMismatch));
}

// If this regresses, a zero-sized or unordered or duplicated part list is
// merged into a corrupt output.
#[test]
fn merge_refuses_zero_unordered_and_duplicate_parts() {
    assert_eq!(
        plan_merge(&[MergePart {
            path: PathBuf::from("/d/a.part001"),
            expected_size: 0,
        }]),
        Err(MergeError::SizeMismatch)
    );

    let unordered = [
        MergePart {
            path: PathBuf::from("/d/b.part002"),
            expected_size: 1,
        },
        MergePart {
            path: PathBuf::from("/d/a.part001"),
            expected_size: 1,
        },
    ];
    assert!(matches!(
        plan_merge(&unordered),
        Err(MergeError::UnorderedParts { .. })
    ));

    let duplicate = [
        MergePart {
            path: PathBuf::from("/d/a.part001"),
            expected_size: 1,
        },
        MergePart {
            path: PathBuf::from("/d/a.part001"),
            expected_size: 1,
        },
    ];
    assert!(matches!(
        plan_merge(&duplicate),
        Err(MergeError::DuplicatePart { .. })
    ));

    assert_eq!(plan_merge(&[]), Err(MergeError::EmptyParts));
}

// If this regresses, a legitimate split cannot be merged back byte for byte.
#[test]
fn merge_of_an_exact_split_restores_the_source_size() {
    let split = plan_split(&PathBuf::from("/d/big.bin"), 10, 4).expect("three parts");
    let parts: Vec<MergePart> = split
        .parts
        .iter()
        .map(|part| MergePart {
            path: part.path.clone(),
            expected_size: part.size,
        })
        .collect();
    let merged = plan_merge(&parts).expect("a split is mergeable");

    assert_eq!(merged.total_size, 10);
    assert_eq!(merged.part_count, 3);
    assert_eq!(
        merged
            .parts
            .iter()
            .map(|part| part.expected_size)
            .collect::<Vec<_>>(),
        vec![4, 4, 2]
    );
}

// If this regresses, a hostile rename list can create a target cycle that the
// planner hands to the executor.
#[test]
fn rename_never_returns_two_operations_with_one_target() {
    // Two sources differing only by case collapse onto one target on Windows;
    // the duplicate-target check is the guard that keeps that from executing.
    let entries = [
        RenameInput {
            path: PathBuf::from("/d/A.txt"),
        },
        RenameInput {
            path: PathBuf::from("/d/a.txt"),
        },
    ];
    let result = plan_rename(
        &entries,
        &RenameRule::Replace {
            find: "a".to_string(),
            replace_with: "same".to_string(),
        },
    );
    match result {
        Ok(plan) => {
            let targets: BTreeSet<String> = plan
                .operations
                .iter()
                .map(|op| op.to.to_string_lossy().to_lowercase())
                .collect();
            assert_eq!(targets.len(), plan.operations.len());
        }
        Err(RenameError::DuplicateTarget { .. } | RenameError::DuplicateSource { .. }) => {}
        Err(other) => panic!("unexpected refusal: {other:?}"),
    }
}

#[cfg(windows)]
fn non_utf8_path() -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    PathBuf::from(OsString::from_wide(&[
        0x0064, 0x002F, 0x0062, 0x0061, 0x0064, 0xD800, 0x002E, 0x0074, 0x0078, 0x0074,
    ]))
}

#[cfg(unix)]
fn non_utf8_path() -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(vec![
        b'/', b'd', b'/', b'b', b'a', b'd', 0xFF, b'.', b't', b'x', b't',
    ]))
}
