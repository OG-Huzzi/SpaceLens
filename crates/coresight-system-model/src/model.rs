//! The canonical unified **system model** (Phase 6.3).
//!
//! ```text
//! Observation ─▶ Identity / Classification ─▶ Application Intelligence
//!                                                   │
//!                                                   ▼
//!                                              Correlation
//!                                                   │
//!                                                   ▼
//!                                             System Model
//!                                                   │
//!                                                   ▼
//!                                            Queries / Insights
//! ```
//!
//! One coherent, **immutable** graph instead of several unrelated maps
//! maintained by callers. The model is produced by a pure function
//! ([`crate::build::build_system_model`]) from facts that were already
//! observed elsewhere; it performs **no I/O**, and it has no way to perform
//! any (it has no filesystem, process, or network capability).
//!
//! ## Design contracts
//!
//! * **Node identity is never a path.** An artifact node is keyed by its
//!   canonical [`ObjectIdentity`] where the platform proved one, and by a
//!   lossless path key otherwise. Path, object identity, and content identity
//!   are three different facts and remain separately representable.
//! * **Containment is not ownership.** [`SystemEdgeKind`] keeps `Contains`,
//!   `LocatedUnder`, `OwnedBy`, `AssociatedWith`, `SharedBy`, and the
//!   relationship kinds distinct.
//! * **Observed ≠ inferred.** Every node carries its [`ProvenanceState`].
//! * **Evidence is traceable.** Every edge and insight carries structured
//!   [`coresight_apps::OwnershipEvidence`] items whose strength was already
//!   clamped by the Phase 6.2 correlation ceilings.
//! * **Conflicts are preserved.** Contradictory claims coexist; nothing is
//!   averaged and nothing is overwritten by arrival order.
//! * **Bounded by admission.** Every collection admits through
//!   [`coresight_apps::BoundedTopK`] — O(limit) memory, never
//!   collect-then-truncate.
//! * **Deterministic.** Canonical ordering everywhere; the model is a pure
//!   function of the input fact *set*.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use coresight_apps::{ApplicationId, OwnershipAssessment, OwnershipEvidence};
use coresight_capabilities::access::AccessState;
use coresight_capabilities::{CapabilityId, CapabilityStatus};
use coresight_classifier::{Category, Subcategory};
use coresight_identity::ObjectIdentity;

use crate::pathkey::ArtifactKey;

/// Limits for one model build. Every limit is explicit; every capped item is
/// counted exactly in [`ModelTruncation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemModelLimits {
    /// Maximum artifact nodes published.
    pub max_artifacts: usize,
    /// Maximum application nodes published.
    pub max_applications: usize,
    /// Maximum edges published (all kinds share this ceiling).
    pub max_edges: usize,
    /// Maximum evidence items retained per edge.
    pub max_evidence_per_edge: usize,
    /// Maximum edges retained per node across all kinds.
    pub max_edges_per_node: usize,
    /// Maximum historical context records published.
    pub max_historical_context: usize,
    /// Maximum insights published.
    pub max_insights: usize,
    /// Maximum analysis candidates published.
    pub max_candidates: usize,
}

impl Default for SystemModelLimits {
    fn default() -> Self {
        SystemModelLimits {
            max_artifacts: 200_000,
            max_applications: 20_000,
            max_edges: 500_000,
            max_evidence_per_edge: 16,
            max_edges_per_node: 256,
            max_historical_context: 4_096,
            max_insights: 4_096,
            max_candidates: 4_096,
        }
    }
}

/// Exact count of everything a limit stopped during one build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTruncation {
    pub artifacts_truncated: u64,
    pub applications_truncated: u64,
    pub edges_truncated: u64,
    pub edges_per_node_truncated: u64,
    pub evidence_truncated: u64,
    pub historical_context_truncated: u64,
    pub insights_truncated: u64,
    pub candidates_truncated: u64,
}

/// How a fact came to be known. The observed/inferred distinction survives
/// every aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProvenanceState {
    /// Read directly from the system.
    Observed,
    /// Derived from observed facts by a documented rule.
    Inferred,
    /// A bound estimate (e.g. a declared install size).
    Estimated,
    /// A plausible candidate, offered but not asserted.
    Candidate,
    /// No usable fact.
    Unknown,
    /// This build cannot service the source at all.
    Unsupported,
    /// The source is absent in this machine state.
    Unavailable,
    /// Attempted and failed.
    Failed,
}

