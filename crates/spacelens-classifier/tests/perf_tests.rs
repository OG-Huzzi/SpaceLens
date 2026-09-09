//! Classification performance benchmark (ignored by default; run with
//! `cargo test -p spacelens-classifier -- --ignored --nocapture`).
//!
//! Methodology (master prompt §28): synthetic entries (no disk I/O), measured
//! wall-clock for classification + streaming aggregation at 10k / 100k / 1M
//! entries. There is no pass/fail throughput gate — the requirements are that
//! it completes, is deterministic, scaling stays approximately linear, memory
//! stays bounded (aggregator + tracker are O(categories/capacity), never
//! O(entries)), and results stay correct. Timings are printed for the record.
//!
//! The workload is deliberately *varied*: rooted platform locations (so the
//! location rules run), conflicting names and extensions (so the gate and the
//! tie-break run), parent/child relationships (so the LRU tracker runs), and
//! both path separators (so host-independent splitting runs).

use spacelens_classifier::{
    classify_streaming, Category, CategoryAggregator, ParentContextTracker, Platform, MAX_EVIDENCE,
};
use spacelens_engine::{EntryKind, FsEntry};
use std::path::PathBuf;
use std::time::Instant;

/// Roots that exercise the authoritative location table on each platform.
const ROOTS: &[(&str, Platform)] = &[
    ("C:/Program Files/App", Platform::Windows),
    ("C:/Users/user/AppData/Local/App", Platform::Windows),
    ("C:/Users/user/Downloads", Platform::Windows),
    ("C:/Windows", Platform::Windows),
    ("/Users/user/Library/Application Support/App", Platform::Mac),
    ("/Users/user/Library/Caches/App", Platform::Mac),
    ("/Applications/App.app", Platform::Mac),
    ("/home/user/.cache/app", Platform::Linux),
    ("/home/user/.config/app", Platform::Linux),
    ("/home/user/project", Platform::Linux),
    ("/var/log", Platform::Linux),
    ("/usr/lib", Platform::Linux),
    ("/opt/app", Platform::Linux),
];

/// Names that exercise conflicts, weak heuristics and every extension table.
const FILE_NAMES: &[&str] = &[
    "setup.exe",
    "update.exe",
    "uninstall.exe",
    "update.log",
    "update.txt",
    "setup.zip",
    "setup.pdf",
    "installer.msi",
    "archive.tar.gz",
    "disk.iso",
    "photo.png",
    "movie.mkv",
    "song.flac",
    "report.pdf",
    "main.rs",
    "main.tsx",
    "notes.txt",
    "unknown.bin",
    "noextension",
    ".env.secret",
];

const DIR_NAMES: &[&str] = &[
    "cache",
    "build",
    "out",
    "tmp",
    "logs",
    "backup",
    "node_modules",
    ".git",
    "venv",
    "__pycache__",
    "steamapps",
    "Downloads",
    "Documents",
    "mystery_dir",
];

/// Deterministic synthetic entry generator. Every entry has a parent so the
/// tracker is exercised, and paths are built from real rooted locations.
fn synthetic_entry(i: u64) -> FsEntry {
    let (root, _platform) = ROOTS[(i % ROOTS.len() as u64) as usize];
    let (name, kind) = match i % 3 {
        0 => (
            DIR_NAMES[(i as usize / 3) % DIR_NAMES.len()].to_string(),
            EntryKind::Dir,
        ),
        // Variant 2 reuses a file name but writes it with backslashes: the
        // host-independent splitter must handle it identically on every host.
        _ => (
            FILE_NAMES[(i as usize / 3) % FILE_NAMES.len()].to_string(),
            EntryKind::File,
        ),
    };
    // Mirror the root's own separator on the second file variant.
    let path = if i % 3 == 2 {
        format!("{root}\\{name}")
    } else {
        format!("{root}/{name}")
    };
    FsEntry {
        id: i,
        parent_id: if i == 0 { None } else { Some(i / 4) },
        path: PathBuf::from(path),
        kind,
        size: i * 1024,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        device: None,
        inode: None,
        hidden: false,
        error: None,
    }
}

/// The platform an entry is classified with, derived the same way the
/// generator derived its root — so location rules actually fire.
fn platform_of(i: u64) -> Platform {
    ROOTS[(i % ROOTS.len() as u64) as usize].1
}

fn run(n: u64) -> (u128, CategoryAggregator, ParentContextTracker) {
    let mut tracker = ParentContextTracker::new();
    let mut agg = CategoryAggregator::new();
    let start = Instant::now();
    for i in 0..n {
        let e = synthetic_entry(i);
        let c = classify_streaming(&e, platform_of(i), &mut tracker);
        agg.push(&c, &e.kind, e.size);
    }
    (start.elapsed().as_millis(), agg, tracker)
}

#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn classification_throughput() {
    let mut prev_per_entry = 0.0f64;
    for n in [10_000u64, 100_000, 1_000_000] {
        let (ms, agg, tracker) = run(n);
        let per_sec = if ms > 0 { n * 1000 / (ms as u64) } else { n };
        println!("classified {n} entries in {ms} ms (~{per_sec} entries/sec)");

        // Correctness anchors (not timing gates): every entry is aggregated
        // exactly once, and every category family the workload can reach is
        // actually reached — so this exercises more than one code path.
        let report = agg.report();
        let total: u64 = report.categories.iter().map(|(_, t)| t.entries).sum();
        assert_eq!(total, n, "every entry must be aggregated exactly once");
        assert_eq!(report.categories.len(), Category::COUNT);
        for cat in [
            Category::Development,
            Category::Documents,
            Category::Archives,
            Category::Logs,
            Category::Applications,
            Category::ApplicationData,
        ] {
            assert!(
                agg.totals(cat).entries > 0,
                "the synthetic workload must reach {cat:?}"
            );
        }

        // Memory is bounded by design, not by the workload.
        assert!(tracker.len() <= tracker.capacity());
        assert!(tracker.capacity() <= ParentContextTracker::MAX_ENTRIES);

        // Approximately linear: per-entry cost must not explode by more than
        // an order of magnitude between successive sizes. Generous on purpose
        // — this is a smoke guard against pathological superlinearity, not a
        // benchmark.
        let per_entry = ms as f64 / n as f64;
        if prev_per_entry > 0.0 {
            assert!(
                per_entry < prev_per_entry * 10.0 + 0.01,
                "per-entry cost grew from {prev_per_entry:.6} to {per_entry:.6} ms \
                 between sizes — superlinear behaviour"
            );
        }
        prev_per_entry = per_entry;
    }
}

/// Determinism at scale, plus evidence boundedness. Cheap enough to leave out
/// of the ignored set so a regression is caught by the normal test run.
#[test]
fn workload_is_deterministic_and_bounded() {
    const N: u64 = 20_000;
    let (_, a, _) = run(N);
    let (_, b, _) = run(N);
    assert_eq!(
        a.report().categories,
        b.report().categories,
        "aggregation must be deterministic across runs"
    );
    // Evidence is bounded for every entry in the workload.
    let mut tracker = ParentContextTracker::new();
    for i in 0..N {
        let e = synthetic_entry(i);
        let c = classify_streaming(&e, platform_of(i), &mut tracker);
        assert!(
            c.evidence.len() <= MAX_EVIDENCE,
            "entry {i} exceeded the evidence bound"
        );
    }
    assert!(tracker.len() <= tracker.capacity());
}
