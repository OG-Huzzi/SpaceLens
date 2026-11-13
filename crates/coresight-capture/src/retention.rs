//! Retention: which captures to keep, and which ones to *propose* for
//! deletion.
//!
//! Retention is the one place a screenshot tool can quietly destroy user
//! data, so the rules are conservative and explicit:
//!
//! * **`keep_last == 0` means keep everything.** Zero is the documented
//!   "unbounded" sentinel, not "delete all". A zero that silently deleted
//!   every capture would be a catastrophic misreading of "keep 0", so it is
//!   special-cased, tested, and explained in the plan note. `max_bytes` is
//!   deliberately ignored in that mode for the same reason.
//! * **The keep window is a promise.** The `keep_last` newest captures are
//!   never proposed for deletion, not even to satisfy a byte budget.
//! * **Bytes only trim older captures.** `max_bytes` may retain *additional*
//!   older captures while they fit, and stops when the next one would not.
//!   An unknown size stops the walk: the crate will not guess that an
//!   unmeasured file fits.
//! * **Unknown size taints the total.** `total_bytes` is `None` when any
//!   *retained* entry has no recorded size, rather than silently summing the
//!   known ones as if they were all.
//! * **Nothing is deleted here.** `delete_candidates` is a proposal; the
//!   shell deletes only after the user confirms, and this crate has no
//!   filesystem access with which it could delete anything.
//! * **Deterministic.** Newest first by `created_order`, ties broken by path,
//!   so any permutation of the input produces the same plan.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One capture as the retention planner sees it.
///
/// The planner never stats a file: the size and the creation order are facts
/// the caller recorded when the capture was taken. `bytes: None` means "not
/// measured", which is treated as unknown rather than zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEntry {
    /// Where the capture lives.
    pub path: PathBuf,
    /// Size in bytes, or `None` when it was never measured.
    pub bytes: Option<u64>,
    /// Monotonic capture order; larger is newer. Supplied by the caller so
    /// the plan does not depend on file timestamps or the wall clock.
    pub created_order: u64,
}

/// How many captures to keep, and how much space they may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicy {
    /// Number of most-recent captures to keep. **`0` means keep everything**
    /// — it is a sentinel for unbounded retention, never "keep none".
    pub keep_last: usize,
    /// Optional byte budget. It only affects captures older than the
    /// `keep_last` window, so it can never delete inside that window; it is
    /// ignored entirely when `keep_last` is `0`.
    pub max_bytes: Option<u64>,
}

impl Default for RetentionPolicy {
    /// The shipped default: keep the 100 newest captures, no byte cap.
    ///
    /// 100 is enough for normal use, small enough that a forgotten folder
    /// does not grow without bound, and expressed as a count rather than a
    /// size so the promise ("your last 100 shots are still there") is easy to
    /// state.
    fn default() -> Self {
        RetentionPolicy {
            keep_last: 100,
            max_bytes: None,
        }
    }
}

/// A reviewable retention proposal.
///
/// `keep` and `delete_candidates` are disjoint and together cover every entry
/// that was passed in; `delete_candidates` is *only* a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPlan {
    /// Captures to retain, newest first.
    pub keep: Vec<PathBuf>,
    /// Captures proposed for deletion, newest first. Nothing has been
    /// deleted: the shell must confirm with the user first.
    pub delete_candidates: Vec<PathBuf>,
    /// Total size of `keep`, or `None` when any retained size is unknown.
    pub total_bytes: Option<u64>,
    /// Plain-language explanation of the decision, safe to show verbatim.
    pub note: String,
}

