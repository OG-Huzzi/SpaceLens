//! Duplicate relationship model (`spacelens.v1.identity.*`).
//!
//! A [`DuplicateGroup`] means: these entries carry byte-identical content.
//! It never means "safe to delete" — that judgment belongs to later phases.
//! The group carries what those phases will need (identity, counts, sizes,
//! object-identity evidence) without coupling to cleanup behavior.
//!
//! The [`DuplicateGroup::from_members`] constructor is the **single source
//! of truth** for member ordering, storage accounting, and detail capping —
//! the pipeline and the tests both go through it, so the arithmetic cannot
//! drift between them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::hash::{ContentHash, HashAlgorithm};

/// Reporting cap for member detail (exact counts are never capped). Kept as
/// a `const` so the contract is visible in the type surface.
pub const DUPLICATE_GROUP_DETAIL_CAP: usize = 64;

/// How [`DuplicateGroup::recoverable_bytes`] was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StorageAccounting {
    /// Every member's file object identity is known; `recoverable_bytes` =
    /// `size × (distinct_objects − 1)`. Hard links were collapsed exactly:
    /// freeing one member of each distinct object frees exactly this much.
    Exact,
    /// Object identity was not fully provable (the platform did not expose
    /// it). `recoverable_bytes` is the upper bound assuming all members are
    /// distinct objects — it may overcount when hard links exist. Reported
    /// honestly instead of pretending precision.
    Estimated,
}

/// Ordering used to sort members within a group. Byte order of the path —
/// stable, locale-independent, cross-platform deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemberOrder {
    PathAscending,
}

/// One member of a duplicate group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMember {
    /// [`spacelens_engine::FsEntry::id`] of the member entry (scan-scoped).
    pub entry_id: u64,
    pub path: PathBuf,
    pub size: u64,
    /// File object identity `(device, inode)` where the platform proved it
    /// (handle-proven at hash time). `None` = not provable on this
    /// platform/volume — accounting degrades to [`StorageAccounting::Estimated`].
    pub object_id: Option<(u64, u64)>,
}

/// A group of ≥2 distinct filesystem entries whose content hashes are
/// identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    /// The shared content identity.
    pub content_hash: ContentHash,
    /// Algorithm that produced [`Self::content_hash`] — identity is
    /// meaningless without it.
    pub algorithm: HashAlgorithm,
    /// Logical size of each member's content (identical within a group).
    pub size: u64,
    /// Number of filesystem entries in the group — exact even when the
    /// member detail is capped.
    pub member_count: u64,
    pub order: MemberOrder,
    /// Members in [`MemberOrder`], capped at the reporting limit passed to
    /// [`Self::from_members`] (bounded memory; never fewer than 2).
    pub members: Vec<DuplicateMember>,
    /// `size × (member_count − 1)`: logical bytes this group represents
    /// beyond the first copy. **Not** a promise of recoverable storage.
    pub logical_duplicate_bytes: u64,
    /// `None` = the group is one file object (pure hard-link aliases):
    /// identical content, identical object, zero duplicated storage.
    pub recoverable_bytes: Option<u64>,
    pub accounting: StorageAccounting,
}

/// Whether member object identities allowed an exact distinct-object count.
enum ObjectCount {
    Exact(u64),
    Unknown,
}

fn distinct_object_count(members: &[DuplicateMember]) -> ObjectCount {
    let mut set = std::collections::BTreeSet::new();
    for m in members {
        match m.object_id {
            Some((d, i)) => {
                set.insert((d, i));
            }
            None => return ObjectCount::Unknown,
        }
    }
    ObjectCount::Exact(set.len() as u64)
}

