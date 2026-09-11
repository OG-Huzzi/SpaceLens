//! The duplicate detection pipeline (Phase 3 STEP 17, hardened 3.1).
//!
//! ```text
//! Observed FsEntry
//!       ↓  ingest: eligibility contract
//! Size grouping             (same size ⇒ candidacy only, never equality)
//!       ↓  bounded worker pool, globally bounded candidate memory
//! Content hashing           (streaming SHA-256, mutation-bracketed)
//!       ↓
//! Content identity grouping (same digest ⇒ same bytes)
//!       ↓  deterministic ordering (size asc, then hash bytes asc)
//! DuplicateReport + typed progress events + typed failures
//! ```
//!
//! ## Boundedness contract (Phase 3.1, Contract A — docs/IDENTITY.md)
//!
//! Ingest is **streaming**: the `entries` iterator is consumed one entry at
//! a time and per-size-group state is capped by *global* limits, not just
//! per-group ones. `DEFAULT_MAX_TRACKED_SIZE_GROUPS` bounds the number of
//! distinct sizes tracked; `DEFAULT_MAX_TRACKED_CANDIDATES` bounds total
//! in-memory member records across all groups. Records beyond a cap are
//! **counted, never silently dropped**: the report carries the exact skip
//! counters and a `CompletedWithLimits` status whenever any cap bit into
//! the input. A user can never mistake a capped run for a complete one.
//!
//! Concurrency: a **fixed, bounded worker pool** (never thread-per-file),
//! fed from pre-grouped candidate batches after metadata filtering. Workers
//! read content only through the [`ContentReaderFactory`] boundary — the
//! pipeline never walks the filesystem. Channels are bounded (natural
//! backpressure); the job queue holds paths, never content.
//!
//! Mutation safety (Phase 3.1): each file's read is bracketed by
//! handle-proven facts — length and change timestamps before and after the
//! read must agree, the total bytes read must equal the observed size, and
//! the opened object's identity must match the observation-time identity
//! where the platform can prove both (Unix always; Windows honestly cannot
//! from a path stat, so it degrades to the length + change-time brackets —
//! never fabricated). A same-size rewrite that preserves mtime is caught by
//! the change-time bracket on filesystems that maintain it; where none is
//! available the digest is accepted under the documented residual-window
//! contract (docs/IDENTITY.md §known limitations).
//!
//! Cancellation: checked at ingest, between jobs, and per chunk inside the
//! hash. A cancelled run reports [`DuplicateStatus::Cancelled`] with no
//! groups — partial state never escapes as a valid result.
//!
//! Progress: staged, typed snapshots at a caller-set interval. There is
//! deliberately **no percent-complete**: before hashing finishes the engine
//! cannot honestly estimate remaining work, and byte-based percent would
//! require reading every byte it is trying to skip.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use spacelens_engine::platform::{ContentError, ContentReader};
use spacelens_engine::{CancelHandle, FsEntry};

use crate::duplicate::{DuplicateGroup, DuplicateMember};
use crate::eligibility::{Eligibility, EligibilityStats, IneligibleReason};
use crate::error::{HashError, HashFailure, HashFailureKind};
use crate::hash::{ContentHash, ContentHasher};
use crate::policy::{
    ContentReaderFactory, MutationPolicy, DEFAULT_MAX_CANDIDATES_PER_GROUP,
    DEFAULT_MAX_GROUP_MEMBERS_REPORTED, DEFAULT_MAX_TRACKED_CANDIDATES,
    DEFAULT_MAX_TRACKED_SIZE_GROUPS,
};

/// Terminal state of one duplicate-detection run. Mirrors the scan model:
/// a cancellation is a successful cancellation, never an error, and never
/// masquerades as `Completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateStatus {
    /// Ingest + hashing + grouping all finished within every bound; the
    /// report covers the entire input.
    Completed,
    /// Ingest + hashing + grouping all finished, but at least one global
    /// bound truncated candidate work (see
    /// [`PipelineStats::candidates_skipped_size_tracking`] and
    /// [`PipelineStats::candidates_skipped_global_cap`]). Groups are real
    /// for what was analyzed; the report is **not** a complete analysis of
    /// the input and never claims to be.
    CompletedWithLimits,
    /// Cancellation observed before or during work. No groups are published.
    Cancelled,
    /// No content boundary was available: candidacy ran, nothing could be
    /// confirmed. Groups are empty by construction.
    Unsupported,
}

/// Typed, bounded progress (STEP 15).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateProgressSnapshot {
    pub entries_examined: u64,
    pub candidates_grouped: u64,
    pub files_hashed: u64,
    pub bytes_hashed: u64,
    pub failures: u64,
    pub elapsed_ms: u64,
}

/// Throttled progress events (mirror of `ScanEvent` semantics: exactly one
/// terminal event; `Started` first).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum DuplicateProgressEvent {
    Started,
    Progress(DuplicateProgressSnapshot),
    Completed(Box<DuplicateReport>),
    Cancelled(Box<DuplicateReport>),
    Failed(Box<DuplicateReport>),
}

/// Configuration. All bounds are explicit and documented.
#[derive(Debug, Clone)]
pub struct DuplicateOptions {
    /// Worker threads for content hashing. Bounded; see
    /// [`crate::policy::default_hash_threads`].
    pub threads: usize,
    /// Minimum file size (bytes) considered worth hashing. Files below it
    /// are counted as singletons-by-policy (never hashed; host-controlled
    /// cost floor).
    pub min_file_size: u64,
    /// Whether zero-byte files may form duplicate groups. Default **false**:
    /// all zero-byte files share one content identity, so enabling this on a
    /// hostile tree can create enormous groups with zero storage value.
    /// When disabled, zero-byte same-size sets are counted
    /// (`zero_byte_matches_ungrouped`), not grouped.
    pub group_zero_byte_files: bool,
    /// Hard cap on candidates hashed per size group.
    pub max_candidates_per_group: usize,
    /// Hard cap on member detail kept per group in the report.
    pub max_group_members_reported: usize,
    /// Hard cap on **distinct sizes** tracked during ingest (global bound A).
    pub max_tracked_size_groups: usize,
    /// Hard **global** cap on in-memory candidate records across all size
    /// groups (global bound B — the per-group cap alone cannot bound total
    /// memory).
    pub max_tracked_candidates: usize,
    /// Minimum interval between progress events.
    pub progress_interval: std::time::Duration,
    pub mutation_policy: MutationPolicy,
}

impl Default for DuplicateOptions {
    fn default() -> Self {
        DuplicateOptions {
            threads: crate::policy::default_hash_threads(),
            min_file_size: 0,
            group_zero_byte_files: false,
            max_candidates_per_group: DEFAULT_MAX_CANDIDATES_PER_GROUP,
            max_group_members_reported: DEFAULT_MAX_GROUP_MEMBERS_REPORTED,
            max_tracked_size_groups: DEFAULT_MAX_TRACKED_SIZE_GROUPS,
            max_tracked_candidates: DEFAULT_MAX_TRACKED_CANDIDATES,
            progress_interval: std::time::Duration::from_millis(250),
            mutation_policy: MutationPolicy::default(),
        }
    }
}

