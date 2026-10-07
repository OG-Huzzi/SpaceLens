//! The generic action classification and safety pipeline (Phase 6.1, Task 5).
//!
//! A broad power-tools product must NOT become a dangerous toolbox. This
//! module defines the contract every future capability action flows
//! through:
//!
//! - Actions are **explicitly classified** — exactly one effect
//!   (read-only / reversible / destructive) plus optional qualifiers
//!   (privileged, permission-sensitive). There is no unclassified action:
//!   [`ProposedAction::new`] requires a classification.
//! - Actions pass the stages **in order** — OBSERVE → ANALYZE → RECOMMEND
//!   → PREVIEW → VALIDATE → EXECUTE → VERIFY → ROLLBACK. The pipeline has
//!   no API to skip a stage ([`ActionPipeline::advance_to`] accepts only
//!   the next stage), and EXECUTE additionally requires an ALLOWED verdict
//!   from the safety gate. No future module can jump directly from
//!   discovery to deletion.
//! - The [`SafetyGate`] is the **veto point**: it depends on nothing but
//!   the action's classification and the build's execution policy, so it
//!   cannot be persuaded by a caller.
//! - In the current build the gate authorizes read-only effects only —
//!   no destructive, reversible, privileged, or permission-sensitive
//!   action can reach EXECUTE until a separately authorized phase widens
//!   [`ExecutionPolicy`] (docs/SECURITY_AND_SAFETY.md).
//!
//! No executor exists in this phase: this module is the boundary, and
//! tests prove the boundary holds.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::capability::CapabilityId;

/// Ordered safety stages. Declaration order IS the rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActionStage {
    Observe,
    Analyze,
    Recommend,
    Preview,
    Validate,
    Execute,
    Verify,
    Rollback,
}

impl ActionStage {
    pub const ORDER: [ActionStage; 8] = [
        ActionStage::Observe,
        ActionStage::Analyze,
        ActionStage::Recommend,
        ActionStage::Preview,
        ActionStage::Validate,
        ActionStage::Execute,
        ActionStage::Verify,
        ActionStage::Rollback,
    ];

    pub fn rank(self) -> u8 {
        match self {
            ActionStage::Observe => 0,
            ActionStage::Analyze => 1,
            ActionStage::Recommend => 2,
            ActionStage::Preview => 3,
            ActionStage::Validate => 4,
            ActionStage::Execute => 5,
            ActionStage::Verify => 6,
            ActionStage::Rollback => 7,
        }
    }

    pub fn next(self) -> Option<ActionStage> {
        match self {
            ActionStage::Observe => Some(ActionStage::Analyze),
            ActionStage::Analyze => Some(ActionStage::Recommend),
            ActionStage::Recommend => Some(ActionStage::Preview),
            ActionStage::Preview => Some(ActionStage::Validate),
            ActionStage::Validate => Some(ActionStage::Execute),
            ActionStage::Execute => Some(ActionStage::Verify),
            ActionStage::Verify => Some(ActionStage::Rollback),
            ActionStage::Rollback => None,
        }
    }
}

/// The five action classes (Phase 6.1, Task 5). The three effect classes
/// are mutually exclusive; the two qualifier classes may accompany any
/// effect. See [`ActionClassification::classes`] for the canonical tag list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActionClass {
    ReadOnly,
    Reversible,
    Destructive,
    Privileged,
    PermissionSensitive,
}

/// The effect an action would have. Exactly one per action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActionEffect {
    ReadOnly,
    Reversible,
    Destructive,
}

impl From<ActionEffect> for ActionClass {
    fn from(e: ActionEffect) -> Self {
        match e {
            ActionEffect::ReadOnly => ActionClass::ReadOnly,
            ActionEffect::Reversible => ActionClass::Reversible,
            ActionEffect::Destructive => ActionClass::Destructive,
        }
    }
}

/// Explicit classification. An action without one cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionClassification {
    effect: ActionEffect,
    requires_privilege: bool,
    touches_permission_gated_data: bool,
}

