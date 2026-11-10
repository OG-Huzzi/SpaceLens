//! Split/merge PLANNING contract tests.
//!
//! These tests prove part naming, offsets, remainder sizing, exact part-count
//! bounds, merge order/size validation, overflow rejection, and that a split
//! plan can be fed back into a merge plan with the same layout. No bytes are
//! read or written anywhere.

use coresight_filetools::{
    plan_merge, plan_split, MergeError, MergePart, SplitError, MAX_SPLIT_PARTS,
};
use std::path::{Path, PathBuf};

fn part(path: &str, expected_size: u64) -> MergePart {
    MergePart {
        path: PathBuf::from(path),
        expected_size,
    }
}

fn expected_part(directory: &str, file_name: &str) -> PathBuf {
    Path::new(directory).join(file_name)
}

#[test]
fn split_names_offsets_and_sizes_are_exact() {
    let plan = plan_split(Path::new("media/movie.mkv"), 10, 4).expect("valid split");

    assert_eq!(plan.source, PathBuf::from("media/movie.mkv"));
    assert_eq!(plan.total_size, 10);
    assert_eq!(plan.part_size, 4);
    assert_eq!(plan.part_count, 3);
    assert_eq!(plan.parts.len(), 3);

    let names = [
        "movie.mkv.part001",
        "movie.mkv.part002",
        "movie.mkv.part003",
    ];
    let sizes = [4u64, 4, 2];
    let offsets = [0u64, 4, 8];
    for (index, split_part) in plan.parts.iter().enumerate() {
        assert_eq!(split_part.index, index as u64 + 1);
        assert_eq!(split_part.path, expected_part("media", names[index]));
        assert_eq!(split_part.size, sizes[index]);
        assert_eq!(split_part.offset, offsets[index]);
    }
}

#[test]
fn split_exact_multiple_has_no_remainder_part() {
    let plan = plan_split(Path::new("media/movie.mkv"), 12, 4).expect("valid split");

    assert_eq!(plan.part_count, 3);
    assert!(plan.parts.iter().all(|split_part| split_part.size == 4));
    assert_eq!(
        plan.parts.iter().fold(0u64, |total, split_part| total
            .saturating_add(split_part.size)),
        12
    );
}

#[test]
fn split_with_part_larger_than_file_yields_one_part() {
    let plan = plan_split(Path::new("media/movie.mkv"), 5, 10).expect("valid split");

    assert_eq!(plan.part_count, 1);
    assert_eq!(plan.parts[0].size, 5);
    assert_eq!(plan.parts[0].offset, 0);
}

#[test]
fn split_pads_part_numbers_for_lexical_order() {
    let plan = plan_split(Path::new("media/movie.mkv"), 12, 1).expect("valid split");

    assert_eq!(plan.part_count, 12);
    assert_eq!(
        plan.parts[11].path,
        expected_part("media", "movie.mkv.part012")
    );
    let paths: Vec<&PathBuf> = plan
        .parts
        .iter()
        .map(|split_part| &split_part.path)
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "zero padding keeps lexical order stable");
}

#[test]
fn split_number_width_expands_past_three_digits() {
    let plan = plan_split(Path::new("media/huge.bin"), 1_001, 1).expect("valid split");

    assert_eq!(plan.part_count, 1_001);
    assert_eq!(
        plan.parts[0].path,
        expected_part("media", "huge.bin.part0001")
    );
    assert_eq!(
        plan.parts[1_000].path,
        expected_part("media", "huge.bin.part1001")
    );
    let paths: Vec<&PathBuf> = plan
        .parts
        .iter()
        .map(|split_part| &split_part.path)
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "width must grow with part count");
}

#[test]
fn split_rejects_zero_part_size() {
    let error = plan_split(Path::new("media/movie.mkv"), 10, 0).expect_err("zero is invalid");
    assert!(matches!(error, SplitError::ZeroPartSize));
}

#[test]
fn split_rejects_an_empty_file() {
    let error = plan_split(Path::new("media/movie.mkv"), 0, 4).expect_err("empty is invalid");
    assert!(matches!(error, SplitError::EmptyFile));
}

#[test]
fn split_rejects_a_path_without_a_file_name() {
    let error = plan_split(Path::new("/"), 10, 4).expect_err("root has no part name");
    assert!(matches!(error, SplitError::NoFileName { .. }));
}

#[test]
fn split_rejects_an_unbounded_part_count_without_allocating() {
    let error = plan_split(Path::new("media/huge.bin"), u64::MAX, 1)
        .expect_err("this would need u64::MAX parts");
    match error {
        SplitError::TooManyParts { count, max } => {
            assert_eq!(count, u64::MAX);
            assert_eq!(max, MAX_SPLIT_PARTS);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn merge_sums_declared_sizes_and_preserves_order() {
    let parts = vec![
        part("/d/backup.bin.part001", 4),
        part("/d/backup.bin.part002", 4),
        part("/d/backup.bin.part003", 2),
    ];
    let plan = plan_merge(&parts).expect("valid merge");

    assert_eq!(plan.total_size, 10);
    assert_eq!(plan.part_count, 3);
    assert_eq!(plan.parts, parts);
}

#[test]
fn merge_rejects_empty_parts() {
    let error = plan_merge(&[]).expect_err("no parts");
    assert!(matches!(error, MergeError::EmptyParts));
}

#[test]
fn merge_rejects_a_zero_sized_part() {
    let error = plan_merge(&[part("/d/backup.bin.part001", 0)]).expect_err("zero size");
    assert!(matches!(error, MergeError::SizeMismatch));
}

#[test]
fn merge_rejects_size_overflow_instead_of_saturating_a_wrong_total() {
    let error = plan_merge(&[
        part("/d/backup.bin.part001", u64::MAX),
        part("/d/backup.bin.part002", 1),
    ])
    .expect_err("total would overflow");
    assert!(matches!(error, MergeError::SizeMismatch));
}

#[test]
fn merge_rejects_unordered_parts() {
    let error = plan_merge(&[
        part("/d/backup.bin.part002", 4),
        part("/d/backup.bin.part001", 4),
    ])
    .expect_err("order is wrong");
    assert!(matches!(error, MergeError::UnorderedParts { .. }));
}

#[test]
fn merge_rejects_duplicate_parts() {
    let error = plan_merge(&[
        part("/d/backup.bin.part001", 4),
        part("/d/backup.bin.part001", 4),
    ])
    .expect_err("same part twice");
    assert!(matches!(error, MergeError::DuplicatePart { .. }));
}

#[test]
fn split_then_merge_round_trips_the_declared_layout() {
    let split = plan_split(Path::new("media/backup.bin"), 10, 4).expect("valid split");
    let merge_parts: Vec<MergePart> = split
        .parts
        .iter()
        .map(|split_part| MergePart {
            path: split_part.path.clone(),
            expected_size: split_part.size,
        })
        .collect();
    let merge = plan_merge(&merge_parts).expect("split layout is mergeable");

    assert_eq!(merge.total_size, split.total_size);
    assert_eq!(merge.part_count, split.part_count);
    assert_eq!(merge.parts, merge_parts);
}
