//! The Phase 4 relationship-intelligence layer: what the verified
//! duplicate pipeline's facts **mean**.
//!
//! ```text
//! Path identity            FsEntry::id / FsEntry::path (scan-scoped)
//!        ↓
//! Object identity          FileIdentity — (volume, file id) proven from
//!                          handles (Unix st_dev/st_ino; Windows
//!                          FILE_ID_INFO). Hard links share it.
//!        ↓
//! Content identity         ContentHash — SHA-256 over bytes.
//!        ↓
//! Relationship identity    THIS MODULE: typed, evidenced, deterministic
//!                          statements about how entries relate.
//! ```
//!
//! ## What this layer is (and is not)
//!
//! A **pure derivation** over [`DuplicateReport`] — no filesystem access,
//! no traversal, no second hashing pass, no I/O of any kind. Every
//! relationship is backed by facts the Phase 3 pipeline already proved
//! under its full mutation/identity/replacement check sequence; nothing is
//! inferred beyond them. This phase reports facts and explainable derived
//! information only — **no deletion, cleanup, movement, or recommendations
//! exist here** (later phases, per the master plan).
//!
//! ## Relationship kinds (explicitly typed, never conflated)
//!
//! - [`RelationshipKind::HardLinkAlias`] — different paths, **same
//!   filesystem object**. Not a duplicate: no second physical copy exists.
//! - [`RelationshipKind::ContentDuplicate`] — **distinct filesystem
//!   objects** carrying byte-identical content (same size + same SHA-256
//!   under the Phase 3 contract). Alias sets inside such a group are
//!   exposed separately ([`Relationship::alias_sets`]) so "4 paths, 3
//!   objects, two of which are aliases of each other" is directly
//!   representable.
//! - **Not relationships:** same size alone, same filename alone, same
//!   pathname alone — none of these ever produce a relationship. Files
//!   whose hashing failed or was skipped are **undetermined**, never
//!   "no duplicates" ([`Undetermined`]).
//!
//! ## Evidence and explainability
//!
//! Every relationship carries categorical [`Evidence`] — never a vague
//! confidence score. "Why are these related?" is answered by rendering the
//! evidence list ([`Relationship::evidence`]) plus the structured facts;
//! see docs/RELATIONSHIPS.md for the explanation contract.
//!
//! ## Determinism
//!
//! Given identical input, output is byte-identical: relationships are
//! sorted by (kind, identity key, first path bytes), members by path
//! bytes, evidence in canonical order, ids derived from content/object
//! identity (never counters). Observation order, worker scheduling, and
//! enumeration order cannot reach the output.
//!
//! ## Boundedness
//!
//! The input report is already bounded (Phase 3 global caps); the
//! derivation adds one record per content group plus one per alias set —
//! transitively bounded — and enforces its own hard cap
//! ([`RelationshipOptions::max_relationship_records`]) with an exact
//! truncation counter. A capped result is visibly capped, never mistaken
//! for a complete one.
//!
//! Contract namespace: `spacelens.v1.relationship.*` (docs/RELATIONSHIPS.md,
//! docs/API_CONTRACTS.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::duplicate::{DuplicateGroup, DuplicateMember, StorageAccounting};
use crate::error::HashFailureKind;
use crate::hash::ContentHash;
use crate::pipeline::{DuplicateReport, DuplicateStatus};

/// Knobs for the derivation. All bounds explicit; overflow counted.
#[derive(Debug, Clone)]
pub struct RelationshipOptions {
    /// Hard cap on relationship records published in one report. The
    /// derivation is transitively bounded by the pipeline's caps, so this
    /// is a belt-and-braces ceiling for hostile inputs; records beyond it
    /// are counted into [`RelationshipReport::relationships_truncated`]
    /// and the status reflects the truncation.
    pub max_relationship_records: usize,
}

impl Default for RelationshipOptions {
    fn default() -> Self {
        RelationshipOptions {
            max_relationship_records: 250_000,
        }
    }
}

/// Overall status of one relationship derivation. **Reuses** the pipeline's
/// typed status (no parallel hierarchy): the derivation cannot be more
/// complete than the run it derives from.
pub type RelationshipStatus = DuplicateStatus;

/// How two or more entries relate. Variants are exhaustive by design:
/// anything not provable is **not** a relationship (it is undetermined).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelationshipKind {
    /// Different paths referring to the **same filesystem object** — an
    /// alias set (hard links). No second physical copy exists.
    HardLinkAlias,
    /// **Distinct filesystem objects** carrying byte-identical content.
    ContentDuplicate,
}

/// Categorical proof for a relationship. Ordered canonically; a
/// relationship's evidence list is always sorted and non-empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Evidence {
    /// The members' content digests are equal (SHA-256, Phase 3 contract:
    /// same size + same digest ⇒ same content).
    ContentHashEqual,
    /// The members' observed sizes are equal. Supporting evidence only —
    /// never sufficient alone.
    SizeEqual,
    /// The members' filesystem object identities are equal — proven from
    /// handles (Unix fstat / Windows FILE_ID_INFO). Proves an alias set.
    ObjectIdentityEqual,
}

