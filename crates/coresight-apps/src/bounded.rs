//! Bounded admission primitives (Phase 6.2).
//!
//! Every collection in the application-intelligence layer that is fed by an
//! unbounded source (a directory, a registry key, a claim stream) admits
//! its items through [`BoundedTopK`]. The structure is the single
//! implementation of the Phase 6.1 admission rule:
//!
//! ```text
//! observe candidate → compare against the bounded admission policy
//!                   → retain only the canonically-smallest `capacity` keys
//! ```
//!
//! ## Complexity
//!
//! * Memory: **O(capacity)** at all times — never O(offered).
//! * Time per offer: **O(log capacity)** (ordered map).
//! * Order independence: keys are immutable, and the retained set is always
//!   exactly the `capacity` canonically-smallest *distinct* keys offered, so
//!   any permutation of the same offers yields the same retained set and
//!   the same overflow accounting (see [`BoundedTopK::overflow`]).
//!
//! A final `truncate` is never used: nothing beyond `capacity` is ever held.

use std::collections::BTreeMap;

/// Keeps the `capacity` canonically-smallest keys offered, with the exact
/// number of offers that were not retained.
#[derive(Debug, Clone)]
pub struct BoundedTopK<K: Ord + Clone, V> {
    capacity: usize,
    items: BTreeMap<K, V>,
    overflow: u64,
}

/// What happened to one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Stored as a new key.
    Admitted,
    /// Stored as a new key, evicting the canonically-largest held key.
    AdmittedEvicting,
    /// The key was already held; the value was kept or replaced by the
    /// caller's precedence function.
    Merged,
    /// Refused: the key is canonically larger than everything held and the
    /// structure is full (or capacity is zero).
    Refused,
}

impl<K: Ord + Clone, V> BoundedTopK<K, V> {
    pub fn new(capacity: usize) -> Self {
        BoundedTopK {
            capacity,
            items: BTreeMap::new(),
            overflow: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of items currently retained — always `<= capacity()`.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Exact count of offers not retained (refused offers plus evictions).
    pub fn overflow(&self) -> u64 {
        self.overflow
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.items.get(key)
    }

    /// Mutable access to a retained value. Used to merge into an already
    /// admitted entry without changing the retained key set (the merge
    /// itself must be the caller's commutative operation, so determinism
    /// is preserved).
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.items.get_mut(key)
    }

    /// Offer `value` under `key`. When the key is already held,
    /// `prefer_new(new, existing)` decides canonically (never by arrival)
    /// whether the new value replaces the held one.
    pub fn offer(&mut self, key: K, value: V, prefer_new: impl Fn(&V, &V) -> bool) -> Admission {
        if let Some(existing) = self.items.get_mut(&key) {
            if prefer_new(&value, existing) {
                *existing = value;
            }
            return Admission::Merged;
        }
        if self.capacity == 0 {
            self.overflow += 1;
            return Admission::Refused;
        }
        if self.items.len() < self.capacity {
            self.items.insert(key, value);
            return Admission::Admitted;
        }
        let largest = self.items.keys().next_back().cloned();
        match largest {
            Some(largest) if key < largest => {
                self.items.remove(&largest);
                self.overflow += 1;
                self.items.insert(key, value);
                Admission::AdmittedEvicting
            }
            _ => {
                self.overflow += 1;
                Admission::Refused
            }
        }
    }

    /// Consume into canonical (ascending-key) order.
    pub fn into_sorted(self) -> (Vec<(K, V)>, u64) {
        (self.items.into_iter().collect(), self.overflow)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_only_the_smallest_capacity_keys() {
        let mut t: BoundedTopK<u32, ()> = BoundedTopK::new(3);
        for k in [9, 1, 7, 3, 5, 2, 8] {
            t.offer(k, (), |_, _| false);
            assert!(t.len() <= 3);
        }
        let (items, overflow) = t.into_sorted();
        assert_eq!(items.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(overflow, 4);
    }

    #[test]
    fn zero_capacity_refuses_everything_and_counts() {
        let mut t: BoundedTopK<u32, ()> = BoundedTopK::new(0);
        assert_eq!(t.offer(1, (), |_, _| false), Admission::Refused);
        assert_eq!(t.len(), 0);
        assert_eq!(t.overflow(), 1);
    }

    #[test]
    fn duplicate_keys_merge_by_precedence_not_arrival() {
        let mut a: BoundedTopK<u32, u32> = BoundedTopK::new(2);
        a.offer(1, 10, |n, e| n > e);
        a.offer(1, 20, |n, e| n > e);
        a.offer(1, 5, |n, e| n > e);
        assert_eq!(a.get(&1), Some(&20));
        let mut b: BoundedTopK<u32, u32> = BoundedTopK::new(2);
        b.offer(1, 5, |n, e| n > e);
        b.offer(1, 20, |n, e| n > e);
        b.offer(1, 10, |n, e| n > e);
        assert_eq!(b.get(&1), Some(&20));
    }

    #[test]
    fn every_permutation_retains_the_same_set() {
        let keys = [4u32, 8, 1, 9, 2, 7];
        let mut canonical: Option<(Vec<u32>, u64)> = None;
        let mut perm = keys.to_vec();
        // Heap's algorithm over 6! = 720 permutations.
        fn heap(k: usize, v: &mut Vec<u32>, visit: &mut dyn FnMut(&[u32])) {
            if k == 1 {
                visit(v);
                return;
            }
            heap(k - 1, v, visit);
            for i in 0..k - 1 {
                if k.is_multiple_of(2) {
                    v.swap(i, k - 1);
                } else {
                    v.swap(0, k - 1);
                }
                heap(k - 1, v, visit);
            }
        }
        heap(perm.len(), &mut perm, &mut |p| {
            let mut t: BoundedTopK<u32, ()> = BoundedTopK::new(3);
            for k in p {
                t.offer(*k, (), |_, _| false);
            }
            let (items, overflow) = t.into_sorted();
            let got = (
                items.into_iter().map(|(k, _)| k).collect::<Vec<_>>(),
                overflow,
            );
            match &canonical {
                None => canonical = Some(got),
                Some(c) => assert_eq!(*c, got),
            }
        });
        assert_eq!(canonical.unwrap().0, [1, 2, 4]);
    }
}
