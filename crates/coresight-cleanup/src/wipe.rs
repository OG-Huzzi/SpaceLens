//! The wiper planner: honest assurances and explicit refusals.
//!
//! jv16 PowerTools' File Wiper is the scope reference here, and two of its
//! advertised behaviours are deliberately **not** reproduced:
//!
//! * *"Overwrite data multiple times with random data."* Multi-pass overwrite
//!   is largely theatre on modern storage. A solid-state controller
//!   wear-levels and remaps writes, and a copy-on-write filesystem (APFS,
//!   Btrfs, ZFS) allocates new blocks instead of rewriting the old ones, so the
//!   bytes a wipe program writes are not necessarily the bytes that held the
//!   data. A software overwrite can run to completion and never touch the
//!   original cells. Claiming "securely wiped" from it would be a false
//!   security claim, so this module never makes it: the best a software
//!   overwrite can earn is [`WipeAssurance::OverwritePlausible`], and only on
//!   rotational media, where no wear levelling stands between the write and the
//!   platter.
//! * *"Generate fake files for plausible deniability before deleting."*
//!   CoreSight declines this feature outright. It produces deceptive
//!   artefacts, offers no cryptographic benefit, and could be mistaken for data
//!   protection. See [`plausible_deniability_supported`], which reports the
//!   refusal so callers can branch on it instead of reimplementing the idea.
//!
//! ## Contracts
//!
//! 1. **Nothing is wiped, read, or written.** [`plan_wipe`] is pure: it turns
//!    data the caller already holds into a reviewable [`WipePlan`]. There is no
//!    I/O, no overwrite primitive, no deletion, and no execution in this
//!    module, and the crate's source-scan guard enforces that.
//! 2. **The medium decides the assurance, never the pass count.** A high pass
//!    count cannot turn wear levelling or copy-on-write into a guarantee; on
//!    those media the only honest answer is full-disk sanitisation, not a
//!    bigger number of writes.
//! 3. **`Unknown` is never treated as safe.** If the platform cannot say what
//!    the target lives on, the planner cannot rule out wear levelling or
//!    copy-on-write, so it must not assume a rotational disk. "We do not know"
//!    is not "magnetic".
//! 4. **Blocked is a reported outcome, not a silent omission.** Every requested
//!    target becomes a [`WipeStep`]; a step that must not run carries its
//!    [`WipeBlocker`] and is counted in [`WipePlan::blocked_count`]. It is never
//!    dropped from the plan the user reviews.
//! 5. **Size uncertainty is never rounded away.** [`WipePlan::total_bytes`] is
//!    `None` as soon as any requested target has an unknown size; a partial sum
//!    would look like a complete one.
//! 6. **Containment is checked before anything else.** A path outside every
//!    allowed root is blocked with [`WipeBlocker::OutsideAllowedRoots`], never
//!    "planned anyway" and never counted as actionable bytes. Neither the paths
//!    nor the roots are canonicalized, so the caller must spell both
//!    consistently. A path containing a parent-directory component (`..`) is
//!    treated as outside: resolving it would require I/O, and this planner
//!    performs none.
//! 7. **Directory detection is spelling-only.** [`WipeTarget`] carries no kind,
//!    and the planner does not stat the disk, so a path whose last byte is a
//!    separator (`/` or `\`) is read as a directory and blocked with
//!    [`WipeBlocker::NotARegularFile`]. The caller states the kind by the
//!    spelling it supplies; the planner never guesses beyond that.
//! 8. **Deterministic.** Steps are ordered by path bytes ascending, with the
//!    blocker and the assurance as tie-breakers, so permuting the input cannot
//!    change the plan.
//!
//! ## The assurance matrix
//!
//! | reported medium | assurance | blocker from the medium |
//! | --- | --- | --- |
//! | [`StorageMedium::SolidState`] | [`WipeAssurance::RequiresFullDiskSanitisation`] | [`WipeBlocker::MediumDefeatsOverwrite`] |
//! | [`StorageMedium::Magnetic`] | [`WipeAssurance::OverwritePlausible`] | none |
//! | [`StorageMedium::CopyOnWrite`] | [`WipeAssurance::RequiresFullDiskSanitisation`] | [`WipeBlocker::MediumDefeatsOverwrite`] |
//! | [`StorageMedium::Unknown`] | [`WipeAssurance::BestEffortOnly`] | [`WipeBlocker::MediumDefeatsOverwrite`] |
//!
//! A magnetic target is blocked only by a non-medium condition: possible
//! snapshots, an unknown size under [`WipeOptions::require_known_size`], a path
//! outside every allowed root, or a directory-shaped path.
//!
//! ### Blocker precedence
//!
//! A [`WipeStep`] carries at most one blocker, so the rules have a documented
//! order; the first match is the one reported:
//!
//! 1. [`WipeBlocker::OutsideAllowedRoots`]
//! 2. [`WipeBlocker::NotARegularFile`]
//! 3. [`WipeBlocker::MediumDefeatsOverwrite`]
//! 4. [`WipeBlocker::SnapshotsMayRetain`]
//! 5. [`WipeBlocker::UnknownSize`]
//!
//! Containment and shape come first because a path CoreSight is not authorized
//! to touch, or one that is not a regular file, is not a wipe candidate at all.
//! The medium verdict is not lost when a scope blocker wins: `assurance` is
//! derived from the medium alone, so a solid-state target still reports
//! [`WipeAssurance::RequiresFullDiskSanitisation`] even when it is also outside
//! every allowed root.
//!
//! ## What this plan does not prove
//!
//! The plan proves what the caller told it and nothing about the disk: the
//! bytes and the medium are inputs, not observations made here. It does not
//! prove that a path still names the same object when a future executor looks
//! at it, and it does not prove that a completed overwrite reached the original
//! cells on any medium. Only full-disk sanitisation (or physical destruction)
//! can be honestly described as effective on wear-levelled and copy-on-write
//! storage, and that is outside this crate.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The smallest pass count that still describes an overwrite at all.
///
/// A wipe with zero passes writes nothing, so planning it under a wipe label
/// would be a false claim; [`plan_wipe`] clamps `0` up to this value and
/// documents that rule on [`WipeOptions::passes`].
const MIN_PASSES: u32 = 1;

