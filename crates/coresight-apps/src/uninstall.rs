//! Software-uninstall planning from evidence — jv16 PowerTools tool #3.
//!
//! jv16's observation is correct: the Windows Control Panel list is only what
//! installers chose to report, and Windows delegates removal to each vendor's
//! own uninstaller, so data is routinely left behind. The dishonest answer is
//! to claim CoreSight can run a better uninstaller, or to sweep name
//! coincidences into a "leftovers" list and delete them. This module does
//! neither. It reconstructs candidate leftovers from evidence the inventory
//! already recorded, states what connects each one, and separates the
//! reversible-looking from the dangerous.
//!
//! ## What a plan is
//!
//! A [`UninstallPlan`] is a *proposal for inspection*, not an execution
//! object. It names every candidate, the evidence behind it, a tier, and a
//! one-sentence explanation. [`plan_action`] can only return
//! [`UninstallAction::InspectOnly`]: this build removes nothing. Any future
//! execution must run the recorded vendor uninstaller first and then feed the
//! leftovers through the typed safety pipeline; it must never delete these
//! paths directly. The `note` field states this in every plan, and it also
//! states that a recorded vendor command was **not** run.
//!
//! ## Tier rules (the whole safety model)
//!
//! | Evidence | Tier | Rule |
//! |---|---|---|
//! | [`LeftoverEvidence::Contended`] | `OffLimits` | Always. Another installed app claims the path; removing it breaks that app. |
//! | any path evidence | `OffLimits` | The path lexically resolves into a supplied system root. |
//! | any path evidence | `OffLimits` | The path lexically resolves outside the user's home **and** outside the declared install root. |
//! | [`LeftoverEvidence::InsideInstallRoot`] | `Safe` | The path is strictly inside the declared root and is not equal to it. |
//! | [`LeftoverEvidence::InsideInstallRoot`] | `Review` | The path is the declared install root itself. The root boundary is never safe-tier. |
//! | [`LeftoverEvidence::InsideInstallRoot`] | `OffLimits` | The path is not actually inside the declared root, or no root was supplied. Inconsistent evidence fails closed. |
//! | [`LeftoverEvidence::MatchesBundleId`] | `Review` | A recorded bundle identifier connects the path to the app. |
//! | [`LeftoverEvidence::MatchesUninstallKey`] | `Review` | A recorded uninstall key connects it. Registry keys are not filesystem paths, so the path scope checks do not apply. |
//! | [`LeftoverEvidence::InsideAppDataRoot`] | `Review` | A recorded manifest attributes the path to the app under a known per-user data root. |
//! | [`LeftoverEvidence::HeuristicNameMatch`] | `Review` | Name match only. The `why` text always says that this is **not recorded evidence**. |
//!
//! The install root is the one explicit exception to the home boundary: on
//! Windows it is normally `C:\Program Files\...`, outside the home, and
//! "strictly inside the declared root" would be meaningless if the home rule
//! overrode it. The root was recorded by the inventory itself, so its strict
//! children are the app's install content. A heuristic name match is still
//! subject to the scope rule: it stays `Review` only inside the home or the
//! declared root, and is `OffLimits` anywhere else. The first three veto rows
//! are checked before the per-evidence tier, so they can move any item to
//! `OffLimits`; no scope check can ever move an item up to `Safe`.
//!
//! ## Rules that keep the plan honest
//!
//! * **Unknown is not zero.** [`UninstallPlan::total_bytes`] is `None` when
//!   any proposed item's size is unknown. [`UninstallPlan::safe_bytes`] adds
//!   only `Safe` items whose size is known, and saturates.
//! * **Off-limits items are kept, not hidden.** They move to
//!   [`UninstallPlan::off_limits_kept`] with their reasons. They are not part
//!   of the proposal, so they do not contribute to its byte totals.
//! * **One path, one decision.** Candidate records that normalize to the same
//!   path are merged. The most severe tier wins, and contention vetoes every
//!   weaker claim on that path. Conflicting or missing sizes make the merged
//!   item's size unknown rather than picking one.
//! * **Deterministic.** Proposed items are ordered by `(tier severity, path
//!   bytes ascending)`, with a stable tie-break; off-limits items are ordered
//!   by path bytes. Permuting the input cannot change the plan.
//! * **Saturating arithmetic.** Byte sums never wrap.
//! * **No side effects.** This module performs no I/O, no process creation, no
//!   network access, and no registry access. It is a planner over supplied
//!   evidence. The integration test suite enforces that with a source scan.
//!
//! ## What this module cannot prove
//!
//! Path normalization is lexical only. It handles `..` and `.` without
//! touching the disk; it does not resolve symlinks, junctions, or mount
//! points. A path that looks inside the home can still be a link to a system
//! directory, and only an execution-time check (against the same protection
//! domains as the cleanup pipeline) can catch that. Tiering is also per item:
//! the planner does not prove that a `Safe` directory contains no `OffLimits`
//! descendant. Both gaps are why this build is inspection-only and why any
//! future executor must re-validate the concrete target set.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pathmatch::ascii_eq_ignore_case;

