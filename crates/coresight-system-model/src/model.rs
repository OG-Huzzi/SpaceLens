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
//! * **Artifact nodes are path occurrences, never objects.** An artifact
//!   node is ONE observed path, keyed by a lossless path-occurrence key
//!   ([`ArtifactKey`]); the canonical [`ObjectIdentity`] rides along as a
//!   separate fact and its own index. `artifact key != object identity`:
//!   several nodes may share one identity (hard-link aliases), several
//!   nodes may share one digest (content duplicates), and a node may carry
//!   neither. Path, object identity, and content identity are three
//!   different facts and remain separately representable.
//! * **Containment is not ownership.** [`SystemEdgeKind`] keeps `Contains`,
//!   `LocatedUnder`, `OwnedBy`, `AssociatedWith`, `SharedBy`, and the
//!   relationship kinds distinct.
//! * **Observed ≠ inferred.** Every node carries its [`ProvenanceState`].
//! * **Evidence is traceable.** Every edge and insight carries structured
//!   [`coresight_apps::OwnershipEvidence`] items whose strength was already
//!   clamped by the Phase 6.2 correlation ceilings.
//! * **Conflicts are preserved.** Contradictory claims coexist; nothing is
//!   averaged and nothing is overwritten by arrival order.
//! * **Truncation is incomplete knowledge, never absence.** A dropped claim,
//!   edge, or history row is counted exactly in [`ModelTruncation`] and can
//!   never surface as "no claim", "no edge", or "no history".
//!   [`ArtifactApplicationStatus::AssociationTruncated`] exists precisely so
//!   a bound can never manufacture a false [`ArtifactApplicationStatus::Unassociated`].
//! * **Bounded by admission.** Every collection admits through
//!   [`coresight_apps::BoundedTopK`] — O(limit) memory, never
//!   collect-then-truncate.
//! * **Deterministic.** Canonical ordering everywhere; the model is a pure
//!   function of the input fact *set*.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use coresight_apps::{
    ApplicationId, EvidenceKind, EvidenceStrength, OwnershipAssessment, OwnershipEvidence,
};
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
    /// Maximum source-coverage summary rows retained.
    pub max_source_states: usize,
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
            max_source_states: 64,
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
    /// Claim pairs admitted but not retained because the bounded claim store
    /// was full. `#[serde(default)]` keeps older payloads readable.
    #[serde(default)]
    pub claims_truncated: u64,
    /// Install-root groupings admitted but not retained because the bounded
    /// root store was full.
    #[serde(default)]
    pub roots_truncated: u64,
    /// Relationship facts rejected because their proofs contradicted or
    /// exceeded the supported participant bound.
    #[serde(default)]
    pub relationships_rejected: u64,
    /// Source-coverage summary rows omitted at the configured output bound.
    #[serde(default)]
    pub source_states_truncated: u64,
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
/// different fact from one no application claimed — and an artifact whose
/// claims were dropped by a bound is a third fact again
/// ([`Self::AssociationTruncated`]).
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
    /// Claims were observed for this artifact but none survived the model's
    /// edge/claim bounds. **Claim truncated ≠ no claim**: this status is
    /// never genuinely unassociated, never feeds orphan reasoning, and the
    /// exact drop count lives in [`ModelTruncation`].
    AssociationTruncated,
    /// Application discovery was unsupported on this host.
    AssociationUnsupported,
    /// Application discovery was unavailable (no source could be read).
    AssociationUnavailable,
    /// Application discovery was attempted and failed.
    AssociationFailed,
}

impl ArtifactApplicationStatus {
    /// True only when the model genuinely observed no claim AND the
    /// application sources were actually usable AND no bound dropped a
    /// claim. This is the ONLY condition under which "orphan-like"
    /// reasoning is permitted, and even then the model does not call it an
    /// orphan (see [`InsightKind::UnassociatedArtifact`]).
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

