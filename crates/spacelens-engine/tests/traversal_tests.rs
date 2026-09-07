//! Traversal correctness: nesting, emptiness, naming, depth.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{opts, run_fake, FakeFs};
use spacelens_engine::summary::ScanStatus;
use spacelens_engine::{CancelHandle, ScanOptions};

#[test]
fn nested_tree_counts_bytes_and_parents() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/fixture");
    fs.add_dir(&root);
    fs.add_dir(&root.join("a"));
    fs.add_dir(&root.join("a").join("sub"));
    fs.add_file(&root.join("a").join("f1.bin"), 1_000);
    fs.add_file(&root.join("a").join("sub").join("f2.bin"), 2_000);
    fs.add_file(&root.join("top.bin"), 4_000);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.files, 3);
    assert_eq!(s.dirs, 3, "root + a + sub");
    assert_eq!(s.links, 0);
    assert_eq!(s.bytes, 7_000);

    let ids: Vec<u64> = rec.entries().map(|e| e.id).collect();
    let unique = ids.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), ids.len(), "ids must be unique");
    assert_eq!(
        rec.entries().next().unwrap().parent_id,
        None,
        "root has no parent"
    );
    for e in rec.entries().skip(1) {
        assert!(e.parent_id.is_some(), "non-root entry needs a parent id");
    }
}

#[test]
fn empty_dirs_are_reported() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/empty-root");
    fs.add_dir(&root);
    fs.add_dir(&root.join("nothing-here"));
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.dirs, 2);
    assert_eq!(s.files, 0);
    assert_eq!(s.bytes, 0);
}

#[test]
fn hidden_flag_is_platform_reported() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/hidden");
    fs.add_dir(&root);
    fs.add_file(&root.join(".secret"), 1);
    fs.set_hidden(&root.join(".secret"), true);
    fs.add_file(&root.join("normal.txt"), 2);
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let hidden = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == ".secret")
        .unwrap();
    assert!(hidden.hidden);
    let normal = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "normal.txt")
        .unwrap();
    assert!(!normal.hidden);
}

#[test]
fn unicode_and_special_names_are_preserved() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/names");
    fs.add_dir(&root);
    let names = [
        "spaced name.txt",
        "日本語のファイル.dat",
        "emoji \u{1f600}.bin",
        "quo\"te.txt",
        "tab\tname.txt",
    ];
    for (i, n) in names.iter().enumerate() {
        fs.add_file(&root.join(n), 10 + i as u64);
    }
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.files, names.len() as u64);
    for n in names {
        assert!(
            rec.entries().any(|e| e.path.file_name().unwrap() == n),
            "missing entry for {n:?}"
        );
    }
}

#[test]
fn deep_nesting_terminates() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/deep");
    fs.add_dir(&root);
    let mut cur = root.clone();
    const DEPTH: u32 = 300;
    for i in 0..DEPTH {
        let next = cur.join(format!("level-{i:04}"));
        fs.add_dir(&next);
        cur = next;
    }
    fs.add_file(&cur.join("bottom.txt"), 1);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.dirs, DEPTH as u64 + 1);
    assert_eq!(s.files, 1);
    assert!(!s.depth_capped, "no cap set: full depth must be walked");
}

#[test]
fn depth_cap_stops_descent_and_flags_summary() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/cap");
    fs.add_dir(&root);
    fs.add_dir(&root.join("l1"));
    fs.add_dir(&root.join("l1").join("l2"));
    fs.add_dir(&root.join("l1").join("l2").join("l3"));
    fs.add_file(&root.join("l1").join("l2").join("l3").join("deep.txt"), 9);

    let options = ScanOptions {
        threads: 2,
        max_depth: Some(2),
        ..ScanOptions::default()
    };
    let rec = run_fake(&fs, &root, options, &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert!(s.depth_capped);
    assert!(rec.entries().any(|e| e.path.file_name().unwrap() == "l3"));
    assert!(!rec
        .entries()
        .any(|e| e.path.file_name().unwrap() == "deep.txt"));
}

#[test]
fn special_node_kinds_map_to_other() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/others");
    fs.add_dir(&root);
    fs.add_other(&root.join("fifo"));
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.other_entries, 1);
    assert!(matches!(
        rec.entries()
            .find(|e| e.path.file_name().unwrap() == "fifo")
            .unwrap()
            .kind,
        spacelens_engine::model::EntryKind::Other
    ));
}
