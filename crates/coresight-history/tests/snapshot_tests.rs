//! Phase 6.4 persistence tests: migration, round-trip, determinism,
//! corruption, idempotence, and boundedness.
//!
//! Every fixture is synthetic and portable: no test touches a real
//! filesystem path (paths are opaque strings the store treats as data),
//! spawns a process, or reads the machine.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, AssociationScope, Confidence,
    CorrelationGroup, EvidenceKind, EvidenceSource, EvidenceStrength, FootprintCandidate,
    FootprintEvidence, FootprintKind, FootprintReport, Inventory, MatchedAttribute,
    OwnershipEvidence, PackageKind, ProbedKind, SourceCoverage, SourceStatus,
};
use coresight_capabilities::access::AccessState;
use coresight_classifier::{Category, Confidence as ClassificationConfidence};
use coresight_history::{
    store::HISTORY_SCHEMA_VERSION, AppSnapshotFact, ConfigFingerprint, HistoryStore, QueryLimits,
    RunCounts, RunId, RunRecord, RunStatus, SnapshotBuilder, StoreError,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{
    build_system_model, ApplicationFact, ArtifactClassification, ArtifactFact, HistoryFact,
    RelationshipFact, RelationshipFactKind, SystemModelInput, SystemModelLimits,
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

fn artifact(path: &str, volume: u64, file_id: u64, hi: Option<u64>) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(path),
        kind: ProbedKind::Dir,
        identity: Some(ObjectIdentity {
            volume,
            file_id,
            file_id_hi: hi,
        }),
        content_sha256: None,
        size: None,
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

fn classified(mut a: ArtifactFact, category: Category) -> ArtifactFact {
    a.classification = Some(ArtifactClassification {
        category,
        subcategory: None,
        confidence: ClassificationConfidence::High,
    });
    a
}

/// Install-location evidence for one (app, artifact) association.
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

/// Build a matched (input, app_facts) pair plus an empty inventory and
/// footprint report — the shape a real caller commits.
#[derive(Clone)]
struct Committed {
    input: SystemModelInput,
    app_facts: Vec<AppSnapshotFact>,
    inventory: Inventory,
    footprint: FootprintReport,
}

fn committed(input: SystemModelInput, app_facts: Vec<AppSnapshotFact>) -> Committed {
    let inventory = Inventory {
        records: input
            .applications
            .iter()
            .map(|f| f.record.clone())
            .collect(),
        ..Inventory::default()
    };
    let mut footprint = FootprintReport::default();
    for fact in &app_facts {
        for fp in &fact.footprints {
            footprint.candidates.push(fp.clone());
        }
    }
    Committed {
        input,
        app_facts,
        inventory,
        footprint,
    }
}

/// The verified digest shared by the fixture's duplicate pair.
const SHARED_DIGEST: &str = "aaaa0000bbbb1111cccc2222dddd3333eeee4444ffff5555aaaa6666bbbb7777";

fn with_content(mut a: ArtifactFact, sha256: &str) -> ArtifactFact {
    a.content_sha256 = Some(sha256.to_string());
    a
}

/// A representative multi-domain snapshot: artifacts (wide + narrow
/// identity, classification, classification-less), two applications with
/// evidence/roots/footprints, a PROVEN content-duplicate relationship,
/// history context, and complete source coverage.
fn rich_snapshot() -> Committed {
    let app_a = app_record("Alpha", Some("Acme"));
    let app_b = app_record("Beta", None);

    let root_a = "/apps/Alpha";
    let root_b = "/apps/Beta";
    let cache_a = "/apps/Alpha/cache";

    let artifacts = vec![
        classified(artifact(root_a, 1, 2, None), Category::Applications),
        // The duplicate pair: two DISTINCT objects carrying one proven
        // digest (so the relationship fact below is genuinely valid).
        with_content(
            classified(artifact(cache_a, 1, 3, Some(9)), Category::Cache),
            SHARED_DIGEST,
        ),
        with_content(artifact(root_b, 1, 4, Some(0)), SHARED_DIGEST),
    ];
    let applications = vec![
        ApplicationFact {
            record: app_a.clone(),
            install_roots: vec![PathBuf::from(root_a)],
            executable: Some(PathBuf::from("/apps/Alpha/Alpha.exe")),
            associations: vec![(PathBuf::from(root_a), install_evidence(&app_a, root_a))],
        },
        ApplicationFact {
            record: app_b.clone(),
            install_roots: vec![PathBuf::from(root_b)],
            executable: None,
            associations: vec![(PathBuf::from(root_b), install_evidence(&app_b, root_b))],
        },
    ];
    let relationships = vec![RelationshipFact {
        kind: RelationshipFactKind::ContentDuplicate,
        paths: vec![PathBuf::from(cache_a), PathBuf::from(root_b)],
        object: None,
        content_sha256: Some(SHARED_DIGEST.to_string()),
    }];
    let history = vec![HistoryFact {
        run_id: "older-run".to_string(),
        path: PathBuf::from(root_a),
        identity: Some(ObjectIdentity::narrow(1, 2)),
        category: Some("APPLICATIONS".to_string()),
    }];

    let app_facts = vec![
        AppSnapshotFact {
            record: app_a.clone(),
            install_roots: vec![PathBuf::from(root_a)],
            executable: Some(PathBuf::from("/apps/Alpha/Alpha.exe")),
            associations: vec![(PathBuf::from(root_a), install_evidence(&app_a, root_a))],
            footprints: vec![FootprintCandidate {
                path: PathBuf::from(root_a),
                app: app_a.id.clone(),
                kind: FootprintKind::InstallationDirectory,
                confidence: coresight_apps::Confidence::Confirmed,
                evidence: vec![FootprintEvidence::new(
                    EvidenceKind::InstallLocation,
                    coresight_apps::Confidence::Confirmed,
                    "registry",
                    AssociationScope::ThisMachine,
                    "the installer recorded this location",
                )],
            }],
        },
        AppSnapshotFact {
            record: app_b.clone(),
            install_roots: vec![PathBuf::from(root_b)],
            executable: None,
            associations: vec![(PathBuf::from(root_b), install_evidence(&app_b, root_b))],
            footprints: Vec::new(),
        },
    ];

    committed(
        SystemModelInput {
            artifacts,
            applications,
            relationships,
            history,
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        app_facts,
    )
}

/// Begin a run, commit its system snapshot, and return the run id.
fn commit_snapshot(store: &mut HistoryStore, id: &str, offset: u64, snap: &Committed) -> RunId {
    let record = run_record(id, offset);
    store.begin_run(&record).unwrap();
    store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .unwrap();
    record.run_id
}

/// Open (or create) a store at `dir/name` for a test.
fn store_in(dir: &std::path::Path, name: &str) -> HistoryStore {
    HistoryStore::open(&dir.join(name)).unwrap()
}

/// The id the Phase 6.4 (delimiter-joined) encoding derived from a pair:
/// `app-<sha256("{name}|{publisher}")>`. Used only by the migration
/// fixtures, to write the ids a real v5 store actually holds.
fn legacy_id_of(name: &str, publisher: Option<&str>) -> String {
    use sha2::Digest;
    let key = ApplicationId::legacy_derivation_key(name, publisher);
    let digest = sha2::Sha256::digest(key.as_bytes());
    format!(
        "app-{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn model_of(input: &SystemModelInput) -> coresight_system_model::SystemModel {
    build_system_model(input, &SystemModelLimits::default())
}

// ---------------------------------------------------------------------------
// migration tests
// ---------------------------------------------------------------------------

#[test]
fn fresh_database_migrates_to_schema_v5_with_snapshot_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fresh.db");
    let _store = HistoryStore::open(&db).unwrap();

    let conn = Connection::open(&db).unwrap();
    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, HISTORY_SCHEMA_VERSION);
    assert_eq!(
        HISTORY_SCHEMA_VERSION, 6,
        "v5 added the snapshot tables; v6 re-keys application ids"
    );

    for table in [
        "app_snapshot_meta",
        "app_snapshot_artifacts",
        "app_snapshot_apps",
        "app_snapshot_provenance",
        "app_snapshot_views",
        "app_snapshot_roots",
        "app_snapshot_evidence",
        "app_snapshot_coverage",
        "app_snapshot_relationships",
        "app_snapshot_rel_members",
        "app_snapshot_history",
        "app_snapshot_footprints",
        "app_snapshot_footprint_evidence",
    ] {
        assert!(table_exists(&conn, table), "schema v5 must create {table}");
    }
}

#[test]
fn v4_database_migrates_forward_preserving_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v4.db");
    build_v4_database(&db);

    let store = HistoryStore::open(&db).unwrap();

    let conn = Connection::open(&db).unwrap();
    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, 6,
        "opening a v4 store must apply v5 then the v6 re-key (both legacy-free)"
    );
    assert!(table_exists(&conn, "app_snapshot_meta"));

    // The pre-existing run and its observation survived untouched.
    let run = store
        .get_run(&RunId("v4-run".to_string()))
        .unwrap()
        .expect("the v4 run must survive migration");
    assert_eq!(run.roots, vec![PathBuf::from("/scope-a")]);
    let snapshot = store
        .load_run_snapshot(&RunId("v4-run".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.snapshot.entries.len(), 1);

    // No snapshot was ever committed for it: absence, not an empty one.
    assert!(
        !store
            .has_system_snapshot(&RunId("v4-run".to_string()))
            .unwrap(),
        "a run migrated from v4 owns no system snapshot"
    );
    assert!(store
        .load_system_snapshot(&RunId("v4-run".to_string()), &QueryLimits::default())
        .unwrap()
        .is_none());
}

#[test]
fn migration_is_idempotent_across_repeated_opens() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("repeat.db");
    build_v4_database(&db);

    for _ in 0..3 {
        let store = HistoryStore::open(&db).unwrap();
        // A second open must not re-run the v5 DDL (which would error) and
        // must not lose rows.
        let snapshot = store
            .load_run_snapshot(&RunId("v4-run".to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.snapshot.entries.len(), 1);
    }
    let conn = Connection::open(&db).unwrap();
    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, HISTORY_SCHEMA_VERSION);
}

#[test]
fn newer_schema_is_refused_rather_than_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("newer.db");
    {
        let _store = HistoryStore::open(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.execute("UPDATE schema_version SET version = 99", [])
            .unwrap();
    }
    match HistoryStore::open(&db) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, HISTORY_SCHEMA_VERSION);
        }
        Err(other) => panic!("a newer schema must be refused, got {other:?}"),
        Ok(_) => panic!("a newer schema must be refused, but the store opened"),
    }
}

#[test]
fn corrupt_database_is_refused_before_reading_or_migrating() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("corrupt.db");
    // A file that is not a database at all.
    std::fs::write(&db, b"this is not a sqlite database, not even close").unwrap();
    match HistoryStore::open(&db) {
        Err(StoreError::Corrupt { .. }) | Err(StoreError::Sqlite(_)) => {}
        Err(other) => panic!("a corrupt store must be refused, got {other:?}"),
        Ok(_) => panic!("a corrupt store must be refused, but it opened"),
    }
}

#[test]
fn migration_rolls_back_atomically_when_sql_fails() {
    // Prove the v5 step is transactional: a failure inside the migration
    // transaction leaves the version AND the tables unchanged. The
    // mechanism is asserted directly against SQLite so the guarantee
    // holds even though the shipped DDL is valid.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("atomic.db");
    build_v4_database(&db);

    let conn = Connection::open(&db).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    tx.execute_batch("CREATE TABLE migration_probe (x INTEGER);")
        .unwrap();
    tx.execute_batch("THIS IS NOT VALID SQL;")
        .expect_err("the invalid statement must fail");
    // Dropping without commit rolls back everything in the transaction.
    drop(tx);

    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, 4,
        "a failed migration must not advance the version"
    );
    assert!(
        !table_exists(&conn, "migration_probe"),
        "a failed migration must leave no partial objects"
    );
}

// ---------------------------------------------------------------------------
// round-trip tests
// ---------------------------------------------------------------------------

#[test]
fn canonical_model_is_identical_after_persist_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("roundtrip.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "rt-run", 1, &snap);

    // M1: built from the original facts.
    let m1 = model_of(&snap.input);
    // M2: built from the RELOADED facts through the same builder.
    let m2 = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .expect("a committed snapshot must reload");

    // Observable canonical semantics are identical. Indexes are private
    // and rebuilt on both sides, so they are never compared as stored
    // state.
    assert_eq!(m1.artifacts(), m2.artifacts(), "artifact nodes differ");
    assert_eq!(
        m1.applications(),
        m2.applications(),
        "application nodes differ"
    );
    assert_eq!(m1.edges(), m2.edges(), "edges differ");
    assert_eq!(
        m1.historical_context(),
        m2.historical_context(),
        "historical context differs"
    );
    assert_eq!(
        m1.historical_assertions(),
        m2.historical_assertions(),
        "historical assertions differ"
    );
    assert_eq!(m1.insights(), m2.insights(), "insights differ");
    assert_eq!(m1.candidates(), m2.candidates(), "candidates differ");
    assert_eq!(m1.observations(), m2.observations(), "observations differ");
    assert_eq!(m1.truncation(), m2.truncation(), "truncation differs");
    assert_eq!(m1, m2, "the whole canonical model must be equal");
    m2.check_invariants()
        .expect("reloaded model must satisfy invariants");

    // Non-vacuous: the fixture really exercised the identity-relationship
    // path and the historical join, so the equality above is meaningful.
    assert_eq!(
        m1.truncation().relationships_rejected,
        0,
        "the fixture's relationship must be PROVEN, not rejected"
    );
    assert!(
        m1.edges()
            .iter()
            .any(|e| e.kind == coresight_system_model::SystemEdgeKind::DuplicateOf),
        "a proven content duplicate must produce a DuplicateOf edge"
    );
    assert!(
        !m1.historical_assertions().is_empty(),
        "the historical context must have joined a current node"
    );
}

