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
    ArtifactNode, CapabilityState, HistoricalContext, ModelTruncation, ObservationSummary,
    ProvenanceState, SourceStateSummary, SystemEdge, SystemEdgeKind, SystemModel,
    SystemModelLimits,
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
    /// Install roots derived by the Phase 6.2 root detector.
    pub install_roots: Vec<PathBuf>,
    /// The executable path the (app, path) evidence already established.
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
struct ClaimFacts {
    evidence: EvidenceAccumulator,
    /// True only when every offered evidence item was structural
    /// containment. Such a claim may produce `AssociatedWith` but never
    /// `OwnedBy` and never counts as credible ownership.
    structural_only: bool,
    /// Classifier/application roles and how they were derived.
    roles: BTreeMap<SystemEdgeKind, ProvenanceState>,
}

impl ClaimFacts {
    fn new(limit: usize) -> Self {
        ClaimFacts {
            evidence: EvidenceAccumulator::new(limit),
            structural_only: true,
            roles: BTreeMap::new(),
        }
    }

    fn offer(
        &mut self,
        evidence: OwnershipEvidence,
        structural: bool,
        role: Option<(SystemEdgeKind, ProvenanceState)>,
    ) {
        self.evidence.offer(evidence);
        self.structural_only &= structural;
        if let Some((kind, provenance)) = role {
            self.roles
                .entry(kind)
                .and_modify(|old| *old = (*old).min(provenance))
                .or_insert(provenance);
        }
    }

    fn assessment(&self) -> OwnershipAssessment {
        if self.structural_only {
            // Containment may be a strong fact about structure, but it is
            // intentionally below the ownership-credibility line.
            OwnershipAssessment::Weak
        } else {
            self.evidence.assessment()
        }
    }
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

