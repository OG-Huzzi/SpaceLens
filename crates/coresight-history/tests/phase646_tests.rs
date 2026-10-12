//! Phase 6.4.6 — application-snapshot INPUT errors and preflight closure.
//!
//! Two guarantees are pinned here:
//!
//! 1. A caller-supplied numeric value the store cannot represent is
//!    **invalid input** (`StoreError::InvalidSnapshotInput`), never
//!    persisted corruption (`StoreError::Corrupt`). Reporting a caller's
//!    `u64::MAX` size as database damage sends an operator hunting for
//!    damage that does not exist.
//! 2. That rejection happens in the PREFLIGHT — before the write
//!    transaction opens — so a rejected REPLACEMENT cannot delete or
//!    partially replace a snapshot that was already valid. Preservation is
//!    proven at the SQL level (every persisted table, byte-for-byte) and
//!    by reloading and rebuilding the original afterwards.
//!
//! Every fixture is synthetic and portable: no real filesystem path, no
//! subprocess, no machine state.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, AssociationScope, Confidence,
    CorrelationGroup, EvidenceKind, EvidenceSource, EvidenceStrength, FootprintKind,
    FootprintReport, Inventory, MatchedAttribute, OwnershipEvidence, PackageKind, ProbedKind,
    SourceCoverage,
};
use coresight_capabilities::access::AccessState;
use coresight_history::{
    AppSnapshotFact, ConfigFingerprint, HistoryStore, QueryLimits, RunCounts, RunId, RunRecord,
    RunStatus, StoreError,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    ArtifactFact, HistoryFact, RelationshipFact, RelationshipFactKind, SystemModelInput,
    SystemModelLimits,
};
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn started_at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000 + offset)
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

fn artifact(path: &str, size: Option<u64>) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity::narrow(1, 1)),
        content_sha256: None,
        size,
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

fn install_evidence(app: &ApplicationRecord, path: &str) -> OwnershipEvidence {
    OwnershipEvidence::new(
        EvidenceKind::InstallLocation,
        EvidenceSource::RegistryMetadata,
        EvidenceStrength::Direct,
        CorrelationGroup::SourceRecord(app.source.clone()),
        AssociationScope::ThisMachine,
        PathBuf::from(path),
        MatchedAttribute::InstallLocation,
        Some(app.name.clone()),
    )
    .with_matched_path(PathBuf::from(path))
}

fn coverage() -> Vec<SourceCoverage> {
    vec![SourceCoverage::complete("win32-uninstall")]
}

/// A minimal, valid single-artifact snapshot.
fn simple_input() -> SystemModelInput {
    SystemModelInput {
        artifacts: vec![artifact("/a/file.txt", Some(128))],
        applications: Vec::new(),
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: coverage(),
    }
}

/// Commit an empty-fact snapshot; returns the resulting error.
fn commit(
    store: &mut HistoryStore,
    run_id: &RunId,
    input: &SystemModelInput,
    inventory: &Inventory,
    footprint: &FootprintReport,
) -> Result<(), StoreError> {
    store.commit_system_snapshot(run_id, input, &[], inventory, footprint)
}

/// Assert the error is a typed INVALID-INPUT rejection naming the exact
/// table, column and rejected value — and explicitly NOT corruption.
fn assert_invalid_input(err: &StoreError, table: &str, column: &str, value: u128) {
    match err {
        StoreError::InvalidSnapshotInput {
            table: t,
            column: c,
            value: v,
            run_id,
        } => {
            assert_eq!(*t, table, "must name the table (got {err})");
            assert_eq!(*c, column, "must name the column (got {err})");
            assert_eq!(*v, value, "must report the rejected value (got {err})");
            assert!(
                run_id.is_some(),
                "the affected run must be reported (got {err})"
            );
        }
        StoreError::Corrupt { .. } => {
            panic!("caller input must NOT be reported as corruption, got {err}")
        }
        other => panic!("expected invalid snapshot input, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Artifact size
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_artifact_size_is_invalid_input_not_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("artifact-size.db")).unwrap();
    let run = run_record("artifact-size-run", 1);
    store.begin_run(&run).unwrap();

    let input = SystemModelInput {
        artifacts: vec![artifact("/a/huge.bin", Some(u64::MAX))],
        ..simple_input()
    };
    let err = commit(
        &mut store,
        &run.run_id,
        &input,
        &Inventory::default(),
        &FootprintReport::default(),
    )
    .expect_err("an oversized artifact size must be rejected");
    assert_invalid_input(&err, "app_snapshot_artifacts", "size", u64::MAX as u128);
    assert!(
        !store.has_system_snapshot(&run.run_id).unwrap(),
        "a rejected first commit must leave no snapshot"
    );
}

#[test]
fn the_largest_representable_artifact_size_is_accepted() {
    // The boundary must be INCLUSIVE of `i64::MAX` — the fix must not
    // narrow the accepted domain by accident.
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("artifact-max.db")).unwrap();
    let run = run_record("artifact-max-run", 1);
    store.begin_run(&run).unwrap();

    let input = SystemModelInput {
        artifacts: vec![artifact("/a/edge.bin", Some(i64::MAX as u64))],
        ..simple_input()
    };
    commit(
        &mut store,
        &run.run_id,
        &input,
        &Inventory::default(),
        &FootprintReport::default(),
    )
    .expect("i64::MAX bytes is representable and must commit");

    let loaded = store
        .load_system_snapshot(&run.run_id, &QueryLimits::default())
        .unwrap()
        .expect("the snapshot must reload");
    assert_eq!(
        loaded.input.artifacts[0].size,
        Some(i64::MAX as u64),
        "the largest representable size must round-trip exactly"
    );
}

