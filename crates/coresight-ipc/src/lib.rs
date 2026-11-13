//! CoreSight IPC command layer — the typed, platform-neutral
//! `coresight.v1.*` request/response contract (`docs/API_CONTRACTS.md`) and
//! the dispatcher a Tauri shell forwards into.
//!
//! This crate contains **no Tauri dependency**, no filesystem access, no
//! subprocess, no network, no database handle, and no scanner. Every fact it
//! serves comes from a [`Session`] the caller populated with already-obtained
//! engine facts. That boundary is what makes the IPC layer testable without a
//! desktop host and impossible to use as a second, unaudited filesystem path.
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
//!    * `startScan` / `cancelScan`: no scan backend is wired here →
//!      `SCAN_UNAVAILABLE`.
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
//! `getDriveSummary`, `getCategories`, `getOpportunities`, `previewPlan`, and
//! `listDrives` are served from registered session facts. `confirmPlan` and
//! `executePlan` are served in the sense that they synchronously adjudicate —
//! and, in this build, correctly refuse. Scanning and history are not
//! available by design.
//!
//! # Deliberate limitations (stated, not hidden)
//!
//! * No scan backend, history store, or executor is attached.
//! * `exclusionsNote` is `null` because this build records no exclusion
//!   report; it is not an empty claim.
//! * Subcategory deltas are unknown (`null`): only top-level category deltas
//!   are derived from a retained previous scan.
//! * A Category row is a semantic bucket; concrete paths exist only in a
//!   plan preview, where they use the workspace's lossless path encoding.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod command;
pub mod dispatch;
pub mod error;
pub mod response;
pub mod session;
mod time;

pub use command::{
    CancelScanRequest, Command, ConfirmPlanRequest, ExecutePlanRequest, GetCategoriesRequest,
    GetDriveSummaryRequest, GetHistoryRequest, GetOpportunitiesRequest, ListDrivesRequest,
    PreviewPlanRequest, StartScanRequest,
};
pub use dispatch::{dispatch, dispatch_json, dispatch_value};
pub use error::{codes, ApiError, Envelope};
pub use response::{
    CancelScanResponse, Category, ConfirmPlanResponse, Drive, DriveKind, DriveSummary,
    ExecutePlanResponse, Opportunity, OpportunityPreview, OpportunityTier, PlanPreview,
    PlanPreviewItem, PlanRejection, PlanResult, PreviewTargetKind, Response, ScanStatus,
    ScanSummaryRef, ScanTotals, SnapshotCategoryDelta, SnapshotDelta, StartScanResponse,
};
pub use session::{ScanBackend, ScanRecord, Session, SessionError};
