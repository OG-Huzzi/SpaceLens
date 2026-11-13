//! The typed response payloads of `coresight.v1.*` (`docs/API_CONTRACTS.md`
//! §Commands), plus the [`Response`] union the dispatcher returns.
//!
//! Every payload serializes with `camelCase` keys, integer byte counts, and
//! RFC 3339 UTC timestamps (rendered by the dispatcher; see the crate docs).
//! The contract's category payload deliberately has **no path field** — the
//! UI must never need filesystem knowledge to render a category.
//!
//! ## Honesty fields are additive
//!
//! The contract lists the *semantic* fields, not an exhaustive JSON schema.
//! Three additive fields exist because the alternative would be a lie:
//!
//! * [`Category::complete`] and [`Category::entries_without_size`] — a byte
//!   total that excludes unmeasured entries must say so.
//! * [`PlanPreview::complete`] and [`PlanPreview::claimed_bytes`] — the
//!   concrete preview total and the category-level claim are different
//!   numbers; both are published.
//! * [`DriveSummary::exclusions_note`] is `None` when this build recorded no
//!   exclusion report, never a fabricated empty string.

use serde::{Deserialize, Serialize};

/// The `startScan` success payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartScanResponse {
    /// The backend-assigned scan id.
    pub scan_id: String,
}

/// The `cancelScan` success payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelScanResponse {
    /// True when the backend accepted the cancellation.
    pub cancelled: bool,
}

/// One category (or subcategory) row. Contains no paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    /// Stable category or subcategory code (`"CACHE"`, `"BUILD_OUTPUT"`).
    pub id: String,
    /// Human-readable name for display.
    pub name: String,
    /// Measured logical bytes in this bucket (integer).
    pub bytes: u64,
    /// `bytes` divided by the parent's measured total. `None` when the
    /// parent total is zero **or unknown** (the scan has unmeasured entries
    /// or observation errors); never a fake `0.0`.
    pub share_of_parent: Option<f64>,
    /// Change versus the previous full-scope scan of the same drive.
    /// `None` when there is no comparable prior scan or either report is
    /// incomplete — unknown is not "no change".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_since_last: Option<i64>,
    /// Number of observed entries in this bucket.
    pub item_count: u64,
    /// Whether this bucket's byte total covers every entry the scan saw
    /// (no entry lacked a size and the scan had no observation errors).
    pub complete: bool,
    /// Entries in this bucket whose size was unknown and therefore
    /// contributed no bytes.
    pub entries_without_size: u64,
}

/// The cleanup safety tier exposed to the UI (`safe|review`).
///
/// `OffLimits` is deliberately absent: the planner never emits it, and the
/// IPC contract has no third actionability state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpportunityTier {
    /// Safe to act on with a single confirmation.
    Safe,
    /// Requires the user to read the reasoning first.
    Review,
}

/// A compact preview attached to one opportunity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpportunityPreview {
    /// One-sentence description of the proposed move and its undo.
    pub summary: String,
    /// Bytes the opportunity would move.
    pub bytes: u64,
    /// Where the files would go, when the planner named a destination.
    pub destination: Option<String>,
    /// How the move can be undone, when the planner named an undo path.
    pub undo_path: Option<String>,
}

/// One cleanup opportunity, with all of its reasoning attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opportunity {
    /// Stable deterministic id, scan-scoped (`"scan-1:CACHE:30000"`) so two
    /// retained scans can never make the same category-level card ambiguous.
    pub id: String,
    /// Safety tier (`safe|review`).
    pub tier: OpportunityTier,
    /// Short title for the UI card.
    pub title: String,
    /// What the opportunity covers, in plain language.
    pub what_it_is: String,
    /// Why it is recommended.
    pub why_recommended: String,
    /// Bytes in the affected bucket.
    pub bytes: u64,
    /// Bytes the opportunity would recover.
    pub recoverable_bytes: u64,
    /// What stays untouched if the user acts.
    pub stays_untouched: String,
    /// What happens after removal.
    pub consequence: String,
    /// The compact preview the confirmation screen can render.
    pub preview: OpportunityPreview,
}

/// Whether a resolved preview target is a file or a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewTargetKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
}

/// One concrete target named by a plan preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreviewItem {
    /// The opportunity this target came from.
    pub item_id: String,
    /// The opportunity's short title.
    pub title: String,
    /// Lossless path encoding (`u:<utf8>` or `e:<hex>`), as produced by
    /// `coresight_history::encode_path`. Never a lossy string.
    pub path: String,
    /// File or directory.
    pub kind: PreviewTargetKind,
    /// Logical lower-bound bytes for a directory; exact logical size for a
    /// file.
    pub bytes: u64,
}

