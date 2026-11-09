//! Deltas between two usage reports: what changed, and where.
//!
//! Built on [`usage_by_category`](crate::aggregate::usage_by_category),
//! this is the "Why did my storage change?" layer — the moat feature in
//! `docs/DIFFERENTIATION.md` (P1: "storage forensics + history"). It
//! answers with per-category byte deltas and a ranked list of the
//! biggest growth, while preserving the honesty rules of the aggregate
//! layer (partial knowledge is reported, never totalled away).

use serde::{Deserialize, Serialize};

use coresight_classifier::Category;

use crate::aggregate::{usage_by_category, UsageReport};

/// One category's change between two runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDelta {
    pub category: Category,
    /// `new - old` in signed bytes; negative means the category shrank.
    pub bytes_delta: i64,
    pub entries_delta: i64,
    /// Total bytes in the newer run for this category.
    pub new_logical_size: u64,
    /// Total bytes in the older run for this category.
    pub old_logical_size: u64,
    /// `new_entry_count - old_entry_count` (positive = more entries).
    pub new_entries: u64,
}

/// The change summary a UI renders for "What changed?".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    /// All categories' deltas in [`Category::ALL`] order.
    pub deltas: Vec<CategoryDelta>,
    /// Signed total-byte change across every bucket.
    pub total_bytes_delta: i64,
    /// Total entries in the newer run.
    pub new_scanned_entries: u64,
    /// Total entries in the older run.
    pub old_scanned_entries: u64,
    /// Entries in the newer run that could not be fully read.
    pub errored_entries: u64,
    /// Entries in the newer run whose byte totals are partial.
    pub entries_without_size: u64,
    /// Whether either report carried observation errors or unknown sizes.
    /// A summary built from partial knowledge must never be presented as
    /// a complete total.
    pub partial: bool,
}

/// A ranked growth fact for the top-N list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Growth {
    pub category: Category,
    /// Signed byte change (only positive-growth entries are ranked).
    pub bytes_delta: i64,
    pub new_logical_size: u64,
}

impl Growth {
    /// The category's canonical IPC code, for stable ordering/display.
    pub fn code(self) -> &'static str {
        self.category.code()
    }
}

/// Compute per-category deltas between two runs.
pub fn category_deltas(old: &UsageReport, new: &UsageReport) -> ChangeSummary {
    let mut deltas = Vec::with_capacity(Category::COUNT);
    let mut total_bytes_delta: i64 = 0;

    for (i, category) in Category::ALL.iter().enumerate() {
        let old_totals = old.category.get(i).copied().unwrap_or_default();
        let new_totals = new.category.get(i).copied().unwrap_or_default();
        let bytes_delta = sat_delta(new_totals.logical_size, old_totals.logical_size);
        total_bytes_delta = total_bytes_delta.saturating_add(bytes_delta);
        deltas.push(CategoryDelta {
            category: *category,
            bytes_delta,
            entries_delta: sat_delta(new_totals.entries, old_totals.entries),
            new_logical_size: new_totals.logical_size,
            old_logical_size: old_totals.logical_size,
            new_entries: new_totals.entries,
        });
    }

    ChangeSummary {
        deltas,
        total_bytes_delta,
        new_scanned_entries: new.scanned_entries,
        old_scanned_entries: old.scanned_entries,
        errored_entries: new.errored_entries,
        entries_without_size: new.entries_without_size,
        partial: new.errored_entries > 0
            || new.entries_without_size > 0
            || old.errored_entries > 0
            || old.entries_without_size > 0,
    }
}

/// Rank the categories that **grew**, largest byte increase first.
///
/// Ties are broken by category code, so the order is deterministic
/// regardless of input permutation or hash iteration (this crate uses
/// neither). Only strictly positive deltas are returned — a shrinking
/// category is a fact, not "growth".
pub fn biggest_growth(old: &UsageReport, new: &UsageReport, limit: usize) -> Vec<Growth> {
    if limit == 0 {
        return Vec::new();
    }
    let mut growths: Vec<Growth> = category_deltas(old, new)
        .deltas
        .iter()
        .filter_map(|d| {
            (d.bytes_delta > 0).then_some(Growth {
                category: d.category,
                bytes_delta: d.bytes_delta,
                new_logical_size: d.new_logical_size,
            })
        })
        .collect();
    growths.sort_by(|a, b| {
        b.bytes_delta
            .cmp(&a.bytes_delta)
            .then_with(|| a.code().cmp(b.code()))
    });
    growths.truncate(limit);
    growths
}

/// Signed difference of two saturating counters, clamped to `i64::MIN` /
/// `i64::MAX` so a maliciously large prior total can never wrap.
fn sat_delta(new: u64, old: u64) -> i64 {
    let diff = (new as i128) - (old as i128);
    if diff > i64::MAX as i128 {
        i64::MAX
    } else if diff < i64::MIN as i128 {
        i64::MIN
    } else {
        diff as i64
    }
}

/// Convenience: aggregate a snapshot and diff it against an earlier one.
pub fn change_since(
    old: &UsageReport,
    new_snapshot: &coresight_history::Snapshot,
) -> ChangeSummary {
    category_deltas(old, &usage_by_category(new_snapshot))
}
