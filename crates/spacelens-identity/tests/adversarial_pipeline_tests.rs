//! Phase 3.1 adversarial tests at the identity layer — synthetic,
//! platform-independent, deterministic.
//!
//! These test the PIPELINE's contracts directly, without a disk:
//!
//! - **mutation during read**: a same-length rewrite mid-read must reject
//!   the file (typed `Changed`, never a group) — the smallest case that
//!   falsifies "mutation-safe";
//! - **object replacement**: observed identity ≠ handle identity ⇒ typed
//!   `Replaced`, never hashed;
//! - **degraded identity**: when either side cannot prove identity, the
//!   pipeline must proceed on the remaining checks AND the accounting must
//!   degrade to `Estimated` — never fabricated `Exact`;
//! - **caps (Contract A)**: distinct-size exhaustion and the global
//!   candidate-record cap produce exact skip counters and
//!   `CompletedWithLimits`, never a silently truncated `Completed`;
//! - **determinism under caps**: same input ⇒ byte-identical reports;
//!   observation order never changes results;
//! - **memory boundedness**: a global allocator counts live allocations
//!   while the pipeline ingests 100k+ hostile entries — peak staged memory
//!   must NOT scale with entry count past the caps (the Phase 3 defect).

use std::alloc::{GlobalAlloc, Layout, System};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use spacelens_engine::platform::{ContentError, ContentReader, HandleStat};
use spacelens_engine::{CancelHandle, FsEntry};
use spacelens_identity::{
    run_duplicates, ContentReaderFactory, DuplicateOptions, DuplicateStatus, HashFailureKind,
};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn entry(id: u64, path: &str, size: u64, object: Option<(u64, u64)>) -> FsEntry {
    FsEntry {
        id,
        parent_id: None,
        path: PathBuf::from(path),
        kind: spacelens_engine::EntryKind::File,
        size,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: object.map(|o| o.0),
        inode: object.map(|o| o.1),
        hidden: false,
        error: None,
    }
}

/// Reader whose per-file behavior is fully scripted per path.
struct ScriptedReader {
    /// (size, identity, mutation_after_bytes, replace_identity_with) per
    /// exact path. `mutation_after_bytes` flips content bytes (same length)
    /// once that many bytes have been served, also bumping the change stamp
    /// — a mid-read rewrite. `replace_identity_with` makes the OPENED
    /// handle report a different identity than observed (the pipeline's
    /// check-1 trigger).
    scripts: std::sync::Mutex<Vec<Script>>,
}

#[derive(Clone)]
struct Script {
    path: PathBuf,
    size: u64,
    identity: Option<(u64, u64)>,
    mutation_after: Option<u64>,
    replace_identity_with: Option<Option<(u64, u64)>>,
}

