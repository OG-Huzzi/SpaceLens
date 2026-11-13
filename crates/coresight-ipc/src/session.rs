//! The platform-neutral session: drives, per-scan facts, retained plans.
//!
//! A `Session` is what a long-lived desktop shell owns. It holds **facts the
//! shell has already obtained** — registered drive identities and completed
//! scan snapshots — and the pure derivations over them. It holds no
//! filesystem handle, no database handle, no scanner, and no executor.
//!
//! ## Registration is explicit
//!
//! [`Session::register_scan`] accepts a [`ScanRecord`] containing a
//! `coresight_history::Snapshot`. The scan's [`UsageReport`] and cleanup
//! plan are derived once at registration; the snapshot is retained because
//! plan previews must resolve concrete paths against it. A scan may only be
//! registered for a drive that was registered first — an implicit drive
//! would invent a mount fact the caller never supplied.
//!
//! ## What a session can and cannot do
//!
//! * Serves: drive listing, drive summaries, category totals, opportunity
//!   cards, and plan previews — all from registered facts.
//! * Cannot serve: scanning (no [`ScanBackend`] is attached in this build),
//!   history (no store is attached), and execution (the current
//!   `ExecutionPolicy` authorizes read-only actions only).
//!
//! Those refusals are the honesty contract, not missing wiring: see the
//! crate documentation and `docs/API_CONTRACTS.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::SystemTime;

use coresight_cleanup::{plan_cleanup, CleanupItem, CleanupPlan, PlannerOptions};
use coresight_history::Snapshot;
use coresight_insights::{usage_by_category, UsageReport};

use crate::command::{CancelScanRequest, StartScanRequest};
use crate::error::{codes, detail_token, ApiError};
use crate::response::{CancelScanResponse, Drive, PlanPreview, StartScanResponse};

/// A scanner the shell may attach in a later phase.
///
/// This crate ships **no implementation** and never invents one. The trait
/// exists so a future Tauri orchestration layer can hand its real scanner to
/// [`Session::attach_scan_backend`] without changing the command surface.
pub trait ScanBackend: Send {
    /// Start a scan and return its id.
    fn start_scan(&mut self, request: &StartScanRequest) -> Result<StartScanResponse, ApiError>;

    /// Cancel a running scan.
    fn cancel_scan(&mut self, request: &CancelScanRequest) -> Result<CancelScanResponse, ApiError>;
}

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
        }
    }
}

impl std::error::Error for SessionError {}

/// The retained, derived state of one registered scan.
pub(crate) struct ScanState {
    /// The caller-supplied facts.
    pub(crate) record: ScanRecord,
    /// Per-category aggregation computed once at registration.
    pub(crate) report: UsageReport,
    /// The category-level cleanup plan, in the planner's canonical order.
    pub(crate) plan: CleanupPlan,
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
        }
    }

    /// Register or refresh one drive identity.
    ///
    /// Registering the same id again replaces the previous record (a drive
    /// may be re-mounted with a new label or free-space reading). Existing
    /// scans remain attached to the id.
    pub fn register_drive(&mut self, drive: Drive) {
        self.drives.insert(drive.id.clone(), drive);
    }

    /// Attach a real scan backend. This build ships none; attaching one is
    /// the explicit action that makes `startScan` / `cancelScan` live.
    pub fn attach_scan_backend(&mut self, backend: Box<dyn ScanBackend>) {
        self.backend = Some(backend);
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
        let plan = plan_cleanup(
            &report,
            &PlannerOptions {
                include_review: true,
                min_recoverable_bytes: 1,
            },
        );
        self.scans.insert(
            record.scan_id.clone(),
            ScanState {
                record,
                report,
                plan,
            },
        );
        Ok(())
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

    pub(crate) fn backend_mut(&mut self) -> Option<&mut dyn ScanBackend> {
        match self.backend.as_mut() {
            Some(backend) => Some(backend.as_mut()),
            None => None,
        }
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

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}
