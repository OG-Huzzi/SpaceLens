//! Pure construction of the unified system model.
//!
//! [`build_system_model`] performs **no I/O**: it joins facts that were
//! already observed by the other subsystems.
//!
//! ```text
//! Raw observations
//!       ↓  validated joins (path, object identity, content identity)
//! relationship derivation (contains / located-under / identity edges)
//!       ↓  evidence aggregation
//! conflict detection (one-vote-per-correlation-group, preserved conflicts)
//!       ↓  index construction
//! final immutable SystemModel
//! ```
//!
//! ## Complexity
//!
//! With `A` artifacts, `N` applications, `R` relationship records, `H`
//! history rows and `L` the configured limits:
//!
//! | Step | Cost |
//! |------|------|
//! | artifact admission | `O(A log max_artifacts)` |
//! | application admission | `O(N log max_applications)` |
//! | containment edges | `O(A · depth)` — ancestor walk per artifact, never `A × N` |
//! | identity/content edges | `O(A log A)` (grouped through a map) |
//! | relationship join | `O(R log max_edges)` |
//! | history projection | `O(H log max_historical_context)` |
//! | finalization (indexes) | `O((A + N + E) log …)` |
//!
//! There is deliberately **no `application × artifact` product**: an
//! application's claim on an artifact is established by containment of the
//! artifact's ancestors in the application's install roots, which is a walk
//! up the artifact's own ancestors (bounded by path depth), not a scan over
//! applications.
//!
//! ## Determinism
//!
//! The model is a pure function of the input fact *multiset*: every
//! collection is admitted into a [`BoundedTopK`] keyed by an immutable
//! canonical key, every tie is broken by a total order over content (never by
//! arrival), and every published collection is re-sorted canonically.
//! Duplicate input facts collapse, so `build(X) == build(X ++ X)`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use coresight_apps::{
    Admission, ApplicationRecord, BoundedTopK, CorrelationGroup, EvidenceAccumulator, EvidenceKind,
    EvidenceSource, EvidenceStrength, MatchedAttribute, OwnershipAssessment, OwnershipEvidence,
    ProbedKind, SourceStatus,
};
use coresight_capabilities::access::AccessState;
use coresight_capabilities::{CapabilityId, CapabilityStatus, CONTRACTS};
use coresight_classifier::{Category, Confidence as ClassificationConfidence, Subcategory};
use coresight_identity::ObjectIdentity;

use crate::model::{
    ApplicationNode, ApplicationState, ApplicationStateReason, ArtifactApplicationStatus,
    ArtifactNode, CapabilityState, HistoricalAssertion, HistoricalContext, HistoricalRelation,
    ModelTruncation, ObservationSummary, ProvenanceState, SourceStateSummary, SystemEdge,
    SystemEdgeKind, SystemModel, SystemModelLimits,
};
use crate::pathkey::ArtifactKey;

// ---------------------------------------------------------------------------
// Input facts
// ---------------------------------------------------------------------------

/// One observed artifact, as produced by the observation layer.
///
/// This is the system model's whole view of the filesystem: paths, canonical
/// object identities, verified content digests, and whatever the classifier
/// and the access layer already concluded. The builder never reads anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFact {
    /// The exact observed path (lossless).
    pub path: PathBuf,
    pub kind: ProbedKind,
    /// The canonical identity, with wide high bits intact, or `None`.
    pub identity: Option<ObjectIdentity>,
    /// A digest the identity engine actually proved, or `None`.
    pub content_sha256: Option<String>,
    pub size: Option<u64>,
    /// Access state observed for this artifact.
    pub access: AccessState,
    /// The classifier's verdict, when the caller ran the classifier.
    pub classification: Option<ArtifactClassification>,
}

/// The classifier's verdict about one artifact, copied (never re-derived).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactClassification {
    pub category: Category,
    pub subcategory: Option<Subcategory>,
    pub confidence: ClassificationConfidence,
}

/// One relationship record from the identity engine, projected into the
/// model's vocabulary. Only facts the engine actually proved are accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipFact {
    pub kind: RelationshipFactKind,
    /// The participating paths.
    pub paths: Vec<PathBuf>,
    /// The proven object identity for the relationship, when it has one.
    pub object: Option<ObjectIdentity>,
    /// The proven content digest, when the relationship has one.
    pub content_sha256: Option<String>,
}

/// The identity-engine relationship kinds the model understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationshipFactKind {
    /// Distinct objects, byte-identical content.
    ContentDuplicate,
    /// Different paths, one filesystem object.
    HardLinkAlias,
}

/// One application-intelligence fact set handed to the builder: the merged
/// inventory plus the Phase 6.2 analysis for those applications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationFact {
    pub record: ApplicationRecord,
    /// Candidate scopes derived by the Phase 6.2 root detector. This builder
    /// only promotes a byte-exact match to a source-recorded install location
    /// (except the desktop-entry Exec parent, which is derived) to observed;
    /// all other roots stay weak structural scope.
    pub install_roots: Vec<PathBuf>,
    /// An executable path surfaced by Phase 6.2 association. It is exact only
    /// when it equals `record.executable_path`; otherwise this builder keeps it
    /// candidate-level and weak rather than upgrading it to an observed fact.
    pub executable: Option<PathBuf>,
    /// Per-(application, path) ownership evidence from Phase 6.2.
    pub associations: Vec<(PathBuf, OwnershipEvidence)>,
}

/// One historical observation, projected from history storage by the caller.
/// The model NEVER derives history from current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryFact {
    pub run_id: String,
    pub path: PathBuf,
    pub identity: Option<ObjectIdentity>,
    pub category: Option<String>,
}

/// Everything the builder joins. Supplied wholly by the caller.
#[derive(Debug, Clone, Default)]
pub struct SystemModelInput {
    pub artifacts: Vec<ArtifactFact>,
    pub applications: Vec<ApplicationFact>,
    pub relationships: Vec<RelationshipFact>,
    pub history: Vec<HistoryFact>,
    /// Honest status of every application source that was consulted.
    pub source_coverage: Vec<coresight_apps::SourceCoverage>,
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/// One claim accumulator for a canonical (application, artifact) pair.
/// Evidence is kept through Phase 6.2's bounded, correlation-aware
/// accumulator; descriptive roles are collected independently from
/// ownership evidence.
///
/// The accumulator itself is O(`max_evidence_per_edge`) memory; the STORE
/// holding these is a [`BoundedTopK`] capped at `max_edges` pairs (a pair
/// that can never publish an edge needs no appraisal), so working memory
/// is a function of [`SystemModelLimits`], never of input cardinality.
struct ClaimFacts {
    evidence: EvidenceAccumulator,
    /// True only when every offered evidence item was structural
    /// containment. Such a claim may produce `AssociatedWith` but never
    /// `OwnedBy` and never counts as credible ownership.
    structural_only: bool,
    /// Role-specific bounded evidence prevents an unrelated strong ownership
    /// claim from upgrading a weak executable/root role.
    roles: BTreeMap<SystemEdgeKind, RoleFacts>,
    evidence_limit: usize,
}

struct RoleFacts {
    evidence: EvidenceAccumulator,
    structural_only: bool,
    provenance: ProvenanceState,
}

impl RoleFacts {
    fn new(limit: usize, provenance: ProvenanceState) -> Self {
        RoleFacts {
            evidence: EvidenceAccumulator::new(limit),
            structural_only: true,
            provenance,
        }
    }

    fn offer(
        &mut self,
        evidence: OwnershipEvidence,
        structural: bool,
        provenance: ProvenanceState,
    ) {
        self.evidence.offer(evidence);
        self.structural_only &= structural;
        self.provenance = self.provenance.min(provenance);
    }

    fn assessment(&self) -> OwnershipAssessment {
        if self.structural_only {
            OwnershipAssessment::Weak
        } else {
            self.evidence.assessment()
        }
    }
}

impl ClaimFacts {
    fn new(limit: usize) -> Self {
        ClaimFacts {
            evidence: EvidenceAccumulator::new(limit),
            structural_only: true,
            roles: BTreeMap::new(),
            evidence_limit: limit,
        }
    }

    fn offer(
        &mut self,
        evidence: OwnershipEvidence,
        structural: bool,
        role: Option<(SystemEdgeKind, ProvenanceState)>,
    ) {
        if let Some((kind, provenance)) = role {
            self.roles
                .entry(kind)
                .or_insert_with(|| RoleFacts::new(self.evidence_limit, provenance))
                .offer(evidence.clone(), structural, provenance);
        }
        self.evidence.offer(evidence);
        self.structural_only &= structural;
    }