/// Plan which captures to keep and which to propose for deletion.
///
/// Ordering is newest first (`created_order` descending) with the path as a
/// tie-break, so the result is identical for any permutation of `captures`.
/// The function reads no clock and no filesystem; sizes and order are inputs.
pub fn plan_retention(captures: &[CaptureEntry], policy: &RetentionPolicy) -> RetentionPlan {
    if captures.is_empty() {
        return RetentionPlan {
            keep: Vec::new(),
            delete_candidates: Vec::new(),
            total_bytes: Some(0),
            note: "No captures are recorded, so there is nothing to keep and nothing to propose \
                   for deletion. This plan deletes nothing."
                .to_string(),
        };
    }

    let mut ordered: Vec<&CaptureEntry> = captures.iter().collect();
    ordered.sort_by(|left, right| {
        right
            .created_order
            .cmp(&left.created_order)
            .then_with(|| left.path.cmp(&right.path))
    });

    let total = ordered.len();
    let mut keep_count = if policy.keep_last == 0 {
        total
    } else {
        policy.keep_last.min(total)
    };

    let mut budget_note: Option<String> = None;
    if policy.keep_last > 0 {
        if let Some(limit) = policy.max_bytes {
            let mut retained_bytes: u64 = 0;
            let mut window_known = true;
            for capture in ordered.iter().take(keep_count) {
                match capture.bytes {
                    Some(bytes) => retained_bytes = retained_bytes.saturating_add(bytes),
                    None => window_known = false,
                }
            }

            if !window_known {
                budget_note = Some(format!(
                    " The {limit} byte budget was not applied to older captures, because a \
                     retained capture has no recorded size."
                ));
            } else if retained_bytes >= limit {
                budget_note = Some(format!(
                    " The {limit} byte budget is already exhausted by the retained captures, so \
                     no older capture is retained."
                ));
            } else {
                let mut extended = keep_count;
                while extended < total {
                    match ordered[extended].bytes {
                        Some(bytes) if retained_bytes.saturating_add(bytes) <= limit => {
                            retained_bytes = retained_bytes.saturating_add(bytes);
                            extended = extended.saturating_add(1);
                        }
                        Some(_) => break,
                        None => {
                            budget_note = Some(format!(
                                " The {limit} byte budget could not be checked against the next \
                                 older capture because its size is unknown, so it and everything \
                                 older are not retained."
                            ));
                            break;
                        }
                    }
                }

                if extended > keep_count {
                    let added = extended.saturating_sub(keep_count);
                    budget_note = Some(format!(
                        " The {limit} byte budget also allows {added} older {} to be retained.",
                        captures_word(added)
                    ));
                } else if budget_note.is_none() {
                    budget_note = Some(format!(
                        " The {limit} byte budget leaves no room for older captures."
                    ));
                }
                keep_count = extended;
            }
        }
    }

    let keep: Vec<PathBuf> = ordered
        .iter()
        .take(keep_count)
        .map(|capture| capture.path.clone())
        .collect();
    let delete_candidates: Vec<PathBuf> = ordered
        .iter()
        .skip(keep_count)
        .map(|capture| capture.path.clone())
        .collect();

    let mut retained_bytes: u64 = 0;
    let mut sizes_known = true;
    for capture in ordered.iter().take(keep_count) {
        match capture.bytes {
            Some(bytes) => retained_bytes = retained_bytes.saturating_add(bytes),
            None => sizes_known = false,
        }
    }
    let total_bytes = if sizes_known {
        Some(retained_bytes)
    } else {
        None
    };

    let mut note = String::new();
    if policy.keep_last == 0 {
        note.push_str(&format!(
            "Keep-last is 0, which means keep everything: all {} are retained and nothing is \
             proposed for deletion.",
            captures_phrase(total)
        ));
        if policy.max_bytes.is_some() {
            note.push_str(
                " The byte budget is deliberately not applied, because a keep-everything policy \
                 must never delete a capture.",
            );
        }
    } else {
        let dropped = total.saturating_sub(keep_count);
        note.push_str(&format!(
            "Keep-last is {}. Retained (newest first): {} of {}. Delete candidates: {}.",
            policy.keep_last,
            keep_count,
            captures_phrase(total),
            if dropped == 0 {
                "none".to_string()
            } else {
                captures_phrase(dropped)
            }
        ));
        if let Some(fragment) = budget_note {
            note.push_str(&fragment);
        }
    }

    match total_bytes {
        Some(bytes) => note.push_str(&format!(" Retained size: {bytes} bytes.")),
        None => note.push_str(
            " Retained size is unknown because at least one retained capture has no recorded \
             size.",
        ),
    }
    note.push_str(
        " This plan is a proposal only: the crate deletes nothing, and the shell may delete only \
         after you confirm.",
    );

    RetentionPlan {
        keep,
        delete_candidates,
        total_bytes,
        note,
    }
}

/// "1 capture" / "3 captures", for notes a human reads.
fn captures_phrase(count: usize) -> String {
    if count == 1 {
        "1 capture".to_string()
    } else {
        format!("{count} captures")
    }
}

/// "capture" / "captures" without the number, for sentences that already
/// state it.
fn captures_word(count: usize) -> &'static str {
    if count == 1 {
        "capture"
    } else {
        "captures"
    }
}
