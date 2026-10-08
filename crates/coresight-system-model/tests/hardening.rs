//! Phase 6.3 hardening regression matrix: adversarial deserialization,
//! adversarial invariants, determinism under duplication, hostile-scale
//! bounds, identity semantics, relationship-proof validation, truncation
//! honesty, history conflicts, and descendant-aware application state.
//!
//! Portable: every fixture is synthetic and no test touches a real
//! filesystem, so the whole suite runs on all three CI platforms.

mod fixtures;

use std::path::PathBuf;

use coresight_apps::{
    ApplicationId, CorrelationGroup, EvidenceKind, EvidenceStrength, OwnershipAssessment,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    artifact_key_for, artifacts_without_application, build_system_model, unresolved_associations,
    ArtifactApplicationStatus, HistoricalRelation, InsightKind, RelationshipFact,
    RelationshipFactKind, SystemModel, SystemModelLimits,
};

use fixtures::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn small_limits() -> SystemModelLimits {
    SystemModelLimits {
        max_artifacts: 64,
        max_applications: 8,
        max_edges: 16,
        max_evidence_per_edge: 2,
        max_edges_per_node: 4,
        max_historical_context: 8,
        max_insights: 16,
        max_candidates: 16,
        max_source_states: 64,
    }
}

fn tiny_limits() -> SystemModelLimits {
    SystemModelLimits {
        max_artifacts: 8,
        max_applications: 4,
        max_edges: 4,
        max_evidence_per_edge: 1,
        max_edges_per_node: 2,
        max_historical_context: 4,
        max_insights: 8,
        max_candidates: 8,
        max_source_states: 64,
    }
}

fn key(p: &str) -> String {
    artifact_key_for(std::path::Path::new(p))
}

// ---------------------------------------------------------------------------
// Deserialization: indexes are never trusted
// ---------------------------------------------------------------------------

#[test]
fn deserialization_rebuilds_indexes_and_rejects_structural_breaks() {
    let mut app = app_fact("App", Some("V"), &["/opt/app"], &["/opt/app/tool"]);
    app.record.executable_path = Some(PathBuf::from("/opt/app/tool"));
    app.executable = app.record.executable_path.clone();
    let model = build_system_model(
        &input(
            vec![dir("/opt/app"), artifact("/opt/app/tool", 1, 1, None)],
            vec![app],
            vec![],
            vec![history(
                "run-1",
                "/opt/app/tool",
                Some(ObjectIdentity::narrow(1, 1)),
            )],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(model.check_invariants().is_ok());
    let json = serde_json::to_string(&model).expect("serializes");

    // 1. Valid round trip: canonical equality, rebuilt indexes.
    let back: SystemModel = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(model, back);
    assert!(back.check_invariants().is_ok());

    // 2. Legacy payloads carrying an `indexes` section still parse — the
    // section is ignored and the same canonical index set is rebuilt.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["indexes"] = serde_json::json!({
        "artifactByKey": {"bogus": 999},
        "applicationById": {},
        "edgesByNode": {"bogus": [42]},
        "artifactsByObject": [],
        "artifactsByContent": {},
        "artifactsByCategory": {},
        "artifactsByApplication": {"bogus": ["bogus"]}
    });
    let rebuilt: SystemModel =
        serde_json::from_value(v).expect("legacy indexes section is ignored");
    assert_eq!(model, rebuilt);
    assert!(rebuilt.check_invariants().is_ok());

    // 3. Corrupted canonical data is rejected, never rebuilt inconsistently.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["artifacts"][0]["key"] = serde_json::json!("not-a-real-key");
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // A canonical-looking key cannot be paired with a different path.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["artifacts"][0]["path"] = serde_json::json!("/different/path");
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // 4. An edge to an unknown node is rejected.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    if let Some(edges) = v["edges"].as_array_mut() {
        edges.push(serde_json::json!({
            "kind": "OWNED_BY",
            "domain": "APPLICATION_INTELLIGENCE",
            "from": "app-missing",
            "to": "art-missing",
            "assessment": "STRONG",
            "provenance": "INFERRED",
            "evidence": []
        }));
    }
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // Domain labels are derived from edge kind and cannot be forged on wire.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["edges"][0]["domain"] = serde_json::json!("CLASSIFICATION");
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // Serialized evidence cannot bypass constructor ceilings by changing its
    // kind while retaining a stronger strength.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let edge = v["edges"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|edge| {
            edge["evidence"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["kind"] == "INSTALL_LOCATION"))
        })
        .expect("fixture has install evidence");
    let evidence = edge["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["kind"] == "INSTALL_LOCATION")
        .unwrap();
    evidence["kind"] = serde_json::json!("FILENAME_SIMILARITY");
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // A role edge cannot cite an unrelated path as evidence for its artifact
    // endpoint, even when the evidence kind and confidence look plausible.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let edge = v["edges"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|edge| edge["kind"] == "APPLICATION_EXECUTABLE")
        .expect("fixture has an executable role edge");
    edge["evidence"][0]["matchedPath"] = serde_json::json!("/unrelated/tool");
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // An exact duplicate edge is not a canonical graph fact.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    if let Some(edges) = v["edges"].as_array_mut() {
        let duplicate = edges.last().cloned().expect("fixture has edges");
        edges.push(duplicate);
    }
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // 5. A payload with the canonical sections omitted entirely parses to
    // an empty-but-valid model only when ALL sections are absent; a
    // half-present payload (edges without nodes) is rejected above.
    let empty: SystemModel = serde_json::from_str(
        r#"{"artifacts":[],"applications":[],"edges":[],"observations":{
        "sourceStates":[],"capabilities":[],"inaccessibleArtifacts":0,
        "unsupportedArtifacts":0,"failedArtifacts":0},
        "truncation":{"artifactsTruncated":0,"applicationsTruncated":0,
        "edgesTruncated":0,"edgesPerNodeTruncated":0,"evidenceTruncated":0,
        "historicalContextTruncated":0,"insightsTruncated":0,
        "candidatesTruncated":0}}"#,
    )
    .expect("empty canonical payload parses");
    assert!(empty.check_invariants().is_ok());
    assert_eq!(empty.artifact_count(), 0);
}