#[test]
fn one_past_the_largest_artifact_size_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("artifact-over.db")).unwrap();
    let run = run_record("artifact-over-run", 1);
    store.begin_run(&run).unwrap();

    let over = (i64::MAX as u64) + 1;
    let input = SystemModelInput {
        artifacts: vec![artifact("/a/over.bin", Some(over))],
        ..simple_input()
    };
    let err = commit(
        &mut store,
        &run.run_id,
        &input,
        &Inventory::default(),
        &FootprintReport::default(),
    )
    .expect_err("i64::MAX + 1 bytes is unrepresentable");
    assert_invalid_input(&err, "app_snapshot_artifacts", "size", over as u128);
}

// ---------------------------------------------------------------------------
// Inventory counters (stored in app_snapshot_meta)
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_inventory_counter_is_invalid_input_not_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("inv-trunc.db")).unwrap();
    let run = run_record("inv-trunc-run", 1);
    store.begin_run(&run).unwrap();

    let inventory = Inventory {
        records_truncated: u64::MAX,
        ..Inventory::default()
    };
    let err = commit(
        &mut store,
        &run.run_id,
        &simple_input(),
        &inventory,
        &FootprintReport::default(),
    )
    .expect_err("an oversized inventory counter must be rejected");
    assert_invalid_input(
        &err,
        "app_snapshot_meta",
        "records_truncated",
        u64::MAX as u128,
    );
}

#[test]
fn an_oversized_rejected_counter_is_invalid_input_not_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("inv-rej.db")).unwrap();
    let run = run_record("inv-rej-run", 1);
    store.begin_run(&run).unwrap();

    let inventory = Inventory {
        records_rejected: u64::MAX,
        ..Inventory::default()
    };
    let err = commit(
        &mut store,
        &run.run_id,
        &simple_input(),
        &inventory,
        &FootprintReport::default(),
    )
    .expect_err("an oversized rejected counter must be rejected");
    assert_invalid_input(
        &err,
        "app_snapshot_meta",
        "records_rejected",
        u64::MAX as u128,
    );
}

// ---------------------------------------------------------------------------
// Footprint counters (stored in app_snapshot_meta)
// ---------------------------------------------------------------------------

#[test]
fn every_oversized_footprint_counter_is_invalid_input_not_corruption() {
    // All four footprint counters are caller-controlled `u64`s persisted
    // into `app_snapshot_meta`; each must carry the same typed policy.
    /// One footprint counter and the `app_snapshot_meta` column it binds.
    type CounterCase = (&'static str, fn(&mut FootprintReport), &'static str);
    let cases: [CounterCase; 4] = [
        (
            "candidates",
            |f| f.candidates_truncated = u64::MAX,
            "fp_candidates_truncated",
        ),
        (
            "children",
            |f| f.children_truncated = u64::MAX,
            "fp_children_truncated",
        ),
        ("apps", |f| f.apps_truncated = u64::MAX, "fp_apps_truncated"),
        (
            "evidence",
            |f| f.evidence_truncated = u64::MAX,
            "fp_evidence_truncated",
        ),
    ];
    for (name, set, column) in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut store = HistoryStore::open(&dir.path().join(format!("fp-{name}.db"))).unwrap();
        let run = run_record("fp-counter-run", 1);
        store.begin_run(&run).unwrap();

        let mut footprint = FootprintReport::default();
        set(&mut footprint);
        let err = commit(
            &mut store,
            &run.run_id,
            &simple_input(),
            &Inventory::default(),
            &footprint,
        )
        .expect_err("an oversized footprint counter must be rejected");
        assert_invalid_input(&err, "app_snapshot_meta", column, u64::MAX as u128);
    }
}

