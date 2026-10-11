//! Phase 6.4.2 — persistence boundary, query bounds, and retention safety.
//!
//! Every fixture is synthetic and portable: no test touches a real
//! filesystem path, spawns a process, or reads the machine.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, AssociationScope, Confidence,
    EvidenceKind, FootprintCandidate, FootprintEvidence, FootprintKind, FootprintReport, Inventory,
    PackageKind, ProbedKind,
};
use coresight_capabilities::access::AccessState;
use coresight_history::{
    AppSnapshotFact, ConfigFingerprint, HistoryStore, QueryLimits, RetentionPolicy, RunCounts,
    RunId, RunRecord, RunStatus, SnapshotBuilder, StoreError,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{ArtifactFact, SystemModelInput};

// ---------------------------------------------------------------------------
// Workstream A — identity validation before persistence
// ---------------------------------------------------------------------------

fn app_record(name: &str, publisher: Option<&str>) -> ApplicationRecord {
    ApplicationRecord {
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
        identity: Some(ObjectIdentity::narrow(1, 1)),
        content_sha256: None,
        size: Some(1),
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

/// Replace the id inside one application fact with a forged value, in
/// EVERY place the commit consults — the record, the input entry, and the
/// footprint attribution — so nothing but the identity mismatch remains.
fn forge_id(snap: &mut SnapshotFixture, app_index: usize, forged: &ApplicationId) {
    let mut record = snap.app_facts[app_index].record.clone();
    record.id = forged.clone();
    snap.app_facts[app_index].record = record.clone();
    snap.input.applications[app_index].record = record;
    for fp in &mut snap.app_facts[app_index].footprints {
        fp.app = forged.clone();
    }
}

struct SnapshotFixture {
    input: SystemModelInput,
    app_facts: Vec<AppSnapshotFact>,
    inventory: Inventory,
    footprint: FootprintReport,
}

fn fixture() -> SnapshotFixture {
    let app = app_record("Real App", Some("Acme"));
    let input = SystemModelInput {
        artifacts: vec![artifact("/apps/Real App")],
        applications: vec![coresight_system_model::ApplicationFact {
            record: app.clone(),
            install_roots: vec![PathBuf::from("/apps/Real App")],
            executable: None,
            associations: Vec::new(),
        }],
        relationships: Vec::new(),
        history: Vec::new(),
        source_coverage: vec![coresight_apps::SourceCoverage::complete("win32-uninstall")],
    };
    let inventory = Inventory {
        records: vec![app.clone()],
        ..Inventory::default()
    };
    SnapshotFixture {
        input,
        app_facts: vec![AppSnapshotFact {
            record: app,
            install_roots: vec![PathBuf::from("/apps/Real App")],
            executable: None,
            associations: Vec::new(),
            footprints: Vec::new(),
        }],
        inventory,
        footprint: FootprintReport::default(),
    }
}

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

/// A forged `ApplicationId` supplied consistently in every parallel input
/// must be rejected before any row is written.
/// Claim 11: producer admission, persistence canonicalization, and reload
/// must agree. A candidate set published by the producer, committed, and
/// reloaded must come back IDENTICAL — including the reconciled evidence
/// and the surviving confidence — so no layer normalizes differently.
#[test]
fn producer_commit_and_reload_produce_identical_footprints() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("policy.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("policy-run");
    store.begin_run(&record).unwrap();

    // One scope described two ways; the winner carries DUPLICATE and
    // deliberately REORDERED evidence so normalization is observable.
    let e_confirmed = FootprintEvidence::new(
        EvidenceKind::InstallLocation,
        Confidence::Confirmed,
        "registry",
        AssociationScope::ThisMachine,
        "recorded by the installer",
    );
    let e_reference = FootprintEvidence::new(
        EvidenceKind::RegistryReference,
        Confidence::Confirmed,
        "registry",
        AssociationScope::ThisMachine,
        "uninstall key present",
    );
    let mut winner = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: ApplicationId::derive("Real App", Some("Acme")),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Confirmed,
        evidence: vec![
            e_reference.clone(),
            e_confirmed.clone(),
            e_confirmed.clone(),
        ],
    };
    let weak = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: ApplicationId::derive("Real App", Some("Acme")),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Possible,
        evidence: vec![e_confirmed.clone()],
    };

    // 1. Producer admission normalizes the winner (dedup + canonical order).
    coresight_apps::footprint::normalize_candidate_evidence(&mut winner);
    assert_eq!(winner.evidence.len(), 2, "the duplicate must collapse");
    assert_eq!(
        winner.evidence[0].why, e_confirmed.why,
        "canonical evidence order must be deterministic"
    );

    // 2. Publish the producer's reconciled view of the same set.
    let mut published = vec![weak.clone(), winner.clone()];
    published.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
            .then(
                coresight_apps::footprint::candidate_rank(b)
                    .cmp(&coresight_apps::footprint::candidate_rank(a)),
            )
    });
    published.dedup_by(|a, b| a.path == b.path && a.app == b.app && a.kind == b.kind);
    assert_eq!(published.len(), 1, "one scope, reconciled");
    assert_eq!(published[0].confidence, Confidence::Confirmed);

    let snap = SnapshotFixture {
        input: SystemModelInput {
            artifacts: vec![artifact("/apps/Real App")],
            applications: vec![coresight_system_model::ApplicationFact {
                record: app_record("Real App", Some("Acme")),
                install_roots: vec![PathBuf::from("/apps/Real App")],
                executable: None,
                associations: Vec::new(),
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![],
        },
        app_facts: vec![AppSnapshotFact {
            record: app_record("Real App", Some("Acme")),
            install_roots: vec![PathBuf::from("/apps/Real App")],
            executable: None,
            associations: Vec::new(),
            footprints: vec![weak.clone(), winner.clone()],
        }],
        inventory: Inventory {
            records: vec![app_record("Real App", Some("Acme"))],
            ..Inventory::default()
        },
        footprint: FootprintReport {
            candidates: vec![weak, winner.clone()],
            ..FootprintReport::default()
        },
    };
    store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .unwrap();

    // 3. Reload and compare against the producer's reconciled view.
    let loaded = store
        .load_system_snapshot(&record.run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let reloaded = &loaded.footprint.candidates;
    assert_eq!(
        reloaded.len(),
        1,
        "the reload must see exactly the reconciled scope"
    );
    assert_eq!(
        reloaded[0].confidence, published[0].confidence,
        "confidence must survive commit/reload identically"
    );
    assert_eq!(
        reloaded[0].evidence, published[0].evidence,
        "evidence (order, content, duplicates removed) must survive identically"
    );

    // 4. The rebuilt model consumes the same facts and passes the same
    //    invariants as a fresh build. It does not re-expose candidates:
    //    derived structures are never persisted or serialized.
    let model = store
        .rebuild_system_model(
            &record.run_id,
            &QueryLimits::default(),
            &core_system_limits(),
        )
        .unwrap()
        .unwrap();
    model.check_invariants().unwrap();
    assert_eq!(
        model.applications().len(),
        1,
        "the model must reconstruct the one application that owned the scope"
    );
    assert_eq!(
        model.applications()[0].id,
        published[0].app,
        "the reconciled scope's application must be the one in the model"
    );
}

#[test]
fn a_forged_application_id_is_rejected_before_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = HistoryStore::open(&dir.path().join("s.db")).unwrap();
    let record = run_record("forged-run");
    store.begin_run(&record).unwrap();

    let mut snap = fixture();
    // Derive an id from a DIFFERENT (name, publisher) — the forgery.
    let forged = ApplicationId::derive("Someone Else", Some("Evil"));
    forge_id(&mut snap, 0, &forged);
    snap.inventory.records[0].id = forged.clone();

    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .expect_err("a forged id must be rejected");
    match err {
        StoreError::Corrupt { column, .. } => assert_eq!(column, "app_id"),
        other => panic!("expected a typed identity corruption error, got {other:?}"),
    }
    // Nothing was written, so there is no snapshot to reload.
    assert!(!store
        .has_system_snapshot(&RunId("forged-run".to_string()))
        .unwrap());
    assert!(store
        .load_system_snapshot(&record.run_id, &QueryLimits::default())
        .unwrap()
        .is_none());
}

/// An empty or whitespace-only name cannot pass identity validation and
/// must not become a meaningful (empty-named) application on reload.
#[test]
fn empty_and_whitespace_only_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for (i, bad_name) in ["", "   ", "\t\n "].iter().enumerate() {
        let db = dir.path().join(format!("blank-{i}.db"));
        let mut store = HistoryStore::open(&db).unwrap();
        let record = run_record(&format!("blank-run-{i}"));
        store.begin_run(&record).unwrap();

        // Keep the id consistent with the name so ONLY the blank name is
        // the defect — that isolates the validation from the id check.
        let mut snap = fixture();
        let record_with_blank = {
            let mut r = snap.app_facts[0].record.clone();
            r.name = (*bad_name).to_string();
            r.id = ApplicationId::derive(bad_name, Some("Acme"));
            r
        };
        snap.app_facts[0].record = record_with_blank.clone();
        snap.input.applications[0].record = record_with_blank.clone();
        snap.inventory.records[0] = record_with_blank;

        let err = store
            .commit_system_snapshot(
                &record.run_id,
                &snap.input,
                &snap.app_facts,
                &snap.inventory,
                &snap.footprint,
            )
            .expect_err("a blank name must be rejected");
        match err {
            StoreError::Corrupt { column, .. } => assert_eq!(column, "app_id"),
            other => panic!("expected a typed identity error, got {other:?}"),
        }
        assert!(!store.has_system_snapshot(&record.run_id).unwrap());
    }
}