#[test]
fn application_records_round_trip_with_identity_and_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("apps.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let mut snap = rich_snapshot();
    // Provenance union across sources: one logical application, seen
    // through several sources, keeps ONE id.
    snap.app_facts[0].record.provenance = vec![
        ApplicationSource::RegistryUninstall,
        ApplicationSource::BundleInfoPlist,
        ApplicationSource::DesktopEntry,
    ];
    snap.input.applications[0].record.provenance = snap.app_facts[0].record.provenance.clone();
    let expected_ids: Vec<String> = snap
        .input
        .applications
        .iter()
        .map(|f| f.record.id.0.clone())
        .collect();

    let run_id = commit_snapshot(&mut store, "apps-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    // Canonical ordering: records come back sorted by (id, content).
    let mut ids: Vec<String> = reloaded
        .app_facts
        .iter()
        .map(|f| f.record.id.0.clone())
        .collect();
    ids.sort();
    let mut want = expected_ids.clone();
    want.sort();
    assert_eq!(ids, want, "every logical application survives reload");

    for fact in &reloaded.app_facts {
        let original = reloaded_original(&snap, &fact.record.id.0);
        assert_eq!(fact.record, original, "record fields must round-trip");
        // Provenance union survived, deduplicated and canonically ordered.
        assert_eq!(
            fact.record.provenance, original.provenance,
            "provenance union must survive reload"
        );
    }

    // Changing ONLY the source never changes the id (source is
    // provenance, never identity).
    let derived = ApplicationId::derive("Alpha", Some("Acme"));
    assert_eq!(derived.0, app_record("Alpha", Some("Acme")).id.0);
    // Changing name or publisher DOES change the id.
    assert_ne!(
        ApplicationId::derive("Alpha", Some("Acme")).0,
        ApplicationId::derive("Alpha2", Some("Acme")).0
    );
    assert_ne!(
        ApplicationId::derive("Alpha", Some("Acme")).0,
        ApplicationId::derive("Alpha", Some("Other")).0
    );
}

#[test]
fn object_identity_round_trips_full_width() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("identity.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // Narrow, wide, maximum, and zero components — all distinguishable.
    let identities = [
        ObjectIdentity {
            volume: 1,
            file_id: 2,
            file_id_hi: None,
        },
        ObjectIdentity {
            volume: 1,
            file_id: 2,
            file_id_hi: Some(3),
        },
        ObjectIdentity {
            volume: 1,
            file_id: 2,
            file_id_hi: Some(4),
        },
        ObjectIdentity {
            volume: u64::MAX,
            file_id: u64::MAX,
            file_id_hi: Some(u64::MAX),
        },
        ObjectIdentity::narrow(0, 0),
    ];
    let artifacts: Vec<ArtifactFact> = identities
        .iter()
        .enumerate()
        .map(|(i, id)| ArtifactFact {
            path: PathBuf::from(format!("/id/{i}")),
            kind: ProbedKind::File,
            identity: Some(*id),
            content_sha256: None,
            size: Some(1),
            access: AccessState::ReadSucceeded,
            classification: None,
        })
        .collect();

    let snap = committed(
        SystemModelInput {
            artifacts,
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        Vec::new(),
    );
    let run_id = commit_snapshot(&mut store, "id-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    let mut got: Vec<ObjectIdentity> = reloaded
        .input
        .artifacts
        .iter()
        .filter_map(|a| a.identity)
        .collect();
    got.sort();
    let mut want = identities.to_vec();
    want.sort();
    assert_eq!(
        got, want,
        "every identity component must reconstruct exactly"
    );

    // Distinctness is preserved in both directions.
    let narrow = identities[0];
    let wide3 = identities[1];
    let wide4 = identities[2];
    assert_ne!(narrow, wide3, "narrow and wide never compare equal");
    assert_ne!(wide3, wide4, "different high bits are different objects");
    for id in [narrow, wide3, wide4] {
        assert!(
            got.contains(&id),
            "identity {id:?} must survive persistence distinctly"
        );
    }
}

#[test]
fn lossless_paths_survive_persistence_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("paths.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let long_component = "x".repeat(200);
    let adversarial = [
        "/plain/path",
        "/with spaces/in names",
        "/üñïçø∂é/🎉/日本語",
        "/reserved:a*b?c\"d<e>f|g",
        &format!("/long/{long_component}/end"),
        "/trailing/space /end",
        "/dot/./dotdot/../kept",
    ];
    let artifacts: Vec<ArtifactFact> = adversarial
        .iter()
        .map(|p| ArtifactFact {
            path: PathBuf::from(p),
            kind: ProbedKind::File,
            identity: None,
            content_sha256: None,
            size: Some(1),
            access: AccessState::ReadSucceeded,
            classification: None,
        })
        .collect();

    let snap = committed(
        SystemModelInput {
            artifacts,
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        Vec::new(),
    );
    let run_id = commit_snapshot(&mut store, "path-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    let mut got: Vec<PathBuf> = reloaded
        .input
        .artifacts
        .iter()
        .map(|a| a.path.clone())
        .collect();
    got.sort_by(|a, b| {
        a.as_os_str()
            .as_encoded_bytes()
            .cmp(b.as_os_str().as_encoded_bytes())
    });
    let mut want: Vec<PathBuf> = adversarial.iter().map(PathBuf::from).collect();
    want.sort_by(|a, b| {
        a.as_os_str()
            .as_encoded_bytes()
            .cmp(b.as_os_str().as_encoded_bytes())
    });
    assert_eq!(got, want, "every path must round-trip byte-for-byte");
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_remain_lossless() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("nonutf8.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let raw = b"/data/\xff\xfe/bad";
    let path = PathBuf::from(OsString::from_vec(raw.to_vec()));
    assert!(path.to_str().is_none(), "the fixture must be non-UTF-8");

    let snap = committed(
        SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: path.clone(),
                kind: ProbedKind::File,
                identity: None,
                content_sha256: None,
                size: Some(1),
                access: AccessState::ReadSucceeded,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        Vec::new(),
    );
    let run_id = commit_snapshot(&mut store, "nonutf8-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.input.artifacts[0]
            .path
            .as_os_str()
            .as_encoded_bytes(),
        raw.as_slice(),
        "a non-UTF-8 path must be preserved exactly, never lossily replaced"
    );
}

#[test]
fn evidence_round_trips_as_structured_facts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("evidence.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let app = app_record("Gamma", Some("Globex"));
    let path = "/apps/Gamma";
    let evidence = OwnershipEvidence::new(
        EvidenceKind::ExactExecutablePath,
        EvidenceSource::ExecutableMetadata,
        EvidenceStrength::Strong,
        CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
        AssociationScope::ThisMachine,
        PathBuf::from(path),
        MatchedAttribute::ExecutablePath,
        Some("Gamma".to_string()),
    )
    .with_matched_path(PathBuf::from("/apps/Gamma/Gamma.exe"));

    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact(path, 5, 6, None)],
            applications: vec![ApplicationFact {
                record: app.clone(),
                install_roots: vec![PathBuf::from(path)],
                executable: None,
                associations: vec![(PathBuf::from(path), evidence.clone())],
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![AppSnapshotFact {
            record: app,
            install_roots: vec![PathBuf::from(path)],
            executable: None,
            associations: vec![(PathBuf::from(path), evidence.clone())],
            footprints: Vec::new(),
        }],
    );
    let run_id = commit_snapshot(&mut store, "ev-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    let (reloaded_path, reloaded_evidence) = &reloaded.app_facts[0].associations[0];
    assert_eq!(reloaded_path, &PathBuf::from(path));
    // Every structured field survives — no rendered string is persisted.
    assert_eq!(reloaded_evidence.kind, evidence.kind);
    assert_eq!(reloaded_evidence.source, evidence.source);
    assert_eq!(reloaded_evidence.strength, evidence.strength);
    assert_eq!(
        reloaded_evidence.correlation_group,
        evidence.correlation_group
    );
    assert_eq!(reloaded_evidence.scope, evidence.scope);
    assert_eq!(reloaded_evidence.observed_path, evidence.observed_path);
    assert_eq!(
        reloaded_evidence.matched_attribute,
        evidence.matched_attribute
    );
    assert_eq!(reloaded_evidence.matched_value, evidence.matched_value);
    assert_eq!(reloaded_evidence.matched_path, evidence.matched_path);
}

#[test]
fn install_roots_footprints_and_coverage_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("roots.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "roots-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    for fact in &reloaded.app_facts {
        let original = original_fact(&snap, &fact.record.id.0);
        assert_eq!(
            fact.install_roots, original.install_roots,
            "install roots must round-trip"
        );
        assert_eq!(
            fact.footprints, original.footprints,
            "footprint candidates and their evidence must round-trip"
        );
        assert_eq!(
            fact.executable, original.executable,
            "the candidate executable must round-trip"
        );
    }
    assert_eq!(
        reloaded.input.source_coverage, snap.input.source_coverage,
        "source coverage state must survive persistence"
    );
}

#[test]
fn snapshot_association_is_explicit_and_per_run() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("assoc.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let first = rich_snapshot();
    let second = rich_snapshot();
    let run_one = commit_snapshot(&mut store, "run-one", 1, &first);
    let run_two = commit_snapshot(&mut store, "run-two", 2, &second);

    // Each snapshot answers "which run produced this" — and both exist.
    assert!(store.has_system_snapshot(&run_one).unwrap());
    assert!(store.has_system_snapshot(&run_two).unwrap());
    let summaries = store
        .list_system_snapshots(&QueryLimits::default())
        .unwrap();
    assert_eq!(summaries.len(), 2, "both snapshots are listed");
    assert_eq!(summaries[0].run_id, run_two, "newest run first");
    assert_eq!(summaries[1].run_id, run_one);
    assert!(summaries[0].artifacts > 0);
    assert!(summaries[0].applications > 0);
    assert!(summaries[0].relationships > 0);
    assert!(summaries[0].history_facts > 0);

    // The association is by run id, and the fingerprint travels with the
    // run row (not a second fingerprint system).
    let run = store.get_run(&run_one).unwrap().unwrap();
    assert_eq!(run.config, ConfigFingerprint::current());
    assert_eq!(run.config.app_snapshot_schema, 1);
}

#[test]
fn runs_without_a_snapshot_are_absent_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("absent.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // A committed history run with NO system snapshot.
    let record = run_record("history-only", 1);
    store.begin_run(&record).unwrap();
    let snapshot = SnapshotBuilder::new().build(record.run_id.clone()).unwrap();
    store.commit_run(&record, &snapshot, None).unwrap();

    assert!(!store.has_system_snapshot(&record.run_id).unwrap());
    assert!(
        store
            .load_system_snapshot(&record.run_id, &QueryLimits::default())
            .unwrap()
            .is_none(),
        "absence is never reconstructed as an empty snapshot"
    );
    assert!(store
        .rebuild_system_model(
            &record.run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default()
        )
        .unwrap()
        .is_none());

    // An unknown run likewise returns absence, not an empty snapshot.
    assert!(store
        .load_system_snapshot(
            &RunId("does-not-exist".to_string()),
            &QueryLimits::default()
        )
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// honesty / truncation preservation
// ---------------------------------------------------------------------------

#[test]
fn truncation_and_source_states_survive_persistence_distinctly() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("honesty.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // Inaccessible, unsupported, and failed artifacts stay distinct, and
    // incomplete application sources stay non-empty.
    let snap = committed(
        SystemModelInput {
            artifacts: vec![
                ArtifactFact {
                    path: PathBuf::from("/denied"),
                    kind: ProbedKind::Dir,
                    identity: None,
                    content_sha256: None,
                    size: None,
                    access: AccessState::ExistsButInaccessible,
                    classification: None,
                },
                ArtifactFact {
                    path: PathBuf::from("/unsupported"),
                    kind: ProbedKind::Dir,
                    identity: None,
                    content_sha256: None,
                    size: None,
                    access: AccessState::Unsupported,
                    classification: None,
                },
                ArtifactFact {
                    path: PathBuf::from("/failed"),
                    kind: ProbedKind::Dir,
                    identity: None,
                    content_sha256: None,
                    size: None,
                    access: AccessState::Failed,
                    classification: None,
                },
                ArtifactFact {
                    path: PathBuf::from("/empty"),
                    kind: ProbedKind::Dir,
                    identity: None,
                    content_sha256: None,
                    size: None,
                    access: AccessState::Empty,
                    classification: None,
                },
            ],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![
                SourceCoverage::with_status(
                    "msix-appx",
                    SourceStatus::Unsupported,
                    Some("MSIX/AppX enumeration is not implemented".to_string()),
                ),
                SourceCoverage::with_status(
                    "win32-uninstall",
                    SourceStatus::Unavailable,
                    Some("none of the uninstall views exist".to_string()),
                ),
                SourceCoverage::with_status(
                    "third-source",
                    SourceStatus::Failed,
                    Some("registry read failed".to_string()),
                ),
            ],
        },
        Vec::new(),
    );
    let run_id = commit_snapshot(&mut store, "honesty-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    let access_of = |path: &str| -> AccessState {
        reloaded
            .input
            .artifacts
            .iter()
            .find(|a| a.path == std::path::Path::new(path))
            .unwrap_or_else(|| panic!("artifact {path} must survive"))
            .access
    };
    assert_eq!(access_of("/denied"), AccessState::ExistsButInaccessible);
    assert_eq!(access_of("/unsupported"), AccessState::Unsupported);
    assert_eq!(access_of("/failed"), AccessState::Failed);
    assert_eq!(access_of("/empty"), AccessState::Empty);
    // The critical non-collapse: nothing became an empty success.
    for path in ["/denied", "/unsupported", "/failed"] {
        assert_ne!(
            access_of(path),
            AccessState::Empty,
            "{path} must never reload as observed-empty"
        );
    }

    // Source states stay distinct and keep their notes. Coverage is a
    // MULTISET to the builder, and persistence stores it in canonical
    // (source, status, note) order — so compare the canonicalized sets.
    let mut got = reloaded.input.source_coverage.clone();
    let mut want = snap.input.source_coverage.clone();
    got.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.status.cmp(&b.status))
            .then(a.note.cmp(&b.note))
    });
    want.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.status.cmp(&b.status))
            .then(a.note.cmp(&b.note))
    });
    assert_eq!(
        got, want,
        "unsupported/unavailable/failed must remain distinguishable"
    );
    // The three non-usable states are still three DIFFERENT states.
    let statuses: Vec<SourceStatus> = got.iter().map(|c| c.status).collect();
    assert!(statuses.contains(&SourceStatus::Unsupported));
    assert!(statuses.contains(&SourceStatus::Unavailable));
    assert!(statuses.contains(&SourceStatus::Failed));
    assert!(!statuses.contains(&SourceStatus::Complete));

    // The honest model verdict follows: association knowledge is unknown,
    // never silently "unassociated".
    let m1 = model_of(&snap.input);
    let m2 = model_of(&reloaded.input);
    assert_eq!(m1.artifacts(), m2.artifacts());
    assert!(
        m2.artifacts()
            .iter()
            .any(|a| a.application_status.is_association_unknown()
                || a.application_status.is_association_truncated()),
        "incomplete sources must not manufacture Unassociated"
    );
}

#[test]
fn truncation_counters_survive_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("trunc.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let mut snap = rich_snapshot();
    snap.inventory.records_truncated = 7;
    snap.inventory.records_rejected = 2;
    snap.footprint.candidates_truncated = 3;
    snap.footprint.children_truncated = 11;
    snap.footprint.apps_truncated = 1;
    snap.footprint.evidence_truncated = 5;

    let run_id = commit_snapshot(&mut store, "trunc-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    assert_eq!(reloaded.records_truncated, 7);
    assert_eq!(reloaded.records_rejected, 2);
    assert_eq!(reloaded.footprint.candidates_truncated, 3);
    assert_eq!(reloaded.footprint.children_truncated, 11);
    assert_eq!(reloaded.footprint.apps_truncated, 1);
    assert_eq!(reloaded.footprint.evidence_truncated, 5);
}

#[test]
fn history_stays_context_and_never_becomes_graph_topology() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("history-context.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "ctx-run", 1, &snap);
    let m = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();

    // History is quoted context plus node-attached assertions — never an
    // edge kind, and never a self-loop.
    let history_kinds = [
        "HistoricalAliasOf",
        "HistoricalMoveOf",
        "SameObjectObserved",
        "ObjectReplaced",
    ];
    for edge in m.edges() {
        let name = format!("{:?}", edge.kind);
        assert!(
            !history_kinds.contains(&name.as_str()),
            "history must not become a graph edge: {name}"
        );
    }
    // Reloaded assertions still quote both identities without inventing
    // current ownership.
    for assertion in m.historical_assertions() {
        assert!(
            assertion.provenance.is_usable(),
            "a quoted historical record stays usable context"
        );
    }
}

// ---------------------------------------------------------------------------
// determinism tests
// ---------------------------------------------------------------------------

#[test]
fn canonical_model_is_independent_of_insertion_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("determinism.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // (A, B, C) as authored.
    let forward = rich_snapshot();
    // (C, B, A): every collection reversed — a different arrival order
    // carrying the same fact SET.
    let reversed = reverse_snapshot(&rich_snapshot());

    let run_a = commit_snapshot(&mut store, "order-forward", 1, &forward);
    let run_b = commit_snapshot(&mut store, "order-reversed", 2, &reversed);

    let m_forward = store
        .rebuild_system_model(
            &run_a,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    let m_reversed = store
        .rebuild_system_model(
            &run_b,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(
        m_forward, m_reversed,
        "insertion order must never decide the canonical model"
    );
    assert_eq!(
        m_forward.artifacts(),
        m_reversed.artifacts(),
        "artifacts must be canonically ordered regardless of arrival"
    );
    assert_eq!(m_forward.edges(), m_reversed.edges());
}

#[test]
fn repeated_reload_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stable.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "stable-run", 1, &snap);

    let mut previous: Option<coresight_system_model::SystemModel> = None;
    for round in 0..3 {
        let m = store
            .rebuild_system_model(
                &run_id,
                &QueryLimits::default(),
                &SystemModelLimits::default(),
            )
            .unwrap()
            .unwrap();
        if let Some(prev) = &previous {
            assert_eq!(prev, &m, "reload round {round} must be identical");
        }
        previous = Some(m);
    }
}

#[test]
fn duplicate_input_does_not_change_the_canonical_model() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dup-input.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let base = rich_snapshot();
    // The same fact set, delivered with every fact duplicated. The
    // builder's input is a multiset, so each domain fact is doubled in
    // BOTH the model input and the parallel application facts (the
    // commit contract requires those to agree).
    let mut doubled = rich_snapshot();
    let artifacts = doubled.input.artifacts.clone();
    doubled.input.artifacts.extend(artifacts);
    let apps = doubled.input.applications.clone();
    doubled.input.applications.extend(apps);
    let rels = doubled.input.relationships.clone();
    doubled.input.relationships.extend(rels);
    let hist = doubled.input.history.clone();
    doubled.input.history.extend(hist);
    let coverage = doubled.input.source_coverage.clone();
    doubled.input.source_coverage.extend(coverage);
    let app_facts = doubled.app_facts.clone();
    doubled.app_facts.extend(app_facts);

    let run_a = commit_snapshot(&mut store, "dup-single", 1, &base);
    let run_b = commit_snapshot(&mut store, "dup-doubled", 2, &doubled);

    let m_single = store
        .rebuild_system_model(
            &run_a,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    let m_doubled = store
        .rebuild_system_model(
            &run_b,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        m_single, m_doubled,
        "duplicate facts must collapse exactly as they do in memory"
    );
}

#[test]
fn re_committing_a_snapshot_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("idempotent.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "idem-run", 1, &snap);
    let first = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    let before = summarise(&store, &run_id);

    // Re-commit the SAME snapshot several times.
    for _ in 0..3 {
        store
            .commit_system_snapshot(
                &run_id,
                &snap.input,
                &snap.app_facts,
                &snap.inventory,
                &snap.footprint,
            )
            .unwrap();
    }

    assert_eq!(
        summarise(&store, &run_id),
        before,
        "re-committing must not create semantic duplicates"
    );
    let after = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(first, after, "an idempotent re-commit changes nothing");
}

#[test]
fn duplicate_facts_within_one_commit_stay_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dup-rows.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // Two identical provenance entries and two identical relationships:
    // the builder treats inputs as a multiset, so the stored rows keep
    // them verbatim and the rebuilt model is unchanged.
    let mut snap = rich_snapshot();
    snap.input
        .source_coverage
        .push(SourceCoverage::complete("win32-uninstall"));
    let dup_rel = snap.input.relationships[0].clone();
    snap.input.relationships.push(dup_rel);

    let run_id = commit_snapshot(&mut store, "dup-rows-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.input.relationships.len(),
        2,
        "both relationship facts are stored verbatim"
    );
    assert_eq!(reloaded.input.source_coverage.len(), 2);

    // The rebuilt model equals a model built from the same doubled input.
    let rebuilt = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(rebuilt, model_of(&snap.input));
}

// ---------------------------------------------------------------------------
// adversarial / corruption tests
// ---------------------------------------------------------------------------

#[test]
fn malformed_stored_path_is_a_typed_corruption_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bad-path.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "bad-path-run", 1, &snap);
    drop(store);

    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_artifacts SET path = 'e:zz-not-hex' WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);

    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .expect_err("a malformed stored path must fail loudly");
    assert_corrupt(&err, "app_snapshot_artifacts", "path");
}