/// Why the planner refuses to promise an overwrite on solid-state media.
const SOLID_STATE_REASON: &str = "solid-state media (SSD/NVMe) remap and wear-level writes, so a software overwrite cannot be shown to reach the cells that held the original data";

/// Why the planner refuses to promise an overwrite on copy-on-write media.
const COPY_ON_WRITE_REASON: &str = "a copy-on-write filesystem (APFS, Btrfs, ZFS) writes new blocks and may retain the old ones, so a software overwrite cannot be shown to reach the original data";

/// Why the planner refuses to promise an overwrite when the medium is unknown.
const UNKNOWN_MEDIUM_REASON: &str = "the platform did not identify the medium, so neither wear levelling nor copy-on-write can be ruled out, and no overwrite claim is possible";

/// What every plan says about this build, independently of its targets.
const PLAN_NOTE: &str = "This build performs no wiping: no file is overwritten, truncated, deleted, or opened, and this plan is a review artefact only. Multi-pass overwrite is not a guarantee on wear-levelled (SSD/NVMe) or copy-on-write storage, because a write may never reach the original cells; on those media only full-disk sanitisation can be honestly described as effective";

/// The blocker tally in a fixed order, so the caveat text is deterministic.
const BLOCKERS_IN_ORDER: [WipeBlocker; 5] = [
    WipeBlocker::OutsideAllowedRoots,
    WipeBlocker::NotARegularFile,
    WipeBlocker::MediumDefeatsOverwrite,
    WipeBlocker::SnapshotsMayRetain,
    WipeBlocker::UnknownSize,
];