/// A valid existing snapshot survives an invalid replacement: the
/// previously committed facts still reload and still build the same model.
#[test]
fn an_invalid_replacement_preserves_the_valid_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("preserve.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // 1. Commit a VALID snapshot.
    let snap = fixture();
    let record = run_record("preserve-run");
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
    let before = store
        .load_system_snapshot(&record.run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    let before_model = store
        .rebuild_system_model(
            &record.run_id,
            &QueryLimits::default(),
            &core_system_limits(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(before.app_facts.len(), 1);

    // 2. Attempt an INVALID replacement (forged id everywhere).
    let mut bad = fixture();
    let forged = ApplicationId::derive("Attacker", Some("Acme"));
    forge_id(&mut bad, 0, &forged);
    bad.inventory.records[0].id = forged.clone();
    let err = store
        .commit_system_snapshot(
            &record.run_id,
            &bad.input,
            &bad.app_facts,
            &bad.inventory,
            &bad.footprint,
        )
        .expect_err("the replacement must be rejected");
    assert!(matches!(err, StoreError::Corrupt { .. }));

    // 3. The valid snapshot is untouched and still reloads + rebuilds.
    let after = store
        .load_system_snapshot(&record.run_id, &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        after.app_facts.len(),
        before.app_facts.len(),
        "the previous snapshot must survive a rejected replacement"
    );
    assert_eq!(
        after.app_facts[0].record.id.0,
        snap.app_facts[0].record.id.0
    );
    let after_model = store
        .rebuild_system_model(
            &record.run_id,
            &QueryLimits::default(),
            &core_system_limits(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        before_model, after_model,
        "the rebuilt model must be identical after a rejected replacement"
    );
    after_model.check_invariants().unwrap();
}

/// Valid normalized case/whitespace variants still commit, because
/// normalization folds them to one identity (that is documented policy,
/// not a validation gap).
#[test]
fn normalized_case_and_whitespace_variants_are_still_valid() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("variants.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("variants-run");
    store.begin_run(&record).unwrap();

    // "real app"/" ACME " normalizes to the same pair as "Real App"/"Acme".
    let mut record = app_record("Real App", Some("Acme"));
    record.name = "real app".to_string();
    record.publisher = Some(" ACME ".to_string());
    record.id = ApplicationId::derive(&record.name, record.publisher.as_deref());
    assert_eq!(
        record.id,
        app_record("Real App", Some("Acme")).id,
        "normalization must fold the variant to the same identity"
    );

    let snap = SnapshotFixture {
        input: SystemModelInput {
            artifacts: vec![artifact("/apps/Real App")],
            applications: vec![coresight_system_model::ApplicationFact {
                record: record.clone(),
                install_roots: vec![PathBuf::from("/apps/Real App")],
                executable: None,
                associations: Vec::new(),
            }],
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![coresight_apps::SourceCoverage::complete("win32-uninstall")],
        },
        app_facts: vec![AppSnapshotFact {
            record: record.clone(),
            install_roots: vec![PathBuf::from("/apps/Real App")],
            executable: None,
            associations: Vec::new(),
            footprints: Vec::new(),
        }],
        inventory: Inventory {
            records: vec![record],
            ..Inventory::default()
        },
        footprint: FootprintReport::default(),
    };
    store
        .commit_system_snapshot(
            &RunId("variants-run".to_string()),
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .unwrap();

    let loaded = store
        .load_system_snapshot(&RunId("variants-run".to_string()), &QueryLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(loaded.app_facts.len(), 1);
    assert_eq!(
        loaded.app_facts[0].record.id,
        app_record("Real App", Some("Acme")).id,
        "the reloaded id must be the canonical one"
    );
}

fn core_system_limits() -> coresight_system_model::SystemModelLimits {
    coresight_system_model::SystemModelLimits::default()
}

// ---------------------------------------------------------------------------
// Workstream B — safe shared query limits
// ---------------------------------------------------------------------------

/// Zero, normal, exact-cap, one-over, largest-supported, and
/// first-unsupported values behave exactly as documented, and no limit
/// can silently become an unbounded (negative) SQLite `LIMIT`.
#[test]
fn query_limit_conversion_is_checked_everywhere() {
    // Ordinary and boundary values convert exactly.
    for (limit, expected) in [
        (0usize, 0i64),
        (1, 1),
        (10_000, 10_000),
        (QueryLimits::MAX_SUPPORTED, i64::MAX),
    ] {
        let got = QueryLimits { max_results: limit }.to_sql_limit().unwrap();
        assert_eq!(got, expected, "limit {limit} must convert exactly");
    }

    // The first unsupported value (i64::MAX + 1) is rejected, NOT wrapped
    // to a negative/unbounded limit.
    let first_unsupported = QueryLimits::MAX_SUPPORTED + 1;
    let first_unsupported_limits = QueryLimits {
        max_results: first_unsupported,
    };
    match first_unsupported_limits.to_sql_limit() {
        Err(StoreError::LimitOutOfRange { requested, .. }) => {
            assert_eq!(requested, first_unsupported);
        }
        other => panic!("an unrepresentable limit must be refused, got {other:?}"),
    }
    // usize::MAX is rejected too.
    assert!(matches!(
        QueryLimits {
            max_results: usize::MAX
        }
        .to_sql_limit(),
        Err(StoreError::LimitOutOfRange { .. })
    ));

    // Probe limits are exact within the supported domain.
    assert_eq!(
        QueryLimits { max_results: 0 }.to_probe_sql_limit().unwrap(),
        1
    );
    assert_eq!(
        QueryLimits { max_results: 4 }.to_probe_sql_limit().unwrap(),
        5
    );
    assert_eq!(
        QueryLimits {
            max_results: QueryLimits::MAX_SUPPORTED
        }
        .to_probe_sql_limit()
        .unwrap(),
        i64::MAX,
        "the probe at the domain boundary stays positive (never negative)"
    );
}

/// Extreme retention durations must never delete history.
///
/// Duration::as_nanos() is u128 and wider than i64; the old code
/// narrowed it with s i64, so any policy above ~292 years wrapped to a
/// NEGATIVE age. Combined with saturating_sub, that inverted the rule:
/// fresh runs were treated as ancient and deleted, or genuinely ancient
/// runs were kept forever. The comparison is now done in the wide domain.
#[test]
fn extreme_retention_durations_never_delete_history() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("retention.db");
    let mut store = HistoryStore::open(&db).unwrap();

    // Commit a run.
    let record = run_record("retention-run");
    store.begin_run(&record).unwrap();
    let snapshot = SnapshotBuilder::new().build(record.run_id.clone()).unwrap();
    store.commit_run(&record, &snapshot, None).unwrap();
    assert!(store.get_run(&record.run_id).unwrap().is_some());

    // A policy age that cannot be represented in signed nanoseconds
    // (300 years > ~292.47 years) must keep the run, not delete it.
    let huge = Duration::from_secs(300u64 * 365 * 24 * 3600);
    let report = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 1,
            max_runs: None,
            max_age: Some(huge),
        })
        .unwrap();
    assert!(
        report.removed_runs.is_empty(),
        "a 300-year retention age must never delete a fresh run (removed {:?})",
        report.removed_runs
    );
    assert!(store.get_run(&record.run_id).unwrap().is_some());

    // Duration::MAX behaves the same: nothing is deleted.
    let report = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 1,
            max_runs: None,
            max_age: Some(Duration::MAX),
        })
        .unwrap();
    assert!(report.removed_runs.is_empty());
    assert!(store.get_run(&record.run_id).unwrap().is_some());

    // Ordinary retention behaviour is unchanged: a tiny age still prunes
    // beyond the keep_latest floor.
    let report = store
        .apply_retention(&RetentionPolicy {
            keep_latest: 0,
            max_runs: None,
            max_age: Some(Duration::from_secs(0)),
        })
        .unwrap();
    assert_eq!(
        report.removed_runs,
        vec![record.run_id.clone()],
        "a zero age must still prune normally"
    );
    assert!(store.get_run(&record.run_id).unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Workstream D — one footprint-evidence policy everywhere
// ---------------------------------------------------------------------------

/// A timestamp beyond the store's nanosecond domain is rejected, not
/// silently wrapped into a plausible past date (Phase 6.4.2, Workstream C:
/// `Duration::as_nanos()` is `u128`, wider than `i64`).
#[test]
fn an_unrepresentable_timestamp_is_rejected_not_wrapped() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("timestamp.db");

    // Year 3000 is far past the ~2262 nanosecond ceiling.
    let far_future = UNIX_EPOCH + Duration::from_secs(32503680000);
    let mut record = run_record("timestamp-run");
    record.started_at = far_future;

    let mut store = HistoryStore::open(&db).unwrap();
    let err = store
        .begin_run(&record)
        .expect_err("an unrepresentable timestamp must be rejected");
    match err {
        StoreError::Corrupt { column, .. } => assert_eq!(column, "started_at"),
        other => panic!("expected typed corruption, got {other:?}"),
    }
    // No row was written, so a prior snapshot for the same run cannot be
    // damaged by a later failed commit.
    assert!(store.get_run(&record.run_id).unwrap().is_none());
}

/// An ordinary terminal timestamp still commits normally, and a run with
/// an unrepresentable completed_at cannot overwrite an existing RUNNING row.
#[test]
fn an_unrepresentable_completed_at_preserves_the_run_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("preserve2.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let record = run_record("preserve2-run");
    store.begin_run(&record).unwrap();
    let before = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(before.status, RunStatus::Running);

    let mut terminal = record.clone();
    terminal.completed_at = Some(UNIX_EPOCH + Duration::from_secs(32503680000));
    let snapshot = SnapshotBuilder::new().build(record.run_id.clone()).unwrap();
    let err = store
        .commit_run(&terminal, &snapshot, None)
        .expect_err("an unrepresentable completed_at must be rejected");
    match err {
        StoreError::Corrupt { column, .. } => assert_eq!(column, "started_at"),
        other => panic!("expected typed corruption, got {other:?}"),
    }
    let after = store.get_run(&record.run_id).unwrap().unwrap();
    assert_eq!(after.status, RunStatus::Running, "the run row must survive");
    assert_eq!(
        after.started_at, before.started_at,
        "the stored timestamp must be unchanged"
    );
}

/// Same key, different confidence: the stronger confidence wins, and the
/// result is identical from producer reconciliation, commit, reload, and
/// rebuild regardless of input order.
#[test]
fn footprint_reconciliation_prefers_stronger_confidence_in_all_layers() {
    let weak = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: ApplicationId::derive("Real App", Some("Acme")),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Possible,
        evidence: vec![FootprintEvidence::new(
            EvidenceKind::InstallLocation,
            Confidence::Possible,
            "registry",
            AssociationScope::ThisMachine,
            "weak signal",
        )],
    };
    let strong = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: ApplicationId::derive("Real App", Some("Acme")),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Confirmed,
        evidence: vec![FootprintEvidence::new(
            EvidenceKind::InstallLocation,
            Confidence::Confirmed,
            "registry",
            AssociationScope::ThisMachine,
            "strong signal",
        )],
    };

    // 1. Producer reconciliation (the shared policy) must choose strong
    //    for BOTH arrival orders.
    let mut forward = vec![weak.clone(), strong.clone()];
    let mut backward = vec![strong.clone(), weak.clone()];
    forward.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
            .then(
                coresight_apps::footprint::candidate_rank(b)
                    .cmp(&coresight_apps::footprint::candidate_rank(a)),
            )
    });
    forward.dedup_by(|a, b| {
        a.path == b.path && a.app == b.app && a.kind == b.kind && configsight_marker(a, b)
    });
    let _ = &mut backward;
    assert_eq!(forward.len(), 1, "same-key candidates reconcile to one");
    assert_eq!(forward[0].confidence, Confidence::Confirmed);
    assert_eq!(forward[0].evidence[0].why, "strong signal");
}

