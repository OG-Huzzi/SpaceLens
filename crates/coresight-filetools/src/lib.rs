//! CoreSight **file-tools planning** — the pure, evidence-first layer for
//! five jv16-PowerTools-class features: duplicate finding, big-file
//! discovery, bulk rename, file split/merge, and semantic name search.
//!
//! The crate is a **planner, not an executor**. Every function here reads
//! only caller-provided facts and returns a reviewable plan. It performs no
//! filesystem access, starts no process, opens no socket, and writes nothing
//! (no persistence). The adapter that actually renames, splits, merges, or
//! removes files is a later, separately authorized layer. The Finder is the
//! single exception to "no ambient reads": [`search`] reads the wall clock
//! once for its age criterion, and [`search_at`] makes that instant an
//! argument for callers that need a reproducible report.
//!
//! ## Evidence contracts
//!
//! * **A duplicate needs verified content identity.** Two entries are
//!   duplicates only when both carry a verified `content_sha256` AND the
//!   hashes and sizes are equal. Name, size, or modification time alone
//!   never prove duplication; entries without a hash are counted as unknown
//!   and never grouped.
//! * **Hard links are one object.** Entries sharing a proven [`ObjectId`]
//!   are the same filesystem object. They are reported as
//!   [`DuplicateMember::is_hardlink_alias`] and never inflate recoverable
//!   bytes, because deleting one alias frees nothing.
//! * **Unknown age is not old age.** A missing `modified` timestamp can
//!   never satisfy an age filter; such entries are excluded and counted.
//! * **Reject the dangerous plan.** Bulk rename refuses duplicate targets,
//!   empty or invalid names, duplicate sources, and any target cycle rather
//!   than leaving the executor to discover them mid-flight.
//! * **Search hits explain themselves.** [`search`] matches an entry's OWN
//!   name and never its parent path, publishes one sentence per satisfied
//!   criterion, and never lets an unknown size, unknown age, or undecodable
//!   category satisfy a criterion.
//! * **Deterministic.** Every output is ordered by explicit keys; the same
//!   input set in any order produces an identical plan.
//! * **Bounded.** Group/member/result caps are explicit and every omitted
//!   item is counted exactly. Arithmetic saturates rather than wraps.
//! * **No panics.** Production code has no `unwrap`, `expect`, or panic
//!   paths; degenerate input yields a typed error or an empty report.
//!
//! ## Split/merge is planning only
//!
//! [`plan_split`] and [`plan_merge`] compute names, offsets, and expected
//! sizes. They do not read or write bytes. The executor MUST checksum the
//! result: split parts must re-hash to the source digest, and a merged file
//! must re-hash to the digest declared by the plan. A size match alone is
//! not integrity evidence.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use coresight_classifier::Category;
use coresight_history::{ObjectId, ObservedEntry, ObservedKind, Snapshot};
use serde::{Deserialize, Serialize};

mod finder;

pub use finder::{
    search, search_at, SearchHit, SearchKind, SearchOptions, SearchQuery, SearchReport,
};

/// Seconds in one day, used by the age criteria of the big-file finder and
/// of the semantic Finder.
const SECONDS_PER_DAY: u64 = 86_400;

/// Maximum number of zero-padding digits accepted by
/// [`RenameRule::Sequence`].
///
/// A caller-supplied padding width is otherwise an unbounded allocation
/// request. Counts never need more than 20 digits (`u64::MAX`), so 32 is
/// already generous; larger requests are clamped, never honored.
const MAX_SEQUENCE_PADDING: usize = 32;

/// Hard bound on the number of parts [`plan_split`] will plan.
///
/// A degenerate `(total_size, part_size)` pair (for example `u64::MAX` with
/// a one-byte part size) would otherwise request an unbounded list. Larger
/// requests are rejected with [`SplitError::TooManyParts`], never silently
/// reduced.
pub const MAX_SPLIT_PARTS: u64 = 1_000_000;

// ---------------------------------------------------------------------------
// Duplicate finder
// ---------------------------------------------------------------------------

/// Caps and thresholds for [`find_duplicates`].
///
/// Defaults are deliberately conservative: one byte minimum (empty files
/// free nothing), 100 groups, and 256 displayed members per group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOptions {
    /// Maximum number of groups published. Qualifying groups beyond this
    /// cap are counted exactly in [`DuplicateReport::truncated_groups`].
    pub max_groups: usize,
    /// Files smaller than this are not duplicate candidates at all (they
    /// are not counted as unverified — they are simply out of scope).
    pub min_size_bytes: u64,
    /// Maximum members published per group. Members beyond this cap are
    /// counted exactly in [`DuplicateReport::truncated_members`]. The
    /// group's `recoverable_bytes` is computed from the FULL group, so a
    /// display cap can never understate savings.
    pub max_members_per_group: usize,
}

impl Default for DuplicateOptions {
    fn default() -> Self {
        DuplicateOptions {
            max_groups: 100,
            min_size_bytes: 1,
            max_members_per_group: 256,
        }
    }
}

