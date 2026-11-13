//! The platform-neutral session: drives, per-scan facts, retained plans.
//!
//! A `Session` is what a long-lived desktop shell owns. It holds **facts the
//! shell has already obtained** — registered drive identities and completed
//! scan snapshots — and the pure derivations over them. It holds no
//! filesystem handle, no database handle, and no executor. A scan is only
//! performed by an explicitly attached [`ScanBackend`](crate::ScanBackend)
//! (the real [`EngineScanBackend`](crate::EngineScanBackend) in this build);
//! the session receives its finished [`Snapshot`] and records it.
//!
//! ## Registration is explicit
//!
//! [`Session::register_scan`] accepts a [`ScanRecord`] containing a
//! `coresight_history::Snapshot`. The scan's [`UsageReport`] and cleanup
//! plan are derived once at registration; the snapshot is retained because
//! plan previews must resolve concrete paths against it.
//!
//! A scan started through [`Session::start_scan`] is registered as `Running`
//! immediately. The shell drains the backend's progress receiver and calls
//! [`Session::apply_scan_update`] with each update; the terminal update
//! replaces the running record with the real snapshot, report, plan, and
//! terminal status. A cancelled or failed scan is retained so its last state
//! is visible, but its observations are never served as a complete report.
//!
//! ## What a session can and cannot do
//!
//! * Serves: drive listing, drive summaries, category totals, opportunity
//!   cards, plan previews, and the ten aggregate view commands — all from
//!   registered facts.
//! * Scans only through an attached backend. With no backend attached,
//!   `startScan`/`cancelScan` return the typed `SCAN_UNAVAILABLE` refusal.
//! * Cannot serve: history (no store is attached), aggregate views for data
//!   the shell has not registered, and execution (the current
//!   `ExecutionPolicy` authorizes read-only actions only).
//!
//! Those refusals are the honesty contract, not missing wiring: see the
//! crate documentation and `docs/API_CONTRACTS.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use coresight_cleanup::{plan_cleanup, CleanupItem, CleanupPlan, PlannerOptions};
use coresight_engine::CancelHandle;
use coresight_history::{RunId, Snapshot};
use coresight_insights::{usage_by_category, UsageReport};
use coresight_systools::startup::StartupItem;
use coresight_webtools::{BlockListPlan, DnsRanking, PrivacyFinding};

use crate::command::StartScanRequest;
use crate::error::{codes, detail_token, ApiError};
use crate::response::{
    CancelScanResponse, Drive, DriveKind, PlanPreview, ScanStatus, StartScanResponse,
};
use crate::scan::{
    default_scan_options, is_usable_root, ScanBackend, ScanFailure, ScanFinished,
    ScanProgressSnapshot, ScanProgressUpdate, ScanRequest,
};

/// One completed (or terminal) scan registered into the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRecord {
    /// Stable scan id chosen by the producing backend.
    pub scan_id: String,
    /// The drive this scan observed.
    pub drive_id: String,
    /// The normalized per-path facts, exactly as the scanner committed them.
    pub snapshot: Snapshot,
    /// Completion time, when the producing run recorded one.
    pub completed_at: Option<SystemTime>,
    /// Terminal or in-flight lifecycle state.
    pub status: crate::response::ScanStatus,
}

/// Why a scan could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The scan id was empty or whitespace-only.
    EmptyScanId,
    /// The drive id was empty or whitespace-only.
    EmptyDriveId,
    /// The scan id is already registered; re-registering would silently
    /// replace facts the UI may already be showing.
    DuplicateScan(String),
    /// No drive with this id was registered first.
    UnknownDrive(String),
    /// A progress update named a scan this session does not hold.
    UnknownScan(String),
    /// A terminal update named a scan that is no longer running.
    ScanNotRunning(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::EmptyScanId => f.write_str("a scan id must not be empty"),
            SessionError::EmptyDriveId => f.write_str("a drive id must not be empty"),
            SessionError::DuplicateScan(id) => write!(f, "scan {id} is already registered"),
            SessionError::UnknownDrive(id) => {
                write!(f, "drive {id} was not registered before its scan")
            }
            SessionError::UnknownScan(id) => write!(f, "scan {id} is not held by this session"),
            SessionError::ScanNotRunning(id) => write!(f, "scan {id} is no longer running"),
        }
    }
}