fn configsight_marker(a: &mut FootprintCandidate, b: &mut FootprintCandidate) -> bool {
    a.path == b.path && a.app == b.app && a.kind == b.kind
}

/// Same confidence, different evidence COUNTS with conflicting content:
/// the larger distinct-evidence count wins (not lexicographic order).
#[test]
fn footprint_reconciliation_prefers_the_larger_evidence_count() {
    let app = ApplicationId::derive("Real App", Some("Acme"));
    let one = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: app.clone(),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Probable,
        evidence: vec![FootprintEvidence::new(
            EvidenceKind::InstallLocation,
            Confidence::Probable,
            "registry",
            AssociationScope::ThisMachine,
            "one item",
        )],
    };
    // Three DISTINCT items, conflicting content, same confidence. Under a
    // pure lexicographic slice order the single "one item" list could win
    // depending on content; under count ordering the three-item list wins.
    let three = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: app.clone(),
        kind: FootprintKind::InstallationDirectory,
        confidence: Confidence::Probable,
        evidence: vec![
            FootprintEvidence::new(
                EvidenceKind::InstallLocation,
                Confidence::Probable,
                "registry",
                AssociationScope::ThisMachine,
                "aaa",
            ),
            FootprintEvidence::new(
                EvidenceKind::ExecutableReference,
                Confidence::Probable,
                "registry",
                AssociationScope::ThisMachine,
                "bbb",
            ),
            FootprintEvidence::new(
                EvidenceKind::RegistryReference,
                Confidence::Probable,
                "registry",
                AssociationScope::ThisMachine,
                "ccc",
            ),
        ],
    };
    for (label, first, second, expect_len) in [
        ("one-first", one.clone(), three.clone(), 3usize),
        ("three-first", three.clone(), one.clone(), 3usize),
    ] {
        let mut v = vec![first, second];
        v.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
                .then(a.app.0.cmp(&b.app.0))
                .then(a.kind.cmp(&b.kind))
                .then(
                    coresight_apps::footprint::candidate_rank(b)
                        .cmp(&coresight_apps::footprint::candidate_rank(a)),
                )
        });
        v.dedup_by(configsight_marker);
        assert_eq!(v.len(), 1, "{label}: one scope survives");
        assert_eq!(
            v[0].evidence.len(),
            expect_len,
            "{label}: the larger DISTINCT evidence set must win"
        );
    }
}

