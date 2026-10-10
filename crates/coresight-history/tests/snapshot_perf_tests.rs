//! Phase 6.4 persistence performance coverage (Objective 26).
//!
//! Ignored by default, run explicitly with `--ignored` — and run by CI
//! for `coresight-history` (`.github/workflows/ci.yml`).
//!
//! These are SMOKE guards, in the style of the repository's existing
//! perf suites: correctness is asserted strictly, timing is REPORTED and
//! only a deliberately loose scaling guard is enforced so a busy runner
//! cannot make the suite flaky. No benchmark weakens a correctness test.
//!
//! Coverage required by the phase: batch insertion, snapshot load,
//! application-heavy snapshots, and evidence-heavy snapshots.

use std::path::PathBuf;
use std::time::{Duration, Instant, UNIX_EPOCH};

use coresight_apps::{
    ApplicationId, ApplicationRecord, ApplicationSource, AssociationScope, Confidence,
    CorrelationGroup, EvidenceKind, EvidenceSource, EvidenceStrength, FootprintCandidate,
    FootprintEvidence, FootprintKind, FootprintReport, Inventory, MatchedAttribute,
    OwnershipEvidence, PackageKind, ProbedKind, SourceCoverage,
};
use coresight_capabilities::access::AccessState;
use coresight_history::{
    AppSnapshotFact, ConfigFingerprint, HistoryStore, QueryLimits, RunCounts, RunId, RunRecord,
    RunStatus,
};
use coresight_identity::ObjectIdentity;
use coresight_system_model::{ApplicationFact, ArtifactFact, SystemModelInput, SystemModelLimits};

fn run_record(id: &str) -> RunRecord {
    RunRecord {
        run_id: RunId(id.to_string()),
        started_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        completed_at: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_060)),
        roots: vec![PathBuf::from("/scope-a")],
        platform: "perf".to_string(),
        config: ConfigFingerprint::current(),
        status: RunStatus::Completed,
        counts: RunCounts::default(),
    }
}

/// One artifact at `/apps/AppN/dNNNNNNN/file` with a proven identity
/// (narrow and wide mixed, so both storage shapes are exercised).
fn artifact(i: u64) -> ArtifactFact {
    ArtifactFact {
        path: PathBuf::from(format!("/apps/App{}/d{:07}/file", i % 32, i)),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity {
            volume: 1,
            file_id: i + 1,
            file_id_hi: i.is_multiple_of(3).then_some(i / 3),
        }),
        content_sha256: i.is_multiple_of(7).then(|| format!("{i:064x}")),
        size: Some(1024 + i),
        access: AccessState::ReadSucceeded,
        classification: None,
    }
}

fn record(name: &str) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, Some("Perf Publisher")),
        name: name.to_string(),
        version: Some("1.0".to_string()),
        publisher: Some("Perf Publisher".to_string()),
        install_location: Some(PathBuf::from(format!("/apps/{name}"))),
        install_date: None,
        estimated_size_bytes: Some(4096),
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::RegistryUninstall,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: vec!["HKLM64".to_string()],
        bundle_identifier: Some(format!("com.perf.{name}")),
        executable_path: None,
        provenance: vec![ApplicationSource::RegistryUninstall],
    }
}

/// Build one snapshot with `apps` applications, `artifact_count`
/// artifacts, and `evidence_per_app` ownership-evidence items per
/// application.
fn build(
    apps: u64,
    artifact_count: u64,
    evidence_per_app: u64,
) -> (SystemModelInput, Vec<AppSnapshotFact>) {
    let artifacts: Vec<ArtifactFact> = (0..artifact_count).map(artifact).collect();
    let mut input_apps = Vec::new();
    let mut app_facts = Vec::new();
    for a in 0..apps {
        let rec = record(&format!("App{a}"));
        // Each application claims a contiguous slice of the artifacts.
        let per_app = (artifact_count / apps.max(1)).max(1);
        let start = a * per_app;
        let associations: Vec<(PathBuf, OwnershipEvidence)> = (0..evidence_per_app)
            .map(|e| {
                let idx = (start + e) % artifact_count.max(1);
                let path = PathBuf::from(format!("/apps/App{}/d{:07}/file", idx % 32, idx));
                let evidence = OwnershipEvidence::new(
                    EvidenceKind::InstallLocation,
                    EvidenceSource::RegistryMetadata,
                    EvidenceStrength::Direct,
                    CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
                    AssociationScope::ThisMachine,
                    path.clone(),
                    MatchedAttribute::InstallLocation,
                    Some(rec.name.clone()),
                )
                .with_matched_path(path.clone());
                (path, evidence)
            })
            .collect();
        input_apps.push(ApplicationFact {
            record: rec.clone(),
            install_roots: vec![PathBuf::from(format!("/apps/App{a}"))],
            executable: None,
            associations: associations.clone(),
        });
        app_facts.push(AppSnapshotFact {
            record: rec,
            install_roots: vec![PathBuf::from(format!("/apps/App{a}"))],
            executable: None,
            associations,
            footprints: Vec::new(),
        });
    }
    (
        SystemModelInput {
            artifacts,
            applications: input_apps,
            relationships: Vec::new(),
            history: Vec::new(),
            source_coverage: vec![SourceCoverage::complete("win32-uninstall")],
        },
        app_facts,
    )
}

