//! Trash/quarantine relocation itself: no overwrite, sidecars, and rollback
//! of a single operation.
//!
//! Contract 1 (never overwrite) and contract 6 (sidecars travel) are proved
//! here, together with the sidecar-failure rollback that contract 5 of the
//! brief implies: a sidecar that cannot move must undo the main file.

mod common;

use std::fs;
use std::path::Path;

use common::assert_inside;
use coresight_actions::{
    move_to_quarantine, move_to_trash_with, DirectoryTrash, MoveError, TrashDestination,
};

#[test]
fn a_trash_move_never_overwrites_an_existing_destination() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source_dir = root.join("source");
    let trash_dir = root.join("trash");
    fs::create_dir_all(&source_dir).expect("the fixture source directory");
    fs::create_dir_all(&trash_dir).expect("the fixture trash directory");

    let source = source_dir.join("report.txt");
    fs::write(&source, b"moved content").expect("the fixture file is writable");
    let pre_existing = trash_dir.join("report.txt");
    fs::write(&pre_existing, b"pre-existing content").expect("the fixture file is writable");
    assert_inside(root, &source);
    assert_inside(root, &pre_existing);

    let moved = move_to_trash_with(&source, &DirectoryTrash::new(&trash_dir))
        .expect("a free name exists next to the taken one");

    assert_eq!(
        moved.destination,
        trash_dir.join("report (1).txt"),
        "the move must derive `name (1).ext` rather than overwrite"
    );
    assert_inside(root, &moved.destination);
    assert_eq!(
        fs::read(&pre_existing).expect("the pre-existing file is still readable"),
        b"pre-existing content",
        "the file that was already there must be untouched"
    );
    assert_eq!(
        fs::read(&moved.destination).expect("the moved file is readable"),
        b"moved content"
    );
    assert!(
        !source.exists(),
        "the source path must be empty after a move"
    );
    assert_eq!(moved.bytes, b"moved content".len() as u64);
}

#[test]
fn repeated_moves_of_the_same_name_derive_the_next_free_name() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    fs::create_dir_all(root.join("first")).expect("the fixture directory");
    fs::create_dir_all(root.join("second")).expect("the fixture directory");

    let first = root.join("first").join("cache.bin");
    let second = root.join("second").join("cache.bin");
    fs::write(&first, b"one").expect("the fixture file is writable");
    fs::write(&second, b"two").expect("the fixture file is writable");
    assert_inside(root, &first);
    assert_inside(root, &second);

    let resolver = DirectoryTrash::new(&trash_dir);
    let first_move = move_to_trash_with(&first, &resolver).expect("the first move");
    let second_move = move_to_trash_with(&second, &resolver).expect("the second move");

    assert_eq!(first_move.destination, trash_dir.join("cache.bin"));
    assert_eq!(second_move.destination, trash_dir.join("cache (1).bin"));
    assert_eq!(
        fs::read(&first_move.destination).expect("readable"),
        b"one",
        "the first move's content must not be replaced by the second"
    );
    assert_eq!(
        fs::read(&second_move.destination).expect("readable"),
        b"two"
    );
    assert_inside(root, &first_move.destination);
    assert_inside(root, &second_move.destination);
}

#[test]
fn a_quarantine_move_creates_its_root_and_avoids_collisions() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let quarantine = root.join("nested").join("quarantine");
    let source_dir = root.join("source");
    fs::create_dir_all(&source_dir).expect("the fixture directory");

    let first = source_dir.join("data.bin");
    fs::write(&first, b"first").expect("the fixture file is writable");
    assert_inside(root, &first);

    let moved = move_to_quarantine(&first, &quarantine).expect("the quarantine root is created");
    assert!(quarantine.is_dir(), "the quarantine root must be created");
    assert_eq!(moved.destination, quarantine.join("data.bin"));
    assert_inside(root, &moved.destination);

    let second = root.join("other").join("data.bin");
    fs::create_dir_all(second.parent().expect("the fixture parent"))
        .expect("the fixture directory");
    fs::write(&second, b"second").expect("the fixture file is writable");
    assert_inside(root, &second);

    let second_move = move_to_quarantine(&second, &quarantine).expect("the second move");
    assert_eq!(second_move.destination, quarantine.join("data (1).bin"));
    assert_eq!(
        fs::read(quarantine.join("data.bin")).expect("readable"),
        b"first"
    );
    assert_eq!(
        fs::read(quarantine.join("data (1).bin")).expect("readable"),
        b"second"
    );
}