/// What a target sits on, as far as the caller could determine.
///
/// The distinction is the whole honesty argument of this module: whether a
/// software overwrite can plausibly reach the original cells depends on the
/// medium, not on the number of passes. The caller reports the medium because
/// this planner performs no observation of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageMedium {
    /// An SSD, NVMe device, or anything else with wear levelling.
    ///
    /// Writes are remapped by the controller, so an overwrite of a logical
    /// address is not an overwrite of a physical cell.
    SolidState,
    /// A rotational disk.
    ///
    /// The only medium where a software overwrite can plausibly reach the
    /// cells that held the data, because there is no remapping layer between
    /// the filesystem and the platter.
    Magnetic,
    /// Copy-on-write filesystem (APFS, Btrfs, ZFS) on any medium.
    ///
    /// The filesystem allocates new blocks for new data and may keep the old
    /// blocks until they are reused, so the write path never revisits the
    /// original blocks by design.
    CopyOnWrite,
    /// The platform could not tell us. Never assume magnetic.
    ///
    /// "Unknown" is an absence of evidence, and an absence of evidence is not
    /// evidence of a rotational disk. Treating it as [`StorageMedium::Magnetic`]
    /// would turn "we could not check" into a security claim, so the planner
    /// keeps it at [`WipeAssurance::BestEffortOnly`] and blocks it.
    Unknown,
}

impl fmt::Display for StorageMedium {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StorageMedium::SolidState => "solid-state",
            StorageMedium::Magnetic => "magnetic",
            StorageMedium::CopyOnWrite => "copy-on-write",
            StorageMedium::Unknown => "unknown",
        })
    }
}

/// What a wipe of one target could honestly claim.
///
/// An overwrite is never promoted to a guarantee, and a caller can only widen
/// the claim by changing the medium, not by adding passes. The variant order is
/// the deterministic sort order used to break ties between otherwise identical
/// steps; it is not a ranking of safety.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WipeAssurance {
    /// The tool cannot honestly claim the data is unrecoverable.
    ///
    /// Used when the medium is unknown: without knowing whether wear levelling
    /// or copy-on-write applies, any stronger wording would be a guess dressed
    /// up as a promise.
    BestEffortOnly {
        /// Why the strongest possible claim is only "best effort".
        reason: String,
    },
    /// Software overwrite on media where it can plausibly reach the original
    /// cells.
    ///
    /// "Plausible" is the honest ceiling: even on a rotational disk, bad
    /// sectors, caches, and the drive's own remapping can keep a copy. The pass
    /// count records what the caller asked for; it is not a security grade.
    OverwritePlausible {
        /// The effective overwrite pass count (at least one).
        passes: u32,
    },
    /// The only technique that can be honestly described as effective on this
    /// medium.
    ///
    /// On solid-state and copy-on-write storage a file-level overwrite cannot
    /// be shown to reach the original blocks, so the honest recommendation is
    /// full-disk sanitisation (or physical destruction), which is outside this
    /// crate's scope.
    RequiresFullDiskSanitisation {
        /// Why a file-level overwrite is not enough on this medium.
        reason: String,
    },
}

/// Why a step must not be carried out.
///
/// A blocker is a hard veto, never a warning: a step carrying one is not
/// actionable and contributes no bytes to [`WipePlan::total_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WipeBlocker {
    /// Solid-state or CoW media where overwrite cannot be proven effective.
    ///
    /// Reported for [`StorageMedium::SolidState`], [`StorageMedium::CopyOnWrite`],
    /// and [`StorageMedium::Unknown`]; see the module assurance matrix.
    MediumDefeatsOverwrite,
    /// Snapshots may retain the data after the file is gone.
    ///
    /// A filesystem snapshot is a second copy the live file's overwrite never
    /// touches, so even a perfect overwrite on magnetic media would not make
    /// the data unrecoverable.
    SnapshotsMayRetain,
    /// Size unknown, so no honest completion check is possible.
    ///
    /// Without a byte count the tool cannot say what "done" would mean, and a
    /// wipe that cannot be checked is a claim, not a result.
    UnknownSize,
    /// Path is outside every allowed root.
    ///
    /// Scope is decided before any medium argument: a path CoreSight is not
    /// authorized to touch is not a wipe candidate at all.
    OutsideAllowedRoots,
    /// Target is a directory.
    ///
    /// Only regular files are wipe candidates; a directory-shaped path is
    /// blocked rather than expanded into its contents, because expansion would
    /// be a different, unreviewed operation.
    NotARegularFile,
}