impl ActionClassification {
    /// The canonical read-only observation classification.
    pub const READ_ONLY: Self = Self {
        effect: ActionEffect::ReadOnly,
        requires_privilege: false,
        touches_permission_gated_data: false,
    };

    pub const fn new(
        effect: ActionEffect,
        requires_privilege: bool,
        touches_permission_gated_data: bool,
    ) -> Self {
        Self {
            effect,
            requires_privilege,
            touches_permission_gated_data,
        }
    }

    pub fn effect(&self) -> ActionEffect {
        self.effect
    }

    pub fn requires_privilege(&self) -> bool {
        self.requires_privilege
    }

    pub fn touches_permission_gated_data(&self) -> bool {
        self.touches_permission_gated_data
    }

    /// The canonical tag list over the five classes.
    pub fn classes(&self) -> Vec<ActionClass> {
        let mut out = vec![ActionClass::from(self.effect)];
        if self.requires_privilege {
            out.push(ActionClass::Privileged);
        }
        if self.touches_permission_gated_data {
            out.push(ActionClass::PermissionSensitive);
        }
        out
    }
}

/// One proposed capability action: what it is, and how it is classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedAction {
    capability: CapabilityId,
    summary: String,
    classification: ActionClassification,
}

impl ProposedAction {
    pub fn new(
        capability: CapabilityId,
        summary: impl Into<String>,
        classification: ActionClassification,
    ) -> Self {
        ProposedAction {
            capability,
            summary: summary.into(),
            classification,
        }
    }

    pub fn capability(&self) -> CapabilityId {
        self.capability
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn classification(&self) -> &ActionClassification {
        &self.classification
    }
}

/// The policy floor for the current build. Only read-only actions may ever
/// execute; widening any flag is a deliberate, separately authorized
/// change with its own review and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub read_only: bool,
    pub reversible: bool,
    pub destructive: bool,
    pub privileged: bool,
    pub permission_sensitive: bool,
}

impl ExecutionPolicy {
    /// This build: observation only. Nothing state-changing executes.
    pub const CURRENT_BUILD: Self = Self {
        read_only: true,
        reversible: false,
        destructive: false,
        privileged: false,
        permission_sensitive: false,
    };
}

/// Why the gate blocked an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    ClassNotAuthorized(ActionClass),
    PrivilegeNotAuthorized,
    PermissionSensitiveNotAuthorized,
}

impl BlockReason {
    /// Stable machine-readable code (IPC-ready).
    pub fn code(self) -> &'static str {
        match self {
            BlockReason::ClassNotAuthorized(ActionClass::ReadOnly) => "READ_ONLY_NOT_AUTHORIZED",
            BlockReason::ClassNotAuthorized(ActionClass::Reversible) => "REVERSIBLE_NOT_AUTHORIZED",
            BlockReason::ClassNotAuthorized(ActionClass::Destructive) => {
                "DESTRUCTIVE_NOT_AUTHORIZED"
            }
            BlockReason::ClassNotAuthorized(ActionClass::Privileged) => "PRIVILEGED_NOT_AUTHORIZED",
            BlockReason::ClassNotAuthorized(ActionClass::PermissionSensitive) => {
                "PERMISSION_SENSITIVE_NOT_AUTHORIZED"
            }
            BlockReason::PrivilegeNotAuthorized => "PRIVILEGE_NOT_AUTHORIZED",
            BlockReason::PermissionSensitiveNotAuthorized => "PERMISSION_SENSITIVE_NOT_AUTHORIZED",
        }
    }
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// The gate's verdict on one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    Allowed,
    Blocked(Vec<BlockReason>),
}

impl GateVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, GateVerdict::Allowed)
    }

    pub fn reasons(&self) -> &[BlockReason] {
        match self {
            GateVerdict::Allowed => &[],
            GateVerdict::Blocked(reasons) => reasons,
        }
    }
}

/// The veto point. Depends on nothing but the classification and the
/// policy — it cannot be persuaded by a caller.
#[derive(Debug, Clone, Copy)]
pub struct SafetyGate {
    policy: ExecutionPolicy,
}

