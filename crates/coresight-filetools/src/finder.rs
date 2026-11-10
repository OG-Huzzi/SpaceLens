//! **Finder** — semantic, evidence-explaining search over a caller-supplied
//! history [`Snapshot`]: the CoreSight replacement for jv16's Finder.
//!
//! A Finder hit is a *filter with a receipt*. Every published hit carries one
//! sentence per criterion it satisfied, so a person can read why a path
//! surfaced before acting on it. This is deliberately not a substring grep
//! over rendered paths: the name criterion reads the entry's own final
//! component and nothing above it.
//!
//! ## Evidence contracts
//!
//! * **The name criterion reads the entry's own name.** `text` is matched
//!   case-insensitively against [`std::path::Path::file_name`] only. Parent
//!   directories are never matched: a folder that happens to contain the
//!   text must not drag its entire subtree into a "cleanup" suggestion.
//!   A blank or whitespace-only `text` is not a criterion at all.
//! * **Unknown is never a match.** A criterion that needs a fact the entry
//!   does not carry — size, modification time, a decodable category — can
//!   never be satisfied by that entry. The two
//!   [`SearchOptions::strict_unknowns`] modes differ only in what happens
//!   *after* that decision, never in whether unknown matches:
//!   `true` excludes the entry and counts it in
//!   [`SearchReport::excluded_unknown`]; `false` skips that single criterion
//!   for that entry and publishes it only when the remaining criteria
//!   positively matched, with [`SearchHit::fully_evaluated`] set to `false`
//!   and without ever claiming the skipped criterion in
//!   [`SearchHit::matched_because`]. An entry for which *no* criterion can be
//!   evaluated has no positive evidence and is excluded and counted in both
//!   modes.
//! * **Every hit explains itself.** `matched_because` holds one sentence per
//!   satisfied criterion in a fixed criterion order (name, category, minimum
//!   size, maximum size, age, kind). It is empty only for the unconstrained
//!   query.
//! * **An unconstrained query matches everything, and is not a
//!   recommendation.** [`SearchQuery::default`] carries no criteria, so it
//!   returns the snapshot bounded by [`SearchOptions::max_results`], with an
//!   empty `matched_because` on every hit. That is a listing, not evidence of
//!   anything: the UI MUST NOT present an unconstrained Finder result as
//!   cleanup candidates or as a recommendation to delete. This function never
//!   refuses the query and never panics on it; the restriction is on how the
//!   result may be presented.
//! * **Deterministic.** Hits are ordered by `(kind rank, size descending with
//!   unknown last, path bytes ascending)`. The same entries in any input
//!   order produce the identical report for the same reference instant.
//! * **Bounded and saturating.** `max_results` truncates the published hits;
//!   `matched` and `truncated` account for every omitted hit, and all totals
//!   and counters saturate instead of wrapping.
//! * **No panics.** Degenerate input (blank text, contradictory size bounds,
//!   zero `max_results`, non-UTF-8 names) yields an empty or bounded report,
//!   never a panic path.
//!
//! ## Time
//!
//! [`SearchQuery`] carries no reference instant, so [`search`] reads the wall
//! clock exactly once for `min_age_days`. [`search_at`] takes that instant as
//! an argument; reproducible runs and tests should prefer it.

use std::path::PathBuf;
use std::time::SystemTime;

use coresight_classifier::Category;
use coresight_history::{ObservedEntry, ObservedKind, Snapshot};
use serde::{Deserialize, Serialize};

use crate::{decode_category, path_bytes, SECONDS_PER_DAY};

// ---------------------------------------------------------------------------
// Query model
// ---------------------------------------------------------------------------

/// The kind of entry a hit describes, normalized to the search vocabulary.
///
/// History names the directory case `Dir`; users and reports say
/// "directory", so the search contract speaks the user-facing word. The
/// declaration order is also the deterministic ordering rank (files first:
/// they are the entries a person can act on individually).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, junction, or other link, observed without following
    /// it. Never silently reported as its target's kind.
    Link,
    /// Anything the scanner could not classify more precisely.
    Other,
}

