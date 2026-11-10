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
//! It proves three things about one pair of values:
//!
//! 1. **[`validate_with_policy`] returned `Allowed`** for this exact plan and
//!    policy, and every item's action is
//!    [`ActionKind::MoveToTrash`](coresight_cleanup::ActionKind) — the only
//!    action this crate can perform reversibly.
//! 2. **The resolved plan describes this plan, not a neighbour.** The plan id,
//!    the item count, and every item's id, category, and tier must agree, and
//!    the resolved plan's declared `target_count` / `total_bytes` must equal
//!    what its target lists actually contain. A caller cannot pair a small
//!    plan with a larger resolved path set.
//! 3. **The reviewed path set is frozen here.** [`AuthorizedPlan`] owns the
//!    flattened target paths and exposes them only immutably; [`execute`]
//!    refuses any target outside that set.
//!
//! It does **not** prove that a human looked at the preview: that is the job
//! of [`ConfirmedPreview`], which is a separate, explicit value the caller must
//! obtain from the preview it rendered. Neither type can prove a human was
//! involved; both make the involvement impossible to forget by accident.
//!
//! Nor can these types defend against an in-process caller that fabricates a
//! [`ResolvedPlan`]: its fields are plain data, not a proof of resolution. The
//! guarantee is that an honest caller who resolves a plan through
//! [`resolve_plan`](coresight_cleanup::resolve_plan) cannot then widen the
//! target list; it is not a sandbox against code that lies. Resolution is also
//! only as fresh as its snapshot: the token binds reviewed **paths**, not the
//! filesystem objects currently at them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use coresight_capabilities::safety::ExecutionPolicy;
use coresight_cleanup::{
    validate_with_policy, ActionKind, BlockReason, CleanupPlan, ResolvedPlan, ValidationVerdict,
};

/// The evidence that a plan passed the safety validator and was resolved.
///
/// Carried inside [`AuthorizedPlan`]; exposed for audit and for a UI that
/// wants to show which policy admitted the plan. It cannot be constructed
/// outside this module (private fields, no constructor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    plan_id: String,
    total_bytes: u64,
    item_count: usize,
    resolved_target_count: usize,
    resolved_total_bytes: u64,
    policy: ExecutionPolicy,
}

impl Authorization {
    /// The id of the plan this token was minted for.
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    /// The plan's **claimed** total recoverable bytes at authorization time.
    ///
    /// This is the planner's category-level claim, not the resolved byte
    /// total; compare [`Self::resolved_total_bytes`] with it (the difference
    /// is [`ResolvedPlan::unmapped_bytes`]) rather than treating it as what
    /// will move.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// How many items the plan carried.
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// How many concrete targets the resolved plan contained.
    pub fn resolved_target_count(&self) -> usize {
        self.resolved_target_count
    }

