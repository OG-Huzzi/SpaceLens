//! The authorization token: the only path from a plan to an execution.
//!
//! ## Why a token instead of a boolean
//!
//! A `bool` can be computed anywhere, passed by anyone, and defaults to
//! `false` only until somebody flips it. [`AuthorizedPlan`] cannot be built at
//! all except through [`authorize()`], its fields are private, and it exposes no
//! mutable accessor — so "this plan passed the safety validator" is a fact
//! carried by the type, not a convention carried by a call site.
//!
//! ## What the token proves, and what it does not
//!
//! It proves that [`validate_with_policy`]
//! returned `Allowed` for this exact plan and policy, and that every item's
//! action is [`ActionKind::MoveToTrash`](coresight_cleanup::ActionKind) — the
//! only action this crate can perform reversibly. It does **not** prove that
//! a human looked at the preview: that is the job of [`ConfirmedPreview`],
//! which is a separate, explicit value the caller must obtain from the preview
//! it rendered. Neither type can prove a human was involved; both make the
//! involvement impossible to forget by accident.

use coresight_capabilities::safety::ExecutionPolicy;
use coresight_cleanup::{
    validate_with_policy, ActionKind, BlockReason, CleanupPlan, ValidationVerdict,
};

/// The evidence that a plan passed the safety validator.
///
/// Carried inside [`AuthorizedPlan`]; exposed for audit and for a UI that
/// wants to show which policy admitted the plan. It cannot be constructed
/// outside this module (private fields, no constructor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    plan_id: String,
    total_bytes: u64,
    item_count: usize,
    policy: ExecutionPolicy,
}

impl Authorization {
    /// The id of the plan this token was minted for.
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    /// The plan's total recoverable bytes at authorization time.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// How many items the plan carried.
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// The policy the plan was validated against.
    ///
    /// A policy is a build-time floor: widening it is a deliberate,
    /// separately reviewed change. Recording it here means a report can say
    /// *which* floor admitted an execution.
    pub fn policy(&self) -> &ExecutionPolicy {
        &self.policy
    }
}

/// A plan that may be executed, and the proof that it may.
///
/// The only constructor is [`authorize()`]. The plan is reachable only by shared
/// reference, so it cannot be edited between authorization and execution and
/// the token's verdict cannot go stale.
#[derive(Debug, Clone)]
pub struct AuthorizedPlan {
    plan: CleanupPlan,
    authorization: Authorization,
}

impl AuthorizedPlan {
    /// The authorized plan, for previewing and for reporting.
    pub fn plan(&self) -> &CleanupPlan {
        &self.plan
    }

    /// The id a [`ConfirmedPreview`] must match.
    pub fn plan_id(&self) -> &str {
        &self.authorization.plan_id
    }

    /// The byte total a [`ConfirmedPreview`] must match.
    ///
    /// This is the number the preview showed (`CleanupPreview::total_bytes` is
    /// copied from it), so a confirmation that saw a different number cannot
    /// silently authorize this plan.
    pub fn total_recoverable_bytes(&self) -> u64 {
        self.plan.total_recoverable_bytes
    }

    /// The token itself, for audit surfaces.
    pub fn authorization(&self) -> &Authorization {
        &self.authorization
    }

    /// The actions this plan authorizes.
    ///
    /// Always exactly one value, [`ActionKind::MoveToTrash`]: [`authorize()`]
    /// refuses every other kind. It is exposed so an executor or UI can assert
    /// the invariant without reaching into the plan's items.
    pub fn authorized_action(&self) -> ActionKind {
        ActionKind::MoveToTrash
    }
}

/// The user's confirmation of the preview they were shown.
///
/// # Why this type exists
///
/// [`crate::execute()`] will not run without one, and the only way to make one
/// that matches is to know the plan's id and its byte total. The intended
/// construction is from the preview the UI actually rendered:
///
/// ```
/// # use coresight_cleanup::{CleanupPreview, CleanupPlan};
/// # fn demo(plan: &CleanupPlan) -> coresight_actions::ConfirmedPreview {
/// let preview = CleanupPreview::from_plan(plan);
/// coresight_actions::ConfirmedPreview::new(preview.plan_id, preview.total_bytes)
/// # }
/// ```
///
/// # What it cannot do
///
/// No type can prove that a human read the preview; a caller can construct
/// this value from the same plan it is about to execute. What the type *does*
/// guarantee is that the confirmation and the plan are the same plan and the
/// same numbers: [`crate::ExecuteError::NotConfirmed`] is returned otherwise.
/// The discipline that a confirmation is only built from a rendered preview is
/// a UI-layer rule backed by this type, not a theorem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPreview {
    plan_id: String,
    total_bytes: u64,
}

