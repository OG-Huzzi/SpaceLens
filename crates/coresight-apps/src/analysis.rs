//! The application-intelligence analysis layer (Phase 6.2).
//!
//! ```text
//! observe(facts)  ──▶  analyze(facts)  ──▶  ApplicationAnalysis
//! ```
//!
//! Everything here is a PURE function of already-observed facts. No
//! filesystem access, no subprocess, no network: callers pass in
//! [`ObservedArtifact`] values produced by the observation layer. That is the
//! "no hidden I/O" contract of Phase 6.2 made structural — the analysis
//! cannot perform I/O because it has no way to.
//!
//! ## Semantics kept distinct
//!
//! [`RelationKind`] separates `Contains` from `Owns` from `AssociatedWith`
//! from `DerivedFrom`. An artifact that merely *lies inside* an install root
//! is `Contains`, never `Owns`.
//!
//! ## Determinism
//!
//! Every ordering is canonical (path bytes, then app id, then kind), every
//! merge is a maximum over a total order or a union, and no map-iteration
//! order reaches the output. `analyze` is a pure function of the fact
//! multiset, so permuting the input cannot change the result.
//!
//! ## Boundedness
//!
//! Analysis works on the facts it is handed; it never enumerates. Retained
//! artifacts and candidates are admitted through [`BoundedTopK`]
//! (**O(limit)** memory), with exact overflow accounting.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::bounded::BoundedTopK;
use crate::domain::{ApplicationId, ApplicationRecord, DiscoveryLimits};
use crate::evidence::{AssociationScope, Confidence, EvidenceKind};
use crate::observe::{PathKey, ProbedKind};
use crate::ownership::{
    EvidenceAccumulator, EvidenceSource, EvidenceStrength, MatchedAttribute, OwnershipAssessment,
    OwnershipEvidence,
};
use crate::roots::ExecutableStatus;

/// An already-observed filesystem fact. Produced by the observation layer
/// from [`crate::observe::PlatformPathProber`]; pure data by the time it
/// reaches analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedArtifact {
    pub path: PathBuf,
    pub kind: ProbedKind,
    /// The canonical object identity, when the platform proved one — the
    /// FULL identity, high bits included. Never a narrowed copy.
    pub identity: Option<coresight_identity::ObjectIdentity>,
    pub size: Option<u64>,
    /// Evidence already attributed to a specific application by the
    /// observation layer (e.g. install-location, executable reference).
    pub attributed_to: Vec<(ApplicationId, EvidenceKind)>,
}

/// How an application relates to one artifact. These are semantically
/// different and are never conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationKind {
    /// The application's install root contains the artifact (structure only).
    Contains,
    /// Evidence supports the application owning the artifact.
    Owns,
    /// Weak/ambiguous evidence links the artifact to the application.
    AssociatedWith,
    /// The artifact is the recorded executable of the application.
    Executable,
    /// The artifact is derived from the application (its cache/logs/data).
    DerivedFrom,
    /// Credible evidence points at several applications (see
    /// [`SharedStatus::Conflicting`]).
    Conflicting,
}

/// Whether an artifact is claimed by one application or several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SharedStatus {
    /// Exactly one application has credible evidence.
    Exclusive,
    /// Two or more applications have credible evidence.
    Shared,
    /// Every claimant has only weak evidence.
    Unknown,
    /// Two or more applications have STRONG-or-better evidence.
    Conflicting,
}

/// One application ↔ artifact relationship with its structured reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRelationship {
    pub app: ApplicationId,
    pub path: PathBuf,
    pub kind: RelationKind,
    pub assessment: OwnershipAssessment,
    pub shared: SharedStatus,
    pub evidence: Vec<OwnershipEvidence>,
    /// Exact count of evidence items offered but not retained (bounded).
    pub evidence_truncated: u64,
}

/// An artifact-centric view: every application plausibly related to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactOwnership {
    pub path: PathBuf,
    pub identity: Option<coresight_identity::ObjectIdentity>,
    pub status: SharedStatus,
    /// Canonically ordered by app id.
    pub claimants: Vec<ArtifactClaimant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactClaimant {
    pub app: ApplicationId,
    pub assessment: OwnershipAssessment,
}