/// The `previewPlan` success payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    /// Deterministic id used by `confirmPlan` / `executePlan`.
    pub plan_id: String,
    /// Exact concrete targets, in plan order then path order.
    pub items: Vec<PlanPreviewItem>,
    /// Sum of the concrete targets' bytes (what the preview actually pins).
    pub bytes: u64,
    /// The category-level plan's recoverable-byte claim.
    pub claimed_bytes: u64,
    /// Where the files would go; `None` when not consistently specified.
    pub destination: Option<String>,
    /// How to undo the move; `None` when not consistently specified.
    pub undo_path: Option<String>,
    /// Honest warnings the user must acknowledge before confirmation.
    pub warnings: Vec<String>,
    /// True only when the underlying scan is complete and every claimed
    /// byte was pinned to a concrete target.
    pub complete: bool,
}

/// One rejected plan item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRejection {
    /// Stable machine-readable reason code.
    pub code: String,
    /// User-facing explanation.
    pub message: String,
}

/// The `confirmPlan` success payload.
///
/// In the current build every plan is rejected with a typed
/// `PLAN_REJECTED_UNSAFE` error instead; this payload exists for the
/// separately authorized phase that widens [`ExecutionPolicy`] and is part
/// of the `v1` shape so the UI does not change later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmPlanResponse {
    /// Whether the plan passed synchronous safety validation.
    pub accepted: bool,
    /// Per-item rejections; empty when accepted.
    pub rejected: Vec<PlanRejection>,
}

/// One executed plan item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanResult {
    /// The opportunity this result belongs to.
    pub item_id: String,
    /// What happened, in plain language.
    pub outcome: String,
    /// Bytes the item accounted for.
    pub bytes: u64,
}

/// The `executePlan` success payload.
///
/// No executor exists in this build; this shape is the documented future
/// contract. `freed_complete` exists so a partial `freed_bytes` can never be
/// presented as the whole truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutePlanResponse {
    /// One result per planned item.
    pub results: Vec<PlanResult>,
    /// Bytes actually freed, when execution succeeded.
    pub freed_bytes: u64,
    /// Whether every planned byte was accounted for by `freed_bytes`.
    pub freed_complete: bool,
    /// Items that could not be completed and were left in place.
    pub leftovers: Vec<String>,
}

/// One category's change between two scans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCategoryDelta {
    /// Stable category code.
    pub category_id: String,
    /// Signed byte change (`new - old`).
    pub bytes_delta: i64,
    /// Whether both reports covered every entry (otherwise the delta is a
    /// lower-bound statement, not a complete one).
    pub complete: bool,
}

/// The `getHistory` payload element (documented future shape; no history
/// store is attached to this build, so the command returns
/// `HISTORY_UNAVAILABLE`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDelta {
    /// Id of the earlier run.
    pub from_scan_id: String,
    /// Id of the later run.
    pub to_scan_id: String,
    /// RFC 3339 UTC completion time of the earlier run.
    pub from_at: String,
    /// RFC 3339 UTC completion time of the later run.
    pub to_at: String,
    /// Signed total-byte change across every category.
    pub total_bytes_delta: i64,
    /// Whether both runs observed their full declared scope.
    pub complete: bool,
    /// Per-category changes in canonical category order.
    pub categories: Vec<SnapshotCategoryDelta>,
}

/// The kind of drive/volume, mirroring the engine's platform-neutral model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveKind {
    /// Fixed internal volume.
    Internal,
    /// Removable volume (USB, SD card).
    Removable,
    /// Network share.
    Network,
    /// The platform could not classify the volume.
    Unknown,
}

/// One drive known to the session (connected or remembered-offline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// Stable drive id (volume serial where the platform proves one).
    pub id: String,
    /// Mount root as observed, e.g. `C:\` or `/`.
    pub root: String,
    /// Volume label when the platform exposes one.
    pub label: Option<String>,
    /// Volume kind.
    pub kind: DriveKind,
    /// Capacity in bytes when the platform reports it.
    pub capacity_bytes: Option<u64>,
    /// Available bytes when the platform reports them.
    pub available_bytes: Option<u64>,
    /// False for a remembered drive that is not currently mounted.
    pub connected: bool,
}

/// Lifecycle of a registered scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanStatus {
    /// Still running; results are not served.
    Running,
    /// Observation completed.
    Completed,
    /// Committed, but at least one bounded pipeline stage reported a limit.
    CompletedWithLimits,
    /// Cancelled mid-run; the observed subset is partial by definition.
    Cancelled,
    /// Failed; nothing may be served as a complete report.
    Failed,
}

