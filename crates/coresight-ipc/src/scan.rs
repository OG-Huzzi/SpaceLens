//! The real scan backend: a filesystem walk that becomes session facts.
//!
//! [`EngineScanBackend`] is the only implementation of [`ScanBackend`]. It
//! wraps the Phase 1 engine (`coresight_engine::scan_with`) and turns the
//! streamed observations into a [`Snapshot`] and a [`UsageReport`]:
//!
//! 1. every emitted `FsEntry` is classified with `coresight_classifier` and
//!    pushed into a [`SnapshotBuilder`];
//! 2. once the walk completes, the Phase 3 duplicate pipeline
//!    (`coresight_identity::run_duplicates`) hashes the files that share a
//!    size. Only members the pipeline *actually verified* get a
//!    `content_sha256`; every other entry stays content-less (unknown), so
//!    the duplicate report can never claim an unverified duplicate;
//! 3. the snapshot is built (the builder sorts deterministically) and the
//!    usage report is aggregated with `coresight_insights::usage_by_category`.
//!
//! # Threading
//!
//! [`ScanBackend::start`] validates the root and spawns one `std::thread`.
//! The caller receives a [`ScanHandle`] whose `progress` receiver is drained
//! by the shell (or by a test). A scan therefore never blocks the command
//! thread. Cancellation flows through the engine's [`CancelHandle`].
//!
//! # Honesty rules
//!
//! * A missing/unreadable root is a typed [`ScanBackendError`] *before* any
//!   work starts — `startScan` cannot silently succeed on a path that does
//!   not exist.
//! * A cancelled or failed walk keeps the observations it made, but its
//!   status is never `Completed`, so the session never serves those bytes as
//!   a whole-scope total.
//! * A non-file entry that the engine observed successfully carries the
//!   engine's own logical size of `0` (directories and links contribute no
//!   file bytes). An entry whose metadata could not be read carries **no**
//!   size — the engine's placeholder `0` is not a measurement — so
//!   `entries_without_size`/`errored_entries` make `plan.partial` fire.
//! * No path, raw OS message, or stack trace is put into an IPC error.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use coresight_classifier::{classify, Platform};
use coresight_engine::{
    default_threads, CancelHandle, FsEntry, Phase, PlatformFs, ProgressSnapshot, ScanEvent,
    ScanOptions, ScanSummary,
};
use coresight_history::{ClassificationRef, ObservedKind, RunId, Snapshot, SnapshotBuilder};
use coresight_identity::{
    run_duplicates, DefaultReaderFactory, DuplicateOptions, DuplicateProgressEvent, DuplicateStatus,
};
use coresight_insights::{usage_by_category, UsageReport};

use crate::error::{codes, ApiError};
use crate::response::ScanStatus;

/// A scan backend the shell may attach to a [`Session`](crate::Session).
///
/// Exactly one method is required: starting a scan is asynchronous and
/// returns the handle the caller drains. Cancellation is performed through
/// the [`ScanHandle::cancel`] token the session retains, so a backend never
/// needs to expose a second lookup table.
pub trait ScanBackend: Send + Sync {
    /// Start one scan.
    ///
    /// # Errors
    ///
    /// A [`ScanBackendError`] when the request is invalid, the root cannot
    /// be observed, or the worker cannot be started. The error is typed and
    /// carries no path, raw OS message, or backtrace.
    fn start(&self, request: ScanRequest) -> Result<ScanHandle, ScanBackendError>;
}

/// Everything one scan needs: its id, its real root, and the engine options.
#[derive(Debug, Clone)]
pub struct ScanRequest {
    /// Stable id chosen by the caller (the session's scan id).
    pub scan_id: String,
    /// The directory (or single file) to walk. Always an absolute path:
    /// relative roots are refused rather than resolved against an implicit
    /// working directory.
    pub root: PathBuf,
    /// Engine scan options. `emit_entries` must stay `true` for a snapshot
    /// to exist; the backend does not override caller options.
    pub options: ScanOptions,
}

/// The live handle of one started scan.
///
/// The receiver yields [`ScanProgressUpdate`] values and ends after exactly
/// one [`ScanProgressUpdate::Finished`].
#[derive(Debug)]
pub struct ScanHandle {
    /// The scan this handle belongs to.
    pub scan_id: String,
    /// Cooperative cancellation token for the engine walk and hash stage.
    pub cancel: CancelHandle,
    /// Progress and terminal updates, in emission order.
    pub progress: Receiver<ScanProgressUpdate>,
}