/// Why a candidate leftover is attributed to the application.
///
/// The variant *is* the strength claim. [`LeftoverEvidence::HeuristicNameMatch`]
/// exists so a name coincidence can be shown and reviewed **without ever
/// being promoted** into a recorded-evidence category. Downstream code can
/// match on this type; it must not flatten the variants into one "leftover"
/// bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum LeftoverEvidence {
    /// Found inside the declared install root.
    ///
    /// This is the only evidence that can reach `Safe`, and only for a strict
    /// child. The root itself is a boundary, not a safe target.
    InsideInstallRoot {
        /// The path exactly as observed.
        path: PathBuf,
    },
    /// Matched a bundle identifier / package name recorded at install time.
    MatchesBundleId {
        /// The path exactly as observed.
        path: PathBuf,
        /// The recorded identifier that matched, e.g. `com.vendor.App`.
        bundle_id: String,
    },
    /// Matched an uninstall registry key recorded at install time.
    MatchesUninstallKey {
        /// The key path exactly as recorded, e.g.
        /// `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\{...}`.
        key_path: String,
    },
    /// Under a known per-user data root, attributed to this app by a recorded
    /// manifest.
    InsideAppDataRoot {
        /// The path exactly as observed.
        path: PathBuf,
        /// The data root's label, e.g. `Roaming` or `Local`.
        root: String,
    },
    /// A shared path another installed app also claims.
    ///
    /// Contention is a veto, not a warning: the item is always `OffLimits`.
    Contended {
        /// The path exactly as observed.
        path: PathBuf,
        /// The other installed applications that claim it.
        other_apps: Vec<String>,
    },
    /// Named like the app but no recorded evidence connects it.
    ///
    /// This is the dangerous category. It is always `Review`, and its reason
    /// always says the link is a name match only and not recorded evidence.
    HeuristicNameMatch {
        /// The path exactly as observed.
        path: PathBuf,
    },
}

impl LeftoverEvidence {
    /// The path (or registry-key path) this evidence points at.
    ///
    /// A plan item needs one uniform target slot for both filesystem paths and
    /// registry keys. For [`LeftoverEvidence::MatchesUninstallKey`] the
    /// recorded key path is placed in that slot; callers must not treat it as
    /// a filesystem path.
    pub fn target_path(&self) -> PathBuf {
        match self {
            LeftoverEvidence::InsideInstallRoot { path }
            | LeftoverEvidence::MatchesBundleId { path, .. }
            | LeftoverEvidence::InsideAppDataRoot { path, .. }
            | LeftoverEvidence::Contended { path, .. }
            | LeftoverEvidence::HeuristicNameMatch { path } => path.clone(),
            LeftoverEvidence::MatchesUninstallKey { key_path } => PathBuf::from(key_path.as_str()),
        }
    }

    /// A stable rank used only to break ties deterministically when several
    /// records describe the same path.
    fn rank(&self) -> u8 {
        match self {
            LeftoverEvidence::InsideInstallRoot { .. } => 0,
            LeftoverEvidence::MatchesBundleId { .. } => 1,
            LeftoverEvidence::MatchesUninstallKey { .. } => 2,
            LeftoverEvidence::InsideAppDataRoot { .. } => 3,
            LeftoverEvidence::HeuristicNameMatch { .. } => 4,
            LeftoverEvidence::Contended { .. } => 5,
        }
    }
}

