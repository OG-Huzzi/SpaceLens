//! Phase 3 integration tests: the identity pipeline against the real
//! platform boundary (`StdFs` + `DefaultReaderFactory`), including real
//! scans, hard links, links-never-followed, mutation-during-hash, and
//! vanishing files. Host-independent by construction: everything runs
//! inside a `tempfile` tempdir.

use std::path::Path;
use std::time::Duration;

use spacelens_engine::platform::std_fs;
use spacelens_engine::{CancelHandle, FsEntry, ScanOptions};
use spacelens_identity::{
    run_duplicates, ContentHash, DefaultReaderFactory, DuplicateOptions, DuplicateProgressEvent,
    DuplicateStatus, StorageAccounting,
};

fn scan_entries(root: &Path) -> Vec<FsEntry> {
    let mut entries = Vec::new();
    let options = ScanOptions {
        threads: 2,
        ..ScanOptions::default()
    };
    spacelens_engine::scan(root, options, &CancelHandle::new(), &mut |e| {
        if let spacelens_engine::ScanEvent::Entry(entry) = e {
            // The scanner emits the scanned root itself as an entry. These
            // tests reason about the tree's *contents*, so drop the root
            // (exact path match; the root is never traversed recursively,
            // so nothing else can share its path).
            if entry.path == root {
                return;
            }
            entries.push(*entry);
        }
    });
    entries
}

fn run_pipeline(
    entries: Vec<FsEntry>,
    opts: &DuplicateOptions,
) -> spacelens_identity::DuplicateReport {
    let factory = DefaultReaderFactory::new(std_fs());
    run_duplicates(
        entries.into_iter(),
        opts,
        &CancelHandle::new(),
        Some(&factory),
        &mut |_| {},
    )
}

fn default_opts() -> DuplicateOptions {
    DuplicateOptions {
        // Real files: keep the zero-byte policy test explicit.
        group_zero_byte_files: true,
        progress_interval: Duration::from_millis(250),
        ..DuplicateOptions::default()
    }
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn scanned_tree_groups_real_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("docs/a/report.pdf"), b"identical-pdf-bytes");
    write(&root.join("docs/b/copy.pdf"), b"identical-pdf-bytes");
    write(&root.join("media/unique.png"), b"png-not-like-others");
    // Same size, different content: candidates but not duplicates.
    write(&root.join("media/x.bin"), b"0123456789");
    write(&root.join("media/y.bin"), b"abcdefghij");

    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(report.groups.len(), 1, "{report:?}");
    let g = &report.groups[0];
    assert_eq!(g.member_count, 2);
    assert_eq!(g.size, 19);
    let paths: Vec<_> = g.members.iter().map(|m| m.path.clone()).collect();
    assert!(paths.contains(&root.join("docs/a/report.pdf")));
    assert!(paths.contains(&root.join("docs/b/copy.pdf")));
    // Same-size different-content pair must not group.
    assert_eq!(report.stats.size_groups_without_duplicates, 1);
    // Real platform proves object identity on this host.
    assert_eq!(g.accounting, StorageAccounting::Exact);
    assert_eq!(g.recoverable_bytes, Some(19));
}

#[test]
fn unicode_and_space_names_classify_and_group() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("répertoire/файл с пробелами.dat"), b"same-bytes");
    write(&root.join("另一目录/copy — 副本.dat"), b"same-bytes");

    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.groups[0].member_count, 2);
}

#[test]
#[cfg(unix)]
fn hard_links_are_one_object_zero_recoverable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let a = root.join("original.dat");
    let b = root.join("alias.dat");
    write(&a, b"hardlinked-content");
    std::fs::hard_link(&a, &b).unwrap();

    let entries = scan_entries(root);
    assert_eq!(entries.len(), 2);
    let report = run_pipeline(entries, &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
    let g = &report.groups[0];
    assert_eq!(g.member_count, 2, "both paths are members");
    assert_eq!(g.accounting, StorageAccounting::Exact);
    assert_eq!(
        g.recoverable_bytes, None,
        "one file object: removing an alias frees nothing"
    );
    assert!(!g.spans_multiple_objects());
    // The members share the object identity recorded from the open handles.
    let ids: Vec<_> = g.members.iter().map(|m| m.object_id).collect();
    assert_eq!(ids[0], ids[1], "handle identity must match for aliases");
}

