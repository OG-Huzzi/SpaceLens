//! Resolving a category-level plan against one concrete snapshot.
//!
//! A [`CleanupPlan`] says *what kinds of data* may be moved and *how many
//! bytes* the planner attributes to them. It deliberately carries no path
//! list: planning is category-level, and the planner never touches the disk.
//! Execution, however, moves concrete paths. Something has to turn the
//! category-level proposal into the exact list of files and directories a
//! user reviews — and that list is what the action layer must be held to.
//! This module is that step, and it is **pure**: it takes an already-built
//! [`Snapshot`](coresight_history::Snapshot) and performs no I/O of any kind
//! (the crate's source-scan guard enforces this).
//!
//! ## The resolution rules (each one is a safety rule)
//!
//! 1. **Exact category match only.** An entry becomes a candidate only when
//!    its stored classification decodes (via the workspace's stable category
//!    codes, e.g. `"CACHE"`, `"TEMPORARY_DATA"`, `"LOGS"`) to exactly the
//!    plan item's category. An unrecognized or missing code matches nothing —
//!    it is never guessed into a category, and never into `Unknown`.
//! 2. **Unobservable entries are never targets.** `observation_error: Some(_)`
//!    means the scan could not fully read the entry; it is skipped and counted
//!    in [`ResolvedPlan::unresolved_entries`].
//! 3. **Unknown size is never a target.** A regular-file entry without a
//!    logical size is skipped and counted, because its recoverable bytes
//!    cannot be stated. (In this workspace's snapshot model `size` is only
//!    ever attached to regular files; a directory's own `size` is `None` by
//!    construction and its bytes are derived from its descendants instead —
//!    see rule 6.)
//! 4. **Only regular files and directories.** Links and other entry kinds are
//!    skipped and counted: following, moving, or measuring a link is a
//!    different, unaudited operation.
//! 5. **A directory absorbs its same-category descendants.** All candidates
//!    of one item are sorted by path bytes and the shallowest directories
//!    win: any candidate strictly beneath an already-kept directory is
//!    dropped. A move must never be attempted twice on nested paths. Dropped
//!    descendants are **not** errors and **not** counted as unresolved: the
//!    parent covers them. Entries skipped by rules 2–4 *before* deduplication
//!    are still counted as unresolved even when a parent directory is a
//!    target, because their bytes are excluded from the parent's lower-bound
//!    sum — the count reports entry-level uncertainty, not uncovered paths.
//!    (Deduplication is per item, i.e. per category: a directory in one
//!    category and a descendant entry in another are both listed, because
//!    neither item can silently absorb the other's bytes. The executor
//!    rejects the second move once the first target is gone, and then rolls
//!    back — see the residual gaps in the crate docs.)
//! 6. **Directory bytes are a logical lower bound.** A directory target's
//!    `bytes` is the saturating sum of the logical sizes of regular files
//!    **beneath** it that were fully observed and have a known size. It is
//!    not allocated size, it is not sparse-aware, and it counts hard-linked
//!    content once per path. It can therefore understate (never fabricate)
//!    the space a move touches.
//! 7. **Totals saturate.** [`ResolvedPlan::total_bytes`] is the saturating sum
//!    of every target's bytes; it can never wrap.
//! 8. **Unmapped bytes are reported, never hidden.** `unmapped_bytes` is
//!    `plan.total_recoverable_bytes` minus `total_bytes`, saturating. A
//!    non-zero value is the honest "the plan claimed bytes we could not pin to
//!    a concrete target" number. It saturates at zero only when the resolved
//!    total meets or exceeds the plan's claim (possible for a directory target
//!    whose descendants include other categories' files).
//! 9. **The target budget is a hard limit, not a truncation.** Exceeding
//!    [`ResolveOptions::max_targets`] is
//!    [`ResolveError::TargetBudgetExceeded`] — never a shortened list,
//!    because a shortened list is a plan the user did not review.
//! 10. **No item may vanish.** An item that resolves to no concrete target at
//!     all is [`ResolveError::PlanItemMissing`]: the plan claimed bytes for it
//!     and the preview would otherwise silently omit it. An empty plan is
//!     [`ResolveError::EmptyPlan`].
//! 11. **Deterministic.** Items stay in plan order; each item's targets are
//!     ordered by path bytes ascending. Entries arriving in a different order
//!     cannot change the result.
//!
//! ## What resolution does not prove
//!
//! Resolution proves that *at snapshot time* each target path existed with the
//! stated kind and category. It does not prove that the object at that path is
//! still the same object when it is moved: the snapshot can be stale, and the
//! filesystem can change between resolve and execute. The action layer binds
//! the reviewed **path set**, not filesystem object identity. Likewise, a
//! caller that fabricates a [`ResolvedPlan`] (its fields are data, not proof)
//! is outside what these types can catch.