impl WipeBlocker {
    /// Stable machine-readable code for this blocker (UI and test contract).
    ///
    /// The codes are part of the review surface: a UI shows them next to the
    /// plain-language reason, and changing one is a user-visible change.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            WipeBlocker::MediumDefeatsOverwrite => "MEDIUM_DEFEATS_OVERWRITE",
            WipeBlocker::SnapshotsMayRetain => "SNAPSHOTS_MAY_RETAIN",
            WipeBlocker::UnknownSize => "UNKNOWN_SIZE",
            WipeBlocker::OutsideAllowedRoots => "OUTSIDE_ALLOWED_ROOTS",
            WipeBlocker::NotARegularFile => "NOT_A_REGULAR_FILE",
        }
    }

    /// Why this blocker exists, in plain language.
    ///
    /// The planner returns reasons with every veto instead of a bare boolean,
    /// so the user can see what made the step unsafe rather than having to
    /// trust that it was.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            WipeBlocker::MediumDefeatsOverwrite => {
                "the medium (solid-state, copy-on-write, or unidentified) defeats a software overwrite: a write may never reach the original cells"
            }
            WipeBlocker::SnapshotsMayRetain => {
                "the filesystem may hold snapshots that retain the data after the file itself is gone"
            }
            WipeBlocker::UnknownSize => {
                "the target's size is unknown, so no honest completion check is possible"
            }
            WipeBlocker::OutsideAllowedRoots => {
                "the path is outside every allowed root, or contains a parent-directory component that cannot be resolved without I/O"
            }
            WipeBlocker::NotARegularFile => {
                "the target is spelled as a directory; only regular files can be wipe candidates"
            }
        }
    }
}

/// One requested wipe target, as the caller describes it.
///
/// The planner never observes these facts itself; it is a pure function over
/// what the caller already knows. Every field is therefore an input that must
/// be reviewed, not a measurement this module can vouch for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeTarget {
    /// The path exactly as the caller spells it. It is not canonicalized and
    /// not resolved: the plan is bound to the spelling the user reviewed.
    pub path: PathBuf,
    /// The file's size in bytes, if known.
    ///
    /// `None` means *unknown*, never zero: zero is a legitimate size for an
    /// empty file. An unknown size prevents an honest completion check, which
    /// is why [`WipeOptions::require_known_size`] can veto the step and why it
    /// always makes [`WipePlan::total_bytes`] unknown.
    pub bytes: Option<u64>,
    /// True when the file is on a filesystem that may have snapshots.
    ///
    /// A snapshot keeps the old blocks after the live file is gone, so no
    /// overwrite of the live file can reach that copy. The flag is conservative
    /// on purpose: when in doubt, say yes.
    pub may_have_snapshots: bool,
}

/// One planned step: a target, what its medium would allow, and any veto.
///
/// A blocked step is still emitted. Dropping it would hide the fact that the
/// user asked about it, and the plan's job is to show every request with its
/// verdict attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeStep {
    /// The target path this step is about, exactly as supplied.
    pub target: PathBuf,
    /// What the medium would allow for this target.
    ///
    /// The assurance is derived from the medium alone (plus the effective pass
    /// count), and is reported even when the step is blocked, so the medium's
    /// capability is never confused with the reason the step cannot run.
    pub assurance: WipeAssurance,
    /// The veto that stops this step, if any.
    ///
    /// `Some(_)` means the step must not be carried out by any layer, because
    /// the planner already knows it cannot be done honestly.
    pub blocked: Option<WipeBlocker>,
}