#[test]
#[cfg(unix)]
fn distinct_copies_are_two_objects_full_recoverable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let a = root.join("one.dat");
    let b = root.join("two.dat");
    write(&a, b"copied-content");
    std::fs::copy(&a, &b).unwrap();

    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.groups.len(), 1);
    let g = &report.groups[0];
    assert_eq!(g.accounting, StorageAccounting::Exact);
    assert_eq!(
        g.recoverable_bytes,
        Some(14),
        "copy: freeing one frees bytes"
    );
    assert_ne!(g.members[0].object_id, g.members[1].object_id);
}

#[test]
fn symlinks_are_never_grouped_or_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("real.txt"), b"target-content");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("alias.txt")).unwrap();
    }
    #[cfg(windows)]
    {
        // File symlinks require the SeCreateSymbolicLink privilege (CI
        // runners usually have it; dev hosts may not). A directory junction
        // needs no privilege — create one as the guaranteed link.
        let _ = std::os::windows::fs::symlink_file(root.join("real.txt"), root.join("alias.txt"));
        if !root.join("alias.txt").exists() {
            let dir_link = root.join("alias_dir");
            if std::fs::symlink_metadata(&dir_link).is_err() {
                // This host may silently refuse symlink/junction creation
                // (privilege or filter-driver interference; see Phase 1's
                // recorded link-test skip). If no link can be created the
                // link-recording assertions cannot run here — the unix CI
                // leg proves them.
                if std::os::windows::fs::symlink_dir(root.join("."), &dir_link).is_err() {
                    eprintln!("skipping link assertions: host cannot create links");
                    return;
                }
            }
        }
    }

    let entries = scan_entries(root);
    let links = entries
        .iter()
        .filter(|e| matches!(e.kind, spacelens_engine::EntryKind::Link(_)))
        .count();
    assert!(links >= 1, "the link must be recorded as a link");
    let report = run_pipeline(entries, &default_opts());
    // Only the real file was eligible; no false duplicate pair with itself.
    assert!(report.groups.is_empty(), "{report:?}");
    assert_eq!(report.eligibility.links, links as u64);
}

#[test]
fn vanishing_file_is_typed_and_never_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("keep.bin"), b"stable-content");
    let gone = root.join("gone.bin");
    write(&gone, b"stable-content");
    let size = std::fs::metadata(&gone).unwrap().len();

    // Observe, then delete before hashing.
    let entries: Vec<FsEntry> = scan_entries(root)
        .into_iter()
        .map(|mut e| {
            if e.path == gone {
                e.size = size; // observed, now vanished
            }
            e
        })
        .collect();
    std::fs::remove_file(&gone).unwrap();

    let report = run_pipeline(entries, &default_opts());
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert!(report.groups.is_empty(), "vanishing file must not group");
    assert_eq!(report.stats.failures, 1);
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.path == gone
                && matches!(f.kind, spacelens_identity::HashFailureKind::Vanished))
    );
}

#[test]
fn file_changed_between_scan_and_hash_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("stable.bin"), b"1111111111");
    let changeling = root.join("changeling.bin");
    write(&changeling, b"2222222222");
    let size = std::fs::metadata(&changeling).unwrap().len();

    let entries: Vec<FsEntry> = scan_entries(root)
        .into_iter()
        .map(|mut e| {
            if e.path == changeling {
                e.size = size;
            }
            e
        })
        .collect();
    // Mutate after the scan observed it.
    write(&changeling, b"33333333333333");

    let report = run_pipeline(entries, &default_opts());
    assert!(
        report
            .groups
            .iter()
            .all(|g| !g.members.iter().any(|m| m.path == changeling)),
        "a changed file must not enter a group: {report:?}"
    );
    assert_eq!(report.stats.failures, 1);
    assert!(matches!(
        report.failures[0].kind,
        spacelens_identity::HashFailureKind::Changed
    ));
}