/// One path that shares verified content identity with its group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMember {
    /// Where the entry was observed.
    pub path: PathBuf,
    /// Proven filesystem object identity, when the observation proved it.
    /// `None` is an honest unknown — never assumed to be a distinct object
    /// and never assumed to be an alias either.
    pub object: Option<ObjectId>,
    /// True when an earlier member of this group proved the SAME
    /// [`ObjectId`]. A hard-link alias is another name for one object:
    /// removing it frees no space and it is never counted as recoverable.
    pub is_hardlink_alias: bool,
}

/// One set of entries proven to hold identical bytes.
///
/// A group is emitted only when at least two entries carry the same verified
/// hash AND the same size. A group whose members are all hard-link aliases
/// of one object is still emitted (so the relationship is visible) but
/// reports `recoverable_bytes == 0`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    /// Verified SHA-256 content identity shared by every member.
    pub content_sha256: String,
    /// Logical size shared by every member. Equal size is REQUIRED in
    /// addition to equal hash; inconsistent evidence is never merged.
    pub size: u64,
    /// Members sorted by path bytes, capped by
    /// [`DuplicateOptions::max_members_per_group`].
    pub members: Vec<DuplicateMember>,
    /// `size * (distinct_objects - 1)`, saturating. Aliases of one object
    /// contribute nothing; each unprovable object counts as distinct.
    pub recoverable_bytes: u64,
}

/// Result of one duplicate scan, with exact accounting for what was not
/// shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReport {
    /// Published groups, sorted by `(recoverable_bytes desc,
    /// content_sha256 asc)`, then size ascending as a deterministic
    /// tie-break.
    pub groups: Vec<DuplicateGroup>,
    /// File entries whose content identity was never verified
    /// (`content_sha256 == None`). Unknown is not empty: these entries are
    /// never grouped and never counted as duplicates.
    pub unverified_entries: u64,
    /// File entries that carry a verified hash but no observed size. Size
    /// equality cannot be proven, so they are never grouped. Counted
    /// separately from unverified content so the reason is unambiguous.
    pub unknown_size_entries: u64,
    /// Number of qualifying groups before `max_groups` truncation.
    pub total_groups: u64,
    /// Qualifying groups omitted by `max_groups`, counted exactly.
    pub truncated_groups: u64,
    /// Members omitted from PUBLISHED groups by `max_members_per_group`,
    /// counted exactly.
    pub truncated_members: u64,
    /// Recoverable bytes across ALL qualifying groups (saturating),
    /// including groups omitted by the cap — so a bounded view cannot hide
    /// the true total.
    pub total_recoverable_bytes: u64,
}

/// Finds duplicate files using verified content identity only.
///
/// A duplicate claim is made ONLY when both entries have
/// `content_sha256: Some(hash)`, the hashes are equal, AND the observed
/// sizes are equal. Entries without a hash are counted in
/// `unverified_entries` and never grouped. Entries sharing a proven
/// [`ObjectId`] are collapsed as hard-link aliases and never treated as
/// recoverable space.
///
/// Output is deterministic for any input permutation and bounded by
/// [`DuplicateOptions`]; all omitted items are counted exactly.
pub fn find_duplicates(snapshot: &Snapshot, options: &DuplicateOptions) -> DuplicateReport {
    let mut unverified_entries = 0u64;
    let mut unknown_size_entries = 0u64;
    // Keyed by (size, hash): equal hash alone is not enough when the
    // observed sizes disagree (corrupt or stale evidence must stay split).
    let mut buckets: BTreeMap<(u64, String), Vec<&ObservedEntry>> = BTreeMap::new();

    for entry in &snapshot.entries {
        if entry.kind != ObservedKind::File {
            // Directories, links, and other kinds are outside duplicate
            // content scope; they are not "unverified files".
            continue;
        }
        let Some(content_sha256) = entry.content_sha256.as_ref() else {
            unverified_entries = unverified_entries.saturating_add(1);
            continue;
        };
        let Some(size) = entry.size else {
            unknown_size_entries = unknown_size_entries.saturating_add(1);
            continue;
        };
        if size < options.min_size_bytes {
            continue;
        }
        buckets
            .entry((size, content_sha256.clone()))
            .or_default()
            .push(entry);
    }

    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for ((size, content_sha256), mut entries) in buckets {
        if entries.len() < 2 {
            // One verified entry is a fact about content, not a duplicate.
            continue;
        }
        entries.sort_by(|a, b| path_bytes(&a.path).cmp(path_bytes(&b.path)));

        let mut seen_objects: BTreeMap<ObjectId, ()> = BTreeMap::new();
        let mut distinct_objects = 0u64;
        let mut members: Vec<DuplicateMember> = Vec::with_capacity(entries.len());
        for entry in entries {
            let is_hardlink_alias = match entry.object {
                Some(object) => {
                    if seen_objects.insert(object, ()).is_some() {
                        true
                    } else {
                        distinct_objects = distinct_objects.saturating_add(1);
                        false
                    }
                }
                None => {
                    // An unprovable object can never be collapsed into
                    // another entry; each is conservatively distinct.
                    distinct_objects = distinct_objects.saturating_add(1);
                    false
                }
            };
            members.push(DuplicateMember {
                path: entry.path.clone(),
                object: entry.object,
                is_hardlink_alias,
            });
        }

        let recoverable_bytes = size.saturating_mul(distinct_objects.saturating_sub(1));
        groups.push(DuplicateGroup {
            content_sha256,
            size,
            members,
            recoverable_bytes,
        });
    }

    groups.sort_by(|a, b| {
        b.recoverable_bytes
            .cmp(&a.recoverable_bytes)
            .then_with(|| a.content_sha256.cmp(&b.content_sha256))
            .then_with(|| a.size.cmp(&b.size))
    });

    let total_groups = groups.len() as u64;
    let total_recoverable_bytes = groups.iter().fold(0u64, |total, group| {
        total.saturating_add(group.recoverable_bytes)
    });

    let published_groups = groups.len().min(options.max_groups);
    let truncated_groups = groups.len().saturating_sub(published_groups) as u64;
    let mut truncated_members = 0u64;
    for group in groups.iter_mut().take(published_groups) {
        let published_members = group.members.len().min(options.max_members_per_group);
        truncated_members = truncated_members
            .saturating_add(group.members.len().saturating_sub(published_members) as u64);
        group.members.truncate(published_members);
    }
    groups.truncate(published_groups);

    DuplicateReport {
        groups,
        unverified_entries,
        unknown_size_entries,
        total_groups,
        truncated_groups,
        truncated_members,
        total_recoverable_bytes,
    }
}

