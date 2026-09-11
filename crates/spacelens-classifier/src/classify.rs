//! The classification entry point: turn a Phase 1 [`FsEntry`] (plus optional
//! context) into an explainable [`Classification`].
//!
//! Contract highlights (master prompt §5, §6, §18, §19):
//! * The raw observation is never destroyed — classification is a separate
//!   value; callers keep both.
//! * Explainable by construction: category + confidence + bounded evidence
//!   whose kinds are captured at match time.
//! * Operates on path/name/extension/metadata only. **Never reads file
//!   contents.** No I/O at all — this whole crate is pure.
//! * Deterministic: same input → same output, always (tested).
//!
//! # `Other` vs `Unknown`
//!
//! * [`Category::Other`] — the entry is *understood* at a basic level (it is a
//!   regular file, or a directory, with a usable name) but no more useful
//!   primary category applies. This is the normal fallback.
//! * [`Category::Unknown`] — SpaceLens genuinely lacks enough trustworthy
//!   information to establish even that basic interpretation. Three
//!   conditions, all detectable and all tested:
//!   1. the observation carries an error (metadata is incomplete),
//!   2. no usable name can be derived from the path,
//!   3. the entry is not a file/dir/link (socket, FIFO, device) **and** no
//!      rule matched it.
//!
//! `Unknown` is never manufactured just to make the enum reachable.

use std::path::Path;

use serde::{Deserialize, Serialize};

use spacelens_engine::{EntryKind, FsEntry};

use crate::category::{Category, Subcategory};
use crate::confidence::Confidence;
use crate::context::{apply_context, ParentContext};
use crate::evidence::{EvidenceKind, EvidenceList, RuleId};
use crate::pathctx::{self, LocationClass};
use crate::platform::Platform;
use crate::rules::{evaluate, MatchOutcome};

/// Engine-internal result of classifying one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    /// Stable IPC identifier: `spacelens.v1.classification` (docs/API_CONTRACTS.md).
    /// Always [`Classification::SCHEMA`] in v1; `String` so the value
    /// round-trips through serde.
    pub schema: String,
    /// The `FsEntry::id` this classification describes.
    pub entry_id: u64,
    pub category: Category,
    pub subcategory: Option<Subcategory>,
    pub confidence: Confidence,
    /// Bounded, deterministically ordered evidence. Every item's
    /// [`EvidenceKind`] is the signal that actually produced its rule match.
    pub evidence: EvidenceList,
    /// The winning rule — the single strongest reason for this decision.
    pub winning_rule: RuleId,
    /// Every rule that matched (table order; location last). Kept for
    /// explainability; bounded by the rule-table size, not by tree size.
    pub matched_rules: Vec<RuleId>,
}

impl Classification {
    pub const SCHEMA: &'static str = "spacelens.v1.classification";
}

/// Classify one entry. `platform` should normally be [`Platform::current`];
/// passing an explicit value keeps the function pure and testable for all
/// three platforms from anywhere.
pub fn classify(entry: &FsEntry, context: &ParentContext, platform: Platform) -> Classification {
    let is_dir = matches!(entry.kind, EntryKind::Dir);
    let (file_name, stem, ext) = split_name(&entry.path);
    let location = pathctx::analyze(&entry.path, platform).location;

    // ---- Unknown: the observation itself is not interpretable -------------
    // A failed observation has incomplete metadata; an entry with no usable
    // name has nothing to interpret. Both are honest `Unknown`, never `Other`.
    if entry.error.is_some() || file_name.is_empty() {
        return unknown(entry.id);
    }

    let outcome: MatchOutcome = evaluate(is_dir, file_name, stem, ext, platform, location);

    // A special entry (socket/FIFO/device) with no signal at all is genuinely
    // uninterpretable, not merely unclassified.
    if matches!(entry.kind, EntryKind::Other)
        && matches!(
            outcome.winner,
            RuleId::PlainFile | RuleId::DirWithoutSignals
        )
    {
        return unknown(entry.id);
    }

    let mut evidence = EvidenceList::new();
    // Evidence for the winning rule first (strongest reason first) …
    evidence.push(outcome.evidence_kind, outcome.winner);
    // … then every other matching rule in table order. Each keeps the
    // EvidenceKind recorded at match time — nothing is rewritten to a
    // placeholder here (see docs/CLASSIFICATION.md, "Evidence fidelity").
    for m in &outcome.matched {
        if m.rule != outcome.winner {
            evidence.push(m.kind, m.rule);
        }
    }

    // ---- Confidence: one policy, mechanically applied ---------------------
    // Corroboration is real knowledge: an authoritative location that carries
    // subject-matter information, or a parent that was itself classified into
    // a semantic category.
    let corroborated =
        location.is_some_and(|l| l.class.corroborates()) || context.raises_confidence();
    let cap = outcome.confidence_cap(corroborated);
    let base = outcome.base_confidence.min(cap);

    // A gated rule only wins when an authoritative location already vouches
    // for it, so that location is real corroboration: raise one band, still
    // inside the same ceiling. No new evidence item is pushed — the location
    // rule that supplied the corroboration is already recorded above.
    let base = if outcome.gate_satisfied {
        base.raise_one_band_capped(cap)
    } else {
        base
    };

    let confidence = apply_context(outcome.category, base, cap, &mut evidence, context);

    Classification {
        schema: Classification::SCHEMA.to_string(),
        entry_id: entry.id,
        category: outcome.category,
        subcategory: outcome.subcategory,
        confidence,
        evidence,
        winning_rule: outcome.winner,
        // Deduplicated while preserving order: a location rule and a name rule
        // may legitimately share a `RuleId` (`/var/cache` matches `CacheDir`
        // twice — once by rooted location, once by name), and the IPC consumer
        // should not see the same id twice.
        matched_rules: {
            let mut seen = std::collections::HashSet::new();
            outcome
                .matched
                .iter()
                .map(|m| m.rule)
                .filter(|r| seen.insert(*r))
                .collect()
        },
    }
}