/// One candidate supplied to the planner, before tiering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverCandidate {
    /// What connects the path to the app.
    pub evidence: LeftoverEvidence,
    /// Size in bytes when known. `None` means "not measured", never zero.
    #[serde(default)]
    pub bytes: Option<u64>,
}

/// The scopes a plan is checked against.
///
/// All values are supplied by the caller so the planner stays pure and
/// platform-neutral. `None` home means "the home boundary is unknown"; only
/// the declared install root (and registry-key evidence) can then be in scope.
/// An empty [`UninstallScope::system_roots`] supplies no system-root veto, so
/// callers must pass the platform's protected roots rather than relying on
/// this module to guess them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallScope {
    /// The current user's home directory, when known. It must be an absolute
    /// path; an empty or relative value is ignored (fail closed) rather than
    /// treated as a root that contains everything.
    pub home_dir: Option<PathBuf>,
    /// The install root the inventory recorded for this specific app. It must
    /// be an absolute path for the same reason.
    pub install_root: Option<PathBuf>,
    /// Filesystem roots that are never actionable (OS install dirs, system
    /// directories, and whatever else the caller's protection policy names).
    pub system_roots: Vec<PathBuf>,
}

/// Everything needed to build one plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallRequest {
    /// Stable application id from the inventory.
    pub app_id: String,
    /// Human display name from the inventory.
    pub display_name: String,
    /// The scopes this app's items are judged against.
    #[serde(default)]
    pub scope: UninstallScope,
    /// The vendor uninstall command exactly as recorded. `None` means **not
    /// recorded**, not "none needed"; `Some("")` is a recorded empty command
    /// and is kept distinct from `None`.
    #[serde(default)]
    pub vendor_uninstaller: Option<String>,
    /// Candidate leftovers from the evidence layer.
    #[serde(default)]
    pub candidates: Vec<LeftoverCandidate>,
}

/// Safety tier for one uninstall-plan item.
///
/// Declaration order is the severity order used by the deterministic sort:
/// `Safe` < `Review` < `OffLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UninstallItemTier {
    /// Reversible-looking app-owned install content: a strict child of the
    /// declared install root. Still inspection-only in this build.
    Safe,
    /// Connected by recorded evidence, but not safe-tier on its own; a human
    /// must read the reason.
    Review,
    /// Never proposed. Kept in `off_limits_kept` so it is visible with its
    /// reason instead of silently disappearing.
    OffLimits,
}

/// One candidate leftover, with the evidence and explanation attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallItem {
    /// The filesystem path, or the registry key path for
    /// [`LeftoverEvidence::MatchesUninstallKey`].
    pub path: PathBuf,
    /// Known size in bytes. `None` means the size could not be measured.
    pub bytes: Option<u64>,
    /// The evidence that produced this item.
    pub evidence: LeftoverEvidence,
    /// The tier assigned by the rules in the module docs.
    pub tier: UninstallItemTier,
    /// One sentence naming the evidence. A [`LeftoverEvidence::HeuristicNameMatch`]
    /// reason always says the link is a name match only, not recorded
    /// evidence.
    pub why: String,
}

/// A complete, inspection-only uninstall plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPlan {
    /// Equal to [`UninstallRequest::app_id`].
    pub app_id: String,
    /// Equal to [`UninstallRequest::display_name`].
    pub display_name: String,
    /// Proposed items (`Safe` and `Review`), ordered by
    /// `(tier severity, path bytes ascending)`.
    pub items: Vec<UninstallItem>,
    /// `None` when **any proposed item** has an unknown size; otherwise the
    /// saturating sum of known sizes. A partial total is never presented as
    /// complete.
    pub total_bytes: Option<u64>,
    /// Saturating sum of known sizes of `Safe` items only.
    pub safe_bytes: u64,
    /// Number of proposed items whose size is unknown.
    pub unknown_size_items: usize,
    /// Items kept for transparency because removal was vetoed. They are not
    /// part of the proposal and do not contribute to its totals.
    pub off_limits_kept: Vec<UninstallItem>,
    /// The vendor uninstall command exactly as recorded. Never executed by
    /// this build; `None` reads as "not recorded", never as "none needed".
    pub vendor_uninstaller: Option<String>,
    /// Plain-language statement of what this build will not do and what any
    /// future execution must do first.
    pub note: String,
}

