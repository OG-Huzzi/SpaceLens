//! Link policy: record-only traversal, cycle immunity, broken-link states.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{opts, run_fake, FakeFs};
use spacelens_engine::model::EntryKind;
use spacelens_engine::summary::ScanStatus;
use spacelens_engine::CancelHandle;

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