/// A filesystem object reference as published by the pipeline
/// (handle-proven `(volume, file id)`; the wide-id high bits participate
/// in identity *comparison* upstream but members publish the pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub volume: u64,
    pub file_id: u64,
}

impl ObjectRef {
    fn from_member_id(id: (u64, u64)) -> Self {
        ObjectRef {
            volume: id.0,
            file_id: id.1,
        }
    }

    /// Deterministic relationship-id fragment for this object.
    fn id_fragment(&self) -> String {
        format!("{:016x}-{:016x}", self.volume, self.file_id)
    }
}

/// One participating path. References the scan-scoped entry — no
/// filesystem records are copied into the relationship model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRef {
    /// [`spacelens_engine::FsEntry::id`] of the member entry (scan-scoped).
    pub entry_id: u64,
    pub path: PathBuf,
    /// Handle-proven object identity where available; `None` = unprovable
    /// on this platform/volume (accounting degrades, never fabricated).
    pub object: Option<ObjectRef>,
}

/// An alias set inside a content-duplicate relationship: one filesystem
/// object reached through ≥2 of the group's paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasSet {
    pub object: ObjectRef,
    /// Paths referring to this object, path-byte ordered. Derived from the
    /// group's *reported* member detail (see [`Relationship::detail_truncated`]).
    pub paths: Vec<PathBuf>,
}

/// A typed, evidenced, deterministic relationship between ≥2 entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    /// Deterministic id derived from the relationship's identity key —
    /// never a counter, stable across runs for the same input.
    pub id: String,
    pub kind: RelationshipKind,
    /// Logical size of the shared content (identical within the
    /// relationship).
    pub size: u64,
    /// Number of participating paths — exact even when member detail is
    /// truncated upstream.
    pub member_count: u64,
    /// Participating paths (path-byte ordered). For content duplicates
    /// this mirrors the group's reported member detail; see
    /// [`Self::detail_truncated`].
    pub members: Vec<MemberRef>,
    /// Distinct filesystem objects among the members where every member's
    /// identity was proven; `None` when any identity is unprovable
    /// (distinctness is then unknown, accounting `Estimated`).
    pub distinct_objects: Option<u64>,
    /// Categorical proof, canonical order, never empty.
    pub evidence: Vec<Evidence>,
    /// Content identity — present exactly for
    /// [`RelationshipKind::ContentDuplicate`].
    pub content: Option<ContentRef>,
    /// Object identity — present exactly for
    /// [`RelationshipKind::HardLinkAlias`].
    pub object: Option<ObjectRef>,
    /// Alias sets within a content duplicate: proven objects reached
    /// through ≥2 member paths. Empty for pure-alias relationships (the
    /// whole relationship is one alias set).
    pub alias_sets: Vec<AliasSet>,
    /// `size × (paths − 1)` for content duplicates; `size × (paths − 1)`
    /// for alias sets (logical bytes the paths describe beyond one).
    /// **Never** a promise of recoverable storage.
    pub logical_duplicate_bytes: u64,
    /// Content duplicates: the group's conservative recoverable estimate
    /// (`size × (distinct_objects − 1)` under `Exact`; the honest upper
    /// bound under `Estimated`). Alias sets: `None` — removing an alias
    /// frees nothing. `None` is an explicit unknown, never a fabricated 0
    /// or a fabricated total.
    pub recoverable_bytes: Option<u64>,
    pub accounting: StorageAccounting,
    /// `true` when the upstream group's member detail was truncated (the
    /// counts above stay exact; alias sets reflect reported detail only).
    pub detail_truncated: bool,
}

/// Content identity as published by the contract (hex form; the raw digest
/// type stays engine-internal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentRef {
    /// Lowercase hex SHA-256 digest.
    pub sha256_hex: String,
    /// Algorithm tag (`"sha256"`) — an identity is meaningless without it.
    pub algorithm: String,
}

impl ContentRef {
    fn from_hash(hash: &ContentHash) -> Self {
        ContentRef {
            sha256_hex: hash.as_hex(),
            algorithm: crate::hash::HashAlgorithm::Sha256.tag().to_string(),
        }
    }
}

/// Files whose relationship status could not be determined, with typed
/// Phase 3 reasons. A run with undetermined files is never presented as a
/// clean "no duplicates" result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Undetermined {
    /// Exact count of files whose hashing failed (typed reason below).
    pub failed: u64,
    /// Exact count of eligible files never examined because a global cap
    /// bit (size-tracking / record budget). Capped runs say so.
    pub not_examined: u64,
    /// Exact per-reason counts of the failed files, sorted by reason.
    pub failed_by_reason: Vec<(HashFailureKind, u64)>,
    /// Bounded typed detail (mirrors the pipeline's capped failure list).
    pub detail: Vec<UndeterminedDetail>,
    /// How many failure details were truncated upstream.
    pub detail_truncated: u64,
}

