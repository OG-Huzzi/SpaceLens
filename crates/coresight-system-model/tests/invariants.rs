//! Determinism, idempotence, invariant, adversarial and hostile-scale tests
//! for the Phase 6.3 unified system model.
//!
//! Portable: every fixture is synthetic and no test touches a real
//! filesystem, so the whole suite runs on all three CI platforms.

mod fixtures;

use std::path::PathBuf;

use coresight_apps::{
    ApplicationId, ApplicationSource, CorrelationGroup, EvidenceKind, SourceStatus,
};
use coresight_capabilities::access::AccessState;
use coresight_classifier::Category;
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    applications_for_artifact, artifact_key_for, artifacts_of_classification, build_system_model,
    conflicting_ownership, shared_artifacts, strongly_associated_artifacts,
    ArtifactApplicationStatus, InsightKind, ProvenanceState, SystemEdgeKind, SystemModel,
    SystemModelLimits,
};

use fixtures::*;

fn limits() -> SystemModelLimits {
    SystemModelLimits::default()
}

/// A moderately rich fixture used by the determinism tests.
fn rich_input() -> coresight_system_model::SystemModelInput {
    let shared = "/shared/runtime.dll";
    let mut a = app_fact("App A", Some("Vendor"), &["/opt/a"], &["/opt/a/a.exe"]);
    a.record.executable_path = Some("/opt/a/a.exe".into());
    a.executable = Some("/opt/a/a.exe".into());
    let b = app_fact_moderate("App B", &[shared]);
    let c = app_fact_moderate("App C", &[shared]);
    let digest = "deadbeef";
    input(
        vec![
            dir("/opt"),
            dir("/opt/a"),
            with_category(artifact("/opt/a/a.exe", 1, 1, None), Category::Applications),
            with_category(artifact("/opt/a/cache", 1, 2, None), Category::Cache),
            with_category(artifact("/opt/a/log.txt", 1, 3, None), Category::Logs),
            with_content(artifact(shared, 1, 4, None), digest),
            with_content(artifact("/opt/b/runtime.dll", 1, 5, None), digest),
            artifact_no_identity("/opt/a/unknown"),
            denied_artifact("/opt/a/protected"),
        ],
        vec![a, b, c],
        vec![content_duplicate(&[shared, "/opt/b/runtime.dll"], digest)],
        vec![history(
            "run-1",
            "/opt/a/a.exe",
            Some(ObjectIdentity::narrow(1, 1)),
        )],
        complete_coverage(),
    )
}