impl ScriptedReader {
    fn new() -> Self {
        ScriptedReader {
            scripts: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn add(&self, path: &str, size: u64, identity: Option<(u64, u64)>) -> &Self {
        self.scripts.lock().unwrap().push(Script {
            path: PathBuf::from(path),
            size,
            identity,
            mutation_after: None,
            replace_identity_with: None,
        });
        self
    }

    /// Serve `size` bytes, then flip every remaining byte (same length) —
    /// a same-length rewrite DURING the read.
    fn mutate_after(&self, path: &str, after: u64) -> &Self {
        let mut s = self.scripts.lock().unwrap();
        let sc = s.iter_mut().find(|x| x.path == Path::new(path)).unwrap();
        sc.mutation_after = Some(after);
        self
    }

    /// The opened handle reports this identity instead of the script's —
    /// an object replacement between scan and open.
    fn opened_identity(&self, path: &str, identity: Option<(u64, u64)>) -> &Self {
        let mut s = self.scripts.lock().unwrap();
        let sc = s.iter_mut().find(|x| x.path == Path::new(path)).unwrap();
        sc.replace_identity_with = Some(identity);
        self
    }
}

struct ScriptedChunks {
    script: Script,
    served: u64,
    /// Flipped to true once the mid-read mutation fired.
    mutated: bool,
    /// Change stamp — bumps when the mutation fires.
    change: SystemTime,
}

impl ScriptedChunks {
    fn byte_at(&self, i: u64, mutated_side: bool) -> u8 {
        let base = (i % 251) as u8;
        if mutated_side {
            !base
        } else {
            base
        }
    }
}

impl ContentReader for ScriptedChunks {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        let take = buf.len().min((self.script.size - self.served) as usize);
        if take == 0 {
            return Ok(None);
        }
        // Fire the mid-read same-length mutation when the scripted offset
        // falls inside this chunk: bytes before `after` keep their values,
        // bytes from `after` onward flip — a torn read the bracket must
        // catch. The change stamp moves at the same moment.
        if let Some(after) = self.script.mutation_after {
            if !self.mutated && self.served <= after && after < self.served + take as u64 {
                self.mutated = true;
                self.change = SystemTime::UNIX_EPOCH
                    .checked_add(std::time::Duration::from_secs(1))
                    .unwrap();
            }
        }
        for (k, slot) in buf.iter_mut().enumerate().take(take) {
            let pos = self.served + k as u64;
            let past_mutation = self.mutated && pos >= self.script.mutation_after.unwrap_or(0);
            *slot = self.byte_at(pos, past_mutation);
        }
        self.served += take as u64;
        Ok(Some(take))
    }

    fn file_identity(&self) -> spacelens_engine::FileIdentity {
        let (device, inode) = match self.script.replace_identity_with {
            Some(overridden) => overridden,
            None => self.script.identity,
        }
        .unzip();
        spacelens_engine::FileIdentity {
            device,
            inode,
            link_count: Some(1),
        }
    }

    fn pre_stat(&self) -> io::Result<HandleStat> {
        Ok(self.stat())
    }

    fn post_stat(&self) -> io::Result<HandleStat> {
        Ok(self.stat())
    }
}

impl ScriptedChunks {
    fn stat(&self) -> HandleStat {
        HandleStat {
            len: self.script.size,
            modified: Some(SystemTime::UNIX_EPOCH),
            changed: Some(self.change),
        }
    }
}

impl ContentReaderFactory for ScriptedReader {
    fn read(
        &self,
        path: &Path,
        feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
    ) -> Result<(), ContentError> {
        let scripts = self.scripts.lock().unwrap();
        let script = scripts
            .iter()
            .find(|s| s.path == path)
            .cloned()
            .ok_or_else(|| ContentError::OpenFailed(io::Error::from_raw_os_error(2)))?;
        drop(scripts);
        let mut chunks = ScriptedChunks {
            script,
            served: 0,
            mutated: false,
            change: SystemTime::UNIX_EPOCH,
        };
        feed(&mut chunks).map_err(ContentError::ReadFailed)
    }
}

fn no_events(_: spacelens_identity::DuplicateProgressEvent) {}

fn run(
    entries: Vec<FsEntry>,
    reader: &ScriptedReader,
    options: &DuplicateOptions,
) -> spacelens_identity::DuplicateReport {
    run_duplicates(
        entries.into_iter(),
        options,
        &CancelHandle::new(),
        Some(reader),
        &mut no_events,
    )
}

// ---------------------------------------------------------------------------
// Defect 1: same-size mutation must never escape
// ---------------------------------------------------------------------------

#[test]
fn same_length_rewrite_during_read_is_rejected_typed_changed() {
    // The adversarial core: content flips mid-read, LENGTH NEVER MOVES.
    // Only the change-time bracket can catch it.
    let reader = ScriptedReader::new();
    reader.add("/f/aaa.bin", 4096, Some((1, 10)));
    reader.add("/f/bbb.bin", 4096, Some((1, 11)));
    reader.mutate_after("/f/bbb.bin", 1024);

    let report = run(
        vec![
            entry(1, "/f/aaa.bin", 4096, Some((1, 10))),
            entry(2, "/f/bbb.bin", 4096, Some((1, 11))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert!(
        report.groups.is_empty(),
        "a mid-read same-length rewrite must not group: {report:?}"
    );
    let failure = report
        .failures
        .iter()
        .find(|f| f.path == Path::new("/f/bbb.bin"))
        .expect("the mutated file must fail, not silently skip");
    assert_eq!(failure.kind, HashFailureKind::Changed);
    assert!(
        failure.message.contains("change-time moved") || failure.message.contains("changed"),
        "the failure must name the mutation: {failure:?}"
    );
    // The stable twin still hashed: exactly one file hashed, one failed.
    assert_eq!(report.stats.files_hashed, 1);
    assert_eq!(report.stats.failures, 1);
}

#[test]
fn mutation_at_last_byte_is_still_rejected() {
    // Flip fires at the very last chunk: post-stat must still see the moved
    // change stamp (the bracket covers the whole read, not just the body).
    let reader = ScriptedReader::new();
    reader.add("/g/aaa.bin", 300, Some((1, 1)));
    reader.add("/g/bbb.bin", 300, Some((1, 2)));
    reader.mutate_after("/g/bbb.bin", 299);

    let report = run(
        vec![
            entry(1, "/g/aaa.bin", 300, Some((1, 1))),
            entry(2, "/g/bbb.bin", 300, Some((1, 2))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert!(report.groups.is_empty(), "{report:?}");
    assert!(report
        .failures
        .iter()
        .any(|f| f.path == Path::new("/g/bbb.bin") && f.kind == HashFailureKind::Changed));
}

#[test]
fn stable_files_still_group_when_brackets_are_enforced() {
    // Guard against an over-rejecting implementation: unmutated scripted
    // files with identical content and STABLE change stamps must group.
    let reader = ScriptedReader::new();
    reader.add("/h/aaa.bin", 4096, Some((1, 10)));
    reader.add("/h/bbb.bin", 4096, Some((1, 11)));

    let report = run(
        vec![
            entry(1, "/h/aaa.bin", 4096, Some((1, 10))),
            entry(2, "/h/bbb.bin", 4096, Some((1, 11))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.stats.failures, 0);
}

// ---------------------------------------------------------------------------
// Defect 2: observed vs opened object
// ---------------------------------------------------------------------------

#[test]
fn opened_identity_disagreement_is_typed_replaced() {
    // Same content, same size — only the identity check distinguishes the
    // impostor. Observed (2, 999), opened reports (9, 9).
    let reader = ScriptedReader::new();
    reader.add("/i/orig.bin", 1024, Some((1, 1)));
    reader.add("/i/swapped.bin", 1024, Some((1, 1)));
    reader.opened_identity("/i/swapped.bin", Some((9, 9)));

    let report = run(
        vec![
            entry(1, "/i/orig.bin", 1024, Some((1, 1))),
            entry(2, "/i/swapped.bin", 1024, Some((2, 999))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert!(
        report.groups.iter().all(|g| !g
            .members
            .iter()
            .any(|m| m.path == Path::new("/i/swapped.bin"))),
        "the impostor must not group: {report:?}"
    );
    let failure = report
        .failures
        .iter()
        .find(|f| f.path == Path::new("/i/swapped.bin"))
        .expect("typed failure required");
    assert_eq!(
        failure.kind,
        HashFailureKind::Replaced,
        "identity disagreement is Replaced, not Changed/Vanished: {failure:?}"
    );
}

#[test]
fn degraded_identity_never_fabricates_and_groups_honestly() {
    // Neither side can prove identity (None everywhere): the pipeline must
    // proceed on the remaining checks, group the identical content, AND
    // report Estimated accounting — never Exact.
    let reader = ScriptedReader::new();
    reader.add("/j/aaa.bin", 512, None);
    reader.add("/j/bbb.bin", 512, None);

    let report = run(
        vec![
            entry(1, "/j/aaa.bin", 512, None),
            entry(2, "/j/bbb.bin", 512, None),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(
        report.groups[0].accounting,
        spacelens_identity::StorageAccounting::Estimated,
        "unprovable identity must degrade honestly"
    );
    assert_eq!(report.stats.failures, 0);
}

#[test]
fn partial_identity_degrades_gracefully_never_treated_as_replacement() {
    // Observation proves identity, handle does not: the observed-vs-opened
    // comparison is skipped (degraded honestly — one-sided identity is
    // never treated as a replacement), the file hashes normally, and the
    // published identity falls back to the observation. Distinct published
    // identities still allow Exact accounting; a member with NEITHER side
    // proven degrades the whole group to Estimated (covered by
    // degraded_identity_never_fabricates_and_groups_honestly).
    let reader = ScriptedReader::new();
    reader.add("/k/aaa.bin", 256, None); // handle proves nothing
    reader.add("/k/bbb.bin", 256, Some((1, 2)));

    let report = run(
        vec![
            entry(1, "/k/aaa.bin", 256, Some((7, 7))), // observed proves
            entry(2, "/k/bbb.bin", 256, Some((1, 2))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert_eq!(report.groups.len(), 1, "degraded ≠ rejected: {report:?}");
    assert_eq!(report.stats.failures, 0, "degraded must not be a failure");
    // The unproven-handle member publishes its OBSERVED identity (7, 7) —
    // honest fallback, never fabricated, never None-with-a-known-identity.
    let a = report.groups[0]
        .members
        .iter()
        .find(|m| m.path == Path::new("/k/aaa.bin"))
        .unwrap();
    assert_eq!(a.object_id, Some((7, 7)));
}

#[test]
fn hard_link_aliases_share_opened_identity_and_group_exact() {
    // Both observed and opened identity are equal: a true alias set.
    // Published identity must be the HANDLE-proven one.
    let reader = ScriptedReader::new();
    reader.add("/l/a.bin", 128, Some((3, 42)));
    reader.add("/l/alias.bin", 128, Some((3, 42)));

    let report = run(
        vec![
            entry(1, "/l/a.bin", 128, Some((3, 42))),
            entry(2, "/l/alias.bin", 128, Some((3, 42))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    let g = &report.groups[0];
    assert_eq!(g.member_count, 2);
    assert_eq!(g.recoverable_bytes, None, "one object: nothing recoverable");
    assert_eq!(g.accounting, spacelens_identity::StorageAccounting::Exact);
    // Handle-proven identity is published even when observation agreed.
    assert_eq!(g.members[0].object_id, Some((3, 42)));
    assert_eq!(g.members[1].object_id, Some((3, 42)));
}

// ---------------------------------------------------------------------------
// Defect 4/5: caps, exact accounting, determinism
// ---------------------------------------------------------------------------

fn many_entries_with_unique_sizes(n: u64) -> Vec<FsEntry> {
    (0..n)
        .map(|i| entry(i, &format!("/u/f{i}.bin"), 1000 + i, Some((1, i))))
        .collect()
}

#[test]
fn distinct_size_cap_reports_completed_with_limits_and_exact_skips() {
    // 50 entries with 50 distinct sizes, tracking cap 10: exactly 40 must be
    // counted as skipped, status must NOT claim Completed.
    let reader = ScriptedReader::new();
    let options = DuplicateOptions {
        max_tracked_size_groups: 10,
        ..DuplicateOptions::default()
    };
    let n = 50u64;
    let report = run(many_entries_with_unique_sizes(n), &reader, &options);
    assert_eq!(
        report.status,
        DuplicateStatus::CompletedWithLimits,
        "a capped run must never look complete: {report:?}"
    );
    assert_eq!(report.stats.candidates_skipped_size_tracking, 40);
    assert_eq!(report.stats.candidates_untracked_total, 40);
    assert_eq!(report.stats.entries_examined, n);
    // Nothing was hashed: every tracked size was a singleton.
    assert_eq!(report.stats.files_hashed, 0);
}

#[test]
fn global_record_cap_bounds_staging_and_reports_limits() {
    // Two sizes, 8 files each; global record cap 8. The first 8 records win
    // (deterministic), the rest are counted. The capped run must say so.
    let reader = ScriptedReader::new();
    for i in 0..8 {
        reader.add(&format!("/m/a{i}.bin"), 100, Some((1, i)));
        reader.add(&format!("/m/b{i}.bin"), 200, Some((1, 100 + i)));
    }
    let options = DuplicateOptions {
        max_tracked_candidates: 8,
        ..DuplicateOptions::default()
    };
    let entries: Vec<FsEntry> = (0..8)
        .flat_map(|i| {
            vec![
                entry(i, &format!("/m/a{i}.bin"), 100, Some((1, i))),
                entry(100 + i, &format!("/m/b{i}.bin"), 200, Some((1, 100 + i))),
            ]
        })
        .collect();
    let report = run(entries, &reader, &options);
    assert_eq!(report.status, DuplicateStatus::CompletedWithLimits);
    assert_eq!(report.stats.candidates_skipped_global_cap, 8);
    // 8 records staged across 2 groups; 8 more members offered and skipped.
    assert!(report.stats.candidates_hashed <= 8);
    assert_eq!(report.stats.candidates_untracked_total, 8);
}

#[test]
fn no_silent_omission_uncapped_run_reports_completed() {
    // The same input under ample caps must report plain Completed with
    // zero skipped — proving the counters are not just always-on noise.
    let reader = ScriptedReader::new();
    reader.add("/n/a.bin", 50, Some((1, 1)));
    reader.add("/n/b.bin", 50, Some((1, 2)));
    let report = run(
        vec![
            entry(1, "/n/a.bin", 50, Some((1, 1))),
            entry(2, "/n/b.bin", 50, Some((1, 2))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(report.stats.candidates_skipped_size_tracking, 0);
    assert_eq!(report.stats.candidates_skipped_global_cap, 0);
    assert_eq!(report.stats.candidates_untracked_total, 0);
}

#[test]
fn capped_runs_are_deterministic_across_shuffled_observation_order() {
    // Same 40 entries, two observation orders, cap 5 tracked sizes: the
    // logical report (groups, stats) must be IDENTICAL — cap policy is
    // order-independent by construction (size-ascending fill).
    let reader = ScriptedReader::new();
    for i in 0..20 {
        reader.add(&format!("/o/a{i}.bin"), 64, Some((1, i)));
    }
    let options = DuplicateOptions {
        max_tracked_size_groups: 5,
        ..DuplicateOptions::default()
    };
    // 20 distinct sizes, 2 entries each would be needed for groups; keep
    // distinct sizes so the cap bites deterministically.
    let entries: Vec<FsEntry> = (0..20)
        .map(|i| entry(i, &format!("/o/s{i}.bin"), 1000 + i * 10, Some((1, i))))
        .collect();
    let mut reversed = entries.clone();
    reversed.reverse();

    let a = run(entries, &reader, &options);
    let b = run(reversed, &reader, &options);
    assert_eq!(a.status, DuplicateStatus::CompletedWithLimits);
    assert_eq!(a.groups, b.groups, "groups must not depend on order");
    assert_eq!(a.stats, b.stats, "cap accounting must be order-independent");
    assert_eq!(a.stats.candidates_skipped_size_tracking, 15);
}

#[test]
fn per_group_cap_counts_exactly_and_keeps_the_group_partial() {
    // 6 files same size, per-group cap 4: 2 skipped by cap, the 4 hashed
    // still form the group (partial analysis, counted, status honest).
    let reader = ScriptedReader::new();
    for i in 0..6 {
        reader.add(&format!("/p/f{i}.bin"), 32, Some((1, i)));
    }
    let options = DuplicateOptions {
        max_candidates_per_group: 4,
        ..DuplicateOptions::default()
    };
    let entries: Vec<FsEntry> = (0..6)
        .map(|i| entry(i, &format!("/p/f{i}.bin"), 32, Some((1, i))))
        .collect();
    let report = run(entries, &reader, &options);
    // Per-group cap is Phase 3 semantics: status stays Completed (the group
    // was analyzed; the SKIPPED members are counted, not hidden).
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(report.stats.candidates_skipped_by_cap, 2);
    assert_eq!(report.stats.files_hashed, 4);
}

// ---------------------------------------------------------------------------
// Defect 4: the memory-boundedness PROOF (counting allocator)
// ---------------------------------------------------------------------------

/// A global allocator that counts live bytes. Precision is not required —
/// the assertion is about SCALE (entries vs cap), not exact bytes.
struct CountingAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let new = Layout::from_size_align_unchecked(new_size, old.align());
        let nptr = System.realloc(ptr, old, new_size);
        if !nptr.is_null() {
            if new.size() > old.size() {
                let d = new.size() - old.size();
                let live = LIVE.fetch_add(d, Ordering::Relaxed) + d;
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(old.size() - new.size(), Ordering::Relaxed);
            }
        }
        nptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// Hostile ingest: N entries, ALL DISTINCT SIZES (the Phase 3 unbounded
/// case), with the tracking caps set LOW. Peak live allocations during the
/// run must stay near the cap budget — not grow with N.
#[test]
fn peak_staging_memory_does_not_scale_with_entry_count() {
    // Warm up allocator bookkeeping outside the measured section.
    let warm: Vec<u8> = vec![0; 1024];
    drop(warm);

    let reader = ScriptedReader::new();
    let options = DuplicateOptions {
        max_tracked_size_groups: 64,
        max_tracked_candidates: 128,
        ..DuplicateOptions::default()
    };

    let n_small = 10_000u64;
    let n_large = 100_000u64;

    fn measure_run(
        n: u64,
        options: &DuplicateOptions,
        reader: &ScriptedReader,
    ) -> (usize, spacelens_identity::PipelineStats) {
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        let entries =
            (0..n).map(|i| entry(i, &format!("/mem/f{i}.bin"), 4000 + i, Some((1, i % 997))));
        let report = run_duplicates(
            entries,
            options,
            &CancelHandle::new(),
            Some(reader),
            &mut no_events,
        );
        assert_eq!(
            report.status,
            DuplicateStatus::CompletedWithLimits,
            "caps must bite: {report:?}"
        );
        (
            PEAK.load(Ordering::Relaxed).saturating_sub(baseline),
            report.stats,
        )
    }

    let (peak_small, stats_small) = measure_run(n_small, &options, &reader);
    let (peak_large, stats_large) = measure_run(n_large, &options, &reader);

    // Both runs staged the SAME bounded amount of work (the caps), while
    // ingesting 10× more entries.
    assert_eq!(stats_small.candidates_skipped_size_tracking, n_small - 64);
    assert_eq!(stats_large.candidates_skipped_size_tracking, n_large - 64);
    // The memory PROOF: 10× the input must not mean 10× the peak staging
    // delta. With a hard cap of 128 records the staging delta must stay in
    // the same order of magnitude (allow 4× headroom for allocator noise,
    // channels, and hashed results).
    assert!(
        peak_large as f64 <= (peak_small as f64 * 4.0).max(peak_small as f64 + (256.0 * 1024.0)),
        "peak staging must not scale with entry count: small={peak_small}, large={peak_large}"
    );
    println!("peak staging delta: {peak_small} B (10k) vs {peak_large} B (100k)");
    // And the absolute peak must be tiny relative to the hostile input:
    // 100k staged entries at ~100 B each would be ~10 MB; the capped run
    // must stay far below that.
    assert!(
        peak_large < 4 * 1024 * 1024,
        "capped staging must stay well under MBs: {peak_large} B"
    );
}

// ---------------------------------------------------------------------------
// Cancellation under pressure (Contract A + STEP 14)
// ---------------------------------------------------------------------------

#[test]
fn cancel_during_capped_ingest_publishes_no_groups() {
    // Cancellation must preempt even a capped, hostile ingest.
    let reader = ScriptedReader::new();
    reader.add("/q/a.bin", 10, Some((1, 1)));
    reader.add("/q/b.bin", 10, Some((1, 2)));
    let cancel = CancelHandle::new();
    // Cancel after the 5th entry of a 100-entry stream.
    let entries = (0..100u64).map(|i| {
        if i == 5 {
            cancel.cancel();
        }
        entry(i, &format!("/q/f{i}.bin"), 10 + i, Some((1, i)))
    });
    let report = run_duplicates(
        entries,
        &DuplicateOptions::default(),
        &cancel,
        Some(&reader),
        &mut no_events,
    );
    assert_eq!(report.status, DuplicateStatus::Cancelled);
    assert!(report.groups.is_empty(), "cancelled runs publish no groups");
}

// ---------------------------------------------------------------------------
// Error-semantics preservation (no regressions)
// ---------------------------------------------------------------------------

#[test]
fn vanished_still_typed_vanished_not_replaced_or_changed() {
    // Path absent from the reader's world: raw ENOENT → Vanished.
    let reader = ScriptedReader::new();
    reader.add("/r/keep.bin", 8, Some((1, 1)));
    let report = run(
        vec![
            entry(1, "/r/keep.bin", 8, Some((1, 1))),
            entry(2, "/r/gone.bin", 8, Some((1, 2))),
        ],
        &reader,
        &DuplicateOptions::default(),
    );
    assert!(report.groups.is_empty());
    let failure = &report.failures[0];
    assert_eq!(failure.path, PathBuf::from("/r/gone.bin"));
    assert_eq!(failure.kind, HashFailureKind::Vanished);
}
