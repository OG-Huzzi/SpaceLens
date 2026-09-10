//! The duplicate detection pipeline (Phase 3 STEP 17).
//!
//! ```text
//! Observed FsEntry
//!       ↓  ingest: eligibility contract
//! Size grouping             (same size ⇒ candidacy only, never equality)
//!       ↓  bounded worker pool, bounded candidates per size group
//! Content hashing           (streaming SHA-256, mutation-checked)
//!       ↓
//! Content identity grouping (same digest ⇒ same bytes)
//!       ↓  deterministic ordering (size asc, then hash bytes asc)
//! DuplicateReport + typed progress events + typed failures
//! ```
//!
//! Concurrency model (STEP 13): a **fixed, bounded worker pool** (never
//! thread-per-file), fed from pre-grouped candidate batches after metadata
//! filtering. Workers read content only through the
//! [`ContentReaderFactory`] boundary — the pipeline never walks the
//! filesystem. Channels are bounded (natural backpressure); the job queue
//! holds paths, never content; hard caps ([`DuplicateOptions`]) keep even
//! hostile inputs (millions of same-size files) bounded.
//!
//! Cancellation (STEP 14): checked at ingest, between jobs, and per chunk
//! inside the hash. A cancelled run reports [`DuplicateStatus::Cancelled`]
//! with no groups — partial state never escapes as a valid result.
//!
//! Progress (STEP 15): staged, typed snapshots at a caller-set interval.
//! There is deliberately **no percent-complete**: before hashing finishes
//! the engine cannot honestly estimate remaining work, and byte-based
//! percent would require reading every byte it is trying to skip.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use spacelens_engine::platform::ContentReader;
use spacelens_engine::{CancelHandle, FsEntry};

use crate::duplicate::{DuplicateGroup, DuplicateMember};
use crate::eligibility::{Eligibility, EligibilityStats, IneligibleReason};
use crate::error::{HashError, HashFailure, HashFailureKind};
use crate::hash::{ContentHash, ContentHasher};
use crate::policy::{
    ContentReaderFactory, MutationPolicy, DEFAULT_MAX_CANDIDATES_PER_GROUP,
    DEFAULT_MAX_GROUP_MEMBERS_REPORTED,
};

/// Terminal state of one duplicate-detection run. Mirrors the scan model:
/// a cancellation is a successful cancellation, never an error, and never
/// masquerades as `Completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateStatus {
    /// Ingest + hashing + grouping all finished; the report is complete.
    Completed,
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
    /// Distinct eligible sizes.
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
    pub candidates_skipped_by_cap: u64,
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

struct Shared {
    cancel: CancelHandle,
    files_hashed: AtomicU64,
    bytes_hashed: AtomicU64,
    failures: AtomicU64,
    failure_detail: Mutex<Vec<HashFailure>>,
    failures_truncated: AtomicU64,
    /// Hash outputs pushed by workers, drained by the coordinator after the
    /// scope joins. O(candidates hashed); the candidate caps bound it.
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
    member: DuplicateMember,
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

    // ---- Stage 1: ingest + eligibility + size grouping ------------------
    // BTreeMap keeps size groups key-ordered (deterministic stage output
    // even before hashing). Values hold members in observation order.
    let mut eligibility = EligibilityStats::default();
    let mut size_groups: BTreeMap<u64, Vec<DuplicateMember>> = BTreeMap::new();
    for entry in entries {
        if cancel.is_cancelled() {
            return finish(
                DuplicateStatus::Cancelled,
                &shared,
                eligibility,
                PipelineStats::default(),
                started_at,
                sink,
            );
        }
        eligibility.examined = eligibility.examined.saturating_add(1);
        match Eligibility::of(&entry) {
            Eligibility::Eligible => {
                eligibility.eligible_files = eligibility.eligible_files.saturating_add(1);
                if entry.size == 0 {
                    eligibility.zero_byte_files = eligibility.zero_byte_files.saturating_add(1);
                }
                if entry.size >= options.min_file_size {
                    size_groups
                        .entry(entry.size)
                        .or_default()
                        .push(DuplicateMember {
                            entry_id: entry.id,
                            path: entry.path.clone(),
                            size: entry.size,
                            // Observation-time identity; hashing replaces it
                            // with handle-proven identity when available.
                            object_id: entry.device.zip(entry.inode),
                        });
                }
            }
            Eligibility::Ineligible(reason) => match reason {
                IneligibleReason::Directory => eligibility.dirs += 1,
                IneligibleReason::Link => eligibility.links += 1,
                IneligibleReason::Special => eligibility.special += 1,
                IneligibleReason::ObservationError { .. } => eligibility.observation_errors += 1,
            },
        }
    }

    let mut stats = PipelineStats {
        entries_examined: eligibility.examined,
        eligible_files: eligibility.eligible_files,
        zero_byte_files: eligibility.zero_byte_files,
        size_groups: size_groups.len() as u64,
        ..PipelineStats::default()
    };

