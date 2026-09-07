//! Cancellation semantics and event-stream invariants.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{opts, run_fake, FakeFs, Recording};
use spacelens_engine::progress::ScanEvent;
use spacelens_engine::summary::ScanStatus;
use spacelens_engine::CancelHandle;

#[test]
fn cancel_before_start_yields_typed_cancelled_state() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/cancel-first");
    fs.add_dir(&root);
    fs.add_file(&root.join("x.txt"), 1);

    let cancel = CancelHandle::new();
    cancel.cancel();
    let rec = run_fake(&fs, &root, opts(), &cancel);
    assert_eq!(rec.summary().status, ScanStatus::Cancelled);
    assert_eq!(rec.entries().count(), 0);
    assert!(matches!(rec.terminal(), Some(ScanEvent::Cancelled(_))));
}

#[test]
fn cancel_during_scan_stops_and_reports_cancelled() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/cancel-mid");
    fs.add_dir(&root);
    for d in 0..20 {
        let dir = root.join(format!("d{d:02}"));
        fs.add_dir(&dir);
        for f in 0..50 {
            fs.add_file(&dir.join(format!("f{f:02}.txt")), 1);
        }
    }
    let total_files = 20u64 * 50;

    let cancel = CancelHandle::new();
    let mut rec = Recording { events: Vec::new() };
    let summary = spacelens_engine::scan_with(fs.as_ref(), &root, opts(), &cancel, &mut |e| {
        if let ScanEvent::Entry(_) = &e {
            // Cancel once a meaningful slice of the tree has streamed.
            if rec.entries().count() == 40 {
                cancel.cancel();
            }
        }
        rec.events.push(e);
    });
    assert_eq!(summary.status, ScanStatus::Cancelled);
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Cancelled);
    assert!(
        s.files < total_files,
        "cancellation must stop before the whole tree: {} < {total_files}",
        s.files
    );
    assert!(matches!(rec.terminal(), Some(ScanEvent::Cancelled(_))));
}

#[test]
fn event_stream_starts_and_ends_exactly_once() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/events");
    fs.add_dir(&root);
    fs.add_file(&root.join("a"), 1);
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());

    assert!(matches!(rec.events.first(), Some(ScanEvent::Started)));
    let terminals = rec
        .events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ScanEvent::Completed(_) | ScanEvent::Cancelled(_) | ScanEvent::Failed(_)
            )
        })
        .count();
    assert_eq!(terminals, 1, "exactly one terminal event");
    assert!(matches!(rec.events.last(), Some(ScanEvent::Completed(_))));

    // Progress snapshots (if any) are monotonic in files_seen.
    let mut last = 0u64;
    for e in &rec.events {
        if let ScanEvent::Progress(p) = e {
            assert!(p.files_seen >= last, "progress must be monotonic");
            last = p.files_seen;
        }
    }
}

#[test]
fn large_logical_sizes_beyond_4gib() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/huge");
    fs.add_dir(&root);
    let big: u64 = 6 * 1024 * 1024 * 1024 + 1; // 6 GiB + 1
    let huge: u64 = 1 << 42; // 4 TiB
    fs.add_file_ex(&root.join("big.bin"), big, Some(1024));
    fs.add_file_ex(&root.join("huge.bin"), huge, Some(2048));
    fs.add_file(&root.join("empty.bin"), 0);

    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.files, 3);
    assert_eq!(
        s.bytes,
        big + huge,
        "u64 byte accounting must not overflow or truncate"
    );
    assert_eq!(s.allocated_bytes, Some(1024 + 2048));
}

#[test]
fn file_root_is_a_one_entry_scan() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/froot");
    fs.add_dir(&PathBuf::from("/"));
    fs.add_file(&root, 123);
    let rec = run_fake(&fs, &root, opts(), &CancelHandle::new());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.files, 1);
    assert_eq!(s.bytes, 123);
    assert_eq!(s.dirs, 0);
}

#[test]
fn stress_thousands_of_entries_no_loss_no_duplicates() {
    let fs = Arc::new(FakeFs::new());
    let root = PathBuf::from("/stress");
    fs.add_dir(&root);
    const DIRS: u64 = 100;
    const FILES_PER_DIR: u64 = 30;
    for d in 0..DIRS {
        let dir = root.join(format!("dir-{d:03}"));
        fs.add_dir(&dir);
        for f in 0..FILES_PER_DIR {
            fs.add_file(&dir.join(format!("file-{f:02}.bin")), f + 1);
        }
    }
    let rec = run_fake(
        &fs,
        &root,
        spacelens_engine::ScanOptions {
            threads: 4,
            ..Default::default()
        },
        &CancelHandle::new(),
    );
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.files, DIRS * FILES_PER_DIR);
    assert_eq!(s.dirs, DIRS + 1);
    let ids: Vec<u64> = rec.entries().map(|e| e.id).collect();
    let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "no duplicate ids under concurrency"
    );
    assert_eq!(
        rec.entries().count() as u64,
        s.files + s.dirs,
        "every entry delivered exactly once"
    );
}