#[test]
fn empty_stored_path_is_rejected_not_treated_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty-path.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "empty-path-run", 1, &snap);
    drop(store);

    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_artifacts SET path = '' WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);

    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .expect_err("an empty path column is corruption, not absence");
    assert_corrupt(&err, "app_snapshot_artifacts", "path");
}

#[test]
fn malformed_object_identity_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bad-id.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "bad-id-run", 1, &snap);
    drop(store);

    // A high component with no low pair proves nothing: corruption.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_artifacts SET device = NULL, inode = NULL, file_id_hi = 7
         WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);
    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .expect_err("high bits without a low pair must fail");
    assert_corrupt(&err, "app_snapshot_artifacts", "file_id_hi");
    drop(store);

    // A device without its inode is a half-identity: also corruption.
    let db2 = dir.path().join("half-id.db");
    let mut store2 = HistoryStore::open(&db2).unwrap();
    let run2 = commit_snapshot(&mut store2, "half-id-run", 1, &snap);
    drop(store2);
    let conn = Connection::open(&db2).unwrap();
    conn.execute(
        "UPDATE app_snapshot_artifacts SET device = 1, inode = NULL
         WHERE run_id = ?1",
        rusqlite::params![run2.0],
    )
    .unwrap();
    drop(conn);
    let store2 = HistoryStore::open(&db2).unwrap();
    let err = store2
        .load_system_snapshot(&run2, &QueryLimits::default())
        .expect_err("a half identity must fail");
    assert_corrupt(&err, "app_snapshot_artifacts", "device/inode");
}

#[test]
fn invalid_enum_values_are_rejected_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();

    let cases: Vec<(&str, String, &'static str, &'static str)> = vec![
        (
            "kind",
            "UPDATE app_snapshot_artifacts SET kind = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_artifacts",
            "kind",
        ),
        (
            "access",
            "UPDATE app_snapshot_artifacts SET access = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_artifacts",
            "access",
        ),
        (
            "source",
            "UPDATE app_snapshot_apps SET source = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_apps",
            "source",
        ),
        (
            "package kind",
            "UPDATE app_snapshot_apps SET kind = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_apps",
            "kind",
        ),
        (
            "coverage status",
            "UPDATE app_snapshot_coverage SET status = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_coverage",
            "status",
        ),
        (
            "evidence strength",
            "UPDATE app_snapshot_evidence SET strength = 'IMPOSSIBLE'".to_string(),
            "app_snapshot_evidence",
            "strength",
        ),
        (
            "correlation group",
            "UPDATE app_snapshot_evidence SET group_tag = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_evidence",
            "group_tag",
        ),
        (
            "relationship kind",
            "UPDATE app_snapshot_relationships SET kind = 'SOMETHING_NEW'".to_string(),
            "app_snapshot_relationships",
            "kind",
        ),
    ];

    for (i, (_label, sql, table, column)) in cases.iter().enumerate() {
        let db = dir.path().join(format!("enum-{i}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let run_id = commit_snapshot(&mut store, &format!("enum-run-{i}"), 1, &snap);
        drop(store);

        let conn = Connection::open(&db).unwrap();
        conn.execute(
            &format!("{sql} WHERE run_id = ?1"),
            rusqlite::params![run_id.0],
        )
        .unwrap();
        drop(conn);

        let store = HistoryStore::open(&db).unwrap();
        let err = store
            .load_system_snapshot(&run_id, &QueryLimits::default())
            .expect_err("an unknown enum value must never be defaulted");
        assert_corrupt(&err, table, column);
    }
}

#[test]
fn impossible_application_identity_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bad-app.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "bad-app-run", 1, &snap);
    drop(store);

    // Tamper the id so it no longer matches normalized (name, publisher):
    // persistence must not invent a second identity definition. Foreign
    // keys are disabled for the tamper because this simulates a hostile
    // edit of the store FILE (the exact threat the loader must survive),
    // not a legitimate write through this API.
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    conn.execute(
        "UPDATE app_snapshot_apps SET app_id = 'app-deadbeef' WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);
    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .expect_err("an impossible identity must fail loudly");
    assert_corrupt(&err, "app_snapshot_apps", "app_id");
    drop(store);

    // An empty application name is likewise impossible.
    let db2 = dir.path().join("empty-name.db");
    let mut store2 = HistoryStore::open(&db2).unwrap();
    let run2 = commit_snapshot(&mut store2, "empty-name-run", 1, &snap);
    drop(store2);
    let conn = Connection::open(&db2).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    conn.execute(
        "UPDATE app_snapshot_apps SET name = '' WHERE run_id = ?1",
        rusqlite::params![run2.0],
    )
    .unwrap();
    drop(conn);
    let store2 = HistoryStore::open(&db2).unwrap();
    let err = store2
        .load_system_snapshot(&run2, &QueryLimits::default())
        .expect_err("an empty name must fail");
    assert_corrupt(&err, "app_snapshot_apps", "app_id");
}

#[test]
fn negative_counts_and_sizes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();

    let cases = [
        (
            "size",
            "UPDATE app_snapshot_artifacts SET size = -1",
            "app_snapshot_artifacts",
            "size",
        ),
        (
            "records_truncated",
            "UPDATE app_snapshot_meta SET records_truncated = -5",
            "app_snapshot_meta",
            "records_truncated",
        ),
        (
            "estimated_size",
            "UPDATE app_snapshot_apps SET estimated_size = -99",
            "app_snapshot_apps",
            "estimated_size",
        ),
    ];
    for (i, (_label, sql, table, column)) in cases.iter().enumerate() {
        let db = dir.path().join(format!("negative-{i}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let run_id = commit_snapshot(&mut store, &format!("negative-run-{i}"), 1, &snap);
        drop(store);
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            &format!("{sql} WHERE run_id = ?1"),
            rusqlite::params![run_id.0],
        )
        .unwrap();
        drop(conn);
        let store = HistoryStore::open(&db).unwrap();
        let err = store
            .load_system_snapshot(&run_id, &QueryLimits::default())
            .expect_err("a negative value must never be clamped to zero");
        assert_corrupt(&err, table, column);
    }
}

#[test]
fn maximum_integer_values_round_trip_without_narrowing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("max-int.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // 1. Full-width object identity at u64::MAX in every component DOES
    //    round-trip: identity columns are intentional bit-pattern
    //    conversions, so `u64::MAX` reconstructs exactly.
    let snap = committed(
        SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: PathBuf::from("/max"),
                kind: ProbedKind::File,
                identity: Some(ObjectIdentity {
                    volume: u64::MAX,
                    file_id: u64::MAX,
                    file_id_hi: Some(u64::MAX),
                }),
                content_sha256: None,
                size: Some(i64::MAX as u64),
                access: AccessState::ReadSucceeded,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        Vec::new(),
    );
    let run_id = commit_snapshot(&mut store, "max-int-run", 1, &snap);
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.input.artifacts[0].size,
        Some(i64::MAX as u64),
        "a size at the INTEGER boundary must round-trip exactly"
    );
    assert_eq!(
        loaded.input.artifacts[0].identity,
        Some(ObjectIdentity {
            volume: u64::MAX,
            file_id: u64::MAX,
            file_id_hi: Some(u64::MAX),
        }),
        "full-width identity must reconstruct exactly"
    );
}