/// How a plan's [`UninstallPlan::vendor_uninstaller`] value must be read.
///
/// The three states are deliberately distinct: a missing record is not the
/// same fact as a record that happens to be blank, and neither one is evidence
/// that the vendor ships no uninstaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VendorUninstallerStatus {
    /// No command was recorded. This is "not recorded", **not** "none needed".
    NotRecorded,
    /// A command was recorded but it is empty after trimming. Recorded, but
    /// not a runnable command.
    RecordedEmpty,
    /// A non-empty command was recorded. It has not been run.
    Recorded,
}

impl UninstallPlan {
    /// Read the recorded vendor command honestly.
    ///
    /// `None` is [`VendorUninstallerStatus::NotRecorded`] and must never be
    /// rendered as "none needed". `Some("")` is
    /// [`VendorUninstallerStatus::RecordedEmpty`] and must never be folded
    /// into `None`.
    pub fn vendor_uninstaller_status(&self) -> VendorUninstallerStatus {
        match self.vendor_uninstaller.as_deref() {
            None => VendorUninstallerStatus::NotRecorded,
            Some(command) if command.trim().is_empty() => VendorUninstallerStatus::RecordedEmpty,
            Some(_) => VendorUninstallerStatus::Recorded,
        }
    }
}

/// What a plan authorizes doing.
///
/// The enum exists so the answer is typed rather than implied. In this build
/// there is exactly one variant: nothing is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UninstallAction {
    /// Inspect the plan only. No deletion, move, uninstall, or registry
    /// change is authorized.
    InspectOnly,
}

/// The action a plan authorizes.
///
/// Always [`UninstallAction::InspectOnly`]. This is not a policy check that a
/// flag could flip; this build has no execution primitive, and the note in
/// every plan says any future execution must run the vendor uninstaller first
/// and then propose the leftovers through the typed safety pipeline.
pub fn plan_action(_plan: &UninstallPlan) -> UninstallAction {
    UninstallAction::InspectOnly
}

/// Build an inspection-only uninstall plan from supplied evidence.
///
/// Pure: reads only `request`, never the filesystem, process table, network,
/// or registry. Items are merged by lexical path, vetoed by contention, tiered
/// by the rules in the module docs, and ordered deterministically.
pub fn plan_uninstall(request: &UninstallRequest) -> UninstallPlan {
    let mut groups: BTreeMap<Vec<u8>, Vec<UninstallItem>> = BTreeMap::new();

    for candidate in &request.candidates {
        let (tier, why) = classify(&candidate.evidence, &request.scope);
        let item = UninstallItem {
            path: candidate.evidence.target_path(),
            bytes: candidate.bytes,
            evidence: candidate.evidence.clone(),
            tier,
            why,
        };
        groups.entry(path_key(&item.path)).or_default().push(item);
    }

    let mut items: Vec<UninstallItem> = Vec::new();
    let mut off_limits_kept: Vec<UninstallItem> = Vec::new();

    for (_, mut group) in groups {
        group.sort_by(compare_items);
        if let Some(merged) = merge_group(group) {
            if merged.tier == UninstallItemTier::OffLimits {
                off_limits_kept.push(merged);
            } else {
                items.push(merged);
            }
        }
    }

    items.sort_by(compare_items);
    off_limits_kept.sort_by(compare_items);

    let mut total_bytes: Option<u64> = Some(0);
    let mut safe_bytes: u64 = 0;
    let mut unknown_size_items: usize = 0;

    for item in &items {
        match item.bytes {
            Some(bytes) => {
                if let Some(running) = total_bytes {
                    total_bytes = Some(running.saturating_add(bytes));
                }
                if item.tier == UninstallItemTier::Safe {
                    safe_bytes = safe_bytes.saturating_add(bytes);
                }
            }
            None => {
                unknown_size_items = unknown_size_items.saturating_add(1);
                // One unknown size makes the total unknown; it must never be
                // presented as a complete sum of the known ones.
                total_bytes = None;
            }
        }
    }

    let note = build_note(request.vendor_uninstaller.as_deref(), off_limits_kept.len());

    UninstallPlan {
        app_id: request.app_id.clone(),
        display_name: request.display_name.clone(),
        items,
        total_bytes,
        safe_bytes,
        unknown_size_items,
        off_limits_kept,
        vendor_uninstaller: request.vendor_uninstaller.clone(),
        note,
    }
}