use std::fmt;
use std::path::{Path, PathBuf};

use coresight_classifier::Category;
use coresight_history::{ObservedKind, Snapshot};
use serde::{Deserialize, Serialize};

use crate::{CleanupPlan, SafetyTier};

/// What a resolved target is.
///
/// The distinction matters to the executor and to byte accounting: a file is
/// moved as one object with its own logical size; a directory is moved as one
/// object whose reported bytes are a lower bound over its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TargetKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
}

/// One concrete thing a plan item authorizes moving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTarget {
    /// The path exactly as observed in the snapshot. It is not canonicalized:
    /// the executor must be held to the same spelling the user reviewed.
    pub path: PathBuf,
    /// Logical size for a file; saturating sum of the logical sizes of the
    /// fully observed regular files beneath a directory (never allocated
    /// size). See rule 6 in the module documentation.
    pub bytes: u64,
    /// Whether this target is a file or a directory.
    pub kind: TargetKind,
}

/// The concrete targets of one plan item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedItem {
    /// The [`CleanupItem::id`](crate::CleanupItem::id) this item came from.
    pub item_id: String,
    /// The item's category, copied from the plan.
    pub category: Category,
    /// The item's safety tier, copied from the plan.
    pub tier: SafetyTier,
    /// The item's targets, ordered by path bytes ascending.
    pub targets: Vec<PlanTarget>,
}

/// A category-level plan resolved against one snapshot.
///
/// This is the value a preview and an execution token must both be built
/// from: it names the concrete files and directories, so "the same plan" can
/// finally mean "the same paths", not merely "the same category totals".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPlan {
    /// Equal to the source [`CleanupPlan::id`].
    pub plan_id: String,
    /// One entry per plan item, in plan order.
    pub items: Vec<ResolvedItem>,
    /// Saturating sum of every target's bytes.
    pub total_bytes: u64,
    /// Number of targets across every item (after directory deduplication).
    pub target_count: usize,
    /// `plan.total_recoverable_bytes` minus `total_bytes`, saturating. The
    /// honest "the plan claimed more than we could pin to concrete targets"
    /// number; it must never be silently zeroed.
    pub unmapped_bytes: u64,
    /// Entries that matched a plan item's category but were skipped as
    /// targets: unobservable entries, entries without a size, links, and
    /// other kinds. Descendants absorbed by a directory target are **not**
    /// counted here.
    pub unresolved_entries: usize,
}

/// Knobs for [`resolve_plan`]. Defaults are conservative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveOptions {
    /// Hard upper bound on the number of targets a resolved plan may contain.
    ///
    /// Exceeding it is an error, never a truncation: a target list the user
    /// did not review is not a plan.
    pub max_targets: usize,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        ResolveOptions {
            // A preview is meant to be read by a human. Ten thousand rows is
            // already far beyond reviewable, and the limit is a fail-closed
            // guard, not a UI pagination policy.
            max_targets: 10_000,
        }
    }
}