/// A complete, reviewable wipe plan.
///
/// This is a review artefact, not an execution token: the crate has no wipe
/// primitive, and no adapter may treat this value as authorization to write to
/// a disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipePlan {
    /// One step per requested target, ordered by path bytes ascending.
    ///
    /// Blocked targets stay in the list: the user asked about them, and
    /// dropping them would hide the veto instead of explaining it.
    pub steps: Vec<WipeStep>,
    /// How many steps carry a blocker.
    ///
    /// Always `<= steps.len()`; the difference is the set of targets the
    /// medium and the conditions would actually allow.
    pub blocked_count: usize,
    /// Saturating sum of the sizes of the steps no blocker stopped.
    ///
    /// Blocked targets contribute no bytes: they will not be wiped by this
    /// plan, and publishing their size would read as capacity to act. The value
    /// is `None` as soon as **any** requested target has an unknown size —
    /// blocked or not — because a total that silently omits an unknown quantity
    /// is not a total. An empty plan has `Some(0)`.
    pub total_bytes: Option<u64>,
    /// Present whenever at least one target is blocked or has an unknown size.
    ///
    /// The caveat names how many targets are affected and which blockers
    /// applied, so a caller cannot render the plan as if it were clean.
    pub caveat: Option<String>,
    /// Plain-language statement of what this build does and does not do.
    ///
    /// Always present, including on a clean plan: the honesty note is a
    /// property of the build, not of the targets.
    pub note: String,
}

/// Knobs for [`plan_wipe`]. Defaults are conservative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeOptions {
    /// How many overwrite passes the caller wants.
    ///
    /// `0` is meaningless — a wipe that writes nothing is not a wipe — so it is
    /// clamped up to `1` rather than planned as a step that carries a wipe
    /// label while doing nothing. A larger number is recorded faithfully, but
    /// it never strengthens [`WipeAssurance`]: on wear-levelled and
    /// copy-on-write media more passes do not reach more cells.
    pub passes: u32,
    /// Whether a target whose size is unknown must be blocked.
    ///
    /// Defaults to `true`. Without a known size there is no honest completion
    /// check, so the step is blocked with [`WipeBlocker::UnknownSize`]; turning
    /// this off leaves the step unblocked but still makes
    /// [`WipePlan::total_bytes`] unknown.
    pub require_known_size: bool,
}

impl Default for WipeOptions {
    /// One pass and known sizes required: the smallest honest configuration.
    fn default() -> Self {
        WipeOptions {
            passes: 1,
            require_known_size: true,
        }
    }
}

/// Whether CoreSight supports jv16's "fake files for plausible deniability".
///
/// Always `false`. The feature is declined, not postponed:
///
/// * **It produces deceptive artefacts.** Writing decoy files is a deceptive
///   act, and a safety-first tool should not manufacture evidence of activity
///   that did not happen.
/// * **It offers no cryptographic benefit.** A decoy does not change the
///   recoverability of the original data and is not encryption; the original
///   blocks are exactly as recoverable as they were before.
/// * **It could be mistaken for data protection.** A user who believes decoys
///   protect them may skip the measure that would actually help — encryption
///   at rest, or full-disk sanitisation.
///
/// The function exists so callers (UI, capability negotiation, tests) can
/// branch on the refusal explicitly. It is deliberately not a stub for a future
/// implementation, and there is no other entry point that produces fake files.
#[must_use]
pub fn plausible_deniability_supported() -> bool {
    false
}

