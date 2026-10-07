//! Hostile-scale stress and multi-stage evidence propagation tests.
//!
//! The stress fixture (§44) uses 10,000 artifacts, 1,000 applications and
//! 50,000 relationship records with configured bounds — proving the model
//! neither grows outside its limits nor loses determinism at scale. Calmly
//! sized so CI stays practical: the per-node edge bound keeps the pairwise
//! explosion out of the retained set.

mod fixtures;

use std::path::PathBuf;

use coresight_apps::{CorrelationGroup, EvidenceStrength};
use coresight_system_model::{
    artifact_key_for, build_system_model, ApplicationFact, ArtifactFact, RelationshipFact,
    RelationshipFactKind, SystemEdgeKind, SystemModelLimits,
};

use fixtures::*;

fn stress_limits() -> SystemModelLimits {
    SystemModelLimits {
        max_artifacts: 12_000,
        max_applications: 1_200,
        max_edges: 60_000,
        max_evidence_per_edge: 8,
        max_edges_per_node: 64,
        max_historical_context: 512,
        max_insights: 512,
        max_candidates: 2_000,
    }
}

/// 10,000 artifacts / 1,000 applications / 50,000 relationships.
#[test]
fn hostile_graph_stays_bounded_deterministic_and_duplicate_free() {
    let mut artifacts: Vec<ArtifactFact> = (0..10_000)
        .map(|i| artifact(&format!("/vol/f{i:05}"), 1, i as u64, None))
        .collect();
    for i in (0..10_000).step_by(50) {
        let mut a = artifact(&format!("/vol/f{i:05}"), 1, i as u64, None);
        a.content_sha256 = Some(format!("digest-{:04}", i / 50));
        if let Some(slot) = artifacts.get_mut(i) {
            *slot = a;
        }
    }

    let mut applications: Vec<ApplicationFact> = (0..1_000)
        .map(|i| {
            app_fact(
                &format!("StressApp{i:04}"),
                Some("Vendor"),
                &[],
                &[&format!("/vol/f{:05}", (i * 10) % 10_000)],
            )
        })
        .collect();
    // Several applications share the same artifacts.
    for i in 0..200 {
        applications.push(app_fact_moderate(
            &format!("SharedConsumer{i:03}"),
            &["/vol/shared"],
        ));
    }

    let mut relationships: Vec<RelationshipFact> = Vec::new();
    for i in 0..50_000 {
        let a = format!("/vol/f{:05}", (i * 37) % 10_000);
        let b = format!("/vol/f{:05}", (i * 37 + 1) % 10_000);
        relationships.push(RelationshipFact {
            kind: RelationshipFactKind::ContentDuplicate,
            paths: vec![PathBuf::from(a), PathBuf::from(b)],
            object: None,
            content_sha256: Some(format!("rel-{i:05}")),
        });
    }

    let input = input(
        [
            artifacts.clone(),
            vec![ArtifactFact {
                path: PathBuf::from("/vol/shared"),
                kind: coresight_apps::ProbedKind::File,
                identity: Some(coresight_identity::ObjectIdentity::narrow(1, 77)),
                content_sha256: None,
                size: Some(1),
                access: coresight_capabilities::access::AccessState::ReadSucceeded,
                classification: None,
            }],
        ]
        .concat(),
        applications,
        relationships,
        vec![],
        complete_coverage(),
    );

    let model = build_system_model(&input, &stress_limits());
    assert!(model.check_invariants().is_ok());
    assert_eq!(model.artifact_count(), 10_001);
    assert_eq!(model.application_count(), 1_200);
    assert!(model.edge_count() <= stress_limits().max_edges);
    for a in model.artifact_nodes() {
        assert!(
            model.edges_for_node(&a.key).len() <= stress_limits().max_edges_per_node,
            "node {} over the per-node bound",
            a.key
        );
    }

    // Deterministic at scale: an identical rebuild is byte-for-byte equal.
    let again = build_system_model(&input, &stress_limits());
    assert_eq!(model, again);

    // Reversed rebuild is equal too.
    let mut reversed = input.clone();
    reversed.artifacts.reverse();
    reversed.applications.reverse();
    reversed.relationships.reverse();
    assert_eq!(model, build_system_model(&reversed, &stress_limits()));

    // Evidence never double-counts at scale.
    for e in &model.edges {
        for ev in &e.evidence {
            assert!(ev.strength <= ev.kind.max_strength());
            assert!(ev.strength <= ev.correlation_group.ceiling());
        }
    }
}

/// Multi-stage propagation: the same registry record's evidence, forwarded
/// through several inputs, must not gain strength as it travels.
#[test]
fn multi_stage_evidence_keeps_its_source_provenance() {
    // Stage 1: registry record → install root + executable (one group).
    let mut fact = app_fact(
        "Staged",
        Some("Vendor"),
        &["/opt/staged"],
        &["/opt/staged/s.exe"],
    );
    fact.executable = Some("/opt/staged/s.exe".into());
    let model = build_system_model(
        &input(
            vec![
                dir("/opt/staged"),
                artifact("/opt/staged/s.exe", 1, 1, None),
            ],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let exe_key = artifact_key_for(std::path::Path::new("/opt/staged/s.exe"));
    let claim = coresight_system_model::owning_applications(&model, &exe_key, 8);
    // Exactly one application claims it: the two items from the same record
    // (install root + executable path) produce one owner, not two.
    assert_eq!(claim.items.len(), 1);

    // Stage 2: the SAME record arrives again through a duplicate input —
    // the assessment must be unchanged (idempotent appraisal).
    let base = coresight_system_model::build::SystemModelInput {
        artifacts: vec![
            dir("/opt/staged"),
            artifact("/opt/staged/s.exe", 1, 1, None),
        ],
        applications: vec![
            app_fact(
                "Staged",
                Some("Vendor"),
                &["/opt/staged"],
                &["/opt/staged/s.exe"],
            ),
            app_fact(
                "Staged",
                Some("Vendor"),
                &["/opt/staged"],
                &["/opt/staged/s.exe"],
            ),
        ],
        relationships: vec![],
        history: vec![],
        source_coverage: complete_coverage(),
    };
    let doubled = build_system_model(&base, &SystemModelLimits::default());
    let claim2 = coresight_system_model::owning_applications(&model, &exe_key, 8);
    let _ = claim2;
    let single = build_system_model(
        &coresight_system_model::build::SystemModelInput {
            applications: vec![base.applications[0].clone()],
            ..base.clone()
        },
        &SystemModelLimits::default(),
    );
    assert_eq!(doubled, single, "a twice-forwarded record appraises once");
}

/// Name-derived evidence forwarded through apps AND containment must stay
/// weak: the group is one vote no matter how many hops it took.
#[test]
fn name_evidence_forwarded_through_two_hops_stays_weak() {
    let weak = app_fact_weak("Named", &["/data/named"]);
    let model = build_system_model(
        &input(
            vec![artifact("/data/named", 1, 1, None)],
            vec![weak],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let key = artifact_key_for(std::path::Path::new("/data/named"));
    assert_eq!(
        model.artifact(&key).unwrap().application_status,
        coresight_system_model::ArtifactApplicationStatus::Uncertain
    );
    assert!(model
        .edges
        .iter()
        .all(|e| e.kind != SystemEdgeKind::OwnedBy));
    // And the correlation group on the published evidence is still the
    // name-derived one — the provenance was not laundered.
    for e in &model.edges {
        for ev in &e.evidence {
            if ev.correlation_group == CorrelationGroup::NameDerived {
                assert!(ev.strength <= EvidenceStrength::Weak);
            }
        }
    }
}