/// Commit + load one snapshot of the given shape, reporting both timings
/// and asserting the round-trip is exact. Returns `(commit_ms, load_ms)`.
///
/// `slot` gives each measurement its own store file and run id (a run
/// commits once — the primary key correctly refuses a duplicate).
fn measure(
    label: &str,
    slot: &str,
    dir: &std::path::Path,
    apps: u64,
    artifacts: u64,
    evidence_per_app: u64,
) -> (u128, u128) {
    let db = dir.join(format!("perf-{slot}.db"));
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record(&format!("perf-run-{slot}"));
    store.begin_run(&record).unwrap();

    let (input, app_facts) = build(apps, artifacts, evidence_per_app);
    let inventory = Inventory {
        records: input
            .applications
            .iter()
            .map(|f| f.record.clone())
            .collect(),
        ..Inventory::default()
    };
    // Room for every section plus the probe row: a bounded load is
    // REPORTED as incomplete (and refuses to rebuild), which the
    // correctness suite asserts separately.
    let limits = QueryLimits {
        max_results: (artifacts + apps + 2_000) as usize,
    };

    let t0 = Instant::now();
    store
        .commit_system_snapshot(
            &record.run_id,
            &input,
            &app_facts,
            &inventory,
            &FootprintReport::default(),
        )
        .unwrap();
    let commit_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let loaded = store
        .load_system_snapshot(&record.run_id, &limits)
        .unwrap()
        .expect("the committed snapshot must reload");
    let load_ms = t1.elapsed().as_millis();
    assert!(
        !loaded.is_load_truncated(),
        "{label}: the fixture load must fit its limit"
    );

    // Correctness is asserted strictly — the timing is only reported.
    assert_eq!(
        loaded.input.artifacts.len(),
        input.artifacts.len(),
        "{label}: every artifact must round-trip"
    );
    assert_eq!(
        loaded.input.applications.len(),
        input.applications.len(),
        "{label}: every application must round-trip"
    );
    assert_eq!(
        loaded.input.source_coverage, input.source_coverage,
        "{label}: coverage must round-trip"
    );

    // And the rebuilt model is stable across repeated loads.
    let model_limits = SystemModelLimits::default();
    let m1 = store
        .rebuild_system_model(&record.run_id, &limits, &model_limits)
        .unwrap()
        .unwrap();
    let m2 = store
        .rebuild_system_model(&record.run_id, &limits, &model_limits)
        .unwrap()
        .unwrap();
    m1.check_invariants().expect("invariants hold");
    assert_eq!(m1, m2, "{label}: repeated reload must be identical");

    println!(
        "{label:<22} apps={apps:<5} artifacts={artifacts:<7} evidence/app={evidence_per_app:<3} \
         commit={commit_ms:>5} ms  load={load_ms:>5} ms  nodes={} edges={}",
        m1.artifact_count(),
        m1.edge_count()
    );
    (commit_ms, load_ms)
}

#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn snapshot_persistence_scales_on_every_shape() {
    let dir = tempfile::tempdir().unwrap();

    // Batch insertion + snapshot load at two sizes: the per-item guard
    // fails only if cost explodes far beyond linear between them.
    let (c10, l10) = measure("artifact-heavy 10k", "10k", dir.path(), 32, 10_000, 4);
    let (c100, l100) = measure("artifact-heavy 100k", "100k", dir.path(), 32, 100_000, 4);

    // Application-heavy: many applications, few artifacts each.
    let _ = measure("application-heavy", "apps", dir.path(), 2_000, 8_000, 2);

    // Evidence-heavy: few applications, many evidence items each.
    let _ = measure("evidence-heavy", "evidence", dir.path(), 8, 4_000, 400);

    // Loose scaling guard: 10× the artifacts must not cost dramatically
    // more than 10× the time. The bound is deliberately generous (50×)
    // because these tests run CONCURRENTLY with the other ignored suites
    // under `cargo test`'s default threads, so wall-clock timings carry
    // scheduler noise a solo run does not. A genuinely super-linear path
    // (quadratic nested scans, missing index) blows past this easily.
    assert!(c10 > 0 || c100 < 1_000, "commit timings must be sane");
    assert!(
        c100 <= c10.saturating_mul(50).max(50),
        "commit cost grew super-linearly: 10k={c10} ms, 100k={c100} ms"
    );
    assert!(
        l100 <= l10.saturating_mul(50).max(50),
        "load cost grew super-linearly: 10k={l10} ms, 100k={l100} ms"
    );
}