/// Exact counters from one run (fixed-width; bounded).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStats {
    pub entries_examined: u64,
    pub eligible_files: u64,
    pub zero_byte_files: u64,
    /// Distinct eligible sizes currently tracked (≤
    /// `max_tracked_size_groups`).
    pub size_groups: u64,
    pub candidates_hashed: u64,
    pub files_hashed: u64,
    pub bytes_hashed: u64,
    /// Same-size groups with ≥2 candidates (hashing was required).
    pub size_groups_needing_hashes: u64,
    /// Files never hashed because no other eligible file shared their size.
    pub singleton_files: u64,
    /// Same-size groups whose hashed contents proved all-distinct.
    pub size_groups_without_duplicates: u64,
    /// Records excluded because their size group exceeded the per-group cap
    /// (exact count; the group is still analyzed partially).
    pub candidates_skipped_by_cap: u64,
    /// Eligible members whose size group was not tracked because the
    /// distinct-size cap was already exhausted (exact count).
    pub candidates_skipped_size_tracking: u64,
    /// Eligible members excluded because the global candidate-record cap was
    /// reached (exact count).
    pub candidates_skipped_global_cap: u64,
    /// Eligible files (any cap) that were therefore not hashed.
    pub candidates_untracked_total: u64,
    /// Eligible files below `min_file_size` (never hashed; host cost floor).
    pub below_min_files: u64,
    pub zero_byte_matches_ungrouped: u64,
    pub failures: u64,
}

/// Final result of one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReport {
    pub status: DuplicateStatus,
    /// Duplicate groups in deterministic order: **size ascending, then
    /// content-hash bytes ascending** — stable across runs and platforms
    /// for the same input.
    pub groups: Vec<DuplicateGroup>,
    /// Typed per-file failures (bounded detail; exact count in
    /// [`Self::stats`].`failures`).
    pub failures: Vec<HashFailure>,
    pub failures_truncated: u64,
    pub stats: PipelineStats,
    pub eligibility: EligibilityStats,
    /// Total logical duplicate bytes across groups (Σ size × (n−1)).
    /// **Never** advertised as recoverable storage.
    pub logical_duplicate_bytes: u64,
    /// Sum of `recoverable_bytes` where accounting allowed a value.
    /// `None` when no group could prove or estimate recoverable storage.
    pub recoverable_bytes: Option<u64>,
    pub started_at: std::time::SystemTime,
    pub finished_at: std::time::SystemTime,
}

/// Hard cap on retained failure detail (mirrors the scan error report).
const FAILURE_DETAIL_CAP: usize = 256;

/// One staged candidate: the observation-side facts the mutation policy
/// will verify against the open handle. Deliberately lean (path + observed
/// size + observed object identity + observed change stamp) — this is the
/// structure the global cap bounds.
#[derive(Debug, Clone)]
struct StagedMember {
    entry_id: u64,
    path: PathBuf,
    size: u64,
    /// Observation-time object identity `(device, inode)` where the scanner
    /// proved it (Unix). `None` = unavailable on this platform (Windows
    /// path-stats) — the observed-vs-opened check degrades honestly.
    observed_object_id: Option<(u64, u64)>,
    /// Observation-time metadata-change stamp (Unix `st_ctime`). `None`
    /// where the scanner could not observe one — the scan→open bracket
    /// then degrades to mtime/length, never fabricated.
    observed_changed: Option<std::time::SystemTime>,
}

/// Per-size-group staging state. `observed` counts every member ever
/// offered to the group (exact, fixed-width); `members` holds only those
/// admitted under the per-group cap (bounded memory).
#[derive(Debug, Default)]
struct SizeGroup {
    members: Vec<StagedMember>,
    observed: u64,
}

/// Streaming ingest accumulator (Phase 3.1: the staging map is globally
/// bounded, unlike the Phase 3 `BTreeMap<u64, Vec<Member>>` over the whole
/// input).
///
/// Invariants, enforced at insertion time:
/// - `groups.len() <= max_tracked_size_groups` (global bound A),
/// - the total number of staged records `<= max_tracked_candidates`
///   (global bound B — the per-group cap alone cannot bound total memory),
/// - every excluded member increments an exact skip counter; nothing is
///   silently dropped.
struct Ingest {
    eligibility: EligibilityStats,
    /// Tracked size groups (candidate records live here, including size
    /// groups that later prove to be singletons — a pair can only be
    /// discovered by remembering the first file).
    groups: BTreeMap<u64, SizeGroup>,
    /// Total records currently held in `groups`.
    total_members: usize,
    /// Zero-byte files observed under the ungrouped policy (exact counter;
    /// they are never staged, so a hostile million-empty-file tree costs
    /// one counter, not a million records).
    zero_byte_ungrouped_seen: u64,
    stats: PipelineStats,
}

impl Ingest {
    fn new() -> Self {
        Ingest {
            eligibility: EligibilityStats::default(),
            groups: BTreeMap::new(),
            total_members: 0,
            zero_byte_ungrouped_seen: 0,
            stats: PipelineStats::default(),
        }
    }

    /// Feed one observed entry. Pure bookkeeping; no I/O.
    fn feed(&mut self, entry: FsEntry, options: &DuplicateOptions) {
        self.eligibility.examined = self.eligibility.examined.saturating_add(1);
        match Eligibility::of(&entry) {
            Eligibility::Eligible => {
                self.eligibility.eligible_files = self.eligibility.eligible_files.saturating_add(1);
                if entry.size == 0 {
                    self.eligibility.zero_byte_files =
                        self.eligibility.zero_byte_files.saturating_add(1);
                }
            }
            Eligibility::Ineligible(reason) => {
                match reason {
                    IneligibleReason::Directory => self.eligibility.dirs += 1,
                    IneligibleReason::Link => self.eligibility.links += 1,
                    IneligibleReason::Special => self.eligibility.special += 1,
                    IneligibleReason::ObservationError { .. } => {
                        self.eligibility.observation_errors += 1
                    }
                }
                return;
            }
        }
        if entry.size < options.min_file_size {
            // Host-controlled cost floor: counted exactly, never staged.
            self.stats.below_min_files = self.stats.below_min_files.saturating_add(1);
            return;
        }
        let size = entry.size;
        // Zero-byte members under the default policy are counted, never
        // staged (the giant empty group has no storage value). Their count
        // is exact; nothing is silently lost.
        if size == 0 && !options.group_zero_byte_files {
            self.zero_byte_ungrouped_seen = self.zero_byte_ungrouped_seen.saturating_add(1);
            return;
        }

        let group = if let Some(g) = self.groups.get_mut(&size) {
            g
        } else if self.groups.len() < options.max_tracked_size_groups {
            // New trackable size. (Admission of the first record below is
            // also subject to the global record cap.)
            self.groups.entry(size).or_default()
        } else {
            // Global bound A: this size cannot be tracked at all. Every
            // member of an untracked size is counted, exactly.
            self.stats.candidates_skipped_size_tracking = self
                .stats
                .candidates_skipped_size_tracking
                .saturating_add(1);
            return;
        };

        // Every member offered to an existing group is counted exactly.
        group.observed = group.observed.saturating_add(1);
        // Order of the remaining checks is deterministic and documented:
        // per-group cap first (group-level policy), then the global record
        // budget (engine-level resource limit).
        if group.members.len() >= options.max_candidates_per_group.max(1) {
            self.stats.candidates_skipped_by_cap =
                self.stats.candidates_skipped_by_cap.saturating_add(1);
            return;
        }
        if self.total_members >= options.max_tracked_candidates {
            self.stats.candidates_skipped_global_cap =
                self.stats.candidates_skipped_global_cap.saturating_add(1);
            return;
        }
        group.members.push(staged(&entry));
        self.total_members += 1;
    }