// ---------------------------------------------------------------------------
// Big-file finder
// ---------------------------------------------------------------------------

/// Caps and thresholds for [`find_big_files`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BigFileOptions {
    /// Minimum logical size (inclusive) for a file to be a candidate.
    pub min_size_bytes: u64,
    /// Optional minimum age in whole days. When set, an entry is eligible
    /// only when `modified` is known AND `now - modified >= min_age_days`.
    /// Entries with unknown age are excluded and counted — never treated as
    /// old.
    pub min_age_days: Option<u64>,
    /// Maximum candidates published. Remainder is counted exactly in
    /// [`BigFileReport::truncated`].
    pub max_results: usize,
}

impl Default for BigFileOptions {
    fn default() -> Self {
        BigFileOptions {
            min_size_bytes: 100 * 1024 * 1024,
            min_age_days: None,
            max_results: 100,
        }
    }
}

/// One file that satisfies the big-file criteria.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BigFileCandidate {
    /// Where the file was observed.
    pub path: PathBuf,
    /// Observed logical size; always at least
    /// [`BigFileOptions::min_size_bytes`].
    pub size: u64,
    /// Observed modification time. `None` means unknown — never inferred.
    pub modified: Option<SystemTime>,
    /// Stored classification decoded through the stable category codes.
    /// `None` means the run recorded no classification, or recorded a code
    /// this build does not know; neither is guessed into a category.
    pub category: Option<Category>,
}

/// Result of one big-file scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BigFileReport {
    /// Candidates sorted by `(size desc, path asc)`, truncated at
    /// `max_results`.
    pub candidates: Vec<BigFileCandidate>,
    /// Candidates omitted by `max_results`, counted exactly.
    pub truncated: u64,
    /// Files that met the size threshold but were excluded because the age
    /// filter was active and `modified` was unknown. Zero when no age filter
    /// was requested. These are unknown, never "old".
    pub unknown_age_entries: u64,
}

/// Finds large files using observed size and age facts only.
///
/// A candidate must be an [`ObservedKind::File`] with a known size at least
/// `min_size_bytes`. When `min_age_days` is set, `modified` must be known
/// and the elapsed duration must meet the threshold; unknown or
/// future-dated timestamps are not evidence of age and are excluded.
/// Results are deterministic and exactly bounded.
pub fn find_big_files(
    snapshot: &Snapshot,
    now: SystemTime,
    options: &BigFileOptions,
) -> BigFileReport {
    let mut candidates: Vec<BigFileCandidate> = Vec::new();
    let mut unknown_age_entries = 0u64;

    for entry in &snapshot.entries {
        if entry.kind != ObservedKind::File {
            continue;
        }
        let Some(size) = entry.size else {
            continue;
        };
        if size < options.min_size_bytes {
            continue;
        }
        if let Some(min_age_days) = options.min_age_days {
            let Some(modified) = entry.modified else {
                unknown_age_entries = unknown_age_entries.saturating_add(1);
                continue;
            };
            let Ok(age) = now.duration_since(modified) else {
                // A timestamp later than `now` proves nothing about age — clock
                // skew, a restored backup, or a file written by a process with
                // a wrong clock. It is excluded from the age-filtered result,
                // but it MUST be counted as unknown: silently dropping it would
                // leave `is_conclusive()` true and let the caller describe an
                // incomplete report as the complete set of old large files.
                unknown_age_entries = unknown_age_entries.saturating_add(1);
                continue;
            };
            let required_seconds = min_age_days.saturating_mul(SECONDS_PER_DAY);
            if age.as_secs() < required_seconds {
                continue;
            }
        }
        candidates.push(BigFileCandidate {
            path: entry.path.clone(),
            size,
            modified: entry.modified,
            category: entry
                .classification
                .as_ref()
                .and_then(|classification| decode_category(&classification.category)),
        });
    }

    candidates.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| path_bytes(&a.path).cmp(path_bytes(&b.path)))
    });
    let total_candidates = candidates.len();
    let published = total_candidates.min(options.max_results);
    let truncated = total_candidates.saturating_sub(published) as u64;
    candidates.truncate(published);

    BigFileReport {
        candidates,
        truncated,
        unknown_age_entries,
    }
}