#[test]
fn sidecars_travel_with_their_file_under_the_same_destination_name() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source_dir = root.join("source");
    let trash_dir = root.join("trash");
    fs::create_dir_all(&source_dir).expect("the fixture directory");
    fs::create_dir_all(&trash_dir).expect("the fixture directory");

    let database = source_dir.join("data.db");
    let wal = source_dir.join("data.db-wal");
    let shm = source_dir.join("data.db-shm");
    fs::write(&database, b"database").expect("writable");
    fs::write(&wal, b"wal frames").expect("writable");
    fs::write(&shm, b"shm index").expect("writable");
    for path in [&database, &wal, &shm] {
        assert_inside(root, path);
    }

    // A collision on the main name proves the sidecar follows the DERIVED
    // destination name rather than the source name.
    fs::write(trash_dir.join("data.db"), b"squatter").expect("writable");

    let moved = move_to_trash_with(&database, &DirectoryTrash::new(&trash_dir))
        .expect("main file and both sidecars move");

    assert_eq!(moved.destination, trash_dir.join("data (1).db"));
    assert_eq!(
        fs::read(trash_dir.join("data (1).db")).expect("readable"),
        b"database"
    );
    assert_eq!(
        fs::read(trash_dir.join("data (1).db-wal")).expect("the -wal travelled"),
        b"wal frames"
    );
    assert_eq!(
        fs::read(trash_dir.join("data (1).db-shm")).expect("the -shm travelled"),
        b"shm index"
    );
    assert_eq!(
        fs::read(trash_dir.join("data.db")).expect("readable"),
        b"squatter",
        "the pre-existing entry keeps its content"
    );
    assert!(!wal.exists() && !shm.exists() && !database.exists());
    for path in [
        trash_dir.join("data (1).db"),
        trash_dir.join("data (1).db-wal"),
        trash_dir.join("data (1).db-shm"),
    ] {
        assert_inside(root, &path);
    }
}

#[test]
fn a_blocked_sidecar_destination_rolls_the_whole_move_back() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source_dir = root.join("source");
    let trash_dir = root.join("trash");
    fs::create_dir_all(&source_dir).expect("the fixture directory");
    fs::create_dir_all(&trash_dir).expect("the fixture directory");

    let database = source_dir.join("data.db");
    let wal = source_dir.join("data.db-wal");
    fs::write(&database, b"database").expect("writable");
    fs::write(&wal, b"wal frames").expect("writable");
    // A sidecar keeps its database's name, so this occupied name cannot be
    // worked around: the operation must fail and restore the main file.
    fs::write(trash_dir.join("data.db-wal"), b"occupied").expect("writable");
    assert_inside(root, &database);
    assert_inside(root, &wal);

    let error = move_to_trash_with(&database, &DirectoryTrash::new(&trash_dir))
        .expect_err("the sidecar's required name is taken");

    assert!(
        matches!(error, MoveError::SidecarBlocked { .. }),
        "a taken sidecar name is reported as such, got {error:?}"
    );
    assert!(
        database.exists(),
        "the main file must be back at its source after the rollback"
    );
    assert_eq!(
        fs::read(&database).expect("readable"),
        b"database",
        "the restored file must still hold its original bytes"
    );
    assert!(wal.exists(), "the sidecar never moved");
    assert!(
        !trash_dir.join("data.db").exists(),
        "the rolled-back main file must not be left in the trash"
    );
    assert_eq!(
        fs::read(trash_dir.join("data.db-wal")).expect("readable"),
        b"occupied",
        "the entry that blocked the sidecar is untouched"
    );
}

#[test]
fn a_missing_source_moves_nothing() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash_dir = root.join("trash");
    let missing = root.join("never-existed.bin");
    assert_inside(root, &missing);

    let error = move_to_trash_with(&missing, &DirectoryTrash::new(&trash_dir))
        .expect_err("a missing source cannot be moved");

    assert!(
        matches!(error, MoveError::SourceUnavailable { .. }),
        "got {error:?}"
    );
    assert!(
        !trash_dir.exists(),
        "a move that cannot start must not create a destination directory"
    );
}

#[test]
fn a_directory_target_moves_and_reports_its_recursive_size() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source = root.join("cache-dir");
    let trash_dir = root.join("trash");
    fs::create_dir_all(source.join("nested")).expect("the fixture directory");
    fs::write(source.join("one.bin"), b"abc").expect("writable");
    fs::write(source.join("nested").join("two.bin"), b"defg").expect("writable");
    assert_inside(root, &source);

    let moved = move_to_trash_with(&source, &DirectoryTrash::new(&trash_dir))
        .expect("a directory is moved as one entry");

    assert_eq!(moved.destination, trash_dir.join("cache-dir"));
    assert_eq!(moved.bytes, 7, "3 + 4 bytes of regular files");
    assert!(moved.destination.is_dir());
    assert_eq!(
        fs::read(moved.destination.join("nested").join("two.bin")).expect("readable"),
        b"defg"
    );
    assert!(!source.exists());
    assert_inside(root, &moved.destination);
}

#[test]
fn a_resolver_that_refuses_a_destination_moves_nothing() {
    struct Refusing;

    impl TrashDestination for Refusing {
        fn destination_for(&self, source: &Path) -> std::io::Result<std::path::PathBuf> {
            Err(std::io::Error::other(format!(
                "refusing to name a destination for {}",
                source.display()
            )))
        }
    }

    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source = root.join("cache.bin");
    fs::write(&source, b"bytes").expect("writable");
    assert_inside(root, &source);

    let error = move_to_trash_with(&source, &Refusing).expect_err("the resolver refused");

    assert!(
        matches!(error, MoveError::DestinationUnresolved { .. }),
        "got {error:?}"
    );
    assert_eq!(fs::read(&source).expect("readable"), b"bytes");
}
