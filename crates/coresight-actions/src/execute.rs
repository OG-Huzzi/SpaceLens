//! The executor: the one function that moves a user's files.
//!
//! ## The contract it enforces
//!
//! * An [`AuthorizedPlan`] and a matching [`ConfirmedPreview`] are required
//!   before anything is touched. The confirmation must name the same plan id
//!   and the same byte total, or [`ExecuteError::NotConfirmed`] is returned
//!   and no path is inspected.
//! * An empty target list is [`ExecuteError::NothingToDo`]; a repeated target
//!   is [`ExecuteError::DuplicateTarget`], refused before the first move so a
//!   caller cannot make the executor fail against itself.
//! * Every successful move — main file **and** each sidecar — is recorded as a
//!   [`MovedFile`]. On the first failure every record is restored in reverse
//!   order. All restored → [`ExecuteError::RolledBack`]; any restore failed →
//!   [`ExecuteError::PartialRollback`], which names every file that is still
//!   out of place.
//!
//! ## What `targets` is (and is not)
//!
//! [`CleanupPlan`](coresight_cleanup::CleanupPlan) is category-level: it
//! carries action kinds, tiers, reasoning, and byte totals, but it has **no
//! path list**. `targets` are therefore the *caller-resolved concrete paths
//! for the plan's items* — the files and directories the user saw in the
//! preview. The executor cannot widen the plan (it has no way to invent a
//! path), but it also cannot prove a given target belongs to it; there is no
//! membership test to perform because the plan contains no paths. The token
//! authorizes the plan's *action kinds and tiers* — always exactly
//! [`ActionKind::MoveToTrash`](coresight_cleanup::ActionKind) — not a set of
//! paths. Path resolution and the honesty of the mapping from plan items to
//! concrete paths are the caller's responsibility, and the preview
//! confirmation binds the plan's numbers, not the target list.
//!
//! ## Ordering
//!
//! Targets are moved in the order given, so a caller that wants the most
//! valuable file moved first (and therefore most likely to be present in a
//! report) can say so. Rollback reverses that order.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::authorize::{AuthorizedPlan, ConfirmedPreview};
use crate::rollback::{restore_all, RollbackFailure};
use crate::trash::{
    destination_parts, ensure_source_exists, relocate_with_sidecars, MovedFile, TrashDestination,
};

/// What actually happened, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReport {
    /// Every entry moved, main files and sidecars alike. Empty only when
    /// nothing was moved (which cannot happen on `Ok`: an empty target list is
    /// refused).
    pub moved: Vec<MovedFile>,
    /// Sum of the sizes observed at the sources immediately before each move.
    ///
    /// This is a *source-side* number: it says how many bytes left their
    /// original paths, not how many bytes the filesystem has reclaimed (a
    /// file in a trash directory still occupies space). Present it with
    /// [`crate::verify()`]'s `verified_bytes`, never instead of it.
    pub freed_bytes: u64,
}

/// Why an execution did not produce a report.
#[derive(Debug)]
pub enum ExecuteError {
    /// The confirmation does not belong to this plan, or its byte total
    /// disagrees with the plan's. Nothing was touched.
    NotConfirmed {
        /// What disagreed, in one sentence.
        detail: String,
    },
    /// No targets were supplied. Nothing was touched.
    NothingToDo,
    /// The same target appears more than once. Nothing was touched.
    ///
    /// The second attempt would fail after the first had moved the entry (the
    /// source is gone), turning a caller's bookkeeping mistake into a
    /// rollback. Refusing up front is the cheaper, clearer failure.
    DuplicateTarget {
        /// The repeated path.
        target: PathBuf,
    },
    /// A move failed and **every** entry moved so far was restored.
    ///
    /// `restored` is the exact number of entries put back (main files and
    /// sidecars); zero means the failure happened before anything moved.
    RolledBack {
        /// The target whose move failed.
        failed: PathBuf,
        /// The original failure's description.
        detail: String,
        /// How many entries were restored.
        restored: usize,
    },
    /// A move failed and **at least one** restore failed. This is the honest
    /// worst case: the set is split, and `unrestored` names every file that is
    /// not back where it came from.
    PartialRollback {
        /// How many entries were restored successfully.
        restored: usize,
        /// Every entry that is still out of place, with its current path.
        unrestored: Vec<RollbackFailure>,
    },
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecuteError::NotConfirmed { detail } => {
                write!(
                    f,
                    "the preview confirmation does not match the plan: {detail}"
                )
            }
            ExecuteError::NothingToDo => f.write_str("no targets were supplied"),
            ExecuteError::DuplicateTarget { target } => {
                write!(f, "target {} appears more than once", target.display())
            }
            ExecuteError::RolledBack {
                failed,
                detail,
                restored,
            } => write!(
                f,
                "moving {} failed ({detail}); {restored} moved entr{} restored, nothing is out \
                 of place",
                failed.display(),
                if *restored == 1 { "y was" } else { "ies were" }
            ),
            ExecuteError::PartialRollback {
                restored,
                unrestored,
            } => {
                write!(
                    f,
                    "a move failed and the rollback was incomplete: {restored} entr{} restored, \
                     {} not restored (",
                    if *restored == 1 { "y was" } else { "ies were" },
                    unrestored.len()
                )?;
                for failure in unrestored {
                    write!(
                        f,
                        "{} is still at {} ({})",
                        failure.original.display(),
                        failure.quarantined.display(),
                        failure.detail
                    )?;
                }
                f.write_str(")")
            }
        }
    }
}