/// Build a wipe plan that states what could and could not be guaranteed.
///
/// The function removes nothing and overwrites nothing: it is a pure mapping
/// from the caller's facts to a reviewable plan, and it performs no I/O.
///
/// * `targets` — the files the user asked about, with whatever size and
///   snapshot knowledge the caller has.
/// * `medium` — what the caller determined the targets live on. It decides the
///   [`WipeAssurance`]; it is never inferred.
/// * `allowed_roots` — the only path prefixes CoreSight may act within. A
///   target outside all of them is blocked, and so is any target containing a
///   parent-directory component, because resolving one would require I/O.
/// * `options` — pass count and size policy; a pass count of `0` is clamped to
///   one.
///
/// Steps come back ordered by path bytes ascending and the result is
/// deterministic: permuting `targets` cannot change the plan. The function is
/// infallible — every target produces a step, possibly a blocked one — because
/// refusing to plan a target the user asked about would hide it.
///
/// # Examples
///
/// A solid-state target cannot honestly be overwritten, so it is blocked and
/// points at sanitisation instead:
///
/// ```
/// use coresight_cleanup::{plan_wipe, StorageMedium, WipeOptions, WipeTarget};
/// use std::path::PathBuf;
///
/// let plan = plan_wipe(
///     &[WipeTarget {
///         path: PathBuf::from("/data/notes.txt"),
///         bytes: Some(64),
///         may_have_snapshots: false,
///     }],
///     StorageMedium::SolidState,
///     &[PathBuf::from("/data")],
///     &WipeOptions::default(),
/// );
///
/// assert_eq!(plan.blocked_count, 1);
/// assert_eq!(plan.total_bytes, Some(0));
/// ```
#[must_use]
pub fn plan_wipe(
    targets: &[WipeTarget],
    medium: StorageMedium,
    allowed_roots: &[PathBuf],
    options: &WipeOptions,
) -> WipePlan {
    let passes = options.passes.max(MIN_PASSES);
    let mut steps: Vec<WipeStep> = Vec::with_capacity(targets.len());
    let mut actionable_bytes = 0u64;
    let mut unknown_sizes = 0usize;

    for target in targets {
        let blocked = blocker_for(target, medium, allowed_roots, options);
        if target.bytes.is_none() {
            unknown_sizes = unknown_sizes.saturating_add(1);
        }
        if blocked.is_none() {
            if let Some(bytes) = target.bytes {
                actionable_bytes = actionable_bytes.saturating_add(bytes);
            }
        }
        steps.push(WipeStep {
            target: target.path.clone(),
            assurance: assurance_for(medium, passes),
            blocked,
        });
    }

    steps.sort_by(|a, b| {
        path_bytes(&a.target)
            .cmp(path_bytes(&b.target))
            .then_with(|| a.blocked.cmp(&b.blocked))
            .then_with(|| a.assurance.cmp(&b.assurance))
    });

    let blocked_count = steps.iter().filter(|step| step.blocked.is_some()).count();
    let total_bytes = if unknown_sizes == 0 {
        Some(actionable_bytes)
    } else {
        None
    };

    WipePlan {
        caveat: caveat_for(&steps, blocked_count, unknown_sizes),
        steps,
        blocked_count,
        total_bytes,
        note: plan_note(medium),
    }
}

/// The assurance a medium can honestly offer, with a pass count of at least one.
///
/// The mapping is deliberately total: there is no fall-through arm that
/// defaults to "plausible", because a default would be exactly the silent
/// upgrade this module exists to prevent.
fn assurance_for(medium: StorageMedium, passes: u32) -> WipeAssurance {
    match medium {
        StorageMedium::Magnetic => WipeAssurance::OverwritePlausible { passes },
        StorageMedium::SolidState => WipeAssurance::RequiresFullDiskSanitisation {
            reason: SOLID_STATE_REASON.to_string(),
        },
        StorageMedium::CopyOnWrite => WipeAssurance::RequiresFullDiskSanitisation {
            reason: COPY_ON_WRITE_REASON.to_string(),
        },
        StorageMedium::Unknown => WipeAssurance::BestEffortOnly {
            reason: UNKNOWN_MEDIUM_REASON.to_string(),
        },
    }
}