#[test]
fn permission_denied_is_typed_and_never_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("readable.bin"), b"openable");
    let locked = root.join("locked.bin");
    write(&locked, b"openable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked, perms).unwrap();
    }
    // On Windows, ACL manipulation needs the windows-sys surface; the unix
    // path proves the typed semantics. Windows still exercises the code
    // path via CI where the file remains readable (no failure asserted).

    let entries = scan_entries(root);
    let report = run_pipeline(entries, &default_opts());
    #[cfg(unix)]
    {
        assert_eq!(report.stats.failures, 1, "{report:?}");
        assert!(matches!(
            report.failures[0].kind,
            spacelens_identity::HashFailureKind::Hash {
                category: spacelens_engine::ErrorCategory::PermissionDenied
            }
        ));
        assert!(report.groups.is_empty());
    }
    #[cfg(not(unix))]
    {
        // Windows: chmod(0) is a no-op (no POSIX ACL mapping), so the file
        // usually stays readable — both files hash and group. The contract
        // under test is: no panic, and the pipeline stays consistent either
        // way. (The typed PermissionDenied path is proven on the unix CI
        // leg; error categorization itself is unit-tested on all hosts.)
        if report.stats.failures == 1 {
            assert!(report.groups.is_empty());
        } else {
            assert_eq!(report.stats.failures, 0);
            assert_eq!(report.groups.len(), 1);
            assert_eq!(report.groups[0].member_count, 2);
        }
    }
    // Restore permissions so tempdir cleanup works.
    #[cfg(unix)]
    {
        if locked.exists() {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&locked).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&locked, perms).unwrap();
        }
    }
}

#[test]
fn zero_byte_duplicates_follow_the_explicit_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("e1"), b"");
    write(&root.join("e2"), b"");
    write(&root.join("real.bin"), b"data");

    let entries = scan_entries(root);

    // Default: counted, not grouped.
    let report = run_pipeline(entries.clone(), &DuplicateOptions::default());
    assert!(report.groups.is_empty(), "{report:?}");
    assert_eq!(report.stats.zero_byte_matches_ungrouped, 2);

    // Opt-in: grouped with empty identity.
    let opts = default_opts();
    let report = run_pipeline(entries, &opts);
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.groups[0].content_hash, ContentHash::empty());
    assert_eq!(report.groups[0].logical_duplicate_bytes, 0);
    assert_eq!(
        report.groups[0].recoverable_bytes,
        Some(0),
        "empty files free nothing (0 bytes — an honest zero, not None)"
    );
}

#[test]
fn deep_nesting_is_handled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut deep = root.to_path_buf();
    for i in 0..20 {
        deep.push(format!("level-{i}"));
    }
    write(&deep.join("deep.bin"), b"deep-content");
    write(&root.join("shallow.bin"), b"deep-content");

    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.groups[0].member_count, 2);
}

#[test]
fn progress_events_are_bounded_and_terminal_is_exact() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("p1.bin"), b"progress-content");
    write(&root.join("p2.bin"), b"progress-content");

    let mut events = Vec::new();
    let factory = DefaultReaderFactory::new(std_fs());
    let report = run_duplicates(
        scan_entries(root).into_iter(),
        &default_opts(),
        &CancelHandle::new(),
        Some(&factory),
        &mut |e| {
            events.push(match &e {
                DuplicateProgressEvent::Started => 0,
                DuplicateProgressEvent::Progress(_) => 1,
                DuplicateProgressEvent::Completed(_) => 2,
                DuplicateProgressEvent::Cancelled(_) => 3,
                DuplicateProgressEvent::Failed(_) => 4,
            });
        },
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(events.first(), Some(&0), "starts with Started");
    assert_eq!(
        events.last(),
        Some(&2),
        "exactly one terminal Completed event"
    );
    assert!(!events.contains(&3) && !events.contains(&4));
    // Throttle: progress interval is 250ms; this fast run must not flood.
    let progress_count = events.iter().filter(|c| **c == 1).count();
    assert!(
        progress_count <= 4,
        "progress must be throttled: {events:?}"
    );
}

