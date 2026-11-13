//! CoreSight IPC command layer — the typed, platform-neutral
//! `coresight.v1.*` request/response contract (`docs/API_CONTRACTS.md`) and
//! the dispatcher a Tauri shell forwards into.
//!
//! This crate contains **no Tauri dependency**. It reaches the filesystem only
//! through the explicitly attached [`ScanBackend`] — this build ships
//! [`EngineScanBackend`], which wraps the real `coresight_engine` walk — and
//! never invokes a subprocess, a network socket, a database handle, or an
//! executor. Every fact it serves comes from that backend or from a
//! [`Session`] the caller populated with already-obtained engine facts.
//!
//! # Wire framing
//!
//! A command is one object with the full namespaced discriminant and a typed
//! payload:
//!
//! ```json
//! { "command": "coresight.v1.getCategories",
//!   "payload": { "scanId": "scan-1", "parentCategory": "CACHE" } }
//! ```
//!
//! A result is an [`Envelope`] with exactly one side set. The success payload
//! is tagged with the same namespaced name and a bare document payload, so
//! `getCategories` really is `Category[]`:
//!
//! ```json
//! { "ok": { "kind": "coresight.v1.getCategories", "payload": [ /* Category[] */ ] } }
//! ```
//!
//! An error is `{ "error": { "code", "message", "detail" } }`. The UI renders
//! `message` verbatim for known codes and a generic safe fallback otherwise;
//! no error here contains a stack trace, a raw OS message, or a path.
//!
//! # The honesty rules (the point of this crate)
//!
//! 1. **Never fabricate.** A command this build cannot genuinely serve returns
//!    a typed error — never placeholder or sample data.
//!    * `startScan` / `cancelScan`: a real backend is attached by the shell;
//!      a session with **no** backend still refuses with `SCAN_UNAVAILABLE`.
//!    * `getHistory`: no history store is attached → `HISTORY_UNAVAILABLE`.
//!    * `executePlan`: no executor exists, and the current
//!      `ExecutionPolicy` authorizes read-only actions only.
//! 2. **Unknown ≠ zero.** Category and drive totals carry `complete` and
//!    `entriesWithoutSize`; a partial byte total can never masquerade as the
//!    whole.
//! 3. **Share can be unknown.** `shareOfParent` is `null` when the parent
//!    total is zero or incomplete — never a fake `0.0`.
//! 4. **Safety rejection is correct behaviour.** `confirmPlan` runs the real
//!    `coresight_cleanup` validator under the current policy. Every cleanup
//!    plan is a reversible move, the policy authorizes only read-only
//!    actions, so **every plan is rejected** with `PLAN_REJECTED_UNSAFE` and
//!    the validator's reasons. `executePlan` refuses with the same typed
//!    rejection. This is the safety contract working as designed, not a bug;
//!    it must stay that way until a separately authorized phase widens the
//!    policy and adds an executor.
//! 5. **Deterministic.** Categories are in `Category::ALL` order,
//!    opportunities are in the planner's order, preview items are in request
//!    order. The same session plus the same command serializes
//!    byte-identically.
//! 6. **No panics.** Production code has no `unwrap`, `expect`, `panic!`, or
//!    `unreachable!`; malformed input becomes a typed error.
//!
//! # What is genuinely served
//!
//! `startScan` runs a real walk through [`EngineScanBackend`] and
//! `cancelScan` stops it through the engine's cancellation token. The ten
//! aggregate view commands are served from the resulting snapshot/report
//! once a scan completes; `getDriveSummary`, `getCategories`,
//! `getOpportunities`, `previewPlan`, and `listDrives` are served from the
//! same registered facts. `confirmPlan` and `executePlan` are served in the
//! sense that they synchronously adjudicate — and, in this build, correctly
//! refuse. History is not available by design.
//!
//! The ten aggregate view commands (`getGlobalStatus`, `getHomeSummary`,
//! `getStorage`, `getCleanup`, `getDuplicateReport`, `getBigFileReport`,
//! `getStartupReport`, `getDnsRanking`, `getBlockListPlan`,
//! `getPrivacySummary`) are served from the latest completed scan plus the
//! startup inventory, DNS ranking, block-list plan, and privacy findings the
//! shell registers. They mirror `src/api.ts` field for field and return
//! `SESSION_UNAVAILABLE` — never sample data — when the session lacks the
//! facts.
//!
//! # Deliberate limitations (stated, not hidden)
//!
//! * No history store and no executor are attached.
//! * Incremental scans do not exist: `fullRescan` is accepted but every scan
//!   is a full walk, because this build keeps no incremental index.
//! * Content hashing runs only over same-size duplicate candidates; files
//!   never hashed stay `content_sha256: None` and are counted as unverified.
//! * `exclusionsNote` is `null` because this build records no exclusion
//!   report; it is not an empty claim.
//! * Subcategory deltas are unknown (`null`): only top-level category deltas
//!   are derived from a retained previous scan.
//! * A Category row is a semantic bucket; concrete paths exist only in a
//!   plan preview, where they use the workspace's lossless path encoding.
//! * Aggregate views read no clock. `generatedAt` and big-file ages use the
//!   scan's registered completion instant; drive health and `etaSeconds` are
//!   `null` because this build records no evidence for them.
//! * `DataCoverage::excludedAreas` is empty because a `ScanRecord` carries no
//!   exclusion report; the field is not an assertion that nothing was
//!   excluded.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod command;
pub mod dispatch;
pub mod error;
mod reports;
pub mod response;
pub mod scan;
pub mod session;
mod time;
pub mod views;