/// A deterministic permutation generator (Heap's algorithm) so the tests do
/// not depend on `rand` and stay reproducible.
fn permutations<T: Clone>(items: &[T], max: usize) -> Vec<Vec<T>> {
    let mut out = Vec::new();
    let mut current = items.to_vec();
    fn heap<T: Clone>(k: usize, v: &mut Vec<T>, out: &mut Vec<Vec<T>>, max: usize) {
        if out.len() >= max {
            return;
        }
        if k <= 1 {
            out.push(v.clone());
            return;
        }
        heap(k - 1, v, out, max);
        for i in 0..k - 1 {
            if k.is_multiple_of(2) {
                v.swap(i, k - 1);
            } else {
                v.swap(0, k - 1);
            }
            heap(k - 1, v, out, max);
        }
    }
    heap(current.len(), &mut current, &mut out, max);
    out
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn build_is_identical_under_reversed_artifacts() {
    let base = rich_input();
    let mut reversed = base.clone();
    reversed.artifacts.reverse();
    assert_eq!(
        build_system_model(&base, &limits()),
        build_system_model(&reversed, &limits()),
        "artifact arrival order must not change the model"
    );
}

#[test]
fn build_is_identical_under_reversed_applications() {
    let base = rich_input();
    let mut reversed = base.clone();
    reversed.applications.reverse();
    assert_eq!(
        build_system_model(&base, &limits()),
        build_system_model(&reversed, &limits())
    );
}

#[test]
fn build_is_identical_under_reversed_relationships_and_history() {
    let base = rich_input();
    let mut reversed = base.clone();
    reversed.relationships.reverse();
    reversed.history.reverse();
    assert_eq!(
        build_system_model(&base, &limits()),
        build_system_model(&reversed, &limits())
    );
}

#[test]
fn build_is_identical_under_every_artifact_permutation() {
    let base = rich_input();
    let canonical = build_system_model(&base, &limits());
    // 8 artifacts: sample permutations rather than all 40,320.
    let perms = permutations(&base.artifacts, 200);
    assert!(perms.len() > 50, "enough permutations were generated");
    for (i, perm) in perms.into_iter().enumerate() {
        let mut candidate = base.clone();
        candidate.artifacts = perm;
        let got = build_system_model(&candidate, &limits());
        assert_eq!(canonical, got, "permutation {i} changed the model");
    }
}

#[test]
fn build_is_identical_under_full_input_rotation() {
    let base = rich_input();
    let canonical = build_system_model(&base, &limits());
    for shift in 0..base.artifacts.len() {
        let mut rotated = base.clone();
        rotated.artifacts.rotate_left(shift);
        rotated
            .applications
            .rotate_left(shift % base.applications.len().max(1));
        assert_eq!(
            canonical,
            build_system_model(&rotated, &limits()),
            "rotation {shift} changed the model"
        );
    }
}

#[test]
fn queries_are_deterministic() {
    let model = build_system_model(&rich_input(), &limits());
    let key = artifact_key_for(std::path::Path::new("/shared/runtime.dll"));
    let first = shared_artifacts(&model, 16)
        .items
        .iter()
        .map(|a| a.key.clone())
        .collect::<Vec<_>>();
    let second = shared_artifacts(&model, 16)
        .items
        .iter()
        .map(|a| a.key.clone())
        .collect::<Vec<_>>();
    assert_eq!(first, second);
    assert!(!conflicting_ownership(&model, 16).items.is_empty() || first.len() <= 1);
    let _ = applications_for_artifact(&model, &key, 8);
}

// ---------------------------------------------------------------------------
// Idempotence
// ---------------------------------------------------------------------------

#[test]
fn build_is_idempotent_under_duplicated_input_facts() {
    let base = rich_input();
    let canonical = build_system_model(&base, &limits());

    let mut doubled = base.clone();
    doubled.artifacts.extend(base.artifacts.iter().cloned());
    doubled
        .applications
        .extend(base.applications.iter().cloned());
    doubled
        .relationships
        .extend(base.relationships.iter().cloned());
    doubled.history.extend(base.history.iter().cloned());

    let got = build_system_model(&doubled, &limits());
    assert_eq!(
        canonical, got,
        "build(X) must equal build(X + duplicate(X)): no duplicate nodes, \
         edges, or double-counted confidence"
    );
}

#[test]
fn duplicate_artifacts_never_produce_duplicate_nodes() {
    let a = artifact("/dup/file", 1, 1, None);
    let model = build_system_model(
        &input(
            vec![a.clone(), a.clone(), a],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(model.artifact_count(), 1);
}

// ---------------------------------------------------------------------------
// Merge commutativity
// ---------------------------------------------------------------------------

#[test]
fn merge_is_commutative_over_input_fragments() {
    // Splitting one fact multiset into two fragments that TOGETHER cover the
    // same facts, in either order, must not change the model: no later input
    // overwrites an earlier one. Both fragments keep the same application,
    // relationship and history facts (join inputs are looked up, not
    // partitioned); only the artifact halves swap.
    let base = rich_input();
    let mid = base.artifacts.len() / 2;
    let mut left = base.clone();
    left.artifacts = base.artifacts[..mid].to_vec();
    let mut right = base.clone();
    right.artifacts = base.artifacts[mid..].to_vec();

    let mut ab = left.clone();
    ab.artifacts.extend(right.artifacts.iter().cloned());
    let mut ba = right.clone();
    ba.artifacts.extend(left.artifacts.iter().cloned());

    assert_eq!(
        build_system_model(&ab, &limits()),
        build_system_model(&base, &limits()),
        "fragment order must not change the model"
    );
    assert_eq!(
        build_system_model(&ba, &limits()),
        build_system_model(&base, &limits()),
        "fragment order must not change the model"
    );
}

// ---------------------------------------------------------------------------
// Identity preservation
// ---------------------------------------------------------------------------

#[test]
fn wide_identity_distinct_high_bits_stay_distinct() {
    let model = build_system_model(
        &input(
            vec![
                artifact("/p/narrow", 7, 100, None),
                artifact("/p/wide-a", 7, 100, Some(1)),
                artifact("/p/wide-b", 7, 100, Some(2)),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    let get = |p: &str| {
        model
            .artifact(&artifact_key_for(std::path::Path::new(p)))
            .unwrap()
            .identity
    };
    assert_eq!(get("/p/narrow"), Some(ObjectIdentity::narrow(7, 100)));
    assert_ne!(
        get("/p/narrow"),
        get("/p/wide-a"),
        "provability is part of identity"
    );
    assert_ne!(
        get("/p/wide-a"),
        get("/p/wide-b"),
        "high bits distinguish objects"
    );
    assert_eq!(
        get("/p/wide-a"),
        Some(ObjectIdentity {
            volume: 7,
            file_id: 100,
            file_id_hi: Some(1)
        })
    );
}

#[test]
fn unproven_identity_is_never_fabricated_from_a_path() {
    let model = build_system_model(
        &input(
            vec![artifact_no_identity("/no/id")],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert!(model.artifacts()[0].identity.is_none());
    // And the model records the uncertainty as a blocker on every candidate.
    for c in model.candidates() {
        assert!(c
            .blockers
            .contains(&coresight_system_model::InsightBlocker::UnprovenObjectIdentity));
    }
}

// ---------------------------------------------------------------------------
// Correlation conservation
// ---------------------------------------------------------------------------

#[test]
fn correlated_evidence_cannot_exceed_its_ceiling() {
    // Three items from ONE source record must not exceed that record's
    // ceiling, however many modules forwarded them.
    let mut fact = app_fact("App", Some("Vendor"), &["/opt/app"], &[]);
    for extra in ["/opt/app/a", "/opt/app/b", "/opt/app/c"] {
        let record_app = fact.record.clone();
        fact.associations.push((
            PathBuf::from(extra),
            install_evidence(
                &record_app,
                extra,
                coresight_apps::EvidenceStrength::Direct,
                EvidenceKind::InstallLocation,
                CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
            ),
        ));
    }
    let model = build_system_model(
        &input(
            vec![
                dir("/opt/app"),
                artifact("/opt/app/a", 1, 1, None),
                artifact("/opt/app/b", 1, 2, None),
                artifact("/opt/app/c", 1, 3, None),
            ],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert!(model.check_invariants().is_ok());
    // Every claim stays within the documented strength vocabulary; nothing
    // exceeds Direct, and no claim became credible from weak evidence alone.
    for e in model.edges() {
        for ev in &e.evidence {
            assert!(ev.strength <= coresight_apps::EvidenceStrength::Direct);
            assert!(ev.strength <= ev.kind.max_strength());
            assert!(ev.strength <= ev.correlation_group.ceiling());
        }
    }
}

#[test]
fn name_derived_evidence_stays_weak_through_the_whole_pipeline() {
    let fact = app_fact_weak("Named", &["/data/named"]);
    let model = build_system_model(
        &input(
            vec![artifact("/data/named", 1, 1, None)],
            vec![fact],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    let key = artifact_key_for(std::path::Path::new("/data/named"));
    let node = model.artifact(&key).unwrap();
    // Weak evidence is not credible, so the artifact is not "associated".
    assert_ne!(
        node.application_status,
        ArtifactApplicationStatus::Associated
    );
    assert_eq!(node.credible_claimants, 0);
    assert!(model
        .edges()
        .iter()
        .all(|e| e.kind != SystemEdgeKind::OwnedBy));
}

// ---------------------------------------------------------------------------
// Bound preservation
// ---------------------------------------------------------------------------

#[test]
fn every_bound_is_respected_and_counted() {
    // Many artifacts, applications and relationships against small limits.
    let mut arts: Vec<_> = (0..500)
        .map(|i| artifact(&format!("/bulk/f{i:04}"), 1, i as u64, None))
        .collect();
    arts.push(artifact("/bulk/shared", 1, 99_999, None));

    let mut apps: Vec<_> = (0..100)
        .map(|i| app_fact(&format!("App{i:03}"), None, &[], &["/bulk/shared"]))
        .collect();
    // One application claiming thousands of artifacts.
    let greedy = app_fact(
        "Greedy",
        None,
        &[],
        &(0..200)
            .map(|i| format!("/bulk/f{i:04}"))
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    apps.push(greedy);

    let rels: Vec<_> = (0..200)
        .map(|i| content_duplicate(&[&format!("/bulk/f{i:04}"), "/bulk/shared"], "same"))
        .collect();

    let limits = SystemModelLimits {
        max_artifacts: 64,
        max_applications: 8,
        max_edges: 128,
        max_evidence_per_edge: 3,
        max_edges_per_node: 16,
        max_historical_context: 8,
        max_insights: 16,
        max_candidates: 16,
        max_source_states: 64,
    };
    let model = build_system_model(
        &input(arts, apps, rels, vec![], complete_coverage()),
        &limits,
    );

    assert!(model.artifact_count() <= limits.max_artifacts);
    assert!(model.application_count() <= limits.max_applications);
    assert!(model.edge_count() <= limits.max_edges);
    assert!(model.historical_context().len() <= limits.max_historical_context);
    assert!(model.insights().len() <= limits.max_insights);
    assert!(model.candidates().len() <= limits.max_candidates);
    for e in model.edges() {
        assert!(e.evidence.len() <= limits.max_evidence_per_edge);
    }
    // Every node respects the per-node edge bound.
    for a in model.artifact_nodes() {
        assert!(
            model.edges_for_node(&a.key).len() <= limits.max_edges_per_node,
            "node {} exceeds the per-node edge bound",
            a.key
        );
    }
    // And the truncation accounting is non-zero where things were dropped.
    assert!(model.truncation().artifacts_truncated > 0);
    assert!(model.truncation().applications_truncated > 0);
    assert!(model.check_invariants().is_ok());
}

#[test]
fn zero_evidence_retention_never_publishes_unproven_high_confidence_roles() {
    let path = "/zero-evidence/app.exe";
    let mut app = app_fact("ZeroEvidence", Some("V"), &[], &[path]);
    app.record.executable_path = Some(path.into());
    app.executable = Some(path.into());
    let limits = SystemModelLimits {
        max_evidence_per_edge: 0,
        ..SystemModelLimits::default()
    };
    let model = build_system_model(
        &input(
            vec![artifact(path, 1, 5, None)],
            vec![app],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits,
    );
    assert!(model.truncation().evidence_truncated > 0);
    assert!(model
        .edges()
        .iter()
        .all(|edge| edge.kind != SystemEdgeKind::OwnedBy
            && edge.kind != SystemEdgeKind::ApplicationExecutable));
    assert!(model.check_invariants().is_ok());
}

#[test]
fn zero_limits_produce_an_empty_but_valid_model() {
    let limits = SystemModelLimits {
        max_artifacts: 0,
        max_applications: 0,
        max_edges: 0,
        max_evidence_per_edge: 0,
        max_edges_per_node: 0,
        max_historical_context: 0,
        max_insights: 0,
        max_candidates: 0,
        max_source_states: 0,
    };
    let model = build_system_model(&rich_input(), &limits);
    assert_eq!(model.artifact_count(), 0);
    assert_eq!(model.application_count(), 0);
    assert_eq!(model.edge_count(), 0);
    assert!(model.check_invariants().is_ok());
    assert!(model.truncation().artifacts_truncated > 0);
}

// ---------------------------------------------------------------------------
// Observation preservation
// ---------------------------------------------------------------------------

#[test]
fn denied_unsupported_and_unavailable_are_never_empty() {
    let model = build_system_model(
        &input(
            vec![
                artifact("/ok", 1, 1, None),
                denied_artifact("/denied"),
                {
                    let mut a = artifact_no_identity("/unsup");
                    a.access = AccessState::Unsupported;
                    a
                },
                {
                    let mut a = artifact_no_identity("/failed");
                    a.access = AccessState::Failed;
                    a
                },
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(model.observations().inaccessible_artifacts, 1);
    assert_eq!(model.observations().unsupported_artifacts, 1);
    assert_eq!(model.observations().failed_artifacts, 1);

    let states: Vec<ProvenanceState> = model
        .artifact_nodes()
        .iter()
        .map(|a| a.provenance)
        .collect();
    assert!(states.contains(&ProvenanceState::Unavailable));
    assert!(states.contains(&ProvenanceState::Unsupported));
    assert!(states.contains(&ProvenanceState::Failed));
    // None of these is collapsed into "Observed (empty)".
    assert!(states.contains(&ProvenanceState::Observed));
    assert_ne!(
        ProvenanceState::from_access(AccessState::ExistsButInaccessible),
        ProvenanceState::from_access(AccessState::Empty)
    );
    assert_ne!(
        ProvenanceState::from_access(AccessState::Unsupported),
        ProvenanceState::from_access(AccessState::Empty)
    );
}

// ---------------------------------------------------------------------------
// Index consistency
// ---------------------------------------------------------------------------

#[test]
fn indexes_cannot_diverge_from_the_canonical_sets() {
    let model = build_system_model(&rich_input(), &limits());
    assert!(model.check_invariants().is_ok());
    assert!(model.check_invariants().is_ok());

    // Every canonical artifact is discoverable through its own key.
    for a in model.artifact_nodes() {
        assert_eq!(model.artifact(&a.key).map(|x| &x.key), Some(&a.key));
    }
    // Every identity is discoverable through the object index.
    for a in model.artifact_nodes() {
        if let Some(object) = a.identity {
            let found = model.artifacts_sharing_object(object);
            assert!(
                found.contains(&a.key.as_str()),
                "artifact {} not indexed by its object identity",
                a.key
            );
        }
    }
    // Every application is discoverable through its id.
    for app in model.applications() {
        assert!(model.application(&app.id).is_some());
    }
    // No duplicate index entries.
    for a in model.artifact_nodes() {
        if let Some(content) = &a.content_sha256 {
            let found = model.artifacts_sharing_content(content);
            let mut sorted = found.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(found, sorted, "index entries must be unique");
        }
    }
}

#[test]
fn every_indexed_edge_touches_its_node() {
    let model = build_system_model(&rich_input(), &limits());
    for a in model.artifact_nodes() {
        for e in model.edges_for_node(&a.key) {
            assert!(
                e.from == a.key || e.to == a.key,
                "edge {} → {} indexed under {}",
                e.from,
                e.to,
                a.key
            );
        }
    }
    for app in model.applications() {
        for e in model.edges_for_node(&app.id.0) {
            assert!(e.from == app.id.0 || e.to == app.id.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Conflict preservation
// ---------------------------------------------------------------------------

#[test]
fn conflicts_survive_regardless_of_arrival_order() {
    let contested = "/c/x";
    let a = app_fact("A", None, &[], &[contested]);
    let b = app_fact("B", None, &[], &[contested]);
    for apps in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
        let model = build_system_model(
            &input(
                vec![artifact(contested, 1, 1, None)],
                apps,
                vec![],
                vec![],
                complete_coverage(),
            ),
            &limits(),
        );
        let key = artifact_key_for(std::path::Path::new(contested));
        assert_eq!(
            model.artifact(&key).unwrap().application_status,
            ArtifactApplicationStatus::Conflicting
        );
        // Both claims remain represented — no silent overwrite.
        let owners = coresight_system_model::owning_applications(&model, &key, 8);
        assert_eq!(owners.items.len(), 2);
    }
}

// ---------------------------------------------------------------------------
// Adversarial fixtures
// ---------------------------------------------------------------------------

#[test]
fn case_variants_are_distinct_paths_not_distinct_objects_by_accident() {
    let model = build_system_model(
        &input(
            vec![
                artifact("/Data/Name", 1, 1, None),
                artifact("/data/name", 1, 2, None),
            ],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    // Two distinct paths → two nodes (no case folding).
    assert_eq!(model.artifact_count(), 2);
    assert!(model.check_invariants().is_ok());
}

#[test]
fn very_long_paths_are_handled_without_loss() {
    let long = format!("/{}/file", "a".repeat(2000));
    let model = build_system_model(
        &input(
            vec![artifact(&long, 1, 1, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(model.artifact_count(), 1);
    assert_eq!(model.artifacts()[0].path, PathBuf::from(&long));
}

#[test]
fn same_name_different_publisher_and_same_publisher_different_name_stay_distinct() {
    let a = app_fact("Same", Some("Pub One"), &[], &["/s/a"]);
    let b = app_fact("Same", Some("Pub Two"), &[], &["/s/b"]);
    let c = app_fact("Other", Some("Pub One"), &[], &["/s/c"]);
    let model = build_system_model(
        &input(
            vec![
                artifact("/s/a", 1, 1, None),
                artifact("/s/b", 1, 2, None),
                artifact("/s/c", 1, 3, None),
            ],
            vec![a, b, c],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(
        model.application_count(),
        3,
        "logical identity is (name, publisher)"
    );
    let ids: Vec<&ApplicationId> = model.applications().iter().map(|a| &a.id).collect();
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
}

#[test]
fn many_applications_pointing_at_one_artifact_stays_bounded_and_conflicting() {
    let apps: Vec<_> = (0..50)
        .map(|i| app_fact(&format!("Many{i:02}"), None, &[], &["/one/artifact"]))
        .collect();
    let model = build_system_model(
        &input(
            vec![artifact("/one/artifact", 1, 1, None)],
            apps,
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    let key = artifact_key_for(std::path::Path::new("/one/artifact"));
    assert_eq!(
        model.artifact(&key).unwrap().application_status,
        ArtifactApplicationStatus::Conflicting
    );
    assert_eq!(model.artifact(&key).unwrap().credible_claimants, 50);
    assert!(model.check_invariants().is_ok());
    // The conflict is reported once, not once per pair of applications.
    let conflicts = model
        .insights()
        .iter()
        .filter(|i| i.kind == InsightKind::ConflictingOwnership)
        .count();
    assert_eq!(conflicts, 1);
}

#[test]
fn source_ordering_does_not_change_the_model() {
    let base = rich_input();
    let mut shuffled = base.clone();
    shuffled.source_coverage = vec![
        coresight_apps::SourceCoverage::with_status(
            "msix-appx",
            SourceStatus::Unsupported,
            Some("not implemented".into()),
        ),
        coresight_apps::SourceCoverage::complete("win32-uninstall"),
    ];
    let model = build_system_model(&shuffled, &limits());
    // Source states are canonically ordered regardless of input order.
    let names: Vec<&str> = model
        .observations()
        .source_states
        .iter()
        .map(|s| s.source.as_str())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

// ---------------------------------------------------------------------------
// Classification coexistence
// ---------------------------------------------------------------------------

#[test]
fn classification_and_application_association_coexist_without_overwriting() {
    let model = build_system_model(&rich_input(), &limits());
    let cache_key = artifact_key_for(std::path::Path::new("/opt/a/cache"));
    let node = model.artifact(&cache_key).unwrap();
    assert_eq!(node.category, Some(Category::Cache));
    assert!(node.classification_confidence.is_some());
    // The classification index agrees, and the association exists alongside.
    let by_cat = artifacts_of_classification(&model, Category::Cache, 16);
    assert!(by_cat.items.iter().any(|n| n.key == cache_key));
    assert_eq!(
        model.artifact(&cache_key).unwrap().category,
        Some(Category::Cache),
        "association must not overwrite classification"
    );
}

#[test]
fn strongly_associated_query_only_returns_credible_associations() {
    let model = build_system_model(&rich_input(), &limits());
    let strong = strongly_associated_artifacts(&model, 64);
    for node in strong.items {
        assert!(node.credible_claimants > 0);
        assert!(node.application_status != ArtifactApplicationStatus::Uncertain);
    }
}

// ---------------------------------------------------------------------------
// History context
// ---------------------------------------------------------------------------

#[test]
fn history_context_is_only_projected_never_invented() {
    use coresight_system_model::HistoricalRelation;
    // No history facts supplied → no historical context and no historical
    // assertions, even though artifacts exist.
    let model = build_system_model(
        &input(
            vec![artifact("/h/file", 1, 1, None)],
            vec![],
            vec![],
            vec![],
            complete_coverage(),
        ),
        &limits(),
    );
    assert!(model.historical_context().is_empty());
    assert!(model.historical_assertions().is_empty());

    // With a history fact whose identity DIFFERS, a move is reported — as
    // a node-attached assertion quoting the stored record, never as a
    // self-loop edge.
    let model = build_system_model(
        &input(
            vec![artifact("/h/file", 1, 1, None)],
            vec![],
            vec![],
            vec![history(
                "run-9",
                "/h/file",
                Some(ObjectIdentity::narrow(1, 42)),
            )],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(model.historical_context().len(), 1);
    assert_eq!(model.historical_assertions().len(), 1);
    let assertion = &model.historical_assertions()[0];
    assert_eq!(assertion.relation, HistoricalRelation::ObjectReplaced);
    assert_eq!(
        assertion.recorded_identity,
        Some(ObjectIdentity::narrow(1, 42))
    );
    assert_eq!(
        assertion.current_identity,
        Some(ObjectIdentity::narrow(1, 1))
    );
    assert_eq!(assertion.provenance, ProvenanceState::Observed);

    // Equal proven identities → an alias observation, not a move.
    let model = build_system_model(
        &input(
            vec![artifact("/h/file", 1, 1, None)],
            vec![],
            vec![],
            vec![history(
                "run-9",
                "/h/file",
                Some(ObjectIdentity::narrow(1, 1)),
            )],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(
        model.historical_assertions()[0].relation,
        HistoricalRelation::SameObjectObserved
    );

    // Unproven on either side → honestly unproven, never asserted same.
    let model = build_system_model(
        &input(
            vec![artifact_no_identity("/h/bare")],
            vec![],
            vec![],
            vec![history("run-9", "/h/bare", None)],
            complete_coverage(),
        ),
        &limits(),
    );
    assert_eq!(
        model.historical_assertions()[0].relation,
        HistoricalRelation::IdentityUnproven
    );
    assert!(model.check_invariants().is_ok());
}

#[test]
fn history_never_fabricates_an_assertion_for_an_unknown_artifact() {
    let model = build_system_model(
        &input(
            vec![],
            vec![],
            vec![],
            vec![history("run-1", "/never/observed", None)],
            complete_coverage(),
        ),
        &limits(),
    );
    // The context record exists, but no assertion is invented for an
    // artifact the current model does not hold.
    assert_eq!(model.historical_context().len(), 1);
    assert!(model.historical_assertions().is_empty());
}

// ---------------------------------------------------------------------------
// Serialization round trip
// ---------------------------------------------------------------------------

#[test]
fn the_model_round_trips_through_serde_without_losing_semantics() {
    let model = build_system_model(&rich_input(), &limits());
    let json = serde_json::to_string(&model).expect("serializes");
    let back: SystemModel = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(
        model, back,
        "identity, provenance and evidence survive the round trip"
    );
    assert!(back.check_invariants().is_ok());
}

/// Non-UTF-8 paths must survive the whole system-model path byte-for-byte.
#[cfg(unix)]
#[test]
fn non_utf8_paths_survive_the_system_model_losslessly() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let raw = b"/data/\xff\xfe/bin";
    let path = PathBuf::from(OsStr::from_bytes(raw));
    let mut fact = artifact_no_identity("/data/placeholder");
    fact.path = path.clone();

    let mut app = app_fact("Weird", None, &[], &[]);
    app.record.id = ApplicationId::derive("Weird", None);
    app.associations.push((
        path.clone(),
        coresight_apps::OwnershipEvidence::new(
            EvidenceKind::InstallLocation,
            coresight_apps::EvidenceSource::InventoryRecord,
            coresight_apps::EvidenceStrength::Direct,
            CorrelationGroup::SourceRecord(ApplicationSource::FilesystemPresence),
            coresight_apps::AssociationScope::ThisMachine,
            path.clone(),
            coresight_apps::MatchedAttribute::InstallLocation,
            Some("Weird".into()),
        ),
    ));

    let model = build_system_model(
        &input(vec![fact], vec![app], vec![], vec![], complete_coverage()),
        &limits(),
    );
    assert_eq!(model.artifact_count(), 1);
    assert_eq!(
        model.artifacts()[0].path.as_os_str().as_bytes(),
        raw,
        "the exact bytes survive into the model"
    );
    let key = artifact_key_for(&path);
    assert_eq!(
        model.artifact(&key).unwrap().path.as_os_str().as_bytes(),
        raw
    );
    // And the key is a lossless encoding of those bytes.
    assert_eq!(
        coresight_system_model::ArtifactKey::of(&path)
            .decode()
            .unwrap()
            .as_os_str()
            .as_bytes(),
        raw
    );
}