impl std::error::Error for SessionError {}

/// The retained, derived state of one registered scan.
pub(crate) struct ScanState {
    /// The caller-supplied facts.
    pub(crate) record: ScanRecord,
    /// Per-category aggregation computed once at registration (and once more
    /// at the terminal update of a real scan).
    pub(crate) report: UsageReport,
    /// The category-level cleanup plan, in the planner's canonical order.
    pub(crate) plan: CleanupPlan,
    /// Live counters of a running scan; `None` once a terminal update landed.
    pub(crate) progress: Option<ScanProgressSnapshot>,
    /// The engine's recorded start instant, when the run reported one.
    pub(crate) started_at: Option<SystemTime>,
    /// The real root this scan walked, when it was started through the
    /// backend. Used only as the display label of the current area.
    pub(crate) root: Option<PathBuf>,
    /// The engine's terminal summary, once the run finished.
    pub(crate) summary: Option<coresight_engine::ScanSummary>,
    /// Why the run failed or was incomplete, when the engine reported it.
    pub(crate) failure: Option<ScanFailure>,
}

/// A previewed plan retained for synchronous confirmation.
pub(crate) struct StoredPlan {
    /// What the user reviewed.
    pub(crate) preview: PlanPreview,
    /// The category-level subset the preview was resolved from.
    pub(crate) plan: CleanupPlan,
}

/// The platform-neutral dispatcher state.
///
/// See the module documentation for the registration and honesty rules.
/// Session state is in-memory only: there is no persistence, no clock read,
/// and no background task.
pub struct Session {
    pub(crate) drives: BTreeMap<String, Drive>,
    pub(crate) scans: BTreeMap<String, ScanState>,
    pub(crate) plans: BTreeMap<String, StoredPlan>,
    pub(crate) confirmed: BTreeSet<String>,
    pub(crate) backend: Option<Box<dyn ScanBackend>>,
    /// Cancellation tokens of the scans that are still running.
    pub(crate) scan_cancels: BTreeMap<String, CancelHandle>,
    /// Scan handles not yet handed to the shell, which owns the receivers.
    pub(crate) scan_handles: BTreeMap<String, crate::scan::ScanHandle>,
    /// Startup inventory supplied by the shell, analysed on read.
    pub(crate) startup_items: Option<Vec<StartupItem>>,
    /// DNS benchmark ranking supplied by the shell.
    pub(crate) dns_ranking: Option<DnsRanking>,
    /// Reversible block-list plan supplied by the shell.
    pub(crate) block_list_plan: Option<BlockListPlan>,
    /// Privacy findings supplied by the shell, summarised on read.
    pub(crate) privacy_findings: Option<Vec<PrivacyFinding>>,
}

impl Session {
    /// An empty session: no drives, no scans, no plans, no backend.
    pub fn new() -> Self {
        Session {
            drives: BTreeMap::new(),
            scans: BTreeMap::new(),
            plans: BTreeMap::new(),
            confirmed: BTreeSet::new(),
            backend: None,
            scan_cancels: BTreeMap::new(),
            scan_handles: BTreeMap::new(),
            startup_items: None,
            dns_ranking: None,
            block_list_plan: None,
            privacy_findings: None,
        }
    }

    /// Register the startup inventory the shell collected.
    ///
    /// The inventory is data, not an observation performed here: this crate
    /// never enumerates startup items. `getStartupReport` analyses exactly the
    /// entries registered by this call.
    pub fn register_startup_inventory(&mut self, items: Vec<StartupItem>) {
        self.startup_items = Some(items);
    }