/// One observation update from a running scan.
///
/// The stream always starts with [`ScanProgressUpdate::Started`], may contain
/// any number of [`ScanProgressUpdate::Progress`] values, and ends with
/// exactly one [`ScanProgressUpdate::Finished`]. Every update belongs to the
/// same scan id carried inside it.
#[derive(Debug, Clone)]
pub enum ScanProgressUpdate {
    /// The worker started; `started_at` is the engine's recorded start.
    Started {
        /// The scan id.
        scan_id: String,
        /// The root being walked, as supplied by the caller.
        root: PathBuf,
        /// The engine's recorded start instant.
        started_at: SystemTime,
    },
    /// A throttled counter snapshot emitted while walking or hashing.
    Progress {
        /// The scan id.
        scan_id: String,
        /// Real counters observed so far.
        snapshot: ScanProgressSnapshot,
    },
    /// The one terminal update. The scan is finished: completed, cancelled,
    /// or failed.
    Finished(Box<ScanFinished>),
}

/// Real counters of one scan, at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgressSnapshot {
    /// Engine phase (`preparing`, `walking`, `finalizing`).
    pub phase: Phase,
    /// Every entry observed so far (files + directories + links + other).
    pub entries_seen: u64,
    /// File entries observed so far.
    pub files_seen: u64,
    /// Directory entries observed so far.
    pub dirs_seen: u64,
    /// Logical bytes of files whose size was observed so far.
    pub bytes_seen: u64,
    /// Observation errors tallied so far.
    pub errors_seen: u64,
    /// Milliseconds since the engine walk began.
    pub elapsed_ms: u64,
}

/// The terminal result of one scan, handed to the session.
#[derive(Debug, Clone)]
pub struct ScanFinished {
    /// The scan id.
    pub scan_id: String,
    /// Terminal lifecycle state. Never [`ScanStatus::Running`].
    pub status: ScanStatus,
    /// The normalized per-path facts built from the real walk.
    pub snapshot: Snapshot,
    /// Per-category aggregation of that snapshot, computed with
    /// `coresight_insights::usage_by_category`.
    pub report: UsageReport,
    /// The engine's typed summary (counts, errors, timestamps).
    pub summary: ScanSummary,
    /// Observation-failure detail when the walk failed; `None` otherwise.
    pub failure: Option<ScanFailure>,
}

/// A typed, path-free description of why a scan failed or was incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFailure {
    /// Stable error-category codes with counts (for example
    /// `PERMISSION_DENIED:3`), never paths or raw OS messages.
    pub reason: String,
    /// Total observation errors tallied by the engine.
    pub error_count: u64,
}

/// Why a backend could not start a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanBackendError {
    /// The request itself is invalid (blank id, relative root).
    InvalidRequest(String),
    /// The root could not be observed at all. `category` is the stable
    /// engine error code, never a path or raw OS message.
    RootUnavailable {
        /// Stable engine error category code (`NOT_FOUND`, `PERMISSION_DENIED`, …).
        category: String,
    },
    /// The worker thread could not be spawned.
    WorkerUnavailable(String),
}

impl ScanBackendError {
    /// The stable IPC error code this backend failure maps to.
    pub fn code(&self) -> &'static str {
        match self {
            ScanBackendError::InvalidRequest(_) => codes::INVALID_REQUEST,
            ScanBackendError::RootUnavailable { .. } => codes::SCAN_ROOT_UNAVAILABLE,
            ScanBackendError::WorkerUnavailable(_) => codes::SCAN_UNAVAILABLE,
        }
    }

    /// Project this failure into the user-facing typed error envelope.
    ///
    /// The message is a reviewed sentence; `detail` carries only the stable
    /// category code or a short reason — never a path, an OS message, or a
    /// backtrace.
    pub fn to_api_error(&self) -> ApiError {
        match self {
            ScanBackendError::InvalidRequest(reason) => ApiError::new(
                codes::INVALID_REQUEST,
                "The scan request is not valid, so no scan was started. Nothing was changed.",
            )
            .with_detail(format!("invalid scan request: {reason}")),
            ScanBackendError::RootUnavailable { category } => ApiError::new(
                codes::SCAN_ROOT_UNAVAILABLE,
                "The scan root does not exist or cannot be observed, so no scan was \
                 started. Nothing was changed.",
            )
            .with_detail(format!("scan root unavailable: {category}")),
            ScanBackendError::WorkerUnavailable(reason) => ApiError::new(
                codes::SCAN_UNAVAILABLE,
                "The scan backend could not start a worker thread, so no scan was \
                 started. Nothing was changed.",
            )
            .with_detail(format!("scan worker unavailable: {reason}")),
        }
    }
}