    /// True when claims existed but bounds discarded them: incomplete
    /// knowledge, never absence.
    pub fn is_association_truncated(self) -> bool {
        matches!(self, ArtifactApplicationStatus::AssociationTruncated)
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
    /// An exact expected install root or executable resolved, with no known
    /// expected-data gap.
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
    /// Observed artifacts lie under a recorded root whose own node was
    /// not observed: a partial footprint, never "nothing observed".
    DescendantObserved,
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
    /// An install-root path; observed only for an exact recorded location,
    /// otherwise inferred weak scope.
    ApplicationInstallRoot,
    /// An executable path; observed only for an exact recorded path, otherwise
    /// a weak candidate that cannot resolve the application by itself.
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
}

impl SystemEdgeKind {
    /// Which domain produced this edge.
    pub fn domain(self) -> EdgeDomain {
        match self {
            SystemEdgeKind::Contains | SystemEdgeKind::LocatedUnder => EdgeDomain::Filesystem,
            SystemEdgeKind::ApplicationInstallRoot
            | SystemEdgeKind::ApplicationExecutable
            | SystemEdgeKind::SharedBy
            | SystemEdgeKind::OwnedBy
            | SystemEdgeKind::AssociatedWith => EdgeDomain::ApplicationIntelligence,
            // Descriptive roles are chosen from the CLASSIFIER's verdict, so
            // their domain is classification — never rewritten by ownership.
            SystemEdgeKind::ApplicationData
            | SystemEdgeKind::ApplicationCache
            | SystemEdgeKind::ApplicationLog
            | SystemEdgeKind::ApplicationConfig => EdgeDomain::Classification,
            SystemEdgeKind::DuplicateOf | SystemEdgeKind::HardLinkAliasOf => EdgeDomain::Identity,
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

/// What a historical record says about the artifact node it joins to.
///
/// History is node-attached context, never a graph edge: a "move" is an
/// assertion ABOUT one current node ("history proves this path previously
/// referred to a different object"), and a self-loop edge would mislead by
/// suggesting a relationship between two nodes. No history is ever inferred
/// from current state — every assertion quotes a caller-supplied record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HistoricalRelation {
    /// Stored history and the current node prove the SAME object identity.
    SameObjectObserved,
    /// Stored history proves an object identity that differs from the
    /// current node's proven identity: the path previously referred to a
    /// different object.
    ObjectReplaced,
    /// The stored record and/or the current node prove no object identity,
    /// so the relation cannot be established. Recorded honestly instead of
    /// asserting sameness without proof.
    IdentityUnproven,
}

/// One historical record joined to its current artifact node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalAssertion {
    /// The current artifact node this record joins to.
    pub artifact_key: String,
    /// The run the fact came from.
    pub run_id: String,
    pub path: PathBuf,
    /// The object identity the stored record carries (quoted, never
    /// re-derived).
    pub recorded_identity: Option<ObjectIdentity>,
    /// The current node's identity (quoted, never re-derived).
    pub current_identity: Option<ObjectIdentity>,
    /// What the record establishes about the node.
    pub relation: HistoricalRelation,
    /// The classification recorded in that run.
    pub category: Option<String>,
    pub provenance: ProvenanceState,
    /// Structured evidence for the assertion.
    pub evidence: Vec<OwnershipEvidence>,
}

impl HistoricalAssertion {
    /// Canonical ordering key: node, then run, then path bytes.
    pub(crate) fn order_key(&self) -> (&str, &str, Vec<u8>) {
        (
            self.artifact_key.as_str(),
            self.run_id.as_str(),
            self.path.as_os_str().as_encoded_bytes().to_vec(),
        )
    }
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
/// Constructed only by [`crate::build::build_system_model`]; every canonical
/// collection is PRIVATE, so external code can read but never mutate a node
/// set without its indexes. The only construction route
/// ([`SystemModel::finalize`]) builds every index from the canonical sets,
/// and deserialization rebuilds the indexes the same way instead of trusting
/// serialized ones. There is no mutating API of any kind.
///
/// Read-only accessors use the repository's existing names where they exist
/// (`artifact_nodes`, `artifact`, `application`, `edges_for_node`, ...); the
/// remaining collections expose `artifacts()`, `applications()`, `edges()`,
/// `historical_context()`, `historical_assertions()`, `insights()`,
/// `candidates()`, `observations()`, and `truncation()`.
///
/// The wire form carries canonical data only: indexes are never serialized
/// and never accepted from input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemModel {
    /// Artifact nodes, canonically ordered by node key.
    artifacts: Vec<ArtifactNode>,
    /// Application nodes, canonically ordered by application id.
    applications: Vec<ApplicationNode>,
    /// Edges, canonically ordered by (kind rank, from, to).
    edges: Vec<SystemEdge>,
    /// Historical context records, canonically ordered.
    historical_context: Vec<HistoricalContext>,
    /// Node-attached historical assertions, canonically ordered.
    historical_assertions: Vec<HistoricalAssertion>,
    /// Higher-order conclusions, canonically ordered by id.
    insights: Vec<SystemInsight>,
    /// Inert read-only candidates, canonically ordered.
    candidates: Vec<SystemCandidate>,
    /// Honest coverage of the domains this model joined.
    observations: ObservationSummary,
    /// Exact truncation accounting for every applied bound.
    truncation: ModelTruncation,
    /// Derived indexes. Never serialized, never deserialized: rebuilt from
    /// the canonical collections on every construction route.
    #[serde(skip)]
    indexes: ModelIndexes,
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

/// One finalized-model ingredient bundle. Grouping the inputs keeps the
/// [`SystemModel::finalize`] signature under the lint limit without changing
/// any semantics.
pub(crate) struct FinalizeInput {
    pub(crate) artifacts: Vec<ArtifactNode>,
    pub(crate) applications: Vec<ApplicationNode>,
    pub(crate) edges: Vec<SystemEdge>,
    pub(crate) historical_context: Vec<HistoricalContext>,
    pub(crate) historical_assertions: Vec<HistoricalAssertion>,
    pub(crate) insights: Vec<SystemInsight>,
    pub(crate) candidates: Vec<SystemCandidate>,
    pub(crate) observations: ObservationSummary,
    pub(crate) truncation: ModelTruncation,
}

/// Canonical-only wire form: every canonical collection, never the derived
/// indexes. Unknown fields are ignored so older payloads (which carried an
/// `indexes` section) still parse — the indexes are always rebuilt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelWire {
    artifacts: Vec<ArtifactNode>,
    applications: Vec<ApplicationNode>,
    edges: Vec<SystemEdge>,
    #[serde(default)]
    historical_context: Vec<HistoricalContext>,
    #[serde(default)]
    historical_assertions: Vec<HistoricalAssertion>,
    #[serde(default)]
    insights: Vec<SystemInsight>,
    #[serde(default)]
    candidates: Vec<SystemCandidate>,
    observations: ObservationSummary,
    truncation: ModelTruncation,
}

impl<'de> Deserialize<'de> for SystemModel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let wire = ModelWire::deserialize(deserializer)?;
        // Deserialize canonical fields → validate structural invariants →
        // rebuild indexes → produce a finalized model. Serialized indexes
        // are never accepted as authoritative (they are not even read).
        validate_canonical_wire(&wire).map_err(D::Error::custom)?;
        let model = SystemModel::finalize(FinalizeInput {
            artifacts: wire.artifacts,
            applications: wire.applications,
            edges: wire.edges,
            historical_context: wire.historical_context,
            historical_assertions: wire.historical_assertions,
            insights: wire.insights,
            candidates: wire.candidates,
            observations: wire.observations,
            truncation: wire.truncation,
        });
        model
            .check_invariants()
            .map_err(|e| D::Error::custom(format!("invalid system model: {e}")))?;
        Ok(model)
    }
}