/// One undetermined file: path + typed Phase 3 reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndeterminedDetail {
    pub path: PathBuf,
    pub reason: HashFailureKind,
    pub message: String,
}

/// Fixed-width counters for one derivation (bounded).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipStats {
    pub relationships: u64,
    pub content_duplicates: u64,
    pub hard_link_alias_sets: u64,
    /// Paths participating in at least one published relationship.
    pub paths_in_relationships: u64,
    pub total_logical_duplicate_bytes: u64,
    /// Sum of `recoverable_bytes` where a value exists; `None` when no
    /// published relationship could prove one (an honest unknown).
    pub total_recoverable_bytes: Option<u64>,
}

/// The Phase 4 result: typed relationships + undetermined summary over one
/// verified pipeline run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipReport {
    /// Reused pipeline status. `Cancelled`/`Unsupported` runs carry no
    /// relationships; `CompletedWithLimits` marks runs where caps bit.
    pub status: RelationshipStatus,
    /// Relationships in canonical deterministic order (kind, identity key,
    /// first path bytes).
    pub relationships: Vec<Relationship>,
    /// Exact count of relationships omitted by
    /// [`RelationshipOptions::max_relationship_records`].
    pub relationships_truncated: u64,
    pub undetermined: Undetermined,
    pub stats: RelationshipStats,
    /// Run provenance — the pipeline run's own timestamps (the existing
    /// canonical run reference; no parallel run-id scheme is invented).
    pub started_at: std::time::SystemTime,
    pub finished_at: std::time::SystemTime,
}

/// Partition one group's members by proven object identity.
/// Proven members group under their `ObjectRef`; unproven members are
/// counted (their distinctness is unknowable, never assumed).
fn partition_by_object(
    group: &DuplicateGroup,
) -> (BTreeMap<ObjectRef, Vec<&DuplicateMember>>, u64) {
    let mut by_object: BTreeMap<ObjectRef, Vec<&DuplicateMember>> = BTreeMap::new();
    let mut unproven = 0u64;
    for member in &group.members {
        match member.object_id {
            Some(id) => {
                by_object
                    .entry(ObjectRef::from_member_id(id))
                    .or_default()
                    .push(member);
            }
            None => unproven += 1,
        }
    }
    (by_object, unproven)
}

fn member_ref(member: &DuplicateMember) -> MemberRef {
    MemberRef {
        entry_id: member.entry_id,
        path: member.path.clone(),
        object: member.object_id.map(ObjectRef::from_member_id),
    }
}

/// Canonical member order: path bytes ascending (matches the upstream
/// group's `MemberOrder::PathAscending`, re-asserted here so the
/// derivation never depends on upstream ordering choices).
fn sort_members(members: &mut [MemberRef]) {
    members.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
    });
}

/// Build the deterministic id for a relationship.
fn relationship_id(kind: RelationshipKind, key: &str) -> String {
    match kind {
        RelationshipKind::HardLinkAlias => format!("alias-{key}"),
        RelationshipKind::ContentDuplicate => format!("content-{key}"),
    }
}