/// The full result of one analysis pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationAnalysis {
    /// Canonically ordered relationships (path bytes, app id, kind).
    pub relationships: Vec<ApplicationRelationship>,
    /// Canonically ordered artifact-centric ownership views.
    pub artifacts: Vec<ArtifactOwnership>,
    /// Read-only, inert recommendation primitives.
    pub candidates: Vec<OwnershipCandidate>,
    /// Exact counts of everything a bound stopped.
    pub truncated: AnalysisTruncation,
}

/// Exact accounting for every bound applied during analysis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTruncation {
    pub relationships_truncated: u64,
    pub artifacts_truncated: u64,
    pub evidence_truncated: u64,
    pub candidates_truncated: u64,
}

/// Inert, read-only recommendation primitives. The presence of a candidate
/// NEVER means authorized, safe, approved, or executed: there is no executor
/// in this phase, and these carry no capability to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CandidateKind {
    /// An artifact strongly associated with an application.
    UninstallArtifact,
    /// An artifact whose application is no longer present.
    Orphan,
    /// An artifact associated with more than one application.
    SharedArtifact,
    /// An artifact with only weak evidence.
    UncertainAssociation,
}

/// A blocker that would prevent an action from being authorized. Analysis
/// only RECORDS blockers; it never resolves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CandidateBlocker {
    /// Ownership is not strongly established.
    InsufficientEvidence,
    /// Several applications claim the artifact.
    ConflictingOwnership,
    /// The artifact is shared between applications.
    SharedArtifact,
    /// Object identity could not be proven.
    UnprovenObjectIdentity,
    /// No executor exists in this build (always true in Phase 6.2).
    NoExecutorInThisPhase,
}

/// An inert analysis candidate. It reports what the evidence SUPPORTS; it
/// performs nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipCandidate {
    pub kind: CandidateKind,
    pub target: PathBuf,
    pub app: Option<ApplicationId>,
    pub confidence: Confidence,
    pub assessment: OwnershipAssessment,
    pub blockers: Vec<CandidateBlocker>,
    /// The structured reasons, never prose alone.
    pub evidence: Vec<OwnershipEvidence>,
}

/// Canonical key for a retained relationship.
fn rel_key(r: &ApplicationRelationship) -> (PathKey, String, RelationKind) {
    (PathKey(r.path.clone()), r.app.0.clone(), r.kind)
}

/// Rank used to prefer one relationship over another for the same key: the
/// stronger assessment wins, then the fuller evidence list — never arrival.
fn rel_rank(r: &ApplicationRelationship) -> (OwnershipAssessment, usize) {
    (r.assessment, r.evidence.len())
}

/// Per-application, per-path evidence accumulator during the first pass.
struct Claim {
    accumulator: EvidenceAccumulator,
}

