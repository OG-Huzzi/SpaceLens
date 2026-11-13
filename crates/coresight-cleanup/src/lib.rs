//! CoreSight **cleanup planning and safety validation** — the pure layer
//! that turns evidence into a plan the user can review before any
//! reversible action is taken (Phase 4.1).
//!
//! jv16 PowerTools is the reference point for *scope*: a power-tools
//! suite with a cleaner, uninstaller, duplicate finder, big-file finder,
//! startup manager, and friends. This crate covers the *beginning* of
//! that surface — the planner, safety validator, preview, and
//! verify/rollback model — at CoreSight quality:
//!
//! * **Explain, don't expose.** Every opportunity carries what it is,
//!   why it's suggested, how much is recovered, what stays untouched,
//!   and what happens after removal.
//! * **Preview before action.** A [`Preview`] is generated from the plan
//!   and is complete only when it names the destination and the undo
//!   path. The validator blocks any plan whose preview is incomplete.
//! * **Resolve before authorizing.** [`resolve_plan`] turns a plan plus one
//!   [`Snapshot`](coresight_history::Snapshot) into the concrete files and
//!   directories that a preview and an execution token must both be held to.
//!   It is pure: the snapshot is data, not a disk read, and the module
//!   reports the plan bytes it could not pin instead of hiding them.
//! * **Safety veto, not persuasion.** The validator composes the
//!   existing [`SafetyGate`](coresight_capabilities::safety::SafetyGate)
//!   with CoreSight's protection domains (off-limits categories,
//!   partial-knowledge rules, self-protection). A blocked plan is
//!   returned with reasons, never partially executed.
//! * **Reversible by default.** The planner emits Trash/quarantine
//!   moves for safe categories. Destructive actions (delete, wipe)
//!   exist in the type system so they can be classified — and the
//!   validator blocks them until a separately authorized policy widens.
//! * **Honest about knowledge.** A plan built from a partial
//!   [`UsageReport`](coresight_insights::UsageReport) (observation
//!   errors or unknown sizes) is flagged `partial` and cannot be marked
//!   safe.
//!
//! ## What this crate deliberately does NOT do
//!
//! * No filesystem access, no trash adapter, no subprocess, no network,
//!   no persistence. There is **no execution primitive here**; the
//!   adapter that moves files is a later, separately authorized phase.
//! * No actual deletion, no registry mutation, no startup-item changes.
//! * No cloud upload, no telemetry, no LLM analysis.
//!
//! The UI (Phase 8) and the Trash/quarantine adapter (later Phase 4)
//! consume the types and functions here; they do not reimplement them.

pub mod planner;
pub mod preview;
pub mod resolve;
pub mod validate;
pub mod wipe;

pub use planner::{plan_cleanup, PlannerOptions};
pub use preview::{CleanupPreview, PreviewItem};
pub use resolve::{
    resolve_plan, PlanTarget, ResolveError, ResolveOptions, ResolvedItem, ResolvedPlan, TargetKind,
};
pub use validate::{validate_plan, validate_with_policy, ValidationReport, ValidationVerdict};
pub use wipe::{
    plan_wipe, plausible_deniability_supported, StorageMedium, WipeAssurance, WipeBlocker,
    WipeOptions, WipePlan, WipeStep, WipeTarget,
};

/// Safety tier for one cleanup opportunity (UX contract, docs/UX_ARCHITECTURE.md).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SafetyTier {
    /// Safe to remove with a single confirmation; reversible by default.
    Safe,
    /// Review-only: the user must read the reasoning before acting.
    Review,
    /// Never proposed as actionable. Kept in the type system so the
    /// validator can reject it explicitly.
    OffLimits,
}

/// What a cleanup item would do. Only `MoveToTrash` is emitted by the
/// planner; the other variants exist so future tools (duplicate finder,
/// uninstaller, wiper, startup manager, ...) can classify their actions
/// through the same validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Move to OS Trash / app quarantine (reversible).
    MoveToTrash,
    /// Permanent delete (destructive; blocked until authorized).
    Delete,
    /// Secure wipe (destructive; blocked until authorized).
    Wipe,
    /// Uninstall an application (reversible via reinstall; blocked until authorized).
    Uninstall,
    /// Disable a startup item (reversible; blocked until authorized).
    DisableStartup,
    /// Modify a registry/installation entry (blocked until authorized).
    ModifyRegistry,
    /// Rename files in bulk (reversible via undo; blocked until authorized).
    RenameFiles,
    /// Split a file into parts (blocked until authorized).
    SplitFile,
    /// Merge split parts (blocked until authorized).
    MergeFile,
    /// Block/unblock a domain (reversible; blocked until authorized).
    BlockDomain,
    /// Change a system setting (blocked until authorized).
    ChangeSetting,
}

impl ActionKind {
    /// Whether the action is reversible by default in CoreSight's model.
    pub fn is_reversible(self) -> bool {
        matches!(
            self,
            ActionKind::MoveToTrash
                | ActionKind::Uninstall
                | ActionKind::DisableStartup
                | ActionKind::RenameFiles
                | ActionKind::BlockDomain
        )
    }
}

/// One planned cleanup step, with its reasoning attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupItem {
    /// Stable, deterministic id (category code + rank). Not random.
    pub id: String,
    /// The human category this item covers.
    pub category: coresight_classifier::Category,
    /// Short title for the UI card, e.g. "Cache".
    pub title: String,
    /// Why this is suggested, in plain language.
    pub why_recommended: String,
    /// Bytes that would be freed if the user acts.
    pub recoverable_bytes: u64,
    /// The safety tier. `OffLimits` items are never emitted by the planner.
    pub tier: SafetyTier,
    /// What stays untouched if the user acts.
    pub what_stays_untouched: String,
    /// What happens after removal, in plain language.
    pub consequence: String,
    /// Where the item would go (Trash/quarantine). `None` is incomplete
    /// and the validator blocks it.
    pub destination: Option<String>,
    /// How to undo. `None` is incomplete and the validator blocks it.
    pub undo: Option<String>,
    /// What this item would do.
    pub action: ActionKind,
}

/// A complete, reviewable cleanup plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPlan {
    /// Deterministic plan id derived from the category totals.
    pub id: String,
    /// Items in canonical (category, size-desc) order.
    pub items: Vec<CleanupItem>,
    /// Sum of recoverable bytes across items (saturating).
    pub total_recoverable_bytes: u64,
    /// Whether the underlying report was partial (errors / unknown sizes).
    pub partial: bool,
    /// Categories deliberately excluded as off-limits.
    pub off_limits_excluded: Vec<coresight_classifier::Category>,
}

/// Why the validator blocked a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockReason {
    pub code: &'static str,
    pub detail: String,
}

impl BlockReason {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        BlockReason {
            code,
            detail: detail.into(),
        }
    }
}