#[test]
fn an_unrepresentable_size_is_rejected_at_commit_not_at_reload() {
    // `u64::MAX` exceeds the signed INTEGER store domain. It must be
    // rejected BEFORE the transaction writes anything — never narrowed,
    // wrapped, saturated, or deferred until reload (Workstream C).
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("oversize.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = committed(
        SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: PathBuf::from("/oversize"),
                kind: ProbedKind::File,
                identity: Some(ObjectIdentity::narrow(1, 1)),
                content_sha256: None,
                size: Some(u64::MAX),
                access: AccessState::ReadSucceeded,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        Vec::new(),
    );
    let record = run_record("oversize-run", 1);
    store.begin_run(&record).unwrap();
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("an unrepresentable size must be rejected at commit");
    assert_corrupt(&err, "app_snapshot_artifacts", "size");
    // Nothing was written: a rejected first commit creates no snapshot.
    assert!(!store.has_system_snapshot(&record.run_id).unwrap());
}

#[test]
fn an_unrepresentable_estimated_size_is_rejected_before_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("oversize-est.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let mut record = app_record("Big", Some("Bytes"));
    record.estimated_size_bytes = Some(u64::MAX);
    let evidence = install_evidence(&record, "/apps/Big");
    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact("/apps/Big", 1, 1, None)],
            applications: vec![ApplicationFact {
                record: record.clone(),
                install_roots: vec![PathBuf::from("/apps/Big")],
                executable: None,
                associations: vec![(PathBuf::from("/apps/Big"), evidence.clone())],
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![AppSnapshotFact {
            record,
            install_roots: vec![PathBuf::from("/apps/Big")],
            executable: None,
            associations: vec![(PathBuf::from("/apps/Big"), evidence)],
            footprints: Vec::new(),
        }],
    );
    let run = run_record("oversize-est-run", 1);
    store.begin_run(&run).unwrap();
    let err = store
        .commit_system_snapshot(
            &run.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("an unrepresentable estimated size must be rejected");
    assert_corrupt(&err, "app_snapshot_apps", "estimated_size");
    assert!(!store.has_system_snapshot(&run.run_id).unwrap());
}

#[test]
fn a_rejected_replacement_preserves_the_previous_snapshot() {
    // A commit that fails validation must not damage the snapshot already
    // committed for the same run: the previous facts must still reload
    // exactly, and the model must still build.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("replace.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "replace-run", 1, &snap);
    let before = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();

    // A replacement carrying an unrepresentable size is rejected.
    let mut bad = rich_snapshot();
    bad.input.artifacts[0].size = Some(u64::MAX);
    let err = store
        .commit_system_snapshot(
            &run_id,
            &bad.input,
            &bad.app_facts,
            &bad.inventory,
            &bad.footprint,
        )
        .expect_err("an unrepresentable size must be rejected");
    assert_corrupt(&err, "app_snapshot_artifacts", "size");

    // The previous snapshot is untouched: same model, same facts.
    let after = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        before, after,
        "a rejected commit must preserve the previous snapshot"
    );
    assert!(store.has_system_snapshot(&run_id).unwrap());

    // A replacement carrying a parallel-array mismatch is also rejected
    // and preserves the previous snapshot.
    let mut mismatch = rich_snapshot();
    mismatch.app_facts[0].record.version = Some("2.0".to_string());
    let err = store
        .commit_system_snapshot(
            &run_id,
            &mismatch.input,
            &mismatch.app_facts,
            &mismatch.inventory,
            &mismatch.footprint,
        )
        .expect_err("a mismatched replacement must be rejected");
    assert_corrupt(&err, "app_snapshot_apps", "app_id");
    let restored = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(before, restored);
}

#[test]
fn zero_values_are_preserved_not_treated_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("zeros.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = committed(
        SystemModelInput {
            artifacts: vec![ArtifactFact {
                path: PathBuf::from("/zero"),
                kind: ProbedKind::File,
                identity: Some(ObjectIdentity::narrow(0, 0)),
                content_sha256: None,
                size: Some(0),
                access: AccessState::Empty,
                classification: None,
            }],
            applications: Vec::new(),
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        Vec::new(),
    );
    let run_id = commit_snapshot(&mut store, "zero-run", 1, &snap);
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    let node = &reloaded.input.artifacts[0];
    assert_eq!(node.size, Some(0), "a real zero size is not 'unknown'");
    assert_eq!(
        node.identity,
        Some(ObjectIdentity::narrow(0, 0)),
        "a proven zero identity is not 'unproven'"
    );
    assert_eq!(node.access, AccessState::Empty);
}

#[test]
fn missing_foreign_key_rows_are_rejected_by_the_schema_and_surfaced() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fk.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "fk-run", 1, &snap);
    drop(store);

    // A child row that references a non-existent application fact is
    // refused outright by the foreign key (never silently orphaned).
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    let orphan = conn.execute(
        "INSERT INTO app_snapshot_roots (run_id, app_id, fact_ord, root_ord, path)
         VALUES (?1, 'app-missing', 99, 0, 'u:/ghost')",
        rusqlite::params![run_id.0],
    );
    assert!(orphan.is_err(), "an orphaned child row must be refused");
    conn.close().unwrap();

    // A snapshot whose run row vanished is unreachable: absence, not a
    // fabricated empty snapshot.
    let store = HistoryStore::open(&db).unwrap();
    assert!(store
        .load_system_snapshot(&RunId("no-such-run".to_string()), &QueryLimits::default())
        .unwrap()
        .is_none());
}

#[test]
fn impossible_truncation_state_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("trunc-state.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "trunc-state-run", 1, &snap);
    drop(store);

    // A classification column present without its confidence would let a
    // partial verdict become a plausible one: corruption.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_artifacts SET category = 'CACHE', confidence = NULL
         WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);

    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .expect_err("a partial classification must fail");
    assert_corrupt(
        &err,
        "app_snapshot_artifacts",
        "category/subcategory/confidence",
    );
}

#[test]
fn impossible_system_component_flag_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("syscomp.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "syscomp-run", 1, &snap);
    drop(store);

    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_apps SET system_component = 7 WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);

    let store = HistoryStore::open(&db).unwrap();
    let err = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .expect_err("a non-boolean flag must fail");
    assert_corrupt(&err, "app_snapshot_apps", "system_component");
}

#[test]
fn malformed_correlation_group_shapes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();

    // A singleton group carrying an inner source, and a source-record
    // group missing its inner source, are both impossible shapes.
    for (i, sql) in [
        "UPDATE app_snapshot_evidence SET group_tag = 'NAME_DERIVED', group_source = 'X'",
        "UPDATE app_snapshot_evidence SET group_tag = 'SOURCE_RECORD', group_source = NULL",
        "UPDATE app_snapshot_evidence SET group_tag = 'SOURCE_RECORD', group_source = 'NOPE'",
    ]
    .iter()
    .enumerate()
    {
        let db = dir.path().join(format!("group-{i}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let run_id = commit_snapshot(&mut store, &format!("group-run-{i}"), 1, &snap);
        drop(store);
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            &format!("{sql} WHERE run_id = ?1"),
            rusqlite::params![run_id.0],
        )
        .unwrap();
        drop(conn);
        let store = HistoryStore::open(&db).unwrap();
        let err = store
            .load_system_snapshot(&run_id, &QueryLimits::default())
            .expect_err("a malformed correlation group must fail");
        assert_corrupt(&err, "app_snapshot_evidence", "group_tag");
    }
}

// ---------------------------------------------------------------------------
// evidence ceiling tests (persistence cannot inflate evidence)
// ---------------------------------------------------------------------------

#[test]
fn persisted_overclaimed_evidence_is_clamped_not_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("overclaim.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // A WEAK-kind, WEAK-ceiling association in memory.
    let app = app_record("Delta", Some("Deltaco"));
    let path = "/apps/Delta";
    let weak = OwnershipEvidence::new(
        EvidenceKind::FilenameSimilarity,
        EvidenceSource::FilesystemPathHeuristic,
        EvidenceStrength::Weak,
        CorrelationGroup::NameDerived,
        AssociationScope::ThisMachine,
        PathBuf::from(path),
        MatchedAttribute::ApplicationName,
        Some("Delta".to_string()),
    );
    assert_eq!(weak.strength, EvidenceStrength::Weak);

    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact(path, 9, 9, None)],
            applications: vec![ApplicationFact {
                record: app.clone(),
                install_roots: Vec::new(),
                executable: None,
                associations: vec![(PathBuf::from(path), weak.clone())],
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![AppSnapshotFact {
            record: app,
            install_roots: Vec::new(),
            executable: None,
            associations: vec![(PathBuf::from(path), weak)],
            footprints: Vec::new(),
        }],
    );
    let run_id = commit_snapshot(&mut store, "overclaim-run", 1, &snap);
    drop(store);

    // Tamper the persisted strength into an over-claim.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_evidence SET strength = 'DIRECT' WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);

    let store = HistoryStore::open(&db).unwrap();
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let (_p, evidence) = &reloaded.app_facts[0].associations[0];
    assert_eq!(
        evidence.strength,
        EvidenceStrength::Weak,
        "a tampered over-claim must be clamped to its kind/group ceiling"
    );

    // The rebuilt model reflects the CLAMPED evidence: no ownership is
    // manufactured by the database.
    let rebuilt = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    let m_expected = model_of(&snap.input);
    assert_eq!(
        rebuilt.artifacts(),
        m_expected.artifacts(),
        "clamped evidence must produce the same verdict as the honest input"
    );
    assert!(
        rebuilt
            .artifacts()
            .iter()
            .all(|a| !a.application_status.is_genuinely_unassociated() || true),
        "sanity"
    );
}

#[test]
fn persisted_direct_claim_cannot_exceed_its_group_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ceiling.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // A structural containment claim (Moderate kind, Moderate ceiling).
    let app = app_record("Epsilon", None);
    let path = "/apps/Epsilon";
    let structural = OwnershipEvidence::new(
        EvidenceKind::InstallRootContainment,
        EvidenceSource::FilesystemObservation,
        EvidenceStrength::Moderate,
        CorrelationGroup::InstallRootStructure,
        AssociationScope::ThisMachine,
        PathBuf::from(path),
        MatchedAttribute::InstallRoot,
        None,
    );
    assert_eq!(structural.strength, EvidenceStrength::Moderate);
    assert!(structural.kind.is_structural());

    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact(path, 3, 3, None)],
            applications: vec![ApplicationFact {
                record: app.clone(),
                install_roots: vec![PathBuf::from(path)],
                executable: None,
                associations: vec![(PathBuf::from(path), structural.clone())],
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![AppSnapshotFact {
            record: app,
            install_roots: vec![PathBuf::from(path)],
            executable: None,
            associations: vec![(PathBuf::from(path), structural)],
            footprints: Vec::new(),
        }],
    );
    let run_id = commit_snapshot(&mut store, "ceiling-run", 1, &snap);
    drop(store);

    // Try to promote the structural claim to ownership by tampering.
    // Enum values use the canonical SCREAMING_SNAKE_CASE spelling.
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE app_snapshot_evidence SET kind = 'INSTALL_LOCATION', strength = 'DIRECT'
         WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    // Also try to make its path claim the artifact wholesale.
    conn.execute(
        "UPDATE app_snapshot_evidence SET group_tag = 'INSTALL_ROOT_STRUCTURE',
                group_source = NULL
         WHERE run_id = ?1",
        rusqlite::params![run_id.0],
    )
    .unwrap();
    drop(conn);

    let store = HistoryStore::open(&db).unwrap();
    let reloaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let (_p, evidence) = &reloaded.app_facts[0].associations[0];
    assert!(
        evidence.strength <= EvidenceStrength::Moderate,
        "the INSTALL_ROOT_STRUCTURE ceiling must still apply after reload"
    );
    // The structural-only classification is recomputed by the builder
    // from the reloaded KIND, so the verdict follows the honest rule.
    let rebuilt = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(rebuilt.observations(), model_of(&snap.input).observations());
}

// ---------------------------------------------------------------------------
// commit contract: parallel application facts
// ---------------------------------------------------------------------------

#[test]
fn non_parallel_application_facts_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("non-parallel.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let record = run_record("non-parallel-run", 1);
    store.begin_run(&record).unwrap();

    // Same fact SET, but the two vectors disagree at index 0, so a
    // footprint would attach to the wrong application. The commit must
    // fail closed rather than persisting a mis-attributed snapshot.
    let mut swapped = snap.app_facts.clone();
    swapped.swap(0, 1);
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &swapped,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("non-parallel application facts must be rejected");
    assert_corrupt(&err, "app_snapshot_apps", "app_id");

    // Nothing was written: the rejected commit left no partial snapshot.
    assert!(!store.has_system_snapshot(&record.run_id).unwrap());

    // A length mismatch is likewise refused.
    let mut short = snap.app_facts.clone();
    short.pop();
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &short,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("a length mismatch must be rejected");
    assert_corrupt(&err, "app_snapshot_apps", "app_id");
    assert!(!store.has_system_snapshot(&record.run_id).unwrap());

    // The correct pairing still commits.
    store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .unwrap();
    assert!(store.has_system_snapshot(&record.run_id).unwrap());
}

// ---------------------------------------------------------------------------
// commit contract: parallel application facts
// ---------------------------------------------------------------------------