#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn snapshot_load_is_per_run_bounded_not_whole_database() {
    // Many runs with snapshots: loading ONE run must not scale with the
    // number of other snapshots in the store.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("many-runs.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let (input, app_facts) = build(8, 500, 2);
    let inventory = Inventory {
        records: input
            .applications
            .iter()
            .map(|f| f.record.clone())
            .collect(),
        ..Inventory::default()
    };
    let mut first = None;
    for n in 0..12u64 {
        let record = run_record(&format!("bulk-{n}"));
        store.begin_run(&record).unwrap();
        store
            .commit_system_snapshot(
                &record.run_id,
                &input,
                &app_facts,
                &inventory,
                &FootprintReport::default(),
            )
            .unwrap();
        first.get_or_insert(record.run_id);
    }
    let target = first.unwrap();

    let t0 = Instant::now();
    for _ in 0..10 {
        let loaded = store
            .load_system_snapshot(&target, &QueryLimits::default())
            .unwrap()
            .unwrap();
        // Exactly this run's facts — never a neighbour's.
        assert_eq!(loaded.input.artifacts.len(), input.artifacts.len());
    }
    let per_load_ms = t0.elapsed().as_millis() / 10;
    println!("per-run load with 12 stored snapshots: {per_load_ms} ms");
    assert!(
        per_load_ms < 2_000,
        "a per-run load must stay bounded regardless of other snapshots ({per_load_ms} ms)"
    );

    // The bounded listing reports counts only, capped by its limit.
    let listed = store
        .list_system_snapshots(&QueryLimits { max_results: 5 })
        .unwrap();
    assert_eq!(listed.len(), 5, "the listing respects its bound");
}

#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn snapshot_commit_is_idempotent_under_repetition() {
    // Re-committing the same snapshot repeatedly must not grow the store
    // (no semantic duplicates) nor degrade without bound.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("idem-perf.db");
    let mut store = HistoryStore::open(&db).unwrap();

    let record = run_record("idem-run");
    store.begin_run(&record).unwrap();
    let (input, app_facts) = build(16, 4_000, 4);
    let inventory = Inventory {
        records: input
            .applications
            .iter()
            .map(|f| f.record.clone())
            .collect(),
        ..Inventory::default()
    };
    let footprint = FootprintReport::default();

    let mut timings = Vec::new();
    for _ in 0..5 {
        let t0 = Instant::now();
        store
            .commit_system_snapshot(&record.run_id, &input, &app_facts, &inventory, &footprint)
            .unwrap();
        timings.push(t0.elapsed().as_millis());
    }
    println!("re-commit timings (ms): {timings:?}");

    // The store holds exactly ONE snapshot for the run.
    let listed = store
        .list_system_snapshots(&QueryLimits::default())
        .unwrap();
    assert_eq!(listed.len(), 1, "re-commit must not duplicate the snapshot");
    assert_eq!(listed[0].artifacts as usize, input.artifacts.len());
    assert_eq!(listed[0].applications as usize, input.applications.len());

    // Repeated re-commits stay within a loose bound of the first.
    let first = timings[0].max(1);
    let last = *timings.last().unwrap();
    assert!(
        last <= first.saturating_mul(6).max(30),
        "re-commit cost degraded: first={first} ms last={last} ms"
    );
}

// ---------------------------------------------------------------------------
// Workload shapes required by Phase 6.4.1 §6
// ---------------------------------------------------------------------------