    fn assessment(&self) -> OwnershipAssessment {
        if self.structural_only || self.evidence.retained_len() == 0 {
            // Structural evidence and evidence entirely omitted by a zero
            // retention bound cannot support a credible published claim.
            OwnershipAssessment::Weak
        } else {
            self.evidence.assessment()
        }
    }
}

/// Maximum participants whose complete pairwise relationship can fit under
/// the global edge limit. The integer binary search avoids overflow and caps
/// subsequent clique generation to O(max_edges) pairs.
fn max_relationship_members(max_edges: usize) -> usize {
    let mut low = 0usize;
    let mut high = max_edges.saturating_add(2);
    while low < high {
        let span = high - low;
        let mid = low + span / 2 + span % 2;
        let pairs = (mid as u128) * (mid.saturating_sub(1) as u128) / 2;
        if pairs <= max_edges as u128 {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// Build the unified system model. Pure, deterministic, bounded.
pub fn build_system_model(input: &SystemModelInput, limits: &SystemModelLimits) -> SystemModel {
    let mut truncation = ModelTruncation::default();

    // ---- 1. Artifact admission (bounded, canonical key order) ----------
    let mut artifact_admitted: BoundedTopK<String, ArtifactNode> =
        BoundedTopK::new(limits.max_artifacts);
    for fact in &input.artifacts {
        let key = ArtifactKey::of(&fact.path).to_string();
        let node = artifact_node(&key, fact);
        match artifact_admitted.offer(key, node, prefer_artifact) {
            Admission::Admitted | Admission::AdmittedEvicting | Admission::Merged => {}
            Admission::Refused => {}
        }
    }
    let (artifact_items, artifacts_overflow) = artifact_admitted.into_sorted();
    truncation.artifacts_truncated = artifacts_overflow;
    let mut artifacts: Vec<ArtifactNode> = artifact_items.into_iter().map(|(_, n)| n).collect();
    artifacts.sort_by(|a, b| a.key.cmp(&b.key));
    let artifact_index: BTreeMap<String, usize> = artifacts
        .iter()
        .enumerate()
        .map(|(i, a)| (a.key.clone(), i))
        .collect();

    // ---- 1b. Precomputed artifact structure (bounded, deterministic) ----
    // Built ONCE here so per-application state resolution never rescans the
    // artifact list: exact-key lookup, parent→children grouping, and
    // install-root→descendant grouping. All three are O(A) memory.
    let artifact_structure = ArtifactStructure::build(&artifacts);

    // ---- 2. Application admission (bounded, canonical id order) -------
    // Duplicate records under one id merge commutatively: the total-order
    // winner supplies every field EXCEPT provenance, which unions across
    // all duplicates (canonically ordered, deduplicated). The merged result
    // is a pure function of the duplicate SET — never of arrival order.
    let mut app_admitted: BoundedTopK<String, ApplicationNode> =
        BoundedTopK::new(limits.max_applications);
    for fact in &input.applications {
        let mut node = application_node(fact, &artifacts, &artifact_structure);
        node.provenance.sort();
        node.provenance.dedup();
        app_admitted.offer(fact.record.id.0.clone(), node, |n, e| {
            prefer_application(n, e)
        });
        // Union provenance across duplicates commutatively: whatever the
        // winner was, the retained node carries every observed source.
        if let Some(held) = app_admitted.get_mut(&fact.record.id.0) {
            for source in &fact.record.provenance {
                if !held.provenance.contains(source) {
                    held.provenance.push(source.clone());
                }
            }
            held.provenance.sort();
            held.provenance.dedup();
        }
    }
    let (app_items, apps_overflow) = app_admitted.into_sorted();
    truncation.applications_truncated = apps_overflow;
    let mut applications: Vec<ApplicationNode> = app_items.into_iter().map(|(_, n)| n).collect();
    applications.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    // ---- 3. Edge admission -------------------------------------------
    let mut edges: BoundedTopK<(u8, String, String), SystemEdge> =
        BoundedTopK::new(limits.max_edges);
    let mut evidence_truncated = 0u64;

    // 3a. Containment: walk each artifact's own ancestors. Bounded by path
    // depth, never by the application count.
    let artifact_by_key: BTreeMap<String, &ArtifactNode> =
        artifacts.iter().map(|a| (a.key.clone(), a)).collect();
    for node in &artifacts {
        for ancestor_key in ancestor_keys(&node.path) {
            if artifact_by_key.contains_key(&ancestor_key) {
                offer_edge(
                    &mut edges,
                    &mut truncation.edges_truncated,
                    &mut evidence_truncated,
                    limits,
                    SystemEdge {
                        kind: SystemEdgeKind::Contains,
                        domain: SystemEdgeKind::Contains.domain(),
                        from: ancestor_key.clone(),
                        to: node.key.clone(),
                        assessment: OwnershipAssessment::Direct,
                        provenance: ProvenanceState::Observed,
                        evidence: Vec::new(),
                    },
                );
                offer_edge(
                    &mut edges,
                    &mut truncation.edges_truncated,
                    &mut evidence_truncated,
                    limits,
                    SystemEdge {
                        kind: SystemEdgeKind::LocatedUnder,
                        domain: SystemEdgeKind::LocatedUnder.domain(),
                        from: node.key.clone(),
                        to: ancestor_key,
                        assessment: OwnershipAssessment::Direct,
                        provenance: ProvenanceState::Observed,
                        evidence: Vec::new(),
                    },
                );
            }
        }
    }

    // 3b. One bounded, correlation-aware accumulator per (application,
    // artifact), held in a BoundedTopK capped at `max_edges` pairs: a pair
    // that can never publish an edge needs no appraisal, so working memory
    // is O(max_edges × max_evidence_per_edge) — a function of the limits,
    // never of input cardinality. Dropped pairs are counted exactly in
    // `claims_truncated`, and their artifacts are remembered (one flag per
    // artifact, O(max_artifacts)) so truncation can never surface as
    // "no claim".
    //
    // Claim loops below consider every input application fact: a claim
    // offered for a retained artifact is an observed claim even when the
    // application node itself did not survive the application bound
    // (truncation memory). Facts naming artifacts the model does not hold
    // seed nothing. Edges that survive still require both endpoints to
    // exist (see 3b-bis), so dropped applications can never smuggle
    // phantom claims into the published graph.
    //
    // ORDER MATTERS for truncation honesty: input applications stream in
    // canonical id order (sorted once, O(N log N)), so the bounded claim
    // store deterministically retains the canonically-smallest pairs no
    // matter how the caller ordered its facts.
    let mut claim_evidence: BoundedTopK<(String, String), ClaimFacts> =
        BoundedTopK::new(limits.max_edges.max(1));
    // Artifact keys for which ANY claim pair was observed (retained or
    // dropped). Bounded by the retained artifact count.
    let mut association_seen: BTreeSet<String> = BTreeSet::new();
    let claim_for = |map: &mut BoundedTopK<(String, String), ClaimFacts>,
                     seen: &mut BTreeSet<String>,
                     truncated: &mut u64,
                     app: &str,
                     artifact_key: &str,
                     evidence: OwnershipEvidence,
                     structural: bool,
                     role: Option<(SystemEdgeKind, ProvenanceState)>| {
        seen.insert(artifact_key.to_string());
        let key = (app.to_string(), artifact_key.to_string());
        if let Some(held) = map.get_mut(&key) {
            held.offer(evidence, structural, role);
            return;
        }
        let mut facts = ClaimFacts::new(limits.max_evidence_per_edge);
        facts.offer(evidence, structural, role);
        if matches!(map.offer(key, facts, |_, _| false), Admission::Refused) {
            *truncated += 1;
        }
    };
    // Stream the caller's application facts directly. Every accumulator below
    // uses canonical keys and a commutative merge, so no sort/copy of the full
    // input list is needed to make the result order-independent.
    for fact in &input.applications {
        let app_id = &fact.record.id;
        // A root emitted by the detector is only a scope unless it is the
        // exact install location recorded in the application metadata. Do
        // not promote a name/layout-derived root to direct, observed
        // ownership merely because it crossed this module boundary.
        for root in fact
            .install_roots
            .iter()
            .chain(fact.record.install_location.iter())
        {
            let key = ArtifactKey::of(root).to_string();
            if !artifact_index.contains_key(&key) {
                continue;
            }
            let recorded = fact.record.source != coresight_apps::ApplicationSource::DesktopEntry
                && fact
                    .record
                    .install_location
                    .as_ref()
                    .is_some_and(|recorded| same_artifact_path(recorded, root));
            let edge_evidence = if recorded {
                OwnershipEvidence::new(
                    EvidenceKind::InstallLocation,
                    EvidenceSource::for_application_source(&fact.record.source),
                    EvidenceStrength::Direct,
                    CorrelationGroup::SourceRecord(fact.record.source.clone()),
                    coresight_apps::AssociationScope::ThisMachine,
                    root.clone(),
                    MatchedAttribute::InstallLocation,
                    Some(fact.record.name.clone()),
                )
            } else {
                OwnershipEvidence::new(
                    EvidenceKind::InstallRootContainment,
                    EvidenceSource::FilesystemObservation,
                    EvidenceStrength::Weak,
                    CorrelationGroup::InstallRootStructure,
                    coresight_apps::AssociationScope::ThisMachine,
                    root.clone(),
                    MatchedAttribute::InstallRoot,
                    None,
                )
            }
            .with_matched_path(root.clone());
            let provenance = if recorded {
                ProvenanceState::Observed
            } else {
                ProvenanceState::Inferred
            };
            if limits.max_evidence_per_edge > 0 {
                offer_edge(
                    &mut edges,
                    &mut truncation.edges_truncated,
                    &mut evidence_truncated,
                    limits,
                    SystemEdge {
                        kind: SystemEdgeKind::ApplicationInstallRoot,
                        domain: SystemEdgeKind::ApplicationInstallRoot.domain(),
                        from: app_id.0.clone(),
                        to: key.clone(),
                        assessment: if recorded {
                            OwnershipAssessment::Direct
                        } else {
                            OwnershipAssessment::Weak
                        },
                        provenance,
                        evidence: vec![edge_evidence.clone()],
                    },
                );
            }
            claim_for(
                &mut claim_evidence,
                &mut association_seen,
                &mut truncation.claims_truncated,
                &app_id.0,
                &key,
                edge_evidence,
                !recorded,
                Some((SystemEdgeKind::ApplicationInstallRoot, provenance)),
            );
        }
        // Exact executable metadata may be Observed; a separate path
        // candidate without that exact record is only weak, candidate-level
        // evidence. Never upgrade an inferred/name-matched path merely because
        // it was projected into this input struct.
        for exe in fact
            .record
            .executable_path
            .iter()
            .chain(fact.executable.iter())
        {
            let key = ArtifactKey::of(exe).to_string();
            if !artifact_index.contains_key(&key) {
                continue;
            }
            let recorded = fact
                .record
                .executable_path
                .as_ref()
                .is_some_and(|recorded| same_artifact_path(recorded, exe));
            let (kind, source, strength, group, provenance, assessment) = if recorded {
                (
                    EvidenceKind::ExactExecutablePath,
                    EvidenceSource::ExecutableMetadata,
                    EvidenceStrength::Strong,
                    CorrelationGroup::SourceRecord(fact.record.source.clone()),
                    ProvenanceState::Observed,
                    OwnershipAssessment::Strong,
                )
            } else {
                (
                    EvidenceKind::FilenameSimilarity,
                    EvidenceSource::FilesystemPathHeuristic,
                    EvidenceStrength::Weak,
                    CorrelationGroup::NameDerived,
                    ProvenanceState::Candidate,
                    OwnershipAssessment::Weak,
                )
            };
            let edge_evidence = OwnershipEvidence::new(
                kind,
                source,
                strength,
                group,
                coresight_apps::AssociationScope::ThisMachine,
                exe.clone(),
                MatchedAttribute::ExecutablePath,
                Some(fact.record.name.clone()),
            )
            .with_matched_path(exe.clone());
            if limits.max_evidence_per_edge > 0 {
                offer_edge(
                    &mut edges,
                    &mut truncation.edges_truncated,
                    &mut evidence_truncated,
                    limits,
                    SystemEdge {
                        kind: SystemEdgeKind::ApplicationExecutable,
                        domain: SystemEdgeKind::ApplicationExecutable.domain(),
                        from: app_id.0.clone(),
                        to: key.clone(),
                        assessment,
                        provenance,
                        evidence: vec![edge_evidence.clone()],
                    },
                );
            }
            claim_for(
                &mut claim_evidence,
                &mut association_seen,
                &mut truncation.claims_truncated,
                &app_id.0,
                &key,
                edge_evidence,
                false,
                Some((SystemEdgeKind::ApplicationExecutable, provenance)),
            );
        }
        // Per-path association evidence from Phase 6.2 is COLLECTED here and
        // published once per (application, artifact) below, so one
        // application can never appear as several claimants of one artifact
        // merely because it offered several evidence items.
        for (path, evidence) in &fact.associations {
            let key = ArtifactKey::of(path).to_string();
            let Some(node) = artifact_by_key.get(&key) else {
                continue;
            };
            let structural = evidence.kind.is_structural();
            // The descriptive role edge is chosen from the CLASSIFIER's
            // verdict — classification is never rewritten by ownership, and
            // ownership never rewrites classification.
            let role = if structural {
                None
            } else {
                let role = claim_edge_kind(node);
                if role == SystemEdgeKind::AssociatedWith {
                    None
                } else {
                    Some((role, ProvenanceState::Inferred))
                }
            };
            claim_for(
                &mut claim_evidence,
                &mut association_seen,
                &mut truncation.claims_truncated,
                &app_id.0,
                &key,
                evidence.clone(),
                structural,
                role,
            );
        }
    }

    // 3b-ter. Containment-derived relationships: any observed artifact lying
    // under an application's install root is RELATED to that application.
    //
    // This is deliberately STRUCTURAL evidence only. It makes the model
    // useful (an application's install tree is related to it) while keeping
    // the containment/ownership distinction intact: the assessment stays
    // below the credibility line, so it can never make the artifact
    // "owned" and never inflates a claimant count.
    //
    // Cost: one ancestor walk per artifact (bounded by path depth), never an
    // applications × artifacts full scan. Both root groups and each root's
    // owner set have explicit admission limits. If either bound drops facts,
    // affected knowledge remains incomplete (never absence).
    {
        // install root key → bounded (application key → root path) set.
        // The product of these limits bounds this secondary index.
        let mut roots_by_key: BoundedTopK<String, BoundedTopK<String, PathBuf>> =
            BoundedTopK::new(limits.max_edges.max(1));
        for fact in &input.applications {
            let app_key = fact.record.id.0.clone();
            for root in fact
                .install_roots
                .iter()
                .chain(fact.record.install_location.iter())
            {
                let key = ArtifactKey::of(root).to_string();
                if let Some(owners) = roots_by_key.get_mut(&key) {
                    owners.offer(app_key.clone(), root.clone(), |_, _| false);
                } else {
                    let mut owners = BoundedTopK::new(limits.max_applications);
                    owners.offer(app_key.clone(), root.clone(), |_, _| false);
                    roots_by_key.offer(key, owners, |_, _| false);
                }
            }
        }
        let dropped_root_groups = roots_by_key.overflow();
        truncation.roots_truncated += dropped_root_groups;
        let dropped_root_owners: u64 = roots_by_key
            .iter()
            .map(|(_, owners)| owners.overflow())
            .sum();
        truncation.claims_truncated += dropped_root_owners;

        // A dropped root key cannot be retained in a side set without
        // reintroducing input-cardinality memory. Conservatively mark every
        // retained artifact as having incomplete association knowledge when
        // any root group was dropped; this can overstate uncertainty, never
        // invent absence.
        if dropped_root_groups > 0 {
            association_seen.extend(artifacts.iter().map(|node| node.key.clone()));
        }

        if roots_by_key.is_empty() {
            // No retained roots: nothing structural to derive.
        } else {
            for node in &artifacts {
                for ancestor_key in ancestor_keys(&node.path) {
                    let Some(owners) = roots_by_key.get(&ancestor_key) else {
                        continue;
                    };
                    if owners.overflow() > 0 {
                        association_seen.insert(node.key.clone());
                    }
                    for (app_key, root) in owners.iter() {
                        // Do not restate a claim the caller already supplied
                        // explicitly for this (app, artifact) pair.
                        if claim_evidence
                            .get(&(app_key.clone(), node.key.clone()))
                            .is_some()
                        {
                            continue;
                        }
                        let evidence = OwnershipEvidence::new(
                            EvidenceKind::InstallRootContainment,
                            EvidenceSource::FilesystemObservation,
                            EvidenceStrength::Weak,
                            CorrelationGroup::InstallRootStructure,
                            coresight_apps::AssociationScope::ThisMachine,
                            node.path.clone(),
                            MatchedAttribute::InstallRoot,
                            None,
                        )
                        .with_matched_path(root.clone());
                        let key = node.key.clone();
                        claim_for(
                            &mut claim_evidence,
                            &mut association_seen,
                            &mut truncation.claims_truncated,
                            app_key,
                            &key,
                            evidence,
                            true,
                            None,
                        );
                    }
                    break; // the deepest containing root is enough
                }
            }
        }
    }

    // 3b-bis. Publish the claim view per (application, artifact). The
    // ownership appraisal lives in ONE place — the accumulator's
    // correlation-aware assessment — so the same underlying signal can never
    // count once for appraisal and again for edge publication.
    //
    // Exactly ONE ownership edge is published per pair: `OwnedBy` when the
    // accumulator's assessment is credible, `AssociatedWith` otherwise.
    // Structural-only evidence (the accumulator deliberately reports
    // `Weak`) can therefore never reach `OwnedBy`. Descriptive role edges
    // collected during offering are published alongside; the fallback
    // `AssociatedWith` role is skipped because the ownership edge already
    // says exactly that, and publishing both would restate one fact twice.
    //
    // Pairs stream in canonical (app, artifact) order, so a full claim
    // store deterministically evicts the largest pairs first.
    //
    // Both endpoints must exist as published NODES: a claim whose
    // application did not survive the application bound publishes no edge
    // (no phantom claims), but the artifact keeps its truncation memory
    // via `association_seen` (see step 7).
    let published_app_ids: BTreeSet<&str> = applications.iter().map(|a| a.id.0.as_str()).collect();
    let (claim_items, claim_overflow) = claim_evidence.into_sorted();
    truncation.claims_truncated += claim_overflow;
    for ((app_key, artifact_key), claim) in &claim_items {
        if !published_app_ids.contains(app_key.as_str()) {
            continue;
        }
        let (evidence, evidence_overflow) = claim.evidence.clone().into_parts();
        evidence_truncated += evidence_overflow;
        let assessment = claim.assessment();
        let credible = assessment.is_credible();
        let (ownership_edge, ownership_provenance) = if credible {
            (SystemEdgeKind::OwnedBy, ProvenanceState::Inferred)
        } else {
            (SystemEdgeKind::AssociatedWith, ProvenanceState::Inferred)
        };
        offer_edge(
            &mut edges,
            &mut truncation.edges_truncated,
            &mut evidence_truncated,
            limits,
            SystemEdge {
                kind: ownership_edge,
                domain: SystemEdgeKind::AssociatedWith.domain(),
                from: app_key.clone(),
                to: artifact_key.clone(),
                assessment,
                provenance: ownership_provenance,
                evidence: evidence.clone(),
            },
        );
        for (role, role_facts) in &claim.roles {
            if !credible && *role == SystemEdgeKind::OwnedBy {
                continue;
            }
            let (role_evidence, role_overflow) = role_facts.evidence.clone().into_parts();
            evidence_truncated += role_overflow;
            if role_evidence.is_empty() {
                continue;
            }
            offer_edge(
                &mut edges,
                &mut truncation.edges_truncated,
                &mut evidence_truncated,
                limits,
                SystemEdge {
                    kind: *role,
                    domain: role.domain(),
                    from: app_key.clone(),
                    to: artifact_key.clone(),
                    assessment: role_facts.assessment(),
                    provenance: role_facts.provenance,
                    evidence: role_evidence,
                },
            );
        }
    }

    // 3c. Identity-engine relationships, VALIDATED against the canonical
    // artifact facts. A relationship fact is a caller projection, not graph
    // truth: every endpoint must be a retained artifact node, and the fact's
    // proofs must agree with what those nodes carry.
    //
    // * `HardLinkAlias`: every endpoint node must prove an object identity,
    //   all proven identities must be equal under FULL comparison (volume,
    //   file id, AND high bits — never narrowed), and a fact-level `object`
    //   proof, when supplied, must equal them too. Missing or disagreeing
    //   proof → rejected with exact accounting.
    // * `ContentDuplicate`: every endpoint node must carry the proven
    //   digest; a fact-level `content_sha256`, when supplied, must equal it.
    //   Missing digests or a disagreeing fact digest → rejected. When all
    //   endpoint identities are known and any two agree, the fact describes
    //   an alias, not a duplicate → rejected (never silently recast).
    //   Every identity must be known and distinct before the model can assert
    //   `DuplicateOf`; an unknown identity is insufficient proof.
    let relationship_member_limit = max_relationship_members(limits.max_edges);
    for rel in &input.relationships {
        // Resolve endpoints into a bounded, canonical set. A relationship
        // with any unknown endpoint or too many participants is rejected as
        // a whole; silently dropping a participant could turn an invalid
        // proof into a smaller, plausible-looking relationship. The member
        // limit also caps clique generation to O(max_edges) candidate pairs.
        let mut member_map: BTreeMap<String, &ArtifactNode> = BTreeMap::new();
        let mut invalid_endpoint = false;
        let mut too_many_members = false;
        for path in &rel.paths {
            let key = ArtifactKey::of(path).to_string();
            if member_map.contains_key(&key) {
                continue;
            }
            let Some(node) = artifact_by_key.get(&key).copied() else {
                invalid_endpoint = true;
                break;
            };
            if member_map.len() >= relationship_member_limit {
                too_many_members = true;
                break;
            }
            member_map.insert(key, node);
        }
        if invalid_endpoint || too_many_members || member_map.len() < 2 {
            truncation.relationships_rejected += 1;
            continue;
        }
        let members: Vec<&ArtifactNode> = member_map.into_values().collect();
        let valid = match rel.kind {
            RelationshipFactKind::HardLinkAlias => {
                // Every member must prove an identity, all equal, full
                // comparison (high bits included — never narrowed).
                let mut identities = members.iter().map(|m| m.identity);
                let first = match identities.next() {
                    Some(Some(id)) => id,
                    _ => {
                        truncation.relationships_rejected += 1;
                        continue;
                    }
                };
                identities.all(|id| id == Some(first)) && rel.object.is_none_or(|o| o == first)
            }
            RelationshipFactKind::ContentDuplicate => {
                // Every member must carry one shared digest; the fact's own
                // digest, when supplied, must be that digest.
                let mut digests = members.iter().map(|m| m.content_sha256.as_deref());
                let first = match digests.next() {
                    Some(Some(d)) => d,
                    _ => {
                        truncation.relationships_rejected += 1;
                        continue;
                    }
                };
                let agreed = digests.all(|d| d == Some(first))
                    && rel.content_sha256.as_deref().is_none_or(|d| d == first);
                // "Distinct objects" requires complete, pairwise-distinct
                // identities. Missing identity is insufficient proof; known
                // equality proves an alias and contradicts this relationship.
                let distinct = {
                    let mut seen: BTreeSet<ObjectIdentity> = BTreeSet::new();
                    members
                        .iter()
                        .all(|m| m.identity.is_some_and(|id| seen.insert(id)))
                };
                agreed && distinct
            }
        };
        if !valid {
            truncation.relationships_rejected += 1;
            continue;
        }
        let keys: Vec<String> = members.iter().map(|m| m.key.clone()).collect();
        let kind = match rel.kind {
            RelationshipFactKind::ContentDuplicate => SystemEdgeKind::DuplicateOf,
            RelationshipFactKind::HardLinkAlias => SystemEdgeKind::HardLinkAliasOf,
        };
        let (group, evidence_kind, strength) = match rel.kind {
            RelationshipFactKind::ContentDuplicate => (
                CorrelationGroup::ContentIdentity,
                EvidenceKind::ContentDigestMatch,
                EvidenceStrength::Strong,
            ),
            RelationshipFactKind::HardLinkAlias => (
                CorrelationGroup::ObjectIdentity,
                EvidenceKind::ObjectIdentityMatch,
                EvidenceStrength::Direct,
            ),
        };
        // Pairwise edges, canonically ordered so the same set of members
        // always yields the same edge set.
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                let (a, b) = (keys[i].clone(), keys[j].clone());
                let (from, to) = if a <= b { (a, b) } else { (b, a) };
                let evidence = vec![OwnershipEvidence::new(
                    evidence_kind,
                    match rel.kind {
                        RelationshipFactKind::ContentDuplicate => EvidenceSource::ContentHash,
                        RelationshipFactKind::HardLinkAlias => {
                            EvidenceSource::FilesystemObservation
                        }
                    },
                    strength,
                    group.clone(),
                    coresight_apps::AssociationScope::ThisMachine,
                    PathBuf::from(&from),
                    MatchedAttribute::ObjectIdentity,
                    None,
                )];
                offer_edge(
                    &mut edges,
                    &mut truncation.edges_truncated,
                    &mut evidence_truncated,
                    limits,
                    SystemEdge {
                        kind,
                        domain: kind.domain(),
                        from,
                        to,
                        assessment: match kind {
                            SystemEdgeKind::HardLinkAliasOf => OwnershipAssessment::Direct,
                            _ => OwnershipAssessment::Strong,
                        },
                        provenance: ProvenanceState::Observed,
                        evidence,
                    },
                );
            }
        }
    }

    // ---- 4. Historical context (only from supplied history facts) -----
    // Duplicate (run, path) facts are a caller projection, never arrival
    // truth: identical payloads collapse, contradictory payloads are BOTH
    // preserved under distinct canonical keys (conflict preservation — the
    // model never picks a historical winner by arrival order). Bounded by
    // `max_historical_context` with exact overflow accounting.
    let mut history_admitted: BoundedTopK<(String, String, HistoryPayloadRank), HistoricalContext> =
        BoundedTopK::new(limits.max_historical_context);
    for fact in &input.history {
        let key = ArtifactKey::of(&fact.path).to_string();
        let ctx = HistoricalContext {
            run_id: fact.run_id.clone(),
            path: fact.path.clone(),
            identity: fact.identity,
            category: fact.category.clone(),
            provenance: ProvenanceState::Observed,
        };
        history_admitted.offer(
            (
                fact.run_id.clone(),
                key,
                HistoryPayloadRank::of(fact.identity, fact.category.as_deref()),
            ),
            ctx,
            |_, _| false,
        );
    }
    let (history_items, history_overflow) = history_admitted.into_sorted();
    truncation.historical_context_truncated = history_overflow;
    let mut historical_context: Vec<HistoricalContext> =
        history_items.into_iter().map(|(_, c)| c).collect();
    // Non-decreasing (run, path): conflicting rows share one key by design.
    historical_context.sort_by(|a, b| {
        a.run_id.cmp(&b.run_id).then(
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes()),
        )
    });
    // Historical assertions join context rows to artifacts that the CURRENT
    // model also holds. A caller-supplied context record for an artifact the
    // model does not hold is still published as data, but it anchors no
    // assertion: an assertion is ABOUT a node, and there is no node. This
    // keeps "no invented facts" literal.
    //
    // Assertions are node-attached context, never graph edges: a stored
    // "move" is quoted as what it is ("history proves this path previously
    // referred to a different object"), not as a self-loop that suggests a
    // relationship between two nodes. Nothing is inferred from current
    // state — the recorded identity is quoted from the caller's fact.
    let mut historical_assertions: Vec<HistoricalAssertion> = Vec::new();
    for ctx in &historical_context {
        let key = ArtifactKey::of(&ctx.path).to_string();
        let Some(node) = artifact_by_key.get(&key) else {
            continue;
        };
        // Same proven identity on both sides: alias observed. Different
        // proven identities: the path previously referred to another
        // object. Anything unproven on either side: honestly unproven —
        // NEVER asserted as sameness without proof.
        let relation = match (ctx.identity, node.identity) {
            (Some(hist), Some(now)) if hist != now => HistoricalRelation::ObjectReplaced,
            (Some(_), Some(_)) => HistoricalRelation::SameObjectObserved,
            _ => HistoricalRelation::IdentityUnproven,
        };
        let evidence = vec![OwnershipEvidence::new(
            EvidenceKind::HistoricalObservation,
            EvidenceSource::HistoryObservation,
            EvidenceStrength::Strong,
            CorrelationGroup::HistoricalObservation,
            coresight_apps::AssociationScope::ThisMachine,
            ctx.path.clone(),
            MatchedAttribute::ObjectIdentity,
            Some(ctx.run_id.clone()),
        )];
        historical_assertions.push(HistoricalAssertion {
            artifact_key: key,
            run_id: ctx.run_id.clone(),
            path: ctx.path.clone(),
            recorded_identity: ctx.identity,
            current_identity: node.identity,
            relation,
            category: ctx.category.clone(),
            provenance: ProvenanceState::Observed,
            evidence,
        });
    }
    historical_assertions.sort_by(|a, b| a.order_key().cmp(&b.order_key()));

    // ---- 5. Publish edges canonically ---------------------------------
    // The global edge store already counted its own refusals into
    // `edges_truncated` at admission; `into_sorted` only reports the final
    // overflow (evictions during admission), which is added, not assigned.
    let (edge_items, edges_overflow) = edges.into_sorted();
    truncation.edges_truncated += edges_overflow;
    truncation.evidence_truncated = evidence_truncated;
    let mut edge_list: Vec<SystemEdge> = edge_items.into_iter().map(|(_, e)| e).collect();
    edge_list.sort_by_key(|a| a.order_key());

    // ---- 6. Per-node edge bound (also bounded by admission) -----------
    let (edge_list, per_node_overflow) = bound_edges_per_node(edge_list, limits);
    truncation.edges_per_node_truncated = per_node_overflow;

    // ---- 7. Derive artifact application status from the FINAL edges ----
    // ...but with truncation memory: `association_seen` records every
    // artifact key for which a claim pair was EVER observed (retained or
    // dropped). An artifact with no surviving claim edge that WAS seen with
    // a claim is AssociationTruncated — incomplete knowledge, never
    // absence. Only an artifact never seen with any claim may become
    // genuinely Unassociated.
    let mut claims_by_artifact: BTreeMap<String, Vec<(String, OwnershipAssessment)>> =
        BTreeMap::new();
    for e in &edge_list {
        if !e.kind.asserts_ownership() {
            continue;
        }
        claims_by_artifact
            .entry(e.to.clone())
            .or_default()
            .push((e.from.clone(), e.assessment));
    }
    let app_source_status = aggregate_source_status(&input.source_coverage);
    for node in &mut artifacts {
        let claims = claims_by_artifact.get(&node.key);
        let credible = claims
            .map(|c| c.iter().filter(|(_, a)| a.is_credible()).count())
            .unwrap_or(0);
        let strong = claims
            .map(|c| {
                c.iter()
                    .filter(|(_, a)| {
                        matches!(a, OwnershipAssessment::Strong | OwnershipAssessment::Direct)
                    })
                    .count()
            })
            .unwrap_or(0);
        node.credible_claimants = credible as u32;
        node.application_status = if strong >= 2 {
            ArtifactApplicationStatus::Conflicting
        } else if credible >= 2 {
            ArtifactApplicationStatus::Shared
        } else if credible == 1 {
            ArtifactApplicationStatus::Associated
        } else if claims.map(|c| !c.is_empty()).unwrap_or(false) {
            ArtifactApplicationStatus::Uncertain
        } else if association_seen.contains(&node.key) {
            // Claims existed but no claim edge survived the bounds.
            ArtifactApplicationStatus::AssociationTruncated
        } else {
            match app_source_status {
                SourceUsability::Usable => ArtifactApplicationStatus::Unassociated,
                SourceUsability::Unsupported => ArtifactApplicationStatus::AssociationUnsupported,
                SourceUsability::Unavailable => ArtifactApplicationStatus::AssociationUnavailable,
                SourceUsability::Failed => ArtifactApplicationStatus::AssociationFailed,
            }
        };
    }
    // SharedBy edges: publish the explicit sharing fact once per artifact
    // that several applications relate to. The app pairs fan out
    // quadratically in the claimant count, so they are admitted through a
    // BoundedTopK (never materialized unbounded): canonically-smallest
    // pairs win, the rest count exactly into `edges_truncated`.
    let mut shared_top: BoundedTopK<(String, String), SystemEdge> =
        BoundedTopK::new(limits.max_edges);
    for node in &artifacts {
        if !matches!(
            node.application_status,
            ArtifactApplicationStatus::Shared | ArtifactApplicationStatus::Conflicting
        ) {
            continue;
        }
        let Some(claims) = claims_by_artifact.get(&node.key) else {
            continue;
        };
        let mut apps: Vec<&(String, OwnershipAssessment)> = claims.iter().collect();
        apps.sort();
        for i in 0..apps.len() {
            for j in (i + 1)..apps.len() {
                let (from, to) = if apps[i].0 <= apps[j].0 {
                    (apps[i].0.clone(), apps[j].0.clone())
                } else {
                    (apps[j].0.clone(), apps[i].0.clone())
                };
                let edge = SystemEdge {
                    kind: SystemEdgeKind::SharedBy,
                    domain: SystemEdgeKind::SharedBy.domain(),
                    from: from.clone(),
                    to: to.clone(),
                    assessment: OwnershipAssessment::Moderate,
                    provenance: ProvenanceState::Inferred,
                    evidence: Vec::new(),
                };
                if matches!(
                    shared_top.offer((from, to), edge, |_, _| false),
                    Admission::Refused
                ) {
                    truncation.edges_truncated += 1;
                }
            }
        }
    }
    let (shared_items, shared_overflow) = shared_top.into_sorted();
    truncation.edges_truncated += shared_overflow;
    let (mut edge_list, extra_overflow) = {
        let mut all = edge_list;
        all.extend(shared_items.into_iter().map(|(_, e)| e));
        let (bounded, over) = bound_edges_per_node(all, limits);
        (bounded, over)
    };
    truncation.edges_per_node_truncated += extra_overflow;
    // Canonical order, with a deterministic preference for the stronger
    // same-fact edge, then remove exact duplicate facts. Arrival order can
    // never decide which of two descriptions of one fact survives.
    edge_list.sort_by(|a, b| {
        a.order_key()
            .cmp(&b.order_key())
            .then(b.rank().cmp(&a.rank()))
    });
    edge_list.dedup_by(|a, b| a.fact_key() == b.fact_key());

    // ---- 8. Insights and candidates -----------------------------------
    let derived = crate::insight::derive(
        &artifacts,
        &applications,
        &edge_list,
        &historical_assertions,
        &input.source_coverage,
        limits,
    );
    truncation.insights_truncated = derived.insights_truncated;
    truncation.candidates_truncated = derived.candidates_truncated;
    truncation.evidence_truncated += derived.evidence_truncated;
    let insights = derived.insights;
    let candidates = derived.candidates;

    // ---- 9. Observation summary ---------------------------------------
    let (observations, source_states_truncated) =
        observation_summary(&artifacts, &input.source_coverage, limits.max_source_states);
    truncation.source_states_truncated = source_states_truncated;

    SystemModel::finalize(crate::model::FinalizeInput {
        artifacts,
        applications,
        edges: edge_list,
        historical_context,
        historical_assertions,
        insights,
        candidates,
        observations,
        truncation,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// How usable the application sources were overall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceUsability {
    Usable,
    Unsupported,
    Unavailable,
    Failed,
}

/// Aggregate the per-source statuses into one honest usability verdict. A
/// partly-usable set is `Usable`; only an entirely-unusable set degrades, and
/// `Unsupported` wins over `Unavailable` so a platform gap is never reported
/// as an absent machine.
fn aggregate_source_status(coverage: &[coresight_apps::SourceCoverage]) -> SourceUsability {
    if coverage.is_empty() {
        // No source was consulted at all: this is an availability gap, not
        // an observed absence.
        return SourceUsability::Unavailable;
    }
    let mut any_usable = false;
    let mut any_unsupported = false;
    let mut any_failed = false;
    for c in coverage {
        match c.status {
            SourceStatus::Complete | SourceStatus::Partial => any_usable = true,
            SourceStatus::Unsupported => any_unsupported = true,
            SourceStatus::Failed => any_failed = true,
            // `Unavailable` and empty coverage both mean the machine state
            // was not readable: the fallback below covers them.
            SourceStatus::Unavailable => {}
        }
    }
    if any_usable {
        SourceUsability::Usable
    } else if any_unsupported {
        SourceUsability::Unsupported
    } else if any_failed {
        SourceUsability::Failed
    } else {
        SourceUsability::Unavailable
    }
}

/// Build an artifact node from one fact.
fn artifact_node(key: &str, fact: &ArtifactFact) -> ArtifactNode {
    ArtifactNode {
        key: key.to_string(),
        path: fact.path.clone(),
        identity: fact.identity,
        content_sha256: fact.content_sha256.clone(),
        observed_kind: fact.kind,
        size: fact.size,
        category: fact.classification.map(|c| c.category),
        subcategory: fact.classification.and_then(|c| c.subcategory),
        classification_confidence: fact.classification.map(|c| c.confidence),
        access: fact.access,
        provenance: ProvenanceState::from_access(fact.access),
        application_status: ArtifactApplicationStatus::Unassociated,
        credible_claimants: 0,
    }
}

/// Canonical rank of one history payload: identical (run, path) facts with
/// contradictory payloads are preserved as DISTINCT rows (conflict
/// preservation — never an arrival-order winner), and identical payloads
/// collapse to one row. The rank is a total order over the payload so the
/// preserved set is deterministic.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HistoryPayloadRank {
    /// `None` sorts before `Some`: proven identities outrank absent ones in
    /// canonical order (order only — never a truth verdict).
    identity: Option<ObjectIdentity>,
    category: Option<HistoryCategoryRank>,
}

/// Canonical rank of a history category string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HistoryCategoryRank(String);

impl HistoryPayloadRank {
    fn of(identity: Option<ObjectIdentity>, category: Option<&str>) -> Self {
        HistoryPayloadRank {
            identity,
            category: category.map(|c| HistoryCategoryRank(c.to_string())),
        }
    }
}

/// Precomputed artifact structure, built ONCE per model build so
/// per-application state resolution never rescans the artifact list.
///
/// * `by_key`: artifact position by exact node key — O(1) exact lookup.
/// * `executable_children_by_parent`: parent key → number of direct observed
///   executable candidates, so a shared install root is summarized once.
/// * `access_by_root`: ancestor key → readable/inaccessible descendant flags;
///   application resolution never rescans all descendants for every app.
///
/// Memory is O(A · depth) worst case (each artifact registers a compact
/// summary under every ancestor key); depth is bounded by each path, and
/// lookups are O(log A). Build cost is O(A · depth) ancestor walks.
struct ArtifactStructure {
    by_key: BTreeMap<String, usize>,
    executable_children_by_parent: BTreeMap<String, usize>,
    access_by_root: BTreeMap<String, RootAccessSummary>,
}

#[derive(Debug, Default, Clone, Copy)]
struct RootAccessSummary {
    readable_descendant: bool,
    inaccessible_descendant: bool,
}

impl ArtifactStructure {
    fn build(artifacts: &[ArtifactNode]) -> Self {
        let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
        let mut executable_children_by_parent: BTreeMap<String, usize> = BTreeMap::new();
        for (i, node) in artifacts.iter().enumerate() {
            by_key.insert(node.key.clone(), i);
            if let Some(parent) = node.path.parent() {
                let parent_key = ArtifactKey::of(parent).to_string();
                if matches!(node.observed_kind, ProbedKind::File)
                    && is_executable_candidate(&node.path)
                {
                    *executable_children_by_parent.entry(parent_key).or_default() += 1;
                }
            }
        }
        // Compact descendant summaries: every artifact updates each proper
        // ancestor once; application state performs one map lookup per root.
        let mut access_by_root: BTreeMap<String, RootAccessSummary> = BTreeMap::new();
        for node in artifacts {
            for ancestor in ancestor_keys(&node.path) {
                let summary = access_by_root.entry(ancestor).or_default();
                summary.readable_descendant |= node.access.is_read();
                summary.inaccessible_descendant |=
                    node.access == AccessState::ExistsButInaccessible;
            }
        }
        ArtifactStructure {
            by_key,
            executable_children_by_parent,
            access_by_root,
        }
    }

    fn lookup<'a>(&self, artifacts: &'a [ArtifactNode], path: &Path) -> Option<&'a ArtifactNode> {
        let key = ArtifactKey::of(path).to_string();
        self.by_key.get(&key).map(|i| &artifacts[*i])
    }
}