/// The first applicable veto for one target, in the documented precedence.
///
/// Order matters because a step carries one blocker: scope and shape are
/// checked first (a path we may not touch, or one that is not a file, is not a
/// wipe candidate at all), then the medium, then the conditions that can
/// outlive a perfect overwrite.
fn blocker_for(
    target: &WipeTarget,
    medium: StorageMedium,
    allowed_roots: &[PathBuf],
    options: &WipeOptions,
) -> Option<WipeBlocker> {
    if !is_within_allowed_roots(&target.path, allowed_roots) {
        return Some(WipeBlocker::OutsideAllowedRoots);
    }
    if looks_like_directory(&target.path) {
        return Some(WipeBlocker::NotARegularFile);
    }
    if medium != StorageMedium::Magnetic {
        return Some(WipeBlocker::MediumDefeatsOverwrite);
    }
    if target.may_have_snapshots {
        return Some(WipeBlocker::SnapshotsMayRetain);
    }
    if options.require_known_size && target.bytes.is_none() {
        return Some(WipeBlocker::UnknownSize);
    }
    None
}

/// Whether `path` is component-wise inside at least one allowed root.
///
/// `Path::starts_with` is component-wise, so `/allowed-extra` is not inside
/// `/allowed`. Neither the path nor the roots are canonicalized (that needs
/// I/O), and the comparison is case-sensitive on every platform, so the caller
/// must spell a root exactly as it spells the targets beneath it. A path
/// containing `..` is refused outright: without resolving it components such
/// as `/allowed/../elsewhere` would otherwise pass the prefix check while
/// escaping the root.
fn is_within_allowed_roots(path: &Path, allowed_roots: &[PathBuf]) -> bool {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return false;
    }
    allowed_roots.iter().any(|root| path.starts_with(root))
}

/// Whether the caller spelled this path as a directory.
///
/// The planner cannot stat the disk, and [`WipeTarget`] carries no kind, so a
/// trailing separator is the only signal available. Both separators are
/// recognised because a Windows caller may spell either and the plan must not
/// change meaning with the platform's mood. This is a spelling convention, not
/// proof of the object's kind: the blocker reports what the caller stated, not
/// what the filesystem holds.
fn looks_like_directory(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    bytes.ends_with(b"/") || bytes.ends_with(b"\\")
}

/// A path's bytes, the workspace's canonical ordering key for paths.
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// The caveat text for a plan, or `None` when nothing needs qualifying.
///
/// The text is built from counts and stable blocker codes only, so two plans
/// built from the same facts produce byte-identical caveats.
fn caveat_for(steps: &[WipeStep], blocked_count: usize, unknown_sizes: usize) -> Option<String> {
    if blocked_count == 0 && unknown_sizes == 0 {
        return None;
    }
    let total = steps.len();
    let mut clauses: Vec<String> = Vec::new();
    if blocked_count > 0 {
        clauses.push(format!(
            "{blocked_count} of {total} target(s) are blocked and cannot be wiped by this plan"
        ));
    }
    if unknown_sizes > 0 {
        clauses.push(format!(
            "{unknown_sizes} of {total} target(s) have an unknown size, so no complete byte total can be stated"
        ));
    }
    let tally: Vec<String> = BLOCKERS_IN_ORDER
        .iter()
        .filter_map(|blocker| {
            let count = steps
                .iter()
                .filter(|step| step.blocked == Some(*blocker))
                .count();
            if count == 0 {
                None
            } else {
                Some(format!("{count}x{}", blocker.code()))
            }
        })
        .collect();
    if !tally.is_empty() {
        clauses.push(format!("blockers: {}", tally.join(", ")));
    }
    let mut text = clauses.join("; ");
    text.push('.');
    Some(text)
}

/// The note every plan carries, with the reported medium appended.
fn plan_note(medium: StorageMedium) -> String {
    format!("{PLAN_NOTE}. Medium reported for this plan: {medium}.")
}