/// Structural validation applied to canonical wire data before the indexes
/// are rebuilt: canonical ordering, key uniqueness, and endpoint existence.
/// Anything failing here is rejected rather than rebuilt.
fn validate_canonical_wire(wire: &ModelWire) -> Result<(), String> {
    for w in wire.artifacts.windows(2) {
        if w[0].key >= w[1].key {
            return Err(format!(
                "artifacts not canonically ordered or duplicate key: {}",
                w[0].key
            ));
        }
        if w[0].key.is_empty() {
            return Err("artifact with an empty node key".to_string());
        }
    }
    if wire.artifacts.last().is_some_and(|a| a.key.is_empty()) {
        return Err("artifact with an empty node key".to_string());
    }
    for artifact in &wire.artifacts {
        if ArtifactKey::of(&artifact.path).as_str() != artifact.key {
            return Err(format!(
                "artifact key does not encode its path: {}",
                artifact.key
            ));
        }
    }
    for w in wire.applications.windows(2) {
        if w[0].id.0 >= w[1].id.0 {
            return Err(format!(
                "applications not canonically ordered or duplicate id: {}",
                w[0].id.0
            ));
        }
    }
    for w in wire.edges.windows(2) {
        if w[0].order_key() >= w[1].order_key() {
            return Err("edges not uniquely canonically ordered".to_string());
        }
    }
    // NOTE: `>` (not `>=`) is deliberate: conflicting history rows share
    // one (run, path) key by design (conflict preservation), and insights
    // legitimately share ... no — insight ids are unique. History rows and
    // assertions may repeat a key; artifacts, applications, insights, and
    // candidates may not.
    for w in wire.historical_context.windows(2) {
        if historical_context_key(&w[0]) > historical_context_key(&w[1]) {
            return Err("historical context not canonically ordered".to_string());
        }
    }
    for w in wire.historical_assertions.windows(2) {
        if w[0].order_key() > w[1].order_key() {
            return Err("historical assertions not canonically ordered".to_string());
        }
    }
    for w in wire.insights.windows(2) {
        if w[0].id >= w[1].id {
            return Err(format!("insights not canonically ordered: {}", w[0].id));
        }
        if w[0].id.is_empty() {
            return Err("insight with an empty id".to_string());
        }
    }
    if wire.insights.last().is_some_and(|i| i.id.is_empty()) {
        return Err("insight with an empty id".to_string());
    }
    for w in wire.candidates.windows(2) {
        if candidate_order_key(&w[0]) >= candidate_order_key(&w[1]) {
            return Err("candidates not canonically ordered".to_string());
        }
    }
    // Every edge endpoint must resolve to a node carried by this payload.
    // Canonical ordering was checked above, so binary search validates
    // endpoints in O(log N) rather than rescanning all artifacts per edge.
    let artifact = |k: &str| {
        wire.artifacts
            .binary_search_by(|a| a.key.as_str().cmp(k))
            .is_ok()
    };
    let application = |k: &str| {
        wire.applications
            .binary_search_by(|a| a.id.0.as_str().cmp(k))
            .is_ok()
    };
    let node_exists = |k: &str| artifact(k) || application(k);
    for e in &wire.edges {
        if e.from.is_empty() || e.to.is_empty() {
            return Err("edge with an empty endpoint".to_string());
        }
        if !node_exists(&e.from) {
            return Err(format!("edge from unknown node: {}", e.from));
        }
        if !node_exists(&e.to) {
            return Err(format!("edge to unknown node: {}", e.to));
        }
    }
    for a in &wire.historical_assertions {
        if !artifact(&a.artifact_key) {
            return Err(format!(
                "historical assertion for unknown artifact: {}",
                a.artifact_key
            ));
        }
    }
    Ok(())
}