impl SearchKind {
    /// Rank used by the deterministic hit order.
    ///
    /// Explicit rather than derived so that adding a variant later cannot
    /// silently reorder reports that callers already render or diff.
    fn rank(self) -> u8 {
        match self {
            SearchKind::File => 0,
            SearchKind::Directory => 1,
            SearchKind::Link => 2,
            SearchKind::Other => 3,
        }
    }
}

/// What to look for. Every field is optional; an empty query matches
/// everything and carries no evidence (see the module contract).
///
/// Criteria are conjunctive: an entry is a hit only when it satisfies every
/// criterion that is present. `Vec` fields are empty-means-any rather than
/// empty-means-none, because an empty multi-select in a UI means "no filter",
/// and silently returning nothing would look like a bug.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// Case-insensitive substring of the entry's OWN name (its final path
    /// component), never of the full path or of a parent directory.
    ///
    /// Leading and trailing whitespace is trimmed before matching, and an
    /// empty or whitespace-only value is treated as "no text criterion" — a
    /// cleared search box must not turn into a match-everything filter that
    /// still looks like a text search.
    pub text: Option<String>,
    /// Stored categories that qualify. Empty means any category. An entry
    /// whose stored code is missing or unknown to this build cannot satisfy
    /// this criterion (it is unknown, not "any").
    pub categories: Vec<Category>,
    /// Inclusive lower size bound in bytes. Unknown size never satisfies it.
    pub min_size_bytes: Option<u64>,
    /// Inclusive upper size bound in bytes. Unknown size never satisfies it.
    pub max_size_bytes: Option<u64>,
    /// Minimum age in whole days, measured from the reference instant.
    /// Unknown age never satisfies it, and a future timestamp is not
    /// evidence of age either.
    pub min_age_days: Option<u64>,
    /// Entry kinds that qualify. Empty means any kind.
    pub kinds: Vec<SearchKind>,
}

/// Execution bounds and the unknown-evidence policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    /// Maximum hits published. Remaining matches are counted exactly in
    /// [`SearchReport::truncated`]; the scan itself is not stopped early, so
    /// `matched` is always the true count.
    pub max_results: usize,
    /// What to do with an entry when a criterion cannot be evaluated for it
    /// (unknown size, unknown age, undecodable category, non-UTF-8 name).
    ///
    /// `true` (the default) is audit mode: the entry is excluded and counted
    /// in [`SearchReport::excluded_unknown`], so missing evidence is visible
    /// in the report instead of quietly shrinking the result.
    /// `false` is filter mode: that one criterion is skipped for that entry,
    /// the remaining criteria still apply, and any published hit is marked
    /// [`SearchHit::fully_evaluated`] `false`.
    ///
    /// Neither mode ever treats unknown as matching, because an entry can
    /// only be published when it positively satisfied at least one
    /// criterion. Audit mode is the default: this crate feeds cleanup
    /// suggestions, and "we could not check" must not disappear from the
    /// accounting without the caller opting out.
    pub strict_unknowns: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            max_results: 100,
            strict_unknowns: true,
        }
    }
}

/// One entry the query selected, with the evidence that selected it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Where the entry was observed.
    pub path: PathBuf,
    /// Observed kind of the entry itself, never its link target's kind.
    pub kind: SearchKind,
    /// Observed logical size. `None` is an honest unknown; it is never
    /// rendered as zero.
    pub size: Option<u64>,
    /// Observed modification time. `None` is an honest unknown; no age is
    /// ever inferred from it.
    pub modified: Option<SystemTime>,
    /// Stored classification decoded through the stable category codes.
    /// `None` means the run recorded nothing, or recorded a code this build
    /// does not know — neither is guessed into a category.
    pub category: Option<Category>,
    /// One short sentence per criterion this hit actually satisfied, in a
    /// fixed order: name, category, minimum size, maximum size, age, kind.
    ///
    /// This is the "why", and it is the only place a criterion may be
    /// claimed. Empty only when the query had no criteria at all.
    pub matched_because: Vec<String>,
    /// True when every criterion of the query was evaluated for this entry.
    ///
    /// `false` is possible only in [`SearchOptions::strict_unknowns`] filter
    /// mode, and it means the hit is a partial candidate: at least one
    /// criterion was skipped for lack of evidence, so the hit does NOT prove
    /// the full query. A UI MUST label such a hit as unverified and MUST NOT
    /// offer it for bulk removal without confirmation.
    pub fully_evaluated: bool,
}