impl ConfirmedPreview {
    /// Record that the preview for `plan_id` showing `total_bytes` was
    /// confirmed.
    pub fn new(plan_id: impl Into<String>, total_bytes: u64) -> Self {
        ConfirmedPreview {
            plan_id: plan_id.into(),
            total_bytes,
        }
    }

    /// The plan id that was shown.
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    /// The byte total that was shown.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// Why a plan could not be authorized.
#[derive(Debug)]
pub enum AuthorizeError {
    /// The safety validator blocked the plan. The reasons are the
    /// validator's own and are binding: the plan can never reach execution.
    Blocked {
        /// Every reason the validator gave, in its order.
        reasons: Vec<BlockReason>,
    },
    /// The plan contains an action this crate cannot perform reversibly.
    ///
    /// Reached only when the policy would have allowed the action (a policy
    /// that forbids it is stopped earlier, as [`AuthorizeError::Blocked`]).
    /// This crate has no deletion, wipe, registry, or subprocess primitive, so
    /// minting a token for such a plan would authorize a lie.
    UnsupportedAction {
        /// The offending plan item.
        item_id: String,
        /// The action it declared.
        action: ActionKind,
    },
    /// The plan contains no items: a token for it would authorize nothing,
    /// and pretending otherwise invites a caller to treat "authorized" as
    /// "there is work to do".
    EmptyPlan,
}

impl std::fmt::Display for AuthorizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthorizeError::Blocked { reasons } => {
                f.write_str("the plan is blocked: ")?;
                let mut first = true;
                for reason in reasons {
                    if !first {
                        f.write_str("; ")?;
                    }
                    first = false;
                    write!(f, "{}: {}", reason.code, reason.detail)?;
                }
                Ok(())
            }
            AuthorizeError::UnsupportedAction { item_id, action } => write!(
                f,
                "item {item_id} declares {action:?}, which this crate cannot perform reversibly"
            ),
            AuthorizeError::EmptyPlan => {
                f.write_str("the plan contains no items, so there is nothing to authorize")
            }
        }
    }
}

impl std::error::Error for AuthorizeError {}

/// Validate `plan` under `policy` and mint the execution token.
///
/// The order of the checks is deliberate:
///
/// 1. An empty plan is refused first — there is nothing to authorize.
/// 2. The safety validator runs on the plan exactly as supplied. A blocked
///    verdict becomes [`AuthorizeError::Blocked`] with the validator's reasons,
///    unchanged, so the UI can show the user why.
/// 3. Only then is each item's action checked against the one action this
///    crate can honestly execute. A `Delete` item that a permissive policy
///    would allow is still refused here, because the executor has no deletion
///    primitive and a token must never claim otherwise.
pub fn authorize(
    plan: CleanupPlan,
    policy: ExecutionPolicy,
) -> Result<AuthorizedPlan, AuthorizeError> {
    if plan.items.is_empty() {
        return Err(AuthorizeError::EmptyPlan);
    }

    if let ValidationVerdict::Blocked(reasons) = validate_with_policy(&plan, policy).verdict {
        return Err(AuthorizeError::Blocked { reasons });
    }

    for item in &plan.items {
        if item.action != ActionKind::MoveToTrash {
            return Err(AuthorizeError::UnsupportedAction {
                item_id: item.id.clone(),
                action: item.action,
            });
        }
    }

    let authorization = Authorization {
        plan_id: plan.id.clone(),
        total_bytes: plan.total_recoverable_bytes,
        item_count: plan.items.len(),
        policy,
    };
    Ok(AuthorizedPlan {
        plan,
        authorization,
    })
}