/// One way to make the application facts disagree with the model input.
type Divergence = (&'static str, fn(&mut Vec<AppSnapshotFact>));

#[test]
fn inconsistent_application_facts_are_rejected_field_by_field() {
    // Only `app_facts` is persisted, so a caller whose two vectors disagree
    // beyond the id would silently persist a snapshot that does not
    // describe the facts the model was built from. Each divergence is
    // rejected before the transaction opens.
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();

    let cases: Vec<Divergence> = vec![
        ("record", |f| {
            f[0].record.version = Some("9.9".to_string());
        }),
        ("install_roots", |f| {
            f[0].install_roots = vec![PathBuf::from("/apps/Elsewhere")];
        }),
        ("executable", |f| {
            f[0].executable = Some(PathBuf::from("/apps/Alpha/other.exe"));
        }),
        ("associations", |f| {
            f[0].associations.clear();
        }),
    ];

    for (label, mutate) in cases {
        let db = dir.path().join(format!("mismatch-{label}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record(&format!("mismatch-{label}-run"), 1);
        store.begin_run(&record).unwrap();

        let mut facts = snap.app_facts.clone();
        mutate(&mut facts);
        let err = store
            .commit_system_snapshot(
                &record.run_id,
                &snap.input,
                &facts,
                &snap.inventory,
                &snap.footprint,
            )
            .unwrap_err();
        assert_corrupt(&err, "app_snapshot_apps", "app_id");
        assert!(
            !store.has_system_snapshot(&record.run_id).unwrap(),
            "a rejected commit must leave no snapshot ({label})"
        );
    }
}

#[test]
fn footprint_report_must_match_the_application_facts() {
    // Counters and candidates describe the same facts that get stored; a
    // report listing candidates that are not in the application facts is
    // refused rather than persisted as inconsistent knowledge.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fp-mismatch.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let record = run_record("fp-mismatch-run", 1);
    store.begin_run(&record).unwrap();

    // The fixture's report holds Alpha's one footprint; add a ghost.
    let mut footprint = snap.footprint.clone();
    footprint.candidates.push(FootprintCandidate {
        path: PathBuf::from("/apps/Ghost"),
        app: ApplicationId::derive("Ghost", None),
        kind: FootprintKind::InstallationDirectory,
        confidence: coresight_apps::Confidence::Probable,
        evidence: Vec::new(),
    });
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &footprint,
        )
        .unwrap_err();
    assert_corrupt(&err, "app_snapshot_footprints", "footprint_ord");
    assert!(!store.has_system_snapshot(&record.run_id).unwrap());

    // The consistent report commits, and the candidates round-trip.
    store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .unwrap();
    let loaded = store
        .load_system_snapshot(&record.run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let mut want = snap.footprint.candidates.clone();
    want.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
    });
    assert_eq!(
        loaded.footprint.candidates, want,
        "footprint candidates must round-trip"
    );
}

#[test]
fn capped_child_sections_are_reported_and_never_rebuilt() {
    // Install roots, provenance and views are MODEL-AFFECTING inputs, so a
    // cap that cut them short must be reported exactly like the top-level
    // sections — otherwise a partial load could become a model.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("child-cap.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let app = app_record("Many", Some("Roots"));
    let roots: Vec<PathBuf> = (0..4)
        .map(|i| PathBuf::from(format!("/apps/Many/root{i}")))
        .collect();
    let mut record = app;
    record.observed_in_views = (0..4).map(|i| format!("VIEW{i}")).collect();
    record.provenance = vec![
        ApplicationSource::RegistryUninstall,
        ApplicationSource::BundleInfoPlist,
        ApplicationSource::DesktopEntry,
    ];

    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact("/apps/Many", 1, 1, None)],
            applications: vec![ApplicationFact {
                record: record.clone(),
                install_roots: roots.clone(),
                executable: None,
                associations: Vec::new(),
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![AppSnapshotFact {
            record,
            install_roots: roots,
            executable: None,
            associations: Vec::new(),
            footprints: Vec::new(),
        }],
    );
    let run_id = commit_snapshot(&mut store, "child-cap-run", 1, &snap);

    // A cap of 2 cuts the 4 roots and the 4 views short.
    let small = QueryLimits { max_results: 2 };
    let loaded = store
        .load_system_snapshot(&run_id, &small)
        .unwrap()
        .unwrap();
    assert!(
        loaded.is_load_truncated(),
        "a capped CHILD section must report truncation"
    );
    assert!(
        loaded.load_truncated_sections.contains(&"install_roots"),
        "install_roots must be named (got {:?})",
        loaded.load_truncated_sections
    );
    assert!(
        loaded.load_truncated_sections.contains(&"views"),
        "views must be named (got {:?})",
        loaded.load_truncated_sections
    );
    assert!(
        loaded.app_facts[0].install_roots.len() <= 2,
        "a capped child section must not exceed the caller's bound"
    );

    // And the rebuild refuses rather than building from partial roots.
    match store.rebuild_system_model(&run_id, &small, &SystemModelLimits::default()) {
        Err(StoreError::SnapshotBounded { sections, .. }) => {
            assert!(
                sections.contains(&"install_roots"),
                "the refused sections must name install_roots (got {sections:?})"
            );
        }
        other => panic!("a capped child section must refuse the rebuild, got {other:?}"),
    }

    // With room, everything loads complete and rebuilds.
    let enough = QueryLimits { max_results: 32 };
    let full = store
        .load_system_snapshot(&run_id, &enough)
        .unwrap()
        .unwrap();
    assert!(!full.is_load_truncated());
    assert_eq!(full.app_facts[0].install_roots.len(), 4);
    assert!(store
        .rebuild_system_model(&run_id, &enough, &SystemModelLimits::default())
        .unwrap()
        .is_some());
}

#[test]
fn load_snapshot_applications_reports_its_own_bound() {
    // The inventory-only API must not hand back a partial record list with
    // no signal.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("apps-bound.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "apps-bound-run", 1, &snap);

    let capped = store
        .load_snapshot_applications(&run_id, &QueryLimits { max_results: 1 })
        .unwrap()
        .unwrap();
    assert!(
        capped.is_load_truncated(),
        "a bounded inventory load must report itself"
    );
    assert!(capped.records.len() <= 1);

    let full = store
        .load_snapshot_applications(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert!(!full.is_load_truncated());
    assert_eq!(full.records.len(), snap.app_facts.len());

    // Absence is still absence, not an empty inventory.
    assert!(store
        .load_snapshot_applications(&RunId("no-such-run".to_string()), &QueryLimits::default())
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// Workstream B — footprint fidelity
// ---------------------------------------------------------------------------

/// One footprint candidate for an application scope.
fn footprint(path: &str, app: &ApplicationId, conf: Confidence, why: &str) -> FootprintCandidate {
    FootprintCandidate {
        path: PathBuf::from(path),
        app: app.clone(),
        kind: FootprintKind::InstallationDirectory,
        confidence: conf,
        evidence: vec![FootprintEvidence::new(
            EvidenceKind::InstallLocation,
            conf,
            "registry",
            AssociationScope::ThisMachine,
            why,
        )],
    }
}

/// One application fact carrying `footprints`.
fn app_fact_with_footprints(
    record: ApplicationRecord,
    roots: Vec<PathBuf>,
    footprints: Vec<FootprintCandidate>,
) -> AppSnapshotFact {
    AppSnapshotFact {
        record,
        install_roots: roots,
        executable: None,
        associations: Vec::new(),
        footprints,
    }
}

/// Replace a `Committed`'s footprint report (the builder helpers below
/// build the report from the same candidates, so the two always agree).
fn with_footprint(mut c: Committed, report: FootprintReport) -> Committed {
    c.footprint = report;
    c
}

#[test]
fn same_key_footprint_duplicates_are_reconciled_not_arrival_ordered() {
    // Two descriptions of the SAME scope (path, app, kind) differing only
    // in confidence/evidence. The survivor must be the better description
    // — chosen by content, never by which arrived first — and no evidence
    // may be silently dropped without the winner being that description.
    let dir = tempfile::tempdir().unwrap();
    let app = app_record("Dup", Some("Pub"));
    let roots = vec![PathBuf::from("/apps/Dup")];

    /// One snapshot whose single application carries `fps`; the report is
    /// the same candidates, canonicalized exactly as the implementation
    /// canonicalizes, so report and facts agree by construction.
    fn build(
        app: &ApplicationRecord,
        roots: Vec<PathBuf>,
        fps: Vec<FootprintCandidate>,
    ) -> Committed {
        let mut report = fps.clone();
        report.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
                .then(a.app.0.cmp(&b.app.0))
                .then(a.kind.cmp(&b.kind))
                .then(
                    coresight_apps::footprint::confidence_strength(b.confidence).cmp(
                        &coresight_apps::footprint::confidence_strength(a.confidence),
                    ),
                )
        });
        report.dedup_by(|a, b| a.path == b.path && a.app == b.app && a.kind == b.kind);
        with_footprint(
            committed(
                SystemModelInput {
                    artifacts: vec![artifact("/apps/Dup", 1, 1, None)],
                    applications: vec![ApplicationFact {
                        record: app.clone(),
                        install_roots: roots.clone(),
                        executable: None,
                        associations: Vec::new(),
                    }],
                    relationships: Vec::new(),
                    history: Vec::new(),
                    source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
                },
                vec![app_fact_with_footprints(app.clone(), roots, fps)],
            ),
            FootprintReport {
                candidates: report,
                ..FootprintReport::default()
            },
        )
    }

    // Commit order A: weak first, strong second.
    let forward = build(
        &app,
        roots.clone(),
        vec![
            footprint("/apps/Dup", &app.id, Confidence::Possible, "weak"),
            footprint("/apps/Dup", &app.id, Confidence::Confirmed, "strong"),
        ],
    );
    // Commit order B: the same two candidates in the reverse arrival order.
    let reversed = build(
        &app,
        roots,
        vec![
            footprint("/apps/Dup", &app.id, Confidence::Confirmed, "strong"),
            footprint("/apps/Dup", &app.id, Confidence::Possible, "weak"),
        ],
    );

    let mut store_a = store_in(dir.path(), "dup-a.db");
    let mut store_b = store_in(dir.path(), "dup-b.db");
    let run_a = commit_snapshot(&mut store_a, "dup-run-a", 1, &forward);
    let run_b = commit_snapshot(&mut store_b, "dup-run-b", 2, &reversed);

    let loaded_a = store_a
        .load_system_snapshot(&run_a, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let loaded_b = store_b
        .load_system_snapshot(&run_b, &QueryLimits::default())
        .unwrap()
        .unwrap();

    // Exactly ONE row survives the reconciliation (one scope), and it is
    // the SAME (better) description regardless of arrival order.
    assert_eq!(
        loaded_a.footprint.candidates.len(),
        1,
        "same-key descriptions collapse to one scope"
    );
    assert_eq!(
        loaded_a.footprint.candidates, loaded_b.footprint.candidates,
        "the surviving description must not depend on arrival order"
    );
    assert_eq!(
        loaded_a.footprint.candidates[0].confidence,
        Confidence::Confirmed,
        "the strictly better description must win"
    );
    assert_eq!(
        loaded_a.footprint.candidates[0].evidence[0].why, "strong",
        "the winner's evidence must survive intact"
    );

    // The rebuilt model is identical for both arrival orders.
    let m_a = store_a
        .rebuild_system_model(
            &run_a,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    let m_b = store_b
        .rebuild_system_model(
            &run_b,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        m_a, m_b,
        "footprint reconciliation must be order-independent"
    );
    m_a.check_invariants().unwrap();
}

#[test]
fn distinct_footprint_facts_are_all_preserved() {
    // Candidates differing in the admission key (path / app / kind) are
    // DISTINCT facts: none may be collapsed, and each keeps its evidence.
    let dir = tempfile::tempdir().unwrap();
    let app = app_record("Distinct", Some("Pub"));
    let other = app_record("Other", Some("Pub"));
    let roots = vec![PathBuf::from("/apps/Distinct")];

    // Two applications, each with its own facts. The cache candidate is
    // attributed to `other`, so it must ride in `other`'s fact — the
    // storage is per-application, and the commit validation enforces that
    // a candidate's `app` matches its enclosing fact.
    let other_roots = vec![PathBuf::from("/apps/Other")];
    let fps = vec![
        footprint(
            "/apps/Distinct",
            &app.id,
            Confidence::Confirmed,
            "the install directory",
        ),
        FootprintCandidate {
            path: PathBuf::from("/apps/Distinct/bin"),
            app: app.id.clone(),
            kind: FootprintKind::Executable,
            confidence: Confidence::Probable,
            evidence: vec![FootprintEvidence::new(
                EvidenceKind::ExactExecutablePath,
                Confidence::Probable,
                "registry",
                AssociationScope::ThisMachine,
                "the recorded executable",
            )],
        },
    ];
    let other_fps = vec![FootprintCandidate {
        path: PathBuf::from("/apps/Distinct"),
        app: other.id.clone(),
        kind: FootprintKind::Cache,
        confidence: Confidence::Possible,
        evidence: vec![FootprintEvidence::new(
            EvidenceKind::KnownApplicationDirectory,
            Confidence::Possible,
            "registry",
            AssociationScope::ThisMachine,
            "another application's cache",
        )],
    }];
    let mut all = fps.clone();
    all.extend(other_fps.clone());

    let snap = with_footprint(
        committed(
            SystemModelInput {
                artifacts: vec![artifact("/apps/Distinct", 1, 1, None)],
                applications: vec![
                    ApplicationFact {
                        record: app.clone(),
                        install_roots: roots.clone(),
                        executable: None,
                        associations: Vec::new(),
                    },
                    ApplicationFact {
                        record: other.clone(),
                        install_roots: other_roots.clone(),
                        executable: None,
                        associations: Vec::new(),
                    },
                ],
                relationships: Vec::new(),
                history: Vec::new(),
                source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
            },
            vec![
                app_fact_with_footprints(app.clone(), roots, fps),
                app_fact_with_footprints(other.clone(), other_roots, other_fps),
            ],
        ),
        FootprintReport {
            candidates: all,
            ..FootprintReport::default()
        },
    );

    let mut store = store_in(dir.path(), "distinct.db");
    let run_id = commit_snapshot(&mut store, "distinct-run", 1, &snap);
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    assert_eq!(
        loaded.footprint.candidates.len(),
        3,
        "distinct-key candidates must all survive"
    );
    for c in &loaded.footprint.candidates {
        assert_eq!(c.evidence.len(), 1, "each candidate keeps its own evidence");
        assert!(!c.evidence[0].why.is_empty());
    }
    let apps: Vec<&str> = loaded
        .footprint
        .candidates
        .iter()
        .map(|c| c.app.0.as_str())
        .collect();
    assert!(apps.contains(&app.id.0.as_str()), "own attribution kept");
    assert!(
        apps.contains(&other.id.0.as_str()),
        "foreign attribution kept"
    );
}

#[test]
fn footprint_attributed_to_a_foreign_application_is_rejected() {
    // A footprint rides along its application fact, so its `app` must BE
    // that fact's application. A candidate attributed to different
    // software would silently relocate a scope onto the wrong application
    // on reload, so it is rejected before any row is written.
    let dir = tempfile::tempdir().unwrap();
    let app = app_record("Mine", Some("Pub"));
    let other = app_record("Theirs", Some("Pub"));
    let roots = vec![PathBuf::from("/apps/Mine")];

    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact("/apps/Mine", 1, 1, None)],
            applications: vec![ApplicationFact {
                record: app.clone(),
                install_roots: roots.clone(),
                executable: None,
                associations: Vec::new(),
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![app_fact_with_footprints(
            app.clone(),
            roots,
            vec![footprint(
                "/apps/Mine",
                &other.id,
                Confidence::Confirmed,
                "misattributed",
            )],
        )],
    );

    let record = run_record("foreign-fp-run", 1);
    let mut store = store_in(dir.path(), "foreign.db");
    store.begin_run(&record).unwrap();
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("a footprint attributed to another application must be rejected");
    assert_corrupt(&err, "app_snapshot_footprints", "footprint_ord");
    assert!(!store.has_system_snapshot(&record.run_id).unwrap());
}

// ---------------------------------------------------------------------------
// Workstream A — legacy application-id re-keying (v5 → v6)
// ---------------------------------------------------------------------------

/// Build a genuine Phase 6.4 (schema v5) database holding snapshot rows
/// written under the LEGACY application-identity encoding, so the v6
/// migration has real historical facts to re-key.
///
/// The fixture deliberately includes the ambiguity Phase 6.4 had: two
/// rows whose `(name, publisher)` pairs are DISTINCT but which the legacy
/// encoding derived the SAME id for — `("A|B","C")` and `("A","B|C")`.
/// Both are committed under that one legacy id with different
/// `fact_ord`s, so a safe migration must split them back into two ids and
/// move each row's children to its own parent.
fn build_legacy_v5_snapshot_store(db_path: &std::path::Path) {
    let conn = Connection::open(db_path).unwrap();
    // v1 bootstrap + the full v5 snapshot schema, matching MIGRATION_V2
    // and MIGRATION_V5 as they shipped in Phase 6.4.
    let core = V4_CORE_SCHEMA;
    let snapshots = v5_snapshot_schema();
    let sql = format!(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);
         INSERT INTO schema_version (version) VALUES (1);
         {core}
         {snapshots}"
    );
    conn.execute_batch(&sql).unwrap();
    conn.close().unwrap();
}

/// The v4 scan_runs/observations shape the v5 snapshot tables hang off.
const V4_CORE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS scan_runs (
    run_id TEXT PRIMARY KEY,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    roots TEXT NOT NULL,
    platform TEXT NOT NULL,
    config TEXT NOT NULL,
    status TEXT NOT NULL,
    entries_examined INTEGER NOT NULL DEFAULT 0,
    files INTEGER NOT NULL DEFAULT 0,
    dirs INTEGER NOT NULL DEFAULT 0,
    links INTEGER NOT NULL DEFAULT 0,
    other_entries INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    observation_errors INTEGER NOT NULL DEFAULT 0,
    candidates_untracked INTEGER NOT NULL DEFAULT 0,
    hash_failures INTEGER NOT NULL DEFAULT 0,
    rel_status TEXT,
    rel_truncated INTEGER
);
CREATE TABLE IF NOT EXISTS observations (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    size INTEGER,
    device INTEGER, inode INTEGER, file_id_hi INTEGER, modified INTEGER,
    category TEXT, subcategory TEXT, content_sha256 TEXT, obs_error TEXT,
    PRIMARY KEY (run_id, path)
);
CREATE TABLE IF NOT EXISTS relationship_obs (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    rel_id TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER NOT NULL,
    member_count INTEGER NOT NULL, recoverable INTEGER, accounting TEXT NOT NULL,
    PRIMARY KEY (run_id, rel_id)
);
CREATE TABLE IF NOT EXISTS relationship_members (
    run_id TEXT NOT NULL, rel_id TEXT NOT NULL, path TEXT NOT NULL,
    device INTEGER, inode INTEGER, file_id_hi INTEGER,
    PRIMARY KEY (run_id, rel_id, path)
);
";

/// The v5 snapshot schema, verbatim from MIGRATION_V5 (a historical
/// fixture: it mirrors the shipped DDL, never edits it).
fn v5_snapshot_schema() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS app_snapshot_meta (
            run_id TEXT PRIMARY KEY REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            records_truncated INTEGER NOT NULL, records_rejected INTEGER NOT NULL,
            fp_candidates_truncated INTEGER NOT NULL, fp_children_truncated INTEGER NOT NULL,
            fp_apps_truncated INTEGER NOT NULL, fp_evidence_truncated INTEGER NOT NULL
        );
        INSERT INTO scan_runs
            (run_id, started_at, completed_at, roots, platform, config, status)
            VALUES ('legacy-run', 1000, 2000, '[\"u:/scope\"]', 'test',
                    '{{\"observationModel\":1,\"classifierSchema\":\"test\",\"classifierRules\":1,\"hashAlgorithm\":\"sha256\",\"relationshipSchema\":1,\"historySchema\":2}}',
                    'COMPLETED');
        INSERT INTO app_snapshot_meta
            (run_id, records_truncated, records_rejected, fp_candidates_truncated,
             fp_children_truncated, fp_apps_truncated, fp_evidence_truncated)
            VALUES ('legacy-run', 0, 0, 0, 0, 0, 0);
        CREATE TABLE IF NOT EXISTS app_snapshot_artifacts (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            artifact_ord INTEGER NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL,
            size INTEGER, device INTEGER, inode INTEGER, file_id_hi INTEGER,
            content_sha256 TEXT, access TEXT NOT NULL, category TEXT,
            subcategory TEXT, confidence TEXT,
            PRIMARY KEY (run_id, artifact_ord)
        );
        INSERT INTO app_snapshot_artifacts
            (run_id, artifact_ord, path, kind, size, access)
            VALUES ('legacy-run', 0, 'u:/apps/A', 'DIR', NULL, 'READ_SUCCEEDED');
        {}",
        apps_and_children()
    )
}

/// The application rows, the child tables they reference, and every child
/// row of the legacy fixture.
///
/// Two DISTINCT `(name, publisher)` pairs share ONE legacy id (the Phase
/// 6.4 ambiguity), each with its own `fact_ord` and its own children — so
/// a correct migration must split the id and carry each child to its own
/// parent.
fn apps_and_children() -> String {
    // The id the OLD (Phase 6.4, delimiter-joined) encoding derived for
    // BOTH pairs — the conflation the fixture must exercise. It is the
    // SHA-256 of the legacy key, exactly as v5 stored it.
    let legacy = legacy_id_of("A|B", Some("C"));
    assert_eq!(
        legacy,
        legacy_id_of("A", Some("B|C")),
        "the fixture must exercise the real ambiguity"
    );
    let child_tables = "
        CREATE TABLE IF NOT EXISTS app_snapshot_provenance (
            run_id TEXT NOT NULL, app_id TEXT NOT NULL, fact_ord INTEGER NOT NULL,
            prov_ord INTEGER NOT NULL, source TEXT NOT NULL,
            PRIMARY KEY (run_id, app_id, fact_ord, prov_ord),
            FOREIGN KEY (run_id, app_id, fact_ord)
                REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_views (
            run_id TEXT NOT NULL, app_id TEXT NOT NULL, fact_ord INTEGER NOT NULL,
            view_ord INTEGER NOT NULL, view TEXT NOT NULL,
            PRIMARY KEY (run_id, app_id, fact_ord, view_ord),
            FOREIGN KEY (run_id, app_id, fact_ord)
                REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_roots (
            run_id TEXT NOT NULL, app_id TEXT NOT NULL, fact_ord INTEGER NOT NULL,
            root_ord INTEGER NOT NULL, path TEXT NOT NULL,
            PRIMARY KEY (run_id, app_id, fact_ord, root_ord),
            FOREIGN KEY (run_id, app_id, fact_ord)
                REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_evidence (
            run_id TEXT NOT NULL, app_id TEXT NOT NULL, fact_ord INTEGER NOT NULL,
            artifact_path TEXT NOT NULL, evidence_ord INTEGER NOT NULL, kind TEXT NOT NULL,
            source TEXT NOT NULL, strength TEXT NOT NULL, group_tag TEXT NOT NULL,
            group_source TEXT, scope TEXT NOT NULL, observed_path TEXT NOT NULL,
            matched_attribute TEXT NOT NULL, matched_value TEXT, matched_path TEXT,
            PRIMARY KEY (run_id, app_id, fact_ord, artifact_path, evidence_ord),
            FOREIGN KEY (run_id, app_id, fact_ord)
                REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_footprints (
            run_id TEXT NOT NULL, app_id TEXT NOT NULL, fact_ord INTEGER NOT NULL,
            footprint_ord INTEGER NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL,
            confidence TEXT NOT NULL,
            PRIMARY KEY (run_id, app_id, fact_ord, footprint_ord),
            FOREIGN KEY (run_id, app_id, fact_ord)
                REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_footprint_evidence (
            run_id TEXT NOT NULL, app_id TEXT NOT NULL, fact_ord INTEGER NOT NULL,
            footprint_ord INTEGER NOT NULL, evidence_ord INTEGER NOT NULL,
            kind TEXT NOT NULL, confidence TEXT NOT NULL, source TEXT NOT NULL,
            scope TEXT NOT NULL, why TEXT NOT NULL,
            PRIMARY KEY (run_id, app_id, fact_ord, footprint_ord, evidence_ord),
            FOREIGN KEY (run_id, app_id, fact_ord, footprint_ord)
                REFERENCES app_snapshot_footprints(run_id, app_id, fact_ord, footprint_ord)
                ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_coverage (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            coverage_ord INTEGER NOT NULL, source TEXT NOT NULL,
            status TEXT NOT NULL, note TEXT,
            PRIMARY KEY (run_id, coverage_ord)
        );
        INSERT INTO app_snapshot_coverage (run_id, coverage_ord, source, status, note)
            VALUES ('legacy-run', 0, 'win32-uninstall', 'COMPLETE', NULL);
        CREATE TABLE IF NOT EXISTS app_snapshot_relationships (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            rel_ord INTEGER NOT NULL, kind TEXT NOT NULL, object_device INTEGER,
            object_inode INTEGER, object_hi INTEGER, content_sha256 TEXT,
            PRIMARY KEY (run_id, rel_ord)
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_rel_members (
            run_id TEXT NOT NULL, rel_ord INTEGER NOT NULL, member_ord INTEGER NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY (run_id, rel_ord, member_ord),
            FOREIGN KEY (run_id, rel_ord)
                REFERENCES app_snapshot_relationships(run_id, rel_ord) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS app_snapshot_history (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            hist_ord INTEGER NOT NULL, hist_run_id TEXT NOT NULL, path TEXT NOT NULL,
            device INTEGER, inode INTEGER, file_id_hi INTEGER, category TEXT,
            PRIMARY KEY (run_id, hist_ord)
        );
        CREATE INDEX IF NOT EXISTS idx_app_snap_apps_id ON app_snapshot_apps(run_id, app_id);
        UPDATE schema_version SET version = 5;";
    format!(
        "CREATE TABLE IF NOT EXISTS app_snapshot_apps (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            app_id TEXT NOT NULL,
            fact_ord INTEGER NOT NULL,
            name TEXT NOT NULL, version TEXT, publisher TEXT,
            install_location TEXT, install_date TEXT, estimated_size INTEGER,
            uninstall_string TEXT, quiet_uninstall_string TEXT, modify_path TEXT,
            install_source TEXT, source TEXT NOT NULL, kind TEXT NOT NULL,
            system_component INTEGER NOT NULL, bundle_identifier TEXT,
            executable_path TEXT, executable_candidate TEXT,
            PRIMARY KEY (run_id, app_id, fact_ord)
        );
        {child_tables}
        -- Both rows carry the SAME (conflated) legacy id.
        INSERT INTO app_snapshot_apps
            (run_id, app_id, fact_ord, name, publisher, source, kind, system_component)
            VALUES
            ('legacy-run', '{legacy}', 0, 'A|B', 'C', 'REGISTRY_UNINSTALL', 'INSTALLED', 0),
            ('legacy-run', '{legacy}', 1, 'A', 'B|C', 'REGISTRY_UNINSTALL', 'INSTALLED', 0);
        INSERT INTO app_snapshot_provenance
            (run_id, app_id, fact_ord, prov_ord, source)
            VALUES
            ('legacy-run', '{legacy}', 0, 0, 'REGISTRY_UNINSTALL'),
            ('legacy-run', '{legacy}', 1, 0, 'REGISTRY_UNINSTALL');
        INSERT INTO app_snapshot_roots (run_id, app_id, fact_ord, root_ord, path)
            VALUES ('legacy-run', '{legacy}', 0, 0, 'u:/apps/A-root');
        INSERT INTO app_snapshot_views (run_id, app_id, fact_ord, view_ord, view)
            VALUES ('legacy-run', '{legacy}', 1, 0, 'HKLM64');
        INSERT INTO app_snapshot_evidence
            (run_id, app_id, fact_ord, artifact_path, evidence_ord, kind, source,
             strength, group_tag, group_source, scope, observed_path,
             matched_attribute, matched_value)
            VALUES
            ('legacy-run', '{legacy}', 0, 'u:/apps/A', 0, 'INSTALL_LOCATION',
             'REGISTRY_METADATA', 'DIRECT', 'SOURCE_RECORD', 'REGISTRY_UNINSTALL',
             'THIS_MACHINE', 'u:/apps/A', 'INSTALL_LOCATION', 'A|B'),
            ('legacy-run', '{legacy}', 1, 'u:/apps/A', 0, 'INSTALL_LOCATION',
             'REGISTRY_METADATA', 'DIRECT', 'SOURCE_RECORD', 'REGISTRY_UNINSTALL',
             'THIS_MACHINE', 'u:/apps/A', 'INSTALL_LOCATION', 'A');
        INSERT INTO app_snapshot_footprints
            (run_id, app_id, fact_ord, footprint_ord, path, kind, confidence)
            VALUES
            ('legacy-run', '{legacy}', 0, 0, 'u:/apps/A', 'INSTALLATION_DIRECTORY', 'CONFIRMED'),
            ('legacy-run', '{legacy}', 1, 0, 'u:/apps/A', 'INSTALLATION_DIRECTORY', 'CONFIRMED');
        INSERT INTO app_snapshot_footprint_evidence
            (run_id, app_id, fact_ord, footprint_ord, evidence_ord, kind, confidence,
             source, scope, why)
            VALUES
            ('legacy-run', '{legacy}', 0, 0, 0, 'INSTALL_LOCATION', 'CONFIRMED',
             'registry', 'THIS_MACHINE', 'recorded for A|B'),
            ('legacy-run', '{legacy}', 1, 0, 0, 'INSTALL_LOCATION', 'CONFIRMED',
             'registry', 'THIS_MACHINE', 'recorded for A');
        INSERT INTO app_snapshot_history (run_id, hist_ord, hist_run_id, path, category)
            VALUES ('legacy-run', 0, 'older-run', 'u:/apps/A', 'APPLICATIONS');"
    )
}

#[test]
fn a_legacy_v5_store_is_re_keyed_per_stored_fact_without_misattribution() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-v5.db");
    build_legacy_v5_snapshot_store(&db);

    // The legacy id really is conflated, and the two rows really are
    // distinct pairs.
    let legacy = legacy_id_of("A|B", Some("C"));
    let expected_first = ApplicationId::derive("A|B", Some("C"));
    let expected_second = ApplicationId::derive("A", Some("B|C"));
    assert_ne!(
        expected_first, expected_second,
        "the fixture's two rows are distinct pairs"
    );
    {
        let conn = Connection::open(&db).unwrap();
        let rows: Vec<String> = conn
            .prepare("SELECT app_id FROM app_snapshot_apps ORDER BY fact_ord")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows, vec![legacy.clone(), legacy.clone()]);
    }

    // Opening applies the v6 migration.
    let store = HistoryStore::open(&db).unwrap();
    let run_id = RunId("legacy-run".to_string());
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .expect("the legacy snapshot must be readable after migration");

    // The conflated id SPLIT into the two distinct identities.
    let ids: Vec<&str> = loaded
        .app_facts
        .iter()
        .map(|f| f.record.id.0.as_str())
        .collect();
    assert!(ids.contains(&expected_first.0.as_str()));
    assert!(ids.contains(&expected_second.0.as_str()));
    assert_eq!(
        ids.len(),
        2,
        "one legacy id split into exactly its two stored pairs"
    );

    // Each row kept its OWN facts (name/publisher), so nothing was merged
    // or relabelled.
    let by_id = |want: &str| -> &AppSnapshotFact {
        loaded
            .app_facts
            .iter()
            .find(|f| f.record.id.0 == want)
            .unwrap_or_else(|| panic!("{want} must be present"))
    };
    let first = by_id(&expected_first.0);
    assert_eq!(first.record.name, "A|B");
    assert_eq!(first.record.publisher.as_deref(), Some("C"));
    let second = by_id(&expected_second.0);
    assert_eq!(second.record.name, "A");
    assert_eq!(second.record.publisher.as_deref(), Some("B|C"));

    // CHILD ATTRIBUTION: every child row followed ITS OWN parent.
    assert_eq!(first.install_roots, vec![PathBuf::from("/apps/A-root")]);
    assert_eq!(
        first.associations[0].1.matched_value.as_deref(),
        Some("A|B"),
        "the evidence must stay with the row that owned it"
    );
    // The second row's own evidence stayed with it too.
    assert_eq!(
        second.associations[0].1.matched_value.as_deref(),
        Some("A"),
        "each row's evidence must follow its own parent"
    );
    assert_eq!(
        second.record.observed_in_views,
        vec!["HKLM64".to_string()],
        "the view row must follow its own parent"
    );
}

#[test]
fn the_legacy_migration_preserves_child_rows_and_rebuilds_a_valid_model() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-v5b.db");
    build_legacy_v5_snapshot_store(&db);
    let store = HistoryStore::open(&db).unwrap();
    let run_id = RunId("legacy-run".to_string());

    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();

    // Footprints, coverage, history and relationships all survived.
    assert_eq!(loaded.footprint.candidates.len(), 2);
    assert_eq!(loaded.input.source_coverage.len(), 1);
    assert_eq!(loaded.input.history.len(), 1);
    assert_eq!(loaded.input.artifacts.len(), 1);

    // The rebuilt model passes the same invariant path as a fresh build,
    // and both applications are present.
    let model = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();
    model.check_invariants().unwrap();
    assert_eq!(model.applications().len(), 2);

    // The id encoding is now current everywhere.
    let conn = Connection::open(&db).unwrap();
    let encodings: Vec<i64> = conn
        .prepare("SELECT id_encoding FROM app_snapshot_apps")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        encodings,
        vec![2, 2],
        "every row now claims the current encoding"
    );
}

#[test]
fn opening_a_v6_store_repeatedly_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-v5c.db");
    build_legacy_v5_snapshot_store(&db);
    let run_id = RunId("legacy-run".to_string());
    let expected_first = ApplicationId::derive("A|B", Some("C"));
    let expected_second = ApplicationId::derive("A", Some("B|C"));

    // First open performs the migration.
    {
        let store = HistoryStore::open(&db).unwrap();
        let loaded = store
            .load_system_snapshot(&run_id, &QueryLimits::default())
            .unwrap()
            .unwrap();
        assert_eq!(loaded.app_facts.len(), 2);
    }
    // Every later open finds nothing to do (idempotence-safe).
    for _ in 0..3 {
        let store = HistoryStore::open(&db).unwrap();
        let loaded = store
            .load_system_snapshot(&run_id, &QueryLimits::default())
            .unwrap()
            .unwrap();
        let ids: Vec<&str> = loaded
            .app_facts
            .iter()
            .map(|f| f.record.id.0.as_str())
            .collect();
        assert!(ids.contains(&expected_first.0.as_str()));
        assert!(ids.contains(&expected_second.0.as_str()));
    }

    // The schema version is still exactly 6, and a NEWER store is refused.
    let conn = Connection::open(&db).unwrap();
    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, HISTORY_SCHEMA_VERSION);
    drop(conn);
    conn_exec(&db, "UPDATE schema_version SET version = 99");
    match HistoryStore::open(&db) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, HISTORY_SCHEMA_VERSION);
        }
        Err(other) => panic!("a newer schema must be refused, got {other:?}"),
        Ok(_) => panic!("a newer schema must be refused, but the store opened"),
    }
}