/// Result of one Finder pass, with exact accounting for what it did and did
/// not evaluate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchReport {
    /// Published hits, sorted by `(kind rank, size descending with unknown
    /// sizes last, path bytes ascending)`, truncated at
    /// [`SearchOptions::max_results`].
    pub hits: Vec<SearchHit>,
    /// Saturating sum of the sizes of the RETURNED hits that have a known
    /// size. A truncated report therefore understates the true total;
    /// `matched` and `truncated` say by how many hits.
    pub total_bytes: u64,
    /// Total hits before truncation, so a bounded view cannot hide how much
    /// it is hiding.
    pub matched: usize,
    /// Hits omitted by `max_results`, counted exactly.
    pub truncated: usize,
    /// Entries excluded because a criterion could not be evaluated for them.
    ///
    /// In audit mode this counts every entry dropped for missing evidence.
    /// In filter mode it counts only the entries that had no evaluable
    /// criterion at all — the one case where a skipped criterion leaves no
    /// positive evidence, so the entry is dropped in both modes.
    pub excluded_unknown: usize,
    /// Number of snapshot entries examined: every entry, exactly once.
    pub entries_examined: usize,
}

// ---------------------------------------------------------------------------
// Criterion evaluation
// ---------------------------------------------------------------------------

/// The three possible outcomes of one criterion for one entry.
///
/// Three states, never two: "we could not evaluate this" must stay
/// distinguishable from "we evaluated it and it did not match". Collapsing
/// them is exactly how missing evidence becomes a silent miss.
enum Evaluation {
    /// Evaluated and satisfied, with the sentence published in
    /// [`SearchHit::matched_because`].
    Satisfied(String),
    /// Evaluated and not satisfied.
    NotSatisfied,
    /// The facts needed are absent or unusable. Never a match, in either
    /// [`SearchOptions::strict_unknowns`] mode.
    Unevaluable,
}

/// Accumulates the per-entry outcome across all applicable criteria.
///
/// Keeping the reasons, the satisfied flag, and the two counters together
/// makes it impossible for a criterion to record "not satisfied" without
/// also being counted as evaluated.
struct Verdict {
    reasons: Vec<String>,
    satisfied: bool,
    evaluated: usize,
    unevaluable: usize,
}

impl Verdict {
    fn new() -> Verdict {
        Verdict {
            reasons: Vec::new(),
            satisfied: true,
            evaluated: 0,
            unevaluable: 0,
        }
    }

    fn record(&mut self, evaluation: Evaluation) {
        match evaluation {
            Evaluation::Satisfied(reason) => {
                self.reasons.push(reason);
                self.evaluated = self.evaluated.saturating_add(1);
            }
            Evaluation::NotSatisfied => {
                self.satisfied = false;
                self.evaluated = self.evaluated.saturating_add(1);
            }
            Evaluation::Unevaluable => {
                self.unevaluable = self.unevaluable.saturating_add(1);
            }
        }
    }
}

/// Maps the history kind onto the search vocabulary.
fn search_kind(kind: ObservedKind) -> SearchKind {
    match kind {
        ObservedKind::File => SearchKind::File,
        ObservedKind::Dir => SearchKind::Directory,
        ObservedKind::Link => SearchKind::Link,
        ObservedKind::Other => SearchKind::Other,
    }
}

/// Matches the entry's own final component, case-insensitively.
///
/// `needle` is the trimmed text as the caller typed it, used verbatim in the
/// published explanation; `needle_folded` is its pre-folded form. Only
/// `file_name` is consulted — the parent chain is deliberately out of reach,
/// because matching it is how a cleanup tool suggests deleting a whole
/// folder on the strength of a directory name.
fn evaluate_name(entry: &ObservedEntry, needle: &str, needle_folded: &str) -> Evaluation {
    let Some(name) = entry.path.file_name() else {
        // A filesystem root has no final component, so it has no name that
        // could contain the text: a definite non-match, not an unknown.
        return Evaluation::NotSatisfied;
    };
    let Some(name) = name.to_str() else {
        // Case-folding a non-UTF-8 name would need a lossy conversion, which
        // can invent or destroy a match. Unknown is never a match.
        return Evaluation::Unevaluable;
    };
    if name.to_lowercase().contains(needle_folded) {
        Evaluation::Satisfied(format!("name contains {needle:?}"))
    } else {
        Evaluation::NotSatisfied
    }
}