/// Derive the relationship report from one verified pipeline run.
///
/// Pure function over [`DuplicateReport`]: no I/O, no clock reads, no
/// randomness. Panics on nothing; every input shape maps to a typed
/// output (an empty/cancelled report yields an empty relationship set).
pub fn derive_relationships(
    report: &DuplicateReport,
    options: &RelationshipOptions,
) -> RelationshipReport {
    let mut relationships: Vec<Relationship> = Vec::new();

    // Defense in depth: the pipeline guarantees that Cancelled/Unsupported
    // reports carry no groups; the derivation additionally refuses to
    // publish relationships from any non-completed run, so partial state
    // can never leak through an upstream regression.
    let run_completed = matches!(
        report.status,
        DuplicateStatus::Completed | DuplicateStatus::CompletedWithLimits
    );

    for group in &report.groups {
        if !run_completed {
            break;
        }
        let (by_object, unproven) = partition_by_object(group);
        let proven_objects = by_object.len() as u64;
        let distinct_objects = if unproven > 0 {
            None // distinctness unknowable while any identity is unproven
        } else {
            Some(proven_objects)
        };
        let detail_truncated = group.detail_truncated();

        // Alias sets: proven objects reached through ≥2 reported paths.
        let mut alias_sets: Vec<AliasSet> = Vec::new();
        for (object, members) in &by_object {
            if members.len() >= 2 {
                let mut paths: Vec<PathBuf> = members.iter().map(|m| m.path.clone()).collect();
                paths.sort();
                alias_sets.push(AliasSet {
                    object: *object,
                    paths,
                });
            }
        }

        // Hard-link-alias relationships — one per alias set. Evidence:
        // object identity equality (proven from handles). Recoverable
        // bytes: None — removing an alias frees nothing.
        for set in &alias_sets {
            let members: Vec<MemberRef> = by_object[&set.object]
                .iter()
                .map(|m| member_ref(m))
                .collect();
            let member_count = members.len() as u64;
            let mut members = members;
            sort_members(&mut members);
            relationships.push(Relationship {
                id: relationship_id(RelationshipKind::HardLinkAlias, &set.object.id_fragment()),
                kind: RelationshipKind::HardLinkAlias,
                size: group.size,
                member_count,
                members,
                distinct_objects: Some(1),
                evidence: vec![Evidence::ObjectIdentityEqual],
                content: None,
                object: Some(set.object),
                alias_sets: Vec::new(),
                logical_duplicate_bytes: group.size.saturating_mul(member_count - 1),
                recoverable_bytes: None,
                accounting: StorageAccounting::Exact,
                detail_truncated,
            });
        }

        // Content-duplicate relationship — only when the group actually
        // represents ≥2 independent objects (or distinctness is unproven).
        // A pure alias set (one object, all paths) is NOT a content
        // duplicate: no second physical copy exists.
        let pure_alias = proven_objects == 1 && unproven == 0;
        if !pure_alias {
            let members: Vec<MemberRef> = group.members.iter().map(member_ref).collect();
            let member_count = group.member_count;
            let mut members = members;
            sort_members(&mut members);
            // Evidence: content equality is the proof; size equality is
            // supporting. Canonical order = enum order (ContentHashEqual
            // < SizeEqual).
            let mut evidence = vec![Evidence::SizeEqual, Evidence::ContentHashEqual];
            evidence.sort();
            relationships.push(Relationship {
                id: relationship_id(
                    RelationshipKind::ContentDuplicate,
                    &group.content_hash.as_hex(),
                ),
                kind: RelationshipKind::ContentDuplicate,
                size: group.size,
                member_count,
                members,
                distinct_objects,
                evidence,
                content: Some(ContentRef::from_hash(&group.content_hash)),
                object: None,
                alias_sets,
                logical_duplicate_bytes: group.logical_duplicate_bytes,
                recoverable_bytes: group.recoverable_bytes,
                accounting: group.accounting,
                detail_truncated,
            });
        }
    }

    // Deterministic canonical order: kind → identity key → first path.
    relationships.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| {
                let pa = a
                    .members
                    .first()
                    .map(|m| m.path.as_os_str().as_encoded_bytes());
                let pb = b
                    .members
                    .first()
                    .map(|m| m.path.as_os_str().as_encoded_bytes());
                pa.cmp(&pb)
            })
    });

    // Hard cap with exact truncation accounting (deterministic: the first
    // max records in canonical order are kept).
    let total = relationships.len() as u64;
    let truncated = total.saturating_sub(options.max_relationship_records as u64);
    relationships.truncate(options.max_relationship_records);

    // Undetermined summary — typed Phase 3 failures plus cap exclusions.
    // Per-reason counts cover the reported detail (capped upstream at 256);
    // `failed` carries the exact total, `detail_truncated` the omitted
    // detail count — the three always reconcile.
    let mut failed_by_reason: BTreeMap<HashFailureKind, u64> = BTreeMap::new();
    for failure in &report.failures {
        *failed_by_reason.entry(failure.kind).or_default() += 1;
    }
    let undetermined = Undetermined {
        failed: report.stats.failures,
        not_examined: report.stats.candidates_untracked_total,
        failed_by_reason: failed_by_reason.into_iter().collect(),
        detail: report
            .failures
            .iter()
            .map(|f| UndeterminedDetail {
                path: f.path.clone(),
                reason: f.kind,
                message: f.message.clone(),
            })
            .collect(),
        detail_truncated: report.failures_truncated,
    };

    // Stats over PUBLISHED relationships only; truncation is reported
    // separately so the two always reconcile: published + truncated =
    // derived.
    let stats = RelationshipStats {
        relationships: relationships.len() as u64,
        content_duplicates: relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::ContentDuplicate)
            .count() as u64,
        hard_link_alias_sets: relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::HardLinkAlias)
            .count() as u64,
        paths_in_relationships: {
            let mut paths: std::collections::BTreeSet<&Path> = std::collections::BTreeSet::new();
            for r in &relationships {
                for m in &r.members {
                    paths.insert(m.path.as_path());
                }
            }
            paths.len() as u64
        },
        total_logical_duplicate_bytes: relationships
            .iter()
            .map(|r| r.logical_duplicate_bytes)
            .fold(0u64, |acc, v| acc.saturating_add(v)),
        total_recoverable_bytes: relationships.iter().try_fold(0u64, |acc, r| {
            r.recoverable_bytes.map(|v| acc.saturating_add(v))
        }),
    };

    RelationshipReport {
        status: report.status,
        relationships,
        relationships_truncated: truncated,
        undetermined,
        stats,
        started_at: report.started_at,
        finished_at: report.finished_at,
    }
}