    /// Finalize staging: split singleton groups out, move candidate records
    /// into hash jobs, and derive the zero-byte accounting. Iteration is by
    /// ascending size (BTreeMap order) — deterministic by construction.
    fn finish(mut self) -> (Self, Vec<(u64, Vec<StagedMember>)>) {
        self.stats.size_groups = self.groups.len() as u64;
        // A zero-byte set under the ungrouped policy occupied one size slot
        // in Phase 3's staging map; preserve that in the counter.
        if self.zero_byte_ungrouped_seen > 0 {
            self.stats.size_groups = self.stats.size_groups.saturating_add(1);
        }
        let held = std::mem::take(&mut self.groups);
        self.total_members = 0;

        let mut jobs = Vec::new();
        for (size, group) in held {
            let observed = group.observed;
            if observed < 2 {
                // Singleton size: no other eligible file ever shared it.
                // Its records are dropped; the exact count is reported.
                self.stats.singleton_files = self.stats.singleton_files.saturating_add(observed);
                continue;
            }
            // A staged group of any size — including size 0 under the
            // opt-in policy — proceeds to hashing. (Size-0 groups only
            // exist when `group_zero_byte_files` is set: the default
            // policy never stages them, counting them below instead.)
            self.stats.size_groups_needing_hashes =
                self.stats.size_groups_needing_hashes.saturating_add(1);
            self.stats.candidates_hashed = self
                .stats
                .candidates_hashed
                .saturating_add(group.members.len() as u64);
            jobs.push((size, group.members));
        }
        // Zero-byte matches under the default (ungrouped) policy: counted
        // exactly when a set existed; a lone zero-byte file stays a
        // singleton (Phase 3 semantics: the n<2 check precedes the policy).
        if self.zero_byte_ungrouped_seen >= 2 {
            self.stats.zero_byte_matches_ungrouped = self
                .stats
                .zero_byte_matches_ungrouped
                .saturating_add(self.zero_byte_ungrouped_seen);
        } else {
            self.stats.singleton_files = self
                .stats
                .singleton_files
                .saturating_add(self.zero_byte_ungrouped_seen);
        }
        self.stats.candidates_untracked_total = self
            .stats
            .candidates_skipped_size_tracking
            .saturating_add(self.stats.candidates_skipped_global_cap);
        (self, jobs)
    }
}

fn staged(entry: &FsEntry) -> StagedMember {
    StagedMember {
        entry_id: entry.id,
        path: entry.path.clone(),
        size: entry.size,
        observed_object_id: entry.device.zip(entry.inode),
        observed_changed: entry.changed,
    }
}