impl ProvenanceState {
    /// Only `Observed`/`Inferred` carry a usable payload — the same rule the
    /// Phase 6.1 [`coresight_capabilities::Observation`] envelope enforces.
    pub fn is_usable(self) -> bool {
        matches!(self, ProvenanceState::Observed | ProvenanceState::Inferred)
    }

    /// Map a path-access fact onto model provenance. `denied != empty`.
    pub fn from_access(state: AccessState) -> Self {
        match state {
            AccessState::ReadSucceeded => ProvenanceState::Observed,
            // A proven-empty read is a genuine observation of emptiness.
            AccessState::Empty => ProvenanceState::Observed,
            AccessState::DoesNotExist => ProvenanceState::Unavailable,
            AccessState::ExistsButInaccessible => ProvenanceState::Unavailable,
            AccessState::NotApplicable => ProvenanceState::Unavailable,
            AccessState::Unsupported => ProvenanceState::Unsupported,
            AccessState::Failed => ProvenanceState::Failed,
        }
    }
}

/// What kind of thing a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SystemNodeKind {
    Artifact,
    Application,
}

/// The canonical **artifact node**: one filesystem object as understood by
/// the unified model.
///
/// Path, object identity, and content identity are three independent facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactNode {
    /// Stable node key (lossless). See [`crate::pathkey`].
    pub key: String,
    /// The exact observed path (lossless).
    pub path: PathBuf,
    /// The canonical object identity, with its wide high bits intact, or
    /// `None` when the platform proved none. Never narrowed, never
    /// fabricated from the path.
    pub identity: Option<ObjectIdentity>,
    /// The verified content digest, when the identity engine proved one.
    pub content_sha256: Option<String>,
    /// The kind reported by the observation layer.
    pub observed_kind: coresight_apps::ProbedKind,
    pub size: Option<u64>,
    /// The classifier's verdict for this artifact, if it was classified.
    pub category: Option<Category>,
    pub subcategory: Option<Subcategory>,
    /// The classification's own confidence band (kept separate from
    /// ownership confidence — they answer different questions).
    pub classification_confidence: Option<coresight_classifier::Confidence>,
    /// The access state observed for this artifact (`denied != empty`).
    pub access: AccessState,
    /// Provenance of the artifact facts.
    pub provenance: ProvenanceState,
    /// How this artifact was classified against applications.
    pub application_status: ArtifactApplicationStatus,
    /// Number of distinct applications with a credible claim.
    pub credible_claimants: u32,
}

/// The model's explicit vocabulary for "which application(s) does this
/// artifact relate to". Critically, **"no association observed" is not
/// "orphan"**: an artifact whose application sources were unsupported is a
/// different fact from one no application claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArtifactApplicationStatus {
    /// No application claim was offered, and application discovery ran.
    Unassociated,
    /// Exactly one application has a credible claim.
    Associated,
    /// Several applications share it.
    Shared,
    /// Several applications hold strong-or-better claims.
    Conflicting,
    /// Claims exist but none is credible.
    Uncertain,
    /// Application discovery was unsupported on this host.
    AssociationUnsupported,
    /// Application discovery was unavailable (no source could be read).
    AssociationUnavailable,
    /// Application discovery was attempted and failed.
    AssociationFailed,
}

impl ArtifactApplicationStatus {
    /// True only when the model genuinely observed no claim AND the
    /// application sources were actually usable. This is the ONLY condition
    /// under which "orphan-like" reasoning is permitted, and even then the
    /// model does not call it an orphan (see [`InsightKind::UnassociatedArtifact`]).
    pub fn is_genuinely_unassociated(self) -> bool {
        matches!(self, ArtifactApplicationStatus::Unassociated)
    }

    /// True when the absence of association is an artifact of source state,
    /// not a fact about the machine.
    pub fn is_association_unknown(self) -> bool {
        matches!(
            self,
            ArtifactApplicationStatus::AssociationUnsupported
                | ArtifactApplicationStatus::AssociationUnavailable
                | ArtifactApplicationStatus::AssociationFailed
        )
    }
}

/// The canonical **application node**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationNode {
    pub id: ApplicationId,
    pub name: String,
    pub publisher: Option<String>,
    pub bundle_identifier: Option<String>,
    pub install_location: Option<PathBuf>,
    pub executable_path: Option<PathBuf>,
    /// Unioned provenance (source is never identity).
    pub provenance: Vec<coresight_apps::ApplicationSource>,
    /// How well this application resolved against observed artifacts.
    pub state: ApplicationState,
    /// Structured reasons behind [`Self::state`].
    pub state_reasons: Vec<ApplicationStateReason>,
}