// ---------------------------------------------------------------------------
// Queryable index (Objective 11): clean lookups without exposing raw
// internal collections.
// ---------------------------------------------------------------------------

/// In-memory lookup index over one [`RelationshipReport`]. Pure, offline,
/// bounded by the report (which is bounded by the pipeline's caps). No
/// database: the persistent store belongs to later phases.
#[derive(Debug, Clone)]
pub struct RelationshipIndex {
    relationships: Vec<Relationship>,
    /// path → relationship positions (BTreeMap: deterministic iteration).
    by_path: BTreeMap<PathBuf, Vec<usize>>,
    /// (volume, file id) → positions of relationships whose members or
    /// alias sets include that object.
    by_object: BTreeMap<(u64, u64), Vec<usize>>,
    /// content digest → positions of content-duplicate relationships.
    by_content: BTreeMap<ContentHash, Vec<usize>>,
}

impl RelationshipIndex {
    /// Build the index. Positions refer to the report's canonical order.
    pub fn build(report: &RelationshipReport) -> Self {
        let mut by_path: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        let mut by_object: BTreeMap<(u64, u64), Vec<usize>> = BTreeMap::new();
        let mut by_content: BTreeMap<ContentHash, Vec<usize>> = BTreeMap::new();
        for (position, relationship) in report.relationships.iter().enumerate() {
            for member in &relationship.members {
                by_path
                    .entry(member.path.clone())
                    .or_default()
                    .push(position);
                if let Some(object) = member.object {
                    by_object
                        .entry((object.volume, object.file_id))
                        .or_default()
                        .push(position);
                }
            }
            for set in &relationship.alias_sets {
                by_object
                    .entry((set.object.volume, set.object.file_id))
                    .or_default()
                    .push(position);
            }
            if let Some(content) = &relationship.content {
                if let Ok(bytes) = hex_to_32(&content.sha256_hex) {
                    by_content
                        .entry(ContentHash::from_bytes(&bytes))
                        .or_default()
                        .push(position);
                }
            }
        }
        RelationshipIndex {
            relationships: report.relationships.clone(),
            by_path,
            by_object,
            by_content,
        }
    }

    /// All relationships in canonical order.
    pub fn relationships(&self) -> &[Relationship] {
        &self.relationships
    }

    /// Relationships involving this exact path (alias + content).
    pub fn relationships_for_path(&self, path: &Path) -> Vec<&Relationship> {
        match self.by_path.get(path) {
            Some(positions) => positions.iter().map(|&p| &self.relationships[p]).collect(),
            None => Vec::new(),
        }
    }

    /// Relationships involving this filesystem object (as a member or via
    /// an alias set) — each relationship at most once, canonical order.
    pub fn relationships_for_object(&self, volume: u64, file_id: u64) -> Vec<&Relationship> {
        match self.by_object.get(&(volume, file_id)) {
            // A relationship can reference the object through several
            // members and its alias set; positions are already in
            // ascending order, so dedup keeps the first occurrence of each.
            Some(positions) => {
                let mut seen = std::collections::BTreeSet::new();
                positions
                    .iter()
                    .filter(|p| seen.insert(**p))
                    .map(|&p| &self.relationships[p])
                    .collect()
            }
            None => Vec::new(),
        }
    }

    /// The content-duplicate relationship proven by this digest, if any.
    pub fn relationships_for_content(&self, hash: &ContentHash) -> Vec<&Relationship> {
        match self.by_content.get(hash) {
            Some(positions) => positions.iter().map(|&p| &self.relationships[p]).collect(),
            None => Vec::new(),
        }
    }

    /// Content-duplicate relationships in canonical order.
    pub fn duplicate_groups(&self) -> Vec<&Relationship> {
        self.relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::ContentDuplicate)
            .collect()
    }

    /// Hard-link alias relationships in canonical order.
    pub fn hard_link_groups(&self) -> Vec<&Relationship> {
        self.relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::HardLinkAlias)
            .collect()
    }
}