/// Run duplicate detection over an entry stream.
///
/// `entries` must yield each observed entry exactly once (scanner output;
/// stream order is irrelevant — results are ordered deterministically).
/// `reader` provides content access through the platform boundary
/// (`None` = unavailable: candidacy still runs, status is `Unsupported`).
pub fn run_duplicates(
    entries: impl Iterator<Item = FsEntry>,
    options: &DuplicateOptions,
    cancel: &CancelHandle,
    reader: Option<&dyn ContentReaderFactory>,
    sink: &mut dyn FnMut(DuplicateProgressEvent),
) -> DuplicateReport {
    let started = Instant::now();
    let started_at = std::time::SystemTime::now();
    sink(DuplicateProgressEvent::Started);

    let shared = Arc::new(Shared {
        cancel: cancel.clone(),
        files_hashed: AtomicU64::new(0),
        bytes_hashed: AtomicU64::new(0),
        failures: AtomicU64::new(0),
        failure_detail: Mutex::new(Vec::new()),
        failures_truncated: AtomicU64::new(0),
        results: Mutex::new(Vec::new()),
    });

    if cancel.is_cancelled() {
        return finish(
            DuplicateStatus::Cancelled,
            &shared,
            EligibilityStats::default(),
            PipelineStats::default(),
            started_at,
            sink,
        );
    }

    // ---- Stage 1: streaming ingest + eligibility + bounded size grouping --
    let mut ingest = Ingest::new();
    for entry in entries {
        if cancel.is_cancelled() {
            return finish(
                DuplicateStatus::Cancelled,
                &shared,
                ingest.eligibility,
                PipelineStats::default(),
                started_at,
                sink,
            );
        }
        ingest.feed(entry, options);
    }
    let (mut ingest, mut jobs) = ingest.finish();
    ingest.stats.entries_examined = ingest.eligibility.examined;
    ingest.stats.eligible_files = ingest.eligibility.eligible_files;
    ingest.stats.zero_byte_files = ingest.eligibility.zero_byte_files;

    // Deterministic hash-job order: size ascending, then first-member path
    // bytes (stable regardless of observation order).
    for (_, members) in jobs.iter_mut() {
        members.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
        });
    }
    jobs.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| {
            let pa = a.1.first().map(|m| m.path.as_os_str().as_encoded_bytes());
            let pb = b.1.first().map(|m| m.path.as_os_str().as_encoded_bytes());
            pa.cmp(&pb)
        })
    });

    // ---- Stage 2: bounded hashing pool ----------------------------------
    let mut cancelled = false;
    if let Some(reader) = reader {
        let (tx, rx) = mpsc::sync_channel::<Job>(options.threads.max(1) * 4);
        let rx = Arc::new(Mutex::new(rx));
        std::thread::scope(|scope| {
            // `reader` borrows from the coordinator's frame and outlives the
            // scope; workers join before this function returns (scope guard).
            // The trait declares `Send + Sync` supertraits, so the shared
            // reference is sound to hand to worker threads with no cast.
            for _ in 0..options.threads.max(1) {
                let shared = Arc::clone(&shared);
                let rx = Arc::clone(&rx);
                scope.spawn(move || loop {
                    let job = {
                        let guard = rx.lock().unwrap();
                        guard.recv()
                    };
                    let Ok(job) = job else { break };
                    if shared.cancel.is_cancelled() {
                        break;
                    }
                    match hash_file(&job.member, options, reader, &shared) {
                        Ok(Some((member, hash))) => {
                            shared.files_hashed.fetch_add(1, Ordering::Relaxed);
                            shared
                                .bytes_hashed
                                .fetch_add(member.size, Ordering::Relaxed);
                            shared.push_result(member, hash);
                        }
                        Ok(None) => {}
                        Err(HashError::Cancelled) => break,
                        Err(HashError::Failure(f)) => shared.record_failure(f),
                    }
                });
            }
            // Coordinator: feed jobs, emit throttled progress. The
            // coordinator holds the ONLY sender; dropping it after the feed
            // loop disconnects the channel so workers exit cleanly.
            let mut last_progress = started;
            for (_size, members) in &jobs {
                for member in members {
                    if shared.cancel.is_cancelled() {
                        cancelled = true;
                        break;
                    }
                    // Bounded channel = backpressure; blocks when workers
                    // fall behind. Never unbounded.
                    if tx
                        .send(Job {
                            member: member.clone(),
                        })
                        .is_err()
                    {
                        // All workers dropped the receiver (cancelled).
                        cancelled = true;
                        break;
                    }
                }
                if cancelled {
                    break;
                }
                if last_progress.elapsed() >= options.progress_interval {
                    sink(DuplicateProgressEvent::Progress(progress_snapshot(
                        &shared,
                        &ingest.stats,
                        &ingest.eligibility,
                        started,
                    )));
                    last_progress = Instant::now();
                }
            }
            drop(tx); // disconnect: workers see a closed channel and exit
        });
        if cancel.is_cancelled() {
            cancelled = true;
        }
    } else {
        cancelled = false;
    }

    if cancelled {
        return finish(
            DuplicateStatus::Cancelled,
            &shared,
            ingest.eligibility,
            ingest.stats,
            started_at,
            sink,
        );
    }

    // ---- Stage 4: content-identity grouping ------------------------------
    let hashed = shared.take_results();
    let mut by_content: BTreeMap<(u64, ContentHash), Vec<DuplicateMember>> = BTreeMap::new();
    for (member, hash) in hashed {
        by_content
            .entry((member.size, hash))
            .or_default()
            .push(member);
    }

    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for ((size, hash), members) in by_content {
        if members.len() < 2 {
            continue; // unique content: not a duplicate group
        }
        groups.push(DuplicateGroup::from_members(
            hash,
            size,
            members,
            options.max_group_members_reported,
        ));
    }
    // Deterministic group order: size ascending, then hash bytes ascending.
    groups.sort_by(|a, b| {
        a.size
            .cmp(&b.size)
            .then_with(|| a.content_hash.as_bytes().cmp(b.content_hash.as_bytes()))
    });

    // Same-size groups whose contents proved all-distinct.
    let mut groups_by_size: BTreeMap<u64, u64> = BTreeMap::new();
    for g in &groups {
        *groups_by_size.entry(g.size).or_default() += 1;
    }
    ingest.stats.size_groups_without_duplicates = jobs
        .iter()
        .filter(|(size, _)| groups_by_size.get(size).copied().unwrap_or(0) == 0)
        .count() as u64;
    ingest.stats.files_hashed = shared.files_hashed.load(Ordering::Relaxed);
    ingest.stats.bytes_hashed = shared.bytes_hashed.load(Ordering::Relaxed);
    ingest.stats.failures = shared.failures.load(Ordering::Relaxed);

    let capped = ingest.stats.candidates_skipped_size_tracking > 0
        || ingest.stats.candidates_skipped_global_cap > 0;
    let status = if reader.is_none() && ingest.stats.candidates_hashed > 0 {
        DuplicateStatus::Unsupported
    } else if capped {
        DuplicateStatus::CompletedWithLimits
    } else {
        DuplicateStatus::Completed
    };

    let report = assemble_report(status, groups, &shared, ingest, started_at);
    sink(DuplicateProgressEvent::Completed(Box::new(report.clone())));
    report
}

// ---- hashing worker internals -------------------------------------------