/// How well an application's footprint resolved. Analysis only — none of
/// these states claims the application is "broken".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApplicationState {
    /// An install root was observed and at least one artifact resolved.
    Resolved,
    /// Some expected parts resolved and some did not.
    PartiallyResolved,
    /// The application was recorded but nothing of it could be observed.
    Unresolved,
    /// Not enough information to judge.
    Unknown,
}

/// One structured reason behind an [`ApplicationState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApplicationStateReason {
    /// The install location was observed to exist.
    InstallRootObserved,
    /// The install location was recorded but not observed.
    InstallRootUnobserved,
    /// The recorded install location does not exist.
    InstallRootMissing,
    /// An exact executable was recorded and observed.
    ExecutableObserved,
    /// An exact executable was recorded but could not be observed.
    ExecutableUnobserved,
    /// No executable was recorded.
    ExecutableNotRecorded,
    /// Some expected application data was inaccessible.
    ExpectedDataInaccessible,
    /// More than one executable candidate was found.
    DuplicateExecutableCandidates,
    /// The application's own metadata contradicted itself.
    ConflictingMetadata,
    /// Application sources were unsupported/unavailable.
    SourcesIncomplete,
}

/// The explicit edge vocabulary. Every variant is supportable by evidence the
/// existing engines actually produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SystemEdgeKind {
    // ---- Artifact → Artifact structure ---------------------------------
    /// A directory artifact contains a child artifact (proven by paths).
    Contains,
    /// An artifact lies under a directory artifact (same fact as
    /// [`Self::Contains`] read from the child's side; both are emitted so
    /// either direction is a single lookup).
    LocatedUnder,
    // ---- Application → Artifact ----------------------------------------
    /// The artifact is the application's install root.
    ApplicationInstallRoot,
    /// The artifact is the application's recorded executable.
    ApplicationExecutable,
    /// The artifact is application data (classifier category corroborates).
    ApplicationData,
    /// The artifact is a cache (classifier category corroborates).
    ApplicationCache,
    /// The artifact is a log (classifier category corroborates).
    ApplicationLog,
    /// The artifact is configuration.
    ApplicationConfig,
    /// Ownership is proven at `Strong`/`Direct`.
    OwnedBy,
    /// Weaker, explicit association.
    AssociatedWith,
    /// Several applications relate to this artifact.
    SharedBy,
    // ---- Artifact → Artifact identity relationships ---------------------
    /// Distinct objects with byte-identical content (identity engine).
    DuplicateOf,
    /// Different paths, same filesystem object (identity engine).
    HardLinkAliasOf,
    // ---- History context (only from real history facts) -----------------
    /// A stored historical observation names this path/object.
    HistoricalAliasOf,
    /// History proves this path previously referred to a different object.
    HistoricalMoveOf,
}

impl SystemEdgeKind {
    /// Which domain produced this edge.
    pub fn domain(self) -> EdgeDomain {
        match self {
            SystemEdgeKind::Contains | SystemEdgeKind::LocatedUnder => EdgeDomain::Filesystem,
            SystemEdgeKind::ApplicationInstallRoot
            | SystemEdgeKind::ApplicationExecutable
            | SystemEdgeKind::ApplicationData
            | SystemEdgeKind::ApplicationCache
            | SystemEdgeKind::ApplicationLog
            | SystemEdgeKind::ApplicationConfig
            | SystemEdgeKind::OwnedBy
            | SystemEdgeKind::AssociatedWith
            | SystemEdgeKind::SharedBy => EdgeDomain::ApplicationIntelligence,
            SystemEdgeKind::DuplicateOf | SystemEdgeKind::HardLinkAliasOf => EdgeDomain::Identity,
            SystemEdgeKind::HistoricalAliasOf | SystemEdgeKind::HistoricalMoveOf => {
                EdgeDomain::History
            }
        }
    }

    /// True for edges that assert ownership semantics (as opposed to mere
    /// structure or a descriptive role).
    ///
    /// This is the ONLY predicate the model uses to count claimants, so a
    /// descriptive role edge (e.g. "this is the app's cache") can never
    /// inflate the number of applications claiming an artifact, and pure
    /// containment can never be read as ownership.
    pub fn asserts_ownership(self) -> bool {
        matches!(
            self,
            SystemEdgeKind::OwnedBy | SystemEdgeKind::AssociatedWith
        )
    }

