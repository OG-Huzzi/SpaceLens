//! Duplicate-finder contract tests.
//!
//! Every fixture is synthetic; no filesystem is touched. These tests prove
//! that a duplicate claim requires a verified content hash AND equal size,
//! that unhashed entries stay unknown, that hard-link aliases collapse to
//! one object with zero recoverable bytes, that caps are counted exactly,
//! and that input permutation cannot change the report.

use coresight_filetools::{find_duplicates, DuplicateOptions};
use coresight_history::{ObjectId, ObservedEntry, ObservedKind, RunId, Snapshot};
use std::path::PathBuf;

fn entry(
    path: &str,
    kind: ObservedKind,
    size: Option<u64>,
    content_sha256: Option<&str>,
    object: Option<ObjectId>,
) -> ObservedEntry {
    ObservedEntry {
        path: PathBuf::from(path),
        kind,
        size,
        object,
        modified: None,
        classification: None,
        content_sha256: content_sha256.map(str::to_string),
        observation_error: None,
    }
}

fn file(path: &str, size: Option<u64>, content_sha256: Option<&str>) -> ObservedEntry {
    entry(path, ObservedKind::File, size, content_sha256, None)
}

fn hashed(path: &str, size: u64, content_sha256: &str) -> ObservedEntry {
    file(path, Some(size), Some(content_sha256))
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("test-run".to_string()),
        entries,
    }
}

fn options(
    max_groups: usize,
    min_size_bytes: u64,
    max_members_per_group: usize,
) -> DuplicateOptions {
    DuplicateOptions {
        max_groups,
        min_size_bytes,
        max_members_per_group,
    }
}

fn portable_options() -> DuplicateOptions {
    options(100, 0, 100)
}

fn object(device: u64, inode: u64, file_id_hi: Option<u64>) -> ObjectId {
    ObjectId {
        device,
        inode,
        file_id_hi,
    }
}

#[test]
fn duplicates_require_a_verified_hash() {
    let report = find_duplicates(
        &snapshot(vec![
            file("/d/a.bin", Some(100), None),
            file("/d/b.bin", Some(100), None),
            file("/d/c.bin", Some(100), None),
            hashed("/d/x.bin", 100, "hash-x"),
            hashed("/d/y.bin", 100, "hash-x"),
        ]),
        &portable_options(),
    );

    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].content_sha256, "hash-x");
    assert_eq!(report.groups[0].members.len(), 2);
    assert_eq!(
        report.unverified_entries, 3,
        "unhashed entries are unknown, never grouped"
    );
}

#[test]
fn equal_hash_different_size_is_not_a_duplicate() {
    let report = find_duplicates(
        &snapshot(vec![
            hashed("/d/a.bin", 100, "same-hash"),
            hashed("/d/b.bin", 200, "same-hash"),
        ]),
        &portable_options(),
    );

    assert!(report.groups.is_empty());
    assert_eq!(report.total_groups, 0);
    assert_eq!(report.unverified_entries, 0);
}

#[test]
fn groups_sort_by_recoverable_bytes_then_hash() {
    // Group "a": 50 * (3 - 1) = 100. Group "b": 100 * (2 - 1) = 100.
    // Group "c": 10 * (4 - 1) = 30. The two 100-byte groups tie and must
    // order by hash ascending.
    let report = find_duplicates(
        &snapshot(vec![
            hashed("/d/a1", 50, "hash-a"),
            hashed("/d/a2", 50, "hash-a"),
            hashed("/d/a3", 50, "hash-a"),
            hashed("/d/b1", 100, "hash-b"),
            hashed("/d/b2", 100, "hash-b"),
            hashed("/d/c1", 10, "hash-c"),
            hashed("/d/c2", 10, "hash-c"),
            hashed("/d/c3", 10, "hash-c"),
            hashed("/d/c4", 10, "hash-c"),
        ]),
        &portable_options(),
    );

    let hashes: Vec<&str> = report
        .groups
        .iter()
        .map(|group| group.content_sha256.as_str())
        .collect();
    assert_eq!(hashes, vec!["hash-a", "hash-b", "hash-c"]);
    let recoverable: Vec<u64> = report
        .groups
        .iter()
        .map(|group| group.recoverable_bytes)
        .collect();
    assert_eq!(recoverable, vec![100, 100, 30]);
}