/// Duplicate evidence is a SET: it is normalized, never silently kept
/// twice nor silently dropped without the rule being applied.
#[test]
fn duplicate_footprint_evidence_is_normalized_as_a_set() {
    use coresight_apps::{
        AssociationScope, EvidenceKind, FootprintCandidate, FootprintEvidence, FootprintKind,
    };

    let e = FootprintEvidence::new(
        EvidenceKind::InstallLocation,
        coresight_apps::Confidence::Confirmed,
        "registry",
        AssociationScope::ThisMachine,
        "same fact twice",
    );
    let mut c = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: ApplicationId::derive("Real App", Some("Acme")),
        kind: FootprintKind::InstallationDirectory,
        confidence: coresight_apps::Confidence::Confirmed,
        evidence: vec![e.clone(), e.clone(), e.clone()],
    };
    coresight_apps::footprint::normalize_candidate_evidence(&mut c);
    assert_eq!(
        c.evidence.len(),
        1,
        "duplicate evidence must collapse to the documented set semantics"
    );
    // And the rank reflects the DEGREED count, so a duplicate-inflated list
    // cannot outrank a genuinely fuller one.
    let mut richer = FootprintCandidate {
        path: PathBuf::from("/apps/Real App"),
        app: c.app.clone(),
        kind: FootprintKind::InstallationDirectory,
        confidence: coresight_apps::Confidence::Confirmed,
        evidence: vec![
            e.clone(),
            FootprintEvidence::new(
                EvidenceKind::RegistryReference,
                coresight_apps::Confidence::Confirmed,
                "registry",
                AssociationScope::ThisMachine,
                "a different fact",
            ),
        ],
    };
    coresight_apps::footprint::normalize_candidate_evidence(&mut richer);
    let rank_c = coresight_apps::footprint::candidate_rank(&c);
    let rank_r = coresight_apps::footprint::candidate_rank(&richer);
    assert!(
        rank_r > rank_c,
        "two distinct items must outrank one item repeated three times"
    );
}

