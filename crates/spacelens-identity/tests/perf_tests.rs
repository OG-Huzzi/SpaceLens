//! Identity/duplicate pipeline performance benchmark (ignored by default;
//! run with `cargo test -p spacelens-identity -- --ignored --nocapture`).
//!
//! Methodology (mirrors the Phase 2 smoke): synthetic entries and an
//! O(1)-memory synthetic content factory — content bytes are derived from
//! the path at read time (nothing is pre-generated, no disk is touched), so
//! the harness itself stays bounded and honest.
//!
//! Four hostile workloads at 10k / 100k / 1M entries:
//! - **mostly unique**: distinct content per file (candidate filter does the
//!   work; hashing is minimal),
//! - **same-size different-content**: every file collides on size (maximum
//!   hashing work, zero groups),
//! - **many true duplicates**: pairs of identical files everywhere (maximum
//!   grouping work),
//! - **many zero-byte files**: the degenerate identity (zero-byte policy
//!   exercise).
//!
//! No pass/fail throughput gate — the requirements are that it completes,
//! is deterministic, scaling stays approximately linear, and memory stays
//! bounded. Timings are printed for the record only.
//!
//! Also home to `huge_file_streams_beyond_4gib` (ignored): the true >4 GiB
//! streaming-contract proof, virtualized so no platform needs multi-GiB
//! disk space to verify it.

use spacelens_engine::{CancelHandle, FsEntry};
use spacelens_identity::{
    run_duplicates, ContentReaderFactory, DuplicateOptions, DuplicateProgressEvent, DuplicateStatus,
};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use spacelens_engine::platform::{ContentError, ContentReader};