    /// True for descriptive role edges. These describe WHAT an artifact is
    /// relative to an application (its install root, executable, data,
    /// cache, logs, configuration). They carry an assessment but are not
    /// counted as ownership claims of their own.
    pub fn is_descriptive_role(self) -> bool {
        matches!(
            self,
            SystemEdgeKind::ApplicationInstallRoot
                | SystemEdgeKind::ApplicationExecutable
                | SystemEdgeKind::ApplicationData
                | SystemEdgeKind::ApplicationCache
                | SystemEdgeKind::ApplicationLog
                | SystemEdgeKind::ApplicationConfig
        )
    }

    /// Canonical rank for ordering edges of one node.
    fn rank(self) -> u8 {
        match self {
            SystemEdgeKind::Contains => 0,
            SystemEdgeKind::LocatedUnder => 1,
            SystemEdgeKind::ApplicationInstallRoot => 2,
            SystemEdgeKind::ApplicationExecutable => 3,
            SystemEdgeKind::ApplicationData => 4,
            SystemEdgeKind::ApplicationCache => 5,
            SystemEdgeKind::ApplicationLog => 6,
            SystemEdgeKind::ApplicationConfig => 7,
            SystemEdgeKind::OwnedBy => 8,
            SystemEdgeKind::AssociatedWith => 9,
            SystemEdgeKind::SharedBy => 10,
            SystemEdgeKind::DuplicateOf => 11,
            SystemEdgeKind::HardLinkAliasOf => 12,
            SystemEdgeKind::HistoricalAliasOf => 13,
            SystemEdgeKind::HistoricalMoveOf => 14,
        }
    }
}

/// Which subsystem a conclusion came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeDomain {
    Filesystem,
    Classification,
    Identity,
    ApplicationIntelligence,
    History,
}

/// One edge of the system graph. Carries the machine-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemEdge {
    pub kind: SystemEdgeKind,
    pub domain: EdgeDomain,
    pub from: String,
    pub to: String,
    /// How strongly the edge itself is established.
    pub assessment: OwnershipAssessment,
    pub provenance: ProvenanceState,
    /// Structured, already-clamped evidence. Never prose alone.
    pub evidence: Vec<OwnershipEvidence>,
}

impl SystemEdge {
    /// Canonical ordering key: kind rank, then `from`, then `to`.
    pub(crate) fn order_key(&self) -> (u8, String, String) {
        (self.kind.rank(), self.from.clone(), self.to.clone())
    }

    /// Deduplication key: two edges describe the same fact when they share
    /// kind and endpoints. Arrival order never decides which one survives —
    /// the stronger, larger-evidence edge does.
    pub(crate) fn fact_key(&self) -> (u8, String, String) {
        (self.kind.rank(), self.from.clone(), self.to.clone())
    }

    /// Strength used to pick between duplicate facts about the same pair.
    pub(crate) fn rank(&self) -> (OwnershipAssessment, usize) {
        (self.assessment, self.evidence.len())
    }
}

/// A stored historical observation projected into the model. **Never**
/// derived from current state — it exists only when the history subsystem
/// supplied the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalContext {
    /// The run the fact came from.
    pub run_id: String,
    pub path: PathBuf,
    /// The object identity recorded for that path in that run.
    pub identity: Option<ObjectIdentity>,
    /// The classification recorded in that run.
    pub category: Option<String>,
    pub provenance: ProvenanceState,
}

/// Which capability a report refers to, and that capability's honest status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityState {
    pub capability: CapabilityId,
    pub status: CapabilityStatus,
    /// Unsatisfied prerequisites, verbatim from the source layer. The model
    /// never upgrades these to "supported".
    pub blockers: Vec<String>,
}

/// Per-domain coverage summary, so an empty result can never be mistaken for
/// a complete one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationSummary {
    /// One honest status per application source.
    pub source_states: Vec<SourceStateSummary>,
    /// Capability states referenced by this model.
    pub capabilities: Vec<CapabilityState>,
    /// Artifacts whose observation was not a completed read, by access state.
    pub inaccessible_artifacts: u64,
    pub unsupported_artifacts: u64,
    pub failed_artifacts: u64,
}

/// One application source's honest status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStateSummary {
    pub source: String,
    pub status: coresight_apps::SourceStatus,
    pub note: Option<String>,
}

/// Higher-order conclusions. Every field is descriptive; none authorizes an
/// action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInsight {
    pub id: String,
    pub kind: InsightKind,
    pub severity: InsightSeverity,
    /// Nodes the insight concerns, canonically ordered.
    pub related_nodes: Vec<String>,
    /// Related applications, when the insight is application-scoped.
    pub related_applications: Vec<ApplicationId>,
    pub evidence: Vec<OwnershipEvidence>,
    /// Machine-readable explanation of how the insight was derived.
    pub explanation: String,
    /// What must be resolved before any later phase could act on it.
    pub blockers: Vec<InsightBlocker>,
}