impl SafetyGate {
    pub const fn with_policy(policy: ExecutionPolicy) -> Self {
        SafetyGate { policy }
    }

    pub fn new() -> Self {
        SafetyGate::with_policy(ExecutionPolicy::CURRENT_BUILD)
    }

    pub fn policy(&self) -> &ExecutionPolicy {
        &self.policy
    }

    pub fn veto(&self, action: &ProposedAction) -> GateVerdict {
        let c = action.classification();
        let mut reasons = Vec::new();
        let effect_class = ActionClass::from(c.effect());
        let effect_allowed = match c.effect() {
            ActionEffect::ReadOnly => self.policy.read_only,
            ActionEffect::Reversible => self.policy.reversible,
            ActionEffect::Destructive => self.policy.destructive,
        };
        if !effect_allowed {
            reasons.push(BlockReason::ClassNotAuthorized(effect_class));
        }
        if c.requires_privilege() && !self.policy.privileged {
            reasons.push(BlockReason::PrivilegeNotAuthorized);
        }
        if c.touches_permission_gated_data() && !self.policy.permission_sensitive {
            reasons.push(BlockReason::PermissionSensitiveNotAuthorized);
        }
        if reasons.is_empty() {
            GateVerdict::Allowed
        } else {
            GateVerdict::Blocked(reasons)
        }
    }
}

impl Default for SafetyGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the pipeline refused a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineError {
    /// Attempted a stage other than the next one in order.
    StageSkipped {
        reached: ActionStage,
        attempted: ActionStage,
    },
    /// The pipeline is complete; no stage follows ROLLBACK.
    AlreadyComplete,
    /// The VALIDATE stage may only be passed by [`ActionPipeline::validate`].
    ValidateRequiresGate,
    /// EXECUTE was attempted without a recorded gate verdict (unreachable
    /// by construction; kept as defense in depth).
    NotValidated,
    /// The safety gate blocked execution; the reasons are binding.
    GateBlocked(Vec<BlockReason>),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::StageSkipped { reached, attempted } => write!(
                f,
                "stage skipped: reached {reached:?}, attempted {attempted:?}"
            ),
            PipelineError::AlreadyComplete => f.write_str("pipeline already complete"),
            PipelineError::ValidateRequiresGate => {
                f.write_str("VALIDATE must be passed through ActionPipeline::validate")
            }
            PipelineError::NotValidated => f.write_str("EXECUTE requires a gate verdict"),
            PipelineError::GateBlocked(reasons) => write!(
                f,
                "safety gate blocked execution: {}",
                reasons
                    .iter()
                    .map(|r| r.code())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for PipelineError {}

/// One stage of a completed pipeline, in order (audit trail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRecord {
    pub stage: ActionStage,
}

/// The safety pipeline over one action. Stages are passed strictly in
/// order; the completed-stages list is an append-only audit trail.
pub struct ActionPipeline {
    action: ProposedAction,
    reached: ActionStage,
    verdict: Option<GateVerdict>,
    completed: Vec<StageRecord>,
}

impl ActionPipeline {
    /// The only constructor: every action begins at OBSERVE.
    pub fn observe(action: ProposedAction) -> Self {
        ActionPipeline {
            action,
            reached: ActionStage::Observe,
            verdict: None,
            completed: vec![StageRecord {
                stage: ActionStage::Observe,
            }],
        }
    }

    pub fn action(&self) -> &ProposedAction {
        &self.action
    }

    pub fn reached(&self) -> ActionStage {
        self.reached
    }

    pub fn completed_stages(&self) -> &[StageRecord] {
        &self.completed
    }