/// Deterministic synthetic entry generator. `family` selects the workload.
/// The size is embedded in the path (`sNNNN.sbin`) so the content factory
/// can re-derive it per-path from any thread (worker threads never see the
/// harness's thread-local state).
fn synthetic_entry(i: u64, family: &str, total: u64) -> FsEntry {
    let size = match family {
        "mostly-unique" => 4096 + (i % 16), // mostly distinct sizes
        "same-size" => 1024,                // every file same size
        "many-duplicates" => 2048,          // every file same size, pairs equal
        "many-zero" => 0,
        _ => unreachable!("unknown family"),
    };
    // Content key: what bytes the file holds. Pairs share it (true
    // duplicates); every other family is per-file unique.
    let content_key = if family == "many-duplicates" {
        i - (i % 2)
    } else {
        i
    };
    let copy = i % 2; // distinct path, same content, per pair
    FsEntry {
        id: i,
        parent_id: None,
        path: PathBuf::from(format!(
            "/synthetic/{family}/batch{}/s{:06}.sbin/k{content_key:016}/file-{:07}-c{copy}.bin",
            i / (total.max(1) / 10 + 1),
            size,
            i
        )),
        kind: spacelens_engine::EntryKind::File,
        size,
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

/// Content factory over synthetic entries: derives bytes AND length from
/// the path at read time (the generator embeds the size in the path).
/// O(1) memory — no file is ever created or stored, any thread works.
struct SyntheticReader;

impl SyntheticReader {
    fn new() -> Self {
        SyntheticReader
    }
}

/// Extract the embedded size from a synthetic path (`/sNNNNNN.sbin/`).
fn synthetic_size_of(path: &Path) -> u64 {
    path.components()
        .find_map(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.strip_prefix('s')
                .and_then(|r| r.strip_suffix(".sbin"))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

/// FNV-1a 64-bit over the content-key path segment. Two different keys
/// collide with negligible probability; the same key always yields the same
/// pattern — exactly the guarantee the harness needs.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Extract the content key from a synthetic path (`/kNNNN.../`).
fn synthetic_key_of(path: &Path) -> u64 {
    path.components()
        .find_map(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.strip_prefix('k').and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(u64::MAX)
}

impl ContentReaderFactory for SyntheticReader {
    fn read(
        &self,
        path: &Path,
        feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
    ) -> Result<(), ContentError> {
        let size = synthetic_size_of(path);
        let key = synthetic_key_of(path);
        // 8-byte repeating pattern unique to the content key, expanded
        // once into a fill buffer (pattern continuity across chunks falls
        // out of the buffer length being a multiple of 8).
        let pattern = fnv1a(&key.to_le_bytes()).to_le_bytes();
        let fill = pattern
            .iter()
            .cycle()
            .take(4096)
            .copied()
            .collect::<Vec<u8>>();
        feed(&mut SyntheticChunks {
            size,
            served: 0,
            fill,
        })
        .map_err(ContentError::ReadFailed)
    }
}

/// Serves exactly `size` bytes of deterministic content derived from the
/// path seed — the pipeline's length/mutation checks see an honest stream.
/// Fill is a bulk patterned-buffer copy, not per-byte arithmetic, so debug
/// builds run near the SHA-256 floor rather than the modulo floor.
struct SyntheticChunks {
    size: u64,
    served: u64,
    fill: Vec<u8>,
}

impl ContentReader for SyntheticChunks {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        if self.served >= self.size {
            return Ok(None);
        }
        let n = buf.len().min((self.size - self.served) as usize);
        let mut done = 0usize;
        while done < n {
            let take = (n - done).min(self.fill.len());
            buf[done..done + take].copy_from_slice(&self.fill[..take]);
            done += take;
        }
        self.served += n as u64;
        Ok(Some(n))
    }
    fn file_identity(&self) -> spacelens_engine::FileIdentity {
        spacelens_engine::FileIdentity::unknown()
    }
    fn file_len(&self) -> io::Result<u64> {
        Ok(self.size)
    }
}

/// Runs one workload end-to-end and returns (ms, report).
fn run_workload(family: &str, n: u64) -> (u128, spacelens_identity::DuplicateReport) {
    let entries: Vec<FsEntry> = (0..n).map(|i| synthetic_entry(i, family, n)).collect();
    let reader = SyntheticReader::new();
    let cancel = CancelHandle::new();
    let start = Instant::now();
    let mut terminal = None;
    let report = run_duplicates(
        entries.into_iter(),
        &DuplicateOptions {
            group_zero_byte_files: family == "many-zero",
            ..DuplicateOptions::default()
        },
        &cancel,
        Some(&reader),
        &mut |e: DuplicateProgressEvent| match e {
            DuplicateProgressEvent::Completed(r)
            | DuplicateProgressEvent::Cancelled(r)
            | DuplicateProgressEvent::Failed(r) => terminal = Some(r.status),
            _ => {}
        },
    );
    (start.elapsed().as_millis(), report)
}

#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn duplicate_pipeline_throughput() {
    // Scale guard on the small sizes (1M hashing is deliberately omitted
    // for the byte-heavy families; 1M runs on the two cheap families).
    for family in ["mostly-unique", "same-size", "many-duplicates", "many-zero"] {
        let mut prev_per_entry = 0.0f64;
        let sizes: &[u64] = if family == "same-size" || family == "many-duplicates" {
            &[10_000, 100_000]
        } else {
            &[10_000, 100_000, 1_000_000]
        };
        for &n in sizes {
            let (ms, report) = run_workload(family, n);
            let per_sec = if ms > 0 {
                n * 1000 / ms.max(1) as u64
            } else {
                n
            };
            println!("{family:>16} {n:>9} entries in {ms:>6} ms (~{per_sec} entries/s)");
            assert_eq!(report.status, DuplicateStatus::Completed, "{family}/{n}");

            // Correctness anchors per family.
            match family {
                "mostly-unique" => {
                    assert!(report.stats.files_hashed > 0);
                }
                "same-size" => {
                    assert_eq!(report.groups.len(), 0, "distinct content must not group");
                    assert_eq!(
                        report.stats.size_groups_without_duplicates, 1,
                        "all entries collided on one size"
                    );
                }
                "many-duplicates" => {
                    assert_eq!(report.groups.len() as u64, n / 2, "one group per pair");
                    assert!(report.groups.iter().all(|g| g.member_count == 2));
                }
                "many-zero" => {
                    assert_eq!(report.groups.len(), 1, "all zeros → one giant group");
                    assert_eq!(report.groups[0].member_count, n);
                }
                _ => unreachable!(),
            }

            // Approximately linear per-entry cost (generous smoke guard).
            let per_entry = ms as f64 / n as f64;
            if prev_per_entry > 0.0 {
                assert!(
                    per_entry < prev_per_entry * 10.0 + 0.01,
                    "{family}: per-entry cost grew {prev_per_entry:.6} → {per_entry:.6} ms"
                );
            }
            prev_per_entry = per_entry;
        }
    }
}

/// Determinism at scale, out of the ignored set so regressions surface in
/// the normal run.
#[test]
fn synthetic_pipeline_is_deterministic() {
    let n = 20_000u64;
    let (ms_a, a) = run_workload("many-duplicates", n);
    let (ms_b, b) = run_workload("many-duplicates", n);
    println!("determinism check: {ms_a} ms vs {ms_b} ms for {n} entries");
    assert_eq!(a.status, DuplicateStatus::Completed);
    assert_eq!(a.groups, b.groups, "same input → identical groups");
    assert_eq!(a.stats.candidates_hashed, b.stats.candidates_hashed);
    assert_eq!(a.stats.files_hashed, b.stats.files_hashed);
}

/// The >4 GiB streaming-contract proof, opt-in (ignored) like the perf
/// suite: a virtual 5 GiB file is served from an O(1)-memory repeating
/// pattern — no disk space, no allocation that scales with size — exercising
/// the streaming hasher's 64-bit length path and chunk-boundary handling on
/// every platform. CI runs it via `cargo test -p spacelens-identity --
/// --ignored`.
#[test]
#[ignore = "costly by design: streams 10 GiB of virtual content; CI runs --ignored"]
fn huge_file_streams_beyond_4gib() {
    let size: u64 = 5 * 1024 * 1024 * 1024 + 7; // > u32::MAX
    let key = 0xA5A5_5A5A_A5A5_5A5Au64;
    let make = |id: u64, sz: u64, name: &str| FsEntry {
        id,
        parent_id: None,
        path: PathBuf::from(format!("/synthetic/huge/s{sz}.sbin/k{key:016}/{name}")),
        kind: spacelens_engine::EntryKind::File,
        size: sz,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        device: None,
        inode: None,
        hidden: false,
        error: None,
    };
    let entries = vec![
        make(1, size, "first.bin"),
        make(2, size, "second.bin"),
        // Same content pattern, one byte longer: different size ⇒ different
        // candidacy bucket; must never join the group.
        make(3, size + 1, "other.bin"),
    ];
    let reader = SyntheticReader::new();
    let report = run_duplicates(
        entries.into_iter(),
        &DuplicateOptions::default(),
        &CancelHandle::new(),
        Some(&reader),
        &mut |_| {},
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    // Only the size-pair is hashed: `other.bin` has a unique size, so the
    // candidate filter correctly never spends a hash on a singleton.
    assert_eq!(report.stats.files_hashed, 2, "{report:?}");
    assert_eq!(
        report.stats.size_groups_without_duplicates, 0,
        "the singleton was filtered before hashing, so no hashed group came back all-distinct"
    );
    assert_eq!(report.groups.len(), 1, "{report:?}");
    let g = &report.groups[0];
    assert_eq!(g.size, size, "sizes beyond 4 GiB must round-trip");
    assert_eq!(g.member_count, 2);
    assert_eq!(g.logical_duplicate_bytes, size);
    assert_ne!(
        g.content_hash,
        spacelens_identity::ContentHash::empty(),
        "a 5 GiB stream must not produce the empty-content identity"
    );
}

/// u64 size plumbing across chunk boundaries — the cheap, always-on
/// companion to `huge_file_streams_beyond_4gib`. Pairs sized exactly one
/// chunk-below, exactly one chunk, and just past a chunk boundary must all
/// group on content; singletons must never be hashed (candidate filter) and
/// never group. Synthetic content: no disk, deterministic, milliseconds.
#[test]
fn u64_sizes_group_across_chunk_boundaries() {
    use spacelens_identity::HASH_CHUNK_LEN;
    let mk = |id: u64, sz: u64, key: u64, name: &str| FsEntry {
        id,
        parent_id: None,
        path: PathBuf::from(format!("/synthetic/sizes/s{sz}.sbin/k{key:016}/{name}")),
        kind: spacelens_engine::EntryKind::File,
        size: sz,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        device: None,
        inode: None,
        hidden: false,
        error: None,
    };
    let (below, exact, above) = (
        (HASH_CHUNK_LEN - 1) as u64,
        HASH_CHUNK_LEN as u64,
        (HASH_CHUNK_LEN + 3) as u64,
    );
    let entries = vec![
        mk(1, below, 11, "below-a.bin"),
        mk(2, below, 11, "below-b.bin"),
        mk(3, exact, 22, "exact-a.bin"),
        mk(4, exact, 22, "exact-b.bin"),
        mk(5, above, 33, "above-a.bin"),
        mk(6, above, 33, "above-b.bin"),
        mk(7, (2 * HASH_CHUNK_LEN + 1) as u64, 44, "singleton.bin"),
    ];
    let reader = SyntheticReader::new();
    let report = run_duplicates(
        entries.into_iter(),
        &DuplicateOptions::default(),
        &CancelHandle::new(),
        Some(&reader),
        &mut |_| {},
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(
        report.stats.files_hashed, 6,
        "singleton never hashed: {report:?}"
    );
    assert_eq!(report.groups.len(), 3, "{report:?}");
    let mut hashes: Vec<_> = report.groups.iter().map(|g| g.content_hash).collect();
    hashes.sort();
    hashes.dedup();
    assert_eq!(
        hashes.len(),
        3,
        "different content keys → different digests"
    );
    for g in &report.groups {
        assert_eq!(g.member_count, 2);
        assert_ne!(g.content_hash, spacelens_identity::ContentHash::empty());
    }
}
