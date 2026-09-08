//! Parent/ancestor context for classification.
//!
//! Context *enriches* classification without requiring the whole tree in
//! memory (master prompt §13/§29). Two mechanisms:
//!
//! 1. [`ParentContext`] — the caller supplies what it knows about the parent
//!    directory (its category, whether it sits in a user-profile tree).
//!    Classification stays a pure function; nothing is retained.
//!
//! 2. [`ParentContextTracker`] — a bounded helper for streaming pipelines:
//!    feed it each classified entry's `(id, parent_id, category)` and it can
//!    answer "what category was this entry's parent?" using a bounded LRU of
//!    recent directories. It never holds the whole tree.

use std::collections::HashMap;
use std::collections::VecDeque;

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
    /// True when the entry sits under the user's profile/home tree.
    pub under_user_profile: bool,
}

impl ParentContext {
    /// Does this context contribute any signal at all?
    pub fn has_signal(&self) -> bool {
        self.parent_category.is_some() || self.under_user_profile
    }
}

/// Bounded recent-directory tracker for streaming classification.
///
/// Capacity-bounded LRU: at most [`ParentContextTracker::MAX_ENTRIES`]
/// directory classifications are retained; older ones are evicted. Memory is
/// therefore O(capacity), never O(tree size).
pub struct ParentContextTracker {
    by_id: HashMap<u64, Category>,
    order: VecDeque<u64>,
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
            by_id: HashMap::with_capacity(capacity.min(1024)),
            order: VecDeque::with_capacity(capacity.min(1024)),
            capacity,
        }
    }

    /// Record a directory's classification (call for every classified dir).
    pub fn record(&mut self, dir_id: u64, category: Category) {
        if self.by_id.contains_key(&dir_id) {
            return;
        }
        if self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.by_id.remove(&oldest);
            }
        }
        self.by_id.insert(dir_id, category);
        self.order.push_back(dir_id);
    }

    /// Look up a parent's category.
    pub fn parent_category(&self, parent_id: Option<u64>) -> Option<Category> {
        parent_id.and_then(|id| self.by_id.get(&id).copied())
    }

    /// How many directory classifications are currently retained.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

impl Default for ParentContextTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply context adjustments to a winning outcome. Context can:
/// * raise confidence by at most one band (never manufacture `High` from
///   nothing — `Low` + context ⇒ `Medium`, `Medium` + context ⇒ `High`),
/// * add [`EvidenceKind::ParentContext`] evidence.
///
/// It can NEVER change the category: the winning rule's category is final.
/// Evidence is appended (bounded by [`EvidenceList::push`]).
pub fn apply_context(
    outcome_category: Category,
    base_confidence: Confidence,
    evidence: &mut EvidenceList,
    context: &ParentContext,
) -> Confidence {
    if !context.has_signal() {
        return base_confidence;
    }
    // Only raise confidence for entries whose winning category is itself a
    // semantic subject (buckets Other/Unknown stay honest).
    if outcome_category.is_bucket() {
        return base_confidence;
    }
    let raised = match base_confidence {
        Confidence::Low => Confidence::Medium,
        Confidence::Medium => Confidence::High,
        other => other,
    };
    evidence.push(EvidenceKind::ParentContext, RuleId::ParentInherited);
    raised
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
        // Oldest entries evicted.
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
    fn context_raises_confidence_one_band() {
        let mut ev = EvidenceList::new();
        let low = apply_context(
            Category::Cache,
            Confidence::Low,
            &mut ev,
            &ParentContext {
                parent_category: Some(Category::UserData),
                under_user_profile: false,
            },
        );
        assert_eq!(low, Confidence::Medium);
        let med = apply_context(
            Category::Cache,
            Confidence::Medium,
            &mut ev,
            &ParentContext {
                parent_category: None,
                under_user_profile: true,
            },
        );
        assert_eq!(med, Confidence::High);
        // High stays High (never lowered by context).
        let high = apply_context(
            Category::Cache,
            Confidence::High,
            &mut ev,
            &ParentContext::default(),
        );
        assert_eq!(high, Confidence::High);
        assert!(high == Confidence::High);
    }

    #[test]
    fn context_cannot_rescue_buckets() {
        let mut ev = EvidenceList::new();
        let c = apply_context(
            Category::Unknown,
            Confidence::Low,
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
            &mut ev,
            &ParentContext::default(),
        );
        assert_eq!(c, Confidence::Low);
        assert!(ev.is_empty());
    }
}
