//! Workstream C — the footprint-evidence read/write contract, proven with
//! a deliberately TAMPERED store.
//!
//! The write path normalizes evidence into a set. A legitimate database
//! therefore never contains duplicate evidence rows. To prove the read
//! path honours the same contract, this file inserts duplicate rows
//! directly through SQL — the only way a duplicate can exist.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use coresight_apps::{
    ApplicationId, ApplicationSource, AssociationScope, Confidence, EvidenceKind, FootprintKind,
    FootprintReport, Inventory, PackageKind, ProbedKind,
};
use coresight_capabilities::access::AccessState;
use coresight_history::{
    AppSnapshotFact, ConfigFingerprint, HistoryStore, QueryLimits, RunCounts, RunId, RunRecord,
    RunStatus, StoreError,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{ArtifactFact, SystemModelInput};

fn run_record(id: &str) -> RunRecord {
    RunRecord {
        run_id: RunId(id.to_string()),
        started_at: UNIX_EPOCH + Duration::from_secs(1_000_000),
        completed_at: None,
        roots: vec![PathBuf::from("/scope-a")],
        platform: "test".to_string(),
        config: ConfigFingerprint::current(),
        status: RunStatus::Completed,
        counts: RunCounts::default(),
    }
}

fn app_record(name: &str, publisher: Option<&str>) -> coresight_apps::ApplicationRecord {
    coresight_apps::ApplicationRecord {
        id: ApplicationId::derive(name, publisher),
        name: name.to_string(),
        version: Some("1.0".to_string()),
        publisher: publisher.map(str::to_string),
        install_location: Some(PathBuf::from(format!("/apps/{name}"))),
        install_date: None,
        estimated_size_bytes: Some(1024),
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::RegistryUninstall,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
        bundle_identifier: None,
        executable_path: None,
        provenance: vec![ApplicationSource::RegistryUninstall],
    }
}

fn artifact(path: &str) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity {
            volume: 1,
            file_id: 1,
            file_id_hi: None,
        }),
        content_sha256: None,
        size: Some(1),
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

/// A snapshot with ONE application whose footprint carries four DISTINCT
/// evidence items, so the read path has real content to normalize.
fn fixture_with_evidence() -> SystemModelInput {
    let app = app_record("Real App", Some("Acme"));
    SystemModelInput {
        artifacts: vec![artifact("/apps/Real App")],
        applications: vec![coresight_system_model::ApplicationFact {
            record: app.clone(),
            install_roots: vec![PathBuf::from("/apps/Real App")],
            executable: None,
            associations: Vec::new(),
        }],
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: Vec::new(),
    }
}

fn app_fact_with(name: &str, evidence_count: usize) -> AppSnapshotFact {
    use coresight_apps::FootprintCandidate;
    let record = app_record(name, Some("Acme"));
    // ONE scope carrying `evidence_count` DISTINCT evidence items.
    let fps = vec![FootprintCandidate {
        path: PathBuf::from(format!("/apps/{name}")),
        app: record.id.clone(),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Confirmed,
        evidence: (0..evidence_count)
            .map(|i| {
                coresight_apps::FootprintEvidence::new(
                    EvidenceKind::InstallLocation,
                    Confidence::Confirmed,
                    "registry",
                    AssociationScope::ThisMachine,
                    &format!("reason {i}"),
                )
            })
            .collect(),
    }];
    AppSnapshotFact {
        record,
        install_roots: vec![PathBuf::from(format!("/apps/{name}"))],
        executable: None,
        associations: Vec::new(),
        footprints: fps,
    }
}

/// Commit a snapshot and return the store plus the run id.
fn committed(db: &Path, name: &str, run_id: &str, evidence_count: usize) -> (HistoryStore, RunId) {
    let mut store = HistoryStore::open(db).unwrap();
    let record = run_record(run_id);
    store.begin_run(&record).unwrap();
    let input = fixture_with_evidence();
    let facts = vec![app_fact_with(name, evidence_count)];
    let inventory = Inventory {
        records: vec![app_record(name, Some("Acme"))],
        ..Inventory::default()
    };
    let fps: Vec<_> = facts[0].footprints.clone();
    store
        .commit_system_snapshot(
            &record.run_id,
            &input,
            &facts,
            &inventory,
            &FootprintReport {
                candidates: fps,
                ..FootprintReport::default()
            },
        )
        .unwrap();
    (store, record.run_id)
}

/// Tamper: rename each evidence row's ordinal, then re-insert the originals
/// so every row exists TWICE at distinct ordinals (the PRIMARY KEY is
/// `(run_id, app_id, fact_ord, footprint_ord, evidence_ord)`, so a naive
/// copy collides — which is exactly why a duplicate can only come from
/// deliberate tampering, never from the write path).
/// One persisted footprint-evidence row, as tampered data.
#[derive(Default)]
struct EvidenceRow {
    app_id: String,
    fact_ord: i64,
    footprint_ord: i64,
    evidence_ord: i64,
    kind: String,
    confidence: String,
    source: String,
    scope: String,
    why: String,
}

