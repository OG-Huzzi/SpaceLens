//! `execute`: all-or-nothing over a whole target list.
//!
//! Contract 2 (rollback works) and contract 3 (partial rollback is honest) are
//! proved here, together with the pre-flight refusals (empty list, duplicate
//! target) and the happy path that feeds `verify`.

mod common;

use std::cell::Cell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use common::{assert_inside, authorized_plan_for, confirmation_for};
use coresight_actions::{execute, verify, DirectoryTrash, ExecuteError, TrashDestination};

#[test]
fn execute_moves_every_target_and_reports_the_moved_bytes() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let first = root.join("a.bin");
    let second = root.join("b.bin");
    fs::write(&first, b"12345").expect("writable");
    fs::write(&second, b"67890").expect("writable");
    assert_inside(root, &first);
    assert_inside(root, &second);

    let authorized = authorized_plan_for(10);
    let confirmed = confirmation_for(&authorized);
    let report = execute(
        &authorized,
        &confirmed,
        &DirectoryTrash::new(&trash_dir),
        &[first.clone(), second.clone()],
    )
    .expect("both files are movable");

    assert_eq!(report.moved.len(), 2);
    assert_eq!(report.freed_bytes, 10);
    assert!(!first.exists() && !second.exists());
    for entry in &report.moved {
        assert_inside(root, &entry.destination);
        assert!(entry.destination.exists());
    }

    let verification = verify(&report);
    assert!(verification.is_complete());
    assert_eq!(verification.verified, 2);
    assert_eq!(verification.verified_bytes, 10);
}

#[test]
fn rollback_restores_every_file_already_moved() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let first = root.join("a.bin");
    fs::write(&first, b"alpha").expect("writable");
    // A target that cannot exist: its parent directory is never created.
    let missing = root.join("absent-dir").join("missing.bin");
    assert_inside(root, &first);
    assert_inside(root, &missing);

    let authorized = authorized_plan_for(5);
    let confirmed = confirmation_for(&authorized);
    let error = execute(
        &authorized,
        &confirmed,
        &DirectoryTrash::new(&trash_dir),
        &[first.clone(), missing.clone()],
    )
    .expect_err("the second target does not exist");

    match error {
        ExecuteError::RolledBack {
            failed,
            detail,
            restored,
        } => {
            assert_eq!(failed, missing, "the error names the target that failed");
            assert_eq!(restored, 1, "exactly one entry had been moved");
            assert!(!detail.is_empty(), "the original failure is described");
        }
        other => panic!("expected ExecuteError::RolledBack, got {other:?}"),
    }

    assert_eq!(
        fs::read(&first).expect("the file is back at its source"),
        b"alpha",
        "restored bytes must be the original bytes"
    );
    assert!(
        !trash_dir.join("a.bin").exists(),
        "a restored file must not also remain in the trash"
    );
}

/// A resolver that lets the first target through and, on the second, plants a
/// non-empty directory at the first target's original path before failing.
///
/// That is the smallest honest reproduction of "the restore is impossible":
/// the entry is safely in quarantine, but its original path is now occupied by
/// something this crate must never delete.
struct SabotageAfterFirstMove {
    inner: DirectoryTrash,
    first_source: PathBuf,
    calls: Cell<usize>,
}

impl TrashDestination for SabotageAfterFirstMove {
    fn destination_for(&self, source: &Path) -> io::Result<PathBuf> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call == 0 {
            return self.inner.destination_for(source);
        }
        fs::create_dir_all(&self.first_source)?;
        fs::write(self.first_source.join("keep.txt"), b"keep")?;
        Err(io::Error::other("injected resolver failure"))
    }
}