impl std::error::Error for ExecuteError {}

/// Move every target, rolling back the whole set on the first failure.
///
/// # Preconditions
///
/// * `confirmed` must name the same plan id and byte total as `authorized`.
/// * `targets` must be non-empty and contain no duplicates.
/// * Each target must exist and be movable into the destination the resolver
///   names (same volume, permissions).
///
/// # Postconditions on `Ok`
///
/// Every target, and every existing `<name>-wal` / `<name>-shm` sidecar, is at
/// its destination; nothing is at its source. The report lists them all. This
/// says nothing about durability: no `fsync` is issued and the rename is not
/// flushed to stable storage, because a trash move is by definition undoable
/// from the trash directory.
///
/// # Postconditions on `Err`
///
/// Either every entry this call moved is back at its source
/// ([`ExecuteError::RolledBack`]) or the error names the entries that are not
/// ([`ExecuteError::PartialRollback`]). There is no third state and no
/// `Ok` result that hides one.
pub fn execute(
    authorized: &AuthorizedPlan,
    confirmed: &ConfirmedPreview,
    destination: &dyn TrashDestination,
    targets: &[PathBuf],
) -> Result<ExecutionReport, ExecuteError> {
    if confirmed.plan_id() != authorized.plan_id() {
        return Err(ExecuteError::NotConfirmed {
            detail: format!(
                "the confirmation names plan {} but the token authorizes {}",
                confirmed.plan_id(),
                authorized.plan_id()
            ),
        });
    }
    if confirmed.total_bytes() != authorized.total_recoverable_bytes() {
        return Err(ExecuteError::NotConfirmed {
            detail: format!(
                "the confirmation shows {} bytes but the plan totals {}",
                confirmed.total_bytes(),
                authorized.total_recoverable_bytes()
            ),
        });
    }
    if targets.is_empty() {
        return Err(ExecuteError::NothingToDo);
    }

    let mut seen: BTreeSet<&Path> = BTreeSet::new();
    for target in targets {
        if !seen.insert(target.as_path()) {
            return Err(ExecuteError::DuplicateTarget {
                target: target.clone(),
            });
        }
    }

    let mut moved: Vec<MovedFile> = Vec::new();
    for target in targets {
        // Inspect the target before asking for a destination: a target that
        // cannot be read must not create a trash directory as a side effect.
        if let Err(e) = ensure_source_exists(target) {
            return Err(roll_back(&moved, target, e.to_string()));
        }
        let preferred = match destination.destination_for(target) {
            Ok(preferred) => preferred,
            Err(e) => {
                return Err(roll_back(&moved, target, e.to_string()));
            }
        };
        let (directory, name) = match destination_parts(&preferred) {
            Ok(parts) => parts,
            Err(detail) => return Err(roll_back(&moved, target, detail)),
        };
        if let Err(e) = relocate_with_sidecars(target, &directory, &name, &mut moved) {
            return Err(roll_back(&moved, target, e.to_string()));
        }
    }

    let freed_bytes = moved
        .iter()
        .fold(0u64, |total, entry| total.saturating_add(entry.bytes));
    Ok(ExecutionReport { moved, freed_bytes })
}

/// Restore everything and turn the failure into the honest error for what
/// actually happened.
fn roll_back(moved: &[MovedFile], failed: &Path, detail: String) -> ExecuteError {
    let outcome = restore_all(moved);
    if outcome.is_complete() {
        ExecuteError::RolledBack {
            failed: failed.to_path_buf(),
            detail,
            restored: outcome.restored,
        }
    } else {
        ExecuteError::PartialRollback {
            restored: outcome.restored,
            unrestored: outcome.failures,
        }
    }
}