#[test]
fn the_largest_representable_footprint_counter_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("fp-max.db")).unwrap();
    let run = run_record("fp-max-run", 1);
    store.begin_run(&run).unwrap();

    let footprint = FootprintReport {
        candidates_truncated: i64::MAX as u64,
        ..FootprintReport::default()
    };
    commit(
        &mut store,
        &run.run_id,
        &simple_input(),
        &Inventory::default(),
        &footprint,
    )
    .expect("i64::MAX is representable and must commit");

    let loaded = store
        .load_system_snapshot(&run.run_id, &QueryLimits::default())
        .unwrap()
        .expect("the snapshot must reload");
    assert_eq!(
        loaded.footprint.candidates_truncated,
        i64::MAX as u64,
        "the largest representable counter must round-trip exactly"
    );
}

// ---------------------------------------------------------------------------
// Application estimated size
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_estimated_size_is_invalid_input_not_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("est-size.db")).unwrap();
    let run = run_record("est-size-run", 1);
    store.begin_run(&run).unwrap();

    let mut record = app_record("Widget", Some("Maker"));
    record.estimated_size_bytes = Some(u64::MAX);
    let evidence = install_evidence(&record, "/apps/Widget");
    let fact = AppSnapshotFact {
        record: record.clone(),
        install_roots: vec![PathBuf::from("/apps/Widget")],
        executable: None,
        associations: vec![(PathBuf::from("/apps/Widget"), evidence)],
        footprints: Vec::new(),
    };
    let input = SystemModelInput {
        artifacts: vec![artifact("/apps/Widget", Some(10))],
        applications: vec![coresight_system_model::ApplicationFact {
            record,
            install_roots: fact.install_roots.clone(),
            executable: None,
            associations: fact.associations.clone(),
        }],
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: coverage(),
    };
    let inventory = Inventory {
        records: vec![fact.record.clone()],
        ..Inventory::default()
    };

    let err = store
        .commit_system_snapshot(
            &run.run_id,
            &input,
            std::slice::from_ref(&fact),
            &inventory,
            &FootprintReport::default(),
        )
        .expect_err("an oversized estimated size must be rejected");
    assert_invalid_input(
        &err,
        "app_snapshot_apps",
        "estimated_size",
        u64::MAX as u128,
    );
}

// ---------------------------------------------------------------------------
// Failed replacement preserves the previous snapshot, byte-for-byte
// ---------------------------------------------------------------------------