#[test]
fn partial_rollback_names_the_file_it_could_not_restore_and_never_returns_ok() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let first = root.join("a.bin");
    let second = root.join("b.bin");
    fs::write(&first, b"alpha").expect("writable");
    fs::write(&second, b"beta").expect("writable");
    assert_inside(root, &first);
    assert_inside(root, &second);

    let resolver = SabotageAfterFirstMove {
        inner: DirectoryTrash::new(&trash_dir),
        first_source: first.clone(),
        calls: Cell::new(0),
    };
    let authorized = authorized_plan_for(9);
    let confirmed = confirmation_for(&authorized);
    let error = execute(
        &authorized,
        &confirmed,
        &resolver,
        &[first.clone(), second.clone()],
    )
    .expect_err("a failed restore must never be reported as success");

    match error {
        ExecuteError::PartialRollback {
            restored,
            unrestored,
        } => {
            assert_eq!(restored, 0, "no entry could be put back");
            assert_eq!(unrestored.len(), 1);
            assert_eq!(unrestored[0].original, first, "the exact original path");
            assert_eq!(
                unrestored[0].quarantined,
                trash_dir.join("a.bin"),
                "the exact current location"
            );
            assert!(
                unrestored[0].detail.contains("not replaced"),
                "the reason must say the occupied path was not overwritten, got {:?}",
                unrestored[0].detail
            );
        }
        other => panic!("expected ExecuteError::PartialRollback, got {other:?}"),
    }

    // The file is not lost and not silently at its source: it is exactly where
    // the error says it is.
    assert_eq!(
        fs::read(trash_dir.join("a.bin")).expect("readable"),
        b"alpha",
        "the unrestored file is still intact in the trash"
    );
    assert!(
        first.is_dir(),
        "the blocker that prevented the restore exists"
    );
    assert_eq!(
        fs::read(first.join("keep.txt")).expect("readable"),
        b"keep",
        "the blocker itself was never overwritten"
    );
}

#[test]
fn an_empty_target_list_is_refused_before_anything_is_touched() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let authorized = authorized_plan_for(0);
    let confirmed = confirmation_for(&authorized);

    let error = execute(
        &authorized,
        &confirmed,
        &DirectoryTrash::new(&trash_dir),
        &[],
    )
    .expect_err("nothing to do is an error, not a success");

    assert!(matches!(error, ExecuteError::NothingToDo), "got {error:?}");
    assert!(
        !trash_dir.exists(),
        "the resolver must not even be asked for a destination"
    );
    assert_inside(root, &trash_dir);
}

#[test]
fn a_repeated_target_is_refused_before_the_first_move() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let first = root.join("a.bin");
    fs::write(&first, b"alpha").expect("writable");
    assert_inside(root, &first);

    let authorized = authorized_plan_for(5);
    let confirmed = confirmation_for(&authorized);
    let error = execute(
        &authorized,
        &confirmed,
        &DirectoryTrash::new(&trash_dir),
        &[first.clone(), first.clone()],
    )
    .expect_err("the same path twice would fail against itself");

    match error {
        ExecuteError::DuplicateTarget { target } => assert_eq!(target, first),
        other => panic!("expected ExecuteError::DuplicateTarget, got {other:?}"),
    }
    assert_eq!(fs::read(&first).expect("readable"), b"alpha");
}

#[test]
fn a_resolver_failure_on_the_first_target_reports_a_zero_entry_rollback() {
    struct Refusing;

    impl TrashDestination for Refusing {
        fn destination_for(&self, _source: &Path) -> io::Result<PathBuf> {
            Err(io::Error::other("no destination"))
        }
    }

    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let first = root.join("a.bin");
    fs::write(&first, b"alpha").expect("writable");
    assert_inside(root, &first);

    let authorized = authorized_plan_for(5);
    let confirmed = confirmation_for(&authorized);
    let error = execute(
        &authorized,
        &confirmed,
        &Refusing,
        std::slice::from_ref(&first),
    )
    .expect_err("the resolver refused");

    match error {
        ExecuteError::RolledBack { restored, .. } => assert_eq!(restored, 0),
        other => panic!("expected ExecuteError::RolledBack, got {other:?}"),
    }
    assert_eq!(fs::read(&first).expect("readable"), b"alpha");
}