impl ScanStatus {
    /// Whether this status observed the run's full declared scope.
    ///
    /// Only these runs may serve category totals or opportunities as a
    /// complete picture. `CompletedWithLimits` qualifies because the limit
    /// concerns bounded pipeline work, not path observation, matching
    /// `coresight_history::RunStatus::observes_full_scope`.
    pub fn observes_full_scope(self) -> bool {
        matches!(
            self,
            ScanStatus::Completed | ScanStatus::CompletedWithLimits
        )
    }
}

/// A compact reference to one scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummaryRef {
    /// The scan id.
    pub scan_id: String,
    /// Its lifecycle state.
    pub status: ScanStatus,
    /// RFC 3339 UTC completion time; `None` when the scan has not finished
    /// or the platform clock value is not representable.
    pub completed_at: Option<String>,
    /// Whether the scan observed its full declared scope.
    pub complete_scope: bool,
}

/// Aggregate totals for one scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanTotals {
    /// The scan these totals come from.
    pub scan_id: String,
    /// Measured logical bytes (integer).
    pub bytes: u64,
    /// Whether every observed entry contributed a known size and no entry
    /// had an observation error.
    pub complete: bool,
    /// Every entry seen, classified or not.
    pub scanned_entries: u64,
    /// Entries whose size was unknown (they contributed no bytes).
    pub entries_without_size: u64,
    /// Entries seen but not fully readable.
    pub errored_entries: u64,
    /// Entries with no classification or an unrecognized category code.
    pub unclassified_entries: u64,
}

/// The `getDriveSummary` success payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveSummary {
    /// The drive this summary describes.
    pub drive_id: String,
    /// Whether the drive is currently mounted.
    pub connected: bool,
    /// Totals from the latest full-scope scan, when one is retained.
    pub totals: Option<ScanTotals>,
    /// What was NOT scanned, when the producing run recorded it. `None`
    /// means this build has no exclusion report — not "nothing excluded".
    pub exclusions_note: Option<String>,
    /// The most recent finished scan, if any.
    pub last_scan: Option<ScanSummaryRef>,
    /// Number of snapshots retained for this drive in the session.
    pub snapshot_count: u64,
}

/// The dispatcher's typed success payload.
///
/// The serde frame is `{ "kind": "coresight.v1.<command>", "payload": ... }`
/// so the payload itself stays exactly the documented shape (for example a
/// bare `Category[]`). This is the one documented mapping the transport
/// adds on top of the per-command payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
#[allow(clippy::large_enum_variant)]
pub enum Response {
    /// `startScan` result.
    #[serde(rename = "coresight.v1.startScan")]
    StartScan(StartScanResponse),
    /// `cancelScan` result.
    #[serde(rename = "coresight.v1.cancelScan")]
    CancelScan(CancelScanResponse),
    /// `getDriveSummary` result.
    #[serde(rename = "coresight.v1.getDriveSummary")]
    DriveSummary(DriveSummary),
    /// `getCategories` result (a `Category[]` payload).
    #[serde(rename = "coresight.v1.getCategories")]
    Categories(Vec<Category>),
    /// `getOpportunities` result (an `Opportunity[]` payload).
    #[serde(rename = "coresight.v1.getOpportunities")]
    Opportunities(Vec<Opportunity>),
    /// `previewPlan` result.
    #[serde(rename = "coresight.v1.previewPlan")]
    PlanPreview(PlanPreview),
    /// `confirmPlan` result.
    #[serde(rename = "coresight.v1.confirmPlan")]
    ConfirmPlan(ConfirmPlanResponse),
    /// `executePlan` result.
    #[serde(rename = "coresight.v1.executePlan")]
    ExecutePlan(ExecutePlanResponse),
    /// `getHistory` result.
    #[serde(rename = "coresight.v1.getHistory")]
    History(Vec<SnapshotDelta>),
    /// `listDrives` result (a `Drive[]` payload).
    #[serde(rename = "coresight.v1.listDrives")]
    Drives(Vec<Drive>),
}

impl Response {
    /// The namespaced command name this payload answers.
    pub fn command_name(&self) -> &'static str {
        match self {
            Response::StartScan(_) => "coresight.v1.startScan",
            Response::CancelScan(_) => "coresight.v1.cancelScan",
            Response::DriveSummary(_) => "coresight.v1.getDriveSummary",
            Response::Categories(_) => "coresight.v1.getCategories",
            Response::Opportunities(_) => "coresight.v1.getOpportunities",
            Response::PlanPreview(_) => "coresight.v1.previewPlan",
            Response::ConfirmPlan(_) => "coresight.v1.confirmPlan",
            Response::ExecutePlan(_) => "coresight.v1.executePlan",
            Response::History(_) => "coresight.v1.getHistory",
            Response::Drives(_) => "coresight.v1.listDrives",
        }
    }
}
