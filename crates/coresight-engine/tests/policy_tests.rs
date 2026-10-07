//! Option-policy honesty (Phase 6.1): an unimplemented follow-mode is
//! refused with a typed error, never silently downgraded.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{opts, run_fake, FakeFs};
use coresight_engine::options::SymlinkPolicy;
use coresight_engine::summary::ScanStatus;
use coresight_engine::{CancelHandle, ErrorCategory, ScanOptions};

#[test]
fn follow_with_cycle_guard_is_refused_typed() {
    let fs = Arc::new(FakeFs::new());
    fs.add_dir(&PathBuf::from("/policy"));
    fs.add_file(&PathBuf::from("/policy/data.txt"), 9);
    let options = ScanOptions {
        symlink_policy: SymlinkPolicy::FollowWithCycleGuard,
        ..opts()
    };
    let rec = run_fake(
        &fs,
        &PathBuf::from("/policy"),
        options,
        &CancelHandle::new(),
    );
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Failed);
    assert_eq!(s.error_count(ErrorCategory::Unsupported), 1);
    assert_eq!(s.errors.total_errors(), 1);
    assert_eq!(rec.entries().count(), 0, "no traversal may happen");
    assert_eq!(s.files, 0);
    assert_eq!(s.dirs, 0);
    assert_eq!(s.links, 0);
}

#[test]
fn refused_policy_does_not_even_stat_the_root() {
    // The root does not exist at all; the reported error must be the policy
    // refusal (Unsupported), proving the filesystem was never consulted.
    let fs = Arc::new(FakeFs::new());
    let options = ScanOptions {
        symlink_policy: SymlinkPolicy::FollowWithCycleGuard,
        ..opts()
    };
    let rec = run_fake(
        &fs,
        &PathBuf::from("/missing"),
        options,
        &CancelHandle::new(),
    );
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Failed);
    assert_eq!(s.error_count(ErrorCategory::Unsupported), 1);
    assert_eq!(s.error_count(ErrorCategory::NotFound), 0);
}

#[test]
fn record_only_is_the_only_implemented_policy() {
    assert!(SymlinkPolicy::RecordOnly.is_implemented());
    assert!(!SymlinkPolicy::FollowWithCycleGuard.is_implemented());
}