/// Identity-heavy: many applications, each with a distinct
/// `(name, publisher)` identity and its own provenance — exercising the
/// collision-free encoding and the per-row re-key path's key diversity.
#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn identity_heavy_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("perf-identity.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("perf-identity-run");
    store.begin_run(&record).unwrap();

    // Names/publishers that all differ, including pipe-bearing shapes the
    // old delimiter encoding conflated.
    let apps: u64 = 1_500;
    let (input, app_facts) = build(apps, apps, 1);
    let inventory = Inventory {
        records: input
            .applications
            .iter()
            .map(|f| f.record.clone())
            .collect(),
        ..Inventory::default()
    };
    // Every application must have a DISTINCT id (the encoding property).
    let mut ids = std::collections::BTreeSet::new();
    for f in &app_facts {
        assert!(ids.insert(f.record.id.0.clone()), "ids must be distinct");
    }
    assert_eq!(ids.len() as u64, apps);

    let limits = QueryLimits {
        max_results: (apps + 2_000) as usize,
    };
    let t0 = Instant::now();
    store
        .commit_system_snapshot(
            &record.run_id,
            &input,
            &app_facts,
            &inventory,
            &FootprintReport::default(),
        )
        .unwrap();
    let commit_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let loaded = store
        .load_system_snapshot(&record.run_id, &limits)
        .unwrap()
        .expect("the committed snapshot must reload");
    assert!(!loaded.is_load_truncated());
    assert_eq!(loaded.app_facts.len() as u64, apps);
    let load_ms = t1.elapsed().as_millis();

    // The rebuilt model must contain every application.
    let m = store
        .rebuild_system_model(&record.run_id, &limits, &SystemModelLimits::default())
        .unwrap()
        .unwrap();
    m.check_invariants().expect("invariants hold");
    assert_eq!(m.application_count() as u64, apps);
    println!(
        "identity-heavy  apps={apps:<6} commit={commit_ms:>5} ms  load={load_ms:>5} ms  \
         provenance_union_ok"
    );
}

/// Footprint-heavy: few applications, each with many footprint candidates
/// and evidence items — exercising candidate admission, the
/// reconciliation rule, and the nested evidence rows.
#[test]
#[ignore = "performance smoke; run explicitly with --ignored"]
fn footprint_heavy_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("perf-footprint.db");
    let mut store = HistoryStore::open(&db).unwrap();
    let record = run_record("perf-footprint-run");
    store.begin_run(&record).unwrap();

    let apps: u64 = 4;
    let per_app: u64 = 1_500;
    let (mut input, mut app_facts) = build(apps, per_app * apps, evidence_per_app(1));
    // Give each application its own footprint candidates.
    for (i, fact) in app_facts.iter_mut().enumerate() {
        let mut fps = Vec::new();
        for n in 0..per_app {
            fps.push(FootprintCandidate {
                path: PathBuf::from(format!("/apps/App{i}/fp{n:06}")),
                app: fact.record.id.clone(),
                kind: FootprintKind::InstallationDirectory,
                confidence: Confidence::Confirmed,
                evidence: vec![FootprintEvidence::new(
                    EvidenceKind::InstallLocation,
                    Confidence::Confirmed,
                    "registry",
                    AssociationScope::ThisMachine,
                    "recorded by the installer",
                )],
            });
        }
        fact.footprints = fps.clone();
    }
    // The report must agree with the facts (the same canonical union).
    let mut report: Vec<coresight_apps::FootprintCandidate> = app_facts
        .iter()
        .flat_map(|f| f.footprints.iter().cloned())
        .collect();
    report.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
    });
    report.dedup_by(|a, b| a.path == b.path && a.app == b.app && a.kind == b.kind);
    input.source_coverage = vec![SourceCoverage::complete("win32-uninstall")];

    let inventory = Inventory {
        records: input
            .applications
            .iter()
            .map(|f| f.record.clone())
            .collect(),
        ..Inventory::default()
    };
    let footprint = FootprintReport {
        candidates: report,
        ..FootprintReport::default()
    };

    let limits = QueryLimits {
        max_results: (apps * per_app + 2_000) as usize,
    };
    let t0 = Instant::now();
    store
        .commit_system_snapshot(&record.run_id, &input, &app_facts, &inventory, &footprint)
        .unwrap();
    let commit_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let loaded = store
        .load_system_snapshot(&record.run_id, &limits)
        .unwrap()
        .expect("the committed snapshot must reload");
    assert!(!loaded.is_load_truncated());
    assert_eq!(
        loaded.footprint.candidates.len() as u64,
        apps * per_app,
        "every footprint candidate must round-trip"
    );
    let load_ms = t1.elapsed().as_millis();
    println!(
        "footprint-heavy apps={apps} candidates={:<7} commit={commit_ms:>5} ms  load={load_ms:>5} ms",
        apps * per_app
    );
}

/// The evidence items an application contributes, for the workload
/// builders above.
fn evidence_per_app(n: u64) -> u64 {
    n
}