/// The honest `Unknown` outcome. Used only when the observation itself is
/// insufficient — never as a fallback for "nothing matched".
fn unknown(entry_id: u64) -> Classification {
    let mut evidence = EvidenceList::new();
    evidence.push(EvidenceKind::EntryKind, RuleId::NoSignals);
    Classification {
        schema: Classification::SCHEMA.to_string(),
        entry_id,
        category: Category::Unknown,
        subcategory: None,
        confidence: Confidence::Unknown,
        evidence,
        winning_rule: RuleId::NoSignals,
        matched_rules: Vec::new(),
    }
}

/// Classify with a streaming parent tracker: records directory results so
/// later children can inherit context. Designed to be fed every entry of a
/// scan in stream order; memory is O(tracker capacity), never O(tree).
pub fn classify_streaming(
    entry: &FsEntry,
    platform: Platform,
    tracker: &mut crate::context::ParentContextTracker,
) -> Classification {
    // "Under a user profile" is derived from the same rooted, anchored,
    // platform-specific location analysis that drives classification — not
    // from a substring scan of the raw path, which would over-match
    // (e.g. `/var/www/root` is not the root account's home).
    let under_user_profile = pathctx::analyze(&entry.path, platform)
        .location
        .is_some_and(|l| l.class == LocationClass::UserHome);
    let context = ParentContext {
        parent_category: tracker.parent_category(entry.parent_id),
        under_user_profile,
    };
    let c = classify(entry, &context, platform);
    if matches!(entry.kind, EntryKind::Dir) {
        tracker.record(entry.id, c.category);
    }
    c
}