/// Tests the entry's decoded category against the requested set.
///
/// An undecodable or missing classification is unknown: it can never satisfy
/// "category is X", because a code this build does not know is not evidence
/// for any category, including [`Category::Unknown`].
fn evaluate_categories(category: Option<Category>, wanted: &[Category]) -> Evaluation {
    match category {
        Some(category) if wanted.contains(&category) => {
            Evaluation::Satisfied(format!("category is {category:?}"))
        }
        Some(_) => Evaluation::NotSatisfied,
        None => Evaluation::Unevaluable,
    }
}

/// Inclusive lower size bound; unknown size cannot satisfy it.
fn evaluate_min_size(size: Option<u64>, minimum: u64) -> Evaluation {
    match size {
        Some(size) if size >= minimum => {
            Evaluation::Satisfied(format!("size {size} bytes is at least {minimum} bytes"))
        }
        Some(_) => Evaluation::NotSatisfied,
        None => Evaluation::Unevaluable,
    }
}

/// Inclusive upper size bound; unknown size cannot satisfy it.
fn evaluate_max_size(size: Option<u64>, maximum: u64) -> Evaluation {
    match size {
        Some(size) if size <= maximum => {
            Evaluation::Satisfied(format!("size {size} bytes is at most {maximum} bytes"))
        }
        Some(_) => Evaluation::NotSatisfied,
        None => Evaluation::Unevaluable,
    }
}

/// Inclusive minimum-age test against an explicit reference instant.
///
/// Unknown age is unknown, and a timestamp later than the reference instant
/// is not age evidence either: the two facts disagree, so the age is not
/// established. Neither case can satisfy the criterion.
fn evaluate_min_age(
    modified: Option<SystemTime>,
    now: SystemTime,
    minimum_days: u64,
) -> Evaluation {
    let Some(modified) = modified else {
        return Evaluation::Unevaluable;
    };
    let Ok(age) = now.duration_since(modified) else {
        return Evaluation::Unevaluable;
    };
    let required_seconds = minimum_days.saturating_mul(SECONDS_PER_DAY);
    if age.as_secs() < required_seconds {
        return Evaluation::NotSatisfied;
    }
    Evaluation::Satisfied(days_phrase(age.as_secs() / SECONDS_PER_DAY))
}

/// Renders a whole-day age as a short sentence fragment.
///
/// The exact elapsed day count is published rather than the threshold, so the
/// explanation says what is true of THIS entry ("41 days old") instead of
/// restating the query ("at least 30 days old").
fn days_phrase(days: u64) -> String {
    if days == 1 {
        "1 day old".to_string()
    } else {
        format!("{days} days old")
    }
}

/// Kind is always known for an observed entry, so this criterion is always
/// evaluable and can never be the reason an entry is excluded.
fn evaluate_kind(kind: SearchKind, wanted: &[SearchKind]) -> Evaluation {
    if wanted.contains(&kind) {
        Evaluation::Satisfied(format!("kind is {kind:?}"))
    } else {
        Evaluation::NotSatisfied
    }
}

// ---------------------------------------------------------------------------
// Ordering helpers
// ---------------------------------------------------------------------------

/// Sort key that puts known sizes in descending order and unknown sizes last.
///
/// Unknown must not sort as zero: that would place every unmeasured entry at
/// the bottom of a "largest first" list as if it had been measured, and could
/// reorder real hits. It gets its own trailing class instead.
fn size_order_key(size: Option<u64>) -> (u8, u64) {
    match size {
        Some(size) => (0, u64::MAX.saturating_sub(size)),
        None => (1, 0),
    }
}