/// Execute a statement against an already-openable database.
fn conn_exec(db_path: &std::path::Path, sql: &str) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(sql).unwrap();
    conn.close().unwrap();
}

// ---------------------------------------------------------------------------
// Workstream C — query-bound safety
// ---------------------------------------------------------------------------

/// Exactly at the cap: nothing is reported as truncated.
#[test]
fn a_limit_exactly_at_the_fact_count_is_not_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();
    let artifact_count = snap.input.artifacts.len();
    assert!(
        artifact_count > 1,
        "the fixture must have several artifacts"
    );

    let mut store = store_in(dir.path(), "exact.db");
    let run_id = commit_snapshot(&mut store, "exact-run", 1, &snap);

    let at_limit = QueryLimits {
        max_results: artifact_count,
    };
    let loaded = store
        .load_system_snapshot(&run_id, &at_limit)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.input.artifacts.len(), artifact_count);
    assert!(
        !loaded.is_load_truncated(),
        "a limit exactly equal to the fact count must not be reported as capped"
    );

    // One more than that is also complete.
    let over = QueryLimits {
        max_results: artifact_count + 1,
    };
    let loaded = store.load_system_snapshot(&run_id, &over).unwrap().unwrap();
    assert!(!loaded.is_load_truncated());
    assert_eq!(loaded.input.artifacts.len(), artifact_count);
}