    /// Register a DNS benchmark ranking produced elsewhere.
    ///
    /// This crate never contacts a resolver; it only projects the supplied
    /// ranking into the `getDnsRanking` payload.
    pub fn register_dns_ranking(&mut self, ranking: DnsRanking) {
        self.dns_ranking = Some(ranking);
    }

    /// Register a reversible block-list plan produced elsewhere.
    ///
    /// This crate never reads or writes a hosts file.
    pub fn register_block_list_plan(&mut self, plan: BlockListPlan) {
        self.block_list_plan = Some(plan);
    }

    /// Register privacy findings collected elsewhere.
    ///
    /// The findings are summarised on read; nothing is removed.
    pub fn register_privacy_findings(&mut self, findings: Vec<PrivacyFinding>) {
        self.privacy_findings = Some(findings);
    }

    /// Register or refresh one drive identity.
    ///
    /// Registering the same id again replaces the previous record (a drive
    /// may be re-mounted with a new label or free-space reading). Existing
    /// scans remain attached to the id.
    pub fn register_drive(&mut self, drive: Drive) {
        self.drives.insert(drive.id.clone(), drive);
    }

    /// Attach a real scan backend. The shell attaches
    /// [`EngineScanBackend`](crate::EngineScanBackend); a session with no
    /// backend keeps refusing `startScan`/`cancelScan` with
    /// `SCAN_UNAVAILABLE`.
    pub fn attach_scan_backend(&mut self, backend: Box<dyn ScanBackend>) {
        self.backend = Some(backend);
    }

    /// Whether a scan backend is attached.
    pub(crate) fn has_scan_backend(&self) -> bool {
        self.backend.is_some()
    }

    /// Register a scan and derive its usage report and plan.
    ///
    /// A `Running` scan may be registered (so the shell can show that it
    /// exists), but commands that need results refuse it with
    /// `SCAN_IN_PROGRESS`. Cancelled and failed scans are retained so their
    /// last-scan state is visible, but their results are never served as a
    /// complete report.
    ///
    /// # Errors
    ///
    /// * [`SessionError::EmptyScanId`] / [`SessionError::EmptyDriveId`] for
    ///   blank ids.
    /// * [`SessionError::UnknownDrive`] when the drive was not registered.
    /// * [`SessionError::DuplicateScan`] when the id is already present.
    pub fn register_scan(&mut self, record: ScanRecord) -> Result<(), SessionError> {
        if record.scan_id.trim().is_empty() {
            return Err(SessionError::EmptyScanId);
        }
        if record.drive_id.trim().is_empty() {
            return Err(SessionError::EmptyDriveId);
        }
        if !self.drives.contains_key(&record.drive_id) {
            return Err(SessionError::UnknownDrive(record.drive_id));
        }
        if self.scans.contains_key(&record.scan_id) {
            return Err(SessionError::DuplicateScan(record.scan_id));
        }
        let report = usage_by_category(&record.snapshot);
        let plan = plan_for(&report);
        self.scans.insert(
            record.scan_id.clone(),
            ScanState {
                record,
                report,
                plan,
                progress: None,
                started_at: None,
                root: None,
                summary: None,
                failure: None,
            },
        );
        Ok(())
    }

