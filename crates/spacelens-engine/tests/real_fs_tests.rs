//! Integration tests against REAL filesystem fixtures.
//!
//! Every fixture is created and destroyed by the test inside a temp dir —
//! never real home directories, never personal data (docs/TESTING_STRATEGY.md).
//! Tests that need link-creation privileges skip with a notice when the OS
//! refuses; CI runners (admin) run them fully.

use std::fs;
use std::path::Path;

use spacelens_engine::model::EntryKind;
use spacelens_engine::progress::ScanEvent;
use spacelens_engine::summary::ScanStatus;
use spacelens_engine::{scan, CancelHandle, ScanOptions, ScanSummary};

pub(crate) struct Recording {
    pub events: Vec<ScanEvent>,
}

impl Recording {
    pub fn entries(&self) -> impl Iterator<Item = &spacelens_engine::FsEntry> {
        self.events.iter().filter_map(|e| match e {
            ScanEvent::Entry(entry) => Some(entry.as_ref()),
            _ => None,
        })
    }

    pub fn summary(&self) -> &ScanSummary {
        self.events
            .iter()
            .find_map(|e| match e {
                ScanEvent::Completed(s) | ScanEvent::Cancelled(s) | ScanEvent::Failed(s) => {
                    Some(s.as_ref())
                }
                _ => None,
            })
            .expect("scan must emit exactly one terminal event")
    }
}

pub(crate) fn scan_dir(root: &Path) -> Recording {
    let mut rec = Recording { events: Vec::new() };
    let summary = scan(
        root,
        ScanOptions {
            threads: 2,
            ..ScanOptions::default()
        },
        &CancelHandle::new(),
        &mut |e| rec.events.push(e),
    );
    assert_eq!(rec.summary().status, summary.status);
    rec
}

/// Creates a symlink that works on the current OS; returns false when the
/// environment cannot really produce one, so callers can skip gracefully.
///
/// The existence re-check matters: some Windows hosts (filter drivers, AV,
/// certain Dev Drive configurations) report `symlink_file` as successful
/// without a reparse point ever appearing on disk. Trusting the return value
/// alone made this suite fail on such hosts even though the engine behaved
/// correctly, so "created" is defined as "the link is observably there".
pub(crate) fn create_link(target: &Path, link: &Path, dir_link: bool) -> bool {
    #[cfg(unix)]
    {
        let _ = dir_link;
        std::os::unix::fs::symlink(target, link).is_ok()
            && std::fs::symlink_metadata(link)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let result = if dir_link {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        result.is_ok()
            && std::fs::symlink_metadata(link)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link, dir_link);
        false
    }
}

#[test]
fn real_nested_tree_metadata_and_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir(root.join("sub")).unwrap();
    fs::create_dir(root.join("sub").join("deeper")).unwrap();
    fs::write(root.join("a.txt"), vec![0u8; 1_234]).unwrap();
    fs::write(root.join("sub").join("b.bin"), vec![7u8; 56_789]).unwrap();
    fs::write(root.join("sub").join("deeper").join("c.txt"), b"tiny").unwrap();

    let rec = scan_dir(&root);
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.files, 3);
    assert_eq!(s.dirs, 3, "root + sub + deeper");
    assert_eq!(s.bytes, 1_234 + 56_789 + 4);

    let a = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "a.txt")
        .unwrap();
    assert_eq!(a.size, 1_234);
    assert!(a.modified.is_some(), "real fs must report mtime");
    assert!(matches!(a.kind, EntryKind::File));
    let deeper = rec
        .entries()
        .find(|e| e.path.file_name().unwrap() == "deeper")
        .unwrap();
    assert!(matches!(deeper.kind, EntryKind::Dir));
}

#[test]
fn real_empty_and_hidden_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("empty")).unwrap();
    fs::write(root.join(".dotfile"), b"x").unwrap();
    fs::write(root.join("regular"), b"y").unwrap();

    let rec = scan_dir(root);
    let s = rec.summary();
    assert_eq!(s.dirs, 2);
    assert_eq!(s.files, 2);

    #[cfg(unix)]
    {
        let dot = rec
            .entries()
            .find(|e| e.path.file_name().unwrap() == ".dotfile")
            .unwrap();
        assert!(dot.hidden, "dot-names are hidden on Unix");
    }
}

#[test]
fn real_unicode_and_space_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let names = [
        "a file with spaces.txt",
        "日本語.txt",
        "emoji \u{1f600}.dat",
    ];
    for n in names {
        fs::write(root.join(n), b"data").unwrap();
    }
    let rec = scan_dir(root);
    let s = rec.summary();
    assert_eq!(s.files, names.len() as u64);
    for n in names {
        assert!(
            rec.entries().any(|e| e.path.file_name().unwrap() == n),
            "missing {n:?}"
        );
    }
}