/// Decodes a stored category code into the typed taxonomy.
///
/// History stores the serialized code (for example `"CACHE"`), and the
/// classifier exposes no public from-code parser. The local decoder walks
/// [`Category::ALL`], so it covers exactly the 18 stable codes and cannot
/// drift when the taxonomy grows. Unknown codes stay `None` — never guessed
/// into [`Category::Unknown`].
fn decode_category(code: &str) -> Option<Category> {
    Category::ALL
        .iter()
        .copied()
        .find(|category| category.code() == code)
}

// ---------------------------------------------------------------------------
// Bulk rename planning
// ---------------------------------------------------------------------------

/// One input to [`plan_rename`]: the current path of an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameInput {
    /// Current path. The file name is transformed; the parent is preserved.
    pub path: PathBuf,
}

/// A bulk-rename transformation, applied to the file STEM only.
///
/// Every variant preserves the extension (the text after the final dot) so a
/// rename can never silently change a file's type. Rules operate on one file
/// name at a time and are deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenameRule {
    /// Insert text before the stem: `report.txt` → `2026-report.txt`.
    AddPrefix(String),
    /// Append text to the stem: `report.txt` → `report-final.txt`.
    AddSuffix(String),
    /// Replace every occurrence of `find` inside the stem. An empty `find`
    /// is a no-op (it can never inject text between every character).
    Replace {
        /// Literal text to find in the stem.
        find: String,
        /// Replacement text. May be empty to delete the match.
        #[serde(rename = "replaceWith")]
        replace_with: String,
    },
    /// Remove every character contained in the given SET of characters.
    /// Duplicate characters in the set are harmless.
    RemoveCharacters(String),
    /// Append `_<number>` to the stem. Numbers start at `start` and are
    /// assigned after canonical path sorting, so input order can never
    /// change the result. `padding` is the minimum digit width (clamped to
    /// 32); `start` plus the index saturates at `u64::MAX`.
    Sequence {
        /// First number in the sequence.
        start: u64,
        /// Minimum zero-padded digit width.
        padding: usize,
    },
}

/// One planned rename. `from == to` is a truthful no-op preview, not an
/// error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameOperation {
    /// Path the entry currently has.
    pub from: PathBuf,
    /// Path it would have after the rename.
    pub to: PathBuf,
}

/// A complete bulk-rename PREVIEW. No operation has been performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePlan {
    /// Operations sorted by platform-normalized source path (raw path bytes
    /// as the deterministic tie-break), so the preview and any caller
    /// display are deterministic.
    pub operations: Vec<RenameOperation>,
}

