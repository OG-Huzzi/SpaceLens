//! Error handling: permission walls, vanishing entries, typed root failures.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{opts, run_fake, FakeFs, E_NOT_FOUND, E_PERM};
use spacelens_engine::summary::ScanStatus;
use spacelens_engine::{CancelHandle, ErrorCategory};

#[test]
fn permission_denied_dir_does_not_fail_scan() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/perm");
    fs.add_dir(&root);
    fs.add_dir(&root.join("locked"));
    fs.fail_list(&root.join("locked"), E_PERM);
    fs.add_dir(&root.join("open"));
    fs.add_file(&root.join("open").join("keep.txt"), 42);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(
        s.status,
        ScanStatus::Completed,
        "one denied dir must not kill the scan"
    );
    assert_eq!(s.error_count(ErrorCategory::PermissionDenied), 1);
    assert_eq!(s.errors.total_errors(), 1);
    assert_eq!(s.files, 1);
    assert_eq!(s.dirs, 3, "root + locked + open all emit dir entries");
}

#[test]
fn vanishing_file_is_recorded_not_fatal() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/vanish");
    fs.add_dir(&root);
    // Listed by read_dir, gone before metadata could be collected — the
    // classic live-filesystem race, simulated by a stat that misses.
    fs.add_file(&root.join("gone.txt"), 777);
    fs.fail_metadata(&root.join("gone.txt"), E_NOT_FOUND);
    fs.add_file(&root.join("stable.txt"), 10);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.error_count(ErrorCategory::NotFound), 1);
    let gone = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "gone.txt")
        .unwrap();
    assert_eq!(
        gone.error,
        Some(spacelens_engine::model::ErrorCategoryRef::NotFound)
    );
    assert_eq!(s.bytes, 10, "vanished file must not add bytes");
}

#[test]
fn permission_denied_file_metadata_is_typed() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/pfile");
    fs.add_dir(&root);
    fs.add_file(&root.join("secret.bin"), 1);
    fs.fail_metadata(&root.join("secret.bin"), E_PERM);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.error_count(ErrorCategory::PermissionDenied), 1);
}

#[test]
fn missing_root_fails_typed() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/does-not-exist");
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Failed);
    assert_eq!(s.error_count(ErrorCategory::NotFound), 1);
    assert!(matches!(
        rec.terminal(),
        Some(spacelens_engine::progress::ScanEvent::Failed(_))
    ));
}

#[test]
fn unreadable_root_fails_typed() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/locked-root");
    fs.add_dir(&root);
    fs.fail_list(&root, E_PERM);
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Failed);
    assert_eq!(s.error_count(ErrorCategory::PermissionDenied), 1);
}

#[test]
fn root_error_preserves_raw_os_code() {
    let fs = Arc::new(FakeFs::new());
    let rec = run_fake(&fs, &PathBuf::from("/nope"), opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.errors.detail.len(), 1);
    assert_eq!(s.errors.detail[0].raw_os, Some(E_NOT_FOUND));
    assert_eq!(s.errors.detail[0].category.code(), "ENTRY_NOT_FOUND");
}
