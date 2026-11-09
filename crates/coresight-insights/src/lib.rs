//! CoreSight **storage insights** — the read-only correlation layer that
//! turns a concrete [`Snapshot`](coresight_history::Snapshot) into the
//! human answers the UI presents.
//!
//! The rest of CoreSight proves facts at the filesystem level (Phase 1–5)
//! and classifies them into semantic categories (Phase 2). This crate is
//! the layer that *answers questions* about those facts without touching
//! the disk again:
//!
//! * "What is using my space?" — [`usage_by_category`]
//! * "What changed since last time?" — [`category_deltas`],
//!   [`ChangeSummary`]
//! * "What grew the most?" — [`biggest_growth`]
//!
//! ## Contracts honored here
//!
//! * **Pure projection.** The only inputs are already-built snapshot
//!   facts; the crate performs no filesystem access, no subprocess
//!   execution, no network, and no persistence. Callers hand it a
//!   [`Snapshot`](coresight_history::Snapshot); it hands back totals.
//! * **Unknown ≠ empty, and Other ≠ Unknown.** An entry whose stored
//!   classification is `None`, or whose category string cannot be mapped
//!   to a known [`Category`](coresight_classifier::Category), is counted
//!   in a dedicated *Unclassified* bucket — never silently folded into
//!   `Other` (which is a semantic bucket, "a regular file with no more
//!   useful primary category") and never dropped.
//! * **Observation errors are data, not deletion.** Entries with
//!   `observation_error` are counted in their category totals AND in a
//!   separate `errored_entries` counter, so totals cannot be silently
//!   understated by unscanned areas.
//! * **None is not zero.** A file with no known size contributes its
//!   entry count but zero bytes, and is distinguished from a file
//!   *known* to be empty (`size: Some(0)`). Bucket totals carry an
//!   explicit `entries_without_size` count so the UI can say what the
//!   byte total does and does not include.
//! * **Saturating arithmetic only.** Every sum uses
//!   `saturating_add`; totals can never wrap to a smaller number.
//! * **Deterministic.** Results are ordered by [`Category::ALL`] order,
//!   with a canonical tie-break on the category code, so the same input
//!   always produces byte-identical output.
//! * **Completeness is reported, never implied.** Each report carries
//!   `scanned_entries`, `errored_entries`, `entries_without_size` and
//!   `unclassified_entries`. A consumer that needs full knowledge must
//!   see that the report is complete before presenting a total.
//! * **Bounded and O(categories).** Aggregation retains only one
//!   [`CategoryTotals`] per category plus counters — never the input
//!   entries — so memory does not scale with the number of files.
//!
//! The renderer is intentionally separate (Phase 8): this crate produces
//! serializable facts and evidence, not strings.

pub mod aggregate;
pub mod deltas;

pub use aggregate::{usage_by_category, CategoryTotals, UsageReport};
pub use deltas::{
    biggest_growth, category_deltas, change_since, CategoryDelta, ChangeSummary, Growth,
};