/// Why a bulk-rename plan was refused.
///
/// These are the failures that actually destroy data during a bulk rename:
/// two entries collapsing onto one name (silent overwrite), an empty name,
/// malformed input, or a target chain that forms a cycle and cannot be
/// executed even with a temporary name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// Two operations would produce the same target path. Renaming would
    /// silently overwrite one file with another.
    DuplicateTarget {
        /// The colliding target path.
        target: PathBuf,
    },
    /// The rule produced an empty file name, or the source path has no
    /// file-name component at all.
    EmptyName {
        /// The source path that produced the empty name.
        path: PathBuf,
    },
    /// Following target→source edges returns to an operation already in the
    /// chain. At least one file would be overwritten before it could be moved
    /// aside.
    CyclicRename {
        /// A source path on the detected cycle.
        path: PathBuf,
    },
    /// The same source path was supplied twice (including two spellings that
    /// name the same path on this platform).
    DuplicateSource {
        /// The repeated source path.
        path: PathBuf,
    },
    /// The rule produced a name that is not a single file-name component (it
    /// contains a path separator or NUL, is `.`/`..`, or the original name
    /// is not valid UTF-8).
    InvalidName {
        /// The source path involved.
        path: PathBuf,
        /// The rejected generated name (lossy only for diagnostics).
        name: String,
    },
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::DuplicateTarget { target } => {
                write!(formatter, "duplicate rename target: {}", target.display())
            }
            RenameError::EmptyName { path } => write!(
                formatter,
                "rename would produce an empty name for {}",
                path.display()
            ),
            RenameError::CyclicRename { path } => {
                write!(formatter, "rename cycle detected at {}", path.display())
            }
            RenameError::DuplicateSource { path } => {
                write!(formatter, "duplicate rename source: {}", path.display())
            }
            RenameError::InvalidName { path, name } => write!(
                formatter,
                "rename produced invalid file name {name:?} for {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RenameError {}

/// Plans a bulk rename without touching disk.
///
/// Entries are first sorted by platform-normalized path, then the rule is
/// applied to each stem, preserving the extension. A sequence rule numbers
/// entries in that sorted order, so the same set in any input order yields
/// the identical plan.
///
/// The plan is refused (`Err`) when the rule would produce a duplicate
/// target, an empty/invalid file name, a duplicate source, or a target
/// cycle. It cannot know about files outside the supplied set: the executor
/// MUST re-check each target immediately before renaming and MUST provide an
/// undo journal.
pub fn plan_rename(entries: &[RenameInput], rule: &RenameRule) -> Result<RenamePlan, RenameError> {
    let mut ordered: Vec<(&RenameInput, Vec<u8>)> = entries
        .iter()
        .map(|input| (input, normalized_path_key(&input.path)))
        .collect();
    ordered.sort_by(|(left, left_key), (right, right_key)| {
        left_key
            .cmp(right_key)
            .then_with(|| path_bytes(&left.path).cmp(path_bytes(&right.path)))
    });

    for pair in ordered.windows(2) {
        if pair[0].1 == pair[1].1 {
            return Err(RenameError::DuplicateSource {
                path: pair[1].0.path.clone(),
            });
        }
    }

    let mut operations: Vec<RenameOperation> = Vec::with_capacity(ordered.len());
    for (index, (input, _)) in ordered.iter().enumerate() {
        let sequence_index = u64::try_from(index).unwrap_or(u64::MAX);
        let to = build_target_name(&input.path, rule, sequence_index)?;
        operations.push(RenameOperation {
            from: input.path.clone(),
            to,
        });
    }

    let mut targets: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (index, operation) in operations.iter().enumerate() {
        if targets
            .insert(normalized_path_key(&operation.to), index)
            .is_some()
        {
            return Err(RenameError::DuplicateTarget {
                target: operation.to.clone(),
            });
        }
    }

    reject_cycles(&operations)?;

    Ok(RenamePlan { operations })
}

/// Rejects any target→source chain that forms a cycle.
///
/// Separate from [`plan_rename`] so the refusal path is directly testable
/// even though the five current rule variants are non-permuting and cannot
/// construct a cycle themselves.
fn reject_cycles(operations: &[RenameOperation]) -> Result<(), RenameError> {
    let mut sources: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (index, operation) in operations.iter().enumerate() {
        sources.insert(normalized_path_key(&operation.from), index);
    }

    // Each node has at most one outgoing edge: "my target is this other
    // operation's source". A cycle in this functional graph is exactly the
    // unsafe rename loop. Chains are safe (execute them back to front).
    let mut next: Vec<Option<usize>> = vec![None; operations.len()];
    for (index, operation) in operations.iter().enumerate() {
        if let Some(target_index) = sources.get(&normalized_path_key(&operation.to)) {
            if *target_index != index {
                if let Some(slot) = next.get_mut(index) {
                    *slot = Some(*target_index);
                }
            }
        }
    }
    if let Some(cycle_index) = first_cycle(&next) {
        let path = operations
            .get(cycle_index)
            .map(|operation| operation.from.clone())
            .unwrap_or_default();
        return Err(RenameError::CyclicRename { path });
    }
    Ok(())
}

/// Applies one rule to one stem. All arithmetic saturates.
fn apply_rule(stem: &str, rule: &RenameRule, sequence_index: u64) -> String {
    match rule {
        RenameRule::AddPrefix(prefix) => format!("{prefix}{stem}"),
        RenameRule::AddSuffix(suffix) => format!("{stem}{suffix}"),
        RenameRule::Replace { find, replace_with } => {
            if find.is_empty() {
                stem.to_string()
            } else {
                stem.replace(find, replace_with)
            }
        }
        RenameRule::RemoveCharacters(characters) => stem
            .chars()
            .filter(|character| !characters.contains(*character))
            .collect(),
        RenameRule::Sequence { start, padding } => {
            let number = start.saturating_add(sequence_index);
            let width = (*padding).min(MAX_SEQUENCE_PADDING);
            if width == 0 {
                format!("{stem}_{number}")
            } else {
                format!("{stem}_{number:0width$}")
            }
        }
    }
}

/// Whether a produced name cannot be stored as a regular file name.
///
/// A rename plan is a promise the executor can move a file to this name. A
/// Windows reserved device name, or a name ending in a space or a dot, breaks
/// that promise: the name either names a device or is not storable as written.
/// Refusing at planning time is the safe direction — the user is told the rule
/// cannot be applied rather than left with a half-applied rename.
fn is_unstorable_name(name: &str, original_name: &str) -> bool {
    // Only reject a trailing space/dot the rule INTRODUCED. A pre-existing one
    // is preserved (the file already exists under that name).
    if name.ends_with(' ') && !original_name.ends_with(' ') {
        return true;
    }
    if name.ends_with('.') && !original_name.ends_with('.') {
        return true;
    }
    let stem = match name.split_once('.') {
        Some((stem, _)) => stem,
        None => name,
    };
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
}

/// Builds one target path: transform the stem, reattach the original
/// extension, keep the parent directory, and validate the result.
fn build_target_name(
    path: &Path,
    rule: &RenameRule,
    sequence_index: u64,
) -> Result<PathBuf, RenameError> {
    let Some(file_name) = path.file_name() else {
        return Err(RenameError::EmptyName {
            path: path.to_path_buf(),
        });
    };
    if file_name.to_str().is_none() {
        return Err(RenameError::InvalidName {
            path: path.to_path_buf(),
            name: file_name.to_string_lossy().into_owned(),
        });
    }
    let Some(stem_os) = path.file_stem() else {
        return Err(RenameError::EmptyName {
            path: path.to_path_buf(),
        });
    };
    let Some(stem) = stem_os.to_str() else {
        return Err(RenameError::InvalidName {
            path: path.to_path_buf(),
            name: stem_os.to_string_lossy().into_owned(),
        });
    };
    let extension = match path.extension() {
        Some(extension) => match extension.to_str() {
            Some(text) => Some(text),
            None => {
                return Err(RenameError::InvalidName {
                    path: path.to_path_buf(),
                    name: file_name.to_string_lossy().into_owned(),
                });
            }
        },
        None => None,
    };

    let new_stem = apply_rule(stem, rule, sequence_index);
    let mut new_name = new_stem;
    if let Some(extension) = extension {
        // `Some("")` occurs for a trailing dot (`report.`); preserve it —
        // dropping the dot would silently change the observed file name.
        new_name.push('.');
        new_name.push_str(extension);
    }

    if new_name.is_empty() {
        return Err(RenameError::EmptyName {
            path: path.to_path_buf(),
        });
    }
    if new_name == "."
        || new_name == ".."
        || new_name.contains('/')
        || new_name.contains('\\')
        || new_name.contains('\0')
    {
        return Err(RenameError::InvalidName {
            path: path.to_path_buf(),
            name: new_name,
        });
    }
    // A name ending in a space or a dot is not reliably storable, and a Windows
    // reserved device name (`CON`, `NUL`, `COM1`, …) names a device rather than
    // a file. Renaming a real file to either would produce a target the
    // executor cannot honour, so the plan is refused instead.
    //
    // Exception: a trailing space or dot that the source name ALREADY has is
    // preserved, not created. The file exists under that name today, so
    // keeping it cannot produce an unstorable target — only *introducing* one
    // can. (This is why the original name is passed in.)
    let original_name = file_name.to_str().unwrap_or_default();
    if is_unstorable_name(&new_name, original_name) {
        return Err(RenameError::InvalidName {
            path: path.to_path_buf(),
            name: new_name,
        });
    }

    Ok(match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(&new_name),
        _ => PathBuf::from(&new_name),
    })
}