/// The insight vocabulary. Deliberately avoids any safety/action claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InsightKind {
    /// An artifact no application claimed while application discovery was
    /// usable. NOT the same as "orphan", and NOT a cleanup candidate.
    UnassociatedArtifact,
    /// Several applications relate to one artifact.
    SharedArtifact,
    /// Several applications hold strong claims on one artifact.
    ConflictingOwnership,
    /// Distinct objects holding identical content.
    DuplicateContent,
    /// One filesystem object reachable through several paths.
    HardLinkAlias,
    /// An application only partly resolved against observed artifacts.
    PartialApplication,
    /// An application recorded but not observed at all.
    UnresolvedApplication,
    /// A stored historical fact is relevant to this artifact.
    HistoricalContext,
}

/// Descriptive severity. Not a safety rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InsightSeverity {
    /// Informational only.
    Informational,
    /// Worth attention.
    Notable,
    /// Needs resolution before any later phase could proceed.
    NeedsResolution,
}

/// A blocker recorded on an insight. Records; never resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InsightBlocker {
    /// Ownership is not strongly established.
    InsufficientEvidence,
    /// Several applications claim the same artifact.
    ConflictingOwnership,
    /// The artifact is shared.
    SharedArtifactOwnership,
    /// Object identity could not be proven.
    UnprovenObjectIdentity,
    /// Application discovery was not usable for this host.
    ApplicationSourcesIncomplete,
    /// A required location was inaccessible.
    InaccessibleData,
    /// History was not available as context.
    HistoryUnavailable,
    /// No executor exists in this build (always present).
    NoExecutorInThisPhase,
}

/// An inert, read-only candidate. The presence of a candidate never means
/// authorized, safe, approved, or executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemCandidate {
    pub action_kind: CandidateActionKind,
    pub target: String,
    pub path: PathBuf,
    pub confidence: coresight_apps::Confidence,
    pub assessment: OwnershipAssessment,
    pub effect: coresight_capabilities::ActionClass,
    pub blockers: Vec<InsightBlocker>,
    pub evidence: Vec<OwnershipEvidence>,
}

/// The kind of analysis candidate. All are inert data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CandidateActionKind {
    /// An artifact associated with an application.
    UninstallArtifact,
    /// An artifact no application claimed.
    Orphan,
    /// An artifact several applications relate to.
    SharedArtifact,
    /// An artifact whose association is not credible.
    UncertainAssociation,
}

/// The immutable, finalized system model.
///
/// Constructed only by [`crate::build::build_system_model`]; every field is
/// public for reading but the type has no mutating API, so callers cannot
/// create an inconsistent graph. All indexes are built at finalization and
/// therefore cannot go stale.
///
/// `indexes` is serialized through a string-keyed form (see
/// [`SerialIndexes`]) because serde maps require string keys; the canonical
/// data is the node/edge sets, and the indexes are exactly reconstructible
/// from them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemModel {
    /// Artifact nodes, canonically ordered by node key.
    pub artifacts: Vec<ArtifactNode>,
    /// Application nodes, canonically ordered by application id.
    pub applications: Vec<ApplicationNode>,
    /// Edges, canonically ordered by (kind rank, from, to).
    pub edges: Vec<SystemEdge>,
    /// Historical context records, canonically ordered.
    pub historical_context: Vec<HistoricalContext>,
    /// Higher-order conclusions, canonically ordered by id.
    pub insights: Vec<SystemInsight>,
    /// Inert read-only candidates, canonically ordered.
    pub candidates: Vec<SystemCandidate>,
    /// Honest coverage of the domains this model joined.
    pub observations: ObservationSummary,
    /// Exact truncation accounting for every applied bound.
    pub truncation: ModelTruncation,
    /// Derived indexes. Kept private-ish via accessor methods so they cannot
    /// be mutated independently of the node sets.
    #[serde(
        serialize_with = "serialize_model_indexes",
        deserialize_with = "deserialize_model_indexes"
    )]
    indexes: ModelIndexes,
}

fn serialize_model_indexes<S>(indexes: &ModelIndexes, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    indexes.to_serial().serialize(serializer)
}

fn deserialize_model_indexes<'de, D>(deserializer: D) -> Result<ModelIndexes, D::Error>
where
    D: serde::Deserializer<'de>,
{
    SerialIndexes::deserialize(deserializer).map(ModelIndexes::from_serial)
}

