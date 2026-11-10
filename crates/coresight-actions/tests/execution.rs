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

use common::{
    assert_inside, authorized_plan_for, cleanup_item, cleanup_plan, confirmation_for,
    reversible_policy,
};
use coresight_actions::{
    authorize, execute, verify, ConfirmedPreview, DirectoryTrash, ExecuteError, TrashDestination,
};
use coresight_cleanup::{resolve_plan, ActionKind, ResolveOptions};
use coresight_history::{ClassificationRef, ObservedEntry, ObservedKind, RunId, Snapshot};

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

    let authorized = authorized_plan_for(vec![(first.clone(), 5), (second.clone(), 5)]);
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

    // Both paths are part of the reviewed set: the missing one is authorized
    // so the failure under test is the move, not membership.
    let authorized = authorized_plan_for(vec![(first.clone(), 5), (missing.clone(), 0)]);
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
    let authorized = authorized_plan_for(vec![(first.clone(), 5), (second.clone(), 4)]);
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
    let authorized = authorized_plan_for(vec![(root.join("unused.bin"), 5)]);
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

    let authorized = authorized_plan_for(vec![(first.clone(), 5)]);
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

    let authorized = authorized_plan_for(vec![(first.clone(), 5)]);
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

#[test]
fn execute_rejects_a_target_that_is_not_in_the_authorized_plan() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let reviewed = root.join("reviewed.bin");
    let unreviewed = root.join("unreviewed.bin");
    fs::write(&reviewed, b"12345").expect("writable");
    fs::write(&unreviewed, b"67890").expect("writable");
    assert_inside(root, &reviewed);
    assert_inside(root, &unreviewed);

    // The user reviewed exactly one path.
    let authorized = authorized_plan_for(vec![(reviewed.clone(), 5)]);
    assert_eq!(
        authorized.authorized_paths().collect::<Vec<_>>(),
        vec![reviewed.as_path()],
        "the token freezes exactly the reviewed path"
    );
    assert!(authorized.is_path_authorized(&reviewed));
    assert!(
        !authorized.is_path_authorized(&unreviewed),
        "a path outside the resolved plan is not authorized"
    );

    let confirmed = confirmation_for(&authorized);
    // The reviewed path comes FIRST, so the test proves the whole list is
    // validated before the first move rather than stopping at the bad entry.
    let error = execute(
        &authorized,
        &confirmed,
        &DirectoryTrash::new(&trash_dir),
        &[reviewed.clone(), unreviewed.clone()],
    )
    .expect_err("a caller-resolved wider path set must be refused");

    match error {
        ExecuteError::TargetNotInPlan { path } => assert_eq!(path, unreviewed),
        other => panic!("expected ExecuteError::TargetNotInPlan, got {other:?}"),
    }
    assert!(
        reviewed.exists(),
        "the authorized first target must not move when any target is unauthorized"
    );
    assert!(
        unreviewed.exists(),
        "the unreviewed file must not be touched"
    );
    assert!(
        !trash_dir.exists(),
        "membership is checked before any destination is resolved"
    );

    // Not a vacuous fixture: the same token and confirmation still execute the
    // reviewed subset.
    let report = execute(
        &authorized,
        &confirmed,
        &DirectoryTrash::new(&trash_dir),
        std::slice::from_ref(&reviewed),
    )
    .expect("the reviewed subset is authorized");
    assert_eq!(report.moved.len(), 1);
    assert!(!reviewed.exists());
    assert_inside(root, &report.moved[0].destination);
}

#[test]
fn execute_moves_exactly_the_targets_resolved_from_a_snapshot() {
    fn observed(path: &Path, size: u64) -> ObservedEntry {
        ObservedEntry {
            path: path.to_path_buf(),
            kind: ObservedKind::File,
            size: Some(size),
            object: None,
            modified: None,
            classification: Some(ClassificationRef {
                category: "CACHE".to_string(),
                subcategory: None,
            }),
            content_sha256: None,
            observation_error: None,
        }
    }

    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash = DirectoryTrash::new(root.join("trash"));
    let first = root.join("cache").join("one.bin");
    let second = root.join("cache").join("two.bin");
    fs::create_dir_all(first.parent().expect("the fixture parent")).expect("the fixture directory");
    fs::write(&first, b"11111").expect("writable");
    fs::write(&second, b"222").expect("writable");
    assert_inside(root, &first);
    assert_inside(root, &second);

    // A real file that the snapshot never observed: resolution cannot
    // authorize it, so it must be refused even though it exists.
    let unobserved = root.join("not-observed.bin");
    fs::write(&unobserved, b"zz").expect("writable");
    assert_inside(root, &unobserved);

    let snapshot = Snapshot {
        run_id: RunId("run:resolve".to_string()),
        entries: vec![observed(&first, 5), observed(&second, 3)],
    };
    let plan = cleanup_plan(
        "plan:resolved",
        vec![cleanup_item("CACHE:8", 8, ActionKind::MoveToTrash)],
    );
    let resolved = resolve_plan(&plan, &snapshot, &ResolveOptions::default())
        .expect("both observed files resolve");
    assert_eq!(resolved.target_count, 2);
    assert_eq!(resolved.total_bytes, 8);
    let authorized = authorize(plan, resolved, reversible_policy())
        .expect("the resolved plan is reversible and complete");
    let confirmed = ConfirmedPreview::from_resolved(authorized.resolved());
    let targets: Vec<PathBuf> = authorized
        .authorized_paths()
        .map(Path::to_path_buf)
        .collect();
    assert_eq!(targets.len(), 2);

    let mut widened = targets.clone();
    widened.push(unobserved.clone());
    let error = execute(&authorized, &confirmed, &trash, &widened)
        .expect_err("an unobserved path must not be executable");
    match error {
        ExecuteError::TargetNotInPlan { path } => assert_eq!(path, unobserved),
        other => panic!("expected ExecuteError::TargetNotInPlan, got {other:?}"),
    }
    assert!(first.exists() && second.exists() && unobserved.exists());

    let report = execute(&authorized, &confirmed, &trash, &targets)
        .expect("exactly the resolved paths execute");
    assert_eq!(report.moved.len(), 2);
    assert_eq!(report.freed_bytes, 8);
    assert!(!first.exists() && !second.exists());
    assert!(unobserved.exists(), "the unobserved file was never touched");
}