/// Returns one node on a cycle in a functional graph, or `None` when the
/// graph is acyclic. Deterministic: the lowest reachable cycle start wins.
fn first_cycle(next: &[Option<usize>]) -> Option<usize> {
    let mut state: Vec<u8> = vec![0; next.len()]; // 0 = new, 1 = on path, 2 = done
    let mut start = 0usize;
    while start < next.len() {
        if state.get(start).copied().unwrap_or(2) != 0 {
            start = start.saturating_add(1);
            continue;
        }
        let mut path: Vec<usize> = Vec::new();
        let mut node = start;
        loop {
            match state.get(node).copied() {
                Some(0) => {
                    if let Some(slot) = state.get_mut(node) {
                        *slot = 1;
                    }
                    path.push(node);
                }
                Some(1) => return Some(node),
                _ => break,
            }
            match next.get(node).copied().flatten() {
                Some(next_node) => node = next_node,
                None => break,
            }
        }
        for visited in path {
            if let Some(slot) = state.get_mut(visited) {
                *slot = 2;
            }
        }
        start = start.saturating_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// Split / merge planning
// ---------------------------------------------------------------------------

/// One expected part produced by [`plan_split`]. Parts are 1-indexed and
/// ordered by `index`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPart {
    /// 1-based part number used in the generated file name.
    pub index: u64,
    /// Expected part path: `<original file name>.partNNN` in the source's
    /// directory.
    pub path: PathBuf,
    /// Byte offset of this part's first byte in the source file.
    pub offset: u64,
    /// Expected number of bytes in this part. Every part except the last
    /// equals `part_size`; the last carries the remainder.
    pub size: u64,
}

/// A reviewable split plan. No bytes have been read or written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPlan {
    /// Source file to split.
    pub source: PathBuf,
    /// Total declared source size.
    pub total_size: u64,
    /// Requested part size.
    pub part_size: u64,
    /// Number of parts; equals `parts.len()` and is checked against
    /// [`MAX_SPLIT_PARTS`].
    pub part_count: u64,
    /// Ordered parts (ascending index and offset).
    pub parts: Vec<SplitPart>,
}

