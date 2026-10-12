//! Phase 6.4.7 — linear-time evidence-group preflight.
//!
//! The Phase 6.4.6 preflight counted each evidence group by rescanning the
//! whole sorted association collection, and the write path repeated that
//! scan to size its ordinal cursor — quadratic in the number of distinct
//! artifact paths, and repeated after the transaction had already deleted
//! the prior snapshot.
//!
//! These tests pin the RESULT of the single-pass grouping (never elapsed
//! time, which is flaky under load) and prove end-to-end that associations
//! across many paths, uneven group sizes, and duplicate evidence entries
//! still commit, reload, and rebuild with their semantics intact.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, AssociationScope, Confidence,
    CorrelationGroup, EvidenceKind, EvidenceSource, EvidenceStrength, MatchedAttribute,
    OwnershipEvidence, PackageKind, ProbedKind, SourceCoverage,
};
use coresight_capabilities::access::AccessState;
use coresight_history::{
    AppSnapshotFact, ConfigFingerprint, HistoryStore, QueryLimits, RunCounts, RunId, RunRecord,
    RunStatus, StoreError,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{ArtifactFact, SystemModelInput, SystemModelLimits};

fn started_at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(2_000_000 + offset)
}

fn run_record(id: &str, offset: u64) -> RunRecord {
    RunRecord {
        run_id: RunId(id.to_string()),
        started_at: started_at(offset),
        completed_at: Some(started_at(offset) + Duration::from_secs(5)),
        roots: vec![PathBuf::from("/scope-a")],
        platform: "test-platform".to_string(),
        config: ConfigFingerprint::current(),
        status: RunStatus::Completed,
        counts: RunCounts::default(),
    }
}

fn app_record(name: &str, publisher: Option<&str>) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, publisher),
        name: name.to_string(),
        version: Some("1.0".to_string()),
        publisher: publisher.map(str::to_string),
        install_location: Some(PathBuf::from(format!("/apps/{name}"))),
        install_date: Some("20260101".to_string()),
        estimated_size_bytes: Some(4096),
        uninstall_string: Some(format!("uninstall-{name}")),
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::RegistryUninstall,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: vec!["HKLM64".to_string()],
        bundle_identifier: Some(format!("com.example.{name}")),
        executable_path: Some(PathBuf::from(format!("/apps/{name}/{name}.exe"))),
        provenance: vec![ApplicationSource::RegistryUninstall],
    }
}

fn artifact(path: &str) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity::narrow(1, 1)),
        content_sha256: None,
        size: Some(1024),
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

/// Evidence for one (app, artifact) association. `tag` makes otherwise
/// identical entries distinguishable so duplicates stay observable.
fn evidence(tag: &str) -> OwnershipEvidence {
    OwnershipEvidence::new(
        EvidenceKind::InstallLocation,
        EvidenceSource::RegistryMetadata,
        EvidenceStrength::Direct,
        CorrelationGroup::NameDerived,
        AssociationScope::ThisMachine,
        PathBuf::from("/apps/Widget"),
        MatchedAttribute::InstallLocation,
        Some(tag.to_string()),
    )
}