/// Why a plan could not be resolved into concrete targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// A plan item produced no concrete target at all.
    ///
    /// The plan claimed bytes for the item, so dropping it would produce a
    /// preview that silently omits something the user was shown. Failing
    /// closed is the only honest outcome.
    PlanItemMissing {
        /// The plan item that could not be pinned to any target.
        item_id: String,
    },
    /// The resolved target count would exceed the caller's hard budget.
    ///
    /// Resolution stops at the first item that crosses the budget, so
    /// `requested` is a lower bound on how many targets the full resolution
    /// would have produced. No partial plan is returned.
    TargetBudgetExceeded {
        /// The budget the caller set.
        max_targets: usize,
        /// The target count reached when the budget was crossed.
        requested: usize,
    },
    /// The plan contains no items; there is nothing to resolve.
    EmptyPlan,
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::PlanItemMissing { item_id } => write!(
                f,
                "plan item {item_id} has no concrete target in the snapshot; \
                 a plan that cannot be previewed as paths must not be executed"
            ),
            ResolveError::TargetBudgetExceeded {
                max_targets,
                requested,
            } => write!(
                f,
                "resolution produced at least {requested} targets, over the budget of \
                 {max_targets}; refusing to truncate a list the user did not review"
            ),
            ResolveError::EmptyPlan => {
                f.write_str("the plan contains no items, so there is nothing to resolve")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Decode a stored classification code into the typed taxonomy.
///
/// History stores the serialized stable code (for example `"CACHE"`); the
/// classifier exposes no public from-code parser. Walking
/// [`Category::ALL`] covers exactly the workspace's stable codes and cannot
/// drift when the taxonomy grows. Unknown codes stay `None` — never guessed
/// into [`Category::Unknown`](coresight_classifier::Category::Unknown).
fn decode_category(code: &str) -> Option<Category> {
    Category::ALL
        .iter()
        .copied()
        .find(|category| category.code() == code)
}

/// A path's bytes, the workspace's canonical ordering key for paths.
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// Saturating sum of the logical sizes of the fully observed regular files
/// strictly beneath `directory`.
///
/// `Path::starts_with` is component-wise: `cache/old` is beneath `cache`, but
/// `cache-extra` is not. Path identity is byte-exact as stored — no case
/// folding or symlink resolution — because the executor is held to the same
/// spelling the snapshot recorded.
fn directory_bytes(snapshot: &Snapshot, directory: &Path) -> u64 {
    let mut total = 0u64;
    for entry in &snapshot.entries {
        if entry.kind != ObservedKind::File || entry.observation_error.is_some() {
            continue;
        }
        if entry.path == directory || !entry.path.starts_with(directory) {
            continue;
        }
        if let Some(size) = entry.size {
            total = total.saturating_add(size);
        }
    }
    total
}

/// One candidate target before directory deduplication and byte derivation.
struct Candidate {
    path: PathBuf,
    kind: TargetKind,
    /// Known logical size for a file; ignored for directories.
    file_bytes: u64,
}

/// Drop every candidate strictly beneath a directory candidate.
///
/// The check is order-independent: a candidate is covered when **any** of its
/// ancestors is a directory candidate, even if a platform's byte ordering
/// would place the descendant first (mixed separators can do that on Windows).
/// Sorting still happens first so the *kept* list is deterministic; it is not
/// the correctness argument.
fn drop_nested_descendants(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let covered: Vec<bool> = {
        let directories: std::collections::BTreeSet<&Path> = candidates
            .iter()
            .filter(|candidate| candidate.kind == TargetKind::Directory)
            .map(|candidate| candidate.path.as_path())
            .collect();
        candidates
            .iter()
            .map(|candidate| {
                candidate
                    .path
                    .ancestors()
                    .skip(1)
                    .any(|ancestor| directories.contains(ancestor))
            })
            .collect()
    };
    candidates
        .into_iter()
        .zip(covered)
        .filter_map(|(candidate, covered)| (!covered).then_some(candidate))
        .collect()
}

/// Resolve `plan` against `snapshot` into concrete targets.
///
/// The result is deterministic: plan items stay in plan order and each item's
/// targets are ordered by path bytes ascending. The function is pure — it
/// reads only the supplied snapshot and performs no I/O, no clock, and no
/// environment access.
///
/// # Errors
///
/// * [`ResolveError::EmptyPlan`] when the plan has no items.
/// * [`ResolveError::PlanItemMissing`] when an item resolves to no target.
/// * [`ResolveError::TargetBudgetExceeded`] when the output would exceed
///   `options.max_targets`.
pub fn resolve_plan(
    plan: &CleanupPlan,
    snapshot: &Snapshot,
    options: &ResolveOptions,
) -> Result<ResolvedPlan, ResolveError> {
    if plan.items.is_empty() {
        return Err(ResolveError::EmptyPlan);
    }

    let mut items: Vec<ResolvedItem> = Vec::with_capacity(plan.items.len());
    let mut total_bytes = 0u64;
    let mut target_count = 0usize;
    let mut unresolved_entries = 0usize;

    for item in &plan.items {
        let mut candidates: Vec<Candidate> = Vec::new();
        for entry in &snapshot.entries {
            // Rule 1 — the entry must decode to exactly this item's category.
            let Some(entry_category) = entry
                .classification
                .as_ref()
                .and_then(|classification| decode_category(&classification.category))
            else {
                continue;
            };
            if entry_category != item.category {
                continue;
            }

            // Rule 2 — an entry we could not fully observe is never a target.
            if entry.observation_error.is_some() {
                unresolved_entries = unresolved_entries.saturating_add(1);
                continue;
            }

            // Rule 4 — only files and directories can be targets.
            let kind = match entry.kind {
                ObservedKind::File => TargetKind::File,
                ObservedKind::Dir => TargetKind::Directory,
                ObservedKind::Link | ObservedKind::Other => {
                    unresolved_entries = unresolved_entries.saturating_add(1);
                    continue;
                }
            };

            // Rule 3 — a file with no known size cannot be planned. A
            // directory's own `size` is `None` by construction in this
            // snapshot model; its bytes come from its descendants (rule 6).
            let file_bytes = match kind {
                TargetKind::File => match entry.size {
                    Some(size) => size,
                    None => {
                        unresolved_entries = unresolved_entries.saturating_add(1);
                        continue;
                    }
                },
                TargetKind::Directory => 0,
            };

            candidates.push(Candidate {
                path: entry.path.clone(),
                kind,
                file_bytes,
            });
        }

        // Rule 11 — deterministic order before deduplication.
        candidates.sort_by(|a, b| path_bytes(&a.path).cmp(path_bytes(&b.path)));
        // Rule 5 — a directory absorbs everything beneath it in this item.
        let kept = drop_nested_descendants(candidates);

        // Rule 10 — an item the plan claimed bytes for must not vanish.
        if kept.is_empty() {
            return Err(ResolveError::PlanItemMissing {
                item_id: item.id.clone(),
            });
        }

        target_count = target_count.saturating_add(kept.len());
        // Rule 9 — hard budget, checked before more work (and never repaired
        // by truncation).
        if target_count > options.max_targets {
            return Err(ResolveError::TargetBudgetExceeded {
                max_targets: options.max_targets,
                requested: target_count,
            });
        }

        let mut targets: Vec<PlanTarget> = Vec::with_capacity(kept.len());
        for candidate in kept {
            let bytes = match candidate.kind {
                TargetKind::File => candidate.file_bytes,
                TargetKind::Directory => directory_bytes(snapshot, &candidate.path),
            };
            // Rule 7 — every addition saturates.
            total_bytes = total_bytes.saturating_add(bytes);
            targets.push(PlanTarget {
                path: candidate.path,
                bytes,
                kind: candidate.kind,
            });
        }

        items.push(ResolvedItem {
            item_id: item.id.clone(),
            category: item.category,
            tier: item.tier,
            targets,
        });
    }

    Ok(ResolvedPlan {
        plan_id: plan.id.clone(),
        items,
        total_bytes,
        target_count,
        // Rule 8 — the honest gap between the claim and the pinning.
        unmapped_bytes: plan.total_recoverable_bytes.saturating_sub(total_bytes),
        unresolved_entries,
    })
}