fn hex_to_32(hex: &str) -> Result<[u8; 32], ()> {
    if hex.len() != 64 {
        return Err(());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duplicate::DuplicateGroup;
    use crate::hash::ContentHash;
    use crate::pipeline::PipelineStats;
    use spacelens_engine::ErrorCategory;
    use std::time::SystemTime;

    fn member(id: u64, path: &str, object: Option<(u64, u64)>) -> DuplicateMember {
        DuplicateMember {
            entry_id: id,
            path: PathBuf::from(path),
            size: 100,
            object_id: object,
        }
    }

    fn group(
        hash_hex: &str,
        members: Vec<DuplicateMember>,
        recoverable: Option<u64>,
        accounting: StorageAccounting,
    ) -> DuplicateGroup {
        let mut g = DuplicateGroup::from_members(
            ContentHash::from_bytes(hash_hex.as_bytes()),
            100,
            members,
            64,
        );
        // from_members derives accounting from member identity; the fixture
        // may override recoverable/accounting for unproven-identity shapes.
        g.recoverable_bytes = recoverable;
        g.accounting = accounting;
        g
    }

    fn report(status: DuplicateStatus, groups: Vec<DuplicateGroup>) -> DuplicateReport {
        DuplicateReport {
            status,
            groups,
            failures: Vec::new(),
            failures_truncated: 0,
            stats: PipelineStats::default(),
            eligibility: Default::default(),
            logical_duplicate_bytes: 0,
            recoverable_bytes: None,
            started_at: SystemTime::UNIX_EPOCH,
            finished_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn exact_duplicates_become_one_content_relationship() {
        let r = report(
            DuplicateStatus::Completed,
            vec![group(
                "alpha",
                vec![
                    member(1, "/b.bin", Some((1, 11))),
                    member(2, "/a.bin", Some((1, 22))),
                ],
                Some(100),
                StorageAccounting::Exact,
            )],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.status, DuplicateStatus::Completed);
        assert_eq!(rel.relationships.len(), 1);
        let c = &rel.relationships[0];
        assert_eq!(c.kind, RelationshipKind::ContentDuplicate);
        assert_eq!(c.member_count, 2);
        assert_eq!(c.distinct_objects, Some(2));
        assert_eq!(
            c.evidence,
            vec![Evidence::ContentHashEqual, Evidence::SizeEqual]
        );
        assert_eq!(c.recoverable_bytes, Some(100));
        assert_eq!(c.accounting, StorageAccounting::Exact);
        assert!(c.content.is_some() && c.object.is_none());
        assert_eq!(c.alias_sets.len(), 0);
        // Deterministic member order regardless of input order.
        assert_eq!(c.members[0].path, PathBuf::from("/a.bin"));
        // Deterministic id from content identity.
        assert!(c.id.starts_with("content-"));
        assert_eq!(rel.stats.content_duplicates, 1);
        assert_eq!(rel.stats.hard_link_alias_sets, 0);
    }

    #[test]
    fn pure_alias_group_is_hard_link_not_content_duplicate() {
        let r = report(
            DuplicateStatus::Completed,
            vec![group(
                "same",
                vec![
                    member(1, "/x", Some((1, 7))),
                    member(2, "/y", Some((1, 7))),
                    member(3, "/z", Some((1, 7))),
                ],
                None,
                StorageAccounting::Exact,
            )],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        // One alias relationship; NO content duplicate (no second copy).
        assert_eq!(rel.relationships.len(), 1);
        let a = &rel.relationships[0];
        assert_eq!(a.kind, RelationshipKind::HardLinkAlias);
        assert_eq!(a.evidence, vec![Evidence::ObjectIdentityEqual]);
        assert_eq!(a.member_count, 3);
        assert_eq!(a.distinct_objects, Some(1));
        assert_eq!(a.recoverable_bytes, None, "removing an alias frees nothing");
        assert!(a.object.is_some() && a.content.is_none());
        assert_eq!(rel.stats.hard_link_alias_sets, 1);
        assert_eq!(rel.stats.content_duplicates, 0);
    }

    #[test]
    fn aliases_plus_independent_copy_span_both_kinds() {
        // A,B = object 7; C = object 9; all same content.
        let r = report(
            DuplicateStatus::Completed,
            vec![group(
                "mixed",
                vec![
                    member(1, "/a", Some((1, 7))),
                    member(2, "/b", Some((1, 7))),
                    member(3, "/c", Some((1, 9))),
                ],
                Some(100),
                StorageAccounting::Exact,
            )],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.relationships.len(), 2);
        // Canonical order: aliases first, then content.
        let alias = &rel.relationships[0];
        let content = &rel.relationships[1];
        assert_eq!(alias.kind, RelationshipKind::HardLinkAlias);
        assert_eq!(
            alias.object,
            Some(ObjectRef {
                volume: 1,
                file_id: 7
            })
        );
        assert_eq!(alias.member_count, 2);
        assert_eq!(content.kind, RelationshipKind::ContentDuplicate);
        assert_eq!(content.member_count, 3);
        assert_eq!(content.distinct_objects, Some(2));
        // The content relationship exposes the alias set explicitly.
        assert_eq!(content.alias_sets.len(), 1);
        assert_eq!(content.alias_sets[0].object.file_id, 7);
        assert_eq!(
            content.alias_sets[0].paths,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
        assert_eq!(content.recoverable_bytes, Some(100));
        assert_eq!(rel.stats.content_duplicates, 1);
        assert_eq!(rel.stats.hard_link_alias_sets, 1);
    }

    #[test]
    fn unproven_identity_degrades_to_estimated_never_fabricated() {
        // Two members, no proven identity: content duplicate with unknown
        // distinctness (Estimated), NO alias sets.
        let r = report(
            DuplicateStatus::Completed,
            vec![group(
                "anon",
                vec![member(1, "/a", None), member(2, "/b", None)],
                Some(100),
                StorageAccounting::Estimated,
            )],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.relationships.len(), 1);
        let c = &rel.relationships[0];
        assert_eq!(c.kind, RelationshipKind::ContentDuplicate);
        assert_eq!(c.distinct_objects, None, "distinctness unknowable");
        assert_eq!(c.accounting, StorageAccounting::Estimated);
        assert!(c.alias_sets.is_empty(), "aliases cannot be proven either");
    }

    #[test]
    fn one_proven_plus_unproven_members_is_not_alias_claimed() {
        // Object 7 proven once + two unproven: no alias set (needs ≥2
        // proven paths on one object), content duplicate with unknown
        // distinctness.
        let r = report(
            DuplicateStatus::Completed,
            vec![group(
                "part",
                vec![
                    member(1, "/a", Some((1, 7))),
                    member(2, "/b", None),
                    member(3, "/c", None),
                ],
                Some(200),
                StorageAccounting::Estimated,
            )],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.relationships.len(), 1);
        let c = &rel.relationships[0];
        assert_eq!(c.kind, RelationshipKind::ContentDuplicate);
        assert_eq!(c.distinct_objects, None);
        assert!(c.alias_sets.is_empty());
    }

    #[test]
    fn cancelled_and_unsupported_runs_publish_no_relationships() {
        for status in [DuplicateStatus::Cancelled, DuplicateStatus::Unsupported] {
            let r = report(
                status,
                vec![group(
                    "x",
                    vec![member(1, "/a", Some((1, 1))), member(2, "/b", Some((1, 2)))],
                    Some(100),
                    StorageAccounting::Exact,
                )],
            );
            let rel = derive_relationships(&r, &RelationshipOptions::default());
            assert_eq!(rel.status, status);
            assert!(rel.relationships.is_empty(), "{status:?}: {rel:?}");
        }
    }

    #[test]
    fn records_cap_truncates_deterministically_and_counts() {
        let mut groups = Vec::new();
        for i in 0..50u64 {
            groups.push(group(
                &format!("g{i:04}"),
                vec![
                    member(i * 2, &format!("/{i:04}/a"), Some((1, i * 2))),
                    member(i * 2 + 1, &format!("/{i:04}/b"), Some((1, i * 2 + 1))),
                ],
                Some(100),
                StorageAccounting::Exact,
            ));
        }
        let r = report(DuplicateStatus::Completed, groups);
        let rel = derive_relationships(
            &r,
            &RelationshipOptions {
                max_relationship_records: 10,
            },
        );
        assert_eq!(rel.relationships.len(), 10);
        assert_eq!(rel.relationships_truncated, 40);
        // Truncation keeps the canonical-order head.
        let full = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.relationships, &full.relationships[..10]);
        // Stats describe PUBLISHED records; published + truncated = derived.
        assert_eq!(
            rel.stats.relationships + rel.relationships_truncated,
            full.stats.relationships
        );
    }

    #[test]
    fn undetermined_summary_carries_typed_reasons() {
        use crate::error::HashFailure;
        let mut r = report(DuplicateStatus::Completed, Vec::new());
        r.failures = vec![
            HashFailure::new(
                PathBuf::from("/denied"),
                HashFailureKind::Hash {
                    category: ErrorCategory::PermissionDenied,
                },
                "denied",
            ),
            HashFailure::new(PathBuf::from("/gone"), HashFailureKind::Vanished, "gone"),
            HashFailure::new(
                PathBuf::from("/swapped"),
                HashFailureKind::Replaced,
                "swapped",
            ),
        ];
        r.stats.failures = 3;
        r.stats.candidates_untracked_total = 5;
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.undetermined.failed, 3);
        assert_eq!(rel.undetermined.not_examined, 5);
        assert_eq!(rel.undetermined.failed_by_reason.len(), 3);
        // Sorted by reason (enum declaration order).
        assert_eq!(
            rel.undetermined.failed_by_reason[0].0,
            HashFailureKind::Hash {
                category: ErrorCategory::PermissionDenied
            }
        );
        assert_eq!(
            rel.undetermined.failed_by_reason[1].0,
            HashFailureKind::Replaced
        );
        assert_eq!(
            rel.undetermined.failed_by_reason[2].0,
            HashFailureKind::Vanished
        );
        assert_eq!(rel.undetermined.detail.len(), 3);
    }

    #[test]
    fn ordering_is_kind_then_identity_then_path() {
        let r = report(
            DuplicateStatus::Completed,
            vec![
                // Two pure-alias sets (objects (1,9) and (1,8)) and one
                // content duplicate across distinct objects. Shuffled group
                // order in the fixture; canonical order must not care.
                group(
                    "zeta",
                    vec![
                        member(1, "/z/a", Some((2, 1))),
                        member(2, "/z/b", Some((2, 2))),
                    ],
                    Some(100),
                    StorageAccounting::Exact,
                ),
                group(
                    "alpha-nine",
                    vec![
                        member(3, "/y/a", Some((1, 9))),
                        member(4, "/y/b", Some((1, 9))),
                    ],
                    None,
                    StorageAccounting::Exact,
                ),
                group(
                    "alpha-eight",
                    vec![
                        member(5, "/w/a", Some((1, 8))),
                        member(6, "/w/b", Some((1, 8))),
                    ],
                    None,
                    StorageAccounting::Exact,
                ),
            ],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        // Aliases first (by object id ascending), then content duplicates.
        assert_eq!(rel.relationships[0].kind, RelationshipKind::HardLinkAlias);
        assert_eq!(rel.relationships[0].object.unwrap().file_id, 8);
        assert_eq!(rel.relationships[1].kind, RelationshipKind::HardLinkAlias);
        assert_eq!(rel.relationships[1].object.unwrap().file_id, 9);
        assert_eq!(
            rel.relationships[2].kind,
            RelationshipKind::ContentDuplicate
        );
    }

    #[test]
    fn ids_are_deterministic_and_collision_free_within_a_report() {
        let r = report(
            DuplicateStatus::Completed,
            vec![
                group(
                    "one",
                    vec![member(1, "/a", Some((1, 7))), member(2, "/b", Some((1, 7)))],
                    None,
                    StorageAccounting::Exact,
                ),
                group(
                    "two",
                    vec![member(3, "/c", Some((1, 7))), member(4, "/d", Some((2, 9)))],
                    Some(100),
                    StorageAccounting::Exact,
                ),
            ],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        let mut ids: Vec<&str> = rel.relationships.iter().map(|x| x.id.as_str()).collect();
        ids.sort();
        let before = ids.clone();
        ids.dedup();
        assert_eq!(ids.len(), before.len(), "ids must be unique: {before:?}");
        let again = derive_relationships(&r, &RelationshipOptions::default());
        assert_eq!(rel.relationships, again.relationships);
        // The alias relationship derived from either group for object (1,7)
        // has the SAME id (identity-derived, not group-derived).
        let alias_ids: Vec<&str> = rel
            .relationships
            .iter()
            .filter(|x| x.kind == RelationshipKind::HardLinkAlias)
            .map(|x| x.id.as_str())
            .collect();
        assert_eq!(alias_ids.len(), 1, "one object → one alias relationship");
    }

    #[test]
    fn relationship_index_answers_all_queries() {
        let r = report(
            DuplicateStatus::Completed,
            vec![
                group(
                    "content-one",
                    vec![
                        member(1, "/a", Some((1, 7))),
                        member(2, "/b", Some((1, 7))),
                        member(3, "/c", Some((1, 9))),
                    ],
                    Some(100),
                    StorageAccounting::Exact,
                ),
                group(
                    "content-two",
                    vec![
                        member(4, "/d", Some((1, 11))),
                        member(5, "/e", Some((1, 12))),
                    ],
                    Some(100),
                    StorageAccounting::Exact,
                ),
            ],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        let index = RelationshipIndex::build(&rel);

        // Path lookups (aliases + content membership).
        assert_eq!(index.relationships_for_path(Path::new("/a")).len(), 2);
        assert_eq!(index.relationships_for_path(Path::new("/c")).len(), 1);
        assert_eq!(index.relationships_for_path(Path::new("/missing")).len(), 0);

        // Object lookups.
        assert_eq!(index.relationships_for_object(1, 7).len(), 2);
        assert_eq!(index.relationships_for_object(1, 9).len(), 1);
        assert_eq!(index.relationships_for_object(9, 9).len(), 0);

        // Content lookups (decode the published hex back to the digest).
        assert_eq!(index.duplicate_groups().len(), 2);
        let first = &index.duplicate_groups()[0];
        let hex = &first.content.as_ref().unwrap().sha256_hex;
        let mut digest = [0u8; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        let hash = ContentHash::from_bytes(&digest);
        assert_eq!(index.relationships_for_content(&hash).len(), 1);
        assert_eq!(index.hard_link_groups().len(), 1);
    }

    #[test]
    fn serialization_is_camel_case_and_stable() {
        let r = report(
            DuplicateStatus::Completed,
            vec![group(
                "ser",
                vec![
                    member(1, "/a", Some((1, 7))),
                    member(2, "/b", Some((1, 7))),
                    member(3, "/c", Some((2, 8))),
                ],
                Some(100),
                StorageAccounting::Exact,
            )],
        );
        let rel = derive_relationships(&r, &RelationshipOptions::default());
        let json = serde_json::to_string(&rel).unwrap();
        assert!(json.contains("\"contentDuplicate\""), "{json}");
        assert!(json.contains("\"sha256Hex\""), "{json}");
        assert!(json.contains("\"hardLinkAlias\""), "{json}");
        assert!(json.contains("\"OBJECT_IDENTITY_EQUAL\""), "{json}");
        assert!(json.contains("\"CONTENT_HASH_EQUAL\""), "{json}");
        let back: RelationshipReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rel);
    }
}