/// Dump every persisted snapshot table as ordered text: each table's own
/// DDL plus every row value, so two states compare exactly rather than by
/// row count.
fn dump_snapshot_tables(db: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(db).unwrap();
    let mut out = Vec::new();
    for table in [
        "app_snapshot_meta",
        "app_snapshot_artifacts",
        "app_snapshot_apps",
        "app_snapshot_provenance",
        "app_snapshot_views",
        "app_snapshot_roots",
        "app_snapshot_evidence",
        "app_snapshot_footprints",
        "app_snapshot_footprint_evidence",
        "app_snapshot_coverage",
        "app_snapshot_relationships",
        "app_snapshot_rel_members",
        "app_snapshot_history",
    ] {
        let sql: String = conn
            .query_row(
                "SELECT COALESCE(sql, '') FROM sqlite_master WHERE type = 'table' AND name = ?1",
                rusqlite::params![table],
                |r| r.get(0),
            )
            .unwrap_or_default();

        let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let cols = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |row| {
                let mut parts = Vec::with_capacity(cols);
                for i in 0..cols {
                    let v: rusqlite::types::Value = row.get(i)?;
                    parts.push(format!("{v:?}"));
                }
                Ok(parts.join("|"))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // Deterministic, scan-order-independent comparison.
        rows.sort();
        out.push((format!("{table}\n{sql}"), rows.join("\n")));
    }
    out
}

/// Count the persisted rows across the snapshot tables.
fn total_rows(dump: &[(String, String)]) -> usize {
    dump.iter()
        .map(|(_, rows)| rows.lines().filter(|l| !l.is_empty()).count())
        .sum()
}

#[test]
fn a_rejected_replacement_preserves_every_persisted_table_byte_for_byte() {
    // Commit a valid snapshot, attempt replacements carrying each category
    // of unrepresentable caller value, and prove NOT ONE byte of the
    // previously valid snapshot changed — including metadata and children.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("preserve.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let run = run_record("preserve-run", 1);
    store.begin_run(&run).unwrap();

    let input = simple_input();
    commit(
        &mut store,
        &run.run_id,
        &input,
        &Inventory::default(),
        &FootprintReport::default(),
    )
    .expect("the valid snapshot must commit");

    let before = dump_snapshot_tables(&db);
    let before_model = store
        .rebuild_system_model(
            &run.run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .expect("the original must rebuild before the rejected replacement");
    assert!(total_rows(&before) > 0, "the fixture must persist rows");

    let big = SystemModelInput {
        artifacts: vec![artifact("/a/huge.bin", Some(u64::MAX))],
        ..input.clone()
    };
    let inv = Inventory {
        records_truncated: u64::MAX,
        ..Inventory::default()
    };
    let fp = FootprintReport {
        evidence_truncated: u64::MAX,
        ..FootprintReport::default()
    };

    let rejections: [(&str, SystemModelInput, Inventory, FootprintReport); 3] = [
        (
            "artifact size",
            big,
            Inventory::default(),
            FootprintReport::default(),
        ),
        (
            "inventory counter",
            input.clone(),
            inv,
            FootprintReport::default(),
        ),
        ("footprint counter", input.clone(), Inventory::default(), fp),
    ];

    for (name, bad_input, bad_inv, bad_fp) in rejections {
        let err = commit(&mut store, &run.run_id, &bad_input, &bad_inv, &bad_fp)
            .expect_err("an unrepresentable value must reject the commit");
        assert!(
            matches!(err, StoreError::InvalidSnapshotInput { .. }),
            "{name} must be rejected as invalid input, got {err:?}"
        );
        assert_eq!(
            before,
            dump_snapshot_tables(&db),
            "a rejected {name} replacement must not alter ANY persisted table"
        );
        let after_model = store
            .rebuild_system_model(
                &run.run_id,
                &QueryLimits::default(),
                &SystemModelLimits::default(),
            )
            .unwrap()
            .expect("the original must still rebuild after a rejected replacement");
        assert_eq!(
            before_model, after_model,
            "a rejected {name} replacement must not change the rebuilt model"
        );
    }

    assert!(
        store.has_system_snapshot(&run.run_id).unwrap(),
        "the original snapshot must still be present"
    );
}

#[test]
fn a_rejected_replacement_preserves_a_populated_snapshots_children() {
    // The preservation proof above uses a sparse snapshot. Repeat it with
    // applications, footprints, coverage, relationships and history, so
    // the child tables actually hold rows that could have been deleted —
    // and make the oversize land in the SECOND artifact, which the old
    // inline-conversion path would only discover mid-insert.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("preserve-full.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let run = run_record("preserve-full-run", 1);
    store.begin_run(&run).unwrap();

    let app = app_record("Alpha", Some("Acme"));
    let root = "/apps/Alpha";
    let cache = "/apps/Alpha/cache";
    // A PROVEN content-duplicate pair: two distinct objects carrying one
    // verified digest, so the relationship fact is genuinely valid (the
    // store validates relationship facts, and would reject a fabricated
    // one for a reason unrelated to this phase).
    const SHARED_DIGEST: &str = "aaaa0000bbbb1111cccc2222dddd3333eeee4444ffff5555aaaa6666bbbb7777";
    let duplicate_a = artifact(root, Some(4096));
    let mut duplicate_b = artifact(cache, Some(2048));
    duplicate_b.content_sha256 = Some(SHARED_DIGEST.to_string());
    let mut duplicate_a = duplicate_a;
    duplicate_a.content_sha256 = Some(SHARED_DIGEST.to_string());
    let input = SystemModelInput {
        artifacts: vec![duplicate_a, duplicate_b, artifact("/apps/Beta", Some(1024))],
        applications: vec![coresight_system_model::ApplicationFact {
            record: app.clone(),
            install_roots: vec![PathBuf::from(root)],
            executable: None,
            associations: vec![(PathBuf::from(root), install_evidence(&app, root))],
        }],
        relationships: vec![RelationshipFact {
            kind: RelationshipFactKind::ContentDuplicate,
            paths: vec![PathBuf::from(root), PathBuf::from(cache)],
            object: None,
            content_sha256: Some(SHARED_DIGEST.to_string()),
        }],
        history: vec![HistoryFact {
            run_id: "older-run".to_string(),
            path: PathBuf::from(root),
            identity: Some(ObjectIdentity::narrow(1, 1)),
            category: None,
        }],
        source_coverage: vec![
            SourceCoverage::complete("win32-uninstall"),
            SourceCoverage::with_status(
                "wmi",
                coresight_apps::SourceStatus::Partial,
                Some("one view failed".to_string()),
            ),
        ],
    };
    let app_facts = vec![AppSnapshotFact {
        record: app.clone(),
        install_roots: vec![PathBuf::from(root)],
        executable: None,
        associations: vec![(PathBuf::from(root), install_evidence(&app, root))],
        footprints: vec![coresight_apps::FootprintCandidate {
            path: PathBuf::from(root),
            app: app.id.clone(),
            kind: FootprintKind::InstallationDirectory,
            confidence: Confidence::Confirmed,
            evidence: vec![coresight_apps::FootprintEvidence::new(
                EvidenceKind::InstallLocation,
                Confidence::Confirmed,
                "registry",
                AssociationScope::ThisMachine,
                "the installer recorded this location",
            )],
        }],
    }];
    let inventory = Inventory {
        records: vec![app.clone()],
        ..Inventory::default()
    };
    let footprint = FootprintReport {
        candidates: app_facts[0].footprints.clone(),
        ..FootprintReport::default()
    };

    store
        .commit_system_snapshot(&run.run_id, &input, &app_facts, &inventory, &footprint)
        .expect("the valid snapshot must commit");

    let before = dump_snapshot_tables(&db);
    let before_model = store
        .rebuild_system_model(
            &run.run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .expect("the original must rebuild");
    // Sanity: the child tables really do carry rows in this fixture.
    let child_rows: usize = before
        .iter()
        .filter(|(name, _)| {
            name.starts_with("app_snapshot_coverage")
                || name.starts_with("app_snapshot_relationships")
                || name.starts_with("app_snapshot_rel_members")
                || name.starts_with("app_snapshot_history")
                || name.starts_with("app_snapshot_apps")
                || name.starts_with("app_snapshot_footprints")
        })
        .map(|(_, rows)| rows.lines().filter(|l| !l.is_empty()).count())
        .sum();
    assert!(
        child_rows >= 6,
        "fixture must persist child rows ({child_rows})"
    );

    // The oversize lands in the SECOND artifact: the preflight must reject
    // it before ANY row is deleted or inserted.
    let mut bad_artifacts = input.artifacts.clone();
    bad_artifacts[1].size = Some(u64::MAX);
    let bad = SystemModelInput {
        artifacts: bad_artifacts,
        ..input.clone()
    };
    let err = store
        .commit_system_snapshot(&run.run_id, &bad, &app_facts, &inventory, &footprint)
        .expect_err("the oversized artifact must be rejected");
    assert_invalid_input(&err, "app_snapshot_artifacts", "size", u64::MAX as u128);

    assert_eq!(
        before,
        dump_snapshot_tables(&db),
        "a rejected replacement must not delete or partially replace the prior snapshot"
    );
    let after_model = store
        .rebuild_system_model(
            &run.run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .expect("the original must still rebuild and remain usable");
    assert_eq!(before_model, after_model);
}

// ---------------------------------------------------------------------------
// Stored corruption is STILL corruption
// ---------------------------------------------------------------------------

#[test]
fn tampering_with_a_stored_value_is_still_reported_as_corruption() {
    // The retype must not weaken read-side strictness: a persisted value
    // that cannot be decoded is corruption, not bad input.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tamper.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let run = run_record("tamper-run", 1);
    store.begin_run(&run).unwrap();

    commit(
        &mut store,
        &run.run_id,
        &simple_input(),
        &Inventory::default(),
        &FootprintReport::default(),
    )
    .expect("the valid snapshot must commit");

    // A NEGATIVE size is unrepresentable as a `u64`, so the strict
    // decoder must refuse it as PERSISTED corruption.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE app_snapshot_artifacts SET size = -1 WHERE artifact_ord = 0",
            [],
        )
        .unwrap();
    }

    let err = store
        .load_system_snapshot(&run.run_id, &QueryLimits::default())
        .expect_err("a tampered stored value must be refused");
    assert!(
        matches!(err, StoreError::Corrupt { .. }),
        "a stored malformed value must remain corruption, got {err:?}"
    );
}
