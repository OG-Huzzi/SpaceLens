//! Big-file finder contract tests.
//!
//! Synthetic snapshots only. These tests prove size/kind filtering, exact
//! truncation, deterministic ordering, the inclusive age boundary, that
//! unknown ages are excluded and counted (never called old), that future
//! timestamps are not age evidence, and that all 18 category codes decode.

use coresight_classifier::Category;
use coresight_filetools::{find_big_files, BigFileOptions};
use coresight_history::{ClassificationRef, ObservedEntry, ObservedKind, RunId, Snapshot};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: u64 = 86_400;

fn at(days: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(days * DAY)
}

fn category_ref(code: &str) -> Option<ClassificationRef> {
    Some(ClassificationRef {
        category: code.to_string(),
        subcategory: None,
    })
}

fn entry(
    path: &str,
    kind: ObservedKind,
    size: Option<u64>,
    modified: Option<SystemTime>,
    category: Option<&str>,
) -> ObservedEntry {
    ObservedEntry {
        path: PathBuf::from(path),
        kind,
        size,
        object: None,
        modified,
        classification: category.and_then(category_ref),
        content_sha256: None,
        observation_error: None,
    }
}

fn file(
    path: &str,
    size: Option<u64>,
    modified: Option<SystemTime>,
    category: Option<&str>,
) -> ObservedEntry {
    entry(path, ObservedKind::File, size, modified, category)
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("big-file-run".to_string()),
        entries,
    }
}

fn options(min_size_bytes: u64, min_age_days: Option<u64>, max_results: usize) -> BigFileOptions {
    BigFileOptions {
        min_size_bytes,
        min_age_days,
        max_results,
    }
}

#[test]
fn candidates_must_be_files_with_a_known_size_at_least_the_threshold() {
    let directory = entry("/d/folder", ObservedKind::Dir, Some(10_000), None, None);
    let report = find_big_files(
        &snapshot(vec![
            file("/d/big.bin", Some(1_000), None, None),
            file("/d/small.bin", Some(999), None, None),
            file("/d/unknown-size.bin", None, None, None),
            directory,
        ]),
        at(100),
        &options(1_000, None, 10),
    );

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].path, PathBuf::from("/d/big.bin"));
    assert_eq!(report.truncated, 0);
}

#[test]
fn results_sort_by_size_then_path_and_truncate_exactly() {
    let report = find_big_files(
        &snapshot(vec![
            file("/d/a.bin", Some(1_000), None, None),
            file("/d/b.bin", Some(1_000), None, None),
            file("/d/c.bin", Some(2_000), None, None),
            file("/d/d.bin", Some(500), None, None),
        ]),
        at(100),
        &options(1, None, 2),
    );

    let paths: Vec<&PathBuf> = report
        .candidates
        .iter()
        .map(|candidate| &candidate.path)
        .collect();
    assert_eq!(
        paths,
        vec![&PathBuf::from("/d/c.bin"), &PathBuf::from("/d/a.bin")]
    );
    assert_eq!(report.truncated, 2);
}

#[test]
fn age_filter_excludes_unknown_ages_and_counts_them() {
    let report = find_big_files(
        &snapshot(vec![
            file("/d/old.bin", Some(1_000), Some(at(10)), None),
            file("/d/recent.bin", Some(1_000), Some(at(99)), None),
            file("/d/unknown-age.bin", Some(1_000), None, None),
        ]),
        at(100),
        &options(1_000, Some(30), 10),
    );

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].path, PathBuf::from("/d/old.bin"));
    assert_eq!(
        report.unknown_age_entries, 1,
        "unknown age is excluded, never treated as old"
    );
}

#[test]
fn age_boundary_is_inclusive() {
    let report = find_big_files(
        &snapshot(vec![
            file("/d/exactly-30-days.bin", Some(1_000), Some(at(70)), None),
            file(
                "/d/one-second-younger.bin",
                Some(1_000),
                Some(at(70) + Duration::from_secs(1)),
                None,
            ),
        ]),
        at(100),
        &options(1_000, Some(30), 10),
    );

    let paths: Vec<&PathBuf> = report
        .candidates
        .iter()
        .map(|candidate| &candidate.path)
        .collect();
    assert_eq!(paths, vec![&PathBuf::from("/d/exactly-30-days.bin")]);
}

#[test]
fn without_an_age_filter_unknown_age_is_a_valid_candidate() {
    let report = find_big_files(
        &snapshot(vec![file("/d/unknown-age.bin", Some(1_000), None, None)]),
        at(100),
        &options(1_000, None, 10),
    );

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].modified, None);
    assert_eq!(report.unknown_age_entries, 0);
}

#[test]
fn a_future_timestamp_is_not_evidence_of_age() {
    let report = find_big_files(
        &snapshot(vec![file(
            "/d/future.bin",
            Some(1_000),
            Some(at(200)),
            None,
        )]),
        at(100),
        &options(1_000, Some(30), 10),
    );

    assert!(report.candidates.is_empty());
    assert_eq!(report.unknown_age_entries, 0);
}

#[test]
fn all_eighteen_stored_category_codes_decode() {
    let entries: Vec<ObservedEntry> = Category::ALL
        .iter()
        .map(|category| {
            file(
                &format!("/d/{}", category.code()),
                Some(100),
                None,
                Some(category.code()),
            )
        })
        .collect();
    let report = find_big_files(
        &snapshot(entries),
        at(100),
        &options(1, None, Category::COUNT),
    );

    assert_eq!(report.candidates.len(), Category::COUNT);
    let decoded: Vec<Option<Category>> = report
        .candidates
        .iter()
        .map(|candidate| candidate.category)
        .collect();
    for category in Category::ALL {
        assert!(
            decoded.contains(&Some(category)),
            "code {} did not decode",
            category.code()
        );
    }
}

#[test]
fn unknown_category_codes_and_missing_classification_stay_none() {
    let report = find_big_files(
        &snapshot(vec![
            file("/d/future-code.bin", Some(100), None, Some("FUTURE_CODE")),
            file("/d/unclassified.bin", Some(100), None, None),
            file("/d/lowercase.bin", Some(100), None, Some("cache")),
        ]),
        at(100),
        &options(1, None, 10),
    );

    assert_eq!(report.candidates.len(), 3);
    assert!(report
        .candidates
        .iter()
        .all(|candidate| candidate.category.is_none()));
}

#[test]
fn report_is_identical_across_input_permutations() {
    let entries = vec![
        file("/d/a.bin", Some(1_000), Some(at(10)), Some("CACHE")),
        file("/d/b.bin", Some(2_000), Some(at(20)), Some("VIDEO")),
        file("/d/c.bin", Some(1_000), None, None),
        file("/d/d.bin", Some(500), Some(at(30)), Some("DOCUMENTS")),
    ];
    let mut reversed = entries.clone();
    reversed.reverse();

    let forward = find_big_files(&snapshot(entries), at(100), &options(1_000, None, 10));
    let backward = find_big_files(&snapshot(reversed), at(100), &options(1_000, None, 10));

    assert_eq!(forward, backward);
}

#[test]
fn zero_max_results_truncates_everything() {
    let report = find_big_files(
        &snapshot(vec![file("/d/big.bin", Some(1_000), None, None)]),
        at(100),
        &options(1, None, 0),
    );

    assert!(report.candidates.is_empty());
    assert_eq!(report.truncated, 1);
}