/// Analyze observed artifacts into application relationships, artifact
/// ownership, and inert candidates.
///
/// Pure, deterministic, and bounded: retained relationships/artifacts/evidence
/// are admitted through [`BoundedTopK`] with **O(limit)** memory, and every
/// capped item is counted exactly.
pub fn analyze(
    apps: &[ApplicationRecord],
    artifacts: &[ObservedArtifact],
    limits: &DiscoveryLimits,
) -> ApplicationAnalysis {
    // --- Pass 1: attribute evidence to (app, path) claims. -------------
    let mut claims: BTreeMap<(String, PathKey), Claim> = BTreeMap::new();
    let mut truncation = AnalysisTruncation::default();

    for artifact in artifacts {
        for (app_id, kind) in &artifact.attributed_to {
            let key = (app_id.0.clone(), PathKey(artifact.path.clone()));
            let claim = claims.entry(key).or_insert_with(|| Claim {
                accumulator: EvidenceAccumulator::new(limits.max_evidence_per_candidate),
            });
            claim
                .accumulator
                .offer(attributed_evidence(artifact, app_id, *kind, apps));
        }
    }

    // A recorded executable path is a DIRECT reference from application
    // metadata — the strongest executable association there is.
    for app in apps {
        let Some(exe) = &app.executable_path else {
            continue;
        };
        let key = (app.id.0.clone(), PathKey(exe.clone()));
        let claim = claims.entry(key).or_insert_with(|| Claim {
            accumulator: EvidenceAccumulator::new(limits.max_evidence_per_candidate),
        });
        claim.accumulator.offer(OwnershipEvidence::new(
            EvidenceKind::ExactExecutablePath,
            EvidenceSource::for_application_source(&app.source),
            EvidenceStrength::Strong,
            crate::ownership::CorrelationGroup::SourceRecord(app.source.clone()),
            AssociationScope::ThisMachine,
            exe.clone(),
            MatchedAttribute::ExecutablePath,
            Some(app.name.clone()),
        ));
    }

    // --- Pass 2: per-artifact sharing / conflict status. ---------------
    let mut per_path: BTreeMap<PathKey, Vec<(String, OwnershipAssessment)>> = BTreeMap::new();
    for ((app, path), claim) in &claims {
        per_path
            .entry(path.clone())
            .or_default()
            .push((app.clone(), claim.accumulator.assessment()));
    }
    let mut status_by_path: BTreeMap<PathKey, SharedStatus> = BTreeMap::new();
    for (path, claimants) in &per_path {
        let credible = claimants.iter().filter(|(_, a)| a.is_credible()).count();
        let strong = claimants
            .iter()
            .filter(|(_, a)| matches!(a, OwnershipAssessment::Strong | OwnershipAssessment::Direct))
            .count();
        let status = if strong >= 2 {
            SharedStatus::Conflicting
        } else if credible >= 2 {
            SharedStatus::Shared
        } else if credible == 1 {
            SharedStatus::Exclusive
        } else {
            SharedStatus::Unknown
        };
        status_by_path.insert(path.clone(), status);
    }

    // --- Pass 3: bounded relationship admission. -----------------------
    let mut retained: BoundedTopK<(PathKey, String, RelationKind), ApplicationRelationship> =
        BoundedTopK::new(limits.max_records);
    let mut candidates: BoundedTopK<(PathKey, String, CandidateKind), OwnershipCandidate> =
        BoundedTopK::new(limits.max_records);
    let mut identity_by_path: BTreeMap<PathKey, Option<coresight_identity::ObjectIdentity>> =
        BTreeMap::new();
    for a in artifacts {
        identity_by_path
            .entry(PathKey(a.path.clone()))
            .or_insert(a.identity);
    }

    for ((app, path), claim) in &claims {
        let accumulator = claim.accumulator.clone();
        let key_path = path.clone();
        let status = status_by_path
            .get(&key_path)
            .copied()
            .unwrap_or(SharedStatus::Unknown);
        let assessment = if status == SharedStatus::Conflicting {
            OwnershipAssessment::Conflicting
        } else {
            accumulator.assessment()
        };
        // Containment-only evidence proves `Contains`, never `Owns`.
        let contains_only = accumulator
            .group_best()
            .keys()
            .all(|g| matches!(g, crate::ownership::CorrelationGroup::InstallRootStructure));
        let kind = if assessment == OwnershipAssessment::Conflicting {
            RelationKind::Conflicting
        } else if contains_only {
            RelationKind::Contains
        } else if matches!(
            assessment,
            OwnershipAssessment::Strong | OwnershipAssessment::Direct
        ) {
            RelationKind::Owns
        } else {
            RelationKind::AssociatedWith
        };
        let (evidence, evidence_overflow) = accumulator.into_parts();
        truncation.evidence_truncated += evidence_overflow;
        let rel = ApplicationRelationship {
            app: ApplicationId(app.clone()),
            path: path.0.clone(),
            kind,
            assessment,
            shared: status,
            evidence,
            evidence_truncated: 0,
        };
        retained.offer(rel_key(&rel), rel, |new, old| rel_rank(new) > rel_rank(old));
    }

    // --- Pass 4: bounded artifact-centric views. -----------------------
    let mut artifact_views: BoundedTopK<
        PathKey,
        (
            Option<coresight_identity::ObjectIdentity>,
            Vec<ArtifactClaimant>,
        ),
    > = BoundedTopK::new(limits.max_records);
    for (path, claimants) in &per_path {
        let mut ordered: Vec<ArtifactClaimant> = claimants
            .iter()
            .map(|(app, assessment)| ArtifactClaimant {
                app: ApplicationId(app.clone()),
                assessment: *assessment,
            })
            .collect();
        ordered.sort_by(|a, b| a.app.0.cmp(&b.app.0));
        let identity = identity_by_path.get(path).copied().flatten();
        artifact_views.offer(path.clone(), (identity, ordered), |_, _| false);
    }

    // --- Pass 5: inert candidates. -------------------------------------
    for ((app, path), claim) in &claims {
        let key_path = path.clone();
        let status = status_by_path
            .get(&key_path)
            .copied()
            .unwrap_or(SharedStatus::Unknown);
        let assessment = if status == SharedStatus::Conflicting {
            OwnershipAssessment::Conflicting
        } else {
            claim.accumulator.assessment()
        };
        let (evidence, _) = claim.accumulator.clone().into_parts();
        let identity_proven = identity_by_path.get(&key_path).copied().flatten().is_some();
        let mut blockers = vec![CandidateBlocker::NoExecutorInThisPhase];
        if !identity_proven {
            blockers.push(CandidateBlocker::UnprovenObjectIdentity);
        }
        let kind = if status == SharedStatus::Conflicting {
            blockers.push(CandidateBlocker::ConflictingOwnership);
            CandidateKind::SharedArtifact
        } else if status == SharedStatus::Shared {
            blockers.push(CandidateBlocker::SharedArtifact);
            CandidateKind::SharedArtifact
        } else if assessment.is_credible() {
            CandidateKind::UninstallArtifact
        } else {
            blockers.push(CandidateBlocker::InsufficientEvidence);
            CandidateKind::UncertainAssociation
        };
        blockers.sort();
        blockers.dedup();
        let candidate = OwnershipCandidate {
            kind,
            target: path.0.clone(),
            app: Some(ApplicationId(app.clone())),
            confidence: assessment
                .strength()
                .map(|s| s.to_confidence())
                .unwrap_or(Confidence::Unknown),
            assessment,
            blockers,
            evidence,
        };
        candidates.offer((key_path, app.clone(), kind), candidate, |_, _| false);
    }

    // --- Publish in canonical order. -----------------------------------
    let (rel_items, rel_overflow) = retained.into_sorted();
    truncation.relationships_truncated = rel_overflow;
    let mut relationships: Vec<ApplicationRelationship> =
        rel_items.into_iter().map(|(_, r)| r).collect();
    relationships.sort_by(|a, b| {
        PathKey(a.path.clone())
            .cmp(&PathKey(b.path.clone()))
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
    });

    let (art_items, art_overflow) = artifact_views.into_sorted();
    truncation.artifacts_truncated = art_overflow;
    let mut artifact_list: Vec<ArtifactOwnership> = art_items
        .into_iter()
        .map(|(path, (identity, claimants))| {
            let status = status_by_path
                .get(&path)
                .copied()
                .unwrap_or(SharedStatus::Unknown);
            ArtifactOwnership {
                path: path.0,
                identity,
                status,
                claimants,
            }
        })
        .collect();
    artifact_list.sort_by_key(|a| PathKey(a.path.clone()));

    let (cand_items, cand_overflow) = candidates.into_sorted();
    truncation.candidates_truncated = cand_overflow;
    let mut candidate_list: Vec<OwnershipCandidate> =
        cand_items.into_iter().map(|(_, c)| c).collect();
    candidate_list.sort_by(|a, b| {
        PathKey(a.target.clone())
            .cmp(&PathKey(b.target.clone()))
            .then(
                a.app
                    .as_ref()
                    .map(|x| x.0.as_str())
                    .unwrap_or("")
                    .cmp(b.app.as_ref().map(|x| x.0.as_str()).unwrap_or("")),
            )
            .then(a.kind.cmp(&b.kind))
    });

    ApplicationAnalysis {
        relationships,
        artifacts: artifact_list,
        candidates: candidate_list,
        truncated: truncation,
    }
}

