//! The classification entry point: turn a Phase 1 [`FsEntry`] (plus optional
//! context) into an explainable [`Classification`].
//!
//! Contract highlights (master prompt §5, §6, §18, §19):
//! * The raw observation is never destroyed — classification is a separate
//!   value; callers keep both.
//! * Explainable by construction: category + confidence + bounded evidence.
//! * Operates on path/name/extension/metadata only. **Never reads file
//!   contents.** No I/O at all — this whole crate is pure.
//! * Deterministic: same input → same output, always (tested).

use std::path::Path;

use serde::{Deserialize, Serialize};

use spacelens_engine::{EntryKind, FsEntry};

use crate::category::{Category, Subcategory};
use crate::confidence::Confidence;
use crate::context::{apply_context, ParentContext};
use crate::evidence::{EvidenceKind, EvidenceList, RuleId};
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
    /// Bounded, deterministically ordered evidence.
    pub evidence: EvidenceList,
    /// The winning rule — the single strongest reason for this decision.
    pub winning_rule: RuleId,
    /// Every rule that matched (table order). Kept for explainability;
    /// bounded by the rule-table size, not by tree size.
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

    let outcome: MatchOutcome = evaluate(is_dir, file_name, stem, ext, platform);

    let mut evidence = EvidenceList::new();
    // Evidence for the winning rule first (strongest reason first).
    evidence.push(outcome.evidence_kind, outcome.winner);
    // Then every other matching rule in table order (explainability).
    for id in &outcome.matched {
        if *id != outcome.winner {
            evidence.push(EvidenceKind::KnownPathPattern, *id);
        }
    }

    // Fallback outcomes (no rule matched): category Other, honest confidence.
    let (category, subcategory, mut confidence) =
        if outcome.winner == RuleId::DirWithoutSignals || outcome.winner == RuleId::PlainFile {
            (outcome.category, None, outcome.base_confidence)
        } else {
            (
                outcome.category,
                outcome.subcategory,
                outcome.base_confidence,
            )
        };

    // Context can raise confidence one band; never changes category.
    confidence = apply_context(category, confidence, &mut evidence, context);

    Classification {
        schema: Classification::SCHEMA.to_string(),
        entry_id: entry.id,
        category,
        subcategory,
        confidence,
        evidence,
        winning_rule: outcome.winner,
        matched_rules: outcome.matched,
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
    let under_user_profile = path_mentions_user_profile(&entry.path, platform);
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

/// Split a path into (file name, stem, extension) without allocating more
/// than the lowercased comparison strings inside matchers.
fn split_name(path: &Path) -> (&str, &str, Option<&str>) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => (name, stem, Some(e)),
        _ => (name, name, None),
    }
}

/// Conservative user-profile detection used only to set
/// [`ParentContext::under_user_profile`]. Looks for conventional home
/// directory components; never guesses beyond that.
fn path_mentions_user_profile(path: &Path, platform: Platform) -> bool {
    let Some(full) = path.to_str() else {
        return false;
    };
    let marker_options: &[&str] = match platform {
        // Win32 accepts both separators; check both so synthetic and
        // normalized paths are handled identically on every host.
        Platform::Windows => &["\\Users\\", "/Users/"],
        Platform::Mac => &["/Users/"],
        Platform::Linux => &["/home/"],
    };
    marker_options.iter().any(|m| full.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::Evidence;
    use crate::rules::rule_category;
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
            device: None,
            inode: None,
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
        // C:\Users\...\Downloads\setup.exe → Downloads / Installer / High.
        // Forward slashes: valid on Windows and parse identically on every
        // host (tests must be host-independent; backslash fixtures would
        // break file_name() extraction on Unix hosts).
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
    fn streaming_tracker_propagates_context() {
        let mut t = crate::context::ParentContextTracker::new();
        let parent = dir(1, None, "/home/me/Downloads");
        let pc = classify_streaming(&parent, Platform::Linux, &mut t);
        assert_eq!(pc.category, Category::Downloads);
        let child = file(2, Some(1), "/home/me/Downloads/notes.txt");
        let cc = classify_streaming(&child, Platform::Linux, &mut t);
        // Directory context (Downloads) does not override the .txt extension;
        // the entry still gets its own meaningful subcategory.
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
        // Evidence order stable too.
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

        let other = entry(2, None, "/x/sock", EntryKind::Other);
        let _ = classify(&other, &ParentContext::default(), Platform::Linux);
    }

    #[test]
    fn rule_category_total_mapping_matches_table() {
        // Every rule in the table must map to a category without panicking.
        for r in crate::rules::RULES {
            let _ = rule_category(r.id);
        }
    }
}