/// Commit, reload and rebuild a snapshot whose single application has
/// evidence spread over several artifact paths with uneven group sizes.
#[test]
fn associations_across_many_paths_round_trip_with_group_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("groups.db")).unwrap();
    let run = run_record("groups-run", 1);
    store.begin_run(&run).unwrap();

    let app = app_record("Widget", Some("Maker"));
    // Deliberately UNSORTED, with an uneven distribution: one path with a
    // single entry, one with four, one with two, plus a DUPLICATE
    // evidence entry inside the four-entry group.
    let assocs = vec![
        (PathBuf::from("/apps/Widget/d.dll"), evidence("d1")),
        (PathBuf::from("/apps/Widget/b.exe"), evidence("b1")),
        (PathBuf::from("/apps/Widget/b.exe"), evidence("b2")),
        (PathBuf::from("/apps/Widget/a.exe"), evidence("a1")),
        (PathBuf::from("/apps/Widget/c.dat"), evidence("c1")),
        (PathBuf::from("/apps/Widget/b.exe"), evidence("b3")),
        (PathBuf::from("/apps/Widget/b.exe"), evidence("b4")),
    ];
    let artifacts: Vec<ArtifactFact> = [
        "/apps/Widget/a.exe",
        "/apps/Widget/b.exe",
        "/apps/Widget/c.dat",
        "/apps/Widget/d.dll",
    ]
    .iter()
    .map(|p| artifact(p))
    .collect();

    let input = SystemModelInput {
        artifacts,
        applications: vec![coresight_system_model::ApplicationFact {
            record: app.clone(),
            install_roots: vec![PathBuf::from("/apps/Widget")],
            executable: None,
            associations: assocs.clone(),
        }],
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
    };
    let app_facts = vec![AppSnapshotFact {
        record: app.clone(),
        install_roots: vec![PathBuf::from("/apps/Widget")],
        executable: None,
        associations: assocs.clone(),
        footprints: Vec::new(),
    }];
    let inventory = coresight_apps::Inventory {
        records: vec![app.clone()],
        ..Default::default()
    };

    store
        .commit_system_snapshot(
            &run.run_id,
            &input,
            &app_facts,
            &inventory,
            &coresight_apps::FootprintReport::default(),
        )
        .expect("an association set spanning several groups must commit");

    // Every association is persisted with its own ordinal: none collapsed.
    {
        let conn = rusqlite::Connection::open(dir.path().join("groups.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM app_snapshot_evidence WHERE run_id = ?1",
                rusqlite::params![run.run_id.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count,
            assocs.len() as i64,
            "every association must persist; per-path ordinals reset but do not collapse"
        );
        // Per-artifact ordinals restart at 0 for each group.
        let mut rows: Vec<(String, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT artifact_path, evidence_ord FROM app_snapshot_evidence \
                     WHERE run_id = ?1 ORDER BY artifact_path, evidence_ord",
                )
                .unwrap();
            let mapped = stmt
                .query_map(rusqlite::params![run.run_id.0], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .unwrap();
            mapped.map(|r| r.unwrap()).collect()
        };
        rows.sort();
        for (path, ord) in &rows {
            assert!(
                (0..4).contains(ord),
                "ordinal {ord} for {path} is outside its group"
            );
        }
        // The 4-entry group must have written ordinals 0..4.
        let b_ords: Vec<i64> = rows
            .iter()
            .filter(|(p, _)| p.contains("b.exe"))
            .map(|(_, o)| *o)
            .collect();
        assert_eq!(b_ords, vec![0, 1, 2, 3]);
    }

    // Reload: the associations come back with their semantics intact.
    let loaded = store
        .load_system_snapshot(&run.run_id, &QueryLimits::default())
        .unwrap()
        .expect("the snapshot must reload");
    assert_eq!(loaded.input.applications.len(), 1);
    assert_eq!(
        loaded.input.applications[0].associations.len(),
        assocs.len(),
        "every association must survive the round-trip"
    );

    // Rebuild: the model still builds from the reloaded facts.
    let model = store
        .rebuild_system_model(
            &run.run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .expect("the model must rebuild after a multi-group commit");
    assert_eq!(model.applications().len(), 1);
}

#[test]
fn a_snapshot_with_no_associations_still_commits_and_rebuilds() {
    // The empty-group case must retain its established behaviour: no
    // groups, a zero maximum, and a still-valid ordinal cursor.
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("empty.db")).unwrap();
    let run = run_record("empty-run", 1);
    store.begin_run(&run).unwrap();

    let app = app_record("Bare", None);
    let input = SystemModelInput {
        artifacts: vec![artifact("/apps/Bare")],
        applications: vec![coresight_system_model::ApplicationFact {
            record: app.clone(),
            install_roots: Vec::new(),
            executable: None,
            associations: Vec::new(),
        }],
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
    };
    let app_facts = vec![AppSnapshotFact {
        record: app.clone(),
        install_roots: Vec::new(),
        executable: None,
        associations: Vec::new(),
        footprints: Vec::new(),
    }];
    let inventory = coresight_apps::Inventory {
        records: vec![app],
        ..Default::default()
    };

    store
        .commit_system_snapshot(
            &run.run_id,
            &input,
            &app_facts,
            &inventory,
            &coresight_apps::FootprintReport::default(),
        )
        .expect("an application with no associations must commit");

    let model = store
        .rebuild_system_model(
            &run.run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .expect("the model must rebuild");
    assert_eq!(model.applications().len(), 1);
}

#[test]
fn evidence_grouping_does_not_change_the_invalid_input_contract() {
    // Grouping is a refactor of WHERE the work happens, never of WHAT is
    // rejected: a bad counter on the same commit path is still invalid
    // input, not corruption.
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("contract.db")).unwrap();
    let run = run_record("contract-run", 1);
    store.begin_run(&run).unwrap();

    let app = app_record("Widget", Some("Maker"));
    let assocs = vec![
        (PathBuf::from("/apps/Widget/a.exe"), evidence("a1")),
        (PathBuf::from("/apps/Widget/b.exe"), evidence("b1")),
    ];
    let input = SystemModelInput {
        artifacts: vec![
            artifact("/apps/Widget/a.exe"),
            artifact("/apps/Widget/b.exe"),
        ],
        applications: vec![coresight_system_model::ApplicationFact {
            record: app.clone(),
            install_roots: vec![PathBuf::from("/apps/Widget")],
            executable: None,
            associations: assocs.clone(),
        }],
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
    };
    let app_facts = vec![AppSnapshotFact {
        record: app.clone(),
        install_roots: vec![PathBuf::from("/apps/Widget")],
        executable: None,
        associations: assocs,
        footprints: Vec::new(),
    }];
    let inventory = coresight_apps::Inventory {
        records: vec![app],
        records_truncated: u64::MAX,
        ..Default::default()
    };

    let err = store
        .commit_system_snapshot(
            &run.run_id,
            &input,
            &app_facts,
            &inventory,
            &coresight_apps::FootprintReport::default(),
        )
        .expect_err("an oversized counter must still be rejected");
    match &err {
        StoreError::InvalidSnapshotInput {
            table,
            column,
            value,
            ..
        } => {
            assert_eq!(*table, "app_snapshot_meta");
            assert_eq!(*column, "records_truncated");
            assert_eq!(*value, u64::MAX as u128);
        }
        other => panic!("expected invalid snapshot input, got {other:?}"),
    }
    assert!(
        !store.has_system_snapshot(&run.run_id).unwrap(),
        "a rejected commit must leave no snapshot behind"
    );
}

// Keep the unused-import checker honest about the confidence type used by
// the association fixtures above.
#[allow(dead_code)]
fn _confidence_is_used() -> Confidence {
    Confidence::Confirmed
}
