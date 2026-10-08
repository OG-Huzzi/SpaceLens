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
//! * **Pure correlation.** [`build_system_model`] receives already-projected
//!   facts and returns a model. The implementation exposes no prober/provider
//!   input and performs no filesystem access, subprocess execution, network
//!   access, persistence, or history inference.
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
    HistoricalAssertion, HistoricalContext, HistoricalRelation, InsightBlocker, InsightKind,
    InsightSeverity, ModelTruncation, NodeRef, NodeRefKind, ObservationSummary, ProvenanceState,
    SourceStateSummary, SystemCandidate, SystemEdge, SystemEdgeKind, SystemInsight, SystemModel,
    SystemModelLimits, SystemNodeKind,
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
    /// primitive, and no lossy semantic path conversion. Canonical storage
    /// is private with read-only accessors; serialized indexes are never
    /// trusted (they are not even read).
    ///
    /// The test module scans every `.rs` file, including this one, so every
    /// forbidden pattern must be assembled at runtime (fragments that never
    /// contain a forbidden substring in any combination); otherwise the
    /// guard flags its own source. Comment lines are stripped before
    /// matching so prose about a forbidden pattern does not trip the guard.
    #[test]
    fn system_model_manifest_has_no_persistence_or_network_clients() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let text = fs::read_to_string(manifest)
            .expect("manifest is readable")
            .to_ascii_lowercase();
        for parts in [
            ["req", "west"],
            ["hy", "per"],
            ["ur", "eq"],
            ["is", "ahc"],
            ["su", "rf"],
            ["sql", "x"],
            ["ru", "sqlite"],
            ["dies", "el"],
            ["cu", "rl"],
            ["to", "kio"],
            ["aw", "c"],
        ] {
            let dependency = parts.concat();
            assert!(
                !text.contains(&dependency),
                "forbidden network/persistence dependency: {dependency}"
            );
        }
    }

    #[test]
    fn system_model_is_platform_neutral_read_only_and_lossless() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let dot = ".";
        let mut forbidden: Vec<String> = Vec::new();
        for word in ["target_os", "windows", "unix"] {
            forbidden.push(["cfg!(", word, ")"].concat());
            forbidden.push(["#[cfg(", word, ")]"].concat());
        }
        forbidden.push(["cfg!(", "target_os"].concat());
        forbidden.push(["#[cfg(", "target_os"].concat());
        forbidden.push(["std::", "os::"].concat());
        forbidden.push(["std::io::", "Write"].concat());
        forbidden.push(["std::", "fs::"].concat());
        forbidden.push(["std::", "env::"].concat());
        forbidden.push(["std::", "time::"].concat());
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
        for pair in [
            ["Tcp", "Stream"],
            ["Udp", "Socket"],
            ["File", "::create"],
            ["Command", "::new"],
            ["Command", "::spawn"],
            ["Child", "::kill"],
            ["std::fs::", "write"],
            ["fs::", "write"],
            [".", "write_all"],
            ["Reg", "SetValue"],
            ["Reg", "DeleteKey"],
            ["Reg", "DeleteValue"],
            ["Reg", "CreateKey"],
            ["PlatformPath", "Prober"],
            ["Platform", "Fs"],
            ["Path", "Prober"],
            ["Win32Uninstall", "Enumerator"],
            ["Win32Registry", "View"],
            ["PackagedApp", "Provider"],
            ["Registry", "View"],
            ["discover_foot", "prints"],
            [".", "exists("],
            [".", "try_exists("],
            [".", "is_file("],
            [".", "is_dir("],
            [".", "metadata("],
            [".", "symlink_metadata("],
            [".", "canonicalize("],
            [".", "read_dir("],
            ["Application", "Provider"],
            ["req", "west"],
            ["hy", "per"],
            ["ur", "eq"],
            ["is", "ahc"],
            ["su", "rf"],
            ["sql", "x"],
            ["dies", "el"],
        ] {
            forbidden.push(pair.concat());
        }
        forbidden.push(["to_str()", dot, "unwrap_or_default()"].concat());
        forbidden.push(["to_str()", dot, "unwrap()"].concat());
        // Assembled so this source never contains them contiguously.
        forbidden.push(["to_string", "_lossy"].concat());
        forbidden.push(["from_utf8", "_lossy"].concat());
        forbidden.push([".", "to_str", "()"].concat());
        forbidden.push(["_", "_mut", "artifacts"].concat());
        let mut checked = 0;
        for entry in fs::read_dir(&src).expect("src tree is readable") {
            let path = entry.expect("src tree is readable").path();
            if path.extension().is_none_or(|e| {
                e.as_encoded_bytes().len() != 2
                    || e.as_encoded_bytes()[0] != b'r'
                    || e.as_encoded_bytes()[1] != b's'
            }) {
                continue;
            }
            let text = fs::read_to_string(&path).expect("source is UTF-8");
            // Unit-test modules may legitimately inspect source files to
            // enforce this very guard; scan only production code to avoid
            // recursively matching the scanner's own filesystem operations.
            let production = text.split("#[cfg(test)]").next().unwrap_or(&text);
            let code: Vec<&str> = production
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect();
            for needle in &forbidden {
                assert!(
                    !code.iter().any(|l| l.contains(needle.as_str())),
                    "{needle:?} found in {}",
                    path.display()
                );
            }
            checked += 1;
        }
        assert!(checked >= 4, "expected to scan the crate's source files");
    }
}