#[test]
fn hard_link_aliases_are_one_object_and_not_recoverable_space() {
    let shared = object(1, 10, None);
    let report = find_duplicates(
        &snapshot(vec![
            entry(
                "/d/alias-b.bin",
                ObservedKind::File,
                Some(1_000),
                Some("same-bytes"),
                Some(shared),
            ),
            entry(
                "/d/original.bin",
                ObservedKind::File,
                Some(1_000),
                Some("same-bytes"),
                Some(shared),
            ),
            entry(
                "/d/copy.bin",
                ObservedKind::File,
                Some(1_000),
                Some("same-bytes"),
                Some(object(1, 11, None)),
            ),
        ]),
        &portable_options(),
    );

    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.members.len(), 3);
    assert_eq!(
        group.recoverable_bytes, 1_000,
        "two aliases of one object plus one distinct object free only one copy"
    );
    // Members are path-sorted: alias-b, copy, original. The first member of
    // each object is canonical; the second alias is flagged.
    let aliases: Vec<bool> = group
        .members
        .iter()
        .map(|member| member.is_hardlink_alias)
        .collect();
    assert_eq!(aliases, vec![false, false, true]);
}

#[test]
fn a_group_of_only_aliases_is_emitted_with_zero_recoverable_bytes() {
    let shared = object(2, 77, Some(1));
    let report = find_duplicates(
        &snapshot(vec![
            entry(
                "/d/one",
                ObservedKind::File,
                Some(500),
                Some("alias-bytes"),
                Some(shared),
            ),
            entry(
                "/d/two",
                ObservedKind::File,
                Some(500),
                Some("alias-bytes"),
                Some(shared),
            ),
            entry(
                "/d/three",
                ObservedKind::File,
                Some(500),
                Some("alias-bytes"),
                Some(shared),
            ),
        ]),
        &portable_options(),
    );

    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.recoverable_bytes, 0);
    assert_eq!(group.members.len(), 3);
    assert_eq!(
        group
            .members
            .iter()
            .filter(|member| member.is_hardlink_alias)
            .count(),
        2
    );
}

#[test]
fn unknown_hash_and_unknown_size_are_counted_separately_and_never_grouped() {
    let mut directory = entry("/d/folder", ObservedKind::Dir, None, None, None);
    directory.size = Some(9_999);
    let report = find_duplicates(
        &snapshot(vec![
            file("/d/no-hash-1", Some(10), None),
            file("/d/no-hash-2", Some(10), None),
            file("/d/no-size-1", None, Some("hash-no-size")),
            file("/d/no-size-2", None, Some("hash-no-size")),
            hashed("/d/verified-1", 10, "verified"),
            hashed("/d/verified-2", 10, "verified"),
            directory,
        ]),
        &portable_options(),
    );

    assert_eq!(report.unverified_entries, 2);
    assert_eq!(report.unknown_size_entries, 2);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].members.len(), 2);
    assert_eq!(report.groups[0].recoverable_bytes, 10);
}

#[test]
fn member_cap_counts_truncated_members_exactly_without_understating_bytes() {
    let entries: Vec<ObservedEntry> = (1..=5)
        .map(|index| hashed(&format!("/d/f{index:02}"), 10, "five-copies"))
        .collect();
    let report = find_duplicates(&snapshot(entries), &options(10, 0, 2));

    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.members.len(), 2);
    assert_eq!(report.truncated_members, 3);
    assert_eq!(
        group.recoverable_bytes, 40,
        "the display cap must not change the full-group savings"
    );
    assert_eq!(group.members[0].path, PathBuf::from("/d/f01"));
    assert_eq!(group.members[1].path, PathBuf::from("/d/f02"));
}