/// Which node should represent a path when the same path arrives twice:
/// the one with more proven facts (identity, content, classification).
fn prefer_artifact(new: &ArtifactNode, existing: &ArtifactNode) -> bool {
    artifact_fact_rank(new) > artifact_fact_rank(existing)
}

fn artifact_fact_rank(a: &ArtifactNode) -> (bool, bool, bool, Option<u64>) {
    (
        a.identity.is_some(),
        a.content_sha256.is_some(),
        a.category.is_some(),
        a.size,
    )
}

/// The shared model recognizes executable candidates only by ASCII-defined
/// suffixes and byte-level comparisons; arbitrary paths are never decoded.
fn is_executable_candidate(path: &Path) -> bool {
    [
        b"exe".as_slice(),
        b"com",
        b"bat",
        b"cmd",
        b"bin",
        b"sh",
        b"app",
    ]
    .iter()
    .any(|extension| coresight_apps::extension_is_ascii(path, extension))
}

/// Choose the role edge from the CLASSIFIER's verdict. Classification is
/// descriptive and is never rewritten by ownership.
fn claim_edge_kind(node: &ArtifactNode) -> SystemEdgeKind {
    match node.category {
        Some(Category::Cache) => SystemEdgeKind::ApplicationCache,
        Some(Category::Logs) => SystemEdgeKind::ApplicationLog,
        // Classification as an application artifact is not proof that the
        // path itself is an executable. The executable role requires an
        // explicit recorded/candidate executable path below.
        Some(Category::Applications) => SystemEdgeKind::AssociatedWith,
        Some(Category::ApplicationData) | Some(Category::SystemData) => {
            SystemEdgeKind::ApplicationData
        }
        _ => SystemEdgeKind::AssociatedWith,
    }
}