/// Build the structured evidence for one observation-layer attribution.
fn attributed_evidence(
    artifact: &ObservedArtifact,
    app_id: &ApplicationId,
    kind: EvidenceKind,
    apps: &[ApplicationRecord],
) -> OwnershipEvidence {
    let app = apps.iter().find(|a| &a.id == app_id);
    let source = app
        .map(|a| EvidenceSource::for_application_source(&a.source))
        .unwrap_or(EvidenceSource::InventoryRecord);
    let group = match kind {
        // Authoritative metadata that names a PATH belongs to the source
        // record that supplied it: two fields read from the SAME registry
        // key / package manifest are one signal, not two.
        EvidenceKind::InstallLocation
        | EvidenceKind::RegistryReference
        | EvidenceKind::PackageIdentity
        | EvidenceKind::ExactExecutablePath
        | EvidenceKind::DesktopEntryReference => crate::ownership::CorrelationGroup::SourceRecord(
            app.map(|a| a.source.clone())
                .unwrap_or(crate::domain::ApplicationSource::FilesystemPresence),
        ),
        EvidenceKind::ObjectIdentityMatch => crate::ownership::CorrelationGroup::ObjectIdentity,
        EvidenceKind::BundleIdentifierReference => {
            crate::ownership::CorrelationGroup::BundleIdentifier
        }
        EvidenceKind::InstallRootContainment | EvidenceKind::BundleStructure => {
            crate::ownership::CorrelationGroup::InstallRootStructure
        }
        // Everything else is name/similarity-derived: ONE group, however
        // many items the observer offers.
        _ => crate::ownership::CorrelationGroup::NameDerived,
    };
    let requested = kind.max_strength();
    let attribute = match kind {
        EvidenceKind::InstallLocation => MatchedAttribute::InstallLocation,
        EvidenceKind::ExactExecutablePath | EvidenceKind::ExecutableReference => {
            MatchedAttribute::ExecutablePath
        }
        EvidenceKind::BundleIdentifierReference => MatchedAttribute::BundleIdentifier,
        EvidenceKind::ObjectIdentityMatch => MatchedAttribute::ObjectIdentity,
        EvidenceKind::InstallRootContainment | EvidenceKind::BundleStructure => {
            MatchedAttribute::InstallRoot
        }
        EvidenceKind::PublisherDirectory => MatchedAttribute::Publisher,
        _ => MatchedAttribute::ApplicationName,
    };
    OwnershipEvidence::new(
        kind,
        source,
        requested,
        group,
        AssociationScope::ThisMachine,
        artifact.path.clone(),
        attribute,
        app.map(|a| a.name.clone()),
    )
}