    /// Advance exactly one stage. Skipping or repeating is refused.
    /// VALIDATE must be passed through [`ActionPipeline::validate`], and
    /// EXECUTE requires an ALLOWED gate verdict — there is no other path.
    pub fn advance_to(&mut self, to: ActionStage) -> Result<(), PipelineError> {
        if to == ActionStage::Validate {
            return Err(PipelineError::ValidateRequiresGate);
        }
        let expected = self.reached.next().ok_or(PipelineError::AlreadyComplete)?;
        if to != expected {
            return Err(PipelineError::StageSkipped {
                reached: self.reached,
                attempted: to,
            });
        }
        if to == ActionStage::Execute {
            match &self.verdict {
                None => return Err(PipelineError::NotValidated),
                Some(GateVerdict::Blocked(reasons)) => {
                    return Err(PipelineError::GateBlocked(reasons.clone()));
                }
                Some(GateVerdict::Allowed) => {}
            }
        }
        self.reached = to;
        self.completed.push(StageRecord { stage: to });
        Ok(())
    }

    /// Pass the VALIDATE stage through the safety gate. The verdict is
    /// recorded; a BLOCKED verdict permanently bars EXECUTE for this
    /// action. There is no way to clear or replace it.
    pub fn validate(&mut self, gate: &SafetyGate) -> Result<GateVerdict, PipelineError> {
        let expected = self.reached.next().ok_or(PipelineError::AlreadyComplete)?;
        if expected != ActionStage::Validate {
            return Err(PipelineError::StageSkipped {
                reached: self.reached,
                attempted: ActionStage::Validate,
            });
        }
        let verdict = gate.veto(&self.action);
        self.verdict = Some(verdict.clone());
        self.reached = ActionStage::Validate;
        self.completed.push(StageRecord {
            stage: ActionStage::Validate,
        });
        Ok(verdict)
    }
}

impl fmt::Debug for ActionPipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionPipeline")
            .field("action", &self.action.summary())
            .field("reached", &self.reached)
            .field(
                "verdict_allows_execute",
                &self.verdict.as_ref().map(|v| v.is_allowed()),
            )
            .field("stages_passed", &self.completed.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readonly_action() -> ProposedAction {
        ProposedAction::new(
            CapabilityId::StorageAnalysis,
            "observe cache sizes",
            ActionClassification::READ_ONLY,
        )
    }

    fn destructive_action() -> ProposedAction {
        ProposedAction::new(
            CapabilityId::StorageAnalysis,
            "delete a cache file",
            ActionClassification::new(ActionEffect::Destructive, false, false),
        )
    }

    #[test]
    fn stage_order_is_strictly_increasing() {
        for pair in ActionStage::ORDER.windows(2) {
            assert_eq!(pair[0].rank() + 1, pair[1].rank());
            assert_eq!(pair[0].next(), Some(pair[1]));
        }
        assert_eq!(ActionStage::Rollback.next(), None);
    }

    #[test]
    fn read_only_action_walks_every_stage_in_order() {
        let mut pipeline = ActionPipeline::observe(readonly_action());
        let gate = SafetyGate::new();
        for stage in [
            ActionStage::Analyze,
            ActionStage::Recommend,
            ActionStage::Preview,
        ] {
            pipeline.advance_to(stage).expect("next stage advances");
        }
        let verdict = pipeline.validate(&gate).expect("validate at VALIDATE");
        assert!(verdict.is_allowed(), "read-only actions pass the gate");
        for stage in [
            ActionStage::Execute,
            ActionStage::Verify,
            ActionStage::Rollback,
        ] {
            pipeline.advance_to(stage).expect("next stage advances");
        }
        let walked: Vec<ActionStage> = pipeline
            .completed_stages()
            .iter()
            .map(|r| r.stage)
            .collect();
        assert_eq!(walked, ActionStage::ORDER, "exactly the full sequence");
    }

    #[test]
    fn stages_cannot_be_skipped() {
        let mut pipeline = ActionPipeline::observe(readonly_action());
        // Discovery → deletion is the forbidden jump.
        assert_eq!(
            pipeline.advance_to(ActionStage::Execute),
            Err(PipelineError::StageSkipped {
                reached: ActionStage::Observe,
                attempted: ActionStage::Execute,
            })
        );
        pipeline.advance_to(ActionStage::Analyze).unwrap();
        // Skipping ahead and repeating both fail.
        assert_eq!(
            pipeline.advance_to(ActionStage::Preview),
            Err(PipelineError::StageSkipped {
                reached: ActionStage::Analyze,
                attempted: ActionStage::Preview,
            })
        );
        assert!(matches!(
            pipeline.advance_to(ActionStage::Analyze),
            Err(PipelineError::StageSkipped { .. })
        ));
        // VALIDATE is gate-only.
        assert_eq!(
            pipeline.advance_to(ActionStage::Validate),
            Err(PipelineError::ValidateRequiresGate)
        );
        // After completion, nothing follows.
        let mut pipeline = ActionPipeline::observe(readonly_action());
        let gate = SafetyGate::new();
        pipeline.advance_to(ActionStage::Analyze).unwrap();
        pipeline.advance_to(ActionStage::Recommend).unwrap();
        pipeline.advance_to(ActionStage::Preview).unwrap();
        pipeline.validate(&gate).unwrap();
        for stage in [
            ActionStage::Execute,
            ActionStage::Verify,
            ActionStage::Rollback,
        ] {
            pipeline.advance_to(stage).unwrap();
        }
        assert_eq!(
            pipeline.advance_to(ActionStage::Rollback),
            Err(PipelineError::AlreadyComplete)
        );
    }

    #[test]
    fn destructive_actions_can_never_reach_execute() {
        let mut pipeline = ActionPipeline::observe(destructive_action());
        let gate = SafetyGate::new();
        pipeline.advance_to(ActionStage::Analyze).unwrap();
        pipeline.advance_to(ActionStage::Recommend).unwrap();
        pipeline.advance_to(ActionStage::Preview).unwrap();
        let verdict = pipeline.validate(&gate).expect("validate runs");
        assert_eq!(
            verdict.reasons(),
            [BlockReason::ClassNotAuthorized(ActionClass::Destructive)]
        );
        // The forbidden jump is blocked with binding reasons — forever.
        assert_eq!(
            pipeline.advance_to(ActionStage::Execute),
            Err(PipelineError::GateBlocked(vec![
                BlockReason::ClassNotAuthorized(ActionClass::Destructive)
            ]))
        );
    }

    #[test]
    fn reversible_and_qualified_actions_are_blocked_in_this_build() {
        let gate = SafetyGate::new();
        let reversible = ProposedAction::new(
            CapabilityId::SoftwareManagement,
            "move an app's cache to trash",
            ActionClassification::new(ActionEffect::Reversible, false, false),
        );
        assert_eq!(
            gate.veto(&reversible).reasons(),
            [BlockReason::ClassNotAuthorized(ActionClass::Reversible)]
        );

        let privileged_read = ProposedAction::new(
            CapabilityId::StartupItems,
            "read a system LaunchDaemon plist",
            ActionClassification::new(ActionEffect::ReadOnly, true, false),
        );
        assert_eq!(
            gate.veto(&privileged_read).reasons(),
            [BlockReason::PrivilegeNotAuthorized]
        );

        let gated_read = ProposedAction::new(
            CapabilityId::PrivacyHousekeeping,
            "observe TCC-protected residue",
            ActionClassification::new(ActionEffect::ReadOnly, false, true),
        );
        assert_eq!(
            gate.veto(&gated_read).reasons(),
            [BlockReason::PermissionSensitiveNotAuthorized]
        );
    }

    #[test]
    fn classification_is_explicit_and_canonical() {
        let c = ActionClassification::new(ActionEffect::Destructive, true, true);
        assert_eq!(
            c.classes(),
            vec![
                ActionClass::Destructive,
                ActionClass::Privileged,
                ActionClass::PermissionSensitive
            ]
        );
        assert_eq!(
            ActionClassification::READ_ONLY.classes(),
            vec![ActionClass::ReadOnly]
        );
    }

    #[test]
    fn validate_requires_the_validate_stage() {
        let mut pipeline = ActionPipeline::observe(readonly_action());
        let gate = SafetyGate::new();
        assert_eq!(
            pipeline.validate(&gate),
            Err(PipelineError::StageSkipped {
                reached: ActionStage::Observe,
                attempted: ActionStage::Validate,
            })
        );
    }
}