    /// Start a real scan and register it as `Running`.
    ///
    /// The root comes from `request.root`, or from the engine's `SysDirs`
    /// boundary when the caller supplied none. A drive record is derived from
    /// the real root when the caller did not name an already-registered
    /// drive, so the scan can be summarized without inventing capacity or
    /// free-space facts.
    ///
    /// # Errors
    ///
    /// * `SCAN_UNAVAILABLE` when no backend is attached.
    /// * `SCAN_ROOT_UNAVAILABLE` when no root was supplied and the platform
    ///   reports no home directory.
    /// * `INVALID_REQUEST` for a relative/blank root.
    /// * The backend's typed error when it cannot observe the root.
    pub fn start_scan(
        &mut self,
        request: &StartScanRequest,
    ) -> Result<StartScanResponse, ApiError> {
        if self.backend.is_none() {
            return Err(no_scan_backend("starting a scan"));
        }
        let root = match request.root.as_ref() {
            Some(root) if is_usable_root(root) => root.clone(),
            Some(_) => {
                return Err(ApiError::new(
                    codes::INVALID_REQUEST,
                    "`root` must be an absolute path. Nothing was started or changed.",
                ))
            }
            None => match coresight_engine::platform::sys_dirs().home() {
                Some(home) if is_usable_root(&home) => home,
                _ => {
                    return Err(ApiError::new(
                        codes::SCAN_ROOT_UNAVAILABLE,
                        "No scan root was supplied and the platform did not report a home \
                         directory, so no scan was started. Nothing was changed.",
                    ))
                }
            },
        };

        let scan_id = self.next_scan_id();
        let handle = match self.backend.as_ref() {
            Some(backend) => backend
                .start(ScanRequest {
                    scan_id: scan_id.clone(),
                    root: root.clone(),
                    options: default_scan_options(),
                })
                .map_err(|error| error.to_api_error())?,
            None => return Err(no_scan_backend("starting a scan")),
        };

        // Only register the drive after the root was proven observable, so a
        // refused scan never leaves a drive row behind.
        let drive_id = self.ensure_scan_drive(&root, &request.drive_id);
        if let Err(error) = self.register_running_scan(&scan_id, &drive_id, &root) {
            // The worker already started; stop it rather than leaving an
            // unregistered scan running.
            handle.cancel.cancel();
            return Err(ApiError::new(
                codes::INVALID_REQUEST,
                "The scan could not be registered in this session, so it was cancelled. \
                 Nothing was changed.",
            )
            .with_detail(error.to_string()));
        }
        self.scan_cancels
            .insert(scan_id.clone(), handle.cancel.clone());
        self.scan_handles.insert(scan_id.clone(), handle);
        Ok(StartScanResponse { scan_id })
    }

    /// Cancel a running scan through its engine [`CancelHandle`].
    ///
    /// # Errors
    ///
    /// * `SCAN_NOT_FOUND` for an unknown scan id.
    /// * `SCAN_NOT_RUNNING` when the scan already reached a terminal state —
    ///   a repeated or late cancel is a typed refusal, never a silent
    ///   success.
    pub fn cancel_scan(&self, scan_id: &str) -> Result<CancelScanResponse, ApiError> {
        let Some(scan) = self.scans.get(scan_id) else {
            return Err(ApiError::new(
                codes::SCAN_NOT_FOUND,
                "The requested scan is not known in this session, so there is nothing to \
                 cancel.",
            )
            .with_detail(format!("unknown scan id: {}", detail_token(scan_id))));
        };
        if scan.record.status != ScanStatus::Running {
            return Err(ApiError::new(
                codes::SCAN_NOT_RUNNING,
                "The requested scan is no longer running, so it cannot be cancelled again. \
                 Nothing was changed.",
            )
            .with_detail(format!(
                "scan id {} has status {:?}",
                detail_token(scan_id),
                scan.record.status
            )));
        }
        let Some(cancel) = self.scan_cancels.get(scan_id) else {
            return Err(ApiError::new(
                codes::SCAN_UNAVAILABLE,
                "The requested scan is running but this session holds no cancellation token \
                 for it. Nothing was changed.",
            )
            .with_detail(format!(
                "scan id {} has no cancellation token",
                detail_token(scan_id)
            )));
        };
        cancel.cancel();
        Ok(CancelScanResponse { cancelled: true })
    }

    /// Take the live handle of a scan so the shell can drain its receiver.
    ///
    /// Returns `None` when the scan is unknown or its handle was already
    /// taken. The session keeps its own cancellation-token clone; the handle
    /// (and therefore the receiver) is the shell's to poll.
    pub fn take_scan_handle(&mut self, scan_id: &str) -> Option<crate::scan::ScanHandle> {
        self.scan_handles.remove(scan_id)
    }