    // ---- Stage 2: candidate selection -----------------------------------
    // Only groups with ≥2 members are hashed; singletons never cost one
    // read byte (STEP 5).
    let mut candidates_to_hash: Vec<(u64, Vec<DuplicateMember>)> = Vec::new();
    let mut zero_byte_ungrouped: u64 = 0;
    for (size, members) in size_groups {
        let n = members.len();
        if n < 2 {
            stats.singleton_files = stats.singleton_files.saturating_add(n as u64);
            continue;
        }
        if size == 0 && !options.group_zero_byte_files {
            zero_byte_ungrouped = zero_byte_ungrouped.saturating_add(n as u64);
            continue;
        }
        stats.size_groups_needing_hashes = stats.size_groups_needing_hashes.saturating_add(1);
        stats.candidates_skipped_by_cap = stats
            .candidates_skipped_by_cap
            .saturating_add(n.saturating_sub(options.max_candidates_per_group) as u64);
        let capped: Vec<DuplicateMember> = if n > options.max_candidates_per_group {
            members
                .into_iter()
                .take(options.max_candidates_per_group)
                .collect()
        } else {
            members
        };
        stats.candidates_hashed = stats.candidates_hashed.saturating_add(capped.len() as u64);
        candidates_to_hash.push((size, capped));
    }
    stats.zero_byte_matches_ungrouped = zero_byte_ungrouped;

    // ---- Stage 3: bounded hashing pool ----------------------------------
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
                    let Ok(job) = job else {
                        break;
                    };
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
            for (_size, members) in &candidates_to_hash {
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
                        &stats,
                        &eligibility,
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
            eligibility,
            stats,
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
    stats.size_groups_without_duplicates = candidates_to_hash
        .iter()
        .filter(|(size, _)| groups_by_size.get(size).copied().unwrap_or(0) == 0)
        .count() as u64;
    stats.files_hashed = shared.files_hashed.load(Ordering::Relaxed);
    stats.bytes_hashed = shared.bytes_hashed.load(Ordering::Relaxed);
    stats.failures = shared.failures.load(Ordering::Relaxed);

    let status = if reader.is_none() && stats.candidates_hashed > 0 {
        DuplicateStatus::Unsupported
    } else {
        DuplicateStatus::Completed
    };

    let report = assemble_report(status, groups, &shared, eligibility, stats, started_at);
    sink(DuplicateProgressEvent::Completed(Box::new(report.clone())));
    report
}

// ---- hashing worker internals -------------------------------------------

/// Hash one file under the full mutation policy (`MutationPolicy::Reject`):
/// 1. the open handle's length must equal the observed size,
/// 2. every chunk is read with cancellation checks; interrupted reads are
///    retried inside the platform layer,
/// 3. total bytes read must equal the observed length.
///
/// Any mismatch is typed — a partial or changing file is never hashed into
/// an identity.
fn hash_file(
    member: &DuplicateMember,
    options: &DuplicateOptions,
    reader: &dyn ContentReaderFactory,
    shared: &Shared,
) -> Result<Option<(DuplicateMember, ContentHash)>, HashError> {
    if shared.cancel.is_cancelled() {
        return Err(HashError::Cancelled);
    }
    let _ = options; // mutation_policy is Reject — the only implemented policy
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
        // Pre-read consistency: handle length must match the observation.
        let len = r.file_len()?;
        if len != observed_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "file changed between scan and hash (observed {observed_size} bytes, handle reports {len})"
                ),
            ));
        }

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
        if bytes_read != observed_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "file changed during hashing (read {bytes_read} of {observed_size} bytes)"
                ),
            ));
        }
        let identity = r.file_identity();
        let object_id = identity.device.zip(identity.inode);
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
            let (member, hash) =
                outcome.map_err(|e| failure_from_io(member, &e, HashPhase::Read))?;
            Ok(Some((member, hash)))
        }
        Err(spacelens_engine::platform::ContentError::Aborted) => {
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
        Err(spacelens_engine::platform::ContentError::OpenFailed(e)) => Err(HashError::Failure(
            failure_from_io(member, &e, HashPhase::Open),
        )),
        Err(spacelens_engine::platform::ContentError::ReadFailed(e)) => Err(HashError::Failure(
            failure_from_io(member, &e, HashPhase::Read),
        )),
    }
}

enum HashPhase {
    Open,
    Read,
}

fn failure_from_io(member: &DuplicateMember, e: &std::io::Error, phase: HashPhase) -> HashFailure {
    use spacelens_engine::ErrorCategory;
    // Mutation-policy rejections carry InvalidData and MUST be typed
    // `Changed` before any category mapping can mislabel them.
    let kind = if e.kind() == std::io::ErrorKind::InvalidData {
        HashFailureKind::Changed
    } else {
        let category = categorize(e);
        match category {
            ErrorCategory::NotFound => HashFailureKind::Vanished,
            ErrorCategory::PermissionDenied => HashFailureKind::Hash {
                category: ErrorCategory::PermissionDenied,
            },
            ErrorCategory::InUse => HashFailureKind::Hash {
                category: ErrorCategory::InUse,
            },
            _ => HashFailureKind::Hash { category },
        }
    };
    let _ = phase;
    HashFailure::new(member.path.clone(), kind, e.to_string())
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
    eligibility: EligibilityStats,
    stats: PipelineStats,
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
        stats,
        eligibility,
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
    let report = assemble_report(status, Vec::new(), shared, eligibility, stats, started_at);
    let event = match status {
        DuplicateStatus::Completed => DuplicateProgressEvent::Completed(Box::new(report.clone())),
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
    use spacelens_engine::platform::ContentError;
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
        fn file_len(&self) -> io::Result<u64> {
            Ok(self.content.len() as u64)
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
            fn file_len(&self) -> io::Result<u64> {
                Ok(self.len)
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
            fn file_len(&self) -> io::Result<u64> {
                Ok(4)
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