/// Report the executable association for one application as structured
/// evidence plus its status. Pure; the caller supplies the observation.
pub fn executable_evidence(
    app: &ApplicationRecord,
    status: ExecutableStatus,
    path: &std::path::Path,
) -> Option<OwnershipEvidence> {
    let kind = match status {
        ExecutableStatus::ObservedExact => EvidenceKind::ExactExecutablePath,
        ExecutableStatus::Inferred => EvidenceKind::BundleStructure,
        ExecutableStatus::Candidate => EvidenceKind::FilenameSimilarity,
        ExecutableStatus::Unknown => return None,
    };
    Some(OwnershipEvidence::new(
        kind,
        EvidenceSource::ExecutableMetadata,
        kind.max_strength(),
        match status {
            ExecutableStatus::ObservedExact => {
                crate::ownership::CorrelationGroup::SourceRecord(app.source.clone())
            }
            ExecutableStatus::Inferred => crate::ownership::CorrelationGroup::InstallRootStructure,
            ExecutableStatus::Candidate => crate::ownership::CorrelationGroup::NameDerived,
            ExecutableStatus::Unknown => crate::ownership::CorrelationGroup::NameDerived,
        },
        AssociationScope::ThisMachine,
        path.to_path_buf(),
        MatchedAttribute::ExecutablePath,
        Some(app.name.clone()),
    ))
}

/// Whether an analysis can ever authorize execution. Always `false`: the
/// safety invariant, expressed as a function so it is testable.
pub fn can_authorize_execution(_analysis: &ApplicationAnalysis) -> bool {
    false
}