/// Canonical precedence between two application nodes admitted under one
/// id: a TOTAL order over every semantically relevant field, so
/// `choose(a, b) == choose(b, a)` always holds and duplicate records
/// resolve identically under any arrival order.
///
/// Primary: resolution state (better-resolved wins). Then: the full reason
/// set, field values (name, publisher, bundle identifier, install
/// location, executable path bytes), and provenance. (The admission key —
/// the application id — is equal for both candidates by construction.)
fn prefer_application(new: &ApplicationNode, existing: &ApplicationNode) -> bool {
    application_rank(new) > application_rank(existing)
}

/// Total-order rank tuple for one application node. Compared
/// lexicographically; every field of the node participates (except the id,
/// which is the admission key and therefore equal for both candidates).
fn application_rank(node: &ApplicationNode) -> impl Ord + use<> {
    fn state_rank(state: ApplicationState) -> u8 {
        match state {
            ApplicationState::Resolved => 3,
            ApplicationState::PartiallyResolved => 2,
            ApplicationState::Unresolved => 1,
            ApplicationState::Unknown => 0,
        }
    }
    fn path_bytes(p: &Option<PathBuf>) -> Option<Vec<u8>> {
        p.as_ref()
            .map(|p| p.as_os_str().as_encoded_bytes().to_vec())
    }
    (
        state_rank(node.state),
        node.state_reasons.clone(),
        node.name.clone(),
        node.publisher.clone(),
        node.bundle_identifier.clone(),
        path_bytes(&node.install_location),
        path_bytes(&node.executable_path),
        node.provenance.clone(),
    )
}

