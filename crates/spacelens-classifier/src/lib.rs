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
//! - **Honest uncertainty**: `Unknown` (insufficient evidence) is distinct
//!   from `Other` (understood, no more useful category). Extension-only
//!   evidence can never reach `High` confidence.
//!
//! Contract namespace: `spacelens.v1.classification.*`
//! (docs/API_CONTRACTS.md).

pub mod aggregate;
pub mod category;
pub mod classify;
pub mod confidence;
pub mod context;
pub mod evidence;
pub mod platform;
pub mod rules;

pub use aggregate::{CategoryAggregator, CategoryReport, CategoryTotals};
pub use category::{Category, Subcategory};
pub use classify::{classify, classify_streaming, Classification};
pub use confidence::Confidence;
pub use context::{ParentContext, ParentContextTracker};
pub use evidence::{Evidence, EvidenceKind, EvidenceList, RuleId, MAX_EVIDENCE};
pub use platform::Platform;
pub use rules::{evaluate, rule_category, MatchOutcome, Rule, RULES};

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
            device: None,
            inode: None,
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
    }
}