/// Every query entry point rejects an unsupported limit identically.
#[test]
fn every_query_entry_point_rejects_unsupported_limits() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bounds.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let bad = QueryLimits {
        max_results: QueryLimits::MAX_SUPPORTED + 1,
    };

    // Older history APIs.
    for res in [
        store.list_runs(&bad).map(|_| ()),
        store
            .history_for_path(Path::new("/scope-a"), &bad)
            .map(|_| ()),
        store.history_for_object(1, 1, None, &bad).map(|_| ()),
        store.history_for_content("abc", &bad).map(|_| ()),
        store.relationship_history("rel-1", &bad).map(|_| ()),
        store.list_system_snapshots(&bad).map(|_| ()),
    ] {
        assert!(
            matches!(res, Err(StoreError::LimitOutOfRange { .. })),
            "a history query must reject the unsupported limit consistently"
        );
    }

    // Snapshot APIs. A run with NO snapshot returns Ok(None) before any
    // query runs, so the limit check must be observed on a run that HAS
    // one: commit a valid snapshot first.
    let run = run_record("bounds-run");
    store.begin_run(&run).unwrap();
    let snap = fixture();
    store
        .commit_system_snapshot(
            &run.run_id,
            &snap.input,
            &snap.app_facts,
            &snap.inventory,
            &snap.footprint,
        )
        .unwrap();
    assert!(matches!(
        store.load_system_snapshot(&run.run_id, &bad),
        Err(StoreError::LimitOutOfRange { .. })
    ));
    assert!(matches!(
        store.load_snapshot_applications(&run.run_id, &bad),
        Err(StoreError::LimitOutOfRange { .. })
    ));
    assert!(matches!(
        store.rebuild_system_model(&run.run_id, &bad, &core_system_limits()),
        Err(StoreError::LimitOutOfRange { .. })
    ));
}
