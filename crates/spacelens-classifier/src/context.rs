//! Parent/ancestor context for classification.
//!
//! Context *enriches* classification without requiring the whole tree in
//! memory (master prompt §13/§29). Two mechanisms:
//!
//! 1. [`ParentContext`] — the caller supplies what it knows about the parent
//!    directory (its category, whether it sits in a user-profile tree).
//!    Classification stays a pure function; nothing is retained.
//!
//! 2. [`ParentContextTracker`] — a bounded **genuine LRU** helper for
//!    streaming pipelines: feed it each classified directory; children look up
//!    their parent's category. A successful lookup *refreshes* recency, so the
//!    eviction victim is always the least recently used key. Memory is
//!    O(capacity), never O(tree).
//!
//! # Confidence discipline
//!
//! Context may raise a winning confidence by at most one band, and the result
//! is clamped to the winner's hard ceiling (see [`crate::confidence`]). Only a
//! parent that was itself classified into a *semantic* category counts: a
//! parent sitting in `Other`/`Unknown` knows nothing, and "somewhere under
//! /home" is not a corroboration either.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::category::Category;
use crate::confidence::Confidence;
use crate::evidence::{EvidenceKind, EvidenceList, RuleId};

/// What the caller knows about an entry's parent directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ParentContext {
    /// The parent directory's own classification, if known.
    pub parent_category: Option<Category>,
    /// Informational: whether the entry sits under the user's profile/home
    /// tree. Recorded for callers, but deliberately **not** sufficient on its
    /// own to raise confidence — "under /home" says nothing about what an
    /// entry is.
    pub under_user_profile: bool,
}

impl ParentContext {
    /// Does this context contribute any signal at all?
    pub fn has_signal(&self) -> bool {
        self.parent_category.is_some() || self.under_user_profile
    }

    /// Whether this context is strong enough to corroborate a weak rule.
    ///
    /// A parent that was itself classified into a bucket (`Other`/`Unknown`)
    /// carries no information and must not inflate confidence.
    pub fn raises_confidence(&self) -> bool {
        matches!(self.parent_category, Some(c) if !c.is_bucket())
    }
}

/// One slot in the LRU's intrusive doubly-linked recency list.
#[derive(Debug, Clone, Copy)]
struct LruNode {
    key: u64,
    value: Category,
    prev: Option<usize>,
    next: Option<usize>,
}

/// Bounded recent-directory tracker for streaming classification.
///
/// **Genuine LRU.** Entries are kept in an intrusive doubly-linked recency
/// list; a successful [`Self::parent_category`] lookup moves the key to the
/// most-recently-used end, so eviction always removes the least recently used
/// key (never merely the oldest inserted). All operations are O(1); the node
/// pool is a fixed `Vec` of at most `capacity` slots, so a hostile tree cannot
/// grow this structure.
pub struct ParentContextTracker {
    /// key → index into `nodes`.
    index: HashMap<u64, usize>,
    nodes: Vec<LruNode>,
    /// Most recently used slot.
    head: Option<usize>,
    /// Least recently used slot (first eviction candidate).
    tail: Option<usize>,
    capacity: usize,
}

impl ParentContextTracker {
    /// Hard bound; a hostile tree cannot grow this structure.
    pub const MAX_ENTRIES: usize = 4096;

    pub fn new() -> Self {
        ParentContextTracker::with_capacity(Self::MAX_ENTRIES)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        ParentContextTracker {
            index: HashMap::new(),
            nodes: Vec::new(),
            head: None,
            tail: None,
            capacity,
        }
    }