impl std::fmt::Display for ScanBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanBackendError::InvalidRequest(reason) => {
                write!(f, "invalid scan request: {reason}")
            }
            ScanBackendError::RootUnavailable { category } => {
                write!(f, "scan root unavailable: {category}")
            }
            ScanBackendError::WorkerUnavailable(reason) => {
                write!(f, "scan worker unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for ScanBackendError {}

/// The production backend: the real Phase 1 walk.
///
/// This type is deliberately stateless; each [`ScanBackend::start`] call owns
/// its worker, its cancellation token, and its channel. The session keeps the
/// cancellation token, the shell keeps the receiver.
#[derive(Debug, Clone, Copy, Default)]
pub struct EngineScanBackend;

impl EngineScanBackend {
    /// Construct the production backend.
    pub fn new() -> Self {
        EngineScanBackend
    }
}

impl ScanBackend for EngineScanBackend {
    fn start(&self, request: ScanRequest) -> Result<ScanHandle, ScanBackendError> {
        if request.scan_id.trim().is_empty() {
            return Err(ScanBackendError::InvalidRequest(
                "the scan id must not be empty".to_string(),
            ));
        }
        if request.root.as_os_str().is_empty() {
            return Err(ScanBackendError::InvalidRequest(
                "the scan root must not be empty".to_string(),
            ));
        }
        if !request.root.is_absolute() {
            return Err(ScanBackendError::InvalidRequest(
                "the scan root must be an absolute path".to_string(),
            ));
        }
        if !request.options.emit_entries {
            // A counters-only walk produces no entries, so a snapshot built
            // from it would be empty while claiming success. Refuse instead.
            return Err(ScanBackendError::InvalidRequest(
                "emit_entries must be true to build a snapshot".to_string(),
            ));
        }

        let platform = coresight_engine::platform::std_fs();
        if let Err(error) = platform.metadata(&request.root) {
            return Err(ScanBackendError::RootUnavailable {
                category: platform.categorize_error(&error).code().to_string(),
            });
        }

        let cancel = CancelHandle::new();
        let (sender, receiver) = mpsc::channel();
        let worker = ScanWorker {
            scan_id: request.scan_id.clone(),
            root: request.root,
            options: request.options,
            cancel: cancel.clone(),
            sender,
            platform,
        };
        let thread = std::thread::Builder::new()
            .name("coresight-scan".to_string())
            .spawn(move || worker.run());
        if let Err(error) = thread {
            return Err(ScanBackendError::WorkerUnavailable(
                error.kind().to_string(),
            ));
        }

        Ok(ScanHandle {
            scan_id: request.scan_id,
            cancel,
            progress: receiver,
        })
    }
}

/// One worker's owned state. Everything is moved in, so the thread owns its
/// channel, token, and options outright.
struct ScanWorker {
    scan_id: String,
    root: PathBuf,
    options: ScanOptions,
    cancel: CancelHandle,
    sender: Sender<ScanProgressUpdate>,
    platform: &'static dyn PlatformFs,
}

impl ScanWorker {
    fn run(self) {
        let ScanWorker {
            scan_id,
            root,
            options,
            cancel,
            sender,
            platform,
        } = self;

        let started_at = SystemTime::now();
        let _ = sender.send(ScanProgressUpdate::Started {
            scan_id: scan_id.clone(),
            root: root.clone(),
            started_at,
        });

        let mut builder = SnapshotBuilder::new();
        // The duplicate pipeline consumes `FsEntry` values; the snapshot
        // builder only borrows them, so retaining the stream here is what
        // lets the verified content identities be attached afterwards.
        let mut entries: Vec<FsEntry> = Vec::new();
        let mut entries_seen: u64 = 0;

        let summary = {
            let mut sink = |event: ScanEvent| match event {
                ScanEvent::Started => {}
                ScanEvent::Entry(entry) => {
                    entries_seen = entries_seen.saturating_add(1);
                    let classification = classify(&entry, &Default::default(), Platform::current());
                    builder.push_entry(
                        &entry,
                        Some(ClassificationRef::from_parts(
                            classification.category,
                            classification.subcategory,
                        )),
                    );
                    entries.push(*entry);
                }
                ScanEvent::Progress(snapshot) => {
                    emit_progress(&sender, &scan_id, Phase::Walking, entries_seen, snapshot);
                }
                // The returned summary is authoritative; the terminal event
                // only mirrors it.
                ScanEvent::Completed(_) | ScanEvent::Cancelled(_) | ScanEvent::Failed(_) => {}
            };
            coresight_engine::scan_with(platform, &root, options, &cancel, &mut sink)
        };

        let mut status = match summary.status {
            coresight_engine::ScanStatus::Completed => {
                if summary.depth_capped {
                    // A depth cap truncates path observation, so the run did
                    // not observe its full declared scope. No terminal status
                    // claims otherwise.
                    ScanStatus::Failed
                } else {
                    ScanStatus::Completed
                }
            }
            coresight_engine::ScanStatus::Cancelled => ScanStatus::Cancelled,
            coresight_engine::ScanStatus::Failed => ScanStatus::Failed,
        };

        // Only a completed walk is worth hashing: a cancelled or failed walk
        // never serves a duplicate report, so hashing its subset would be
        // wasted work. The token is still checked by the hash pipeline, so a
        // cancellation during hashing is honored too.
        if status == ScanStatus::Completed && !cancel.is_cancelled() {
            let walk_files = count_kind(&entries, ObservedKind::File);
            let walk_dirs = count_kind(&entries, ObservedKind::Dir);
            let walk_bytes = summary.bytes;
            let walk_errors = summary.errors.total_errors();
            emit_progress(
                &sender,
                &scan_id,
                Phase::Finalizing,
                entries_seen,
                ProgressSnapshot {
                    files_seen: walk_files,
                    dirs_seen: walk_dirs,
                    bytes_seen: walk_bytes,
                    errors_seen: walk_errors,
                    elapsed_ms: 0,
                },
            );
            let reader = DefaultReaderFactory::new(platform);
            let duplicate_options = DuplicateOptions {
                // Zero-byte files share one content identity but can never
                // recover storage; the pipeline's own default already
                // declines to group them. One byte is the honest floor for a
                // candidate whose bytes are worth reading.
                min_file_size: 1,
                ..DuplicateOptions::default()
            };
            let report = run_duplicates(
                entries.into_iter(),
                &duplicate_options,
                &cancel,
                Some(&reader),
                // Hashing may take a while; only its elapsed time is
                // published. The walk's entry/byte counters are final, so the
                // UI's progress never goes backwards.
                &mut |event| {
                    if let DuplicateProgressEvent::Progress(progress) = event {
                        emit_progress(
                            &sender,
                            &scan_id,
                            Phase::Finalizing,
                            entries_seen,
                            ProgressSnapshot {
                                files_seen: walk_files,
                                dirs_seen: walk_dirs,
                                bytes_seen: walk_bytes,
                                errors_seen: walk_errors,
                                elapsed_ms: progress.elapsed_ms,
                            },
                        );
                    }
                },
            );
            if report.status == DuplicateStatus::Cancelled || cancel.is_cancelled() {
                status = ScanStatus::Cancelled;
            } else {
                for group in &report.groups {
                    let hex = group.content_hash.as_hex();
                    for member in &group.members {
                        builder.set_content(&member.path, hex.clone());
                    }
                }
            }
        }

        let snapshot = match builder.build(RunId(scan_id.clone())) {
            Ok(snapshot) => normalize_sizes(snapshot),
            Err(_) => {
                // The engine guarantees path-unique entries; if that
                // invariant were ever violated, the honest outcome is a
                // failed scan with no snapshot — never a merged or guessed
                // one.
                let _ = sender.send(ScanProgressUpdate::Finished(Box::new(ScanFinished {
                    scan_id: scan_id.clone(),
                    status: ScanStatus::Failed,
                    snapshot: Snapshot {
                        run_id: RunId(scan_id.clone()),
                        entries: Vec::new(),
                    },
                    report: usage_by_category(&Snapshot {
                        run_id: RunId(scan_id),
                        entries: Vec::new(),
                    }),
                    summary,
                    failure: Some(ScanFailure {
                        reason: "SNAPSHOT_BUILD_FAILED".to_string(),
                        error_count: 0,
                    }),
                })));
                return;
            }
        };

        let report = usage_by_category(&snapshot);
        let failure = failure_for(&summary, status);
        let _ = sender.send(ScanProgressUpdate::Finished(Box::new(ScanFinished {
            scan_id,
            status,
            snapshot,
            report: with_engine_errors(report, &summary),
            summary,
            failure,
        })));
    }
}

/// Fold errors the engine recorded for areas that have no emitted entry (for
/// example a directory whose listing was refused) into the usage report.
///
/// `UsageReport::errored_entries` counts entries that carry an
/// `observation_error`. A directory that could not be listed has no child
/// entry to carry one, but the engine still tallied the failure in
/// `ScanSummary::errors`; dropping it would let an incomplete walk look
/// complete. The adjustment adds exactly the errors that are **not** already
/// represented by an entry (`total_errors - entries_with_errors`), so a
/// metadata failure is never counted twice.
fn with_engine_errors(mut report: UsageReport, summary: &ScanSummary) -> UsageReport {
    let unrepresented = summary
        .errors
        .total_errors()
        .saturating_sub(summary.entries_with_errors);
    report.errored_entries = report.errored_entries.saturating_add(unrepresented);
    report
}

/// Emit one throttled progress update.
fn emit_progress(
    sender: &Sender<ScanProgressUpdate>,
    scan_id: &str,
    phase: Phase,
    entries_seen: u64,
    snapshot: ProgressSnapshot,
) {
    let _ = sender.send(ScanProgressUpdate::Progress {
        scan_id: scan_id.to_string(),
        snapshot: ScanProgressSnapshot {
            phase,
            entries_seen,
            files_seen: snapshot.files_seen,
            dirs_seen: snapshot.dirs_seen,
            bytes_seen: snapshot.bytes_seen,
            errors_seen: snapshot.errors_seen,
            elapsed_ms: snapshot.elapsed_ms,
        },
    });
}

/// Count the retained entries of one observed kind.
fn count_kind(entries: &[FsEntry], kind: ObservedKind) -> u64 {
    let mut count = 0u64;
    for entry in entries {
        let observed = match &entry.kind {
            coresight_engine::EntryKind::File => ObservedKind::File,
            coresight_engine::EntryKind::Dir => ObservedKind::Dir,
            coresight_engine::EntryKind::Link(_) => ObservedKind::Link,
            coresight_engine::EntryKind::Other => ObservedKind::Other,
        };
        if observed == kind {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Map the engine's size facts onto the history model.
///
/// * an entry with an observation error has **no** size — the engine's
///   placeholder `0` was not a measurement;
/// * a successfully observed non-file (directory, link, special node) has the
///   engine's own logical size of `0` file bytes;
/// * a successfully observed file keeps its measured size.
///
/// This keeps `entries_without_size` meaning "size genuinely unknown" instead
/// of "this entry is not a file", so a clean walk is a complete byte total
/// while a walk with unreadable entries stays honestly partial. Entry order
/// is untouched — the builder already sorted.
fn normalize_sizes(mut snapshot: Snapshot) -> Snapshot {
    for entry in &mut snapshot.entries {
        if entry.observation_error.is_some() {
            entry.size = None;
        } else if entry.kind != ObservedKind::File {
            entry.size = Some(0);
        }
    }
    snapshot
}

/// Build the path-free failure summary for a terminal scan.
fn failure_for(summary: &ScanSummary, status: ScanStatus) -> Option<ScanFailure> {
    let error_count = summary.errors.total_errors();
    if error_count == 0 && status != ScanStatus::Failed {
        return None;
    }
    let mut reason = if summary.depth_capped {
        "DEPTH_CAP_REACHED".to_string()
    } else if summary.errors.counts.is_empty() {
        "SCAN_FAILED".to_string()
    } else {
        summary
            .errors
            .counts
            .iter()
            .map(|(category, count)| format!("{}:{}", category.code(), count))
            .collect::<Vec<String>>()
            .join(",")
    };
    if reason.is_empty() {
        reason = "SCAN_FAILED".to_string();
    }
    Some(ScanFailure {
        reason,
        error_count,
    })
}

/// The `coresight.v1.scanProgress` event payload (`docs/API_CONTRACTS.md`).
///
/// Exactly the documented fields: `scanId`, `filesSeen`, `bytesSeen`, and the
/// optional `currentCategory`/`etaSec`. `currentCategory` is `None` because
/// the engine does not report a per-entry category during a walk; `etaSec` is
/// `None` because this build records no arrival-rate estimate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressEvent {
    /// The scan this update belongs to.
    pub scan_id: String,
    /// Entries observed so far (real progress).
    pub files_seen: u64,
    /// Bytes observed so far (real progress, never a finished total).
    pub bytes_seen: u64,
    /// Category currently being classified, when one is known.
    pub current_category: Option<String>,
    /// Estimated seconds remaining, when an estimate exists.
    pub eta_sec: Option<u64>,
}

/// The `coresight.v1.scanComplete` event payload.
///
/// `status` is the terminal lifecycle state (`completed`, `cancelled`,
/// `failed`); `summary` carries the real observed counts of that run. A
/// cancelled or failed run's `summary` describes only what it observed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCompleteEvent {
    /// The scan this event terminates.
    pub scan_id: String,
    /// Terminal lifecycle state.
    pub status: ScanStatus,
    /// Real counts of the finished run.
    pub summary: ScanCompleteSummary,
}

/// The numeric summary attached to `coresight.v1.scanComplete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCompleteSummary {
    /// Entries observed by the run.
    pub files_seen: u64,
    /// Directories observed by the run.
    pub dirs_seen: u64,
    /// Logical bytes observed by the run.
    pub bytes_seen: u64,
    /// Entries discovered but not fully readable.
    pub entries_with_errors: u64,
    /// Entries whose size could not be established.
    pub entries_without_size: u64,
    /// Whether the optional depth cap truncated traversal.
    pub depth_capped: bool,
    /// Wall-clock duration of the run in milliseconds.
    pub elapsed_ms: u64,
}

impl ScanProgressUpdate {
    /// The `scanProgress` payload for this update, when it has one.
    ///
    /// `Started` and `Finished` produce `None`; `Finished` is reported by
    /// [`ScanProgressUpdate::complete_event`] instead.
    pub fn progress_event(&self) -> Option<ScanProgressEvent> {
        match self {
            ScanProgressUpdate::Progress { scan_id, snapshot } => Some(ScanProgressEvent {
                scan_id: scan_id.clone(),
                files_seen: snapshot.entries_seen,
                bytes_seen: snapshot.bytes_seen,
                current_category: None,
                eta_sec: None,
            }),
            _ => None,
        }
    }

    /// The `scanComplete` payload for this update, when it is terminal.
    pub fn complete_event(&self) -> Option<ScanCompleteEvent> {
        let ScanProgressUpdate::Finished(finished) = self else {
            return None;
        };
        let elapsed_ms = finished
            .summary
            .finished_at
            .duration_since(finished.summary.started_at)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Some(ScanCompleteEvent {
            scan_id: finished.scan_id.clone(),
            status: finished.status,
            summary: ScanCompleteSummary {
                files_seen: finished.report.scanned_entries,
                dirs_seen: finished.summary.dirs,
                bytes_seen: finished.report.total_logical_size(),
                entries_with_errors: finished.report.errored_entries,
                entries_without_size: finished.report.entries_without_size,
                depth_capped: finished.summary.depth_capped,
                elapsed_ms,
            },
        })
    }
}

/// Default engine options for a shell scan.
///
/// Small worker count, record-only link policy, no depth cap, and entries
/// emitted (a snapshot needs them). `fullRescan` needs no option: this build
/// keeps no incremental index, so every scan is already a full walk of the
/// supplied root — claiming an incremental shortcut would be a lie.
pub fn default_scan_options() -> ScanOptions {
    ScanOptions {
        threads: default_threads(),
        symlink_policy: coresight_engine::SymlinkPolicy::RecordOnly,
        max_depth: None,
        emit_entries: true,
        progress_interval: Duration::from_millis(250),
    }
}

/// Whether a path is a usable, absolute scan root.
///
/// This is the one root check the session performs before handing the path to
/// the backend; the backend repeats it because it is a public entry point.
pub fn is_usable_root(root: &Path) -> bool {
    !root.as_os_str().is_empty() && root.is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_codes_are_typed() {
        assert_eq!(
            ScanBackendError::InvalidRequest("x".into()).code(),
            codes::INVALID_REQUEST
        );
        assert_eq!(
            ScanBackendError::RootUnavailable {
                category: "NOT_FOUND".into()
            }
            .code(),
            codes::SCAN_ROOT_UNAVAILABLE
        );
        assert_eq!(
            ScanBackendError::WorkerUnavailable("x".into()).code(),
            codes::SCAN_UNAVAILABLE
        );
    }

    #[test]
    fn default_options_emit_entries_and_never_follow_links() {
        let options = default_scan_options();
        assert!(options.emit_entries);
        assert_eq!(
            options.symlink_policy,
            coresight_engine::SymlinkPolicy::RecordOnly
        );
        assert_eq!(options.max_depth, None);
    }

    #[test]
    fn relative_and_blank_roots_are_not_usable() {
        assert!(!is_usable_root(Path::new("")));
        assert!(!is_usable_root(Path::new("relative/path")));
        assert!(is_usable_root(&std::env::temp_dir()));
    }

    #[test]
    fn normalize_sizes_keeps_measured_sizes_and_marks_errors_unknown() {
        let file = FsEntry {
            id: 1,
            parent_id: None,
            path: PathBuf::from("/f"),
            kind: coresight_engine::EntryKind::File,
            size: 7,
            allocated_size: None,
            modified: None,
            created: None,
            accessed: None,
            changed: None,
            device: None,
            inode: None,
            file_id_hi: None,
            hidden: false,
            error: None,
        };
        let dir = FsEntry {
            id: 2,
            path: PathBuf::from("/d"),
            kind: coresight_engine::EntryKind::Dir,
            size: 0,
            ..file.clone()
        };
        let mut unreadable = file.clone();
        unreadable.id = 3;
        unreadable.path = PathBuf::from("/e");
        unreadable.error = Some(coresight_engine::ErrorCategoryRef::PermissionDenied);
        let mut builder = SnapshotBuilder::new();
        builder.push_entry(&file, None);
        builder.push_entry(&dir, None);
        builder.push_entry(&unreadable, None);
        let built = builder.build(RunId("r".to_string()));
        assert!(built.is_ok(), "the fixture paths are unique");
        let Ok(snapshot) = built else { return };
        let snapshot = normalize_sizes(snapshot);
        // Canonical order is by path bytes: /d, /e, /f.
        assert_eq!(snapshot.entries[0].size, Some(0));
        assert_eq!(snapshot.entries[1].size, None);
        assert_eq!(snapshot.entries[2].size, Some(7));
    }

    #[test]
    fn engine_errors_without_an_entry_are_folded_into_the_report() {
        use coresight_engine::{ErrorCategory, ScanError, ScanErrorReport};

        let report = UsageReport {
            errored_entries: 1, // one entry already carried an error
            ..UsageReport::default()
        };
        let mut errors = ScanErrorReport::new();
        for _ in 0..3 {
            errors.record(ScanError {
                category: ErrorCategory::PermissionDenied,
                path: PathBuf::from("p"),
                message: "m".to_string(),
                raw_os: None,
            });
        }
        let summary = ScanSummary {
            status: coresight_engine::ScanStatus::Completed,
            root: PathBuf::from("r"),
            files: 0,
            dirs: 1,
            links: 0,
            other_entries: 0,
            bytes: 0,
            allocated_bytes: None,
            // Two of the three errors are already represented by entry
            // errors; one is an unreadable area with no emitted entry.
            entries_with_errors: 2,
            errors,
            started_at: std::time::UNIX_EPOCH,
            finished_at: std::time::UNIX_EPOCH,
            depth_capped: false,
        };
        let adjusted = with_engine_errors(report, &summary);
        assert_eq!(
            adjusted.errored_entries, 2,
            "the entry error is not double counted and the area error is added"
        );
    }
}