/// Tamper: re-insert every stored evidence row at a FRESH ordinal, so the
/// table ends up holding each row twice. The PRIMARY KEY on
/// `(run_id, app_id, fact_ord, footprint_ord, evidence_ord)` is exactly
/// why a duplicate can only arise from deliberate tampering — the write
/// path never produces one.
fn duplicate_all_evidence(db: &Path, run_id: &str) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let rows: Vec<EvidenceRow> = {
        let mut stmt = conn
            .prepare(
                "SELECT app_id, fact_ord, footprint_ord, evidence_ord, kind, confidence,
                        source, scope, why
                   FROM app_snapshot_footprint_evidence WHERE run_id = ?1",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![run_id], |r| {
            Ok(EvidenceRow {
                app_id: r.get(0)?,
                fact_ord: r.get(1)?,
                footprint_ord: r.get(2)?,
                evidence_ord: r.get(3)?,
                kind: r.get(4)?,
                confidence: r.get(5)?,
                source: r.get(6)?,
                scope: r.get(7)?,
                why: r.get(8)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let mut extra = 1000i64;
    for row in rows {
        conn.execute(
            "INSERT INTO app_snapshot_footprint_evidence
               (run_id, app_id, fact_ord, footprint_ord, evidence_ord,
                kind, confidence, source, scope, why)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                run_id,
                row.app_id,
                row.fact_ord,
                row.footprint_ord,
                extra + row.evidence_ord,
                row.kind,
                row.confidence,
                row.source,
                row.scope,
                row.why,
            ],
        )
        .unwrap();
        extra += 10;
    }
    conn.close().unwrap();
}

/// A tampered store with duplicate evidence rows must reload as the
/// canonical SET, not as a list — the documented contract.
#[test]
fn duplicate_persisted_evidence_is_normalized_into_the_set_contract() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tamper.db");
    let (store, run_id) = committed(&db, "Real App", "tamper-run", 3);
    let app_id = ApplicationId::derive("Real App", Some("Acme"));

    // Sanity: the honest store reloads 2 evidence items.
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let honest = loaded
        .footprint
        .candidates
        .iter()
        .find(|c| c.app == app_id)
        .unwrap();
    assert_eq!(honest.evidence.len(), 3);
    assert_eq!(honest.evidence[0].why, "reason 0");
    assert_eq!(honest.evidence[1].why, "reason 1");

    drop(store);
    // Tamper: every evidence row is now duplicated.
    duplicate_all_evidence(&db, &run_id.0);
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM app_snapshot_footprint_evidence WHERE run_id = ?1",
                rusqlite::params![run_id.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 6, "the tamper doubled the evidence rows");
    }

    // The read path must normalize back to the set (2 items), not expose
    // the duplicates as a 4-item list.
    let store = HistoryStore::open(&db).unwrap();
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let reloaded = loaded
        .footprint
        .candidates
        .iter()
        .find(|c| c.app == app_id)
        .unwrap();
    assert_eq!(
        reloaded.evidence.len(),
        3,
        "duplicate persisted evidence must reload as the canonical set"
    );
    assert_eq!(
        reloaded.evidence, honest.evidence,
        "the normalized set must equal the honest one, order included"
    );

    // And the rebuild still succeeds with the same application count.
    let model = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &coresight_system_model::SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    model.check_invariants().unwrap();
    assert_eq!(model.applications().len(), 1);
}

/// The cap must be applied BEFORE normalization, so a genuinely oversized
/// section still reports truncation instead of being hidden by dedup.
#[test]
fn the_evidence_cap_is_applied_before_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cap.db");
    // One application with FOUR distinct evidence items.
    let (store, run_id) = committed(&db, "Real App", "cap-run", 4);
    drop(store);

    // A limit of 3 caps the 4-item section: the cap must be detected.
    let store = HistoryStore::open(&db).unwrap();
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits { max_results: 3 })
        .unwrap()
        .unwrap();
    let app_id = ApplicationId::derive("Real App", Some("Acme"));
    let c = loaded
        .footprint
        .candidates
        .iter()
        .find(|c| c.app == app_id)
        .unwrap();
    assert_eq!(c.evidence.len(), 3, "the cap bounds the section");
    assert!(
        loaded.is_load_truncated(),
        "the truncation signal must survive (the cap is applied before dedup)"
    );
    assert!(
        loaded
            .load_truncated_sections
            .contains(&"footprint_evidence"),
        "sections: {:?}",
        loaded.load_truncated_sections
    );
    // A rebuild refuses incomplete facts.
    assert!(matches!(
        store.rebuild_system_model(
            &run_id,
            &QueryLimits { max_results: 3 },
            &coresight_system_model::SystemModelLimits::default()
        ),
        Err(StoreError::SnapshotBounded { .. })
    ));
}