/// Number of criteria actually present. A blank `text` is not one.
fn criteria_count(query: &SearchQuery, has_text: bool) -> usize {
    [
        has_text,
        !query.categories.is_empty(),
        query.min_size_bytes.is_some(),
        query.max_size_bytes.is_some(),
        query.min_age_days.is_some(),
        !query.kinds.is_empty(),
    ]
    .iter()
    .filter(|present| **present)
    .count()
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Searches `snapshot` for entries matching every criterion of `query`.
///
/// Thin wrapper over [`search_at`] that reads the wall clock once, because
/// [`SearchQuery`] carries no reference instant for `min_age_days`. Callers
/// that already hold the run's reference time should call [`search_at`] so
/// their report is reproducible.
pub fn search(snapshot: &Snapshot, query: &SearchQuery, options: &SearchOptions) -> SearchReport {
    search_at(snapshot, query, options, SystemTime::now())
}

/// Searches `snapshot` as of `now`, without reading any ambient clock.
///
/// Criteria are conjunctive and every applicable criterion is evaluated for
/// every entry, so `matched_because` can always name everything that was
/// satisfied. An entry is published only when it positively satisfied at
/// least one criterion; a criterion that cannot be evaluated is handled per
/// [`SearchOptions::strict_unknowns`] and is never counted as a match.
///
/// Results are deterministic for a fixed `now` and bounded by
/// [`SearchOptions::max_results`]; omitted hits are counted exactly.
pub fn search_at(
    snapshot: &Snapshot,
    query: &SearchQuery,
    options: &SearchOptions,
    now: SystemTime,
) -> SearchReport {
    let needle = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let needle_folded = needle.map(str::to_lowercase);
    let active_criteria = criteria_count(query, needle.is_some());

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut entries_examined: usize = 0;
    let mut excluded_unknown: usize = 0;

    for entry in &snapshot.entries {
        entries_examined = entries_examined.saturating_add(1);

        let kind = search_kind(entry.kind);
        let category = entry
            .classification
            .as_ref()
            .and_then(|classification| decode_category(&classification.category));

        let mut verdict = Verdict::new();
        if let (Some(needle), Some(folded)) = (needle, needle_folded.as_deref()) {
            verdict.record(evaluate_name(entry, needle, folded));
        }
        if !query.categories.is_empty() {
            verdict.record(evaluate_categories(category, &query.categories));
        }
        if let Some(minimum) = query.min_size_bytes {
            verdict.record(evaluate_min_size(entry.size, minimum));
        }
        if let Some(maximum) = query.max_size_bytes {
            verdict.record(evaluate_max_size(entry.size, maximum));
        }
        if let Some(minimum_days) = query.min_age_days {
            verdict.record(evaluate_min_age(entry.modified, now, minimum_days));
        }
        if !query.kinds.is_empty() {
            verdict.record(evaluate_kind(kind, &query.kinds));
        }

        if !verdict.satisfied {
            // A definite non-match: at least one criterion was evaluated and
            // the entry failed it. Missing evidence elsewhere is irrelevant,
            // so this is not an unknown-evidence exclusion.
            continue;
        }
        if active_criteria > 0 && verdict.evaluated == 0 {
            // Every criterion was unevaluable: there is no positive evidence
            // at all, so the entry cannot be published in either mode. This
            // is the one case filter mode still counts, because otherwise the
            // entry would vanish with no trace anywhere in the report.
            excluded_unknown = excluded_unknown.saturating_add(1);
            continue;
        }
        if options.strict_unknowns && verdict.unevaluable > 0 {
            excluded_unknown = excluded_unknown.saturating_add(1);
            continue;
        }

        hits.push(SearchHit {
            path: entry.path.clone(),
            kind,
            size: entry.size,
            modified: entry.modified,
            category,
            matched_because: verdict.reasons,
            fully_evaluated: verdict.unevaluable == 0,
        });
    }

    hits.sort_by(|left, right| {
        left.kind
            .rank()
            .cmp(&right.kind.rank())
            .then_with(|| size_order_key(left.size).cmp(&size_order_key(right.size)))
            .then_with(|| path_bytes(&left.path).cmp(path_bytes(&right.path)))
    });

    let matched = hits.len();
    let published = matched.min(options.max_results);
    let truncated = matched.saturating_sub(published);
    hits.truncate(published);

    let total_bytes = hits.iter().fold(0u64, |total, hit| {
        total.saturating_add(hit.size.unwrap_or(0))
    });

    SearchReport {
        hits,
        total_bytes,
        matched,
        truncated,
        excluded_unknown,
        entries_examined,
    }
}