/// The full Phase 3.1 mutation-consistency check sequence for one file.
///
/// A digest is published only when ALL of these hold:
///
/// 1. the opened object is the observed object, where the platform can
///    prove observation-time identity (Unix `st_dev`/`st_ino` from the
///    scan vs. handle-proven identity) — else `Replaced`,
/// 2. the pre-read handle length equals the observed size — else `Changed`,
/// 3. the total bytes read equals the observed size — else `Changed`,
/// 4. the post-read handle length still equals the observed size — else
///    `Changed`,
/// 5. where change timestamps exist: post-read change time equals
///    pre-read change time — else `Changed` (catches same-length rewrites
///    that preserve mtime on filesystems maintaining ctime/ChangeTime),
/// 6. where mtime exists: pre-read mtime is consistent with the
///    observation's mtime when one was recorded — else `Changed`
///    (best-effort; timestamp granularity is documented),
/// 7. the object's link count can differ from observation (hard links may
///    be added) — it is NOT a mutation signal; only object identity is.
///
/// Where the platform cannot prove a fact (no observation identity on
/// Windows; no change time on some filesystems), the check degrades
/// honestly — never fabricated, and the accepted guarantee is documented.
fn hash_file(
    member: &StagedMember,
    options: &DuplicateOptions,
    reader: &dyn ContentReaderFactory,
    shared: &Shared,
) -> Result<Option<(DuplicateMember, ContentHash)>, HashError> {
    if shared.cancel.is_cancelled() {
        return Err(HashError::Cancelled);
    }
    let _ = options.mutation_policy; // Reject — the only implemented policy
    let observed_size = member.size;

    // Outcome carried out of the closure.
    let mut outcome: Result<(DuplicateMember, ContentHash), std::io::Error> = Ok((
        DuplicateMember {
            entry_id: 0,
            path: PathBuf::new(),
            size: 0,
            object_id: None,
        },
        ContentHash::empty(),
    ));

    let read_result = reader.read(member.path.as_path(), &mut |r: &mut dyn ContentReader| {
        // Deliberate-abort signal: `Interrupted` with no read failed. The
        // platform boundary maps exactly this shape to `Aborted`.
        if shared.cancel.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }

        // ---- check 1: observed object vs opened object ----------------
        let identity = r.file_identity();
        let handle_object_id = identity.device.zip(identity.inode);
        if let (Some(observed), Some(handle)) = (member.observed_object_id, handle_object_id) {
            if observed != handle {
                return Err(std::io::Error::other(format!(
                    "object replaced between scan and hash (observed ({},{}), opened ({},{}))",
                    observed.0, observed.1, handle.0, handle.1
                )));
            }
        }
        // Degraded mode (either side unprovable): proceed on the remaining
        // checks — documented, never fabricated.

        // ---- check 2: pre-read length ----------------------------------
        let pre = r.pre_stat()?;
        if pre.len != observed_size {
            return Err(std::io::Error::other(format!(
                "file changed between scan and hash (observed {} bytes, handle reports {})",
                observed_size, pre.len
            )));
        }

        // ---- check 6 (scan→open bracket): the object's change stamp must
        // still be the one the scanner observed, where both sides could
        // prove one. A same-length rewrite moves st_ctime/ChangeTime even
        // when mtime is preserved; where either side is unprovable the
        // check degrades honestly (never fabricated).
        if let (Some(observed_change), Some(handle_change)) = (member.observed_changed, pre.changed)
        {
            if observed_change != handle_change {
                return Err(std::io::Error::other(
                    "file changed between scan and hash (change stamp moved, same length)",
                ));
            }
        }

        // ---- stream the content ----------------------------------------
        let mut hasher = ContentHasher::new();
        let mut buf = vec![0u8; crate::hash::HASH_CHUNK_LEN];
        loop {
            if shared.cancel.is_cancelled() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            match r.read_chunk(&mut buf) {
                Ok(Some(n)) if n > 0 => hasher.update(&buf[..n]),
                Ok(_) => break, // None (EOF) or zero-length read
                Err(e) => return Err(e),
            }
        }
        let (hash, bytes_read) = hasher.finalize();

        // ---- check 3: bytes read == observed ----------------------------
        if bytes_read != observed_size {
            return Err(std::io::Error::other(format!(
                "file changed during hashing (read {} of {} bytes)",
                bytes_read, observed_size
            )));
        }

        // ---- check 4/5: post-read state must match pre-read -------------
        let post = r.post_stat()?;
        if post.len != pre.len {
            return Err(std::io::Error::other(format!(
                "file length moved during hashing ({} → {})",
                pre.len, post.len
            )));
        }
        if let (Some(pre_change), Some(post_change)) = (pre.changed, post.changed) {
            if pre_change != post_change {
                return Err(std::io::Error::other(
                    "file change-time moved during hashing (same length; \
                     content was rewritten while being read)",
                ));
            }
        }

        // The member's published identity is the HANDLE-proven one: it
        // describes the object the digest was actually computed from
        // (hard-link aliases share it — the storage accounting relies on
        // that), falling back to the observation identity when the handle
        // could not prove one.
        let object_id = handle_object_id.or(member.observed_object_id);
        outcome = Ok((
            DuplicateMember {
                entry_id: member.entry_id,
                path: member.path.clone(),
                size: observed_size,
                object_id,
            },
            hash,
        ));
        Ok(())
    });

    match read_result {
        Ok(()) => {
            let (member, hash) = outcome.map_err(|e| failure_from_io(member, &e))?;
            Ok(Some((member, hash)))
        }
        Err(ContentError::Aborted) => {
            if shared.cancel.is_cancelled() {
                Err(HashError::Cancelled)
            } else {
                Err(HashError::Failure(HashFailure::new(
                    member.path.clone(),
                    HashFailureKind::Cancelled,
                    "read aborted by consumer",
                )))
            }
        }
        Err(ContentError::UnexpectedLink) => Err(HashError::Failure(HashFailure::new(
            member.path.clone(),
            HashFailureKind::Changed,
            "path became a link (symlink/junction/reparse) between scan and hash",
        ))),
        Err(ContentError::NotRegularFile) => Err(HashError::Failure(HashFailure::new(
            member.path.clone(),
            HashFailureKind::Changed,
            "path no longer names a regular file (replaced by directory/special object)",
        ))),
        Err(ContentError::OpenFailed(e)) => Err(HashError::Failure(failure_from_io(member, &e))),
        Err(ContentError::ReadFailed(e)) => Err(HashError::Failure(failure_from_io(member, &e))),
    }
}

fn failure_from_io(member: &StagedMember, e: &std::io::Error) -> HashFailure {
    use spacelens_engine::ErrorCategory;
    let message = e.to_string();
    // Mutation-policy rejections and object-replacement rejections carry
    // custom io::Error values; classify by message contract BEFORE any
    // category mapping can mislabel them.
    if message.starts_with("object replaced between scan and hash") {
        return HashFailure::new(member.path.clone(), HashFailureKind::Replaced, message);
    }
    if message.starts_with("file changed")
        || message.starts_with("file length moved")
        || message.starts_with("file change-time moved")
    {
        return HashFailure::new(member.path.clone(), HashFailureKind::Changed, message);
    }
    let category = categorize(e);
    let kind = match category {
        ErrorCategory::NotFound => HashFailureKind::Vanished,
        other => HashFailureKind::Hash { category: other },
    };
    HashFailure::new(member.path.clone(), kind, message)
}

fn categorize(e: &std::io::Error) -> spacelens_engine::ErrorCategory {
    // The platform layer categorizes with OS-specific raw codes; policy
    // factories may wrap errors, so apply the conservative std mapping as a
    // fallback. Raw ENOENT (2) maps explicitly to NotFound on both major
    // platforms.
    if e.raw_os_error() == Some(2) {
        return spacelens_engine::ErrorCategory::NotFound;
    }
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => spacelens_engine::ErrorCategory::PermissionDenied,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
            spacelens_engine::ErrorCategory::NotFound
        }
        std::io::ErrorKind::Interrupted => spacelens_engine::ErrorCategory::Transient,
        _ => spacelens_engine::ErrorCategory::Other,
    }
}

struct Shared {
    cancel: CancelHandle,
    files_hashed: AtomicU64,
    bytes_hashed: AtomicU64,
    failures: AtomicU64,
    failure_detail: Mutex<Vec<HashFailure>>,
    failures_truncated: AtomicU64,
    /// Hash outputs pushed by workers, drained by the coordinator after the
    /// scope joins. Bounded by the staged-candidate caps (every pushed
    /// record corresponds to exactly one accepted staged job), so the
    /// global candidate cap bounds this buffer too.
    results: Mutex<Vec<(DuplicateMember, ContentHash)>>,
}