#[test]
fn large_sparse_file_hashes_streaming() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Sparse files via `set_len`: logical size without physical allocation.
    // Two all-zero files of identical length have identical content by
    // construction, so they must group. Size kept moderate: debug-build
    // hashing of zero-filled regions is slow on CI runners (~28 MiB/s on
    // macOS); the true >4 GiB streaming proof lives in the synthetic
    // `huge_file_streams_beyond_4gib` perf test, which needs no disk at all.
    let big = root.join("huge.bin");
    let other = root.join("twin.bin");
    let size: u64 = 256 * 1024 * 1024 + 7;
    for p in [&big, &other] {
        let Ok(f) = std::fs::File::create(p) else {
            return; // environment cannot provide sparse files
        };
        if f.set_len(size).is_err() {
            drop(f);
            let _ = std::fs::remove_file(p);
            return;
        }
    }

    // Runner temp dirs can contain unrelated noise files; filter to this
    // test's fixtures so assertions test engine semantics, not the host.
    let entries: Vec<_> = scan_entries(root)
        .into_iter()
        .filter(|e| {
            matches!(
                e.path.file_name().and_then(|n| n.to_str()),
                Some("huge.bin") | Some("twin.bin")
            )
        })
        .collect();
    assert_eq!(entries.len(), 2, "fixtures must be observed: {entries:?}");
    assert_eq!(entries.iter().map(|e| e.size).max(), Some(size));

    let report = run_pipeline(entries, &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
    let g = &report.groups[0];
    assert_eq!(g.size, size, "file sizes must round-trip exactly");
    assert_eq!(g.member_count, 2);
    assert_eq!(
        g.logical_duplicate_bytes, size,
        "u64 arithmetic, no overflow"
    );
}

#[test]
fn repeated_runs_identical_logical_results() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("r1.bin"), b"repeat-me");
    write(&root.join("r2.bin"), b"repeat-me");
    write(&root.join("r3.bin"), b"different");

    let build = || run_pipeline(scan_entries(root), &default_opts());
    let a = build();
    let b = build();
    assert_eq!(a.groups, b.groups);
    assert_eq!(a.stats.candidates_hashed, b.stats.candidates_hashed);
    assert_eq!(a.stats.files_hashed, b.stats.files_hashed);
    assert_eq!(a.eligibility, b.eligibility);
    assert_eq!(a.logical_duplicate_bytes, b.logical_duplicate_bytes);
}

#[test]
fn cancelled_before_start_reports_cancelled_with_no_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("c1.bin"), b"cancel-content");
    write(&root.join("c2.bin"), b"cancel-content");

    let cancel = CancelHandle::new();
    cancel.cancel();
    let factory = DefaultReaderFactory::new(std_fs());
    let report = run_duplicates(
        scan_entries(root).into_iter(),
        &default_opts(),
        &cancel,
        Some(&factory),
        &mut |_| {},
    );
    assert_eq!(report.status, DuplicateStatus::Cancelled);
    assert!(report.groups.is_empty());
}

/// Hostile: many same-size different-content files (candidate storm).
#[test]
fn hostile_same_size_storm_stays_correct_and_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let n = 300u32;
    for i in 0..n {
        let content = format!("unique-content-{i:06}-{:08x}", i.wrapping_mul(0x9E3779B9));
        write(&root.join(format!("f{i:04}.bin")), content.as_bytes());
    }
    // All the same size by construction; all distinct by content.
    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert!(report.groups.is_empty(), "distinct content must not group");
    assert_eq!(report.stats.candidates_hashed, n as u64);
    assert_eq!(report.stats.size_groups_without_duplicates, 1);
}

