//! Classification performance benchmark (ignored by default; run with
//! `cargo test -p spacelens-classifier -- --ignored --nocapture`).
//!
//! Methodology (master prompt §28): synthetic entries (no disk I/O), measured
//! wall-clock for classification + streaming aggregation at 10k / 100k / 1M
//! entries. No pass/fail throughput gate — the requirements are that it
//! completes, is deterministic, memory stays bounded (aggregator + tracker
//! are O(categories/capacity), never O(entries)), and results stay correct.
//! Timing is printed for the record only.

use spacelens_classifier::{
    classify_streaming, Category, CategoryAggregator, ParentContextTracker, Platform,
};
use spacelens_engine::{EntryKind, FsEntry};
use std::path::PathBuf;
use std::time::Instant;

/// Deterministic synthetic entry generator mixing hit and miss patterns.
fn synthetic_entry(i: u64) -> FsEntry {
    let (name, kind) = match i % 10 {
        0 => ("node_modules".to_string(), EntryKind::Dir),
        1 => (format!("document_{i}.pdf"), EntryKind::File),
        2 => ("cache".to_string(), EntryKind::Dir),
        3 => (format!("photo_{i}.png"), EntryKind::File),
        4 => ("tmp".to_string(), EntryKind::Dir),
        5 => (format!("movie_{i}.mkv"), EntryKind::File),
        6 => (format!("archive_{i}.zip"), EntryKind::File),
        7 => (format!("unknown_blob_{i}.xyzzy"), EntryKind::File),
        8 => (format!("app_{i}.rs"), EntryKind::File),
        _ => (format!("mystery_{i}"), EntryKind::Dir),
    };
    FsEntry {
        id: i,
        parent_id: if i == 0 { None } else { Some(i / 10) },
        path: PathBuf::from(format!("/synthetic/{i}/{name}")),
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

fn run(n: u64) -> (u128, CategoryAggregator) {
    let mut tracker = ParentContextTracker::new();
    let mut agg = CategoryAggregator::new();
    let start = Instant::now();
    for i in 0..n {
        let e = synthetic_entry(i);
        let c = classify_streaming(&e, Platform::Linux, &mut tracker);
        agg.push(&c, &e.kind, e.size);
    }
    (start.elapsed().as_millis(), agg)
}

#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn classification_throughput() {
    for n in [10_000u64, 100_000, 1_000_000] {
        let (ms, agg) = run(n);
        let per_sec = if ms > 0 { n * 1000 / (ms as u64) } else { n };
        println!("classified {n} entries in {ms} ms (~{per_sec} entries/sec)");
        // Correctness anchors (not timing gates):
        let report = agg.report();
        let total: u64 = report.categories.iter().map(|(_, t)| t.entries).sum();
        assert_eq!(total, n, "every entry must be aggregated exactly once");
        let dev = agg.totals(Category::Development);
        assert!(dev.entries > 0, "synthetic workload must hit Development");
        // Determinism: rerun the small case and compare reports.
        if n == 10_000 {
            let (ms2, agg2) = run(n);
            assert_eq!(
                agg.report().categories,
                agg2.report().categories,
                "aggregation must be deterministic"
            );
            println!("  (determinism check rerun: {ms2} ms)");
        }
    }
}
