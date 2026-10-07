//! Insight derivation and the bounded, typed query surface.
//!
//! Every insight is descriptive. None of them authorizes an action, and none
//! of them calls anything an orphan: an artifact with no application claim is
//! reported as [`InsightKind::UnassociatedArtifact`], and only when the
//! application sources were actually usable.

use std::collections::BTreeMap;

use coresight_apps::{ApplicationId, BoundedTopK, OwnershipAssessment, OwnershipEvidence};
use coresight_capabilities::ActionClass;

use crate::model::{
    ApplicationClaim, ApplicationNode, ApplicationState, ArtifactApplicationStatus, ArtifactNode,
    CandidateActionKind, InsightBlocker, InsightKind, InsightSeverity, NodeRef, NodeRefKind,
    SystemCandidate, SystemEdge, SystemEdgeKind, SystemInsight, SystemModelLimits,
};

/// Derive insights and inert candidates from the finalized graph.
///
/// Returns `(insights, candidates, insights_truncated, candidates_truncated)`.
pub(crate) fn derive(
    artifacts: &[ArtifactNode],
    applications: &[ApplicationNode],
    edges: &[SystemEdge],
    coverage: &[coresight_apps::SourceCoverage],
    limits: &SystemModelLimits,
) -> (Vec<SystemInsight>, Vec<SystemCandidate>, u64, u64) {
    let sources_incomplete = coverage.iter().any(|c| {
        !matches!(
            c.status,
            coresight_apps::SourceStatus::Complete | coresight_apps::SourceStatus::Partial
        )
    });

    let mut insight_top: BoundedTopK<String, SystemInsight> = BoundedTopK::new(limits.max_insights);
    let mut candidate_top: BoundedTopK<(String, String, CandidateActionKind), SystemCandidate> =
        BoundedTopK::new(limits.max_candidates);

    // ---- Claim map, built once (O(E)) ---------------------------------
    let mut claims: BTreeMap<String, Vec<(&str, OwnershipAssessment, &SystemEdge)>> =
        BTreeMap::new();
    for e in edges {
        if e.kind.asserts_ownership() && e.kind != SystemEdgeKind::SharedBy {
            claims
                .entry(e.to.clone())
                .or_default()
                .push((e.from.as_str(), e.assessment, e));
        }
    }

    // ---- Per-artifact insights ---------------------------------------
    for node in artifacts {
        let node_claims = claims.get(&node.key);
        let credible: Vec<&(&str, OwnershipAssessment, &SystemEdge)> = node_claims
            .map(|c| c.iter().filter(|(_, a, _)| a.is_credible()).collect())
            .unwrap_or_default();
        let strong: Vec<&(&str, OwnershipAssessment, &SystemEdge)> = node_claims
            .map(|c| {
                c.iter()
                    .filter(|(_, a, _)| {
                        matches!(a, OwnershipAssessment::Strong | OwnershipAssessment::Direct)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut blockers = vec![InsightBlocker::NoExecutorInThisPhase];
        if node.identity.is_none() {
            blockers.push(InsightBlocker::UnprovenObjectIdentity);
        }
        if sources_incomplete {
            blockers.push(InsightBlocker::ApplicationSourcesIncomplete);
        }
        if node.access == coresight_capabilities::access::AccessState::ExistsButInaccessible {
            blockers.push(InsightBlocker::InaccessibleData);
        }

        // Conflicts and sharing.
        if strong.len() >= 2 {
            blockers.push(InsightBlocker::ConflictingOwnership);
            push_insight(
                &mut insight_top,
                limits,
                node,
                InsightKind::ConflictingOwnership,
                InsightSeverity::NeedsResolution,
                strong.iter().map(|(a, _, _)| (*a).to_string()).collect(),
                strong.iter().map(|(_, _, e)| e.evidence.clone()).collect(),
                format!(
                    "{} applications hold strong-or-better evidence for one artifact",
                    strong.len()
                ),
                blockers.clone(),
            );
        } else if credible.len() >= 2 {
            blockers.push(InsightBlocker::SharedArtifactOwnership);
            push_insight(
                &mut insight_top,
                limits,
                node,
                InsightKind::SharedArtifact,
                InsightSeverity::Notable,
                credible.iter().map(|(a, _, _)| (*a).to_string()).collect(),
                credible
                    .iter()
                    .map(|(_, _, e)| e.evidence.clone())
                    .collect(),
                format!(
                    "{} applications have credible evidence for one artifact",
                    credible.len()
                ),
                blockers.clone(),
            );
        } else if node.application_status == ArtifactApplicationStatus::Unassociated {
            blockers.push(InsightBlocker::InsufficientEvidence);
            push_insight(
                &mut insight_top,
                limits,
                node,
                InsightKind::UnassociatedArtifact,
                InsightSeverity::Informational,
                Vec::new(),
                Vec::new(),
                "no application claim was observed while application sources were usable"
                    .to_string(),
                blockers.clone(),
            );
        }

        // Identity-engine facts.
        if node.content_sha256.is_some() {
            let siblings = artifacts
                .iter()
                .filter(|a| {
                    a.key != node.key
                        && a.content_sha256.is_some()
                        && a.content_sha256 == node.content_sha256
                })
                .map(|a| a.key.clone())
                .collect::<Vec<_>>();
            if !siblings.is_empty() {
                let mut related = siblings;
                related.push(node.key.clone());
                related.sort();
                related.dedup();
                push_insight(
                    &mut insight_top,
                    limits,
                    node,
                    InsightKind::DuplicateContent,
                    InsightSeverity::Informational,
                    Vec::new(),
                    Vec::new(),
                    "distinct objects hold byte-identical content (verified digest)".to_string(),
                    blockers.clone(),
                );
            }
        }
        if node.identity.is_some() {
            let aliases = artifacts
                .iter()
                .filter(|a| a.key != node.key && a.identity == node.identity)
                .count();
            if aliases > 0 {
                push_insight(
                    &mut insight_top,
                    limits,
                    node,
                    InsightKind::HardLinkAlias,
                    InsightSeverity::Informational,
                    Vec::new(),
                    Vec::new(),
                    "several paths refer to one filesystem object (proven identity)".to_string(),
                    blockers.clone(),
                );
            }
        }
    }

    // ---- Per-application insights -------------------------------------
    for app in applications {
        match app.state {
            ApplicationState::PartiallyResolved => push_app_insight(
                &mut insight_top,
                limits,
                app,
                InsightKind::PartialApplication,
                InsightSeverity::Notable,
                "the application resolved against some observed artifacts but not all",
                sources_incomplete,
            ),
            ApplicationState::Unresolved => push_app_insight(
                &mut insight_top,
                limits,
                app,
                InsightKind::UnresolvedApplication,
                InsightSeverity::Notable,
                "the application is recorded but nothing of it was observed",
                sources_incomplete,
            ),
            _ => {}
        }
    }

    // ---- Historical context -------------------------------------------
    for e in edges.iter().filter(|e| {
        matches!(
            e.kind,
            SystemEdgeKind::HistoricalAliasOf | SystemEdgeKind::HistoricalMoveOf
        )
    }) {
        let Some(node) = artifacts.iter().find(|a| a.key == e.from) else {
            continue;
        };
        let mut blockers = vec![InsightBlocker::NoExecutorInThisPhase];
        if node.identity.is_none() {
            blockers.push(InsightBlocker::UnprovenObjectIdentity);
        }
        let kind = InsightKind::HistoricalContext;
        let severity = if e.kind == SystemEdgeKind::HistoricalMoveOf {
            InsightSeverity::Notable
        } else {
            InsightSeverity::Informational
        };
        push_insight(
            &mut insight_top,
            limits,
            node,
            kind,
            severity,
            Vec::new(),
            vec![e.evidence.clone()],
            if e.kind == SystemEdgeKind::HistoricalMoveOf {
                "stored history shows this path previously referred to a different object"
                    .to_string()
            } else {
                "stored history observed this path with the same object identity".to_string()
            },
            blockers,
        );
    }

    // ---- Candidates (inert data) --------------------------------------
    for node in artifacts {
        let node_claims = claims.get(&node.key);
        let credible = node_claims
            .map(|c| c.iter().filter(|(_, a, _)| a.is_credible()).count())
            .unwrap_or(0);
        let strong = node_claims
            .map(|c| {
                c.iter()
                    .filter(|(_, a, _)| {
                        matches!(a, OwnershipAssessment::Strong | OwnershipAssessment::Direct)
                    })
                    .count()
            })
            .unwrap_or(0);

        let mut blockers = vec![InsightBlocker::NoExecutorInThisPhase];
        if node.identity.is_none() {
            blockers.push(InsightBlocker::UnprovenObjectIdentity);
        }
        if sources_incomplete {
            blockers.push(InsightBlocker::ApplicationSourcesIncomplete);
        }

        let (kind, assessment, app) = if strong >= 2 {
            blockers.push(InsightBlocker::ConflictingOwnership);
            (
                CandidateActionKind::SharedArtifact,
                OwnershipAssessment::Conflicting,
                None,
            )
        } else if credible >= 2 {
            blockers.push(InsightBlocker::SharedArtifactOwnership);
            (
                CandidateActionKind::SharedArtifact,
                OwnershipAssessment::Moderate,
                None,
            )
        } else if credible == 1 {
            let owner = node_claims.and_then(|c| {
                c.iter()
                    .find(|(_, a, _)| a.is_credible())
                    .map(|(a, _, _)| ApplicationId((*a).to_string()))
            });
            (
                CandidateActionKind::UninstallArtifact,
                node_claims
                    .and_then(|c| {
                        c.iter()
                            .find(|(_, a, _)| a.is_credible())
                            .map(|(_, a, _)| *a)
                    })
                    .unwrap_or(OwnershipAssessment::Moderate),
                owner,
            )
        } else if node.application_status == ArtifactApplicationStatus::Unassociated {
            blockers.push(InsightBlocker::InsufficientEvidence);
            (
                CandidateActionKind::Orphan,
                OwnershipAssessment::Unknown,
                None,
            )
        } else {
            blockers.push(InsightBlocker::InsufficientEvidence);
            (
                CandidateActionKind::UncertainAssociation,
                OwnershipAssessment::Unknown,
                None,
            )
        };

        blockers.sort();
        blockers.dedup();
        let evidence = node_claims
            .map(|c| {
                let mut ev: Vec<OwnershipEvidence> =
                    c.iter().flat_map(|(_, _, e)| e.evidence.clone()).collect();
                ev.sort();
                ev.dedup();
                ev.truncate(limits.max_evidence_per_edge);
                ev
            })
            .unwrap_or_default();

        let candidate = SystemCandidate {
            action_kind: kind,
            target: node.key.clone(),
            path: node.path.clone(),
            confidence: assessment
                .strength()
                .map(|s| s.to_confidence())
                .unwrap_or(coresight_apps::Confidence::Unknown),
            assessment,
            // The candidate records what an action WOULD be classified as;
            // no executor can act on it.
            effect: ActionClass::Destructive,
            blockers,
            evidence,
        };
        candidate_top.offer(
            (node.key.clone(), app.map(|a| a.0).unwrap_or_default(), kind),
            candidate,
            |new, old| new.evidence.len() > old.evidence.len(),
        );
    }

    let (insight_items, insight_overflow) = insight_top.into_sorted();
    let mut insights: Vec<SystemInsight> = insight_items.into_iter().map(|(_, i)| i).collect();
    insights.sort_by(|a, b| a.id.cmp(&b.id));
    insights.dedup_by(|a, b| a.id == b.id);

    let (cand_items, cand_overflow) = candidate_top.into_sorted();
    let mut candidates: Vec<SystemCandidate> = cand_items.into_iter().map(|(_, c)| c).collect();
    candidates.sort_by(|a, b| {
        a.target
            .cmp(&b.target)
            .then(a.action_kind.cmp(&b.action_kind))
    });

    (insights, candidates, insight_overflow, cand_overflow)
}

#[allow(clippy::too_many_arguments)]
fn push_insight(
    top: &mut BoundedTopK<String, SystemInsight>,
    limits: &SystemModelLimits,
    node: &ArtifactNode,
    kind: InsightKind,
    severity: InsightSeverity,
    related_applications: Vec<String>,
    evidence_groups: Vec<Vec<OwnershipEvidence>>,
    explanation: String,
    blockers: Vec<InsightBlocker>,
) {
    let mut evidence: Vec<OwnershipEvidence> = evidence_groups.into_iter().flatten().collect();
    evidence.sort();
    evidence.dedup();
    evidence.truncate(limits.max_evidence_per_edge);
    let id = format!("ins-{:?}-{}", kind, node.key)
        .to_lowercase()
        .replace(' ', "-");
    let mut blockers = blockers;
    blockers.sort();
    blockers.dedup();
    let insight = SystemInsight {
        id: id.clone(),
        kind,
        severity,
        related_nodes: vec![node.key.clone()],
        related_applications: related_applications
            .into_iter()
            .map(ApplicationId)
            .collect(),
        evidence,
        explanation,
        blockers,
    };
    top.offer(id, insight, |new, old| {
        (new.severity, new.evidence.len()) > (old.severity, old.evidence.len())
    });
}

fn push_app_insight(
    top: &mut BoundedTopK<String, SystemInsight>,
    limits: &SystemModelLimits,
    app: &ApplicationNode,
    kind: InsightKind,
    severity: InsightSeverity,
    explanation: &str,
    sources_incomplete: bool,
) {
    let id = format!("ins-{:?}-{}", kind, app.id.0)
        .to_lowercase()
        .replace(' ', "-");
    let mut blockers = vec![InsightBlocker::NoExecutorInThisPhase];
    if sources_incomplete {
        blockers.push(InsightBlocker::ApplicationSourcesIncomplete);
    }
    if app
        .state_reasons
        .contains(&crate::model::ApplicationStateReason::ExpectedDataInaccessible)
    {
        blockers.push(InsightBlocker::InaccessibleData);
    }
    blockers.sort();
    blockers.dedup();
    let insight = SystemInsight {
        id: id.clone(),
        kind,
        severity,
        related_nodes: vec![app.id.0.clone()],
        related_applications: vec![app.id.clone()],
        evidence: Vec::new(),
        explanation: explanation.to_string(),
        blockers,
    };
    let _ = limits;
    top.offer(id, insight, |new, old| new.severity > old.severity);
}

// ---------------------------------------------------------------------------
// Bounded, typed queries
// ---------------------------------------------------------------------------

/// The result of a bounded query: typed items plus the exact number of
/// matches a limit stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult<T> {
    pub items: Vec<T>,
    /// Exactly how many additional matches existed beyond `limit`.
    pub truncated: u64,
}

impl<T> QueryResult<T> {
    fn new(items: Vec<T>, truncated: u64) -> Self {
        QueryResult { items, truncated }
    }
}

/// Default page size for queries that do not specify one.
pub const DEFAULT_QUERY_LIMIT: usize = 256;

/// Applications with a credible claim on one artifact, canonically ordered.
///
/// One row per (application, edge kind): the descriptive role edges
/// (install root, executable, data, cache, logs, configuration) and the
/// ownership edges are all reported, because the caller asked *how* the
/// application relates — whereas claimant COUNTING uses only the ownership
/// edges, so one application can never be counted twice.
///
/// Complexity: `O(deg(node) log deg)`.
pub fn applications_for_artifact(
    model: &crate::model::SystemModel,
    artifact_key: &str,
    limit: usize,
) -> QueryResult<ApplicationClaim> {
    let rows: Vec<(ApplicationId, String, SystemEdgeKind, SystemEdge)> = model
        .edges_for_node(artifact_key)
        .into_iter()
        .filter(|e| {
            e.to == artifact_key && (e.kind.asserts_ownership() || e.kind.is_descriptive_role())
        })
        .filter_map(|e| {
            let app = model.application(&ApplicationId(e.from.clone()))?;
            Some((app.id.clone(), app.name.clone(), e.kind, e.clone()))
        })
        .collect();
    // Group by application so exactly one row exists per app: role edges
    // describe HOW the app relates, and the strongest owned/associated
    // appraisal is the row's assessment.
    let mut grouped: BTreeMap<
        String,
        (ApplicationId, String, Vec<SystemEdgeKind>, Vec<SystemEdge>),
    > = BTreeMap::new();
    for (id, name, kind, edge) in rows {
        grouped
            .entry(id.0.clone())
            .or_insert_with(|| (id.clone(), name.clone(), Vec::new(), Vec::new()))
            .2
            .push(kind);
        grouped.get_mut(&id.0).expect("just inserted").3.push(edge);
    }
    let mut out: Vec<ApplicationClaim> = grouped
        .into_values()
        .map(|(application, application_name, mut edge_kinds, edges)| {
            edge_kinds.sort();
            edge_kinds.dedup();
            let assessment = edges
                .iter()
                .filter(|e| {
                    e.kind == SystemEdgeKind::OwnedBy || e.kind == SystemEdgeKind::AssociatedWith
                })
                .map(|e| e.assessment)
                .max()
                .unwrap_or(OwnershipAssessment::Unknown);
            let mut evidence: Vec<OwnershipEvidence> =
                edges.into_iter().flat_map(|e| e.evidence).collect();
            evidence.sort();
            evidence.dedup();
            ApplicationClaim {
                application,
                application_name,
                edge_kinds,
                assessment,
                evidence,
            }
        })
        .collect();
    out.sort_by(|a, b| a.application.0.cmp(&b.application.0));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Distinct applications with a credible ownership claim on one artifact.
///
/// Unlike [`applications_for_artifact`], this counts each application ONCE
/// regardless of how many kinds of edge link it, so it is the query to use
/// for "who owns this".
pub fn owning_applications(
    model: &crate::model::SystemModel,
    artifact_key: &str,
    limit: usize,
) -> QueryResult<ApplicationId> {
    let mut out: Vec<ApplicationId> = model
        .edges_for_node(artifact_key)
        .into_iter()
        .filter(|e| e.to == artifact_key && e.kind.asserts_ownership())
        .filter(|e| e.assessment.is_credible())
        .map(|e| ApplicationId(e.from.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup();
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifact node keys an application claims, canonically ordered.
pub fn artifacts_for_application(
    model: &crate::model::SystemModel,
    app: &ApplicationId,
    limit: usize,
) -> QueryResult<String> {
    let mut out: Vec<String> = model
        .edges_for_node(&app.0)
        .into_iter()
        .filter(|e| e.from == app.0 && e.kind.asserts_ownership())
        .map(|e| e.to.clone())
        .collect();
    out.sort();
    out.dedup();
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifacts carrying one classification category.
pub fn artifacts_of_classification(
    model: &crate::model::SystemModel,
    category: coresight_classifier::Category,
    limit: usize,
) -> QueryResult<NodeRef> {
    let mut keys: Vec<String> = model
        .artifacts_of_category(category)
        .into_iter()
        .map(str::to_string)
        .collect();
    keys.sort();
    let overflow = keys.len().saturating_sub(limit) as u64;
    keys.truncate(limit);
    QueryResult::new(
        keys.into_iter()
            .map(|k| NodeRef {
                kind: NodeRefKind::Artifact,
                key: k,
            })
            .collect(),
        overflow,
    )
}

/// Artifacts several applications relate to.
pub fn shared_artifacts(
    model: &crate::model::SystemModel,
    limit: usize,
) -> QueryResult<&ArtifactNode> {
    let mut out: Vec<&ArtifactNode> = model
        .artifact_nodes()
        .iter()
        .filter(|a| {
            matches!(
                a.application_status,
                ArtifactApplicationStatus::Shared | ArtifactApplicationStatus::Conflicting
            )
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifacts with conflicting strong claims.
pub fn conflicting_ownership(
    model: &crate::model::SystemModel,
    limit: usize,
) -> QueryResult<&ArtifactNode> {
    let mut out: Vec<&ArtifactNode> = model
        .artifact_nodes()
        .iter()
        .filter(|a| a.application_status == ArtifactApplicationStatus::Conflicting)
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifacts whose association is unresolved — either no claim was observed
/// while sources were usable, or the association could not be established at
/// all because the sources were unsupported/unavailable/failed.
pub fn unresolved_associations(
    model: &crate::model::SystemModel,
    limit: usize,
) -> QueryResult<&ArtifactNode> {
    let mut out: Vec<&ArtifactNode> = model
        .artifact_nodes()
        .iter()
        .filter(|a| {
            a.application_status.is_genuinely_unassociated()
                || a.application_status.is_association_unknown()
                || a.application_status == ArtifactApplicationStatus::Uncertain
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifacts with a credible (`>= Moderate`) association.
pub fn strongly_associated_artifacts(
    model: &crate::model::SystemModel,
    limit: usize,
) -> QueryResult<&ArtifactNode> {
    let mut out: Vec<&ArtifactNode> = model
        .artifact_nodes()
        .iter()
        .filter(|a| a.credible_claimants > 0)
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifacts with no application claim observed while the application
/// sources were actually usable. This is the ONLY "no application" query, and
/// it deliberately excludes artifacts whose association was never observable.
pub fn artifacts_without_application(
    model: &crate::model::SystemModel,
    limit: usize,
) -> QueryResult<&ArtifactNode> {
    let mut out: Vec<&ArtifactNode> = model
        .artifact_nodes()
        .iter()
        .filter(|a| a.application_status.is_genuinely_unassociated())
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Artifacts sharing one canonical object identity (hard-link aliases).
pub fn aliases_of_object(
    model: &crate::model::SystemModel,
    object: coresight_identity::ObjectIdentity,
) -> Vec<&str> {
    model.artifacts_sharing_object(object)
}

/// Artifacts whose association could not be established because the
/// application sources were not usable. Distinct from "no claim observed".
pub fn association_unknown_artifacts(
    model: &crate::model::SystemModel,
    limit: usize,
) -> QueryResult<&ArtifactNode> {
    let mut out: Vec<&ArtifactNode> = model
        .artifact_nodes()
        .iter()
        .filter(|a| a.application_status.is_association_unknown())
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}

/// Insights of one kind, canonically ordered.
pub fn insights_of_kind(
    model: &crate::model::SystemModel,
    kind: InsightKind,
    limit: usize,
) -> QueryResult<&SystemInsight> {
    let mut out: Vec<&SystemInsight> = model.insights.iter().filter(|i| i.kind == kind).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    let overflow = out.len().saturating_sub(limit) as u64;
    out.truncate(limit);
    QueryResult::new(out, overflow)
}