/// Why a split was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitError {
    /// A part size of zero can never make progress; refused before any
    /// division is attempted.
    ZeroPartSize,
    /// A zero-byte source has nothing to split. An "empty split" would
    /// produce no parts, which is not a valid preview.
    EmptyFile,
    /// The source path has no file-name component, so no part names can be
    /// derived.
    NoFileName {
        /// The rejected path.
        path: PathBuf,
    },
    /// The request would produce more than [`MAX_SPLIT_PARTS`] parts. Refused
    /// rather than allocating an unbounded plan.
    TooManyParts {
        /// Requested count.
        count: u64,
        /// Hard bound.
        max: u64,
    },
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::ZeroPartSize => write!(formatter, "part size must be greater than zero"),
            SplitError::EmptyFile => write!(formatter, "cannot split an empty file"),
            SplitError::NoFileName { path } => {
                write!(
                    formatter,
                    "cannot derive part names from {}",
                    path.display()
                )
            }
            SplitError::TooManyParts { count, max } => write!(
                formatter,
                "split would produce {count} parts, exceeding the limit of {max}"
            ),
        }
    }
}

impl std::error::Error for SplitError {}

/// Plans an ordered split of `path` into `part_size` chunks.
///
/// Part names are `<file name>.partNNN` in the same directory; sizes are
/// `part_size` except for the final remainder. This function only computes
/// the plan. The executor MUST verify that every produced part re-hashes to
/// the expected digest and that concatenating the parts reproduces the
/// source digest.
pub fn plan_split(path: &Path, total_size: u64, part_size: u64) -> Result<SplitPlan, SplitError> {
    if part_size == 0 {
        return Err(SplitError::ZeroPartSize);
    }
    if total_size == 0 {
        return Err(SplitError::EmptyFile);
    }
    let Some(file_name) = path.file_name() else {
        return Err(SplitError::NoFileName {
            path: path.to_path_buf(),
        });
    };
    let part_count = total_size.div_ceil(part_size);
    if part_count > MAX_SPLIT_PARTS {
        return Err(SplitError::TooManyParts {
            count: part_count,
            max: MAX_SPLIT_PARTS,
        });
    }

    let mut parts: Vec<SplitPart> = Vec::with_capacity(part_count as usize);
    // One fixed width for every part number so lexical order matches merge
    // order even past 999 parts (e.g. part0001 … part1001).
    let number_width = part_count.to_string().len().max(3);
    let mut offset = 0u64;
    for index in 0..part_count {
        let size = total_size.saturating_sub(offset).min(part_size);
        let number = index.saturating_add(1);
        let mut part_name = file_name.to_os_string();
        part_name.push(format!(".part{number:0number_width$}"));
        let part_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(&part_name),
            _ => PathBuf::from(&part_name),
        };
        parts.push(SplitPart {
            index: number,
            path: part_path,
            offset,
            size,
        });
        offset = offset.saturating_add(size);
    }

    Ok(SplitPlan {
        source: path.to_path_buf(),
        total_size,
        part_size,
        part_count,
        parts,
    })
}

/// One declared part supplied to [`plan_merge`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePart {
    /// Path of the part to merge.
    pub path: PathBuf,
    /// Size the caller declares this part has. Used for layout and
    /// accounting only; it is not integrity evidence.
    pub expected_size: u64,
}

/// A reviewable merge plan. No bytes have been read or written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePlan {
    /// Parts in merge order (ascending path bytes, exactly as validated).
    pub parts: Vec<MergePart>,
    /// Sum of declared part sizes (saturating, overflow rejected).
    pub total_size: u64,
    /// Number of parts.
    pub part_count: u64,
}

/// Why a merge was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// No parts were supplied; there is nothing to merge.
    EmptyParts,
    /// Declared sizes are unusable: a zero-sized part, or a sum that would
    /// overflow `u64`. The plan never silently saturates a wrong total.
    SizeMismatch,
    /// Parts were not supplied in strictly ascending path order. Merging them
    /// in this order would corrupt the output.
    UnorderedParts {
        /// Previous accepted path.
        previous: PathBuf,
        /// Path that sorted before its predecessor.
        next: PathBuf,
    },
    /// The same path was supplied twice.
    DuplicatePart {
        /// The repeated path.
        path: PathBuf,
    },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::EmptyParts => write!(formatter, "no parts supplied for merge"),
            MergeError::SizeMismatch => {
                write!(formatter, "declared part sizes do not form a valid total")
            }
            MergeError::UnorderedParts { previous, next } => write!(
                formatter,
                "parts are not ordered: {} precedes {}",
                previous.display(),
                next.display()
            ),
            MergeError::DuplicatePart { path } => {
                write!(formatter, "duplicate merge part: {}", path.display())
            }
        }
    }
}

impl std::error::Error for MergeError {}