#[test]
fn wire_level_inconsistencies_are_rejected_not_rebuilt() {
    // The wire format is the only route to an inconsistent internal
    // representation (canonical storage is private): every structural break
    // must be rejected before indexes are rebuilt.
    let model = build_system_model(
        &input(
            vec![artifact("/w/a", 1, 1, None), artifact("/w/b", 1, 2, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let json = serde_json::to_string(&model).expect("serializes");

    // 6. Duplicate artifact keys.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let dup = v["artifacts"][0].clone();
    v["artifacts"].as_array_mut().unwrap().push(dup);
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // 7. Unordered artifacts.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let arr = v["artifacts"].as_array_mut().unwrap();
    arr.swap(0, 1);
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // 8. Historical assertion for an unknown artifact.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["historicalAssertions"] = serde_json::json!([{
        "artifactKey": "art-ghost",
        "runId": "run-1",
        "path": "/ghost",
        "recordedIdentity": null,
        "currentIdentity": null,
        "relation": "IDENTITY_UNPROVEN",
        "category": null,
        "provenance": "OBSERVED",
        "evidence": []
    }]);
    assert!(serde_json::from_value::<SystemModel>(v).is_err());

    // 9. Swapping two artifact keys breaks canonical ordering: rejected.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let k0 = v["artifacts"][0]["key"].clone();
    let k1 = v["artifacts"][1]["key"].clone();
    v["artifacts"][0]["key"] = k1;
    v["artifacts"][1]["key"] = k0;
    assert!(serde_json::from_value::<SystemModel>(v).is_err());
}

#[test]
fn deserialization_rejects_candidate_without_inert_executor_blocker() {
    let model = build_system_model(
        &input(
            vec![artifact("/wire/orphan", 1, 1, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(!model.candidates().is_empty());
    let mut value = serde_json::to_value(model).expect("serializes");
    value["candidates"][0]["blockers"] = serde_json::json!([]);
    assert!(serde_json::from_value::<SystemModel>(value).is_err());

    let mut value = serde_json::to_value(build_system_model(
        &input(
            vec![artifact("/wire/orphan", 1, 1, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    ))
    .expect("serializes");
    value["candidates"][0]["effect"] = serde_json::json!("READ_ONLY");
    assert!(serde_json::from_value::<SystemModel>(value).is_err());
}

// ---------------------------------------------------------------------------
// Truncation honesty: claim truncated ≠ no claim
// ---------------------------------------------------------------------------

#[test]
fn truncated_claims_never_become_unassociated_or_orphans() {
    // One artifact with a real claim, then a global edge bound of zero:
    // every claim edge is dropped by the bound.
    let limits = SystemModelLimits {
        max_edges: 0,
        ..SystemModelLimits::default()
    };
    let model = build_system_model(
        &input(
            vec![artifact("/t/tool", 1, 1, None)],
            vec![app_fact("T", None, &[], &["/t/tool"])],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits,
    );
    assert!(model.check_invariants().is_ok());
    let node = model.artifact(&key("/t/tool")).unwrap();
    // Claim present before the bound, dropped by the bound → truncated.
    assert_eq!(
        node.application_status,
        ArtifactApplicationStatus::AssociationTruncated,
        "a dropped claim must not become Unassociated"
    );
    assert!(!node.application_status.is_genuinely_unassociated());
    // No UnassociatedArtifact insight, no Orphan candidate.
    assert!(
        !model
            .insights()
            .iter()
            .any(|i| i.kind == InsightKind::UnassociatedArtifact),
        "truncation must not manufacture an unassociated insight"
    );
    assert!(
        !model
            .candidates()
            .iter()
            .any(|c| c.action_kind == coresight_system_model::CandidateActionKind::Orphan),
        "truncation must not manufacture an orphan candidate"
    );
    assert!(
        artifacts_without_application(&model, 16).items.is_empty(),
        "the no-application query must stay empty under truncation"
    );
    // ... but the artifact IS visible as unresolved-with-cause, and the
    // truncation counter records the drop.
    assert_eq!(unresolved_associations(&model, 16).items.len(), 1);
    assert!(model.truncation().edges_truncated > 0);
}

#[test]
fn per_node_bound_drops_also_produce_truncated_not_unassociated() {
    // Many claimants, per-node bound of 1: the artifact keeps one edge but
    // loses the rest — still associated (one credible claim survives), and
    // the drop is counted.
    let limits = SystemModelLimits {
        max_edges_per_node: 1,
        ..SystemModelLimits::default()
    };
    let apps: Vec<_> = (0..6)
        .map(|i| app_fact(&format!("N{i}"), None, &[], &["/n/tool"]))
        .collect();
    let model = build_system_model(
        &input(
            vec![artifact("/n/tool", 1, 1, None)],
            apps,
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits,
    );
    assert!(model.check_invariants().is_ok());
    let node = model.artifact(&key("/n/tool")).unwrap();
    assert_ne!(
        node.application_status,
        ArtifactApplicationStatus::Unassociated
    );
    assert!(model.truncation().edges_per_node_truncated > 0);
}

// ---------------------------------------------------------------------------
// Hostile-scale bounds: 100k associations, 100k roots
// ---------------------------------------------------------------------------

#[test]
fn source_coverage_summary_is_bounded_and_counted_at_admission() {
    let limits = SystemModelLimits {
        max_source_states: 2,
        ..small_limits()
    };
    let coverage = (0..10_000)
        .map(|i| coresight_apps::SourceCoverage::complete(&format!("source-{i:05}")))
        .collect();
    let model = build_system_model(&input(vec![], vec![], vec![], vec![], coverage), &limits);
    assert!(model.observations().source_states.len() <= 2);
    assert_eq!(model.truncation().source_states_truncated, 9_998);
    assert!(model.check_invariants().is_ok());
}

#[test]
fn one_application_with_100k_associations_stays_bounded_and_counted() {
    // 100,000+ association records from ONE application against a tiny
    // claim store: most pairs are refused at admission, counted exactly.
    // Enough associations name RETAINED artifacts that the store (cap 16)
    // overflows many times over.
    let paths: Vec<String> = (0..100_000).map(|i| format!("/h/f{i:06}")).collect();
    let mut arts: Vec<_> = paths
        .iter()
        .take(64)
        .enumerate()
        .map(|(i, p)| artifact(p, 1, i as u64, None))
        .collect();
    arts.push(artifact("/h/other", 1, 999_999, None));
    let mut fact = app_fact("Huge", None, &[], &[]);
    for p in &paths {
        fact.associations.push((
            PathBuf::from(p),
            install_evidence(
                &fact.record.clone(),
                p,
                EvidenceStrength::Direct,
                EvidenceKind::InstallLocation,
                CorrelationGroup::SourceRecord(fact.record.source.clone()),
            ),
        ));
    }
    let model = build_system_model(
        &input(arts, vec![fact], vec![], vec![], complete_coverage()),
        &small_limits(),
    );
    assert!(model.check_invariants().is_ok());
    assert!(model.edge_count() <= small_limits().max_edges);
    // The pair store admitted at most max_edges pairs; drops are counted.
    assert!(model.truncation().claims_truncated > 0);
}

#[test]
fn one_application_with_100k_roots_stays_bounded_and_counted() {
    let roots: Vec<String> = (0..100_000).map(|i| format!("/r{i:06}")).collect();
    let mut fact = app_fact("Rooty", None, &[], &[]);
    fact.install_roots = roots.iter().map(PathBuf::from).collect();
    fact.record.install_location = Some(PathBuf::from("/r000001"));
    let model = build_system_model(
        &input(
            vec![dir("/r000001"), artifact("/r000001/f", 1, 1, None)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &small_limits(),
    );
    assert!(model.check_invariants().is_ok());
    assert!(model.truncation().roots_truncated > 0);
}

#[test]
fn dropped_root_group_cannot_create_false_unassociated_or_orphan() {
    // /z-root is canonically beyond the small retained root prefix. Its
    // descendant must stay incomplete, never be mislabeled absent.
    let mut roots: Vec<String> = (0..100).map(|i| format!("/r{i:03}")).collect();
    roots.push("/z-root".to_string());
    let mut app = app_fact("Rooty", None, &[], &[]);
    app.install_roots = roots.iter().map(PathBuf::from).collect();
    let model = build_system_model(
        &input(
            vec![artifact("/z-root/descendant", 9, 1, None)],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &small_limits(),
    );
    let node = model
        .artifact(&key("/z-root/descendant"))
        .expect("retained artifact");
    assert_eq!(
        node.application_status,
        ArtifactApplicationStatus::AssociationTruncated
    );
    assert!(model.truncation().roots_truncated > 0);
    assert!(!model.candidates().iter().any(|c| {
        c.target == node.key && c.action_kind == coresight_system_model::CandidateActionKind::Orphan
    }));
}

#[test]
fn root_owner_overflow_is_bounded_and_never_claims_absence() {
    let apps: Vec<_> = (0..32)
        .map(|i| app_fact(&format!("Owner{i:02}"), None, &["/many"], &[]))
        .collect();
    let model = build_system_model(
        &input(
            vec![artifact("/many/descendant", 7, 3, None)],
            apps,
            vec![],
            vec![],
            complete_coverage(),
        ),
        &small_limits(),
    );
    let node = model
        .artifact(&key("/many/descendant"))
        .expect("retained artifact");
    assert_ne!(
        node.application_status,
        ArtifactApplicationStatus::Unassociated
    );
    assert!(model.truncation().claims_truncated > 0);
    assert!(!model.candidates().iter().any(|c| {
        c.target == node.key && c.action_kind == coresight_system_model::CandidateActionKind::Orphan
    }));
}

// ---------------------------------------------------------------------------
// Relationship proof validation
// ---------------------------------------------------------------------------

fn rel_paths(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn wrong_object_identity_is_rejected_with_accounting() {
    let object = ObjectIdentity {
        volume: 1,
        file_id: 7,
        file_id_hi: Some(3),
    };
    let wrong = ObjectIdentity {
        volume: 1,
        file_id: 7,
        file_id_hi: Some(4),
    };
    let model = build_system_model(
        &input(
            vec![
                artifact("/w/a", 1, 7, Some(3)),
                artifact("/w/b", 1, 7, Some(3)),
            ],
            vec![],
            vec![RelationshipFact {
                kind: RelationshipFactKind::HardLinkAlias,
                paths: rel_paths(&["/w/a", "/w/b"]),
                object: Some(wrong),
                content_sha256: None,
            }],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(model.check_invariants().is_ok());
    assert!(
        model.edges().is_empty(),
        "nothing unproven becomes graph truth"
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    let _ = object;
}

#[test]
fn same_object_offered_as_content_duplicate_is_rejected() {
    let digest = "dd01";
    let model = build_system_model(
        &input(
            vec![
                with_content(artifact("/s/a", 1, 9, None), digest),
                with_content(artifact("/s/b", 1, 9, None), digest),
            ],
            vec![],
            vec![content_duplicate(&["/s/a", "/s/b"], digest)],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(model.check_invariants().is_ok());
    assert!(
        model
            .edges()
            .iter()
            .all(|e| e.kind != coresight_system_model::SystemEdgeKind::DuplicateOf),
        "one object is an alias, never a content duplicate"
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    // ... and no DuplicateContent insight either (identities agree).
    assert!(!model
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::DuplicateContent));
}

#[test]
fn wrong_digest_and_missing_proof_are_rejected() {
    let digest = "ee01";
    // Wrong fact digest vs node digests.
    let model = build_system_model(
        &input(
            vec![
                with_content(artifact("/d/a", 1, 11, None), digest),
                with_content(artifact("/d/b", 1, 12, None), digest),
            ],
            vec![],
            vec![content_duplicate(&["/d/a", "/d/b"], "wrong-digest")],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(model.edges().is_empty());

    // Missing proof: nodes carry no digest at all.
    let model = build_system_model(
        &input(
            vec![artifact("/d/a", 1, 11, None), artifact("/d/b", 1, 12, None)],
            vec![],
            vec![content_duplicate(&["/d/a", "/d/b"], digest)],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(model.edges().is_empty());

    // Unknown endpoint: one path is not a retained artifact.
    let model = build_system_model(
        &input(
            vec![with_content(artifact("/d/a", 1, 11, None), digest)],
            vec![],
            vec![content_duplicate(&["/d/a", "/d/never-observed"], digest)],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(model.edges().is_empty());
    assert!(model.check_invariants().is_ok());
}

#[test]
fn relationship_rejects_partial_unknown_and_overwide_proofs() {
    let digest = "wide-proof";
    let distinct = vec![
        with_content(artifact("/wide/a", 1, 1, None), digest),
        with_content(artifact("/wide/b", 1, 2, None), digest),
    ];
    let model = build_system_model(
        &input(
            distinct,
            vec![],
            vec![content_duplicate(
                &["/wide/a", "/wide/b", "/wide/not-retained"],
                digest,
            )],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(model
        .edges()
        .iter()
        .all(|e| e.kind != coresight_system_model::SystemEdgeKind::DuplicateOf));

    // Unknown identity cannot prove distinct objects, even when the other
    // endpoint is known and both carry the same digest.
    let model = build_system_model(
        &input(
            vec![
                with_content(artifact_no_identity("/wide/unknown"), digest),
                with_content(artifact("/wide/known", 1, 8, None), digest),
            ],
            vec![],
            vec![content_duplicate(&["/wide/unknown", "/wide/known"], digest)],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(model
        .edges()
        .iter()
        .all(|e| e.kind != coresight_system_model::SystemEdgeKind::DuplicateOf));

    // With max_edges=4 only three members (three pairs) can be admitted as
    // one complete relationship; four endpoints are rejected atomically.
    let limits = SystemModelLimits {
        max_edges: 4,
        ..small_limits()
    };
    let artifacts = (0..4)
        .map(|i| with_content(artifact(&format!("/wide/f{i}"), 3, i, None), digest))
        .collect();
    let model = build_system_model(
        &input(
            artifacts,
            vec![],
            vec![content_duplicate(
                &["/wide/f0", "/wide/f1", "/wide/f2", "/wide/f3"],
                digest,
            )],
            vec![],
            complete_coverage(),
        ),
        &limits,
    );
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(model
        .edges()
        .iter()
        .all(|e| e.kind != coresight_system_model::SystemEdgeKind::DuplicateOf));
}

#[test]
fn hard_link_alias_without_any_proven_identity_is_rejected() {
    // Neither endpoint proves an identity: missing proof, not an alias.
    let object = ObjectIdentity::narrow(9, 9);
    let model = build_system_model(
        &input(
            vec![artifact_no_identity("/u/a"), artifact_no_identity("/u/b")],
            vec![],
            vec![hard_link_alias(&["/u/a", "/u/b"], object)],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(model.check_invariants().is_ok());
    assert!(model.edges().is_empty());
    assert_eq!(model.truncation().relationships_rejected, 1);
    assert!(!model
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::HardLinkAlias));
}

#[test]
fn duplicate_content_insight_requires_proven_distinct_identities() {
    let digest = "ff01";
    // Same identity + same digest → alias only, never DuplicateContent.
    let m1 = build_system_model(
        &input(
            vec![
                with_content(artifact("/i/a", 1, 21, None), digest),
                with_content(artifact("/i/b", 1, 21, None), digest),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(
        m1.insights()
            .iter()
            .any(|i| i.kind == InsightKind::HardLinkAlias),
        "same identity is an alias"
    );
    assert!(
        !m1.insights()
            .iter()
            .any(|i| i.kind == InsightKind::DuplicateContent),
        "same identity must never read as distinct objects"
    );

    // Different identities + same digest → genuine DuplicateContent.
    let m2 = build_system_model(
        &input(
            vec![
                with_content(artifact("/i/a", 1, 21, None), digest),
                with_content(artifact("/i/b", 1, 22, None), digest),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(m2
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::DuplicateContent));

    // Unknown identity + same digest → NEITHER claim (distinctness
    // unestablished): no false "distinct objects".
    let m3 = build_system_model(
        &input(
            vec![
                with_content(artifact_no_identity("/i/a"), digest),
                with_content(artifact_no_identity("/i/b"), digest),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(
        !m3.insights()
            .iter()
            .any(|i| i.kind == InsightKind::DuplicateContent),
        "unproven distinctness must not become a duplicate claim"
    );

    // One known identity plus one unknown identity is still insufficient
    // to prove two distinct objects.
    let m5 = build_system_model(
        &input(
            vec![
                with_content(artifact("/i/known", 1, 31, None), digest),
                with_content(artifact_no_identity("/i/unknown"), digest),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(!m5
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::DuplicateContent));

    // Wide identities differing only in high bits are distinct objects.
    let m4 = build_system_model(
        &input(
            vec![
                with_content(artifact("/i/a", 1, 21, Some(1)), digest),
                with_content(artifact("/i/b", 1, 21, Some(2)), digest),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(
        m4.insights()
            .iter()
            .any(|i| i.kind == InsightKind::DuplicateContent),
        "high bits distinguish objects (never narrowed)"
    );
    for m in [&m1, &m2, &m3, &m4, &m5] {
        assert!(m.check_invariants().is_ok());
    }
}

// ---------------------------------------------------------------------------
// Determinism: duplicate records, history conflicts, permutations
// ---------------------------------------------------------------------------

#[test]
fn duplicate_application_records_resolve_by_total_order_not_arrival() {
    // Same id, same reason COUNT, deliberately different content.
    let mut a = app_fact("Dup", Some("Pub"), &[], &["/dup/x"]);
    a.record.bundle_identifier = Some("com.example.one".into());
    let mut b = app_fact("Dup", Some("Pub"), &[], &["/dup/x"]);
    b.record.bundle_identifier = Some("com.example.two".into());
    assert_eq!(
        a.record.id, b.record.id,
        "fixture must share one logical id"
    );
    for apps in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
        let model = build_system_model(
            &input(
                vec![artifact("/dup/x", 1, 1, None)],
                apps,
                vec![],
                vec![],
                complete_coverage(),
            ),
            &SystemModelLimits::default(),
        );
        let node = &model.applications()[0];
        assert_eq!(model.application_count(), 1);
        // Total order: "com.example.two" > "com.example.one" wins either way.
        assert_eq!(
            node.bundle_identifier.as_deref(),
            Some("com.example.two"),
            "arrival order must not decide the winner"
        );
        assert!(model.check_invariants().is_ok());
    }
}

#[test]
fn conflicting_history_facts_are_preserved_never_arrival_ordered() {
    use coresight_system_model::HistoryFact;
    let fact_a = HistoryFact {
        run_id: "run-1".to_string(),
        path: PathBuf::from("/cf/file"),
        identity: Some(ObjectIdentity::narrow(1, 1)),
        category: Some("CACHE".to_string()),
    };
    let fact_b = HistoryFact {
        run_id: "run-1".to_string(),
        path: PathBuf::from("/cf/file"),
        identity: Some(ObjectIdentity::narrow(1, 2)),
        category: Some("LOGS".to_string()),
    };
    let mk = |order: &[HistoryFact]| {
        build_system_model(
            &input(
                vec![artifact("/cf/file", 1, 1, None)],
                vec![],
                vec![],
                order.to_vec(),
                complete_coverage(),
            ),
            &SystemModelLimits::default(),
        )
    };
    let ab = mk(&[fact_a.clone(), fact_b.clone()]);
    let ba = mk(&[fact_b.clone(), fact_a.clone()]);
    assert_eq!(ab, ba, "history arrival order must not change the model");
    // Conflict preserved: BOTH contradictory rows survive.
    assert_eq!(ab.historical_context().len(), 2);
    // Identical facts still collapse.
    let twice = mk(&[fact_a.clone(), fact_a.clone()]);
    let once = mk(std::slice::from_ref(&fact_a));
    assert_eq!(twice, once, "identical history facts collapse");
    assert_eq!(twice.historical_context().len(), 1);
}

#[test]
fn history_conflict_on_category_only_is_also_preserved() {
    use coresight_system_model::HistoryFact;
    // Same run+path, category present vs absent: distinct payloads, both kept.
    let model = build_system_model(
        &input(
            vec![artifact("/cc/file", 1, 1, None)],
            vec![],
            vec![],
            vec![
                HistoryFact {
                    run_id: "run-7".to_string(),
                    path: PathBuf::from("/cc/file"),
                    identity: Some(ObjectIdentity::narrow(1, 1)),
                    category: Some("CACHE".to_string()),
                },
                HistoryFact {
                    run_id: "run-7".to_string(),
                    path: PathBuf::from("/cc/file"),
                    identity: Some(ObjectIdentity::narrow(1, 1)),
                    category: None,
                },
            ],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.historical_context().len(), 2);
    assert!(model.check_invariants().is_ok());
}

#[test]
fn desktop_exec_parent_children_do_not_claim_a_partial_footprint() {
    let mut fact = app_fact("DesktopMissing", None, &["/usr/bin"], &[]);
    fact.record.source = coresight_apps::ApplicationSource::DesktopEntry;
    fact.record.provenance = vec![coresight_apps::ApplicationSource::DesktopEntry];
    fact.record.install_location = Some(PathBuf::from("/usr/bin"));
    fact.record.executable_path = Some(PathBuf::from("/usr/bin/expected-tool"));
    let model = build_system_model(
        &input(
            vec![
                dir("/usr/bin"),
                artifact("/usr/bin/unrelated-tool", 1, 2, None),
            ],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(
        model.applications()[0].state,
        coresight_system_model::ApplicationState::Unresolved
    );
    assert!(!model.applications()[0]
        .state_reasons
        .contains(&coresight_system_model::ApplicationStateReason::DescendantObserved));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn desktop_exec_parent_is_not_mistaken_for_recorded_install_root() {
    let mut fact = app_fact("DesktopRoot", None, &["/desktop/bin"], &[]);
    fact.record.source = coresight_apps::ApplicationSource::DesktopEntry;
    fact.record.provenance = vec![coresight_apps::ApplicationSource::DesktopEntry];
    fact.record.install_location = Some(PathBuf::from("/desktop/bin"));
    fact.record.executable_path = Some(PathBuf::from("/desktop/bin/tool"));
    fact.executable = fact.record.executable_path.clone();
    let model = build_system_model(
        &input(
            vec![
                dir("/desktop/bin"),
                artifact("/desktop/bin/tool", 1, 4, None),
            ],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let root_edge = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationInstallRoot)
        .expect("derived root remains visible as weak scope");
    assert_eq!(root_edge.assessment, OwnershipAssessment::Weak);
    assert_eq!(
        root_edge.provenance,
        coresight_system_model::ProvenanceState::Inferred
    );
    let executable_edge = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationExecutable)
        .expect("exact Exec metadata remains an observed executable role");
    assert_eq!(executable_edge.assessment, OwnershipAssessment::Strong);
    assert_eq!(
        executable_edge.provenance,
        coresight_system_model::ProvenanceState::Observed
    );
    assert!(model.check_invariants().is_ok());
}

#[test]
fn detector_root_is_not_promoted_to_observed_ownership() {
    let app = app_fact("RootCandidate", Some("V"), &["/roots/guessed"], &[]);
    let model = build_system_model(
        &input(
            vec![dir("/roots/guessed")],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let edge = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationInstallRoot)
        .expect("root candidate has a descriptive edge");
    assert_eq!(edge.assessment, OwnershipAssessment::Weak);
    assert_eq!(
        edge.provenance,
        coresight_system_model::ProvenanceState::Inferred
    );
    assert!(!model
        .edges()
        .iter()
        .any(|edge| edge.kind == coresight_system_model::SystemEdgeKind::OwnedBy));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn unrecorded_executable_candidate_cannot_resolve_or_confirm_ownership() {
    let mut app = app_fact("ExeCandidate", None, &[], &[]);
    app.executable = Some(PathBuf::from("/apps/candidate.exe"));
    let model = build_system_model(
        &input(
            vec![artifact("/apps/candidate.exe", 1, 1, None)],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(
        model.applications()[0].state,
        coresight_system_model::ApplicationState::PartiallyResolved
    );
    let executable = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationExecutable)
        .expect("candidate path remains visible");
    assert_eq!(executable.assessment, OwnershipAssessment::Weak);
    assert_eq!(
        executable.provenance,
        coresight_system_model::ProvenanceState::Candidate
    );
    assert!(!model
        .edges()
        .iter()
        .any(|edge| edge.kind == coresight_system_model::SystemEdgeKind::OwnedBy));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn inaccessible_recorded_root_is_unknown_not_absent() {
    let mut fact = app_fact("Denied", Some("V"), &["/denied/app"], &[]);
    fact.record.install_location = Some(PathBuf::from("/denied/app"));
    let mut root = dir("/denied/app");
    root.access = coresight_capabilities::access::AccessState::ExistsButInaccessible;
    let model = build_system_model(
        &input(vec![root], vec![fact], vec![], vec![], complete_coverage()),
        &SystemModelLimits::default(),
    );
    assert_eq!(
        model.applications()[0].state,
        coresight_system_model::ApplicationState::Unknown
    );
    assert!(model.applications()[0]
        .state_reasons
        .contains(&coresight_system_model::ApplicationStateReason::ExpectedDataInaccessible));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn observed_executable_with_unobserved_recorded_root_is_only_partial() {
    let mut fact = app_fact("RootGap", Some("V"), &["/gap/app"], &[]);
    fact.record.install_location = Some(PathBuf::from("/gap/app"));
    fact.record.executable_path = Some(PathBuf::from("/gap/app/bin/tool"));
    fact.executable = fact.record.executable_path.clone();
    let model = build_system_model(
        &input(
            vec![artifact("/gap/app/bin/tool", 1, 1, None)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(
        model.applications()[0].state,
        coresight_system_model::ApplicationState::PartiallyResolved,
        "the exact executable cannot hide an unobserved expected root"
    );
    assert!(model.applications()[0]
        .state_reasons
        .contains(&coresight_system_model::ApplicationStateReason::InstallRootUnobserved));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn lexically_different_install_root_is_not_promoted_to_exact() {
    let mut app = app_fact("LexicalRoot", None, &["/lexical-root/app"], &[]);
    app.record.install_location = Some(PathBuf::from("/lexical-root/./app"));
    let model = build_system_model(
        &input(
            vec![dir("/lexical-root/app")],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let edge = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationInstallRoot)
        .expect("candidate root remains visible");
    assert_eq!(
        edge.provenance,
        coresight_system_model::ProvenanceState::Inferred
    );
    assert_eq!(edge.assessment, OwnershipAssessment::Weak);
    assert_eq!(
        model.applications()[0].state,
        coresight_system_model::ApplicationState::Unresolved
    );
    assert!(model.check_invariants().is_ok());
}

#[test]
fn lexically_different_executable_path_is_not_promoted_to_exact() {
    let mut app = app_fact("LexicalPath", None, &[], &[]);
    app.record.executable_path = Some(PathBuf::from("/lexical/./tool"));
    app.executable = Some(PathBuf::from("/lexical/tool"));
    let model = build_system_model(
        &input(
            vec![artifact("/lexical/tool", 1, 3, None)],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let edge = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationExecutable)
        .expect("candidate path edge remains visible");
    assert_eq!(
        edge.provenance,
        coresight_system_model::ProvenanceState::Candidate
    );
    assert_eq!(edge.assessment, OwnershipAssessment::Weak);
    assert_eq!(
        model.applications()[0].state,
        coresight_system_model::ApplicationState::PartiallyResolved
    );
    assert!(model.check_invariants().is_ok());
}

#[test]
fn unrelated_strong_claim_does_not_upgrade_candidate_executable_role() {
    let path = "/apps/candidate.exe";
    let mut app = app_fact("ExeCandidateStrongClaim", None, &[], &[path]);
    app.executable = Some(PathBuf::from(path));
    let model = build_system_model(
        &input(
            vec![artifact(path, 1, 2, None)],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(model
        .edges()
        .iter()
        .any(|edge| edge.kind == coresight_system_model::SystemEdgeKind::OwnedBy));
    let executable = model
        .edges()
        .iter()
        .find(|edge| edge.kind == coresight_system_model::SystemEdgeKind::ApplicationExecutable)
        .expect("candidate executable role remains separately visible");
    assert_eq!(executable.assessment, OwnershipAssessment::Weak);
    assert_eq!(
        executable.provenance,
        coresight_system_model::ProvenanceState::Candidate
    );
    assert!(executable.evidence.iter().all(|e| {
        e.kind == EvidenceKind::FilenameSimilarity && e.strength == EvidenceStrength::Weak
    }));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn descendant_observed_with_unavailable_root_is_partial_never_unresolved() {
    // The root itself is not an artifact node, but artifacts under it are
    // observed: a partial footprint, not "nothing observed".
    let mut fact = app_fact("Partial", Some("V"), &[], &[]);
    fact.record.install_location = Some(PathBuf::from("/pp/app"));
    fact.install_roots = vec![PathBuf::from("/pp/app")];
    let model = build_system_model(
        &input(
            vec![artifact("/pp/app/bin/tool", 1, 1, None)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    let app = &model.applications()[0];
    assert_eq!(
        app.state,
        coresight_system_model::ApplicationState::PartiallyResolved
    );
    assert!(app
        .state_reasons
        .contains(&coresight_system_model::ApplicationStateReason::DescendantObserved));
    assert!(model.check_invariants().is_ok());
}

// ---------------------------------------------------------------------------
// Evidence bounds: thousands of items admitted through O(max_evidence)
// ---------------------------------------------------------------------------

#[test]
fn thousands_of_claimant_edges_stay_bounded_with_exact_accounting() {
    // 2,000 credible claimants on one artifact, evidence bound of 1: the
    // candidate's evidence streams through a bounded accumulator instead
    // of materializing thousands of items before truncating.
    let limits = SystemModelLimits {
        max_edges: 50_000,
        max_edges_per_node: 50_000,
        max_evidence_per_edge: 1,
        ..SystemModelLimits::default()
    };
    let apps: Vec<_> = (0..2_000)
        .map(|i| app_fact(&format!("Ev{i:04}"), None, &[], &["/ev/tool"]))
        .collect();
    let model = build_system_model(
        &input(
            vec![artifact("/ev/tool", 1, 1, None)],
            apps,
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits,
    );
    assert!(model.check_invariants().is_ok());
    for c in model.candidates() {
        assert!(
            c.evidence.len() <= limits.max_evidence_per_edge,
            "candidate evidence bounded at admission"
        );
    }
    for insight in model.insights() {
        assert!(
            insight.evidence.len() <= limits.max_evidence_per_edge,
            "insight evidence bounded at admission"
        );
    }
    // Drops were counted exactly (edges, evidence, or both).
    assert!(
        model.truncation().evidence_truncated > 0
            || model.truncation().edges_truncated > 0
            || model.truncation().edges_per_node_truncated > 0
            || model.truncation().claims_truncated > 0,
        "hostile evidence fan-out must be counted: {:?}",
        model.truncation()
    );
}

// ---------------------------------------------------------------------------
// Scale guard: 50k artifacts, no quadratic behavior
// ---------------------------------------------------------------------------

#[test]
fn fifty_thousand_artifacts_build_with_grouped_not_quadratic_insights() {
    // One shared digest across the whole set: an OLD pairwise scan would do
    // ~2.5B comparisons here; group summaries keep it linearithmic.
    let arts: Vec<_> = (0..50_000)
        .map(|i| {
            let mut a = artifact(&format!("/big/f{i:05}"), 1, i as u64, None);
            a.content_sha256 = Some("same-digest-for-all".to_string());
            a
        })
        .collect();
    let model = build_system_model(
        &input(arts, vec![], vec![], vec![], complete_coverage()),
        &SystemModelLimits::default(),
    );
    assert!(model.check_invariants().is_ok());
    assert_eq!(model.artifact_count(), 50_000);
    // DuplicateContent fires per node (identities are pairwise distinct).
    assert!(model
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::DuplicateContent));
}

// ---------------------------------------------------------------------------
// Index consistency across shapes
// ---------------------------------------------------------------------------

#[test]
fn every_index_matches_its_canonical_scan() {
    let digest = "ab12";
    let model = build_system_model(
        &input(
            vec![
                dir("/ix"),
                with_content(artifact("/ix/a", 1, 1, None), digest),
                with_content(artifact("/ix/b", 1, 2, None), digest),
                artifact("/ix/c", 1, 1, None),
            ],
            vec![app_fact("Ix", None, &["/ix"], &["/ix/a"])],
            vec![content_duplicate(&["/ix/a", "/ix/b"], digest)],
            vec![history("r1", "/ix/a", Some(ObjectIdentity::narrow(1, 1)))],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(model.check_invariants().is_ok());

    // Artifact lookup == canonical scan.
    for a in model.artifact_nodes() {
        assert_eq!(model.artifact(&a.key).map(|x| &x.key), Some(&a.key));
    }
    // Object lookup == canonical scan.
    let obj = ObjectIdentity::narrow(1, 1);
    let mut via_index = model
        .artifacts_sharing_object(obj)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    via_index.sort();
    let mut via_scan = model
        .artifact_nodes()
        .iter()
        .filter(|a| a.identity == Some(obj))
        .map(|a| a.key.clone())
        .collect::<Vec<_>>();
    via_scan.sort();
    assert_eq!(via_index, via_scan);
    // Content lookup == canonical scan.
    let mut via_index = model
        .artifacts_sharing_content(digest)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    via_index.sort();
    let mut via_scan = model
        .artifact_nodes()
        .iter()
        .filter(|a| a.content_sha256.as_deref() == Some(digest))
        .map(|a| a.key.clone())
        .collect::<Vec<_>>();
    via_scan.sort();
    assert_eq!(via_index, via_scan);
    // Edge lookup == canonical scan.
    for a in model.artifact_nodes() {
        let mut via_index: Vec<(&str, &str)> = model
            .edges_for_node(&a.key)
            .into_iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        via_index.sort();
        let mut via_scan: Vec<(&str, &str)> = model
            .edges()
            .iter()
            .filter(|e| e.from == a.key || e.to == a.key)
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        via_scan.sort();
        assert_eq!(via_index, via_scan, "edge index diverges for {}", a.key);
    }

    // Empty and single-node models satisfy the same invariant.
    let empty = build_system_model(
        &input(vec![], vec![], vec![], vec![], complete_coverage()),
        &SystemModelLimits::default(),
    );
    assert!(empty.check_invariants().is_ok());
    let single = build_system_model(
        &input(
            vec![artifact("/only", 1, 1, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert!(single.check_invariants().is_ok());
    assert_eq!(single.artifact(&key("/only")).unwrap().key, key("/only"));
}

// ---------------------------------------------------------------------------
// History assertions carry evidence and stay queryable
// ---------------------------------------------------------------------------

#[test]
fn historical_assertions_are_node_attached_with_evidence() {
    let model = build_system_model(
        &input(
            vec![artifact("/ha/f", 1, 5, None)],
            vec![],
            vec![],
            vec![history(
                "run-2",
                "/ha/f",
                Some(ObjectIdentity::narrow(1, 9)),
            )],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    assert_eq!(model.historical_assertions().len(), 1);
    let a = &model.historical_assertions()[0];
    assert_eq!(a.relation, HistoricalRelation::ObjectReplaced);
    assert_eq!(a.artifact_key, key("/ha/f"));
    assert!(!a.evidence.is_empty());
    // The historical-context insight still fires (from the assertion, not
    // from any edge — and there are no self-loop edges anywhere).
    assert!(model
        .insights()
        .iter()
        .any(|i| i.kind == InsightKind::HistoricalContext));
    assert!(
        model.edges().iter().all(|e| e.from != e.to),
        "no self-loop edges in the model"
    );
    assert!(model.check_invariants().is_ok());
}

// ---------------------------------------------------------------------------
// Truncated models still satisfy every invariant
// ---------------------------------------------------------------------------

#[test]
fn truncated_models_keep_bidirectional_index_consistency() {
    let arts: Vec<_> = (0..200)
        .map(|i| artifact(&format!("/tm/f{i:04}"), 1, i as u64, None))
        .collect();
    let apps: Vec<_> = (0..20)
        .map(|i| app_fact(&format!("Tm{i:02}"), None, &[], &["/tm/f0001"]))
        .collect();
    let model = build_system_model(
        &input(arts, apps, vec![], vec![], complete_coverage()),
        &tiny_limits(),
    );
    assert!(model.check_invariants().is_ok());
    assert!(model.truncation().artifacts_truncated > 0);
    assert!(model.truncation().applications_truncated > 0);
}

// ---------------------------------------------------------------------------
// Identity contract: key != identity, aliases share identity
// ---------------------------------------------------------------------------

#[test]
fn artifact_key_is_a_path_occurrence_never_an_identity() {
    let object = ObjectIdentity {
        volume: 3,
        file_id: 31,
        file_id_hi: Some(7),
    };
    let model = build_system_model(
        &input(
            vec![
                artifact("/alias/one", 3, 31, Some(7)),
                artifact("/alias/two", 3, 31, Some(7)),
            ],
            vec![],
            vec![hard_link_alias(&["/alias/one", "/alias/two"], object)],
            vec![],
            complete_coverage(),
        ),
        &SystemModelLimits::default(),
    );
    // Two path occurrences → two nodes, one shared identity.
    assert_eq!(model.artifact_count(), 2);
    assert_ne!(key("/alias/one"), key("/alias/two"));
    let shared = model.artifacts_sharing_object(object);
    assert_eq!(shared.len(), 2);
    assert!(model.check_invariants().is_ok());
}

// ---------------------------------------------------------------------------
// Appliction-id determinism smoke: unused alias guard
// ---------------------------------------------------------------------------

#[test]
fn application_ids_derive_from_name_and_publisher_only() {
    assert_eq!(
        ApplicationId::derive("Same", Some("Pub")),
        ApplicationId::derive("Same", Some("Pub"))
    );
    assert_ne!(
        ApplicationId::derive("Same", Some("Pub")),
        ApplicationId::derive("Same", Some("Other"))
    );
}