    /// Apply one backend update to this session.
    ///
    /// Progress updates only touch the live counters of a running scan. The
    /// terminal update installs the real snapshot, recomputes the usage
    /// report and the plan, records the terminal status, and releases the
    /// cancellation token.
    ///
    /// # Errors
    ///
    /// [`SessionError::UnknownScan`] for an update naming an unknown scan,
    /// [`SessionError::ScanNotRunning`] when a second terminal update arrives
    /// for a scan that already finished.
    pub fn apply_scan_update(&mut self, update: ScanProgressUpdate) -> Result<(), SessionError> {
        match update {
            ScanProgressUpdate::Started {
                scan_id,
                root,
                started_at,
            } => {
                let Some(scan) = self.scans.get_mut(&scan_id) else {
                    return Err(SessionError::UnknownScan(scan_id));
                };
                scan.started_at = Some(started_at);
                scan.root = Some(root);
                Ok(())
            }
            ScanProgressUpdate::Progress { scan_id, snapshot } => {
                let Some(scan) = self.scans.get_mut(&scan_id) else {
                    return Err(SessionError::UnknownScan(scan_id));
                };
                if scan.record.status != ScanStatus::Running {
                    return Err(SessionError::ScanNotRunning(scan_id));
                }
                scan.progress = Some(snapshot);
                Ok(())
            }
            ScanProgressUpdate::Finished(finished) => self.finish_scan(*finished),
        }
    }

    /// Register the `Running` record a freshly started scan needs.
    fn register_running_scan(
        &mut self,
        scan_id: &str,
        drive_id: &str,
        root: &Path,
    ) -> Result<(), SessionError> {
        if scan_id.trim().is_empty() {
            return Err(SessionError::EmptyScanId);
        }
        if drive_id.trim().is_empty() {
            return Err(SessionError::EmptyDriveId);
        }
        if !self.drives.contains_key(drive_id) {
            return Err(SessionError::UnknownDrive(drive_id.to_string()));
        }
        if self.scans.contains_key(scan_id) {
            return Err(SessionError::DuplicateScan(scan_id.to_string()));
        }
        let snapshot = Snapshot {
            run_id: RunId(scan_id.to_string()),
            entries: Vec::new(),
        };
        let report = usage_by_category(&snapshot);
        self.scans.insert(
            scan_id.to_string(),
            ScanState {
                record: ScanRecord {
                    scan_id: scan_id.to_string(),
                    drive_id: drive_id.to_string(),
                    snapshot,
                    completed_at: None,
                    status: ScanStatus::Running,
                },
                plan: plan_for(&report),
                report,
                progress: None,
                // The backend's first `Started` update supplies the engine's
                // recorded start instant; until it is drained, the honest
                // value is "not yet recorded".
                started_at: None,
                root: Some(root.to_path_buf()),
                summary: None,
                failure: None,
            },
        );
        Ok(())
    }

    /// Install a terminal scan result.
    fn finish_scan(&mut self, finished: ScanFinished) -> Result<(), SessionError> {
        let ScanFinished {
            scan_id,
            status,
            snapshot,
            report,
            summary,
            failure,
        } = finished;
        if status == ScanStatus::Running {
            return Err(SessionError::ScanNotRunning(scan_id));
        }
        let Some(scan) = self.scans.get_mut(&scan_id) else {
            return Err(SessionError::UnknownScan(scan_id));
        };
        if scan.record.status != ScanStatus::Running {
            return Err(SessionError::ScanNotRunning(scan_id));
        }
        let completed_at = summary.finished_at;
        let started_at = summary.started_at;
        let plan = plan_for(&report);
        scan.record.snapshot = snapshot;
        scan.record.status = status;
        scan.record.completed_at = Some(completed_at);
        scan.report = report;
        scan.plan = plan;
        scan.progress = None;
        scan.started_at = Some(started_at);
        scan.summary = Some(summary);
        scan.failure = failure;
        self.scan_cancels.remove(&scan_id);
        Ok(())
    }

