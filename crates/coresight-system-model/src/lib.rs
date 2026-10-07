//! CoreSight unified **system model** — Phase 6.3.
//!
//! The first coherent, in-memory model of the machine: it joins the facts the
//! other subsystems already established (filesystem observations, canonical
//! object identity, classification, identity relationships, application
//! inventory and ownership evidence, capability state, and history context)
//! into one immutable graph with an explicit, explainable semantics.
//!
//! ```text
//! Observation
//!     ↓
//! Identity / Classification
//!     ↓
//! Application Intelligence
//!     ↓
//! Correlation            ← this crate
//!     ↓
//! System Model
//!     ↓
//! Queries / Insights
//! ```
//!
//! ## Contracts honored here
//!
//! * **Pure correlation.** [`build_system_model`] receives facts and returns a
//!   model. It performs no filesystem access, no subprocess execution, no
//!   network access, and no history inference — it has no capability to do
//!   any of those.
//! * **One object identity.** Artifacts carry the canonical
//!   [`coresight_apps::ObjectIdentity`] `{ volume, file_id, file_id_hi }`
//!   with its wide high bits intact, or `None` when the platform proved none.
//!   There is no second representation and no path-derived pseudo-identity.
//! * **Path is not identity.** Path, object identity, and content identity are
//!   three separate facts on every artifact node.
//! * **Containment is not ownership.** [`SystemEdgeKind`] keeps `Contains`,
//!   `LocatedUnder`, `OwnedBy`, `AssociatedWith`, `SharedBy`, `DuplicateOf`
//!   and `HardLinkAliasOf` distinct.
//! * **Observed ≠ inferred.** Every node and edge carries a
//!   [`ProvenanceState`]; the distinction survives aggregation.
//! * **Traceable confidence.** Every edge and insight carries structured
//!   evidence whose strength was already clamped by the Phase 6.2 correlation
//!   ceilings, so evidence that traveled through several modules cannot
//!   double-count itself into a stronger claim.
//! * **Conflicts are preserved.** Nothing is averaged; a later input never
//!   overwrites an earlier contradictory one.
//! * **Unknown ≠ empty.** `Unsupported`/`Unavailable`/`Failed` sources are
//!   represented on the artifacts they affect, never as an empty result.
//! * **Bounded and deterministic.** Every collection admits through
//!   [`coresight_apps::BoundedTopK`] — O(limit) working memory, exact
//!   overflow accounting, and a result that is a pure function of the input
//!   fact *set*.
//! * **Read-only.** There is no executor, no destructive primitive, no
//!   subprocess, no network, and no persistence anywhere in this crate.
//!
//! See `docs/SYSTEM_MODEL.md` for the full contract.

pub mod build;
pub mod insight;
pub mod model;
pub mod pathkey;

pub use build::{
    build_system_model, ApplicationFact, ArtifactClassification, ArtifactFact, HistoryFact,
    RelationshipFact, RelationshipFactKind, SystemModelInput,
};
pub use insight::{
    aliases_of_object, applications_for_artifact, artifacts_for_application,
    artifacts_of_classification, artifacts_without_application, association_unknown_artifacts,
    conflicting_ownership, insights_of_kind, owning_applications, shared_artifacts,
    strongly_associated_artifacts, unresolved_associations, QueryResult, DEFAULT_QUERY_LIMIT,
};
pub use model::{
    artifact_key_for, ApplicationClaim, ApplicationNode, ApplicationState, ApplicationStateReason,
    ArtifactApplicationStatus, ArtifactNode, CandidateActionKind, CapabilityState, EdgeDomain,
    HistoricalContext, InsightBlocker, InsightKind, InsightSeverity, ModelTruncation, NodeRef,
    NodeRefKind, ObservationSummary, ProvenanceState, SourceStateSummary, SystemCandidate,
    SystemEdge, SystemEdgeKind, SystemInsight, SystemModel, SystemModelLimits, SystemNodeKind,
};
pub use pathkey::ArtifactKey;

/// Re-exported so callers can name the identity and evidence types without
/// importing two crates for one concept.
pub use coresight_apps::{OwnershipAssessment, OwnershipEvidence};
pub use coresight_identity::ObjectIdentity;

/// The safety invariant, as a function: no system model can ever authorize
/// execution. There is no executor in this build, and the model carries no
/// transition into one.
pub fn can_authorize_execution(_model: &SystemModel) -> bool {
    false
}

/// The safety invariant for one candidate: an inert candidate never becomes
/// an authorized action.
pub fn candidate_is_authorized(_candidate: &SystemCandidate) -> bool {
    false
}

#[cfg(test)]
mod architecture_guard_tests {
    use std::fs;
    use std::path::Path;

    /// The system model is SHARED, platform-neutral code. It must contain no
    /// platform conditionals, no subprocess/network capability, no mutating
    /// primitive, and no lossy semantic path conversion.
    ///
    /// The test module scans every `.rs` file, including this one, so every
    /// forbidden pattern must be assembled at runtime (fragments that never
    /// contain a forbidden substring in any combination); otherwise the
    /// guard flags its own source.
    #[test]
    fn system_model_is_platform_neutral_read_only_and_lossless() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let dot = ".";
        let mut forbidden: Vec<String> = Vec::new();
        for word in ["target_os", "windows", "unix"] {
            forbidden.push(["cfg!(", word, ")"].concat());
            forbidden.push(["#[cfg(", word, ")]"].concat());
        }
        for tail in [
            "process",
            "net",
            "remove_file",
            "remove_dir",
            "remove_dir_all",
            "set_permissions",
            "OpenOptions",
            "rusqlite",
        ] {
            forbidden.push(["std::", tail].concat());
        }
        for pair in [["Tcp", "Stream"], ["Udp", "Socket"], ["File", "::create"]] {
            forbidden.push(pair.concat());
        }
        forbidden.push(["Command", "::new"].concat());
        forbidden.push(["to_str()", dot, "unwrap_or_default()"].concat());
        forbidden.push(["to_str()", dot, "unwrap()"].concat());
        let mut checked = 0;
        for entry in fs::read_dir(&src).expect("src tree is readable") {
            let path = entry.expect("src tree is readable").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("source is UTF-8");
            for needle in &forbidden {
                assert!(
                    !text.contains(needle.as_str()),
                    "{needle:?} found in {}",
                    path.display()
                );
            }
            checked += 1;
        }
        assert!(checked >= 4, "expected to scan the crate's source files");
    }
}