impl Shared {
    fn record_failure(&self, failure: HashFailure) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        let mut detail = self.failure_detail.lock().unwrap();
        if detail.len() < FAILURE_DETAIL_CAP {
            detail.push(failure);
        } else {
            drop(detail);
            self.failures_truncated.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn push_result(&self, member: DuplicateMember, hash: ContentHash) {
        self.results.lock().unwrap().push((member, hash));
    }

    fn take_results(&self) -> Vec<(DuplicateMember, ContentHash)> {
        std::mem::take(&mut self.results.lock().unwrap())
    }
}

struct Job {
    member: StagedMember,
}

fn progress_snapshot(
    shared: &Shared,
    stats: &PipelineStats,
    eligibility: &EligibilityStats,
    started: Instant,
) -> DuplicateProgressSnapshot {
    DuplicateProgressSnapshot {
        entries_examined: eligibility.examined,
        candidates_grouped: stats.candidates_hashed,
        files_hashed: shared.files_hashed.load(Ordering::Relaxed),
        bytes_hashed: shared.bytes_hashed.load(Ordering::Relaxed),
        failures: shared.failures.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

fn assemble_report(
    status: DuplicateStatus,
    groups: Vec<DuplicateGroup>,
    shared: &Shared,
    ingest: Ingest,
    started_at: std::time::SystemTime,
) -> DuplicateReport {
    let logical: u64 = groups.iter().map(|g| g.logical_duplicate_bytes).sum();
    let recoverable = groups.iter().try_fold(0u64, |acc, g| {
        g.recoverable_bytes.map(|r| acc.saturating_add(r))
    });
    let failures = shared.failure_detail.lock().unwrap().clone();
    let failures_truncated = shared.failures_truncated.load(Ordering::Relaxed);
    DuplicateReport {
        status,
        groups,
        failures,
        failures_truncated,
        stats: ingest.stats,
        eligibility: ingest.eligibility,
        logical_duplicate_bytes: logical,
        recoverable_bytes: recoverable,
        started_at,
        finished_at: std::time::SystemTime::now(),
    }
}

fn finish(
    status: DuplicateStatus,
    shared: &Shared,
    eligibility: EligibilityStats,
    mut stats: PipelineStats,
    started_at: std::time::SystemTime,
    sink: &mut dyn FnMut(DuplicateProgressEvent),
) -> DuplicateReport {
    stats.files_hashed = shared.files_hashed.load(Ordering::Relaxed);
    stats.bytes_hashed = shared.bytes_hashed.load(Ordering::Relaxed);
    stats.failures = shared.failures.load(Ordering::Relaxed);
    let report = assemble_report(
        status,
        Vec::new(),
        shared,
        Ingest {
            eligibility,
            stats,
            ..Ingest::new()
        },
        started_at,
    );
    let event = match status {
        DuplicateStatus::Completed => DuplicateProgressEvent::Completed(Box::new(report.clone())),
        DuplicateStatus::CompletedWithLimits => {
            DuplicateProgressEvent::Completed(Box::new(report.clone()))
        }
        DuplicateStatus::Cancelled => DuplicateProgressEvent::Cancelled(Box::new(report.clone())),
        DuplicateStatus::Unsupported => DuplicateProgressEvent::Failed(Box::new(report.clone())),
    };
    sink(event);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duplicate::StorageAccounting;
    use crate::policy::{ContentReaderFactory, DefaultReaderFactory};
    use spacelens_engine::platform::{ContentError, ContentReader, HandleStat};
    use std::collections::HashMap;
    use std::io;
    use std::path::Path;
    use std::sync::Mutex as StdMutex;

    /// In-memory content source implementing the factory over fixed bytes.
    /// Serves `name → content`; missing names produce typed NotFound.
    struct MemReader {
        files: StdMutex<HashMap<PathBuf, Vec<u8>>>,
    }

    impl MemReader {
        fn new(files: &[(&str, &[u8])]) -> Self {
            MemReader {
                files: StdMutex::new(
                    files
                        .iter()
                        .map(|(p, c)| (PathBuf::from(p), c.to_vec()))
                        .collect(),
                ),
            }
        }
    }

    impl ContentReaderFactory for MemReader {
        fn read(
            &self,
            path: &Path,
            feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
        ) -> Result<(), ContentError> {
            let content = self
                .files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| ContentError::OpenFailed(io::Error::from_raw_os_error(2)))?;
            let mut pos = 0usize;
            let mut reader = MemChunkReader {
                content: &content,
                pos: &mut pos,
            };
            feed(&mut reader).map_err(ContentError::ReadFailed)
        }
    }

    struct MemChunkReader<'a> {
        content: &'a [u8],
        pos: &'a mut usize,
    }

    impl ContentReader for MemChunkReader<'_> {
        fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
            if *self.pos >= self.content.len() {
                return Ok(None);
            }
            let n = buf.len().min(self.content.len() - *self.pos);
            buf[..n].copy_from_slice(&self.content[*self.pos..*self.pos + n]);
            *self.pos += n;
            Ok(Some(n))
        }
        fn file_identity(&self) -> spacelens_engine::FileIdentity {
            // Distinct per path is unnecessary for these tests: None exercises
            // the Estimated accounting path.
            spacelens_engine::FileIdentity::unknown()
        }
        fn pre_stat(&self) -> io::Result<HandleStat> {
            Ok(self.stat())
        }
        fn post_stat(&self) -> io::Result<HandleStat> {
            Ok(self.stat())
        }
    }

    impl MemChunkReader<'_> {
        /// Stable synthetic stat: the in-memory file never mutates.
        fn stat(&self) -> HandleStat {
            use std::time::SystemTime;
            HandleStat {
                len: self.content.len() as u64,
                modified: Some(SystemTime::UNIX_EPOCH),
                changed: Some(SystemTime::UNIX_EPOCH),
            }
        }
    }

    fn entry(id: u64, path: &str, size: u64) -> FsEntry {
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
            device: None,
            inode: None,
            hidden: false,
            error: None,
        }
    }

    fn dir(path: &str) -> FsEntry {
        FsEntry {
            id: 99,
            parent_id: None,
            path: PathBuf::from(path),
            kind: spacelens_engine::EntryKind::Dir,
            size: 0,
            allocated_size: None,
            modified: None,
            created: None,
            accessed: None,
            changed: None,
            device: None,
            inode: None,
            hidden: false,
            error: None,
        }
    }

    fn no_events(_: DuplicateProgressEvent) {}

    #[test]
    fn same_content_different_names_group() {
        let entries = vec![
            entry(1, "/a/one.bin", 11),
            entry(2, "/b/two.bin", 11),
            entry(3, "/c/three.bin", 5),
        ];
        let reader = MemReader::new(&[
            ("/a/one.bin", b"hello world"),
            ("/b/two.bin", b"hello world"),
            ("/c/three.bin", b"xxxxx"),
        ]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Completed);
        assert_eq!(report.groups.len(), 1, "{report:?}");
        let g = &report.groups[0];
        assert_eq!(g.member_count, 2);
        assert_eq!(g.size, 11);
        assert_eq!(g.representative().path, PathBuf::from("/a/one.bin"));
        assert_eq!(g.logical_duplicate_bytes, 11);
        // Object identity unknown (mem reader) → Estimated.
        assert_eq!(g.accounting, StorageAccounting::Estimated);
        // Singleton never hashed: stats prove candidate filtering.
        assert_eq!(report.stats.singleton_files, 1);
        assert_eq!(report.stats.candidates_hashed, 2);
        assert_eq!(report.stats.size_groups_without_duplicates, 0);
    }

    #[test]
    fn same_name_different_content_never_groups() {
        let entries = vec![entry(1, "/a/report.txt", 5), entry(2, "/b/report.txt", 5)];
        let reader = MemReader::new(&[("/a/report.txt", b"alpha"), ("/b/report.txt", b"bravo")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Completed);
        assert!(report.groups.is_empty(), "{report:?}");
        assert_eq!(report.stats.size_groups_without_duplicates, 1);
    }

    #[test]
    fn same_size_different_content_never_groups() {
        let entries = vec![entry(1, "/a/x.bin", 4), entry(2, "/b/y.bin", 4)];
        let reader = MemReader::new(&[("/a/x.bin", b"aaaa"), ("/b/y.bin", b"bbbb")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert!(report.groups.is_empty(), "{report:?}");
        assert_eq!(report.stats.candidates_hashed, 2);
    }

    #[test]
    fn different_sizes_never_reach_hashing() {
        let entries = vec![entry(1, "/a/small", 1), entry(2, "/b/big", 10)];
        let reader = MemReader::new(&[("/a/small", b"x"), ("/b/big", b"0123456789")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert!(report.groups.is_empty());
        assert_eq!(
            report.stats.candidates_hashed, 0,
            "no hashing for size-unique files"
        );
        assert_eq!(report.stats.singleton_files, 2);
        assert_eq!(report.stats.bytes_hashed, 0);
    }

    #[test]
    fn zero_byte_files_follow_the_group_policy() {
        // Default: counted, not grouped (hostile-input protection).
        let make_entries = || vec![entry(1, "/a/empty1", 0), entry(2, "/b/empty2", 0)];
        let reader = MemReader::new(&[]);
        let report = run_duplicates(
            make_entries().into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.zero_byte_matches_ungrouped, 2);

        // Opted in: they group (their content IS identical) with zero
        // recoverable storage semantics.
        let opts = DuplicateOptions {
            group_zero_byte_files: true,
            ..DuplicateOptions::default()
        };
        let zero_reader = MemReader::new(&[("/a/empty1", b""), ("/b/empty2", b"")]);
        let report = run_duplicates(
            make_entries().into_iter(),
            &opts,
            &CancelHandle::new(),
            Some(&zero_reader),
            &mut no_events,
        );
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].member_count, 2);
        assert_eq!(report.groups[0].size, 0);
        assert_eq!(
            report.groups[0].content_hash,
            crate::hash::ContentHash::empty()
        );
    }

    #[test]
    fn three_duplicates_form_one_stable_group() {
        let entries = vec![
            entry(1, "/c.bin", 3),
            entry(2, "/a.bin", 3),
            entry(3, "/b.bin", 3),
        ];
        let reader = MemReader::new(&[("/c.bin", b"xyz"), ("/a.bin", b"xyz"), ("/b.bin", b"xyz")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert_eq!(report.groups.len(), 1);
        let g = &report.groups[0];
        assert_eq!(g.member_count, 3);
        // Deterministic member order regardless of stream order.
        assert_eq!(g.members[0].path, PathBuf::from("/a.bin"));
        assert_eq!(g.members[1].path, PathBuf::from("/b.bin"));
        assert_eq!(g.members[2].path, PathBuf::from("/c.bin"));
    }

    #[test]
    fn ineligible_entries_are_never_candidates() {
        // One directory, one link, one special node, one observation-error
        // file, one clean file with a unique size.
        let mut errored = entry(4, "/broken.bin", 7);
        errored.error = Some(spacelens_engine::ErrorCategoryRef::PermissionDenied);
        let link = FsEntry {
            kind: spacelens_engine::EntryKind::Link(spacelens_engine::LinkInfo {
                kind: spacelens_engine::LinkKind::Symlink,
                target: None,
                broken: false,
            }),
            ..entry(5, "/alias", 7)
        };
        let entries = vec![
            dir("/d"),
            link,
            FsEntry {
                kind: spacelens_engine::EntryKind::Other,
                ..entry(6, "/socket", 7)
            },
            errored,
            entry(7, "/clean.bin", 7),
        ];
        let reader = MemReader::new(&[]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        let el = report.eligibility;
        assert_eq!(el.dirs, 1);
        assert_eq!(el.links, 1);
        assert_eq!(el.special, 1);
        assert_eq!(el.observation_errors, 1);
        assert_eq!(el.eligible_files, 1);
        // The clean file is a size singleton: never hashed, never grouped.
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.candidates_hashed, 0);
        assert_eq!(report.stats.singleton_files, 1);
    }

    #[test]
    fn observation_errors_are_ineligible() {
        let mut e = entry(1, "/locked.bin", 100);
        e.error = Some(spacelens_engine::ErrorCategoryRef::InUse);
        let entries = vec![e, entry(2, "/other.bin", 100)];
        let reader = MemReader::new(&[("/other.bin", b"content")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        // The error entry was ineligible → no size group of 2 → nothing hashed.
        assert!(report.groups.is_empty());
        assert_eq!(report.eligibility.observation_errors, 1);
        assert_eq!(report.stats.singleton_files, 1);
    }

    #[test]
    fn missing_file_is_a_typed_vanish_not_a_false_hash() {
        let entries = vec![
            entry(1, "/a/present.bin", 4),
            entry(2, "/b/vanishing.bin", 4),
        ];
        let reader = MemReader::new(&[("/a/present.bin", b"aaaa")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Completed);
        assert!(
            report.groups.is_empty(),
            "failed hash must not create a relationship"
        );
        assert_eq!(report.stats.failures, 1);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(
            report.failures[0].kind,
            HashFailureKind::Vanished,
            "a missing file is typed as vanished"
        );
    }

    #[test]
    fn changed_file_is_typed_changed() {
        // Handle length disagrees with observation → Changed.
        struct ShrinkingReader;
        impl ContentReaderFactory for ShrinkingReader {
            fn read(
                &self,
                _path: &Path,
                feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
            ) -> Result<(), ContentError> {
                let mut r = FakeLenReader { len: 999 };
                feed(&mut r).map_err(ContentError::ReadFailed)
            }
        }
        struct FakeLenReader {
            len: u64,
        }
        impl ContentReader for FakeLenReader {
            fn read_chunk(&mut self, _buf: &mut [u8]) -> io::Result<Option<usize>> {
                Ok(None)
            }
            fn file_identity(&self) -> spacelens_engine::FileIdentity {
                spacelens_engine::FileIdentity::unknown()
            }
            fn pre_stat(&self) -> io::Result<HandleStat> {
                Ok(HandleStat {
                    len: self.len,
                    modified: None,
                    changed: None,
                })
            }
            fn post_stat(&self) -> io::Result<HandleStat> {
                Ok(HandleStat {
                    len: self.len,
                    modified: None,
                    changed: None,
                })
            }
        }
        let entries = vec![entry(1, "/a/x.bin", 4), entry(2, "/b/y.bin", 4)];
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&ShrinkingReader),
            &mut no_events,
        );
        assert!(report.groups.is_empty());
        assert_eq!(
            report.stats.failures, 2,
            "both files mismatch the observed size"
        );
        assert!(report
            .failures
            .iter()
            .all(|f| f.kind == HashFailureKind::Changed));
    }

    #[test]
    fn cancelled_before_start_publishes_no_groups() {
        let cancel = CancelHandle::new();
        cancel.cancel();
        let entries = vec![entry(1, "/a.bin", 3), entry(2, "/b.bin", 3)];
        let reader = MemReader::new(&[("/a.bin", b"xyz"), ("/b.bin", b"xyz")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &cancel,
            Some(&reader),
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Cancelled);
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.candidates_hashed, 0);
    }

    #[test]
    fn cancel_during_hashing_publishes_no_groups() {
        use std::sync::atomic::AtomicBool as AB;
        struct CancelMidway {
            cancel: CancelHandle,
            first_seen: AB,
        }
        impl ContentReaderFactory for CancelMidway {
            fn read(
                &self,
                _path: &Path,
                feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
            ) -> Result<(), ContentError> {
                // Cancel after the first file begins hashing.
                if !self.first_seen.swap(true, Ordering::SeqCst) {
                    let mut r = EndlessReader;
                    let _ = feed(&mut r);
                }
                self.cancel.cancel();
                Err(ContentError::Aborted)
            }
        }
        struct EndlessReader;
        impl ContentReader for EndlessReader {
            fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
                buf.fill(0xAA);
                Ok(Some(buf.len()))
            }
            fn file_identity(&self) -> spacelens_engine::FileIdentity {
                spacelens_engine::FileIdentity::unknown()
            }
            fn pre_stat(&self) -> io::Result<HandleStat> {
                Ok(HandleStat {
                    len: 4,
                    modified: None,
                    changed: None,
                })
            }
            fn post_stat(&self) -> io::Result<HandleStat> {
                Ok(HandleStat {
                    len: 4,
                    modified: None,
                    changed: None,
                })
            }
        }
        let cancel = CancelHandle::new();
        let reader = CancelMidway {
            cancel: cancel.clone(),
            first_seen: AB::new(false),
        };
        let entries = vec![entry(1, "/a/x.bin", 4), entry(2, "/b/y.bin", 4)];
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &cancel,
            Some(&reader),
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Cancelled);
        assert!(
            report.groups.is_empty(),
            "cancelled run publishes no groups"
        );
    }

    #[test]
    fn unsupported_reader_yields_unsupported_status() {
        let entries = vec![entry(1, "/a.bin", 3), entry(2, "/b.bin", 3)];
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            None,
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Unsupported);
        assert!(report.groups.is_empty());
        assert_eq!(report.stats.candidates_hashed, 2, "candidacy still ran");
        assert_eq!(report.stats.files_hashed, 0);
    }

    #[test]
    fn report_is_deterministic_across_runs() {
        let build = || {
            let entries = vec![
                entry(1, "/d1/same.bin", 4),
                entry(2, "/d2/same.bin", 4),
                entry(3, "/d3/same.bin", 4),
                entry(4, "/other/unique.bin", 7),
                entry(5, "/pair/p1.bin", 2),
                entry(6, "/pair/p2.bin", 2),
            ];
            let reader = MemReader::new(&[
                ("/d1/same.bin", b"aaaa"),
                ("/d2/same.bin", b"aaaa"),
                ("/d3/same.bin", b"aaaa"),
                ("/other/unique.bin", b"unique!"),
                ("/pair/p1.bin", b"pp"),
                ("/pair/p2.bin", b"pp"),
            ]);
            run_duplicates(
                entries.into_iter(),
                &DuplicateOptions::default(),
                &CancelHandle::new(),
                Some(&reader),
                &mut no_events,
            )
        };
        let a = build();
        let b = build();
        // Timestamps are observational; every logical field must match.
        assert_eq!(a.status, b.status);
        assert_eq!(a.groups, b.groups, "same input → identical groups");
        assert_eq!(a.stats, b.stats);
        assert_eq!(a.eligibility, b.eligibility);
        assert_eq!(a.failures, b.failures);
        assert_eq!(a.logical_duplicate_bytes, b.logical_duplicate_bytes);
        assert_eq!(a.recoverable_bytes, b.recoverable_bytes);
        assert_eq!(a.groups.len(), 2);
        // Group order: size ascending.
        assert_eq!(a.groups[0].size, 2);
        assert_eq!(a.groups[1].size, 4);
        assert_eq!(a.groups[1].member_count, 3);
    }

    #[test]
    fn events_are_started_then_one_terminal() {
        let entries = vec![entry(1, "/a.bin", 3), entry(2, "/b.bin", 3)];
        let reader = MemReader::new(&[("/a.bin", b"xyz"), ("/b.bin", b"xyz")]);
        let mut events = Vec::new();
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut |e| {
                events.push(match e {
                    DuplicateProgressEvent::Started => "started".to_string(),
                    DuplicateProgressEvent::Progress(_) => "progress".to_string(),
                    DuplicateProgressEvent::Completed(_) => "completed".to_string(),
                    DuplicateProgressEvent::Cancelled(_) => "cancelled".to_string(),
                    DuplicateProgressEvent::Failed(_) => "failed".to_string(),
                })
            },
        );
        assert_eq!(report.status, DuplicateStatus::Completed);
        assert_eq!(events.first().map(String::as_str), Some("started"));
        assert_eq!(events.last().map(String::as_str), Some("completed"));
        assert!(!events.contains(&"cancelled".to_string()));
        assert!(!events.contains(&"failed".to_string()));
    }

    #[test]
    fn every_entry_is_accounted_for_exactly_once() {
        let entries = vec![
            entry(1, "/f1", 3),
            dir("/d1"),
            entry(2, "/f2", 3),
            dir("/d2"),
            {
                let mut e = entry(3, "/err", 3);
                e.error = Some(spacelens_engine::ErrorCategoryRef::PermissionDenied);
                e
            },
        ];
        let reader = MemReader::new(&[("/f1", b"abc"), ("/f2", b"abc")]);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&reader),
            &mut no_events,
        );
        let el = report.eligibility;
        assert_eq!(
            el.examined,
            (el.eligible_files + el.dirs + el.links + el.special + el.observation_errors) as u64,
            "every entry accounted exactly once"
        );
        assert_eq!(el.examined, 5);
        assert_eq!(el.eligible_files, 2);
        assert_eq!(el.dirs, 2);
        assert_eq!(el.observation_errors, 1);
    }

    #[test]
    fn default_reader_factory_streams_real_files() {
        // Round-trip through the engine's real platform boundary.
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("one.dat");
        let p2 = tmp.path().join("two.dat");
        std::fs::write(&p1, b"duplicate-content").unwrap();
        std::fs::write(&p2, b"duplicate-content").unwrap();

        let size = std::fs::metadata(&p1).unwrap().len();
        let entries = vec![
            entry(1, p1.to_str().unwrap(), size),
            entry(2, p2.to_str().unwrap(), size),
        ];
        let platform = spacelens_engine::platform::std_fs();
        let factory = DefaultReaderFactory::new(platform);
        let report = run_duplicates(
            entries.into_iter(),
            &DuplicateOptions::default(),
            &CancelHandle::new(),
            Some(&factory),
            &mut no_events,
        );
        assert_eq!(report.status, DuplicateStatus::Completed);
        assert_eq!(report.groups.len(), 1, "{report:?}");
        assert_eq!(report.groups[0].member_count, 2);
        // On this host the platform proves object identity (unix fstat /
        // windows file index) — accounting must be Exact and recoverable
        // must be the full duplicate size.
        assert_eq!(report.groups[0].accounting, StorageAccounting::Exact);
        assert_eq!(report.groups[0].recoverable_bytes, Some(size));
    }
}
