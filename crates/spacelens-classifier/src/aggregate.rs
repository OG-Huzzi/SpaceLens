//! Streaming aggregation of classifications into per-category totals.
//!
//! Contract (master prompt §22, §29):
//! * `u64` accounting with saturating arithmetic — cannot overflow.
//! * O(number of categories) memory — never O(entries). Feeding it every
//!   entry of a scan does NOT retain the entries.
//! * Deterministic report order ([`Category::ALL`] order, stable IPC output).

use serde::{Deserialize, Serialize};

use spacelens_engine::EntryKind;

use crate::category::Category;
use crate::classify::Classification;

/// Per-category totals. All arithmetic saturates; totals can never lie by
/// wrapping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotals {
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    /// Sum of logical sizes (saturating).
    pub logical_size: u64,
}

/// Streaming aggregator. Feed every [`Classification`]; retain only this
/// struct. Memory is O([`Category::ALL`].len()).
#[derive(Debug, Clone, Default)]
pub struct CategoryAggregator {
    totals: [CategoryTotals; 17],
    /// Entries whose classification was a bucket (Other/Unknown), tracked
    /// separately so consumers can measure classification coverage.
    unclassified_entries: u64,
    classified_entries: u64,
}

impl CategoryAggregator {
    pub fn new() -> Self {
        CategoryAggregator {
            totals: [CategoryTotals::default(); 17],
            unclassified_entries: 0,
            classified_entries: 0,
        }
    }

    /// Absorb one classification. Does NOT retain the classification.
    pub fn push(&mut self, class: &Classification, entry_kind: &EntryKind, size: u64) {
        let idx = category_index(class.category);
        let t = &mut self.totals[idx];
        t.entries = t.entries.saturating_add(1);
        t.logical_size = t.logical_size.saturating_add(size);
        match entry_kind {
            EntryKind::File => t.files = t.files.saturating_add(1),
            EntryKind::Dir => t.directories = t.directories.saturating_add(1),
            _ => {}
        }
        if class.category.is_bucket() {
            self.unclassified_entries = self.unclassified_entries.saturating_add(1);
        } else {
            self.classified_entries = self.classified_entries.saturating_add(1);
        }
    }

    /// Totals for a category (zeroed if absent).
    pub fn totals(&self, category: Category) -> CategoryTotals {
        self.totals[category_index(category)]
    }

    /// Snapshot in canonical [`Category::ALL`] order — deterministic output.
    pub fn report(&self) -> CategoryReport {
        CategoryReport {
            categories: Category::ALL
                .iter()
                .map(|c| (*c, self.totals(*c)))
                .collect(),
            classified_entries: self.classified_entries,
            unclassified_entries: self.unclassified_entries,
        }
    }
}

/// Deterministic, serializable aggregation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryReport {
    /// Categories in canonical order; zero-total categories included.
    pub categories: Vec<(Category, CategoryTotals)>,
    /// Entries classified into a semantic (non-bucket) category.
    pub classified_entries: u64,
    /// Entries in Other/Unknown buckets — coverage visibility, never hidden.
    pub unclassified_entries: u64,
}

fn category_index(c: Category) -> usize {
    Category::ALL.iter().position(|x| *x == c).unwrap_or(15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::Confidence;
    use crate::evidence::EvidenceList;
    use crate::evidence::RuleId;
    use std::path::PathBuf;

    fn classification(category: Category) -> Classification {
        Classification {
            schema: Classification::SCHEMA.to_string(),
            entry_id: 1,
            category,
            subcategory: None,
            confidence: Confidence::Medium,
            evidence: EvidenceList::new(),
            winning_rule: RuleId::NoSignals,
            matched_rules: Vec::new(),
        }
    }

    #[test]
    fn aggregates_u64_saturating() {
        let mut agg = CategoryAggregator::new();
        let c = classification(Category::Games);
        agg.push(&c, &EntryKind::File, u64::MAX);
        agg.push(&c, &EntryKind::File, 1);
        let t = agg.totals(Category::Games);
        assert_eq!(t.entries, 2);
        assert_eq!(t.logical_size, u64::MAX, "saturates, never wraps");
    }

    #[test]
    fn report_is_canonical_order_and_complete() {
        let mut agg = CategoryAggregator::new();
        agg.push(&classification(Category::Cache), &EntryKind::Dir, 0);
        let report = agg.report();
        let cats: Vec<Category> = report.categories.iter().map(|(c, _)| *c).collect();
        assert_eq!(cats, Category::ALL.to_vec());
        assert_eq!(report.categories[0].0, Category::Applications); // canonical first
    }

    #[test]
    fn buckets_tracked_for_coverage() {
        let mut agg = CategoryAggregator::new();
        agg.push(&classification(Category::Other), &EntryKind::File, 10);
        agg.push(&classification(Category::Unknown), &EntryKind::File, 10);
        agg.push(&classification(Category::Documents), &EntryKind::File, 10);
        let r = agg.report();
        assert_eq!(r.unclassified_entries, 2);
        assert_eq!(r.classified_entries, 1);
    }

    #[test]
    fn counts_files_and_dirs_separately() {
        let mut agg = CategoryAggregator::new();
        agg.push(&classification(Category::Logs), &EntryKind::Dir, 0);
        agg.push(&classification(Category::Logs), &EntryKind::File, 5);
        let t = agg.totals(Category::Logs);
        assert_eq!(t.entries, 2);
        assert_eq!(t.files, 1);
        assert_eq!(t.directories, 1);
        assert_eq!(t.logical_size, 5);
    }

    #[test]
    fn aggregator_does_not_grow_with_entries() {
        // Structural check: aggregator is fixed-size arrays + counters.
        let size = std::mem::size_of::<CategoryAggregator>();
        let limit = std::mem::size_of::<[CategoryTotals; 17]>() + 32;
        assert!(size <= limit, "aggregator grew: {size} > {limit}");
    }

    #[test]
    fn pathbuf_roundtrip_via_json() {
        // Sanity: aggregation output survives IPC serialization.
        let mut agg = CategoryAggregator::new();
        agg.push(&classification(Category::Video), &EntryKind::File, 7);
        let json = serde_json::to_string(&agg.report()).unwrap();
        assert!(json.contains("\"VIDEO\""));
        let _back: CategoryReport = serde_json::from_str(&json).unwrap();
        let _ = PathBuf::new(); // keep import used on all platforms
    }
}