/// Split a path into (file name, stem, extension).
///
/// Splits on **both** `/` and `\` and ignores Windows drive prefixes, so a
/// synthetic Windows path yields the same name on a Linux or macOS host. This
/// is the property that keeps the cross-platform suite host-independent
/// (docs/CROSS_PLATFORM.md).
fn split_name(path: &Path) -> (&str, &str, Option<&str>) {
    let full = path.to_str().unwrap_or_default();
    let name = full
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => (name, stem, Some(e)),
        _ => (name, name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::Evidence;
    use std::path::PathBuf;

    fn entry(id: u64, parent: Option<u64>, path: &str, kind: EntryKind) -> FsEntry {
        FsEntry {
            id,
            parent_id: parent,
            path: PathBuf::from(path),
            kind,
            size: 0,
            allocated_size: None,
            modified: None,
            created: None,
            accessed: None,
            changed: None,
            device: None,
            inode: None,
            file_id_hi: None,
            hidden: false,
            error: None,
        }
    }

    fn file(id: u64, parent: Option<u64>, path: &str) -> FsEntry {
        entry(id, parent, path, EntryKind::File)
    }

    fn dir(id: u64, parent: Option<u64>, path: &str) -> FsEntry {
        entry(id, parent, path, EntryKind::Dir)
    }

    #[test]
    fn schema_is_versioned() {
        assert_eq!(Classification::SCHEMA, "spacelens.v1.classification");
    }

    #[test]
    fn downloads_installer_example_from_master_prompt() {
        // C:/Users/.../Downloads/setup.exe → Downloads / Installer / High.
        // Forward slashes: valid on Windows and parse identically on every
        // host (tests must be host-independent; backslash fixtures would
        // break name extraction on Unix hosts).
        let e = file(1, Some(9), "C:/Users/me/Downloads/setup.exe");
        let c = classify(
            &e,
            &ParentContext {
                parent_category: Some(Category::Downloads),
                under_user_profile: true,
            },
            Platform::Windows,
        );
        assert_eq!(c.category, Category::Downloads);
        assert_eq!(c.subcategory, Some(Subcategory::Installer));
        assert_eq!(c.confidence, Confidence::High);
        assert_eq!(c.winning_rule, RuleId::InstallerName);
        assert!(c
            .evidence
            .iter()
            .any(|ev: &Evidence| ev.kind == EvidenceKind::ParentContext));
    }

    #[test]
    fn extension_only_cannot_exceed_medium() {
        let e = file(1, None, "/somewhere/neutral/photo.png");
        let c = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Images);
        assert!(c.confidence <= Confidence::EXTENSION_ONLY_CAP);
    }

    #[test]
    fn parent_context_raises_but_never_changes_category() {
        let e = file(1, Some(2), "/data/neutral/blob.bin");
        let no_ctx = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(no_ctx.category, Category::Other);
        let with_ctx = classify(
            &e,
            &ParentContext {
                parent_category: Some(Category::Cache),
                under_user_profile: false,
            },
            Platform::Linux,
        );
        assert_eq!(with_ctx.category, Category::Other, "category unchanged");
    }

    #[test]
    fn raw_entry_is_preserved_and_untouched() {
        let e = file(42, None, "/x/report.pdf");
        let before = e.path.clone();
        let _ = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(e.path, before, "classification must not mutate the entry");
    }

    #[test]
    fn hidden_dotfile_without_extension_is_other() {
        let e = file(1, None, "/data/.nv");
        let c = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Other);
    }

    #[test]
    fn unicode_and_space_names_classify_by_extension() {
        let e = file(1, None, "/data/документ отчёт.pdf");
        let c = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Documents);
    }

    #[test]
    fn multiple_extensions_use_final_one() {
        let e = file(1, None, "/data/archive.tar.gz");
        let c = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Archives);
    }

    #[test]
    fn unknown_when_observation_is_incomplete() {
        let mut e = file(1, None, "/data/thing.bin");
        e.error = Some(spacelens_engine::model::ErrorCategoryRef::MetadataUnavailable);
        let c = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Unknown);
        assert_eq!(c.confidence, Confidence::Unknown);
        assert_eq!(c.winning_rule, RuleId::NoSignals);
    }

    #[test]
    fn unknown_when_name_is_unusable() {
        for p in ["", "/", "//"] {
            let e = file(1, None, p);
            let c = classify(&e, &ParentContext::default(), Platform::Linux);
            assert_eq!(c.category, Category::Unknown, "path {p:?}");
            assert_eq!(c.confidence, Confidence::Unknown);
        }
    }

    #[test]
    fn unknown_for_uninterpretable_entry_kind_without_signal() {
        let sock = entry(1, None, "/x/sock", EntryKind::Other);
        let c = classify(&sock, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Unknown);
        // … but a name signal still wins: a FIFO named *.log is a log.
        let named = entry(2, None, "/x/thing.log", EntryKind::Other);
        let c = classify(&named, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Logs);
    }

    #[test]
    fn other_is_the_normal_fallback_not_unknown() {
        let e = file(1, None, "/data/plainfile.bin");
        let c = classify(&e, &ParentContext::default(), Platform::Linux);
        assert_eq!(c.category, Category::Other);
        assert_eq!(c.confidence, Confidence::Low);
        assert_eq!(c.winning_rule, RuleId::PlainFile);
    }

    #[test]
    fn streaming_tracker_propagates_context() {
        let mut t = crate::context::ParentContextTracker::new();
        let parent = dir(1, None, "/home/me/Downloads");
        let pc = classify_streaming(&parent, Platform::Linux, &mut t);
        assert_eq!(pc.category, Category::Downloads);
        let child = file(2, Some(1), "/home/me/Downloads/notes.txt");
        let cc = classify_streaming(&child, Platform::Linux, &mut t);
        // Directory context (Downloads) does not override the .txt extension.
        assert_eq!(cc.category, Category::Documents);
        assert!(cc
            .evidence
            .iter()
            .any(|ev: &Evidence| ev.kind == EvidenceKind::ParentContext));
    }

    #[test]
    fn classification_is_deterministic() {
        let e = file(1, None, "/w/Audio/song.mp3");
        let a = classify(&e, &ParentContext::default(), Platform::Windows);
        let b = classify(&e, &ParentContext::default(), Platform::Windows);
        assert_eq!(a, b);
        let ev_a: Vec<_> = a.evidence.iter().collect();
        let ev_b: Vec<_> = b.evidence.iter().collect();
        assert_eq!(ev_a, ev_b);
    }

    #[test]
    fn links_and_special_entries_do_not_panic() {
        let link = entry(
            1,
            None,
            "/x/loop",
            EntryKind::Link(spacelens_engine::LinkInfo {
                kind: spacelens_engine::LinkKind::Symlink,
                target: None,
                broken: true,
            }),
        );
        let c = classify(&link, &ParentContext::default(), Platform::Linux);
        // A link named "loop" has no content signals: honest Other.
        assert_eq!(c.category, Category::Other);
    }

    #[test]
    fn split_name_is_host_independent() {
        // Both separators work on every host, and drive prefixes are dropped.
        assert_eq!(
            split_name(Path::new("C:/Users/u/a.exe")),
            ("a.exe", "a", Some("exe"))
        );
        assert_eq!(
            split_name(Path::new(r"C:\Users\u\a.exe")),
            ("a.exe", "a", Some("exe"))
        );
        assert_eq!(
            split_name(Path::new("/a/b/c.tar.gz")),
            ("c.tar.gz", "c.tar", Some("gz"))
        );
        assert_eq!(
            split_name(Path::new("/a/b/.hidden")),
            (".hidden", ".hidden", None)
        );
        assert_eq!(split_name(Path::new("plain")), ("plain", "plain", None));
    }
}