/// Serialization form of the lookup indexes. Serde cannot emit a non-string
/// map key, so the canonical [`ObjectIdentity`] index is stored as a sorted
/// list of `(identity, keys)` pairs; [`ModelIndexes::from_serial`] restores
/// it canonically. Indexes are never part of any semantic comparison —
/// they are fully derivable from the node/edge sets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SerialIndexes {
    artifact_by_key: BTreeMap<String, usize>,
    application_by_id: BTreeMap<String, usize>,
    edges_by_node: BTreeMap<String, Vec<usize>>,
    artifacts_by_object: Vec<(ObjectIdentity, Vec<String>)>,
    artifacts_by_content: BTreeMap<String, Vec<String>>,
    artifacts_by_category: BTreeMap<String, Vec<String>>,
    artifacts_by_application: BTreeMap<String, Vec<String>>,
}

/// Derived lookup indexes. Constructed once, at finalization, from the
/// canonical node/edge sets — so they cannot diverge from them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModelIndexes {
    /// artifact node key → index into `artifacts`.
    pub(crate) artifact_by_key: BTreeMap<String, usize>,
    /// application id → index into `applications`.
    pub(crate) application_by_id: BTreeMap<String, usize>,
    /// node key (artifact or application) → edge indices.
    pub(crate) edges_by_node: BTreeMap<String, Vec<usize>>,
    /// object identity → artifact node keys (aliases share an identity).
    pub(crate) artifacts_by_object: BTreeMap<ObjectIdentity, Vec<String>>,
    /// content digest → artifact node keys.
    pub(crate) artifacts_by_content: BTreeMap<String, Vec<String>>,
    /// classification category code → artifact node keys. Keyed by the
    /// category's STABLE code string (e.g. `"CACHE"`), because the category
    /// enum carries no total order and the code is the contract surface.
    pub(crate) artifacts_by_category: BTreeMap<String, Vec<String>>,
    /// application id → artifact node keys with a credible claim.
    pub(crate) artifacts_by_application: BTreeMap<String, Vec<String>>,
}

impl ModelIndexes {
    fn to_serial(&self) -> SerialIndexes {
        let mut by_object: Vec<(ObjectIdentity, Vec<String>)> = self
            .artifacts_by_object
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        by_object.sort();
        SerialIndexes {
            artifact_by_key: self.artifact_by_key.clone(),
            application_by_id: self.application_by_id.clone(),
            edges_by_node: self.edges_by_node.clone(),
            artifacts_by_object: by_object,
            artifacts_by_content: self.artifacts_by_content.clone(),
            artifacts_by_category: self.artifacts_by_category.clone(),
            artifacts_by_application: self.artifacts_by_application.clone(),
        }
    }

    fn from_serial(value: SerialIndexes) -> Self {
        let mut artifacts_by_object: BTreeMap<ObjectIdentity, Vec<String>> = BTreeMap::new();
        for (identity, mut keys) in value.artifacts_by_object {
            keys.sort();
            keys.dedup();
            artifacts_by_object.insert(identity, keys);
        }
        ModelIndexes {
            artifact_by_key: value.artifact_by_key,
            application_by_id: value.application_by_id,
            edges_by_node: value.edges_by_node,
            artifacts_by_object,
            artifacts_by_content: value.artifacts_by_content,
            artifacts_by_category: value.artifacts_by_category,
            artifacts_by_application: value.artifacts_by_application,
        }
    }
}

/// One finalized-model ingredient bundle. Grouping the eight inputs keeps the
/// [`SystemModel::finalize`] signature under the lint limit without changing
/// any semantics.
pub(crate) struct FinalizeInput {
    pub(crate) artifacts: Vec<ArtifactNode>,
    pub(crate) applications: Vec<ApplicationNode>,
    pub(crate) edges: Vec<SystemEdge>,
    pub(crate) historical_context: Vec<HistoricalContext>,
    pub(crate) insights: Vec<SystemInsight>,
    pub(crate) candidates: Vec<SystemCandidate>,
    pub(crate) observations: ObservationSummary,
    pub(crate) truncation: ModelTruncation,
}