/// Merge the records that describe one path.
///
/// Returns `None` only for an empty group, which cannot be produced by
/// [`plan_uninstall`]. The most severe tier wins: one `OffLimits` record
/// vetoes every weaker record at the same path, and one `Contended` record
/// vetoes everything (its other-app list is unioned). Bytes are merged
/// honestly: missing or conflicting measurements become `None`.
fn merge_group(group: Vec<UninstallItem>) -> Option<UninstallItem> {
    let bytes = merged_bytes(&group);
    let contended: Vec<&UninstallItem> = group
        .iter()
        .filter(|item| matches!(item.evidence, LeftoverEvidence::Contended { .. }))
        .collect();

    if !contended.is_empty() {
        let mut other_apps: Vec<String> = Vec::new();
        for item in &contended {
            if let LeftoverEvidence::Contended {
                other_apps: names, ..
            } = &item.evidence
            {
                other_apps.extend(names.iter().cloned());
            }
        }
        other_apps.sort();
        other_apps.dedup();
        let path = group
            .iter()
            .map(|item| item.path.clone())
            .min_by(|a, b| path_bytes(a).cmp(path_bytes(b)))?;
        return Some(UninstallItem {
            path: path.clone(),
            bytes,
            evidence: LeftoverEvidence::Contended {
                path,
                other_apps: other_apps.clone(),
            },
            tier: UninstallItemTier::OffLimits,
            why: contended_why(&other_apps),
        });
    }

    group
        .into_iter()
        .reduce(|best, item| {
            if compare_items(&item, &best) == std::cmp::Ordering::Greater {
                item
            } else {
                best
            }
        })
        .map(|mut merged| {
            // The group's merged size overrides whichever record was selected as
            // the explanation, so conflicting or missing measurements stay
            // unknown.
            merged.bytes = bytes;
            merged
        })
}

/// Merge sizes for one path. Any `None`, or two different `Some` values,
/// makes the result unknown; picking one measurement would fabricate
/// certainty.
fn merged_bytes(items: &[UninstallItem]) -> Option<u64> {
    let mut value: Option<u64> = None;
    for item in items {
        let bytes = item.bytes?;
        match value {
            None => value = Some(bytes),
            Some(existing) if existing == bytes => {}
            Some(_) => return None,
        }
    }
    value
}

/// Canonical item order: tier, then path bytes, then a stable tie-break.
///
/// The first two keys are the contract. The remaining keys only make the
/// result independent of input order when two records describe the same path
/// with the same tier.
fn compare_items(a: &UninstallItem, b: &UninstallItem) -> std::cmp::Ordering {
    a.tier
        .cmp(&b.tier)
        .then_with(|| path_bytes(&a.path).cmp(path_bytes(&b.path)))
        .then_with(|| evidence_rank(a).cmp(&evidence_rank(b)))
        .then_with(|| a.why.cmp(&b.why))
}

fn evidence_rank(item: &UninstallItem) -> u8 {
    item.evidence.rank()
}

/// A normalized, ASCII-case-folded key for path-identity grouping.
///
/// Two spellings that differ only in `.`/`..` segments or ASCII case describe
/// the same target on the platforms this inventory serves. Grouping them
/// prevents double-counting bytes and, more importantly, prevents a
/// contention veto from being bypassed by a second spelling of the same path.
fn path_key(path: &Path) -> Vec<u8> {
    normalize_lexically(path)
        .as_os_str()
        .as_encoded_bytes()
        .to_ascii_lowercase()
}

/// The workspace's canonical byte ordering key for a path.
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// Lexically resolve `.` and `..` without touching the filesystem.
///
/// This deliberately does not resolve links, junctions, or mount points; the
/// module docs state that limitation. A `..` at a root is clamped, as the OS
/// does, so a path cannot escape upward by spelling.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut depth: usize = 0;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => {
                out.push(component.as_os_str());
                depth = 0;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if depth > 0 {
                    out.pop();
                    depth -= 1;
                } else if !out.has_root() {
                    out.push(component.as_os_str());
                }
            }
            Component::Normal(part) => {
                out.push(part);
                depth += 1;
            }
        }
    }

    out
}