    /// Record a directory's classification (call for every classified dir).
    ///
    /// Re-recording an existing key is a deliberate no-op: the first
    /// classification of a directory is the one children see, and a duplicate
    /// must not be able to change it or to consume capacity.
    pub fn record(&mut self, dir_id: u64, category: Category) {
        if self.capacity == 0 || self.index.contains_key(&dir_id) {
            return;
        }
        let slot = if self.nodes.len() < self.capacity {
            self.nodes.push(LruNode {
                key: dir_id,
                value: category,
                prev: None,
                next: None,
            });
            self.nodes.len() - 1
        } else {
            // Recycle the least recently used slot.
            let victim = match self.tail {
                Some(v) => v,
                None => return,
            };
            self.detach(victim);
            self.index.remove(&self.nodes[victim].key);
            self.nodes[victim] = LruNode {
                key: dir_id,
                value: category,
                prev: None,
                next: None,
            };
            victim
        };
        self.index.insert(dir_id, slot);
        self.push_front(slot);
    }

    /// Look up a parent's category, refreshing its recency on a hit.
    pub fn parent_category(&mut self, parent_id: Option<u64>) -> Option<Category> {
        let id = parent_id?;
        let slot = *self.index.get(&id)?;
        let value = self.nodes[slot].value;
        // Recency update: this is what makes the tracker an LRU rather than a
        // FIFO ring.
        self.detach(slot);
        self.push_front(slot);
        Some(value)
    }

    /// Peek without touching recency (used by tests and diagnostics).
    pub fn peek(&self, id: u64) -> Option<Category> {
        self.index.get(&id).map(|s| self.nodes[*s].value)
    }

    /// How many directory classifications are currently retained.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The configured bound.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Keys from most to least recently used (diagnostics/tests).
    pub fn recency_order(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.index.len());
        let mut cur = self.head;
        while let Some(slot) = cur {
            out.push(self.nodes[slot].key);
            cur = self.nodes[slot].next;
        }
        out
    }

    fn push_front(&mut self, slot: usize) {
        self.nodes[slot].prev = None;
        self.nodes[slot].next = self.head;
        if let Some(h) = self.head {
            self.nodes[h].prev = Some(slot);
        }
        self.head = Some(slot);
        if self.tail.is_none() {
            self.tail = Some(slot);
        }
    }

    fn detach(&mut self, slot: usize) {
        let (prev, next) = {
            let n = self.nodes[slot];
            (n.prev, n.next)
        };
        match prev {
            Some(p) => self.nodes[p].next = next,
            None => self.head = next,
        }
        match next {
            Some(x) => self.nodes[x].prev = prev,
            None => self.tail = prev,
        }
        self.nodes[slot].prev = None;
        self.nodes[slot].next = None;
    }
}