impl SystemModel {
    /// Finalize a model from its one grouped input, building every index.
    /// This is the only constructor; it assumes the caller already
    /// canonicalized and bounded the collections (see [`crate::build`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalize(bundle: FinalizeInput) -> Self {
        let FinalizeInput {
            artifacts,
            applications,
            edges,
            historical_context,
            insights,
            candidates,
            observations,
            truncation,
        } = bundle;
        let mut indexes = ModelIndexes::default();
        for (i, a) in artifacts.iter().enumerate() {
            indexes.artifact_by_key.insert(a.key.clone(), i);
            if let Some(object) = a.identity {
                indexes
                    .artifacts_by_object
                    .entry(object)
                    .or_default()
                    .push(a.key.clone());
            }
            if let Some(content) = &a.content_sha256 {
                indexes
                    .artifacts_by_content
                    .entry(content.clone())
                    .or_default()
                    .push(a.key.clone());
            }
            if let Some(category) = a.category {
                indexes
                    .artifacts_by_category
                    .entry(category.code().to_string())
                    .or_default()
                    .push(a.key.clone());
            }
        }
        for (i, app) in applications.iter().enumerate() {
            indexes.application_by_id.insert(app.id.0.clone(), i);
        }
        for (i, e) in edges.iter().enumerate() {
            indexes
                .edges_by_node
                .entry(e.from.clone())
                .or_default()
                .push(i);
            // A self-loop touches one node once: pushing `i` twice would
            // create a duplicate index entry for a single fact.
            if e.to != e.from {
                indexes
                    .edges_by_node
                    .entry(e.to.clone())
                    .or_default()
                    .push(i);
            }
            // Application → artifact index, for credible application claims.
            // A `SharedBy` edge links two APPLICATIONS; its `to` endpoint is
            // not an artifact and must never enter this index, or the index
            // would claim a non-node exists.
            if indexes.application_by_id.contains_key(&e.from)
                && indexes.artifact_by_key.contains_key(&e.to)
            {
                let is_claim = e.kind.asserts_ownership() || e.kind == SystemEdgeKind::SharedBy;
                if is_claim && e.assessment.is_credible() {
                    indexes
                        .artifacts_by_application
                        .entry(e.from.clone())
                        .or_default()
                        .push(e.to.clone());
                }
            }
        }
        // Index vectors are canonical and duplicate-free by construction;
        // dedup defensively so a caller cannot observe a duplicate entry.
        for v in indexes.artifacts_by_object.values_mut() {
            v.sort();
            v.dedup();
        }
        for v in indexes.artifacts_by_content.values_mut() {
            v.sort();
            v.dedup();
        }
        for v in indexes.artifacts_by_category.values_mut() {
            v.sort();
            v.dedup();
        }
        for v in indexes.artifacts_by_application.values_mut() {
            v.sort();
            v.dedup();
        }
        SystemModel {
            artifacts,
            applications,
            edges,
            historical_context,
            insights,
            candidates,
            observations,
            truncation,
            indexes,
        }
    }

    /// Every artifact node (canonical order).
    pub fn artifact_nodes(&self) -> &[ArtifactNode] {
        &self.artifacts
    }

    /// Look up one artifact by node key.
    pub fn artifact(&self, key: &str) -> Option<&ArtifactNode> {
        self.indexes
            .artifact_by_key
            .get(key)
            .and_then(|i| self.artifacts.get(*i))
    }

    /// Look up one application by id.
    pub fn application(&self, id: &ApplicationId) -> Option<&ApplicationNode> {
        self.indexes
            .application_by_id
            .get(&id.0)
            .and_then(|i| self.applications.get(*i))
    }

    /// Every edge touching `node_key`, in canonical edge order.
    pub fn edges_for_node(&self, node_key: &str) -> Vec<&SystemEdge> {
        let mut out: Vec<&SystemEdge> = self
            .indexes
            .edges_by_node
            .get(node_key)
            .map(|idxs| idxs.iter().filter_map(|i| self.edges.get(*i)).collect())
            .unwrap_or_default();
        out.sort_by_key(|a| a.order_key());
        out
    }

    /// Artifact keys sharing one canonical object identity.
    pub fn artifacts_sharing_object(&self, object: ObjectIdentity) -> Vec<&str> {
        self.indexes
            .artifacts_by_object
            .get(&object)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Artifact keys sharing one verified content digest.
    pub fn artifacts_sharing_content(&self, sha256: &str) -> Vec<&str> {
        self.indexes
            .artifacts_by_content
            .get(sha256)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Artifact keys carrying one classification category.
    pub fn artifacts_of_category(&self, category: Category) -> Vec<&str> {
        self.indexes
            .artifacts_by_category
            .get(category.code())
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Index consistency invariant: every indexed target exists, and every
    /// canonical node is discoverable through its required index. Used by
    /// tests and by [`Self::check_invariants`].
    pub fn indexes_are_consistent(&self) -> bool {
        let node_exists = |k: &str| self.indexes.artifact_by_key.contains_key(k);
        for (object, keys) in &self.indexes.artifacts_by_object {
            for k in keys {
                let Some(artifact) = self
                    .indexes
                    .artifact_by_key
                    .get(k)
                    .and_then(|i| self.artifacts.get(*i))
                else {
                    return false;
                };
                if artifact.identity != Some(*object) {
                    return false;
                }
            }
        }
        for (content, keys) in &self.indexes.artifacts_by_content {
            for k in keys {
                let Some(artifact) = self
                    .indexes
                    .artifact_by_key
                    .get(k)
                    .and_then(|i| self.artifacts.get(*i))
                else {
                    return false;
                };
                if artifact.content_sha256.as_deref() != Some(content.as_str()) {
                    return false;
                }
            }
        }
        for (category, keys) in &self.indexes.artifacts_by_category {
            for k in keys {
                let Some(artifact) = self
                    .indexes
                    .artifact_by_key
                    .get(k)
                    .and_then(|i| self.artifacts.get(*i))
                else {
                    return false;
                };
                if artifact.category.map(|c| c.code()) != Some(category.as_str()) {
                    return false;
                }
            }
        }
        for keys in self.indexes.artifacts_by_application.values() {
            if !keys.iter().all(|k| node_exists(k)) {
                return false;
            }
        }
        for (node, idxs) in &self.indexes.edges_by_node {
            // Every edge-index target must exist and really touch the node.
            if !self.indexes.artifact_by_key.contains_key(node)
                && !self.indexes.application_by_id.contains_key(node)
            {
                return false;
            }
            for i in idxs {
                let Some(edge) = self.edges.get(*i) else {
                    return false;
                };
                if &edge.from != node && &edge.to != node {
                    return false;
                }
            }
        }
        true
    }

    /// Full model self-check: index consistency plus canonical ordering.
    pub fn check_invariants(&self) -> Result<(), String> {
        if !self.indexes_are_consistent() {
            return Err("indexes diverge from the canonical node/edge sets".to_string());
        }
        for w in self.artifacts.windows(2) {
            if w[0].key >= w[1].key {
                return Err(format!("artifacts not canonically ordered: {}", w[0].key));
            }
        }
        for w in self.applications.windows(2) {
            if w[0].id.0 >= w[1].id.0 {
                return Err(format!(
                    "applications not canonically ordered: {}",
                    w[0].id.0
                ));
            }
        }
        for w in self.edges.windows(2) {
            if w[0].order_key() > w[1].order_key() {
                return Err("edges not canonically ordered".to_string());
            }
        }
        for w in self.insights.windows(2) {
            if w[0].id >= w[1].id {
                return Err(format!("insights not canonically ordered: {}", w[0].id));
            }
        }
        Ok(())
    }

    /// Total artifact count.
    pub fn artifact_count(&self) -> usize {
        self.artifacts.len()
    }

    /// Total application count.
    pub fn application_count(&self) -> usize {
        self.applications.len()
    }

    /// Total edge count.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

/// Which node a query result refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeRefKind {
    Artifact,
    Application,
}

/// A typed reference to one node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRef {
    pub kind: NodeRefKind,
    pub key: String,
}

impl NodeRef {
    pub fn artifact(key: impl Into<String>) -> Self {
        NodeRef {
            kind: NodeRefKind::Artifact,
            key: key.into(),
        }
    }

    pub fn application(id: &ApplicationId) -> Self {
        NodeRef {
            kind: NodeRefKind::Application,
            key: id.0.clone(),
        }
    }
}

/// One application's claim on one artifact, as seen by a query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationClaim {
    pub application: ApplicationId,
    pub application_name: String,
    /// Every distinct relationship kind connecting this application to the
    /// artifact (e.g. `OwnedBy` + `ApplicationCache`). One row per app, so
    /// descriptive role edges cannot inflate the claimant count.
    pub edge_kinds: Vec<SystemEdgeKind>,
    pub assessment: OwnershipAssessment,
    pub evidence: Vec<OwnershipEvidence>,
}

/// Convenience: the artifact key for a path (lossless).
pub fn artifact_key_for(path: &Path) -> String {
    ArtifactKey::of(path).to_string()
}

/// Re-exported so downstream code does not re-import the apps crate for the
/// two types the system model's public API exposes directly.
pub use coresight_apps::{ApplicationId as ModelApplicationId, FootprintKind as ArtifactRole};