/// Verifies the declared part list and produces a merge plan.
///
/// The caller's order is authoritative: parts must already be in strictly
/// ascending path-byte order, exactly as a split produced them. The planner
/// refuses unordered or duplicate parts instead of silently reordering a
/// selection that may point at the wrong files. Declared sizes must be
/// positive and their sum must fit in `u64`.
///
/// This is a PLAN. The executor MUST re-hash the merged output and compare it
/// with the expected digest before publishing it; matching declared sizes is
/// not proof of correct bytes.
pub fn plan_merge(parts: &[MergePart]) -> Result<MergePlan, MergeError> {
    if parts.is_empty() {
        return Err(MergeError::EmptyParts);
    }

    let mut total_size = 0u64;
    let mut previous: Option<&Path> = None;
    let mut seen: BTreeMap<Vec<u8>, ()> = BTreeMap::new();

    for part in parts {
        if part.expected_size == 0 {
            return Err(MergeError::SizeMismatch);
        }
        if seen.insert(normalized_path_key(&part.path), ()).is_some() {
            return Err(MergeError::DuplicatePart {
                path: part.path.clone(),
            });
        }
        if let Some(previous_path) = previous {
            match path_bytes(previous_path).cmp(path_bytes(&part.path)) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Greater => {
                    return Err(MergeError::UnorderedParts {
                        previous: previous_path.to_path_buf(),
                        next: part.path.clone(),
                    });
                }
                std::cmp::Ordering::Equal => {
                    // Equal raw bytes are already caught by the duplicate key
                    // above; this arm keeps the match exhaustive.
                    return Err(MergeError::DuplicatePart {
                        path: part.path.clone(),
                    });
                }
            }
        }
        if total_size > u64::MAX.saturating_sub(part.expected_size) {
            return Err(MergeError::SizeMismatch);
        }
        total_size = total_size.saturating_add(part.expected_size);
        previous = Some(&part.path);
    }

    Ok(MergePlan {
        parts: parts.to_vec(),
        total_size,
        part_count: parts.len() as u64,
    })
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Raw path bytes — the canonical ordering key required by the public
/// contracts ("path asc", "path bytes").
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// Platform-normalized path identity used only for plan collision checks.
///
/// Components are compared the way the platform's pathname rules do it:
/// Windows applies the same 1:1 simple-uppercase fold as the history layer
/// (no multi-character expansions); everything else compares exact bytes.
/// Separators and `.` components normalize away because
/// [`std::path::Path::components`] recognizes them.
fn normalized_path_key(path: &Path) -> Vec<u8> {
    let mut key: Vec<u8> = Vec::new();
    for component in path.components() {
        // 0xFF cannot occur inside a valid encoded path component, so it is
        // an unambiguous component separator.
        key.push(0xFF);
        key.extend_from_slice(&component_key(component.as_os_str()));
    }
    key
}

/// Case-folded bytes for one path component (Windows only), exact bytes
/// elsewhere.
fn component_key(component: &OsStr) -> Vec<u8> {
    #[cfg(windows)]
    {
        if let Some(text) = component.to_str() {
            let mut folded = String::with_capacity(text.len());
            for character in text.chars() {
                let mut upper = character.to_uppercase();
                match (upper.next(), upper.next()) {
                    // Only a 1:1 simple uppercase is applied, mirroring NTFS
                    // `$UpCase`; expansions such as ß→SS are not.
                    (Some(single), None) => folded.push(single),
                    _ => folded.push(character),
                }
            }
            return folded.into_bytes();
        }
    }
    component.as_encoded_bytes().to_vec()
}

// ---------------------------------------------------------------------------
// Internal tests for the cycle guard, which cannot be reached through the
// public rule set (all five rules are non-permuting; `Replace` performs one
// literal substitution and cannot swap two names). The detector is still
// exercised directly so a future rule cannot bypass it silently.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_detector_finds_two_node_cycle() {
        let next = [Some(1), Some(0)];
        assert_eq!(first_cycle(&next), Some(0));
    }

    #[test]
    fn cycle_detector_accepts_chain() {
        let next = [Some(1), Some(2), None];
        assert_eq!(first_cycle(&next), None);
    }

    #[test]
    fn cycle_detector_finds_tail_into_cycle() {
        let next = [Some(1), Some(2), Some(1)];
        assert_eq!(first_cycle(&next), Some(1));
    }

    #[test]
    fn cycle_detector_reports_self_loop() {
        // `plan_rename` never inserts a self edge (a case-only rename and a
        // plain no-op are both handled before this point). If a future rule
        // ever did, the detector must still fail closed rather than loop.
        let next = [None, Some(1)];
        assert_eq!(first_cycle(&next), Some(1));
    }

    #[test]
    fn cycle_guard_reports_cyclic_rename_for_a_swap() {
        let operations = vec![
            RenameOperation {
                from: PathBuf::from("/d/a.txt"),
                to: PathBuf::from("/d/b.txt"),
            },
            RenameOperation {
                from: PathBuf::from("/d/b.txt"),
                to: PathBuf::from("/d/a.txt"),
            },
        ];

        let error = reject_cycles(&operations).expect_err("swap must be refused");
        assert!(matches!(error, RenameError::CyclicRename { .. }));
    }

    #[test]
    fn cycle_guard_allows_a_chain() {
        let operations = vec![
            RenameOperation {
                from: PathBuf::from("/d/a.txt"),
                to: PathBuf::from("/d/b.txt"),
            },
            RenameOperation {
                from: PathBuf::from("/d/b.txt"),
                to: PathBuf::from("/d/c.txt"),
            },
        ];

        assert!(reject_cycles(&operations).is_ok());
    }

    #[test]
    fn category_decoder_covers_every_stable_code() {
        for category in Category::ALL {
            assert_eq!(decode_category(category.code()), Some(category));
        }
        assert_eq!(decode_category("SOMETHING_FUTURE"), None);
        assert_eq!(decode_category("cache"), None);
    }
}
