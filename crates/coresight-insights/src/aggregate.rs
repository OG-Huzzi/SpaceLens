//! Aggregation of a single snapshot into per-category usage totals.
//!
//! One pass over the snapshot's entries, retaining exactly one
//! [`CategoryTotals`] per category plus a handful of counters. The
//! result is **O(categories)** memory regardless of how many files the
//! scan covered.

use serde::{Deserialize, Serialize};

use coresight_classifier::Category;
use coresight_history::{ObservedKind, Snapshot};

/// Per-category totals. All arithmetic saturates; totals can never lie by
/// wrapping.
///
/// `files` counts entries whose kind is `File`; `directories` counts
/// `Dir`. `logical_size` sums **only** the sizes that were known —
/// `entries_without_size` says how many entries contributed no size, so a
/// consumer can never mistake a partial byte total for a complete one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotals {
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    /// Sum of known logical sizes (saturating).
    pub logical_size: u64,
    /// Entries in this bucket whose size was unknown (`None`).
    pub entries_without_size: u64,
}

/// The result of aggregating one snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    /// One entry per category, in [`Category::ALL`] order.
    /// `category[17]` is `Unknown`, where every unclassified entry lands.
    pub category: Vec<CategoryTotals>,
    /// Every entry seen, classified or not.
    pub scanned_entries: u64,
    /// Entries seen but not fully readable (`observation_error` set).
    /// These are still counted in their category totals; the counter
    /// exists so a report that omits unreadable areas cannot be presented
    /// as a complete total.
    pub errored_entries: u64,
    /// Entries whose size was `None` — they contributed no bytes.
    pub entries_without_size: u64,
    /// Entries with no stored classification, or whose stored category
    /// string maps to no known [`Category`]. Bucketed under `Unknown`,
    /// never folded into `Other`.
    pub unclassified_entries: u64,
}

impl UsageReport {
    /// Index a category's totals by its canonical position.
    pub fn totals_for(&self, category: Category) -> CategoryTotals {
        let index = Category::ALL
            .iter()
            .position(|c| *c == category)
            .unwrap_or(Category::Unknown as usize);
        self.category.get(index).copied().unwrap_or_default()
    }

    /// Total bytes across every bucket (saturating).
    pub fn total_logical_size(&self) -> u64 {
        self.category
            .iter()
            .fold(0u64, |acc, t| acc.saturating_add(t.logical_size))
    }
}

/// Map a snapshot entry's stored classification to a category.
///
/// The stored form is a serialized [`Category`] name (e.g. `"CACHE"`,
/// `"TEMPORARY_DATA"`). Anything that does not map — a `None`
/// classification, or an unrecognized string from an older rules
/// version — returns `None`; the caller buckets it under `Unknown`
/// rather than `Other`, so a rules-version change can never silently
/// launder unknowns into a semantic bucket.
fn classify_stored(category_name: Option<&str>) -> Option<Category> {
    match category_name? {
        // `Category::code()` — the stable IPC identifier.
        "APPLICATIONS" => Some(Category::Applications),
        "APPLICATION_DATA" => Some(Category::ApplicationData),
        "GAMES" => Some(Category::Games),
        "DOCUMENTS" => Some(Category::Documents),
        "IMAGES" => Some(Category::Images),
        "VIDEO" => Some(Category::Video),
        "AUDIO" => Some(Category::Audio),
        "DOWNLOADS" => Some(Category::Downloads),
        "ARCHIVES" => Some(Category::Archives),
        "DEVELOPMENT" => Some(Category::Development),
        "TEMPORARY_DATA" => Some(Category::TemporaryData),
        "CACHE" => Some(Category::Cache),
        "LOGS" => Some(Category::Logs),
        "BACKUPS" => Some(Category::Backups),
        "SYSTEM_DATA" => Some(Category::SystemData),
        "USER_DATA" => Some(Category::UserData),
        "OTHER" => Some(Category::Other),
        "UNKNOWN" => Some(Category::Unknown),
        _ => None,
    }
}

/// Aggregate one snapshot into per-category usage totals.
pub fn usage_by_category(snapshot: &Snapshot) -> UsageReport {
    let mut totals = [CategoryTotals::default(); Category::COUNT];
    let mut report = UsageReport {
        category: Vec::with_capacity(Category::COUNT),
        ..Default::default()
    };

    for entry in &snapshot.entries {
        report.scanned_entries = report.scanned_entries.saturating_add(1);

        let bucket = classify_stored(entry.classification.as_ref().map(|c| c.category.as_str()));
        let index = bucket
            .and_then(|c| Category::ALL.iter().position(|x| *x == c))
            .unwrap_or(Category::Unknown as usize);
        let totals = &mut totals[index];

        totals.entries = totals.entries.saturating_add(1);
        match entry.kind {
            ObservedKind::File => totals.files = totals.files.saturating_add(1),
            ObservedKind::Dir => totals.directories = totals.directories.saturating_add(1),
            ObservedKind::Link | ObservedKind::Other => {}
        }
        match entry.size {
            Some(size) => {
                totals.logical_size = totals.logical_size.saturating_add(size);
            }
            None => {
                totals.entries_without_size = totals.entries_without_size.saturating_add(1);
                report.entries_without_size = report.entries_without_size.saturating_add(1);
            }
        }
        if bucket.is_none() {
            report.unclassified_entries = report.unclassified_entries.saturating_add(1);
        }
        if entry.observation_error.is_some() {
            report.errored_entries = report.errored_entries.saturating_add(1);
        }
    }

    report.category = totals.to_vec();
    report
}