#[test]
fn zero_member_cap_still_counts_every_hidden_member() {
    let entries: Vec<ObservedEntry> = (1..=3)
        .map(|index| hashed(&format!("/d/g{index}"), 10, "zero-cap"))
        .collect();
    let report = find_duplicates(&snapshot(entries), &options(10, 0, 0));

    assert_eq!(report.groups.len(), 1);
    assert!(report.groups[0].members.is_empty());
    assert_eq!(report.truncated_members, 3);
    assert_eq!(report.groups[0].recoverable_bytes, 20);
}

#[test]
fn group_cap_counts_truncated_groups_exactly_and_keeps_the_true_total() {
    let report = find_duplicates(
        &snapshot(vec![
            hashed("/d/small-1", 10, "hash-small"),
            hashed("/d/small-2", 10, "hash-small"),
            hashed("/d/mid-1", 20, "hash-mid"),
            hashed("/d/mid-2", 20, "hash-mid"),
            hashed("/d/big-1", 30, "hash-big"),
            hashed("/d/big-2", 30, "hash-big"),
        ]),
        &options(2, 0, 10),
    );

    assert_eq!(report.total_groups, 3);
    assert_eq!(report.groups.len(), 2);
    assert_eq!(report.truncated_groups, 1);
    assert_eq!(report.total_recoverable_bytes, 60);
    let hashes: Vec<&str> = report
        .groups
        .iter()
        .map(|group| group.content_sha256.as_str())
        .collect();
    assert_eq!(hashes, vec!["hash-big", "hash-mid"]);
}

#[test]
fn min_size_filters_candidates_without_calling_them_unverified() {
    let report = find_duplicates(
        &snapshot(vec![
            hashed("/d/tiny-1", 5, "tiny"),
            hashed("/d/tiny-2", 5, "tiny"),
            hashed("/d/large-1", 100, "large"),
            hashed("/d/large-2", 100, "large"),
        ]),
        &options(10, 10, 10),
    );

    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].size, 100);
    assert_eq!(report.unverified_entries, 0);
}

#[test]
fn recoverable_bytes_saturate_instead_of_wrapping() {
    let entries = vec![
        hashed("/d/a1", u64::MAX, "hash-one"),
        hashed("/d/a2", u64::MAX, "hash-one"),
        hashed("/d/a3", u64::MAX, "hash-one"),
        hashed("/d/b1", u64::MAX, "hash-two"),
        hashed("/d/b2", u64::MAX, "hash-two"),
        hashed("/d/b3", u64::MAX, "hash-two"),
    ];
    let report = find_duplicates(&snapshot(entries), &options(10, 0, 10));

    assert_eq!(report.groups.len(), 2);
    assert!(
        report
            .groups
            .iter()
            .all(|group| group.recoverable_bytes == u64::MAX),
        "each group saturates at u64::MAX"
    );
    assert_eq!(
        report.total_recoverable_bytes,
        u64::MAX,
        "the aggregate saturates too"
    );
}

#[test]
fn report_is_identical_across_input_permutations() {
    let entries = vec![
        hashed("/d/z1", 10, "hash-z"),
        hashed("/d/z2", 10, "hash-z"),
        hashed("/d/a1", 20, "hash-a"),
        hashed("/d/a2", 20, "hash-a"),
        hashed("/d/m1", 30, "hash-m"),
        hashed("/d/m2", 30, "hash-m"),
        file("/d/unknown", Some(30), None),
        entry(
            "/d/hard-1",
            ObservedKind::File,
            Some(40),
            Some("hash-hard"),
            Some(object(3, 3, None)),
        ),
        entry(
            "/d/hard-2",
            ObservedKind::File,
            Some(40),
            Some("hash-hard"),
            Some(object(3, 3, None)),
        ),
    ];
    let mut reversed = entries.clone();
    reversed.reverse();

    let forward = find_duplicates(&snapshot(entries), &portable_options());
    let backward = find_duplicates(&snapshot(reversed), &portable_options());

    assert_eq!(forward, backward);
}