    // ---- 2. Application admission (bounded, canonical id order) -------
    let mut app_admitted: BoundedTopK<String, ApplicationNode> =
        BoundedTopK::new(limits.max_applications);
    for fact in &input.applications {
        let node = application_node(fact, &artifacts, &artifact_index);
        app_admitted.offer(fact.record.id.0.clone(), node, |n, e| {
            n.state_reasons.len() > e.state_reasons.len()
        });
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
    // artifact). The map is bounded by the artifact and edge limits: a pair
    // is admitted only when the corresponding artifact node exists, and
    // the global edge admission below caps published pairs. Feeding every
    // evidence item for one pair through the accumulator preserves Phase
    // 6.2's ceilings across module boundaries: the same underlying signal,
    // forwarded as several items, still counts once.
    let mut claim_evidence: BTreeMap<(String, String), ClaimFacts> = BTreeMap::new();
    let claim_for = |map: &mut BTreeMap<(String, String), ClaimFacts>,
                     app: &str,
                     artifact_key: &str,
                     evidence: OwnershipEvidence,
                     structural: bool,
                     role: Option<(SystemEdgeKind, ProvenanceState)>| {
        map.entry((app.to_string(), artifact_key.to_string()))
            .or_insert_with(|| ClaimFacts::new(limits.max_evidence_per_edge))
            .offer(evidence, structural, role);
    };
    for fact in &input.applications {
        let app_id = &fact.record.id;
        // Install-root edges.
        for root in &fact.install_roots {
            let key = ArtifactKey::of(root).to_string();
            if !artifact_index.contains_key(&key) {
                continue;
            }
            let edge_evidence = OwnershipEvidence::new(
                EvidenceKind::InstallLocation,
                EvidenceSource::for_application_source(&fact.record.source),
                EvidenceStrength::Direct,
                CorrelationGroup::SourceRecord(fact.record.source.clone()),
                coresight_apps::AssociationScope::ThisMachine,
                root.clone(),
                MatchedAttribute::InstallLocation,
                Some(fact.record.name.clone()),
            )
            .with_matched_path(root.clone());
            offer_edge(
                &mut edges,
                &mut evidence_truncated,
                limits,
                SystemEdge {
                    kind: SystemEdgeKind::ApplicationInstallRoot,
                    domain: SystemEdgeKind::ApplicationInstallRoot.domain(),
                    from: app_id.0.clone(),
                    to: key.clone(),
                    assessment: OwnershipAssessment::Direct,
                    provenance: ProvenanceState::Observed,
                    evidence: vec![edge_evidence.clone()],
                },
            );
            claim_for(
                &mut claim_evidence,
                &app_id.0,
                &key,
                edge_evidence,
                false,
                None,
            );
        }
        // Recorded executable.
        if let Some(exe) = &fact.executable {
            let key = ArtifactKey::of(exe).to_string();
            if artifact_index.contains_key(&key) {
                let edge_evidence = OwnershipEvidence::new(
                    EvidenceKind::ExactExecutablePath,
                    EvidenceSource::ExecutableMetadata,
                    EvidenceStrength::Strong,
                    CorrelationGroup::SourceRecord(fact.record.source.clone()),
                    coresight_apps::AssociationScope::ThisMachine,
                    exe.clone(),
                    MatchedAttribute::ExecutablePath,
                    Some(fact.record.name.clone()),
                )
                .with_matched_path(exe.clone());
                offer_edge(
                    &mut edges,
                    &mut evidence_truncated,
                    limits,
                    SystemEdge {
                        kind: SystemEdgeKind::ApplicationExecutable,
                        domain: SystemEdgeKind::ApplicationExecutable.domain(),
                        from: app_id.0.clone(),
                        to: key.clone(),
                        assessment: OwnershipAssessment::Strong,
                        provenance: ProvenanceState::Observed,
                        evidence: vec![edge_evidence.clone()],
                    },
                );
                claim_for(
                    &mut claim_evidence,
                    &app_id.0,
                    &key,
                    edge_evidence,
                    false,
                    Some((
                        SystemEdgeKind::ApplicationExecutable,
                        ProvenanceState::Observed,
                    )),
                );
            }
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
    // `applications × artifacts` scan.
    {
        // install root key → (application key, root path)
        let mut roots_by_key: BTreeMap<String, Vec<(String, PathBuf)>> = BTreeMap::new();
        for fact in &input.applications {
            let app_key = fact.record.id.0.clone();
            let mut roots: Vec<PathBuf> = fact.install_roots.clone();
            if let Some(loc) = &fact.record.install_location {
                roots.push(loc.clone());
            }
            for root in roots {
                roots_by_key
                    .entry(ArtifactKey::of(&root).to_string())
                    .or_default()
                    .push((app_key.clone(), root));
            }
        }
        if !roots_by_key.is_empty() {
            for node in &artifacts {
                for ancestor_key in ancestor_keys(&node.path) {
                    let Some(owners) = roots_by_key.get(&ancestor_key) else {
                        continue;
                    };
                    for (app_key, root) in owners {
                        // Do not restate a claim the caller already supplied
                        // explicitly for this (app, artifact) pair.
                        if claim_evidence.contains_key(&(app_key.clone(), node.key.clone())) {
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
                        claim_for(&mut claim_evidence, app_key, &key, evidence, true, None);
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
    let mut published_claims: BTreeMap<String, Vec<(String, OwnershipAssessment)>> =
        BTreeMap::new();
    for ((app_key, artifact_key), claim) in &claim_evidence {
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
        for (role, role_provenance) in &claim.roles {
            if !credible && *role == SystemEdgeKind::OwnedBy {
                continue;
            }
            offer_edge(
                &mut edges,
                &mut evidence_truncated,
                limits,
                SystemEdge {
                    kind: *role,
                    domain: role.domain(),
                    from: app_key.clone(),
                    to: artifact_key.clone(),
                    assessment,
                    provenance: *role_provenance,
                    evidence: evidence.clone(),
                },
            );
        }
        published_claims
            .entry(app_key.clone())
            .or_default()
            .push((artifact_key.clone(), assessment));
    }

    // 3c. Identity-engine relationships.
    for rel in &input.relationships {
        let keys: Vec<String> = rel
            .paths
            .iter()
            .map(|p| ArtifactKey::of(p).to_string())
            .filter(|k| artifact_index.contains_key(k))
            .collect();
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
    let mut history_admitted: BoundedTopK<(String, String), HistoricalContext> =
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
        history_admitted.offer((fact.run_id.clone(), key), ctx, |_, _| false);
    }
    let (history_items, history_overflow) = history_admitted.into_sorted();
    truncation.historical_context_truncated = history_overflow;
    let mut historical_context: Vec<HistoricalContext> =
        history_items.into_iter().map(|(_, c)| c).collect();
    historical_context.sort_by(|a, b| {
        a.run_id.cmp(&b.run_id).then(
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes()),
        )
    });
    // Historical edges join to artifacts that the CURRENT model also holds.
    // A caller-supplied context record for an artifact the model does not
    // hold is still published as data, but it cannot anchor an edge: an
    // edge needs both endpoints. This keeps "no invented facts" literal.
    let mut hist_edges: Vec<SystemEdge> = Vec::new();
    for ctx in &historical_context {
        let key = ArtifactKey::of(&ctx.path).to_string();
        let Some(node) = artifact_by_key.get(&key) else {
            continue;
        };
        // A stored identity that differs from the current one is a proven
        // move; an equal one is a proven alias observation. Both come from
        // real history, never from current state.
        let kind = match (ctx.identity, node.identity) {
            (Some(hist), Some(now)) if hist != now => SystemEdgeKind::HistoricalMoveOf,
            (Some(_), Some(_)) => SystemEdgeKind::HistoricalAliasOf,
            _ => SystemEdgeKind::HistoricalAliasOf,
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
        // The edge points at the artifact node itself: source and target are
        // the same key, so the edge is indexed exactly once per node even
        // though it touches two conceptual roles.
        hist_edges.push(SystemEdge {
            kind,
            domain: kind.domain(),
            from: key.clone(),
            to: key,
            assessment: OwnershipAssessment::Strong,
            provenance: ProvenanceState::Observed,
            evidence,
        });
    }
    for edge in hist_edges {
        offer_edge(&mut edges, &mut evidence_truncated, limits, edge);
    }

    // ---- 5. Publish edges canonically ---------------------------------
    let (edge_items, edges_overflow) = edges.into_sorted();
    truncation.edges_truncated = edges_overflow;
    truncation.evidence_truncated = evidence_truncated;
    let mut edge_list: Vec<SystemEdge> = edge_items.into_iter().map(|(_, e)| e).collect();
    edge_list.sort_by_key(|a| a.order_key());

    // ---- 6. Per-node edge bound (also bounded by admission) -----------
    let (edge_list, per_node_overflow) = bound_edges_per_node(edge_list, limits);
    truncation.edges_per_node_truncated = per_node_overflow;

    // ---- 7. Derive artifact application status from the FINAL edges ----
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
    // that several applications relate to.
    let mut shared_edges: Vec<SystemEdge> = Vec::new();
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
                shared_edges.push(SystemEdge {
                    kind: SystemEdgeKind::SharedBy,
                    domain: SystemEdgeKind::SharedBy.domain(),
                    from,
                    to,
                    assessment: OwnershipAssessment::Moderate,
                    provenance: ProvenanceState::Inferred,
                    evidence: Vec::new(),
                });
            }
        }
    }
    let (mut edge_list, extra_overflow) = {
        let mut all = edge_list;
        all.extend(shared_edges);
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
    let (insights, candidates, insight_overflow, candidate_overflow) = crate::insight::derive(
        &artifacts,
        &applications,
        &edge_list,
        &input.source_coverage,
        limits,
    );
    truncation.insights_truncated = insight_overflow;
    truncation.candidates_truncated = candidate_overflow;

    // ---- 9. Observation summary ---------------------------------------
    let observations = observation_summary(&artifacts, &input.source_coverage);

    SystemModel::finalize(crate::model::FinalizeInput {
        artifacts,
        applications,
        edges: edge_list,
        historical_context,
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

/// Choose the role edge from the CLASSIFIER's verdict. Classification is
/// descriptive and is never rewritten by ownership.
fn claim_edge_kind(node: &ArtifactNode) -> SystemEdgeKind {
    match node.category {
        Some(Category::Cache) => SystemEdgeKind::ApplicationCache,
        Some(Category::Logs) => SystemEdgeKind::ApplicationLog,
        Some(Category::Applications) => SystemEdgeKind::ApplicationExecutable,
        Some(Category::ApplicationData) | Some(Category::SystemData) => {
            SystemEdgeKind::ApplicationData
        }
        _ => SystemEdgeKind::AssociatedWith,
    }
}

/// Determine an application's resolution state from observed artifacts.
fn application_node(
    fact: &ApplicationFact,
    artifacts: &[ArtifactNode],
    artifact_index: &BTreeMap<String, usize>,
) -> ApplicationNode {
    let mut reasons: BTreeSet<ApplicationStateReason> = BTreeSet::new();
    let mut resolved_any = false;
    let mut expected_any = false;

    let lookup = |p: &Path| -> Option<&ArtifactNode> {
        let key = ArtifactKey::of(p).to_string();
        artifact_index.get(&key).map(|i| &artifacts[*i])
    };

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
            Some(_) => {
                reasons.insert(ApplicationStateReason::InstallRootUnobserved);
            }
            None => {
                reasons.insert(ApplicationStateReason::InstallRootUnobserved);
            }
        }
    }
    if let Some(exe) = &fact.executable {
        expected_any = true;
        match lookup(exe) {
            Some(node) if node.access.is_read() => {
                resolved_any = true;
                reasons.insert(ApplicationStateReason::ExecutableObserved);
            }
            _ => {
                reasons.insert(ApplicationStateReason::ExecutableUnobserved);
            }
        }
    } else {
        reasons.insert(ApplicationStateReason::ExecutableNotRecorded);
    }
    // Duplicate executable candidates among the application's install roots.
    let exe_candidates = fact
        .install_roots
        .iter()
        .filter_map(|root| lookup(root))
        .flat_map(|root| {
            artifacts
                .iter()
                .filter(move |a| a.path.parent() == Some(root.path.as_path()))
                .filter(|a| matches!(a.observed_kind, ProbedKind::File))
        })
        .filter(|a| {
            a.path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    matches!(
                        e.to_ascii_lowercase().as_str(),
                        "exe" | "com" | "bat" | "cmd" | "bin" | "sh" | "app"
                    )
                })
                .unwrap_or(false)
        })
        .count();
    if exe_candidates > 1 {
        reasons.insert(ApplicationStateReason::DuplicateExecutableCandidates);
    }
    // Inaccessible expected data under the application's roots.
    let inaccessible = fact
        .install_roots
        .iter()
        .filter_map(|root| lookup(root))
        .any(|root| {
            artifacts.iter().any(|a| {
                a.path.starts_with(&root.path) && a.access == AccessState::ExistsButInaccessible
            })
        });
    if inaccessible {
        reasons.insert(ApplicationStateReason::ExpectedDataInaccessible);
    }
    let state = if resolved_any {
        if reasons.contains(&ApplicationStateReason::InstallRootMissing)
            || reasons.contains(&ApplicationStateReason::ExecutableUnobserved)
            || inaccessible
            || exe_candidates > 1
        {
            ApplicationState::PartiallyResolved
        } else {
            ApplicationState::Resolved
        }
    } else if expected_any {
        ApplicationState::Unresolved
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
        provenance: fact.record.provenance.clone(),
        state,
        state_reasons: reasons.into_iter().collect(),
    }
}

/// Offer one edge into the bounded admission set, applying the per-edge
/// evidence bound. Deduplication is by fact key with a content-based
/// preference, so arrival order never decides which fact survives.
fn offer_edge(
    edges: &mut BoundedTopK<(u8, String, String), SystemEdge>,
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
    edges.offer(edge.fact_key(), edge, |new, old| new.rank() > old.rank());
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
) -> ObservationSummary {
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
    let mut source_states: Vec<SourceStateSummary> = coverage
        .iter()
        .map(|c| SourceStateSummary {
            source: c.source.clone(),
            status: c.status,
            note: c.note.clone(),
        })
        .collect();
    source_states.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.status.cmp(&b.status))
            .then(a.note.cmp(&b.note))
    });
    source_states.dedup();

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

    ObservationSummary {
        source_states,
        capabilities,
        inaccessible_artifacts,
        unsupported_artifacts,
        failed_artifacts,
    }
}

/// Descriptive blockers recorded from the capability's own honest status. The
/// model reports what the capability says; it never invents support.
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
            out.push("macOS protected locations require Full Disk Access".to_string());
        }
        CapabilityId::SoftwareManagement => {
            out.push("no execution exists in this build (analysis only)".to_string());
        }
        _ => {}
    }
    out.sort();
    out
}