    /// Ensure a drive record exists for a real scan root.
    ///
    /// Capacity and free space stay `None`: this session did not query them,
    /// and a fabricated number would be worse than an honest unknown.
    fn ensure_scan_drive(&mut self, root: &Path, requested: &str) -> String {
        let id = if requested.trim().is_empty() {
            format!("root:{}", root.display())
        } else {
            requested.to_string()
        };
        if !self.drives.contains_key(&id) {
            self.drives.insert(
                id.clone(),
                Drive {
                    id: id.clone(),
                    root: root.to_string_lossy().into_owned(),
                    label: None,
                    kind: DriveKind::Unknown,
                    capacity_bytes: None,
                    available_bytes: None,
                    connected: true,
                },
            );
        }
        id
    }

    /// The next unused session-local scan id (`scan-1`, `scan-2`, …).
    fn next_scan_id(&self) -> String {
        let mut sequence = self.scans.len().saturating_add(1);
        loop {
            let candidate = format!("scan-{sequence}");
            if !self.scans.contains_key(&candidate) && !self.scan_cancels.contains_key(&candidate) {
                return candidate;
            }
            sequence = sequence.saturating_add(1);
        }
    }

    pub(crate) fn drive(&self, drive_id: &str) -> Option<&Drive> {
        self.drives.get(drive_id)
    }

    pub(crate) fn scan(&self, scan_id: &str) -> Option<&ScanState> {
        self.scans.get(scan_id)
    }

    pub(crate) fn plan(&self, plan_id: &str) -> Option<&StoredPlan> {
        self.plans.get(plan_id)
    }

    pub(crate) fn store_plan(&mut self, plan: StoredPlan) {
        self.plans.insert(plan.preview.plan_id.clone(), plan);
    }

    pub(crate) fn mark_confirmed(&mut self, plan_id: &str) {
        self.confirmed.insert(plan_id.to_string());
    }

    pub(crate) fn is_confirmed(&self, plan_id: &str) -> bool {
        self.confirmed.contains(plan_id)
    }

    /// The startup inventory registered into this session, if any.
    pub(crate) fn startup_items(&self) -> Option<&[StartupItem]> {
        self.startup_items.as_deref()
    }

    /// The DNS ranking registered into this session, if any.
    pub(crate) fn dns_ranking(&self) -> Option<&DnsRanking> {
        self.dns_ranking.as_ref()
    }

    /// The block-list plan registered into this session, if any.
    pub(crate) fn block_list_plan(&self) -> Option<&BlockListPlan> {
        self.block_list_plan.as_ref()
    }

    /// The privacy findings registered into this session, if any.
    pub(crate) fn privacy_findings(&self) -> Option<&[PrivacyFinding]> {
        self.privacy_findings.as_deref()
    }

    /// The scan that best describes the current state, across every drive.
    ///
    /// A running scan outranks any finished one (a scan in progress is the
    /// current activity); among finished scans the newest `(completed_at,
    /// scan_id)` wins. This is deterministic and independent of registration
    /// order.
    pub(crate) fn latest_scan(&self) -> Option<&ScanState> {
        let running = self
            .scans
            .values()
            .filter(|scan| scan.record.status == ScanStatus::Running)
            .max_by(|left, right| left.record.scan_id.cmp(&right.record.scan_id));
        running.or_else(|| {
            self.scans.values().max_by(|left, right| {
                left.record
                    .completed_at
                    .cmp(&right.record.completed_at)
                    .then_with(|| left.record.scan_id.cmp(&right.record.scan_id))
            })
        })
    }

    /// The most recent scan that observed its full declared scope.
    pub(crate) fn latest_full_scope_scan(&self) -> Option<&ScanState> {
        self.scans
            .values()
            .filter(|scan| scan.record.status.observes_full_scope())
            .max_by(|left, right| {
                left.record
                    .completed_at
                    .cmp(&right.record.completed_at)
                    .then_with(|| left.record.scan_id.cmp(&right.record.scan_id))
            })
    }

