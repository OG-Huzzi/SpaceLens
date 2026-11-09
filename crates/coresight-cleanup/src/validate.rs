//! The validator: veto power over any plan the planner emits.
//!
//! It composes two independent checks:
//!
//! 1. **CoreSight's protection domains** — off-limits categories, partial
//!    knowledge, incomplete previews, byte-accounting mismatches,
//!    duplicate ids, and the requirement that every item carry its
//!    reasoning.
//! 2. **The capabilities safety gate** — the existing
//!    [`SafetyGate`](coresight_capabilities::safety::SafetyGate) vetoes
//!    any action the current build's [`ExecutionPolicy`] does not
//!    authorize. In the current build only read-only actions execute, so
//!    a reversible plan is *correctly* blocked until a separately
//!    authorized phase widens the policy.
//!
//! A blocked plan is returned with reasons, never partially executed.

use std::collections::BTreeSet;

use coresight_capabilities::capability::CapabilityId;
use coresight_capabilities::safety::{
    ActionClassification, ActionEffect, ExecutionPolicy, ProposedAction, SafetyGate,
};

use crate::validate::ValidationVerdict::{Allowed, Blocked};
use crate::{ActionKind, BlockReason, CleanupPlan, SafetyTier};

/// The verdict on a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationVerdict {
    Allowed,
    Blocked(Vec<BlockReason>),
}

impl ValidationVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Allowed)
    }

    pub fn reasons(&self) -> &[BlockReason] {
        match self {
            Allowed => &[],
            Blocked(reasons) => reasons,
        }
    }
}

/// The full validation report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub verdict: ValidationVerdict,
}

impl ValidationReport {
    pub fn is_allowed(&self) -> bool {
        self.verdict.is_allowed()
    }

    /// Human-readable blocking reasons (for the UI to show the user).
    pub fn blocking_messages(&self) -> Vec<String> {
        self.verdict
            .reasons()
            .iter()
            .map(|r| format!("{}: {}", r.code, r.detail))
            .collect()
    }
}

/// Map a cleanup action to its capabilities classification.
fn classify(action: ActionKind, item: &crate::CleanupItem) -> ActionClassification {
    let effect = match action {
        ActionKind::MoveToTrash
        | ActionKind::Uninstall
        | ActionKind::DisableStartup
        | ActionKind::RenameFiles
        | ActionKind::BlockDomain => ActionEffect::Reversible,
        ActionKind::Delete | ActionKind::Wipe => ActionEffect::Destructive,
        ActionKind::ModifyRegistry
        | ActionKind::SplitFile
        | ActionKind::MergeFile
        | ActionKind::ChangeSetting => ActionEffect::Destructive,
    };
    let permission_sensitive = matches!(
        item.category,
        coresight_classifier::Category::Downloads
            | coresight_classifier::Category::UserData
            | coresight_classifier::Category::Documents
    );
    ActionClassification::new(effect, false, permission_sensitive)
}

/// Validate a plan against the default (current-build) policy.
pub fn validate_plan(plan: &CleanupPlan) -> ValidationReport {
    validate_with_policy(plan, ExecutionPolicy::CURRENT_BUILD)
}

/// Validate a plan against an explicit policy. Production code uses
/// [`validate_plan`] (the current build); tests use this to prove the
/// reversible path is executable once authorized.
pub fn validate_with_policy(plan: &CleanupPlan, policy: ExecutionPolicy) -> ValidationReport {
    let mut reasons: Vec<BlockReason> = Vec::new();
    let gate = SafetyGate::with_policy(policy);

    // 1) Protection domains.
    if plan.partial && plan.items.iter().any(|it| it.tier == SafetyTier::Safe) {
        reasons.push(BlockReason::new(
            "PARTIAL_KNOWLEDGE",
            "the underlying report is partial (observation errors or unknown sizes); safe-tier recommendations require complete knowledge",
        ));
    }
    let mut seen_ids = BTreeSet::new();
    let mut byte_sum: u64 = 0;
    for item in &plan.items {
        if item.tier == SafetyTier::OffLimits {
            reasons.push(BlockReason::new(
                "OFF_LIMITS_CATEGORY",
                format!(
                    "category {} is off-limits and can never be proposed as actionable",
                    item.category.code()
                ),
            ));
        }
        if item.why_recommended.is_empty()
            || item.what_stays_untouched.is_empty()
            || item.consequence.is_empty()
        {
            reasons.push(BlockReason::new(
                "MISSING_REASONING",
                format!("item {} is missing its explanation", item.id),
            ));
        }
        if item.destination.is_none() || item.undo.is_none() {
            reasons.push(BlockReason::new(
                "INCOMPLETE_PREVIEW",
                format!(
                    "item {} does not name both a destination and an undo path",
                    item.id
                ),
            ));
        }
        if !seen_ids.insert(item.id.clone()) {
            reasons.push(BlockReason::new(
                "DUPLICATE_ITEM",
                format!("duplicate plan item id {}", item.id),
            ));
        }
        byte_sum = byte_sum.saturating_add(item.recoverable_bytes);
        // 2) Safety gate per item.
        let action = ProposedAction::new(
            CapabilityId::StorageAnalysis,
            format!("{}: {}", item.title, item.id),
            classify(item.action, item),
        );
        if let coresight_capabilities::safety::GateVerdict::Blocked(blocked) = gate.veto(&action) {
            for b in blocked {
                reasons.push(BlockReason::new(
                    "GATE_BLOCKED",
                    format!("item {}: {}", item.id, b.code()),
                ));
            }
        }
    }
    if byte_sum != plan.total_recoverable_bytes {
        reasons.push(BlockReason::new(
            "BYTE_ACCOUNTING_MISMATCH",
            format!(
                "plan total {} does not equal the sum of its items {}",
                plan.total_recoverable_bytes, byte_sum
            ),
        ));
    }

    ValidationReport {
        verdict: if reasons.is_empty() {
            Allowed
        } else {
            Blocked(reasons)
        },
    }
}