/// Compare two path components under ASCII case folding.
///
/// The inventory can see the same component spelled with different case
/// across Windows sources. Folding ASCII keeps the system-root veto from being
/// bypassed by spelling. Non-ASCII bytes must match exactly; they never fold.
fn component_eq(a: Component<'_>, b: Component<'_>) -> bool {
    ascii_eq_ignore_case(
        a.as_os_str().as_encoded_bytes(),
        b.as_os_str().as_encoded_bytes(),
    )
}

/// An empty or relative "root" is not an anchor: treating `""` as a root would
/// make every path "inside" it. Production callers must supply absolute
/// paths; the drive-letter check keeps the synthetic Windows-style roots used
/// by the portable test matrix working on non-Windows hosts.
fn is_anchor(path: &Path) -> bool {
    if path.has_root() {
        return true;
    }
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(first)), Some(_)) => {
            let bytes = first.as_encoded_bytes();
            bytes.len() == 2
                && bytes.first().is_some_and(|byte| byte.is_ascii_alphabetic())
                && bytes.get(1) == Some(&b':')
        }
        _ => false,
    }
}

/// True when `path` is `root` or lies beneath it, component-wise, after
/// lexical normalization and ASCII case folding.
fn path_within_folded(root: &Path, path: &Path) -> bool {
    let root = normalize_lexically(root);
    if !is_anchor(&root) {
        return false;
    }
    let path = normalize_lexically(path);
    let mut root_components = root.components();
    let mut path_components = path.components();

    loop {
        match (root_components.next(), path_components.next()) {
            (Some(root_part), Some(path_part)) => {
                if !component_eq(root_part, path_part) {
                    return false;
                }
            }
            (None, _) => return true,
            (Some(_), None) => return false,
        }
    }
}

/// True when the two paths denote the same lexical target under the same
/// folding rules.
fn paths_equal_folded(a: &Path, b: &Path) -> bool {
    path_within_folded(a, b) && path_within_folded(b, a)
}

/// Tier one evidence record against the supplied scopes.
fn classify(evidence: &LeftoverEvidence, scope: &UninstallScope) -> (UninstallItemTier, String) {
    match evidence {
        LeftoverEvidence::Contended { other_apps, .. } => {
            (UninstallItemTier::OffLimits, contended_why(other_apps))
        }
        LeftoverEvidence::MatchesUninstallKey { key_path } => (
            UninstallItemTier::Review,
            format!("Matches the uninstall registry key \"{key_path}\" recorded at install time."),
        ),
        _ => classify_path_evidence(evidence, scope),
    }
}

/// Tier one filesystem-path evidence record. Contention is handled before this
/// function; the wildcard fails closed if a future variant is added without a
/// rule.
fn classify_path_evidence(
    evidence: &LeftoverEvidence,
    scope: &UninstallScope,
) -> (UninstallItemTier, String) {
    let resolved = normalize_lexically(&evidence.target_path());

    if let Some(system_root) = scope
        .system_roots
        .iter()
        .find(|root| path_within_folded(root, &resolved))
    {
        return (
            UninstallItemTier::OffLimits,
            format!(
                "{}; kept off-limits because it resolves into the system root \"{}\".",
                evidence_clause(evidence),
                system_root.display()
            ),
        );
    }

    let inside_home = scope
        .home_dir
        .as_ref()
        .is_some_and(|home| path_within_folded(home, &resolved));
    let inside_install_root = scope
        .install_root
        .as_ref()
        .is_some_and(|root| path_within_folded(root, &resolved));

    if !inside_home && !inside_install_root {
        let reason = if scope.home_dir.is_none() {
            "no user home was supplied and the path is outside the declared install root"
        } else {
            "the path resolves outside the user's home and outside the declared install root"
        };
        return (
            UninstallItemTier::OffLimits,
            format!(
                "{}; kept off-limits because {reason}.",
                evidence_clause(evidence)
            ),
        );
    }

    match evidence {
        LeftoverEvidence::InsideInstallRoot { .. } => match &scope.install_root {
            None => (
                UninstallItemTier::OffLimits,
                format!(
                    "{}; kept off-limits because no declared install root was supplied to check it against.",
                    evidence_clause(evidence)
                ),
            ),
            Some(root) if paths_equal_folded(root, &resolved) => (
                UninstallItemTier::Review,
                "This is the declared install root itself (InsideInstallRoot evidence), not a strict child; the root boundary is reviewed, never safe-tier."
                    .to_string(),
            ),
            Some(root) if path_within_folded(root, &resolved) => (
                UninstallItemTier::Safe,
                "Strictly inside the declared install root that the inventory recorded for this app (InsideInstallRoot evidence); a strict child of the recorded root is app-owned install content."
                    .to_string(),
            ),
            Some(_) => (
                UninstallItemTier::OffLimits,
                format!(
                    "{}; kept off-limits because the path is not actually inside the declared install root.",
                    evidence_clause(evidence)
                ),
            ),
        },
        LeftoverEvidence::MatchesBundleId { bundle_id, .. } => (
            UninstallItemTier::Review,
            format!("Matches the bundle identifier \"{bundle_id}\" recorded at install time."),
        ),
        LeftoverEvidence::InsideAppDataRoot { root, .. } => (
            UninstallItemTier::Review,
            format!(
                "Attributed by a recorded manifest to this app inside the per-user data root \"{root}\"."
            ),
        ),
        LeftoverEvidence::HeuristicNameMatch { .. } => (
            UninstallItemTier::Review,
            "Name match only: the path is named like the app, but no recorded evidence connects it — this is a heuristic, not recorded evidence."
                .to_string(),
        ),
        _ => (
            UninstallItemTier::OffLimits,
            format!(
                "{}; kept off-limits because it could not be classified safely.",
                evidence_clause(evidence)
            ),
        ),
    }
}

