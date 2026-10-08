//! Phase 6.3 scenario tests: the seven required fixture graphs, plus the
//! observed/inferred, containment-vs-ownership, and unknown-vs-empty
//! contracts.
//!
//! Portable: runs on every CI platform, touches no real filesystem.

mod fixtures;

use coresight_apps::SourceStatus;
use coresight_capabilities::access::AccessState;
use coresight_classifier::Category;
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    applications_for_artifact, artifact_key_for, artifacts_without_application,
    association_unknown_artifacts, build_system_model, can_authorize_execution,
    candidate_is_authorized, conflicting_ownership, shared_artifacts, ArtifactApplicationStatus,
    InsightKind, ProvenanceState, SystemEdgeKind, SystemModelLimits,
};

use fixtures::*;

fn limits() -> SystemModelLimits {
    SystemModelLimits::default()
}

// ---------------------------------------------------------------------------
// Scenario A — clean application
// ---------------------------------------------------------------------------

#[test]
fn scenario_a_clean_application_resolves_with_all_roles() {
    let root = "/opt/clean";
    let mut fact = app_fact("Clean App", Some("Vendor"), &[root], &[]);
    fact.record.install_location = Some(root.into());
    fact.record.executable_path = Some(format!("{root}/clean").into());
    fact.executable = Some(format!("{root}/clean").into());

    let model = build_system_model(
        &input(
            vec![
                dir(root),
                with_category(
                    artifact(&format!("{root}/clean"), 1, 10, None),
                    Category::Applications,
                ),
                with_category(
                    artifact(&format!("{root}/clean.conf"), 1, 11, None),
                    Category::ApplicationData,
                ),
                with_category(
                    artifact(&format!("{root}/cache"), 1, 12, None),
                    Category::Cache,
                ),
                with_category(
                    artifact(&format!("{root}/app.log"), 1, 13, None),
                    Category::Logs,
                ),
            ],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    assert!(model.check_invariants().is_ok());
    let app = &model.applications()[0];
    assert_eq!(app.name, "Clean App");
    assert!(
        matches!(
            app.state,
            coresight_system_model::ApplicationState::Resolved
                | coresight_system_model::ApplicationState::PartiallyResolved
        ),
        "clean app should resolve: {:?}",
        app.state
    );

    // The recorded executable edge exists and is observed.
    let exe_key = artifact_key_for(std::path::Path::new(&format!("{root}/clean")));
    let edges = model.edges_for_node(&exe_key);
    assert!(edges
        .iter()
        .any(|e| e.kind == SystemEdgeKind::ApplicationExecutable
            && e.provenance == ProvenanceState::Observed));

    // Every artifact carries its classifier category WITHOUT it being
    // overwritten by the application association.
    let cache_key = artifact_key_for(std::path::Path::new(&format!("{root}/cache")));
    assert_eq!(
        model.artifact(&cache_key).unwrap().category,
        Some(Category::Cache)
    );
    let claims = applications_for_artifact(&model, &cache_key, 16);
    assert_eq!(claims.items.len(), 1, "cache is associated with the app");
    assert_eq!(
        model.artifact(&cache_key).unwrap().category,
        Some(Category::Cache)
    );
}

// ---------------------------------------------------------------------------
// Scenario B — shared runtime
// ---------------------------------------------------------------------------

#[test]
fn scenario_b_shared_runtime_is_shared_not_exclusively_owned() {
    let shared = "/shared/runtime.dll";
    // Two applications each hold a CREDIBLE (moderate) claim on one shared
    // runtime: shared, not conflicting, and never exclusively owned.
    let a = app_fact_moderate("App A", &[shared]);
    let b = app_fact_moderate("App B", &[shared]);

    let model = build_system_model(
        &input(
            vec![artifact(shared, 1, 100, None)],
            vec![a, b],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    let key = artifact_key_for(std::path::Path::new(shared));
    let node = model.artifact(&key).unwrap();
    assert_eq!(node.application_status, ArtifactApplicationStatus::Shared);
    assert_eq!(node.credible_claimants, 2);

    // Sharing must never be presented as exclusive ownership.
    assert!(shared_artifacts(&model, 16).items.len() == 1);
    // Both claims are preserved, and each application is counted once.
    let owners = coresight_system_model::owning_applications(&model, &key, 16);
    assert_eq!(owners.items.len(), 2, "both apps own-share it: {owners:?}");
    let claims = applications_for_artifact(&model, &key, 16);
    assert_eq!(claims.items.len(), 2, "one row per app: {claims:?}");
    // And it is NOT reported as an orphan.
    assert!(artifacts_without_application(&model, 16).items.is_empty());
}

// ---------------------------------------------------------------------------
// Scenario C — conflicting metadata
// ---------------------------------------------------------------------------

#[test]
fn scenario_c_conflicting_strong_claims_are_preserved() {
    let contested = "/contested/tool.exe";
    let a = app_fact("App A", Some("Vendor"), &[], &[contested]);
    let b = app_fact("App B", Some("Other"), &[], &[contested]);

    let model = build_system_model(
        &input(
            vec![artifact(contested, 1, 200, None)],
            vec![a, b],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    let key = artifact_key_for(std::path::Path::new(contested));
    let node = model.artifact(&key).unwrap();
    assert_eq!(
        node.application_status,
        ArtifactApplicationStatus::Conflicting
    );
    assert_eq!(conflicting_ownership(&model, 16).items.len(), 1);

    // Nothing silently resolved the conflict to one owner.
    let claims = applications_for_artifact(&model, &key, 16);
    assert_eq!(claims.items.len(), 2, "both owners preserved: {claims:?}");

    // The conflict is visible as an insight needing resolution.
    let conflicts: Vec<_> = model
        .insights()
        .iter()
        .filter(|i| i.kind == InsightKind::ConflictingOwnership)
        .collect();
    assert_eq!(conflicts.len(), 1);
    assert!(conflicts[0]
        .blockers
        .contains(&coresight_system_model::InsightBlocker::ConflictingOwnership));
}

// ---------------------------------------------------------------------------
// Scenario D — duplicate executable by content
// ---------------------------------------------------------------------------

#[test]
fn scenario_d_duplicate_content_is_not_confused_with_hard_link() {
    let digest = "aa11";
    let a_path = "/opt/a/tool";
    let b_path = "/opt/b/tool";
    let model = build_system_model(
        &input(
            vec![
                with_content(artifact(a_path, 1, 300, None), digest),
                with_content(artifact(b_path, 1, 301, None), digest),
            ],
            vec![],
            vec![content_duplicate(&[a_path, b_path], digest)],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    let dupes: Vec<_> = model
        .edges()
        .iter()
        .filter(|e| e.kind == SystemEdgeKind::DuplicateOf)
        .collect();
    assert_eq!(dupes.len(), 1, "one pairwise duplicate edge");
    assert!(
        model
            .edges()
            .iter()
            .all(|e| e.kind != SystemEdgeKind::HardLinkAliasOf),
        "a content duplicate must never be published as a hard-link alias"
    );
    // Distinct objects: different identities on the two nodes.
    let a_key = artifact_key_for(std::path::Path::new(a_path));
    let b_key = artifact_key_for(std::path::Path::new(b_path));
    assert_ne!(
        model.artifact(&a_key).unwrap().identity,
        model.artifact(&b_key).unwrap().identity
    );
    assert!(model
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::DuplicateContent));
}

// ---------------------------------------------------------------------------
// Scenario E — hard-link alias
// ---------------------------------------------------------------------------

#[test]
fn scenario_e_hard_link_alias_shares_one_object() {
    let object = ObjectIdentity {
        volume: 1,
        file_id: 400,
        file_id_hi: Some(9),
    };
    let p1 = "/data/one";
    let p2 = "/data/two";
    let model = build_system_model(
        &input(
            vec![artifact(p1, 1, 400, Some(9)), artifact(p2, 1, 400, Some(9))],
            vec![],
            vec![hard_link_alias(&[p1, p2], object)],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    let aliases: Vec<_> = model
        .edges()
        .iter()
        .filter(|e| e.kind == SystemEdgeKind::HardLinkAliasOf)
        .collect();
    assert_eq!(aliases.len(), 1);
    assert!(
        model
            .edges()
            .iter()
            .all(|e| e.kind != SystemEdgeKind::DuplicateOf),
        "an alias is one object, not a content duplicate"
    );
    let shared = coresight_system_model::aliases_of_object(&model, object);
    assert_eq!(shared.len(), 2, "both paths reach the same object");
}

// ---------------------------------------------------------------------------
// Scenario F — inaccessible data
// ---------------------------------------------------------------------------

#[test]
fn scenario_f_denied_data_is_not_empty_and_is_recorded() {
    let root = "/opt/app";
    let protected = "/opt/app/protected";
    let mut fact = app_fact("Guarded", Some("Vendor"), &[root], &[]);
    fact.record.install_location = Some(root.into());

    let model = build_system_model(
        &input(
            vec![dir(root), denied_artifact(protected)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    let key = artifact_key_for(std::path::Path::new(protected));
    let node = model.artifact(&key).unwrap();
    assert_eq!(node.access, AccessState::ExistsButInaccessible);
    assert_eq!(node.provenance, ProvenanceState::Unavailable);
    // Denied is a counted observation, never silently "empty".
    assert_eq!(model.observations().inaccessible_artifacts, 1);
    assert_ne!(node.provenance, ProvenanceState::Observed);

    // The application state records the inaccessible expected data.
    assert!(model.applications()[0]
        .state_reasons
        .contains(&coresight_system_model::ApplicationStateReason::ExpectedDataInaccessible));
}

// ---------------------------------------------------------------------------
// Scenario G — unsupported source
// ---------------------------------------------------------------------------

#[test]
fn scenario_g_unsupported_source_is_not_an_empty_inventory() {
    let model = build_system_model(
        &input(
            vec![artifact("/some/file", 1, 500, None)],
            vec![],
            vec![],
            vec![],
            unsupported_coverage(),
        ),
        &limits(),
    );

    // The artifact is NOT reported as "no application claims it" — the
    // association is simply not knowable.
    let key = artifact_key_for(std::path::Path::new("/some/file"));
    let node = model.artifact(&key).unwrap();
    assert_eq!(
        node.application_status,
        ArtifactApplicationStatus::AssociationUnsupported
    );
    assert!(!node.application_status.is_genuinely_unassociated());
    assert!(artifacts_without_application(&model, 16).items.is_empty());
    assert_eq!(association_unknown_artifacts(&model, 16).items.len(), 1);

    // And the source status is reported honestly.
    assert_eq!(model.observations().source_states.len(), 1);
    assert_eq!(
        model.observations().source_states[0].status,
        SourceStatus::Unsupported
    );
}

#[test]
fn unavailable_and_failed_sources_are_distinguishable_from_empty() {
    for (coverage, expected) in [
        (
            unavailable_coverage(),
            ArtifactApplicationStatus::AssociationUnavailable,
        ),
        (
            failed_coverage(),
            ArtifactApplicationStatus::AssociationFailed,
        ),
    ] {
        let model = build_system_model(
            &input(
                vec![artifact("/x/y", 1, 1, None)],
                vec![],
                vec![],
                vec![],
                coverage,
            ),
            &limits(),
        );
        let key = artifact_key_for(std::path::Path::new("/x/y"));
        assert_eq!(model.artifact(&key).unwrap().application_status, expected);
        assert!(artifacts_without_application(&model, 16).items.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Containment vs ownership
// ---------------------------------------------------------------------------

#[test]
fn containment_never_becomes_ownership() {
    let inside = "/opt/thing/lib/inner.dll";
    let fact = app_fact_containment("Thing", &[inside]);
    let model = build_system_model(
        &input(
            vec![artifact(inside, 1, 600, None)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );

    let key = artifact_key_for(std::path::Path::new(inside));
    let node = model.artifact(&key).unwrap();
    // Structural evidence cannot produce a credible ownership claim.
    assert_ne!(
        node.application_status,
        ArtifactApplicationStatus::Associated
    );
    assert!(
        model
            .edges()
            .iter()
            .all(|e| e.kind != SystemEdgeKind::OwnedBy),
        "containment-only evidence must never publish an OwnedBy edge"
    );
    let assoc: Vec<_> = model
        .edges()
        .iter()
        .filter(|e| e.kind == SystemEdgeKind::AssociatedWith)
        .collect();
    assert_eq!(assoc.len(), 1);
    assert!(!assoc[0].assessment.is_credible());
}

#[test]
fn contains_edges_are_pure_structure_and_carry_no_ownership() {
    let model = build_system_model(
        &input(
            vec![dir("/root"), artifact("/root/child", 1, 1, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    let contains: Vec<_> = model
        .edges()
        .iter()
        .filter(|e| e.kind == SystemEdgeKind::Contains)
        .collect();
    assert_eq!(contains.len(), 1);
    assert!(!contains[0].kind.asserts_ownership());
    let under: Vec<_> = model
        .edges()
        .iter()
        .filter(|e| e.kind == SystemEdgeKind::LocatedUnder)
        .collect();
    assert_eq!(under.len(), 1, "both directions are published");
}

// ---------------------------------------------------------------------------
// Observed vs inferred survives aggregation
// ---------------------------------------------------------------------------

#[test]
fn observed_and_inferred_remain_distinguishable() {
    let exe = "/opt/app/app.exe";
    let mut fact = app_fact("App", Some("V"), &[], &[exe]);
    fact.record.executable_path = Some(exe.into());
    fact.executable = Some(exe.into());
    let model = build_system_model(
        &input(
            vec![artifact(exe, 1, 700, None)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    let key = artifact_key_for(std::path::Path::new(exe));
    let edges = model.edges_for_node(&key);
    let exact = edges
        .iter()
        .find(|e| e.kind == SystemEdgeKind::ApplicationExecutable)
        .expect("exact executable edge");
    assert_eq!(exact.provenance, ProvenanceState::Observed);
    assert!(exact.kind.is_descriptive_role());
    // The ownership edge derived from it is a SEPARATE, Inferred edge — the
    // observed fact is never conflated with the inferred conclusion.
    let ownership = edges
        .iter()
        .find(|e| e.kind == SystemEdgeKind::OwnedBy)
        .expect("ownership edge");
    assert_eq!(ownership.provenance, ProvenanceState::Inferred);
    assert_ne!(exact.provenance, ownership.provenance);
    // And the observed/inferred distinction is preserved on the model itself.
    assert!(model
        .edges()
        .iter()
        .any(|e| e.provenance == ProvenanceState::Observed));
    assert!(model
        .edges()
        .iter()
        .any(|e| e.provenance == ProvenanceState::Inferred));
}

// ---------------------------------------------------------------------------
// Safety invariant
// ---------------------------------------------------------------------------

#[test]
fn no_model_can_authorize_execution() {
    let model = build_system_model(
        &input(
            vec![artifact("/a", 1, 1, None)],
            vec![app_fact("App", None, &[], &["/a"])],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert!(!can_authorize_execution(&model));
    for c in model.candidates() {
        assert!(!candidate_is_authorized(c));
        assert!(c
            .blockers
            .contains(&coresight_system_model::InsightBlocker::NoExecutorInThisPhase));
    }
}

// ---------------------------------------------------------------------------
// Capability honesty
// ---------------------------------------------------------------------------

#[test]
fn capability_state_is_descriptive_and_never_upgraded() {
    let model = build_system_model(
        &input(vec![], vec![], vec![], vec![], complete_coverage()),
        &limits(),
    );
    let footprint = model
        .observations()
        .capabilities
        .iter()
        .find(|c| c.capability == coresight_capabilities::CapabilityId::ApplicationFootprint)
        .expect("footprint capability is referenced");
    // The real capability is Partial; the model must not report it as done.
    assert_ne!(
        footprint.status,
        coresight_capabilities::CapabilityStatus::Implemented
    );
    assert!(!footprint.blockers.is_empty(), "partial carries blockers");
    // Platform-neutral: shared code must never invent a macOS-specific
    // requirement on any host.
    assert!(
        footprint
            .blockers
            .iter()
            .all(|b| !b.contains("macOS") && !b.contains("Full Disk Access")),
        "no platform-specific blocker from shared code: {footprint:?}"
    );
    let sw = model
        .observations()
        .capabilities
        .iter()
        .find(|c| c.capability == coresight_capabilities::CapabilityId::SoftwareManagement)
        .unwrap();
    assert!(
        sw.blockers.iter().any(|b| b.contains("no execution")),
        "the no-executor boundary is reported"
    );
}
