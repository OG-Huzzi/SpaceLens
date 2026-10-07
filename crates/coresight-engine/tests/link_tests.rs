//! Link policy: record-only traversal, cycle immunity, broken-link states.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{opts, run_fake, FakeFs, E_PERM};
use coresight_engine::model::EntryKind;
use coresight_engine::summary::ScanStatus;
use coresight_engine::{CancelHandle, ErrorCategory};

#[test]
fn symlink_cycle_terminates_and_is_not_followed() {
    // a/link -> b, b/link -> a: a filesystem cycle. This test hanging means
    // the traversal policy broke. Cycle targets are never counted as dirs.
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/cycle");
    fs.add_dir(&root);
    fs.add_dir(&root.join("a"));
    fs.add_dir(&root.join("b"));
    fs.add_symlink(&root.join("a").join("link-to-b"), &root.join("b"));
    fs.add_symlink(&root.join("b").join("link-to-a"), &root.join("a"));
    fs.add_file(&root.join("a").join("file.txt"), 5);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(
        s.status,
        ScanStatus::Completed,
        "cycle must not hang or fail"
    );
    assert_eq!(s.links, 2);
    assert_eq!(s.dirs, 3, "only real dirs: root, a, b");
    assert_eq!(s.files, 1);
}

#[test]
fn self_referential_link_does_not_hang() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/self");
    fs.add_dir(&root);
    fs.add_symlink(&root.join("loop"), &root.join("loop"));
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.links, 1);
    let entry = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "loop")
        .unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert_eq!(info.target.as_deref(), Some(Path::new("/self/loop")));
            assert!(!info.broken);
        }
        other => panic!("expected link, got {other:?}"),
    }
}

#[test]
fn broken_link_is_explicit_state() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/broken");
    fs.add_dir(&root);
    fs.add_symlink(&root.join("dangling"), &root.join("missing.txt"));
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    let entry = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "dangling")
        .unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert!(info.broken);
            assert_eq!(
                info.target.as_deref(),
                Some(Path::new("/broken/missing.txt"))
            );
        }
        other => panic!("expected link, got {other:?}"),
    }
    assert_eq!(s.links, 1);
}

#[test]
fn links_are_never_counted_as_dirs_or_descended() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/linkdir");
    fs.add_dir(&root);
    fs.add_dir(&root.join("real"));
    fs.add_file(&root.join("real").join("inner.txt"), 3);
    // Link to a directory containing a file — the file must NOT be double-
    // counted by walking through the link.
    fs.add_symlink(&root.join("dirlink"), &root.join("real"));

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.links, 1);
    assert_eq!(s.files, 1, "file behind dir link must not be counted twice");
    assert!(rec
        .entries()
        .all(|e| !e.path.to_string_lossy().contains("dirlink")
            || matches!(e.kind, EntryKind::Link(_))));
}

#[test]
fn link_targets_may_be_relative() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/rel");
    fs.add_dir(&root);
    fs.add_dir(&root.join("sub"));
    fs.add_file(&root.join("sub").join("data.txt"), 7);
    // Relative target as a real OS would report it for same-dir links.
    fs.add_symlink(&root.join("sub").join("alias"), &PathBuf::from("data.txt"));
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.links, 1);
    let entry = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "alias")
        .unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert_eq!(info.target.as_deref(), Some(Path::new("data.txt")));
            assert!(
                !info.broken,
                "relative target must resolve against the link's dir"
            );
        }
        other => panic!("expected link, got {other:?}"),
    }
}

#[test]
fn root_symlink_gets_child_link_semantics() {
    // A root that is itself a link is recorded like any other link: honest
    // target, honest broken flag, never followed (Phase 6.1).
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/root-link");
    let target = PathBuf::from("/real-dir");
    fs.add_dir(&target);
    fs.add_file(&target.join("inside.txt"), 4);
    fs.add_symlink(&root, &target);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.links, 1);
    assert_eq!(s.dirs, 0, "a root link is never followed into its target");
    assert_eq!(s.files, 0, "files behind a root link are not counted");
    let entry = rec.entries().find(|e| e.parent_id.is_none()).unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert_eq!(info.target.as_deref(), Some(Path::new("/real-dir")));
            assert!(!info.broken);
        }
        other => panic!("expected link, got {other:?}"),
    }
}

#[test]
fn root_broken_symlink_is_explicit_state() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/dangling-root");
    fs.add_symlink(&root, &PathBuf::from("/missing-target"));
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.links, 1);
    assert_eq!(s.entries_with_errors, 1);
    let entry = rec.entries().find(|e| e.parent_id.is_none()).unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert!(info.broken);
            assert_eq!(info.target.as_deref(), Some(Path::new("/missing-target")));
        }
        other => panic!("expected link, got {other:?}"),
    }
}

#[test]
fn unreadable_link_target_is_typed_not_silent() {
    // read_link_target failing (e.g. permission denied on the link) must
    // not silently become "no target": the entry carries the real category
    // and the error is tallied (Phase 6.1).
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/links");
    fs.add_dir(&root);
    fs.add_symlink(&root.join("locked"), &PathBuf::from("/elsewhere"));
    fs.fail_read_link(&root.join("locked"), E_PERM);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    let entry = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "locked")
        .unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert_eq!(
                info.target, None,
                "target unreadable — but typed, not silent"
            );
            assert!(
                !info.broken,
                "an unreadable target is not a proven-broken target"
            );
        }
        other => panic!("expected link, got {other:?}"),
    }
    assert_eq!(
        entry.error,
        Some(coresight_engine::model::ErrorCategoryRef::PermissionDenied)
    );
    assert_eq!(s.error_count(ErrorCategory::PermissionDenied), 1);
    assert_eq!(s.entries_with_errors, 1);
}

#[test]
fn root_link_target_read_error_matches_child_semantics() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/locked-root-link");
    fs.add_symlink(&root, &PathBuf::from("/elsewhere"));
    fs.fail_read_link(&root, E_PERM);
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    let entry = rec.entries().find(|e| e.parent_id.is_none()).unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert_eq!(info.target, None);
            assert!(!info.broken);
        }
        other => panic!("expected link, got {other:?}"),
    }
    assert_eq!(
        entry.error,
        Some(coresight_engine::model::ErrorCategoryRef::PermissionDenied)
    );
    assert_eq!(s.error_count(ErrorCategory::PermissionDenied), 1);
    assert_eq!(s.entries_with_errors, 1);
    assert_eq!(s.links, 1);
}

#[test]
fn link_metadata_failure_keeps_its_real_category() {
    // A link whose own stat fails (e.g. permission denied) is not "broken":
    // the entry keeps the metadata error's real category, and no target or
    // broken claims are made about a link we could not stat (Phase 6.1).
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/ghost-link");
    let target = PathBuf::from("/elsewhere");
    fs.add_dir(&root);
    fs.add_dir(&target);
    fs.add_symlink(&root.join("ghost"), &target);
    fs.fail_metadata(&root.join("ghost"), E_PERM);
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    let entry = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "ghost")
        .unwrap();
    match &entry.kind {
        EntryKind::Link(info) => {
            assert!(
                !info.broken,
                "metadata failure does not prove the target missing"
            );
            assert_eq!(info.target, None);
        }
        other => panic!("expected link, got {other:?}"),
    }
    assert_eq!(
        entry.error,
        Some(coresight_engine::model::ErrorCategoryRef::PermissionDenied)
    );
    assert_eq!(s.error_count(ErrorCategory::PermissionDenied), 1);
    assert_eq!(s.entries_with_errors, 1);
}