/// A clause (no final period) that names the evidence. Veto reasons append
/// this so the user can see both what matched and why it is still refused.
fn evidence_clause(evidence: &LeftoverEvidence) -> String {
    match evidence {
        LeftoverEvidence::InsideInstallRoot { .. } => {
            "The evidence places this path inside the app's declared install root".to_string()
        }
        LeftoverEvidence::MatchesBundleId { bundle_id, .. } => {
            format!("Matches the bundle identifier \"{bundle_id}\" recorded at install time")
        }
        LeftoverEvidence::MatchesUninstallKey { key_path } => {
            format!("Matches the uninstall registry key \"{key_path}\" recorded at install time")
        }
        LeftoverEvidence::InsideAppDataRoot { root, .. } => format!(
            "A recorded manifest attributes this path to the app inside the per-user data root \"{root}\""
        ),
        LeftoverEvidence::Contended { other_apps, .. } => {
            if other_apps.is_empty() {
                "Contention was recorded for this path".to_string()
            } else {
                format!(
                    "This path is also claimed by installed app(s) {}",
                    other_apps.join(", ")
                )
            }
        }
        LeftoverEvidence::HeuristicNameMatch { .. } => {
            "Name match only: the path is named like the app, but no recorded evidence connects it — this is a heuristic, not recorded evidence"
                .to_string()
        }
    }
}

/// The one-sentence reason for a contended path.
fn contended_why(other_apps: &[String]) -> String {
    if other_apps.is_empty() {
        "Contention was recorded for this path without other application names; because it may be shared, it is always off-limits."
            .to_string()
    } else {
        format!(
            "Also claimed by installed app(s) {}; removing this shared path would break them, so it is always off-limits.",
            other_apps.join(", ")
        )
    }
}

/// The plan-level honesty note. `vendor` is kept separate from the field so
/// the missing / empty / recorded distinction cannot be lost while composing
/// prose.
fn build_note(vendor: Option<&str>, off_limits: usize) -> String {
    let vendor_line = match vendor {
        None => {
            "Vendor uninstaller: not recorded — \"not recorded\" means this build has no recorded command, not that the vendor ships none."
        }
        Some(command) if command.trim().is_empty() => {
            "Vendor uninstaller: recorded as an empty command — recorded, never run by this build; an empty recording is not evidence that the vendor ships no uninstaller."
        }
        Some(_) => {
            "Vendor uninstaller: a command was recorded in inventory metadata — recorded, never run by this build."
        }
    };

    format!(
        "This build removes nothing: every item is inspection only, and no vendor uninstaller is run. {vendor_line} Any future execution must run the vendor uninstaller first and then propose the leftovers through the typed safety pipeline (plan, validate, preview, confirm, reversible move, verify); it must never delete these paths directly. {off_limits} off-limits item(s) are kept for transparency and are not part of the proposal."
    )
}