impl Default for ParentContextTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply context adjustments to a winning outcome.
///
/// Context can:
/// * raise confidence by at most **one** band, clamped to `cap`
///   (never manufacture `High` from nothing, never escape the ceiling),
/// * add [`EvidenceKind::ParentContext`] evidence.
///
/// It can NEVER change the category: the winning rule's category is final, and
/// bucket outcomes (`Other`/`Unknown`) are never rescued.
pub fn apply_context(
    outcome_category: Category,
    confidence: Confidence,
    cap: Confidence,
    evidence: &mut EvidenceList,
    context: &ParentContext,
) -> Confidence {
    if !context.raises_confidence() {
        return confidence;
    }
    // Bucket categories are never rescued — not even with evidence.
    if outcome_category.is_bucket() {
        return confidence;
    }
    evidence.push(EvidenceKind::ParentContext, RuleId::ParentInherited);
    confidence.raise_one_band_capped(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_is_bounded() {
        let mut t = ParentContextTracker::with_capacity(8);
        for id in 0..100u64 {
            t.record(id, Category::Cache);
        }
        assert_eq!(t.len(), 8);
        assert_eq!(t.parent_category(Some(0)), None);
        assert_eq!(t.parent_category(Some(99)), Some(Category::Cache));
    }

    #[test]
    fn tracker_lookup_by_parent() {
        let mut t = ParentContextTracker::new();
        t.record(7, Category::Downloads);
        assert_eq!(t.parent_category(Some(7)), Some(Category::Downloads));
        assert_eq!(t.parent_category(None), None);
        assert_eq!(t.parent_category(Some(8)), None);
    }

    #[test]
    fn duplicate_record_is_idempotent() {
        let mut t = ParentContextTracker::with_capacity(4);
        t.record(1, Category::Cache);
        t.record(1, Category::Games);
        assert_eq!(t.parent_category(Some(1)), Some(Category::Cache));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn lookup_refreshes_recency_this_is_a_real_lru() {
        // Finding 3: the canonical LRU proof.
        // insert A, B, C; lookup A; insert D  =>  B is evicted, A survives.
        let mut t = ParentContextTracker::with_capacity(3);
        t.record(1, Category::Cache); // A
        t.record(2, Category::Cache); // B
        t.record(3, Category::Cache); // C
        assert_eq!(t.parent_category(Some(1)), Some(Category::Cache)); // touch A
        t.record(4, Category::Cache); // D evicts the LRU

        assert_eq!(t.peek(1), Some(Category::Cache), "A was used: must remain");
        assert_eq!(t.peek(2), None, "B is least recently used: must be evicted");
        assert_eq!(t.peek(3), Some(Category::Cache), "C remains");
        assert_eq!(t.peek(4), Some(Category::Cache), "D remains");
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn fifo_would_fail_the_same_scenario() {
        // Documents the difference explicitly: a FIFO ring would evict A.
        let mut t = ParentContextTracker::with_capacity(3);
        t.record(1, Category::Cache);
        t.record(2, Category::Cache);
        t.record(3, Category::Cache);
        let _ = t.parent_category(Some(1));
        t.record(4, Category::Cache);
        assert!(
            t.peek(1).is_some() && t.peek(2).is_none(),
            "LRU must not behave like FIFO"
        );
    }

    #[test]
    fn repeated_lookups_are_stable() {
        let mut t = ParentContextTracker::with_capacity(3);
        t.record(1, Category::Logs);
        t.record(2, Category::Logs);
        for _ in 0..10 {
            assert_eq!(t.parent_category(Some(1)), Some(Category::Logs));
        }
        t.record(3, Category::Logs);
        // 1 is MRU, 2 is LRU.
        t.record(4, Category::Logs);
        assert_eq!(t.peek(2), None);
        assert!(t.peek(1).is_some() && t.peek(3).is_some() && t.peek(4).is_some());
    }

    #[test]
    fn lookup_of_missing_key_is_none_and_changes_nothing() {
        let mut t = ParentContextTracker::with_capacity(3);
        t.record(1, Category::Cache);
        t.record(2, Category::Cache);
        let before = t.recency_order();
        assert_eq!(t.parent_category(Some(999)), None);
        assert_eq!(t.parent_category(None), None);
        assert_eq!(t.recency_order(), before, "a miss must not touch recency");
    }

    #[test]
    fn capacity_zero_stores_nothing() {
        let mut t = ParentContextTracker::with_capacity(0);
        t.record(1, Category::Cache);
        assert_eq!(t.len(), 0);
        assert_eq!(t.parent_category(Some(1)), None);
        assert!(t.is_empty());
    }

    #[test]
    fn capacity_one_keeps_only_the_latest() {
        let mut t = ParentContextTracker::with_capacity(1);
        t.record(1, Category::Cache);
        assert_eq!(t.parent_category(Some(1)), Some(Category::Cache));
        t.record(2, Category::Cache);
        assert_eq!(t.len(), 1);
        assert_eq!(t.peek(1), None);
        assert_eq!(t.peek(2), Some(Category::Cache));
    }

    #[test]
    fn eviction_order_is_least_recently_used() {
        let mut t = ParentContextTracker::with_capacity(4);
        for id in 1..=4u64 {
            t.record(id, Category::Cache);
        }
        // Touch order: 3, 1, 4  => LRU is 2.
        let _ = t.parent_category(Some(3));
        let _ = t.parent_category(Some(1));
        let _ = t.parent_category(Some(4));
        t.record(5, Category::Cache);
        assert_eq!(t.peek(2), None);
        assert_eq!(t.recency_order(), vec![5, 4, 1, 3]);
    }

    #[test]
    fn node_pool_never_exceeds_capacity() {
        let mut t = ParentContextTracker::with_capacity(16);
        for id in 0..10_000u64 {
            t.record(id, Category::Cache);
            if id % 3 == 0 {
                let _ = t.parent_category(Some(id / 2));
            }
        }
        assert_eq!(t.len(), 16);
        assert!(
            t.nodes.len() <= 16,
            "node pool must stay O(capacity): {} slots",
            t.nodes.len()
        );
    }

    #[test]
    fn duplicate_record_does_not_consume_capacity_or_change_order() {
        let mut t = ParentContextTracker::with_capacity(2);
        t.record(1, Category::Cache);
        t.record(2, Category::Cache);
        let before = t.recency_order();
        t.record(2, Category::Logs);
        assert_eq!(t.recency_order(), before);
        assert_eq!(t.peek(2), Some(Category::Cache), "first value wins");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn context_raises_confidence_one_band_within_cap() {
        let strong = ParentContext {
            parent_category: Some(Category::UserData),
            under_user_profile: false,
        };
        let mut ev = EvidenceList::new();
        assert_eq!(
            apply_context(
                Category::Cache,
                Confidence::Low,
                Confidence::High,
                &mut ev,
                &strong
            ),
            Confidence::Medium
        );
        assert_eq!(
            apply_context(
                Category::Cache,
                Confidence::Medium,
                Confidence::High,
                &mut ev,
                &strong
            ),
            Confidence::High
        );
        // Clamped to the ceiling: an extension rule can never be raised.
        assert_eq!(
            apply_context(
                Category::Images,
                Confidence::Medium,
                Confidence::EXTENSION_ONLY_CAP,
                &mut ev,
                &strong
            ),
            Confidence::Medium
        );
        // Clamped to the heuristic ceiling.
        assert_eq!(
            apply_context(
                Category::Cache,
                Confidence::Low,
                Confidence::HEURISTIC_CAP,
                &mut ev,
                &strong
            ),
            Confidence::Low
        );
    }

    #[test]
    fn user_profile_flag_alone_does_not_raise_confidence() {
        // "Somewhere under /home" is not corroboration.
        let weak = ParentContext {
            parent_category: None,
            under_user_profile: true,
        };
        assert!(!weak.raises_confidence());
        let mut ev = EvidenceList::new();
        assert_eq!(
            apply_context(
                Category::Cache,
                Confidence::Low,
                Confidence::High,
                &mut ev,
                &weak
            ),
            Confidence::Low
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn bucket_parent_does_not_raise_confidence() {
        let bucket_parent = ParentContext {
            parent_category: Some(Category::Other),
            under_user_profile: true,
        };
        assert!(!bucket_parent.raises_confidence());
        let mut ev = EvidenceList::new();
        assert_eq!(
            apply_context(
                Category::Cache,
                Confidence::Low,
                Confidence::High,
                &mut ev,
                &bucket_parent
            ),
            Confidence::Low
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn context_cannot_rescue_buckets() {
        let mut ev = EvidenceList::new();
        let c = apply_context(
            Category::Unknown,
            Confidence::Low,
            Confidence::High,
            &mut ev,
            &ParentContext {
                parent_category: Some(Category::UserData),
                under_user_profile: true,
            },
        );
        assert_eq!(c, Confidence::Low);
        assert!(ev.is_empty(), "no evidence for bucket categories");
    }

    #[test]
    fn no_context_no_change() {
        let mut ev = EvidenceList::new();
        let c = apply_context(
            Category::Cache,
            Confidence::Low,
            Confidence::High,
            &mut ev,
            &ParentContext::default(),
        );
        assert_eq!(c, Confidence::Low);
        assert!(ev.is_empty());
    }
}