pub use command::{
    CancelScanRequest, Command, ConfirmPlanRequest, ExecutePlanRequest, GetBigFileReportRequest,
    GetBlockListPlanRequest, GetCategoriesRequest, GetCleanupRequest, GetDnsRankingRequest,
    GetDriveSummaryRequest, GetDuplicateReportRequest, GetGlobalStatusRequest, GetHistoryRequest,
    GetHomeSummaryRequest, GetOpportunitiesRequest, GetPrivacySummaryRequest,
    GetStartupReportRequest, GetStorageRequest, ListDrivesRequest, PreviewPlanRequest,
    StartScanRequest,
};
pub use dispatch::{dispatch, dispatch_json, dispatch_value};
pub use error::{codes, ApiError, Envelope};
pub use response::{
    CancelScanResponse, Category, ConfirmPlanResponse, Drive, DriveKind, DriveSummary,
    ExecutePlanResponse, Opportunity, OpportunityPreview, OpportunityTier, PlanPreview,
    PlanPreviewItem, PlanRejection, PlanResult, PreviewTargetKind, Response, ScanStatus,
    ScanSummaryRef, ScanTotals, SnapshotCategoryDelta, SnapshotDelta, StartScanResponse,
};
pub use scan::{
    default_scan_options, is_usable_root, EngineScanBackend, ScanBackend, ScanBackendError,
    ScanCompleteEvent, ScanCompleteSummary, ScanFailure, ScanFinished, ScanHandle,
    ScanProgressEvent, ScanProgressSnapshot, ScanProgressUpdate, ScanRequest,
};
pub use session::{ScanRecord, Session, SessionError};
pub use views::{
    BigFileReportView, BigFileView, BlockListEntryView, BlockListPlanView, CategoryDeltaView,
    CategoryTotals, ChangeSinceLastScanView, CleanupDataView, CleanupEvidence, CleanupEvidenceKind,
    CleanupItemView, CleanupTier, DataCompleteness, DataCoverage, DnsExcludedCandidate,
    DnsExclusionReasonView, DnsPrivacyPolicyView, DnsRankedCandidate, DnsRankingView,
    DriveConnectionView, DriveHealthView, DriveKindView, DriveView, DuplicateGroupView,
    DuplicateMemberView, DuplicateReportView, GlobalStatusView, HomeSummaryView,
    PrivacyFindingKindView, PrivacyFindingView, PrivacySummaryView, RejectedDomainView,
    ScanStateView, ScanStatusView, StartupItemView, StartupReportView, StartupTierView,
    StorageDataView, StorageLocation,
};