    /// All drives, connected first, then by id. Deterministic and independent
    /// of registration order.
    pub(crate) fn drives_in_list_order(&self) -> Vec<&Drive> {
        let mut drives: Vec<&Drive> = self.drives.values().collect();
        drives.sort_by(|left, right| {
            right
                .connected
                .cmp(&left.connected)
                .then_with(|| left.id.cmp(&right.id))
        });
        drives
    }

    /// Scans for one drive, oldest → newest by `(completed_at, scan_id)`.
    ///
    /// `None` completion times sort first (a not-yet-finished scan is not
    /// "later" than a finished one). The tie-break on id keeps the order
    /// deterministic when two runs share a timestamp.
    pub(crate) fn scans_for_drive(&self, drive_id: &str) -> Vec<&ScanState> {
        let mut scans: Vec<&ScanState> = self
            .scans
            .values()
            .filter(|scan| scan.record.drive_id == drive_id)
            .collect();
        scans.sort_by(|left, right| {
            left.record
                .completed_at
                .cmp(&right.record.completed_at)
                .then_with(|| left.record.scan_id.cmp(&right.record.scan_id))
        });
        scans
    }

    /// The full-scope scan immediately before `current`, if any.
    pub(crate) fn previous_full_scope_scan<'a>(
        &'a self,
        current: &ScanState,
    ) -> Option<&'a ScanState> {
        let scans = self.scans_for_drive(&current.record.drive_id);
        let index = scans
            .iter()
            .position(|scan| scan.record.scan_id == current.record.scan_id)?;
        scans[..index]
            .iter()
            .rev()
            .find(|scan| scan.record.status.observes_full_scope())
            .copied()
    }

    /// Resolve a scan-scoped opportunity id to its scan and plan item.
    ///
    /// The id is `"<scanId>:<plannerId>"` (for example
    /// `"scan-1:CACHE:30000"`), so two retained scans can never make the
    /// same category-level card ambiguous. If two matches still occurred
    /// (impossible with unique scan ids), the request is refused rather
    /// than guessed.
    pub(crate) fn find_opportunity(
        &self,
        opportunity_id: &str,
    ) -> Result<(&ScanState, &CleanupItem), ApiError> {
        let mut matches: Vec<(&ScanState, &CleanupItem)> = Vec::new();
        for scan in self.scans.values() {
            for item in &scan.plan.items {
                if opportunity_id_for(&scan.record.scan_id, item) == opportunity_id {
                    matches.push((scan, item));
                }
            }
        }
        match matches.as_slice() {
            [] => Err(ApiError::new(
                codes::UNKNOWN_OPPORTUNITY,
                "The requested opportunity is not part of any scan in this session. \
                 Refresh the opportunity list and try again.",
            )
            .with_detail(format!(
                "unknown opportunity id: {}",
                detail_token(opportunity_id)
            ))),
            [(scan, item)] => Ok((scan, item)),
            _ => Err(ApiError::new(
                codes::INVALID_REQUEST,
                "The requested opportunity id exists in more than one retained scan, \
                 so it is ambiguous. Refresh the opportunity list and try again.",
            )
            .with_detail(format!(
                "ambiguous opportunity id in {} scans: {}",
                matches.len(),
                detail_token(opportunity_id)
            ))),
        }
    }
}

/// The scan-scoped, deterministic IPC id of one planner item.
pub(crate) fn opportunity_id_for(scan_id: &str, item: &CleanupItem) -> String {
    format!("{scan_id}:{}", item.id)
}

/// Derive the category-level cleanup plan for a report.
fn plan_for(report: &UsageReport) -> CleanupPlan {
    plan_cleanup(
        report,
        &PlannerOptions {
            include_review: true,
            min_recoverable_bytes: 1,
        },
    )
}

/// The typed refusal used when no scan backend is attached.
pub(crate) fn no_scan_backend(action: &str) -> ApiError {
    ApiError::new(
        codes::SCAN_UNAVAILABLE,
        "No scan backend is attached to this CoreSight session, so scanning is \
         unavailable. Nothing was started or cancelled.",
    )
    .with_detail(format!(
        "refused {action}: this session has no scanner attached"
    ))
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}