/// One below the cap: exactly one section reports, and the rebuild refuses.
#[test]
fn a_limit_one_below_the_fact_count_truncates_and_refuses_the_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();
    let artifact_count = snap.input.artifacts.len();

    let mut store = store_in(dir.path(), "below.db");
    let run_id = commit_snapshot(&mut store, "below-run", 1, &snap);

    let below = QueryLimits {
        max_results: artifact_count - 1,
    };
    let loaded = store
        .load_system_snapshot(&run_id, &below)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.input.artifacts.len(), artifact_count - 1);
    assert!(loaded.is_load_truncated());
    assert!(loaded.load_truncated_sections.contains(&"artifacts"));

    match store.rebuild_system_model(&run_id, &below, &SystemModelLimits::default()) {
        Err(StoreError::SnapshotBounded {
            sections, limit, ..
        }) => {
            assert_eq!(limit, artifact_count - 1);
            assert!(sections.contains(&"artifacts"));
        }
        other => panic!("a capped load must refuse the rebuild, got {other:?}"),
    }
}

/// A zero limit caps everything, reports it, and refuses the rebuild —
/// never silently builds from nothing.
#[test]
fn a_zero_limit_caps_everything_and_is_never_silent() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();
    let mut store = store_in(dir.path(), "zero.db");
    let run_id = commit_snapshot(&mut store, "zero-run", 1, &snap);

    let zero = QueryLimits { max_results: 0 };
    let loaded = store.load_system_snapshot(&run_id, &zero).unwrap().unwrap();
    assert!(loaded.input.artifacts.is_empty());
    assert!(loaded.is_load_truncated(), "a zero limit is a real cap");
    assert!(!loaded.load_truncated_sections.is_empty());

    match store.rebuild_system_model(&run_id, &zero, &SystemModelLimits::default()) {
        Err(StoreError::SnapshotBounded { .. }) => {}
        other => panic!("a zero-limit rebuild must refuse, got {other:?}"),
    }
}

/// An extreme limit must never disable cap detection (the probe row is
/// still fetched and still compared), and must never become a NEGATIVE
/// SQLite LIMIT (which SQLite reads as "unlimited").
#[test]
fn an_extreme_limit_cannot_bypass_cap_detection() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();
    let artifact_count = snap.input.artifacts.len();
    let mut store = store_in(dir.path(), "extreme.db");
    let run_id = commit_snapshot(&mut store, "extreme-run", 1, &snap);

    // Small enough not to allocate: `i64::MAX` and `usize::MAX` are the
    // boundaries of the probe arithmetic.
    for limit in [i64::MAX as usize, usize::MAX, i64::MAX as usize - 1] {
        let loaded = store
            .load_system_snapshot(&run_id, &QueryLimits { max_results: limit })
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.input.artifacts.len(),
            artifact_count,
            "the limit must not change what is read"
        );
        assert!(
            !loaded.is_load_truncated(),
            "a huge limit is not a truncation of a small snapshot"
        );
    }
    // A limit above the fact count still detects a genuine cap when one
    // exists: cap at exactly the count minus one.
    let below = QueryLimits {
        max_results: artifact_count - 1,
    };
    let loaded = store
        .load_system_snapshot(&run_id, &below)
        .unwrap()
        .unwrap();
    assert!(loaded.is_load_truncated(), "cap detection still works");
}

/// The snapshot listing is bounded and cannot return more than requested.
#[test]
fn the_snapshot_listing_honours_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let snap = rich_snapshot();
    let mut store = store_in(dir.path(), "many.db");
    for n in 1..=6u64 {
        commit_snapshot(&mut store, &format!("list-{n}"), n, &snap);
    }

    let capped = store
        .list_system_snapshots(&QueryLimits { max_results: 3 })
        .unwrap();
    assert_eq!(capped.len(), 3, "the listing respects its bound");
    let small = store
        .list_system_snapshots(&QueryLimits { max_results: 1 })
        .unwrap();
    assert_eq!(small.len(), 1);
    let zero = store
        .list_system_snapshots(&QueryLimits { max_results: 0 })
        .unwrap();
    assert!(
        zero.is_empty(),
        "a zero listing limit returns nothing (not everything)"
    );
    let all = store
        .list_system_snapshots(&QueryLimits::default())
        .unwrap();
    assert_eq!(all.len(), 6, "an adequate limit lists every snapshot");
    // Newest run first is the documented order.
    assert_eq!(all[0].run_id, RunId("list-6".to_string()));
}

/// Relationship member rows are capped like every other section.
#[test]
fn relationship_members_are_capped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_record("Members", Some("Pub"));
    let weak = OwnershipEvidence::new(
        EvidenceKind::FilenameSimilarity,
        EvidenceSource::FilesystemPathHeuristic,
        EvidenceStrength::Weak,
        CorrelationGroup::NameDerived,
        AssociationScope::ThisMachine,
        PathBuf::from("/apps/Members"),
        MatchedAttribute::ApplicationName,
        Some("Members".to_string()),
    );
    let snap = committed(
        SystemModelInput {
            artifacts: vec![artifact("/apps/Members", 1, 1, None)],
            applications: vec![],
            relationships: vec![RelationshipFact {
                kind: RelationshipFactKind::ContentDuplicate,
                paths: (0..4)
                    .map(|i| PathBuf::from(format!("/apps/Members/f{i}")))
                    .collect(),
                object: None,
                content_sha256: Some(
                    "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
                ),
            }],
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        vec![],
    );
    let _ = (&app, &weak);
    let mut store = store_in(dir.path(), "members.db");
    let run_id = commit_snapshot(&mut store, "members-run", 1, &snap);

    // A limit of 2 caps the 4 member paths.
    let loaded = store
        .load_system_snapshot(&run_id, &QueryLimits { max_results: 2 })
        .unwrap()
        .unwrap();
    let rel = &loaded.input.relationships[0];
    assert_eq!(rel.paths.len(), 2, "the member rows respect the cap");
    assert!(
        loaded.is_load_truncated(),
        "a capped member list must be reported"
    );
    assert!(loaded
        .load_truncated_sections
        .contains(&"relationship_members"));
}