/// Hostile: many true duplicates form one stable group.
#[test]
fn hostile_many_duplicates_one_group() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let n = 300u32;
    for i in 0..n {
        write(&root.join(format!("dup{i:04}.bin")), b"duplicate-payload");
    }
    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
    let g = &report.groups[0];
    assert_eq!(g.member_count, n as u64);
    // Detail is capped at the default (64 members); the count stays exact.
    assert_eq!(
        g.members.len(),
        spacelens_identity::duplicate::DUPLICATE_GROUP_DETAIL_CAP
    );
    assert!(g.detail_truncated());
    assert_eq!(
        g.logical_duplicate_bytes,
        17 * (n as u64 - 1),
        "17-byte payload × (n−1)"
    );
}

/// Hostile: hundreds of zero-byte files under the opt-in policy.
#[test]
fn hostile_many_zero_byte_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let n = 500u32;
    for i in 0..n {
        write(&root.join(format!("z{i:04}.bin")), b"");
    }
    let mut opts = default_opts();
    opts.group_zero_byte_files = true;
    let report = run_pipeline(scan_entries(root), &opts);
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.groups[0].member_count, n as u64);
    assert_eq!(report.groups[0].logical_duplicate_bytes, 0);
    assert_eq!(
        report.groups[0].recoverable_bytes,
        Some(0),
        "zero bytes honestly"
    );
}

/// Hostile: thousands of duplicate groups stay deterministic and complete.
#[test]
fn hostile_many_groups_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let groups = 400u32;
    for i in 0..groups {
        let content = format!("group-payload-{i:06}");
        write(&root.join(format!("g{i:04}_a.bin")), content.as_bytes());
        write(&root.join(format!("g{i:04}_b.bin")), content.as_bytes());
    }
    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.groups.len(), groups as usize);
    assert_eq!(report.logical_duplicate_bytes, {
        let per = 20u64; // "group-payload-NNNNNN".len()
        per * groups as u64
    });
    // Deterministic order: sizes equal, so hash bytes ascending.
    let hashes: Vec<ContentHash> = report.groups.iter().map(|g| g.content_hash).collect();
    let mut sorted = hashes.clone();
    sorted.sort();
    assert_eq!(
        hashes, sorted,
        "group order must be by hash bytes within equal size"
    );
}

#[test]
fn mixed_platform_paths_never_leak_into_grouping() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Names that *look* like path fragments on the other OS are just bytes;
    // grouping must not special-case them. (A literal `C:`-prefixed name is
    // impossible as a Windows test fixture — it is a drive-relative path —
    // so the fixture uses a `D:`-adjacent look-alike.)
    write(&root.join("drive_C_colon.dat"), b"pathish-content");
    write(&root.join("backslash name.dat"), b"pathish-content");
    let report = run_pipeline(scan_entries(root), &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
}

#[test]
fn entries_with_scan_errors_are_ineligible() {
    // Synthesize an entry stream with an observation error (locked file
    // semantics) alongside a clean duplicate pair; the pipeline must skip
    // the errored entry.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("ok1.bin"), b"clean-pair");
    write(&root.join("ok2.bin"), b"clean-pair");
    let mut entries = scan_entries(root);
    entries.push(FsEntry {
        id: 999,
        parent_id: None,
        path: root.join("phantom.bin"),
        kind: spacelens_engine::EntryKind::File,
        size: 10,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: None,
        inode: None,
        file_id_hi: None,
        hidden: false,
        error: Some(spacelens_engine::ErrorCategoryRef::InUse),
    });

    let report = run_pipeline(entries, &default_opts());
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.eligibility.observation_errors, 1);
    assert!(report.groups[0]
        .members
        .iter()
        .all(|m| m.path != root.join("phantom.bin")));
}