/// Canonical ordering key of one historical-context record.
fn historical_context_key(ctx: &HistoricalContext) -> (&str, Vec<u8>) {
    (
        ctx.run_id.as_str(),
        ctx.path.as_os_str().as_encoded_bytes().to_vec(),
    )
}

/// Canonical ordering key of one candidate.
fn candidate_order_key(c: &SystemCandidate) -> (&str, CandidateActionKind) {
    (c.target.as_str(), c.action_kind)
}

impl SystemModel {
    /// Finalize a model from its one grouped input, building every index.
    /// This is the only constructor; it assumes the caller already
    /// canonicalized and bounded the collections (see [`crate::build`]).
    /// Deserialization funnels through here too, after structural
    /// validation — so indexes always derive from canonical data and can
    /// never be stale or poisoned.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalize(bundle: FinalizeInput) -> Self {
        let FinalizeInput {
            artifacts,
            applications,
            edges,
            historical_context,
            historical_assertions,
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
            historical_assertions,
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

    /// Every artifact node (canonical order). Alias of [`Self::artifact_nodes`].
    pub fn artifacts(&self) -> &[ArtifactNode] {
        &self.artifacts
    }

    /// Every application node (canonical order).
    pub fn applications(&self) -> &[ApplicationNode] {
        &self.applications
    }

    /// Every edge (canonical order).
    pub fn edges(&self) -> &[SystemEdge] {
        &self.edges
    }

    /// Every historical-context record (canonical order).
    pub fn historical_context(&self) -> &[HistoricalContext] {
        &self.historical_context
    }

    /// Every node-attached historical assertion (canonical order).
    pub fn historical_assertions(&self) -> &[HistoricalAssertion] {
        &self.historical_assertions
    }

    /// Every insight (canonical order).
    pub fn insights(&self) -> &[SystemInsight] {
        &self.insights
    }

    /// Every inert candidate (canonical order).
    pub fn candidates(&self) -> &[SystemCandidate] {
        &self.candidates
    }

    /// Honest coverage of the domains this model joined.
    pub fn observations(&self) -> &ObservationSummary {
        &self.observations
    }

    /// Exact truncation accounting for every applied bound.
    pub fn truncation(&self) -> &ModelTruncation {
        &self.truncation
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

    /// Full model self-check: genuine bidirectional index consistency plus
    /// canonical ordering. Every direction is proven both ways:
    ///
    /// ```text
    /// canonical → index AND index → canonical
    /// ```
    ///
    /// for artifacts, applications, edges, and every secondary index
    /// (object, content, category, application). Duplicate keys, dangling
    /// positions, impossible node references, and unordered collections all
    /// fail.
    pub fn check_invariants(&self) -> Result<(), String> {
        // ---- Canonical → index: every canonical member is indexed. -----
        for (i, a) in self.artifacts.iter().enumerate() {
            if ArtifactKey::of(&a.path).as_str() != a.key {
                return Err(format!("artifact key does not encode its path: {}", a.key));
            }
            match self.indexes.artifact_by_key.get(&a.key) {
                Some(&pos) if pos == i => {}
                Some(&pos) => {
                    return Err(format!(
                        "artifact index position {pos} does not point at canonical position {i} ({})",
                        a.key
                    ));
                }
                None => {
                    return Err(format!("canonical artifact missing from index: {}", a.key));
                }
            }
        }
        for (i, app) in self.applications.iter().enumerate() {
            match self.indexes.application_by_id.get(&app.id.0) {
                Some(&pos) if pos == i => {}
                Some(&pos) => {
                    return Err(format!(
                        "application index position {pos} does not point at canonical position {i} ({})",
                        app.id.0
                    ));
                }
                None => {
                    return Err(format!(
                        "canonical application missing from index: {}",
                        app.id.0
                    ));
                }
            }
        }
        // Edge facts retain their source-domain and confidence semantics.
        // These checks also stop a canonical payload from relabeling an
        // inferred claim as observed (or a descriptive edge as ownership).
        for (i, edge) in self.edges.iter().enumerate() {
            if edge.domain != edge.kind.domain() {
                return Err(format!("edge {i} domain disagrees with its kind"));
            }
            let from_is_artifact = self.artifact(&edge.from).is_some();
            let to_is_artifact = self.artifact(&edge.to).is_some();
            let from_is_application = self.application_by_id(&edge.from).is_some();
            let to_is_application = self.application_by_id(&edge.to).is_some();
            let endpoint_types_valid = match edge.kind {
                SystemEdgeKind::Contains
                | SystemEdgeKind::LocatedUnder
                | SystemEdgeKind::DuplicateOf
                | SystemEdgeKind::HardLinkAliasOf => from_is_artifact && to_is_artifact,
                SystemEdgeKind::ApplicationInstallRoot
                | SystemEdgeKind::ApplicationExecutable
                | SystemEdgeKind::ApplicationData
                | SystemEdgeKind::ApplicationCache
                | SystemEdgeKind::ApplicationLog
                | SystemEdgeKind::ApplicationConfig
                | SystemEdgeKind::OwnedBy
                | SystemEdgeKind::AssociatedWith => from_is_application && to_is_artifact,
                SystemEdgeKind::SharedBy => from_is_application && to_is_application,
            };
            if !endpoint_types_valid {
                return Err(format!(
                    "edge {i} endpoints have types incompatible with its kind"
                ));
            }
            if matches!(
                edge.kind,
                SystemEdgeKind::SharedBy
                    | SystemEdgeKind::DuplicateOf
                    | SystemEdgeKind::HardLinkAliasOf
            ) && edge.from >= edge.to
            {
                return Err(format!(
                    "symmetric edge {i} is self-linked or not canonically oriented"
                ));
            }
            let provenance_valid = match edge.kind {
                SystemEdgeKind::ApplicationInstallRoot | SystemEdgeKind::ApplicationExecutable => {
                    matches!(
                        edge.provenance,
                        ProvenanceState::Observed
                            | ProvenanceState::Inferred
                            | ProvenanceState::Candidate
                    )
                }
                SystemEdgeKind::Contains
                | SystemEdgeKind::LocatedUnder
                | SystemEdgeKind::DuplicateOf
                | SystemEdgeKind::HardLinkAliasOf => edge.provenance == ProvenanceState::Observed,
                SystemEdgeKind::ApplicationData
                | SystemEdgeKind::ApplicationCache
                | SystemEdgeKind::ApplicationLog
                | SystemEdgeKind::ApplicationConfig
                | SystemEdgeKind::OwnedBy
                | SystemEdgeKind::AssociatedWith
                | SystemEdgeKind::SharedBy => edge.provenance == ProvenanceState::Inferred,
            };
            if !provenance_valid {
                return Err(format!("edge {i} provenance disagrees with its kind"));
            }
            let role_evidence_points_to_target = edge.evidence.iter().all(|e| {
                ArtifactKey::of(&e.observed_path).to_string() == edge.to
                    && e.matched_path
                        .as_ref()
                        .is_some_and(|path| ArtifactKey::of(path).to_string() == edge.to)
            });
            let role_semantics_valid = match (edge.kind, edge.provenance) {
                (SystemEdgeKind::ApplicationInstallRoot, ProvenanceState::Observed) => {
                    edge.assessment == OwnershipAssessment::Direct
                        && role_evidence_points_to_target
                        && edge.evidence.iter().any(|e| {
                            e.kind == EvidenceKind::InstallLocation
                                && e.strength == EvidenceStrength::Direct
                        })
                }
                (SystemEdgeKind::ApplicationInstallRoot, ProvenanceState::Inferred) => {
                    edge.assessment == OwnershipAssessment::Weak
                        && role_evidence_points_to_target
                        && edge.evidence.iter().any(|e| {
                            e.kind == EvidenceKind::InstallRootContainment
                                && e.strength <= EvidenceStrength::Weak
                        })
                }
                (SystemEdgeKind::ApplicationInstallRoot, _) => false,
                (SystemEdgeKind::ApplicationExecutable, ProvenanceState::Observed) => {
                    edge.assessment == OwnershipAssessment::Strong
                        && role_evidence_points_to_target
                        && edge.evidence.iter().any(|e| {
                            e.kind == EvidenceKind::ExactExecutablePath
                                && e.strength == EvidenceStrength::Strong
                        })
                }
                (SystemEdgeKind::ApplicationExecutable, ProvenanceState::Candidate) => {
                    edge.assessment == OwnershipAssessment::Weak
                        && role_evidence_points_to_target
                        && edge.evidence.iter().any(|e| {
                            e.kind == EvidenceKind::FilenameSimilarity
                                && e.strength == EvidenceStrength::Weak
                        })
                }
                (SystemEdgeKind::ApplicationExecutable, _) => false,
                _ => true,
            };
            if !role_semantics_valid {
                return Err(format!("edge {i} role confidence exceeds its evidence"));
            }
            if edge.evidence.iter().any(|e| {
                e.strength > e.kind.max_strength() || e.strength > e.correlation_group.ceiling()
            }) {
                return Err(format!("edge {i} evidence exceeds a kind/group ceiling"));
            }
            if edge.evidence.windows(2).any(|w| w[0] >= w[1]) {
                return Err(format!("edge {i} evidence is not unique and ordered"));
            }
            match edge.kind {
                SystemEdgeKind::Contains | SystemEdgeKind::LocatedUnder
                    if edge.assessment != OwnershipAssessment::Direct =>
                {
                    return Err(format!("filesystem edge {i} is not a direct observation"));
                }
                SystemEdgeKind::OwnedBy if !edge.assessment.is_credible() => {
                    return Err(format!("OwnedBy edge {i} is not credible"));
                }
                SystemEdgeKind::AssociatedWith if edge.assessment.is_credible() => {
                    return Err(format!("AssociatedWith edge {i} is credible"));
                }
                SystemEdgeKind::SharedBy
                    if edge.assessment != OwnershipAssessment::Moderate
                        || !edge.evidence.is_empty() =>
                {
                    return Err(format!(
                        "SharedBy edge {i} has invalid assessment or evidence"
                    ));
                }
                SystemEdgeKind::DuplicateOf if edge.assessment != OwnershipAssessment::Strong => {
                    return Err(format!("DuplicateOf edge {i} is not strongly proven"));
                }
                SystemEdgeKind::HardLinkAliasOf
                    if edge.assessment != OwnershipAssessment::Direct =>
                {
                    return Err(format!("HardLinkAliasOf edge {i} is not directly proven"));
                }
                _ => {}
            }
        }
        // Every edge is indexed by EVERY endpoint it touches.
        for (i, e) in self.edges.iter().enumerate() {
            for endpoint in [&e.from, &e.to] {
                let Some(idxs) = self.indexes.edges_by_node.get(endpoint) else {
                    return Err(format!("edge {i} endpoint not indexed: {endpoint}"));
                };
                if idxs.binary_search(&i).is_err() {
                    return Err(format!("edge {i} missing from endpoint index: {endpoint}"));
                }
            }
        }
        // ---- Index → canonical: every index entry resolves exactly. -----
        if self.indexes.artifact_by_key.len() != self.artifacts.len() {
            return Err("artifact index size differs from canonical artifact count".to_string());
        }
        for (key, pos) in &self.indexes.artifact_by_key {
            let Some(node) = self.artifacts.get(*pos) else {
                return Err(format!("artifact index position {pos} dangles ({key})"));
            };
            if &node.key != key {
                return Err(format!(
                    "artifact index position {pos} points at {} instead of {key}",
                    node.key
                ));
            }
        }
        if self.indexes.application_by_id.len() != self.applications.len() {
            return Err(
                "application index size differs from canonical application count".to_string(),
            );
        }
        for (id, pos) in &self.indexes.application_by_id {
            let Some(node) = self.applications.get(*pos) else {
                return Err(format!("application index position {pos} dangles ({id})"));
            };
            if &node.id.0 != id {
                return Err(format!(
                    "application index position {pos} points at {} instead of {id}",
                    node.id.0
                ));
            }
        }
        // Object index: exact membership match, no duplicates.
        let mut object_seen: BTreeMap<ObjectIdentity, usize> = BTreeMap::new();
        for a in &self.artifacts {
            if let Some(object) = a.identity {
                *object_seen.entry(object).or_default() += 1;
            }
        }
        if self.indexes.artifacts_by_object.len() != object_seen.len() {
            return Err("object index covers a different identity set".to_string());
        }
        for (object, keys) in &self.indexes.artifacts_by_object {
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();
            if *keys != sorted {
                return Err("object index membership is not unique and ordered".to_string());
            }
            for k in keys {
                let Some(node) = self.artifact(k) else {
                    return Err(format!("object index member is not a node: {k}"));
                };
                if node.identity != Some(*object) {
                    return Err(format!("object index member has a different identity: {k}"));
                }
            }
            if keys.len() != object_seen.get(object).copied().unwrap_or(0) {
                return Err(
                    "object index membership does not match canonical identities".to_string(),
                );
            }
        }
        // Content index: exact membership match, no duplicates.
        let mut content_seen: BTreeMap<&str, usize> = BTreeMap::new();
        for a in &self.artifacts {
            if let Some(content) = a.content_sha256.as_deref() {
                *content_seen.entry(content).or_default() += 1;
            }
        }
        if self.indexes.artifacts_by_content.len() != content_seen.len() {
            return Err("content index covers a different digest set".to_string());
        }
        for (content, keys) in &self.indexes.artifacts_by_content {
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();
            if *keys != sorted {
                return Err("content index membership is not unique and ordered".to_string());
            }
            for k in keys {
                let Some(node) = self.artifact(k) else {
                    return Err(format!("content index member is not a node: {k}"));
                };
                if node.content_sha256.as_deref() != Some(content.as_str()) {
                    return Err(format!("content index member has a different digest: {k}"));
                }
            }
            if keys.len() != content_seen.get(content.as_str()).copied().unwrap_or(0) {
                return Err("content index membership does not match canonical digests".to_string());
            }
        }
        // Category index: exact membership match, no duplicates.
        let mut category_seen: BTreeMap<&str, usize> = BTreeMap::new();
        for a in &self.artifacts {
            if let Some(category) = a.category {
                *category_seen.entry(category.code()).or_default() += 1;
            }
        }
        if self.indexes.artifacts_by_category.len() != category_seen.len() {
            return Err("category index covers a different category set".to_string());
        }
        for (category, keys) in &self.indexes.artifacts_by_category {
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();
            if *keys != sorted {
                return Err("category index membership is not unique and ordered".to_string());
            }
            for k in keys {
                let Some(node) = self.artifact(k) else {
                    return Err(format!("category index member is not a node: {k}"));
                };
                if node.category.map(|c| c.code()) != Some(category.as_str()) {
                    return Err(format!(
                        "category index member has a different category: {k}"
                    ));
                }
            }
            if keys.len() != category_seen.get(category.as_str()).copied().unwrap_or(0) {
                return Err(
                    "category index membership does not match canonical categories".to_string(),
                );
            }
        }
        // Application index: exact match against the credible claim edges —
        // no more, no fewer, no duplicates, every member a real artifact.
        // NOTE: a `SharedBy` edge links two APPLICATIONS (its `to` endpoint
        // is an application, not an artifact), so it is checked for endpoint
        // existence but excluded from the artifact-membership comparison.
        let mut expected_by_app: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for e in &self.edges {
            let is_claim = (e.kind.asserts_ownership() || e.kind == SystemEdgeKind::SharedBy)
                && e.assessment.is_credible();
            if !is_claim {
                continue;
            }
            if self.application_by_id(&e.from).is_none() {
                return Err(format!("claim edge from unknown application: {}", e.from));
            }
            if e.kind == SystemEdgeKind::SharedBy {
                // App↔app sharing fact: both endpoints must be applications.
                if self.application_by_id(&e.to).is_none() {
                    return Err(format!("SharedBy edge to non-application: {}", e.to));
                }
                continue;
            }
            if self.artifact(&e.to).is_none() {
                return Err(format!(
                    "claim edge references a non-node: {} → {}",
                    e.from, e.to
                ));
            }
            expected_by_app
                .entry(e.from.as_str())
                .or_default()
                .push(e.to.as_str());
        }
        for members in expected_by_app.values_mut() {
            members.sort();
            members.dedup();
        }
        if self.indexes.artifacts_by_application.len() != expected_by_app.len() {
            return Err("application index covers a different application set".to_string());
        }
        for (app, keys) in &self.indexes.artifacts_by_application {
            if self.application_by_id(app).is_none() {
                return Err(format!(
                    "application index key is not an application: {app}"
                ));
            }
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();
            if *keys != sorted {
                return Err("application index membership is not unique and ordered".to_string());
            }
            for k in keys {
                if self.artifact(k).is_none() {
                    return Err(format!("application index member is not an artifact: {k}"));
                }
            }
            let expected = expected_by_app
                .get(app.as_str())
                .cloned()
                .unwrap_or_default();
            let actual: Vec<&str> = keys.iter().map(String::as_str).collect();
            if actual != expected {
                return Err(format!(
                    "application index membership does not match credible claim edges: {app}"
                ));
            }
        }
        // Edge index: every entry touches a real node and its edge; no
        // dangling positions, no duplicates.
        for (node, idxs) in &self.indexes.edges_by_node {
            if idxs.is_empty() {
                return Err(format!("edge index contains an empty entry: {node}"));
            }
            if self.artifact(node).is_none() && self.application_by_id(node).is_none() {
                return Err(format!("edge index key is not a node: {node}"));
            }
            let mut sorted = idxs.clone();
            sorted.sort();
            sorted.dedup();
            if *idxs != sorted {
                return Err(format!("edge index for {node} is not unique and ordered"));
            }
            for i in idxs {
                let Some(edge) = self.edges.get(*i) else {
                    return Err(format!("edge index position {i} dangles ({node})"));
                };
                if &edge.from != node && &edge.to != node {
                    return Err(format!("edge {i} does not touch its index node {node}"));
                }
            }
        }
        // ---- Canonical ordering for every ordered collection. -----------
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
            if w[0].order_key() >= w[1].order_key() {
                return Err("edges not uniquely canonically ordered".to_string());
            }
        }
        // History rows and assertions sort non-decreasing (`>` rejects):
        // conflicting rows legitimately share one (run, path) key.
        for w in self.historical_context.windows(2) {
            if historical_context_key(&w[0]) > historical_context_key(&w[1]) {
                return Err("historical context not canonically ordered".to_string());
            }
        }
        for w in self.historical_assertions.windows(2) {
            if w[0].order_key() > w[1].order_key() {
                return Err("historical assertions not canonically ordered".to_string());
            }
        }
        for w in self.insights.windows(2) {
            if w[0].id >= w[1].id {
                return Err(format!("insights not canonically ordered: {}", w[0].id));
            }
        }
        for w in self.candidates.windows(2) {
            if candidate_order_key(&w[0]) >= candidate_order_key(&w[1]) {
                return Err("candidates not canonically ordered".to_string());
            }
        }
        for w in self.observations.source_states.windows(2) {
            let previous = (&w[0].source, w[0].status, &w[0].note);
            let next = (&w[1].source, w[1].status, &w[1].note);
            if previous >= next {
                return Err("source states are not unique and canonically ordered".to_string());
            }
        }
        for candidate in &self.candidates {
            let Some(target) = self.artifact(&candidate.target) else {
                return Err(format!(
                    "candidate target is not an artifact: {}",
                    candidate.target
                ));
            };
            if target.path != candidate.path {
                return Err(format!(
                    "candidate path disagrees with target: {}",
                    candidate.target
                ));
            }
            if candidate.effect != coresight_capabilities::ActionClass::Destructive {
                return Err(format!(
                    "candidate {} has a non-destructive effect label",
                    candidate.target
                ));
            }
            let expected_confidence = candidate
                .assessment
                .strength()
                .map(|strength| strength.to_confidence())
                .unwrap_or(coresight_apps::Confidence::Unknown);
            if candidate.confidence != expected_confidence {
                return Err(format!(
                    "candidate confidence disagrees with its assessment: {}",
                    candidate.target
                ));
            }
            if !candidate
                .blockers
                .contains(&InsightBlocker::NoExecutorInThisPhase)
            {
                return Err(format!(
                    "candidate {} is missing NoExecutorInThisPhase",
                    candidate.target
                ));
            }
            if candidate.blockers.windows(2).any(|w| w[0] >= w[1]) {
                return Err(format!(
                    "candidate blockers are not unique and ordered: {}",
                    candidate.target
                ));
            }
            if candidate.evidence.windows(2).any(|w| w[0] >= w[1]) {
                return Err(format!(
                    "candidate evidence is not unique and ordered: {}",
                    candidate.target
                ));
            }
        }
        Ok(())
    }

    /// Application id → canonical position. The private counterpart of the
    /// public [`Self::application`] lookup, for invariant checking.
    fn application_by_id(&self, id: &str) -> Option<usize> {
        self.indexes.application_by_id.get(id).copied()
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