fn same_artifact_path(left: &Path, right: &Path) -> bool {
    ArtifactKey::of(left) == ArtifactKey::of(right)
}

/// Determine an application's resolution state from observed artifacts.
///
/// Exact state semantics (all lookups are indexed — never full scans):
///
/// ```text
/// install root read / executable read  → resolved_any
/// root recorded, node missing/absent   → InstallRootMissing/Unobserved
/// root recorded, descendants observed  → DescendantObserved
/// resolved_any + any gap               → PartiallyResolved
/// resolved_any + no gap                → Resolved
/// expected (root/exe recorded) + descendant observed, root itself
///   unavailable                        → PartiallyResolved (a partial
///   footprint is evidence, not absence)
/// expected + nothing observed          → Unresolved
/// expected root/executable inaccessible → Unknown (not absence)
/// candidate executable observed         → PartiallyResolved, never Resolved
/// nothing recorded at all              → Unknown
/// ```
///
/// A descendant observed under an unavailable root is a partial footprint:
/// it must never collapse to `Unresolved` merely because the exact root
/// node is absent.
fn application_node(
    fact: &ApplicationFact,
    artifacts: &[ArtifactNode],
    structure: &ArtifactStructure,
) -> ApplicationNode {
    let mut reasons: BTreeSet<ApplicationStateReason> = BTreeSet::new();
    let mut resolved_any = false;
    let mut expected_any = false;
    let mut descendant_observed = false;
    let mut inaccessible = false;

    let lookup = |p: &Path| -> Option<&ArtifactNode> { structure.lookup(artifacts, p) };
    // Desktop-entry install locations are derived from Exec's parent and are
    // not trusted as install roots. Its broad directory scope cannot make
    // unrelated children look like a partial application footprint.
    let install_roots_are_authoritative =
        fact.record.source != coresight_apps::ApplicationSource::DesktopEntry;

    if install_roots_are_authoritative {
        if let Some(loc) = &fact.record.install_location {
            expected_any = true;
            match lookup(loc) {
                Some(node) if node.access.is_read() => {
                    resolved_any = true;
                    reasons.insert(ApplicationStateReason::InstallRootObserved);
                }
                Some(node) if node.access == AccessState::DoesNotExist => {
                    reasons.insert(ApplicationStateReason::InstallRootMissing);
                }
                Some(node) if node.access == AccessState::ExistsButInaccessible => {
                    reasons.insert(ApplicationStateReason::InstallRootUnobserved);
                    reasons.insert(ApplicationStateReason::ExpectedDataInaccessible);
                    inaccessible = true;
                }
                Some(_) => {
                    reasons.insert(ApplicationStateReason::InstallRootUnobserved);
                }
                None => {
                    reasons.insert(ApplicationStateReason::InstallRootUnobserved);
                }
            }
            // Descendant evidence via the precomputed compact root-access
            // summary: a readable retained artifact under the exact recorded
            // root counts even when the root node itself was not observed.
            let root_key = ArtifactKey::of(loc).to_string();
            descendant_observed |= structure
                .access_by_root
                .get(&root_key)
                .is_some_and(|summary| summary.readable_descendant);
        }
        // Detector roots can contribute partial evidence for sources whose
        // install-location field is authoritative, but never resolve by
        // themselves.
        for seed in &fact.install_roots {
            let root_key = ArtifactKey::of(seed).to_string();
            if structure
                .access_by_root
                .get(&root_key)
                .is_some_and(|summary| summary.readable_descendant)
            {
                descendant_observed = true;
                break;
            }
        }
    }
    if let Some(exe) = &fact.record.executable_path {
        // Only the exact path recorded in source metadata can resolve the
        // executable expectation.
        expected_any = true;
        match lookup(exe) {
            Some(node) if node.access.is_read() => {
                resolved_any = true;
                reasons.insert(ApplicationStateReason::ExecutableObserved);
            }
            Some(node) if node.access == AccessState::ExistsButInaccessible => {
                reasons.insert(ApplicationStateReason::ExecutableUnobserved);
                reasons.insert(ApplicationStateReason::ExpectedDataInaccessible);
                inaccessible = true;
            }
            _ => {
                reasons.insert(ApplicationStateReason::ExecutableUnobserved);
            }
        }
    } else {
        reasons.insert(ApplicationStateReason::ExecutableNotRecorded);
    }
    // A separate executable path with no exact source-recorded counterpart is
    // candidate evidence. If observed, it contributes to a partial footprint,
    // never a fully resolved state.
    if let Some(candidate) = &fact.executable {
        if !fact
            .record
            .executable_path
            .as_ref()
            .is_some_and(|recorded| same_artifact_path(recorded, candidate))
            && lookup(candidate).is_some_and(|node| node.access.is_read())
        {
            descendant_observed = true;
        }
    }
    // Duplicate executable candidates are summarized once per direct parent
    // during the artifact pass. A per-app set de-duplicates repeated roots;
    // its size is bounded by the number of artifact parent keys.
    let mut seen_executable_roots = BTreeSet::new();
    let exe_candidates: usize = if install_roots_are_authoritative {
        fact.install_roots
            .iter()
            .filter_map(|root| {
                let key = ArtifactKey::of(root).to_string();
                let count = structure.executable_children_by_parent.get(&key)?;
                seen_executable_roots.insert(key).then_some(*count)
            })
            .sum()
    } else {
        0
    };
    if exe_candidates > 1 {
        reasons.insert(ApplicationStateReason::DuplicateExecutableCandidates);
    }
    // Inaccessible expected data is summarized per root during the artifact
    // pass; this is O(number of declared roots), not roots × artifacts.
    let inaccessible = inaccessible
        || (install_roots_are_authoritative
            && fact
                .record
                .install_location
                .iter()
                .chain(fact.install_roots.iter())
                .any(|root| {
                    structure
                        .access_by_root
                        .get(&ArtifactKey::of(root).to_string())
                        .is_some_and(|summary| summary.inaccessible_descendant)
                }));
    if inaccessible {
        reasons.insert(ApplicationStateReason::ExpectedDataInaccessible);
    }
    if descendant_observed {
        reasons.insert(ApplicationStateReason::DescendantObserved);
    }
    let state = if resolved_any {
        if reasons.contains(&ApplicationStateReason::InstallRootMissing)
            || reasons.contains(&ApplicationStateReason::InstallRootUnobserved)
            || reasons.contains(&ApplicationStateReason::ExecutableUnobserved)
            || inaccessible
            || exe_candidates > 1
        {
            ApplicationState::PartiallyResolved
        } else {
            ApplicationState::Resolved
        }
    } else if expected_any || descendant_observed {
        if descendant_observed {
            // Observed descendants (even when they came from a candidate path)
            // make this a partial footprint, never "nothing observed".
            ApplicationState::PartiallyResolved
        } else if inaccessible {
            // Inaccessible expected data is unknown, not proof that the
            // application footprint is absent or unresolved.
            ApplicationState::Unknown
        } else {
            ApplicationState::Unresolved
        }
    } else {
        ApplicationState::Unknown
    };

    ApplicationNode {
        id: fact.record.id.clone(),
        name: fact.record.name.clone(),
        publisher: fact.record.publisher.clone(),
        bundle_identifier: fact.record.bundle_identifier.clone(),
        install_location: fact.record.install_location.clone(),
        executable_path: fact.record.executable_path.clone(),
        // Source provenance is a finite enum set. Collecting into a set
        // deduplicates during admission, so even a caller-supplied vector with
        // many repeated source values never creates an unbounded retained
        // provenance collection. The caller unions duplicates commutatively.
        provenance: fact
            .record
            .provenance
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        state,
        state_reasons: reasons.into_iter().collect(),
    }
}

