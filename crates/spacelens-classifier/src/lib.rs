//! SpaceLens classification engine — Phase 2.
//!
//! The UNDERSTAND layer on top of the Phase 1 observation engine
//! (docs/ARCHITECTURE.md): raw filesystem observations in, explainable
//! semantic classifications out.
//!
//! Design contracts honored here:
//! - **Pure / offline**: no I/O, no network, no file-content reads. The
//!   classifier operates on path/name/extension/metadata only.
//! - **Explainable by construction**: every result carries category +
//!   confidence + bounded typed evidence and the winning rule id.
//! - **Deterministic**: same input → same output; rule-table order is a
//!   versioned contract.
//! - **Bounded memory**: evidence ≤ [`evidence::MAX_EVIDENCE`] items; the
//!   streaming tracker and aggregator are capacity-bounded, never O(tree).
//! - **Platform-neutral core**: platform knowledge is data
//!   ([`platform::Platform`]); exactly one `cfg!` site exists.
//! - **Honest uncertainty**: `Unknown` (the observation itself is
//!   uninterpretable) is distinct from `Other` (understood, no more useful
//!   category). Extension-only evidence can never reach `High` confidence; a
//!   pure heuristic can never reach `Medium`.
//! - **Two strengths of knowledge**: rooted platform locations
//!   (`pathctx::LOCATION_RULES`) are authoritative; bare names
//!   (`build`, `cache`, `setup`) are heuristics and are gated or capped
//!   accordingly. Installer signals (name *and* extension) may only decide
//!   inside an authoritative download location — an extension says what bytes
//!   are, never where a file came from.
//!
//! Contract namespace: `spacelens.v1.classification.*`
//! (docs/API_CONTRACTS.md).

pub mod aggregate;
pub mod category;
pub mod classify;
pub mod confidence;
pub mod context;
pub mod evidence;
pub mod pathctx;
pub mod platform;
pub mod rules;

pub use aggregate::{CategoryAggregator, CategoryReport, CategoryTotals};
pub use category::{Category, Subcategory};
pub use classify::{classify, classify_streaming, Classification};
pub use confidence::{Confidence, RuleKind};
pub use context::{apply_context, ParentContext, ParentContextTracker};
pub use evidence::{Evidence, EvidenceKind, EvidenceList, RuleId, MAX_EVIDENCE};
pub use pathctx::{LocationClass, LocationMatch, LocationRule, PathContext, LOCATION_RULES};
pub use platform::Platform;
pub use rules::{
    evaluate, rule_by_id, rule_category, MatchOutcome, Rule, RuleGate, RULES, RULES_VERSION,
};

#[cfg(test)]
mod tests {
    use super::*;
    use spacelens_engine::{EntryKind, FsEntry};
    use std::path::PathBuf;

    fn entry(path: &str, kind: EntryKind) -> FsEntry {
        FsEntry {
            id: 1,
            parent_id: None,
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

    #[test]
    fn crate_never_panics_on_degenerate_paths() {
        let cases = [
            "",
            ".",
            "..",
            "...",
            ".....",
            " . ",
            "\t\t",
            "name.",
            ".hidden",
            "trailing dots...",
        ];
        for p in cases {
            let e = entry(p, EntryKind::File);
            let c = classify(&e, &ParentContext::default(), Platform::Linux);
            let _ = format!("{c:?}"); // any result is fine; must not panic
        }
    }

    #[test]
    fn public_api_smoke() {
        let e = entry("/x/cache", EntryKind::Dir);
        let c = classify(&e, &ParentContext::default(), Platform::Windows);
        assert_eq!(c.category, Category::Cache);
        assert_eq!(c.winning_rule, RuleId::CacheDir);
        // A heuristic that is uncorroborated stays in the Low band.
        assert_eq!(c.confidence, Confidence::Low);
    }

    #[test]
    fn crate_never_panics_on_absurd_paths() {
        let deep = "/".repeat(5000);
        let long = "a".repeat(10_000);
        for p in [
            "C:\\",
            "\\\\?\\C:\\huge",
            deep.as_str(),
            long.as_str(),
            "\u{0}\u{1}",
        ] {
            for kind in [EntryKind::File, EntryKind::Dir] {
                let e = entry(p, kind);
                let _ = classify(&e, &ParentContext::default(), Platform::Windows);
                let _ = classify(&e, &ParentContext::default(), Platform::Linux);
            }
        }
    }
}