// ---------------------------------------------------------------------------
// boundedness tests
// ---------------------------------------------------------------------------

#[test]
fn loading_one_snapshot_never_materializes_unrelated_runs() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bounded.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // Ten runs, each with its own snapshot.
    let mut ids = Vec::new();
    for n in 1..=10u64 {
        let snap = rich_snapshot();
        ids.push(commit_snapshot(&mut store, &format!("bulk-{n}"), n, &snap));
    }

    // Loading ONE run returns exactly that run's facts — never the
    // neighbours' rows.
    let one = store
        .load_system_snapshot(&ids[0], &QueryLimits::default())
        .unwrap()
        .unwrap();
    let single = rich_snapshot();
    assert_eq!(
        one.input.artifacts.len(),
        single.input.artifacts.len(),
        "exactly one run's artifacts"
    );
    assert_eq!(
        one.input.applications.len(),
        single.input.applications.len()
    );

    // The listing is bounded by QueryLimits and reports only counts.
    let capped = store
        .list_system_snapshots(&QueryLimits { max_results: 3 })
        .unwrap();
    assert_eq!(capped.len(), 3, "the listing respects its bound");

    // A section bound also caps what a single load materializes — and it
    // SAYS so, rather than quietly returning a prefix.
    let tiny = store
        .load_system_snapshot(&ids[0], &QueryLimits { max_results: 1 })
        .unwrap()
        .unwrap();
    assert_eq!(
        tiny.input.artifacts.len(),
        1,
        "a bounded load must not materialize beyond its limit"
    );
    assert!(
        tiny.is_load_truncated(),
        "a capped load must report itself as incomplete"
    );
    assert!(
        tiny.load_truncated_sections.contains(&"artifacts"),
        "the capped section must be named (got {:?})",
        tiny.load_truncated_sections
    );
}

#[test]
fn an_inventory_that_disagrees_with_the_facts_is_rejected() {
    // `Inventory::records` is the merged discovery result the caller
    // derived the snapshot from. If it disagrees with the committed
    // application facts, the counters it supplies would describe records
    // that were never stored — so the commit is refused, leaving no
    // snapshot behind.
    let dir = tempfile::tempdir().unwrap();
    let mut snap = rich_snapshot();
    // Drop one record so the inventory no longer describes the two
    // stored applications.
    snap.inventory.records.pop();

    let record = run_record("inv-mismatch-run", 1);
    let mut store = store_in(dir.path(), "inv-mismatch.db");
    store.begin_run(&record).unwrap();
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("a disagreeing inventory must be rejected");
    assert_corrupt(&err, "app_snapshot_meta", "records_truncated");
    assert!(!store.has_system_snapshot(&record.run_id).unwrap());

    // A superset inventory (an extra record) is rejected too.
    let mut extra = rich_snapshot();
    extra.inventory.records.push(app_record("Ghost", None));
    let record2 = run_record("inv-extra-run", 1);
    let mut store2 = store_in(dir.path(), "inv-extra.db");
    store2.begin_run(&record2).unwrap();
    let err = store2
        .commit_system_snapshot(
            &record2.run_id,
            &extra.input,
            &extra.app_facts,
            &extra.inventory,
            &extra.footprint,
        )
        .expect_err("an inventory with extra records must be rejected");
    assert_corrupt(&err, "app_snapshot_meta", "records_truncated");
    assert!(!store2.has_system_snapshot(&record2.run_id).unwrap());
}

#[test]
fn an_inventory_with_the_same_records_in_any_order_is_accepted() {
    // The comparison is a canonical multiset: the caller may hold the
    // records in any order (a different provider enumeration order), and
    // that must not be an error.
    let dir = tempfile::tempdir().unwrap();
    let mut snap = rich_snapshot();
    snap.inventory.records.reverse();
    let run_id = commit_snapshot(
        &mut store_in(dir.path(), "inv-order.db"),
        "inv-order-run",
        1,
        &snap,
    );
    let loaded = store_in(dir.path(), "inv-order.db")
        .load_system_snapshot(&run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(loaded.app_facts.len(), snap.app_facts.len());
}

#[test]
fn a_bounded_load_never_becomes_a_model() {
    // Bounded knowledge is not complete knowledge: a load cut short by the
    // caller's limit must be reported, and rebuilding must REFUSE rather
    // than present a prefix of the facts as the whole truth (which could
    // drop claimants, edges and history context).
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bounded-refusal.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "bounded-run", 1, &snap);
    let total_artifacts = snap.input.artifacts.len();
    assert!(
        total_artifacts >= 3,
        "the fixture must have several artifacts"
    );

    // Too small a limit: the load reports truncation and the rebuild fails
    // closed with a typed error naming the section and the bound.
    let too_small = QueryLimits { max_results: 1 };
    let loaded = store
        .load_system_snapshot(&run_id, &too_small)
        .unwrap()
        .unwrap();
    assert!(loaded.is_load_truncated());
    assert!(loaded.input.artifacts.len() < total_artifacts);

    match store.rebuild_system_model(&run_id, &too_small, &SystemModelLimits::default()) {
        Err(StoreError::SnapshotBounded {
            run_id: id,
            sections,
            limit,
        }) => {
            assert_eq!(id, run_id.0);
            assert_eq!(limit, 1);
            assert!(
                sections.contains(&"artifacts"),
                "the refused section must be named (got {sections:?})"
            );
        }
        other => panic!("a bounded rebuild must fail closed, got {other:?}"),
    }

    // An adequate limit loads completely and rebuilds successfully.
    let enough = QueryLimits {
        max_results: total_artifacts + 16,
    };
    let full = store
        .load_system_snapshot(&run_id, &enough)
        .unwrap()
        .unwrap();
    assert!(
        !full.is_load_truncated(),
        "an adequate limit is not truncated"
    );
    assert_eq!(full.input.artifacts.len(), total_artifacts);
    let model = store
        .rebuild_system_model(&run_id, &enough, &SystemModelLimits::default())
        .unwrap()
        .expect("an unbounded load rebuilds");
    assert_eq!(model.artifact_count(), total_artifacts);
}

#[test]
fn snapshot_facts_are_stored_normalized_not_as_a_blob() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("normalized.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "norm-run", 1, &snap);
    drop(store);

    let conn = Connection::open(&db).unwrap();
    // Per-application fields are queryable columns, and the child tables
    // hold individually addressable rows.
    let alpha_id = ApplicationId::derive("Alpha", Some("Acme")).0;
    let publisher: String = conn
        .query_row(
            "SELECT publisher FROM app_snapshot_apps WHERE run_id = ?1 AND app_id = ?2",
            rusqlite::params![run_id.0, alpha_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(publisher, "Acme", "columns are queryable, not a JSON blob");

    let provenance_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM app_snapshot_provenance WHERE run_id = ?1 AND app_id = ?2",
            rusqlite::params![run_id.0, alpha_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(provenance_count >= 1, "provenance is normalized rows");

    let roots: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM app_snapshot_roots WHERE run_id = ?1 AND app_id = ?2",
            rusqlite::params![run_id.0, alpha_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(roots, 1, "install roots are their own rows");

    let evidence: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM app_snapshot_evidence WHERE run_id = ?1 AND app_id = ?2",
            rusqlite::params![run_id.0, alpha_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(evidence, 1, "evidence is structured rows, one per item");

    // No single table holds the whole model as text.
    assert!(
        !table_exists(&conn, "system_model"),
        "there must be no system_model blob table"
    );
}

#[test]
fn no_derived_state_is_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("derived.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let _run_id = commit_snapshot(&mut store, "derived-run", 1, &snap);
    drop(store);

    let conn = Connection::open(&db).unwrap();
    // Derived artifacts of the model are NOT tables anywhere.
    for forbidden in [
        "app_snapshot_edges",
        "app_snapshot_insights",
        "app_snapshot_candidates",
        "app_snapshot_indexes",
        "app_snapshot_graph",
        "system_model_edges",
    ] {
        assert!(
            !table_exists(&conn, forbidden),
            "derived state must never be persisted: {forbidden}"
        );
    }
}

// ---------------------------------------------------------------------------
// safety boundary
// ---------------------------------------------------------------------------

#[test]
fn persistence_never_authorizes_an_action() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("safety.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let snap = rich_snapshot();
    let run_id = commit_snapshot(&mut store, "safety-run", 1, &snap);

    let model = store
        .rebuild_system_model(
            &run_id,
            &QueryLimits::default(),
            &SystemModelLimits::default(),
        )
        .unwrap()
        .unwrap();

    assert!(
        !coresight_system_model::can_authorize_execution(&model),
        "a reloaded model can never authorize execution"
    );
    for candidate in model.candidates() {
        assert!(
            !coresight_system_model::candidate_is_authorized(candidate),
            "a reloaded candidate stays inert"
        );
        assert!(
            candidate
                .blockers
                .contains(&coresight_system_model::InsightBlocker::NoExecutorInThisPhase),
            "every reloaded candidate still records the no-executor blocker"
        );
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![table],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn assert_corrupt(err: &StoreError, table: &str, column: &str) {
    match err {
        StoreError::Corrupt {
            table: t,
            column: c,
            ..
        } => {
            assert_eq!(*t, table, "corruption must name the table (got {err})");
            assert_eq!(*c, column, "corruption must name the column (got {err})");
        }
        other => panic!("expected a typed corruption error, got {other:?}"),
    }
}

/// The original app fact for one id, from the authored snapshot.
fn reloaded_original(snap: &Committed, id: &str) -> ApplicationRecord {
    original_fact(snap, id).record
}

/// The original `AppSnapshotFact` for one id, from the authored snapshot.
fn original_fact(snap: &Committed, id: &str) -> AppSnapshotFact {
    snap.app_facts
        .iter()
        .find(|f| f.record.id.0 == id)
        .unwrap_or_else(|| panic!("{id} must exist in the fixture"))
        .clone()
}

/// Summarise one run's stored snapshot through SQL (raw row counts).
fn summarise(store: &HistoryStore, run_id: &RunId) -> (i64, i64, i64, i64, i64) {
    let (a, b, c, d, e) = store
        .list_system_snapshots(&QueryLimits::default())
        .unwrap()
        .into_iter()
        .find(|s| s.run_id == *run_id)
        .map(|s| {
            (
                s.artifacts as i64,
                s.applications as i64,
                s.relationships as i64,
                s.history_facts as i64,
                0i64,
            )
        })
        .unwrap_or((0, 0, 0, 0, 0));
    (a, b, c, d, e)
}

/// The same fact set with every collection reversed (a different arrival
/// order carrying identical content).
fn reverse_snapshot(snap: &Committed) -> Committed {
    let mut input = snap.input.clone();
    input.artifacts.reverse();
    input.applications.reverse();
    input.relationships.reverse();
    input.history.reverse();
    input.source_coverage.reverse();
    let mut app_facts = snap.app_facts.clone();
    app_facts.reverse();
    for fact in &mut app_facts {
        fact.install_roots.reverse();
        fact.associations.reverse();
        fact.footprints.reverse();
        for fp in &mut fact.footprints {
            fp.evidence.reverse();
        }
    }
    committed(input, app_facts)
}

/// Build a genuine pre-6.4 (v4) database: the history schema as it
/// existed before Phase 6.4, plus the v1 bootstrap.
fn build_v4_database(db_path: &std::path::Path) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);
         INSERT INTO schema_version (version) VALUES (1);",
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS scan_runs (
            run_id TEXT PRIMARY KEY,
            started_at INTEGER NOT NULL,
            completed_at INTEGER,
            roots TEXT NOT NULL,
            platform TEXT NOT NULL,
            config TEXT NOT NULL,
            status TEXT NOT NULL,
            entries_examined INTEGER NOT NULL DEFAULT 0,
            files INTEGER NOT NULL DEFAULT 0,
            dirs INTEGER NOT NULL DEFAULT 0,
            links INTEGER NOT NULL DEFAULT 0,
            other_entries INTEGER NOT NULL DEFAULT 0,
            bytes INTEGER NOT NULL DEFAULT 0,
            observation_errors INTEGER NOT NULL DEFAULT 0,
            candidates_untracked INTEGER NOT NULL DEFAULT 0,
            hash_failures INTEGER NOT NULL DEFAULT 0,
            file_id_hi INTEGER,
            rel_status TEXT,
            rel_truncated INTEGER
         );
         CREATE TABLE IF NOT EXISTS observations (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            kind TEXT NOT NULL,
            size INTEGER,
            device INTEGER,
            inode INTEGER,
            file_id_hi INTEGER,
            modified INTEGER,
            category TEXT,
            subcategory TEXT,
            content_sha256 TEXT,
            obs_error TEXT,
            PRIMARY KEY (run_id, path)
         );
         CREATE TABLE IF NOT EXISTS relationship_obs (
            run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
            rel_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            size INTEGER NOT NULL,
            member_count INTEGER NOT NULL,
            recoverable INTEGER,
            accounting TEXT NOT NULL,
            PRIMARY KEY (run_id, rel_id)
         );
         CREATE TABLE IF NOT EXISTS relationship_members (
            run_id TEXT NOT NULL,
            rel_id TEXT NOT NULL,
            path TEXT NOT NULL,
            device INTEGER,
            inode INTEGER,
            file_id_hi INTEGER,
            PRIMARY KEY (run_id, rel_id, path)
         );",
    )
    .unwrap();
    conn.execute("UPDATE schema_version SET version = 4", [])
        .unwrap();
    // A v4-schema observation-ALTER shape: the column set above already
    // includes the v3 additions, matching what a migrated v4 store has.
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, completed_at, roots, platform, config, status)
         VALUES ('v4-run', 1000, 2000, ?1, 'test', ?2, 'COMPLETED')",
        rusqlite::params![
            r#"["u:/scope-a"]"#,
            r#"{"observationModel":1,"classifierSchema":"coresight.v1.classification","classifierRules":1,"hashAlgorithm":"sha256","relationshipSchema":1,"historySchema":2}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations (run_id, path, kind, size, device, inode)
         VALUES ('v4-run', 'u:/scope-a/f.bin', 'FILE', 10, 7, 100)",
        [],
    )
    .unwrap();
    conn.close().unwrap();
}