/// Offer one edge into the bounded admission set, applying the per-edge
/// evidence bound. Deduplication is by fact key with a content-based
/// preference, so arrival order never decides which fact survives. A
/// refused edge is a dropped edge: it counts into `edges_truncated` here
/// so truncation memory survives even when the edge never reaches any
/// list. (The per-node bound counts its own drops separately.)
fn offer_edge(
    edges: &mut BoundedTopK<(u8, String, String), SystemEdge>,
    edges_truncated: &mut u64,
    evidence_truncated: &mut u64,
    limits: &SystemModelLimits,
    mut edge: SystemEdge,
) {
    edge.evidence.sort();
    edge.evidence.dedup();
    if edge.evidence.len() > limits.max_evidence_per_edge {
        *evidence_truncated += (edge.evidence.len() - limits.max_evidence_per_edge) as u64;
        edge.evidence.truncate(limits.max_evidence_per_edge);
    }
    if matches!(
        edges.offer(edge.fact_key(), edge, |new, old| new.rank() > old.rank()),
        Admission::Refused
    ) {
        *edges_truncated += 1;
    }
}

/// Bound the number of edges attached to any single node. Retains the
/// canonically-first edges per node, counted exactly.
fn bound_edges_per_node(
    edges: Vec<SystemEdge>,
    limits: &SystemModelLimits,
) -> (Vec<SystemEdge>, u64) {
    if limits.max_edges_per_node == 0 {
        return (Vec::new(), edges.len() as u64);
    }
    let mut per_node: BTreeMap<String, u64> = BTreeMap::new();
    let mut kept: Vec<SystemEdge> = Vec::with_capacity(edges.len());
    let mut overflow = 0u64;
    // Edges arrive canonically ordered, so the retained set is the
    // canonically-first per node.
    let mut sorted = edges;
    sorted.sort_by_key(|a| a.order_key());
    for edge in sorted {
        let from_count = per_node.get(&edge.from).copied().unwrap_or(0) as usize;
        let to_count = per_node.get(&edge.to).copied().unwrap_or(0) as usize;
        if from_count >= limits.max_edges_per_node || to_count >= limits.max_edges_per_node {
            overflow += 1;
            continue;
        }
        *per_node.entry(edge.from.clone()).or_insert(0) += 1;
        if edge.from != edge.to {
            *per_node.entry(edge.to.clone()).or_insert(0) += 1;
        }
        kept.push(edge);
    }
    (kept, overflow)
}