/// Re-exported for callers that need the group arithmetic directly.
pub use crate::ownership::assess_groups as assess;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ApplicationSource, PackageKind};

    fn app(name: &str, publisher: Option<&str>) -> ApplicationRecord {
        ApplicationRecord {
            id: ApplicationId::derive(name, publisher),
            name: name.to_string(),
            version: None,
            publisher: publisher.map(str::to_string),
            install_location: None,
            install_date: None,
            estimated_size_bytes: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            install_source: None,
            source: ApplicationSource::RegistryUninstall,
            kind: PackageKind::Installed,
            system_component: false,
            observed_in_views: Vec::new(),
            bundle_identifier: None,
            executable_path: None,
            provenance: Vec::new(),
        }
    }

    fn artifact(path: &str, attributed: Vec<(ApplicationId, EvidenceKind)>) -> ObservedArtifact {
        ObservedArtifact {
            path: PathBuf::from(path),
            kind: ProbedKind::File,
            identity: Some(coresight_identity::ObjectIdentity {
                volume: 1,
                file_id: 42,
                file_id_hi: Some(7),
            }),
            size: Some(10),
            attributed_to: attributed,
        }
    }

    #[test]
    fn one_app_one_artifact_is_exclusive() {
        let a = app("Solo", Some("Vendor"));
        let artifacts = vec![artifact(
            "/data/solo/cache",
            vec![(a.id.clone(), EvidenceKind::InstallLocation)],
        )];
        let out = analyze(&[a], &artifacts, &DiscoveryLimits::default());
        assert_eq!(out.relationships.len(), 1);
        assert_eq!(out.relationships[0].kind, RelationKind::Owns);
        assert_eq!(out.relationships[0].shared, SharedStatus::Exclusive);
        assert_eq!(out.artifacts[0].status, SharedStatus::Exclusive);
    }

    #[test]
    fn two_apps_sharing_an_artifact_is_shared_not_exclusive() {
        let a = app("Alpha", Some("Vendor"));
        let b = app("Beta", Some("Vendor"));
        let artifacts = vec![artifact(
            "/shared/runtime",
            vec![
                (a.id.clone(), EvidenceKind::InstallRootContainment),
                (b.id.clone(), EvidenceKind::InstallRootContainment),
            ],
        )];
        let out = analyze(&[a, b], &artifacts, &DiscoveryLimits::default());
        assert_eq!(out.artifacts[0].status, SharedStatus::Shared);
        // Containment alone never becomes ownership.
        for rel in &out.relationships {
            assert_eq!(rel.kind, RelationKind::Contains);
        }
    }

    #[test]
    fn two_strong_claims_are_conflicting_and_never_silently_resolved() {
        let a = app("Alpha", Some("Vendor"));
        let b = app("Beta", Some("Vendor"));
        let artifacts = vec![artifact(
            "/contested/app.exe",
            vec![
                (a.id.clone(), EvidenceKind::InstallLocation),
                (b.id.clone(), EvidenceKind::InstallLocation),
            ],
        )];
        let out = analyze(&[a, b], &artifacts, &DiscoveryLimits::default());
        assert_eq!(out.artifacts[0].status, SharedStatus::Conflicting);
        assert_eq!(out.artifacts[0].claimants.len(), 2, "both owners preserved");
        for rel in &out.relationships {
            assert_eq!(rel.kind, RelationKind::Conflicting);
            assert_eq!(rel.assessment, OwnershipAssessment::Conflicting);
        }
    }

    #[test]
    fn weak_plus_strong_is_exclusive_and_not_conflicted() {
        let a = app("Alpha", Some("Vendor"));
        let b = app("Beta", Some("Vendor"));
        let artifacts = vec![artifact(
            "/mixed/x",
            vec![
                (a.id.clone(), EvidenceKind::InstallLocation),
                (b.id.clone(), EvidenceKind::FilenameSimilarity),
            ],
        )];
        let out = analyze(&[a, b], &artifacts, &DiscoveryLimits::default());
        assert_eq!(out.artifacts[0].status, SharedStatus::Exclusive);
    }

    #[test]
    fn weak_only_claims_are_unknown_not_shared() {
        let a = app("Alpha", Some("Vendor"));
        let b = app("Beta", Some("Vendor"));
        let artifacts = vec![artifact(
            "/weak/x",
            vec![
                (a.id.clone(), EvidenceKind::FilenameSimilarity),
                (b.id.clone(), EvidenceKind::FilenameSimilarity),
            ],
        )];
        let out = analyze(&[a, b], &artifacts, &DiscoveryLimits::default());
        assert_eq!(out.artifacts[0].status, SharedStatus::Unknown);
    }

    #[test]
    fn analysis_is_permutation_invariant() {
        let a = app("Alpha", Some("Vendor"));
        let b = app("Beta", Some("Vendor"));
        let arts = vec![
            artifact(
                "/z/one",
                vec![
                    (a.id.clone(), EvidenceKind::InstallLocation),
                    (b.id.clone(), EvidenceKind::FilenameSimilarity),
                ],
            ),
            artifact(
                "/a/two",
                vec![(b.id.clone(), EvidenceKind::InstallLocation)],
            ),
        ];
        let mut reversed = arts.clone();
        reversed.reverse();
        let forward = analyze(&[a.clone(), b.clone()], &arts, &DiscoveryLimits::default());
        let backward = analyze(&[b, a], &reversed, &DiscoveryLimits::default());
        assert_eq!(forward, backward, "arrival order must not matter");
    }

    #[test]
    fn relationships_and_candidates_are_bounded_with_exact_accounting() {
        let a = app("Bulk", Some("Vendor"));
        let arts: Vec<ObservedArtifact> = (0..500)
            .map(|i| {
                artifact(
                    &format!("/bulk/f{i:05}"),
                    vec![(a.id.clone(), EvidenceKind::InstallLocation)],
                )
            })
            .collect();
        let limits = DiscoveryLimits {
            max_records: 25,
            ..DiscoveryLimits::default()
        };
        let out = analyze(&[a], &arts, &limits);
        assert_eq!(out.relationships.len(), 25);
        assert_eq!(out.artifacts.len(), 25);
        assert_eq!(out.candidates.len(), 25);
        assert_eq!(out.truncated.relationships_truncated, 475);
        assert_eq!(out.truncated.artifacts_truncated, 475);
        assert_eq!(out.truncated.candidates_truncated, 475);
    }

    #[test]
    fn candidates_are_inert_and_carry_blockers() {
        let a = app("Alpha", Some("Vendor"));
        let artifacts = vec![artifact(
            "/a/owned",
            vec![(a.id.clone(), EvidenceKind::InstallLocation)],
        )];
        let out = analyze(&[a], &artifacts, &DiscoveryLimits::default());
        let c = &out.candidates[0];
        assert!(
            c.blockers
                .contains(&CandidateBlocker::NoExecutorInThisPhase),
            "no-executor blocker is always recorded"
        );
        assert!(!can_authorize_execution(&out));
    }

    #[test]
    fn unproven_identity_is_recorded_as_a_blocker_and_never_fabricated() {
        let a = app("Alpha", Some("Vendor"));
        let mut art = artifact(
            "/a/unknown",
            vec![(a.id.clone(), EvidenceKind::InstallLocation)],
        );
        art.identity = None;
        let out = analyze(&[a], &artifacts_with(art), &DiscoveryLimits::default());
        assert!(out.artifacts[0].identity.is_none(), "never fabricated");
        assert!(out.candidates[0]
            .blockers
            .contains(&CandidateBlocker::UnprovenObjectIdentity));
    }

    fn artifacts_with(a: ObservedArtifact) -> Vec<ObservedArtifact> {
        vec![a]
    }

    #[test]
    fn recorded_executable_is_a_direct_reference() {
        let mut a = app("Alpha", Some("Vendor"));
        a.executable_path = Some(PathBuf::from("/opt/alpha/alpha"));
        let out = analyze(&[a], &[], &DiscoveryLimits::default());
        let rel = out
            .relationships
            .iter()
            .find(|r| r.path.as_os_str() == "/opt/alpha/alpha")
            .expect("executable relationship");
        assert_eq!(rel.kind, RelationKind::Owns);
        assert_eq!(rel.assessment, OwnershipAssessment::Strong);
    }

    #[test]
    fn wide_object_identity_is_preserved_through_analysis() {
        let a = app("Alpha", Some("Vendor"));
        let art = ObservedArtifact {
            path: PathBuf::from("/wide/file"),
            kind: ProbedKind::File,
            identity: Some(coresight_identity::ObjectIdentity {
                volume: 9,
                file_id: 1,
                file_id_hi: Some(2),
            }),
            size: None,
            attributed_to: vec![(a.id.clone(), EvidenceKind::InstallLocation)],
        };
        let out = analyze(&[a], &[art], &DiscoveryLimits::default());
        let id = out.artifacts[0].identity.expect("identity preserved");
        assert_eq!(id.file_id_hi, Some(2), "high bits never collapse");
        assert_eq!(
            id,
            coresight_identity::ObjectIdentity {
                volume: 9,
                file_id: 1,
                file_id_hi: Some(2)
            }
        );
    }

    #[test]
    fn assess_is_the_shared_group_function() {
        let mut m = BTreeMap::new();
        m.insert(
            crate::ownership::CorrelationGroup::NameDerived,
            EvidenceStrength::Weak,
        );
        assert_eq!(assess(&m), OwnershipAssessment::Weak);
    }
}