impl DuplicateGroup {
    /// Build a group from ≥2 same-content members. Sorts members
    /// deterministically, computes storage accounting from object identity,
    /// and caps member detail (keeping at least 2 so the group remains a
    /// group). `max_reported` below 2 is treated as 2.
    pub fn from_members(
        content_hash: ContentHash,
        size: u64,
        mut members: Vec<DuplicateMember>,
        max_reported: usize,
    ) -> DuplicateGroup {
        debug_assert!(members.len() >= 2, "a duplicate group needs >=2 members");
        members.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
        });
        let member_count = members.len() as u64;
        let (recoverable, accounting) = match distinct_object_count(&members) {
            ObjectCount::Exact(d) if d >= 2 => {
                (Some(size.saturating_mul(d - 1)), StorageAccounting::Exact)
            }
            ObjectCount::Exact(_) => (None, StorageAccounting::Exact),
            ObjectCount::Unknown => (
                Some(size.saturating_mul(member_count.saturating_sub(1))),
                StorageAccounting::Estimated,
            ),
        };
        DuplicateGroup {
            content_hash,
            algorithm: HashAlgorithm::Sha256,
            size,
            member_count,
            order: MemberOrder::PathAscending,
            logical_duplicate_bytes: size.saturating_mul(member_count.saturating_sub(1)),
            members: members.into_iter().take(max_reported.max(2)).collect(),
            recoverable_bytes: recoverable,
            accounting,
        }
    }

    /// The first member in deterministic order — a stable representative.
    pub fn representative(&self) -> &DuplicateMember {
        &self.members[0]
    }

    /// `true` when the members span more than one file object. Groups that
    /// are `false` are pure hard-link alias sets: real relationships, but no
    /// duplicated storage.
    pub fn spans_multiple_objects(&self) -> bool {
        self.recoverable_bytes.is_some()
    }

    /// Whether member detail was truncated by the reporting cap.
    pub fn detail_truncated(&self) -> bool {
        (self.members.len() as u64) < self.member_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: u64, path: &str, object_id: Option<(u64, u64)>) -> DuplicateMember {
        DuplicateMember {
            entry_id: id,
            path: PathBuf::from(path),
            size: 100,
            object_id,
        }
    }

    #[test]
    fn logical_duplicate_bytes_is_counted_exactly() {
        let g = DuplicateGroup::from_members(
            ContentHash::from_bytes(b"fixture"),
            100,
            vec![
                member(1, "/a", Some((1, 10))),
                member(2, "/b", Some((1, 11))),
                member(3, "/c", Some((1, 12))),
            ],
            64,
        );
        assert_eq!(g.member_count, 3);
        assert_eq!(g.logical_duplicate_bytes, 200);
        assert_eq!(g.recoverable_bytes, Some(200));
        assert_eq!(g.accounting, StorageAccounting::Exact);
    }

    #[test]
    fn hard_link_aliases_do_not_count_as_recoverable() {
        // Three paths, ONE object: logical duplicates exist, recoverable
        // storage is zero — removing "one copy" frees nothing.
        let g = DuplicateGroup::from_members(
            ContentHash::from_bytes(b"fixture"),
            100,
            vec![
                member(1, "/a", Some((1, 10))),
                member(2, "/b", Some((1, 10))),
                member(3, "/c", Some((1, 10))),
            ],
            64,
        );
        assert!(g.logical_duplicate_bytes > 0);
        assert_eq!(g.recoverable_bytes, None);
        assert!(!g.spans_multiple_objects());
        assert_eq!(g.accounting, StorageAccounting::Exact);
    }

    #[test]
    fn partial_alias_sets_count_only_distinct_objects() {
        // 4 paths, 2 distinct objects → recoverable = size × (2 − 1).
        let g = DuplicateGroup::from_members(
            ContentHash::from_bytes(b"fixture"),
            100,
            vec![
                member(1, "/a", Some((1, 10))),
                member(2, "/b", Some((1, 10))),
                member(3, "/c", Some((1, 20))),
                member(4, "/d", Some((1, 20))),
            ],
            64,
        );
        assert_eq!(g.recoverable_bytes, Some(100));
        assert_eq!(g.accounting, StorageAccounting::Exact);
    }

    #[test]
    fn unknown_object_identity_is_estimated_not_exact() {
        let g = DuplicateGroup::from_members(
            ContentHash::from_bytes(b"fixture"),
            100,
            vec![member(1, "/a", None), member(2, "/b", None)],
            64,
        );
        assert_eq!(g.recoverable_bytes, Some(100));
        assert_eq!(g.accounting, StorageAccounting::Estimated);
    }

    #[test]
    fn members_are_sorted_by_path_bytes_and_representative_is_first() {
        let g = DuplicateGroup::from_members(
            ContentHash::from_bytes(b"fixture"),
            100,
            vec![
                member(2, "/z", Some((1, 11))),
                member(1, "/a", Some((1, 10))),
            ],
            64,
        );
        assert_eq!(g.order, MemberOrder::PathAscending);
        assert_eq!(g.representative().path, PathBuf::from("/a"));
        assert_eq!(g.members[1].path, PathBuf::from("/z"));
    }

    #[test]
    fn detail_cap_keeps_exactly_two_minimum_and_count_exact() {
        let members: Vec<DuplicateMember> = (0..10)
            .map(|i| member(i, &format!("/p{i}"), Some((1, i))))
            .collect();
        let g = DuplicateGroup::from_members(ContentHash::from_bytes(b"fixture"), 100, members, 5);
        assert_eq!(g.member_count, 10);
        assert_eq!(g.members.len(), 5);
        assert!(g.detail_truncated());

        let members: Vec<DuplicateMember> = (0..10)
            .map(|i| member(i, &format!("/p{i}"), Some((1, i))))
            .collect();
        let g = DuplicateGroup::from_members(
            ContentHash::from_bytes(b"fixture"),
            100,
            members,
            1, // below minimum: clamped to 2
        );
        assert_eq!(g.members.len(), 2, "a group always keeps >=2 members");
        assert!(g.detail_truncated());
    }
}