/// Artifact keys of every proper ancestor directory of `path` that could be
/// an artifact node. Bounded by the path's own depth.
fn ancestor_keys(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = path.parent();
    while let Some(p) = current {
        out.push(ArtifactKey::of(p).to_string());
        current = p.parent();
    }
    out
}

/// The honest observation summary.
fn observation_summary(
    artifacts: &[ArtifactNode],
    coverage: &[coresight_apps::SourceCoverage],
    max_source_states: usize,
) -> (ObservationSummary, u64) {
    let mut inaccessible_artifacts = 0u64;
    let mut unsupported_artifacts = 0u64;
    let mut failed_artifacts = 0u64;
    for a in artifacts {
        match a.access {
            AccessState::ExistsButInaccessible => inaccessible_artifacts += 1,
            AccessState::Unsupported => unsupported_artifacts += 1,
            AccessState::Failed => failed_artifacts += 1,
            _ => {}
        }
    }
    let mut source_state_top: BoundedTopK<
        (String, SourceStatus, Option<String>),
        SourceStateSummary,
    > = BoundedTopK::new(max_source_states);
    for coverage in coverage {
        let state = SourceStateSummary {
            source: coverage.source.clone(),
            status: coverage.status,
            note: coverage.note.clone(),
        };
        source_state_top.offer(
            (state.source.clone(), state.status, state.note.clone()),
            state,
            |_, _| false,
        );
    }
    let (source_state_items, source_states_truncated) = source_state_top.into_sorted();
    let source_states = source_state_items
        .into_iter()
        .map(|(_, state)| state)
        .collect();

    // Capability state is read verbatim from the Phase 6.1 registry: the
    // model never upgrades a partial capability to "fully supported".
    let capabilities: Vec<CapabilityState> = CONTRACTS
        .iter()
        .filter(|c| {
            matches!(
                c.id,
                CapabilityId::ApplicationInventory
                    | CapabilityId::ApplicationFootprint
                    | CapabilityId::StorageAnalysis
                    | CapabilityId::HistoricalObservations
                    | CapabilityId::SoftwareManagement
            )
        })
        .map(|c| CapabilityState {
            capability: c.id,
            status: c.status,
            blockers: capability_blockers(c.id, c.status),
        })
        .collect();

    (
        ObservationSummary {
            source_states,
            capabilities,
            inaccessible_artifacts,
            unsupported_artifacts,
            failed_artifacts,
        },
        source_states_truncated,
    )
}

/// Descriptive blockers recorded from the capability's own honest status. The
/// model reports what the capability says; it never invents support — and
/// it never invents platform-specific requirements either. Shared code
/// carries no platform conditionals: blocker text is platform-neutral, and
/// platform details belong to platform-specific observations.
fn capability_blockers(id: CapabilityId, status: CapabilityStatus) -> Vec<String> {
    let mut out = Vec::new();
    if matches!(
        status,
        CapabilityStatus::Planned | CapabilityStatus::Deferred
    ) {
        out.push("capability has no provider in this build".to_string());
    }
    if matches!(status, CapabilityStatus::Partial) {
        out.push("capability is platform-limited; coverage is partial".to_string());
    }
    match id {
        CapabilityId::ApplicationFootprint => {
            out.push(
                "footprint enumeration is partial on this host; protected locations may be unreadable"
                    .to_string(),
            );
        }
        CapabilityId::SoftwareManagement => {
            out.push("no execution exists in this build (analysis only)".to_string());
        }
        _ => {}
    }
    out.sort();
    out
}