    /// The resolved plan's saturating byte total.
    pub fn resolved_total_bytes(&self) -> u64 {
        self.resolved_total_bytes
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
/// The only constructor is [`authorize()`]. The plan and its resolved targets
/// are reachable only by shared reference, so they cannot be edited between
/// authorization and execution and the token's verdict cannot go stale.
#[derive(Debug, Clone)]
pub struct AuthorizedPlan {
    plan: CleanupPlan,
    resolved: ResolvedPlan,
    authorized_paths: BTreeSet<PathBuf>,
    authorization: Authorization,
}

impl AuthorizedPlan {
    /// The authorized plan, for previewing and for reporting.
    pub fn plan(&self) -> &CleanupPlan {
        &self.plan
    }

    /// The resolved plan the token was minted from: the concrete targets the
    /// user reviewed.
    pub fn resolved(&self) -> &ResolvedPlan {
        &self.resolved
    }

    /// The id a [`ConfirmedPreview`] must match.
    pub fn plan_id(&self) -> &str {
        &self.authorization.plan_id
    }

    /// The plan's claimed byte total (`CleanupPlan::total_recoverable_bytes`).
    ///
    /// Informational: confirmation binds [`Self::resolved_total_bytes`], the
    /// number actually pinned to targets.
    pub fn total_recoverable_bytes(&self) -> u64 {
        self.plan.total_recoverable_bytes
    }

    /// The resolved plan's saturating byte total, which a [`ConfirmedPreview`]
    /// must match.
    pub fn resolved_total_bytes(&self) -> u64 {
        self.resolved.total_bytes
    }

    /// The number of concrete targets, which a [`ConfirmedPreview`] must
    /// match. A confirmation rendered for three files cannot authorize nine.
    pub fn resolved_target_count(&self) -> usize {
        self.resolved.target_count
    }

    /// Every target path this token authorizes, and nothing else.
    ///
    /// Returned as an iterator over shared paths so a caller can display or
    /// audit the set but can never widen it. Membership is byte-exact on the
    /// resolved path: no canonicalization, prefix matching, or case folding is
    /// applied, because those would let a spelling the user never saw slip
    /// through.
    pub fn authorized_paths(&self) -> impl Iterator<Item = &Path> {
        self.authorized_paths.iter().map(|path| path.as_path())
    }

    /// Whether `path` is one of the resolved targets frozen into this token.
    ///
    /// Byte-exact on the path as resolved; see [`Self::authorized_paths`].
    pub fn is_path_authorized(&self, path: &Path) -> bool {
        self.authorized_paths.contains(path)
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
/// that matches is to know the resolved plan's id, target count, and byte
/// total. The intended construction is from the resolved plan the UI actually
/// rendered:
///
/// ```
/// # use coresight_cleanup::ResolvedPlan;
/// # fn demo(resolved: &ResolvedPlan) -> coresight_actions::ConfirmedPreview {
/// coresight_actions::ConfirmedPreview::from_resolved(resolved)
/// # }
/// ```
///
/// # What it cannot do
///
/// No type can prove that a human read the preview; a caller can construct
/// this value from the same resolved plan it is about to execute. What the
/// type *does* guarantee is that the confirmation and the execution are the
/// same plan and the same numbers: [`crate::ExecuteError::NotConfirmed`] is
/// returned otherwise. The discipline that a confirmation is only built from a
/// rendered preview is a UI-layer rule backed by this type, not a theorem. The
/// `target_count` binding additionally means a confirmation rendered for a
/// three-file preview cannot authorize a nine-target execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPreview {
    plan_id: String,
    total_bytes: u64,
    target_count: usize,
}

impl ConfirmedPreview {
    /// Record that the preview for `plan_id` showing `total_bytes` across
    /// `target_count` targets was confirmed.
    ///
    /// `total_bytes` and `target_count` are the **resolved** plan's numbers
    /// ([`ResolvedPlan::total_bytes`] and [`ResolvedPlan::target_count`]), not
    /// the category-level plan's claimed total. [`Self::from_resolved`] is the
    /// safer constructor when the resolved plan is at hand.
    pub fn new(plan_id: impl Into<String>, total_bytes: u64, target_count: usize) -> Self {
        ConfirmedPreview {
            plan_id: plan_id.into(),
            total_bytes,
            target_count,
        }
    }

    /// Record that the preview of `resolved` was confirmed.
    ///
    /// All three fields come from the same value, so the confirmation cannot
    /// accidentally mix one plan's id with another plan's numbers.
    pub fn from_resolved(resolved: &ResolvedPlan) -> Self {
        ConfirmedPreview {
            plan_id: resolved.plan_id.clone(),
            total_bytes: resolved.total_bytes,
            target_count: resolved.target_count,
        }
    }

    /// The plan id that was shown.
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    /// The resolved byte total that was shown.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// The number of targets that was shown.
    pub fn target_count(&self) -> usize {
        self.target_count
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
    /// The resolved plan does not describe this plan, or its declared totals
    /// do not match its own target lists.
    ///
    /// A token must never freeze a path set the plan does not account for, so
    /// this is refused before the validator runs and before anything else is
    /// considered.
    ResolvedPlanMismatch {
        /// What disagreed, in one sentence.
        detail: String,
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
            AuthorizeError::ResolvedPlanMismatch { detail } => {
                write!(f, "the resolved plan does not describe this plan: {detail}")
            }
            AuthorizeError::EmptyPlan => {
                f.write_str("the plan contains no items, so there is nothing to authorize")
            }
        }
    }
}

impl std::error::Error for AuthorizeError {}

/// Check that `resolved` is the resolution of `plan`, and flatten its targets.
///
/// This is deliberately stricter than "same plan id". The id is derived from
/// category totals, so it is a useful first check but not a proof: the item
/// list must match one-for-one (id, category, tier), every item must carry at
/// least one target, no path may repeat, and the declared `target_count`,
/// `total_bytes`, and `unmapped_bytes` must equal what the target lists and
/// the plan's claim actually imply. Without that last check a caller could
/// hand over a resolved plan that *claims* three targets while carrying nine,
/// and the confirmation binding would compare against the lie.
fn verify_resolved_plan(
    plan: &CleanupPlan,
    resolved: &ResolvedPlan,
) -> Result<BTreeSet<PathBuf>, AuthorizeError> {
    if resolved.plan_id != plan.id {
        return Err(AuthorizeError::ResolvedPlanMismatch {
            detail: format!(
                "the resolved plan names {} but the plan is {}",
                resolved.plan_id, plan.id
            ),
        });
    }
    if resolved.items.len() != plan.items.len() {
        return Err(AuthorizeError::ResolvedPlanMismatch {
            detail: format!(
                "the plan carries {} items but the resolved plan carries {}",
                plan.items.len(),
                resolved.items.len()
            ),
        });
    }

    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut target_count = 0usize;
    let mut total_bytes = 0u64;
    for (index, (item, resolved_item)) in plan.items.iter().zip(resolved.items.iter()).enumerate() {
        if resolved_item.item_id != item.id {
            return Err(AuthorizeError::ResolvedPlanMismatch {
                detail: format!(
                    "resolved item {index} is {} but the plan's item is {}",
                    resolved_item.item_id, item.id
                ),
            });
        }
        if resolved_item.category != item.category {
            return Err(AuthorizeError::ResolvedPlanMismatch {
                detail: format!(
                    "resolved item {} carries category {} but the plan says {}",
                    item.id,
                    resolved_item.category.code(),
                    item.category.code()
                ),
            });
        }
        if resolved_item.tier != item.tier {
            return Err(AuthorizeError::ResolvedPlanMismatch {
                detail: format!(
                    "resolved item {} carries tier {:?} but the plan says {:?}",
                    item.id, resolved_item.tier, item.tier
                ),
            });
        }
        if resolved_item.targets.is_empty() {
            return Err(AuthorizeError::ResolvedPlanMismatch {
                detail: format!(
                    "resolved item {} has no targets, so the plan item was never resolved",
                    item.id
                ),
            });
        }
        for target in &resolved_item.targets {
            if !paths.insert(target.path.clone()) {
                return Err(AuthorizeError::ResolvedPlanMismatch {
                    detail: format!(
                        "path {} appears more than once in the resolved plan",
                        target.path.display()
                    ),
                });
            }
            target_count = target_count.saturating_add(1);
            total_bytes = total_bytes.saturating_add(target.bytes);
        }
    }

    if resolved.target_count != target_count {
        return Err(AuthorizeError::ResolvedPlanMismatch {
            detail: format!(
                "the resolved plan declares {} targets but its items contain {}",
                resolved.target_count, target_count
            ),
        });
    }
    if resolved.total_bytes != total_bytes {
        return Err(AuthorizeError::ResolvedPlanMismatch {
            detail: format!(
                "the resolved plan declares {} bytes but its targets total {}",
                resolved.total_bytes, total_bytes
            ),
        });
    }
    let expected_unmapped = plan
        .total_recoverable_bytes
        .saturating_sub(resolved.total_bytes);
    if resolved.unmapped_bytes != expected_unmapped {
        return Err(AuthorizeError::ResolvedPlanMismatch {
            detail: format!(
                "the resolved plan declares {} unmapped bytes but the plan claim and resolved \
                 total imply {}",
                resolved.unmapped_bytes, expected_unmapped
            ),
        });
    }

    Ok(paths)
}

/// Validate `plan` under `policy` and mint the execution token for exactly the
/// targets described by `resolved`.
///
/// The order of the checks is deliberate:
///
/// 1. An empty plan is refused first — there is nothing to authorize.
/// 2. The resolved plan must describe this plan structurally (see
///    [`verify_resolved_plan`]). A token must never freeze a path set that the
///    plan does not account for.
/// 3. The safety validator runs on the plan exactly as supplied. A blocked
///    verdict becomes [`AuthorizeError::Blocked`] with the validator's reasons,
///    unchanged, so the UI can show the user why.
/// 4. Only then is each item's action checked against the one action this
///    crate can honestly execute. A `Delete` item that a permissive policy
///    would allow is still refused here, because the executor has no deletion
///    primitive and a token must never claim otherwise.
pub fn authorize(
    plan: CleanupPlan,
    resolved: ResolvedPlan,
    policy: ExecutionPolicy,
) -> Result<AuthorizedPlan, AuthorizeError> {
    if plan.items.is_empty() {
        return Err(AuthorizeError::EmptyPlan);
    }

    let authorized_paths = verify_resolved_plan(&plan, &resolved)?;

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
        resolved_target_count: resolved.target_count,
        resolved_total_bytes: resolved.total_bytes,
        policy,
    };
    Ok(AuthorizedPlan {
        plan,
        resolved,
        authorized_paths,
        authorization,
    })
}