#[test]
fn real_links_recorded_without_recursion() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("target")).unwrap();
    fs::write(root.join("target").join("inside.txt"), b"x").unwrap();
    fs::write(root.join("plain.txt"), b"y").unwrap();

    let file_link_ok = create_link(&root.join("plain.txt"), &root.join("file-link"), false);
    let dir_link_ok = create_link(&root.join("target"), &root.join("dir-link"), true);
    let broken_ok = create_link(
        &root.join("no-such-target"),
        &root.join("broken-link"),
        false,
    );
    if !file_link_ok || !dir_link_ok || !broken_ok {
        eprintln!(
            "SKIP-PARTIAL: link creation refused by OS (file={file_link_ok} dir={dir_link_ok} \
             broken={broken_ok}); full link-policy coverage runs on privileged CI runners"
        );
    }

    let rec = scan_dir(root);
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(
        s.files, 2,
        "plain.txt + inside.txt — inside.txt must be counted exactly once (never via the link)"
    );
    assert_eq!(
        s.links,
        (file_link_ok as u64) + (dir_link_ok as u64) + (broken_ok as u64)
    );

    if dir_link_ok {
        let dl = rec
            .entries()
            .find(|e| e.path.file_name().unwrap() == "dir-link")
            .unwrap();
        assert!(matches!(dl.kind, EntryKind::Link(_)));
    }
    if broken_ok {
        let bl = rec
            .entries()
            .find(|e| e.path.file_name().unwrap() == "broken-link")
            .unwrap();
        match &bl.kind {
            EntryKind::Link(info) => assert!(info.broken),
            other => panic!("expected broken link, got {other:?}"),
        }
    }
}

#[test]
fn real_long_paths_beyond_260_chars() {
    let dir = tempfile::tempdir().unwrap();
    let mut root = dir.path().to_path_buf();
    // Build a path well beyond MAX_PATH given a ~40-char temp prefix.
    for i in 0..12 {
        root = root.join(format!("lvl-{i:02}-{}", "d".repeat(24)));
    }
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("leaf.bin"), b"reachable").unwrap();

    let rec = scan_dir(dir.path());
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert!(
        rec.entries()
            .any(|e| e.path.file_name().unwrap() == "leaf.bin"),
        "long-path leaf must be reachable (path length > 260)"
    );
    assert_eq!(s.files, 1);
}

#[test]
fn real_large_logical_file_beyond_4gib() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    // set_len extends the file logically; NTFS still reserves the extent, so
    // on a near-full disk this must skip honestly rather than fake success.
    // CI runners (large disks) execute the assertion path.
    let f = fs::File::create(&path).unwrap();
    if let Err(e) = f.set_len(5 * 1024 * 1024 * 1024 + 123) {
        if e.kind() == std::io::ErrorKind::StorageFull || e.raw_os_error() == Some(112) {
            eprintln!("SKIP: not enough disk space to stage a >4 GiB logical file here");
            return;
        }
        panic!("unexpected set_len failure: {e}");
    }
    drop(f);

    let rec = scan_dir(dir.path());
    let s = rec.summary();
    assert_eq!(s.files, 1);
    assert_eq!(
        s.bytes,
        5 * 1024 * 1024 * 1024 + 123,
        ">4 GiB size must be exact"
    );
}

#[test]
fn real_progress_throttled_and_completion_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("d")).unwrap();
    for i in 0..50 {
        fs::write(root.join("d").join(format!("f{i:03}.txt")), b"z").unwrap();
    }
    let rec = scan_dir(root);
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
    assert_eq!(terminals, 1);
    assert!(matches!(rec.events.last(), Some(ScanEvent::Completed(_))));
    // A fast scan may legitimately emit zero interim Progress events; any
    // that appear must be monotonic in files_seen.
    let mut last = 0u64;
    for e in &rec.events {
        if let ScanEvent::Progress(p) = e {
            assert!(p.files_seen >= last);
            last = p.files_seen;
        }
    }
}

/// Scale smoke: correctness at 10k files. Timing is *printed*, never a pass
/// criterion (docs/TESTING_STRATEGY.md forbids wall-clock pass gates). Run:
///   cargo test -p spacelens-engine -- --ignored --nocapture
#[test]
#[ignore = "perf smoke: run explicitly; correctness at scale, timing reported not asserted"]
fn perf_smoke_10k_files_correct_and_timed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    const DIRS: usize = 20;
    const FILES: usize = 500;
    for d in 0..DIRS {
        let sub = root.join(format!("dir{d:02}"));
        fs::create_dir(&sub).unwrap();
        for i in 0..FILES {
            fs::write(sub.join(format!("f{i:04}.txt")), b"payload").unwrap();
        }
    }
    let started = std::time::Instant::now();
    let rec = scan_dir(root);
    let elapsed = started.elapsed();
    let s = rec.summary();
    assert_eq!(s.status, ScanStatus::Completed);
    assert_eq!(s.files, (DIRS * FILES) as u64);
    assert_eq!(s.dirs, (DIRS + 1) as u64);
    println!(
        "perf-smoke: {} files / {} dirs scanned in {:?} (~{} files/sec)",
        s.files,
        s.dirs,
        elapsed,
        s.files * 1000 / elapsed.as_millis().max(1) as u64
    );
}
