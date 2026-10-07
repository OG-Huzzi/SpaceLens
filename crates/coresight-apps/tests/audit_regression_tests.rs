//! Phase 6 audit regression tests (second-order audit): the invariants
//! that keep application intelligence honest.
//!
//! Covered:
//! - A1 source coverage: unsupported/failed/unavailable are never empty
//!   success; per-view absence is Partial, not silence.
//! - A2 application identity: deterministic, version-independent,
//!   publisher/path-sensitive enough to keep unrelated apps distinct.
//! - A3 boundedness: every discovery path has an explicit limit and
//!   counts truncation exactly; children are capped deterministically.
//! - A4 determinism: input order never changes ids, ordering, or
//!   classification.
//! - A5 shared resources: name-coincidence evidence never becomes
//!   strong ownership; shared runtimes cannot claim exclusive data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coresight_apps::{
    discover_footprints, merge_inventory, offer_name, offer_path, ApplicationId,
    ApplicationProvider, ApplicationRecord, ApplicationSource, BoundedListing, Confidence,
    DiscoveryLimits, FootprintKind, FootprintReport, KnownRoots, OwnershipStrength, PackageKind,
    PackagedAppProvider, PathProber, ProviderOutcome, RegistryValue, RegistryView, SourceCoverage,
    SourceStatus, SubkeyEnumeration, UninstallView, Win32UninstallEnumerator, WindowsAppxProvider,
};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct FakeRegistry {
    keys: BTreeMap<String, Vec<String>>,
    values: BTreeMap<(String, String), RegistryValue>,
    present: BTreeMap<String, bool>,
}

impl FakeRegistry {
    fn with_subkeys(mut self, key: &str, subs: &[&str]) -> Self {
        self.keys.insert(
            key.to_string(),
            subs.iter().map(|s| s.to_string()).collect(),
        );
        self.present.insert(key.to_string(), true);
        self
    }
    fn with_value(mut self, key: &str, name: &str, v: RegistryValue) -> Self {
        self.values.insert((key.to_string(), name.to_string()), v);
        self
    }
    fn absent(mut self, key: &str) -> Self {
        self.present.insert(key.to_string(), false);
        self
    }
}

impl RegistryView for FakeRegistry {
    fn subkeys_bounded(&self, key: &str, max: usize) -> SubkeyEnumeration {
        let mut set = std::collections::BTreeSet::new();
        let mut truncated = 0u64;
        for name in self.keys.get(key).cloned().unwrap_or_default() {
            offer_name(&mut set, max, name, &mut truncated);
        }
        SubkeyEnumeration {
            keys: set.into_iter().collect(),
            skipped_oversized: 0,
            truncated,
            incomplete: false,
        }
    }
    fn get_value(&self, key: &str, name: &str) -> Option<RegistryValue> {
        self.values
            .get(&(key.to_string(), name.to_string()))
            .cloned()
    }
    fn key_present(&self, key: &str) -> bool {
        *self.present.get(key).unwrap_or(&false)
    }
}

// Registry keys keep BACKSLASH separators: `split_hive_path` and the
// production view constants use `HKLM\...` key syntax (registry
// semantics, not filesystem paths).
fn root(view: UninstallView) -> &'static str {
    match view {
        UninstallView::Hklm64 => r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        UninstallView::Hklm32 => {
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"
        }
        UninstallView::Hkcu => r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    }
}

fn key(view: UninstallView, sub: &str) -> String {
    format!("{}\\{}", root(view), sub)
}

#[derive(Default)]
struct FakeFs {
    dirs: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl FakeFs {
    fn with_dirs(mut self, parent: &str, children: &[&str]) -> Self {
        self.dirs.insert(
            PathBuf::from(parent),
            children.iter().map(PathBuf::from).collect(),
        );
        self
    }
}

impl PathProber for FakeFs {
    fn children_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
        let mut set = std::collections::BTreeSet::new();
        let mut overflow = 0u64;
        for name in self.dirs.get(dir).cloned().unwrap_or_default() {
            offer_path(&mut set, max, name, &mut overflow);
        }
        BoundedListing {
            names: set.into_iter().collect(),
            overflow,
        }
    }
    fn entries_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
        self.children_bounded(dir, max)
    }
}

fn app(name: &str, publisher: Option<&str>) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, publisher),
        name: name.to_string(),
        version: None,
        publisher: publisher.map(str::to_string),
        install_location: None,
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::RegistryUninstall,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// A1 — source coverage semantics
// ---------------------------------------------------------------------------

#[test]
fn a1_appx_provider_reports_unsupported_coverage_not_empty_success() {
    let outcome = WindowsAppxProvider.enumerate_outcome();
    assert!(outcome.records.is_empty());
    assert_eq!(outcome.coverage.status, SourceStatus::Unsupported);
    assert!(outcome.coverage.note.is_some());
    // And the raw Result stays an explicit error.
    assert!(WindowsAppxProvider.enumerate().is_err());
    assert_eq!(WindowsAppxProvider.package_source(), "appxmanifest");
}

#[test]
fn a1_absent_views_are_unavailable_or_partial_never_complete() {
    // Only HKCU exists; HKLM views absent on this machine.
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hkcu), &["UserApp"])
        .with_value(
            &key(UninstallView::Hkcu, "UserApp"),
            "DisplayName",
            RegistryValue::Sz("User App".into()),
        )
        .absent(root(UninstallView::Hklm64))
        .absent(root(UninstallView::Hklm32));
    let outcome = Win32UninstallEnumerator::new(fake).enumerate_outcome();
    assert_eq!(outcome.records.len(), 1);
    assert_eq!(
        outcome.coverage.status,
        SourceStatus::Partial,
        "two of three views absent is Partial, never Complete: {outcome:?}"
    );
    let note = outcome
        .coverage
        .note
        .expect("partial coverage explains itself");
    assert!(
        note.contains("HKLM-64"),
        "note names the absent views: {note}"
    );
    assert!(note.contains("HKLM-32"));
}

#[test]
fn a1_all_views_absent_is_unavailable_not_empty_inventory() {
    let fake = FakeRegistry::default()
        .absent(root(UninstallView::Hklm64))
        .absent(root(UninstallView::Hklm32))
        .absent(root(UninstallView::Hkcu));
    let outcome = Win32UninstallEnumerator::new(fake).enumerate_outcome();
    assert!(outcome.records.is_empty());
    assert_eq!(
        outcome.coverage.status,
        SourceStatus::Unavailable,
        "no registry views at all is Unavailable, not a successful empty scan"
    );

    // The inventory must carry that status, not a bare empty list.
    let inv = merge_inventory(vec![outcome], &DiscoveryLimits::default());
    assert!(inv.records.is_empty());
    assert_eq!(inv.sources.len(), 1);
    assert_eq!(inv.sources[0].status, SourceStatus::Unavailable);
}

#[test]
fn a1_all_views_present_and_read_is_complete() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["A"])
        .with_value(
            &key(UninstallView::Hklm64, "A"),
            "DisplayName",
            RegistryValue::Sz("A".into()),
        )
        .with_subkeys(root(UninstallView::Hklm32), &[])
        .with_subkeys(root(UninstallView::Hkcu), &[]);
    let outcome = Win32UninstallEnumerator::new(fake).enumerate_outcome();
    assert_eq!(outcome.coverage.status, SourceStatus::Complete);
    assert!(outcome.coverage.note.is_none());
}

// ---------------------------------------------------------------------------
// A2 — application identity
// ---------------------------------------------------------------------------

#[test]
fn a2_identity_is_stable_for_identical_metadata() {
    let a = ApplicationId::derive("Example App", Some("Vendor"));
    let b = ApplicationId::derive("Example App", Some("Vendor"));
    assert_eq!(a, b);
    // Case/whitespace normalization is the documented normalization.
    let c = ApplicationId::derive("  example app ", Some("VENDOR"));
    assert_eq!(a, c);
}

#[test]
fn a2_identity_does_not_depend_on_version() {
    // The id is derived from (name, publisher, source) only — a version
    // upgrade must NOT change it.
    let r1 = app("Example App", Some("Vendor"));
    let mut r2 = app("Example App", Some("Vendor"));
    r2.version = Some("99.0".into());
    assert_eq!(r1.id, r2.id);
}

#[test]
fn a2_same_name_different_publisher_is_a_different_application() {
    let a = ApplicationId::derive("Setup", Some("Vendor A"));
    let b = ApplicationId::derive("Setup", Some("Vendor B"));
    assert_ne!(a, b);
    // Missing publisher must not collide with a named publisher.
    let c = ApplicationId::derive("Setup", None);
    assert_ne!(a, c);
    assert_ne!(b, c);
}

#[test]
fn a2_identity_is_the_logical_application_not_the_source() {
    // The Phase 6.1 identity rule: a logical application is identified by
    // its normalized (name, publisher); the discovery source is PROVENANCE.
    // The same application observed through two sources shares one
    // ApplicationId (and merge_inventory collapses it into one record with
    // unioned provenance), so the id can never disagree with the merge key.
    let win32 = ApplicationId::derive("Example", None);
    let msix = ApplicationId::derive("Example", None);
    assert_eq!(win32, msix, "source is provenance, not identity");

    // Cross-source merge: one logical application, provenance unioned,
    // coverage from both sources.
    let win32_record = ApplicationRecord {
        id: win32.clone(),
        name: "Example".to_string(),
        version: Some("1.0".to_string()),
        publisher: None,
        install_location: None,
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: Some("MsiExec /x".to_string()),
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::RegistryUninstall,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: vec!["HKLM-64".to_string()],
    };
    let msix_record = ApplicationRecord {
        id: msix.clone(),
        name: "Example".to_string(),
        version: None,
        publisher: None,
        install_location: None,
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::PackagedApp,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: vec![],
    };
    let outcomes = vec![
        ProviderOutcome {
            records: vec![win32_record],
            coverage: SourceCoverage::complete("win32-uninstall"),
        },
        ProviderOutcome {
            records: vec![msix_record],
            coverage: SourceCoverage::complete("msix-appx"),
        },
    ];
    let inv = merge_inventory(outcomes, &DiscoveryLimits::default());
    assert_eq!(inv.records.len(), 1, "one logical application");
    assert_eq!(inv.records[0].id, win32);
    assert_eq!(
        inv.records[0].source,
        ApplicationSource::RegistryUninstall,
        "the more complete record wins (canonical precedence)"
    );
    assert_eq!(
        inv.records[0].observed_in_views,
        vec!["HKLM-64".to_string()],
        "win32 provenance kept"
    );
    assert_eq!(inv.sources.len(), 2, "both sources report coverage");
}

#[test]
fn a2_duplicate_registry_records_do_not_multiply_identity() {
    // Same application recorded in all three views: one inventory entry,
    // one identity, provenance unioned.
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["Dup"])
        .with_subkeys(root(UninstallView::Hklm32), &["Dup"])
        .with_subkeys(root(UninstallView::Hkcu), &["Dup"]);
    let mut fake = fake;
    for view in UninstallView::ALL {
        fake = fake
            .with_value(
                &key(view, "Dup"),
                "DisplayName",
                RegistryValue::Sz("Dup App".into()),
            )
            .with_value(
                &key(view, "Dup"),
                "Publisher",
                RegistryValue::Sz("Vendor".into()),
            );
    }
    let outcome = Win32UninstallEnumerator::new(fake).enumerate_outcome();
    assert_eq!(outcome.records.len(), 3, "three raw records");
    let inv = merge_inventory(vec![outcome], &DiscoveryLimits::default());
    assert_eq!(inv.records.len(), 1, "one merged application");
    assert_eq!(inv.records[0].observed_in_views.len(), 3);
    // All three raw records derived the same id.
    // (The merge collapsed exactly because the identity is stable.)
}

// ---------------------------------------------------------------------------
// A3 — boundedness with exact truncation accounting
// ---------------------------------------------------------------------------

#[test]
fn a3_inventory_truncation_is_exact_and_deterministic() {
    let mut records = Vec::new();
    for i in 0..10 {
        records.push(app(&format!("App {i:02}"), Some("Vendor")));
    }
    let limits = DiscoveryLimits {
        max_records: 4,
        ..Default::default()
    };
    let inv = merge_inventory(
        vec![ProviderOutcome {
            records,
            coverage: SourceCoverage::complete("win32-uninstall"),
        }],
        &limits,
    );
    assert_eq!(inv.records.len(), 4);
    assert_eq!(inv.records_truncated, 6, "overflow counted exactly");
    // The published subset is the canonical prefix — deterministic.
    let names: Vec<&str> = inv.records.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["App 00", "App 01", "App 02", "App 03"]);
}

#[test]
fn a3_overlong_names_are_rejected_not_truncated_and_counted() {
    let mut long = app("A", Some("Vendor"));
    long.name = "x".repeat(600);
    long.id = ApplicationId::derive(&long.name, Some("Vendor"));
    let records = vec![app("Fine", Some("Vendor")), long];
    let inv = merge_inventory(
        vec![ProviderOutcome {
            records,
            coverage: SourceCoverage::complete("win32-uninstall"),
        }],
        &DiscoveryLimits::default(),
    );
    assert_eq!(inv.records.len(), 1);
    assert_eq!(inv.records[0].name, "Fine");
    assert_eq!(
        inv.records_rejected, 1,
        "rejection must be explicit, never silent"
    );
    assert_eq!(inv.records_truncated, 0, "rejection is not truncation");
}

#[test]
fn a3_footprint_children_bound_is_exact_and_deterministic() {
    // 10 children under ProgramData, bound of 3.
    let children: Vec<String> = (0..10)
        .map(|i| format!("C:/ProgramData/App{i:02}"))
        .collect();
    let child_refs: Vec<&str> = children.iter().map(String::as_str).collect();
    let fs = FakeFs::default().with_dirs("C:/ProgramData", &child_refs);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..Default::default()
    };
    let limits = DiscoveryLimits {
        max_children_per_root: 3,
        ..Default::default()
    };
    let report = discover_footprints(&[app("App05", None)], &roots, &fs, &limits);
    assert_eq!(
        report.children_truncated, 7,
        "every unexamined child is counted: {report:?}"
    );
    // The examined subset is the canonically-first 3 children, so the
    // match for App05 is deliberately NOT examined and NOT claimed.
    assert!(report.candidates.is_empty());
}

#[test]
fn a3_footprint_candidate_bound_is_exact() {
    let apps = [app("Alpha", None)];
    let fs = FakeFs::default().with_dirs(
        "C:/ProgramData",
        &[
            "C:/ProgramData/Alpha",
            "C:/ProgramData/Alpha2",
            "C:/ProgramData/AlphaCache",
        ],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..Default::default()
    };
    let limits = DiscoveryLimits {
        max_records: 2,
        ..Default::default()
    };
    let report = discover_footprints(&apps, &roots, &fs, &limits);
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.candidates_truncated, 1);
}

#[test]
fn a3_oversized_skipped_subkeys_are_counted_into_partial_coverage() {
    // A view that reports skipped oversized names must surface Partial
    // coverage with the exact count — not a silent clean end.
    #[derive(Default)]
    struct SkipReportingRegistry {
        inner: FakeRegistry,
    }
    impl RegistryView for SkipReportingRegistry {
        fn subkeys_bounded(&self, key: &str, max: usize) -> coresight_apps::SubkeyEnumeration {
            let mut inner = self.inner.subkeys_bounded(key, max);
            // The simulated platform skipped two oversized key names.
            inner.skipped_oversized = 2;
            inner
        }
        fn get_value(&self, key: &str, name: &str) -> Option<RegistryValue> {
            self.inner.get_value(key, name)
        }
        fn key_present(&self, _key: &str) -> bool {
            true
        }
    }
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["A"])
        .with_subkeys(root(UninstallView::Hklm32), &[])
        .with_subkeys(root(UninstallView::Hkcu), &[])
        .with_value(
            &key(UninstallView::Hklm64, "A"),
            "DisplayName",
            RegistryValue::Sz("App A".into()),
        );
    let outcome =
        Win32UninstallEnumerator::new(SkipReportingRegistry { inner: fake }).enumerate_outcome();
    assert_eq!(outcome.records.len(), 1);
    assert_eq!(outcome.coverage.status, SourceStatus::Partial);
    let note = outcome.coverage.note.unwrap();
    // 2 skipped subkeys per view x 3 views = exactly 6, verbatim.
    assert!(
        note.contains("6 subkeys were skipped"),
        "exact skip count reported: {note}"
    );
}

#[test]
fn a3_hostile_large_registry_view_is_bounded_and_counted() {
    // A view with 100 subkeys and a bound of 10.
    let subs: Vec<String> = (0..100).map(|i| format!("K{i:03}")).collect();
    let sub_refs: Vec<&str> = subs.iter().map(String::as_str).collect();
    let mut fake = FakeRegistry::default().with_subkeys(root(UninstallView::Hklm64), &sub_refs);
    for s in &subs {
        fake = fake.with_value(
            &key(UninstallView::Hklm64, s),
            "DisplayName",
            RegistryValue::Sz(format!("App {s}")),
        );
    }
    let outcome = Win32UninstallEnumerator::new(fake)
        .with_max_subkeys_per_view(10)
        .enumerate_outcome();
    assert_eq!(outcome.records.len(), 10);
    assert_eq!(
        outcome.coverage.status,
        SourceStatus::Partial,
        "truncation must surface as Partial coverage"
    );
    let note = outcome.coverage.note.unwrap();
    assert!(
        note.contains("90"),
        "the exact overflow count is reported: {note}"
    );
}

// ---------------------------------------------------------------------------
// A4 — determinism under shuffled input
// ---------------------------------------------------------------------------

#[test]
fn a4_inventory_is_identical_under_input_permutations() {
    let mut records_a = vec![
        app("Zulu", Some("Vendor")),
        app("Alpha", Some("Vendor")),
        app("Mike", Some("Other")),
        app("Alpha", Some("Other")),
    ];
    let mut records_b = records_a.clone();
    records_b.reverse();
    records_a.swap(0, 2);

    let inv_a = merge_inventory(
        vec![ProviderOutcome {
            records: records_a,
            coverage: SourceCoverage::complete("win32-uninstall"),
        }],
        &DiscoveryLimits::default(),
    );
    let inv_b = merge_inventory(
        vec![ProviderOutcome {
            records: records_b,
            coverage: SourceCoverage::complete("win32-uninstall"),
        }],
        &DiscoveryLimits::default(),
    );
    assert_eq!(inv_a, inv_b, "input order must not affect the inventory");
}

#[test]
fn a4_footprint_candidates_are_identical_under_app_permutations() {
    let fs = FakeFs::default().with_dirs(
        "C:/ProgramData",
        &[
            "C:/ProgramData/Alpha",
            "C:/ProgramData/Beta",
            "C:/ProgramData/Gamma",
        ],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..Default::default()
    };
    let order_1 = vec![app("Alpha", None), app("Beta", None), app("Gamma", None)];
    let mut order_2 = order_1.clone();
    order_2.reverse();
    let r1 = discover_footprints(&order_1, &roots, &fs, &DiscoveryLimits::default());
    let r2 = discover_footprints(&order_2, &roots, &fs, &DiscoveryLimits::default());
    assert_eq!(r1, r2, "app order must not affect footprint output");
}

// ---------------------------------------------------------------------------
// A5 — shared resources and weak evidence
// ---------------------------------------------------------------------------

#[test]
fn a5_weak_name_evidence_never_becomes_definite_ownership() {
    // A directory that merely shares the app's name must map to at most
    // Possible ownership, never Definite/Probable.
    let apps = [app("Steam", Some("Valve"))];
    let fs = FakeFs::default().with_dirs("C:/ProgramData", &["C:/ProgramData/Steam"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..Default::default()
    };
    let report = discover_footprints(&apps, &roots, &fs, &DiscoveryLimits::default());
    assert!(
        !report.candidates.is_empty(),
        "name match yields a candidate"
    );
    for cand in &report.candidates {
        let strength = OwnershipStrength::from_confidence(cand.confidence);
        assert!(
            strength <= OwnershipStrength::Possible,
            "name coincidence cannot claim stronger than Possible: {cand:?}"
        );
        // And never Confirmed without install-location evidence.
        assert_ne!(cand.confidence, Confidence::Confirmed);
    }
}

#[test]
fn a5_shared_runtime_directory_is_not_exclusive_app_data() {
    // Two applications and one shared "Shared" directory that matches
    // neither name: no candidate may be invented for either app.
    let apps = [
        app("App One", Some("Vendor A")),
        app("App Two", Some("Vendor B")),
    ];
    let fs = FakeFs::default().with_dirs("C:/ProgramData", &["C:/ProgramData/Shared"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..Default::default()
    };
    let report = discover_footprints(&apps, &roots, &fs, &DiscoveryLimits::default());
    assert!(
        report.candidates.is_empty(),
        "an unrelated shared directory must not be claimed by any app: {report:?}"
    );
}

#[test]
fn a5_shared_runtime_records_carry_no_confirmed_footprint() {
    // A shared runtime (kind SharedRuntime) with no install location:
    // nothing may be Confirmed for it from name matching alone.
    let mut vc = app("Microsoft Visual C++ Redistributable", Some("Microsoft"));
    vc.kind = PackageKind::SharedRuntime;
    let fs = FakeFs::default().with_dirs(
        "C:/ProgramData",
        &[
            "C:/ProgramData/Microsoft",
            "C:/ProgramData/Microsoft Visual C++ Redistributable",
        ],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..Default::default()
    };
    let report = discover_footprints(&[vc], &roots, &fs, &DiscoveryLimits::default());
    for cand in &report.candidates {
        assert_ne!(
            cand.confidence,
            Confidence::Confirmed,
            "shared runtime: only installer-recorded locations may be Confirmed"
        );
        assert_ne!(cand.kind, FootprintKind::InstallationDirectory);
    }
}

#[test]
fn a5_install_location_evidence_is_the_only_confirmed_source() {
    let mut a = app("VLC media player", Some("VideoLAN"));
    a.install_location = Some(PathBuf::from("C:/Program Files/VideoLAN/VLC"));
    let fs = FakeFs::default();
    let report = discover_footprints(
        &[a],
        &KnownRoots::default(),
        &fs,
        &DiscoveryLimits::default(),
    );
    let confirmed: Vec<_> = report
        .candidates
        .iter()
        .filter(|c| c.confidence == Confidence::Confirmed)
        .collect();
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].kind, FootprintKind::InstallationDirectory);
}

// ---------------------------------------------------------------------------
// Phase 6.1 independent-verification additions: determinism under
// permutation, and TRUE boundedness (bounds exist where memory can grow).
// ---------------------------------------------------------------------------

#[test]
fn det_inventory_merge_is_byte_identical_under_every_permutation() {
    // Same records, every relevant arrival order: the published inventory
    // must be byte-identical (serde-rendered), including the winner of
    // equal-completeness ties (canonical precedence — never arrival order).
    fn outcome(
        tag: &str,
        version: Option<&str>,
        uninstall: Option<&str>,
        views: &[&str],
    ) -> ProviderOutcome {
        let record = ApplicationRecord {
            id: ApplicationId::derive("Tied App", Some("Vendor")),
            name: "Tied App".to_string(),
            version: version.map(str::to_string),
            publisher: Some("Vendor".to_string()),
            install_location: None,
            install_date: None,
            estimated_size_bytes: None,
            uninstall_string: uninstall.map(str::to_string),
            quiet_uninstall_string: None,
            modify_path: None,
            install_source: None,
            source: ApplicationSource::RegistryUninstall,
            kind: PackageKind::Installed,
            system_component: false,
            observed_in_views: views.iter().map(|v| v.to_string()).collect(),
        };
        let _ = tag;
        ProviderOutcome {
            records: vec![record],
            coverage: SourceCoverage::complete(tag),
        }
    }

    // Two records with EQUAL completeness but different content: the
    // winner must be the same record under any arrival order.
    let a = outcome("view-a", Some("1.0"), Some("UninstallA"), &["HKLM-64"]);
    let b = outcome("view-b", Some("2.0"), Some("UninstallB"), &["HKLM-32"]);
    let orders: Vec<Vec<ProviderOutcome>> = vec![vec![a.clone(), b.clone()], vec![b, a]];

    let rendered: Vec<String> = orders
        .into_iter()
        .map(|o| {
            let inv = merge_inventory(o, &DiscoveryLimits::default());
            serde_json::to_string(&inv).unwrap()
        })
        .collect();
    assert_eq!(rendered[0], rendered[1], "arrival order must not matter");
}

#[test]
fn det_footprint_discovery_is_identical_under_permutation() {
    // The same app/root facts probed in different app orders (and with the
    // prober yielding children in different orders) publish identical
    // reports.
    fn app(id_tag: &str) -> ApplicationRecord {
        ApplicationRecord {
            id: ApplicationId::derive(id_tag, Some("Vendor")),
            name: id_tag.to_string(),
            version: None,
            publisher: Some("Vendor".to_string()),
            install_location: None,
            install_date: None,
            estimated_size_bytes: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            install_source: None,
            source: ApplicationSource::RegistryUninstall,
            kind: PackageKind::Installed,
            system_component: false,
            observed_in_views: vec![],
        }
    }
    let app_a = app("Alpha");
    let app_b = app("Beta");
    let roots = KnownRoots {
        local_app_data: Some(PathBuf::from("C:/Users/u/AppData/Local")),
        ..KnownRoots::default()
    };
    // The fake prober enumerates in insertion order; the two instances
    // enumerate the same set in DIFFERENT orders.
    let fs_a =
        FakeFs::default().with_dirs("C:/Users/u/AppData/Local", &["Beta", "AlphaTool", "zebra"]);
    let fs_b =
        FakeFs::default().with_dirs("C:/Users/u/AppData/Local", &["zebra", "AlphaTool", "Beta"]);

    let mut report_a = discover_footprints(
        &[app_a.clone(), app_b.clone()],
        &roots,
        &fs_a,
        &DiscoveryLimits::default(),
    );
    let mut report_b =
        discover_footprints(&[app_b, app_a], &roots, &fs_b, &DiscoveryLimits::default());
    let by_path = |r: &mut FootprintReport| {
        r.candidates.sort_by(|x, y| {
            x.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(y.path.as_os_str().as_encoded_bytes())
        });
        r.clone()
    };
    assert_eq!(by_path(&mut report_a), by_path(&mut report_b));
}

#[test]
fn bnd_registry_topk_keeps_canonical_smallest_and_counts_exactly() {
    // 10_000 subkeys against a 100-name bound: exactly 100 canonically-
    // smallest names are kept and the overflow is exact — and the real
    // contract is that the enumeration NEVER materialized all 10_000
    // (the bound is enforced where memory would grow).
    let names: Vec<String> = (0..10_000).map(|i| format!("key-{i:05}")).collect();
    let view = FakeRegistry::default()
        .with_subkeys(
            root(UninstallView::Hklm64),
            &names.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        )
        .with_value(
            &key(UninstallView::Hklm64, "key-00000"),
            "DisplayName",
            RegistryValue::Sz("Kept App".into()),
        );
    let enumerator = Win32UninstallEnumerator::new(view).with_max_subkeys_per_view(100);
    let outcome = enumerator.enumerate_outcome();
    assert_eq!(
        outcome.records.len(),
        1,
        "the canonically-first key is examined"
    );
    assert_eq!(outcome.records[0].name, "Kept App");
    let coverage = outcome.coverage;
    assert_eq!(coverage.status, SourceStatus::Partial);
    let note = coverage.note.unwrap_or_default();
    assert!(note.contains("9900"), "exact overflow counted: {note}");
}

#[test]
fn bnd_registry_topk_shuffles_keep_the_same_subset() {
    // The KEPT subset must be the canonically-smallest names regardless of
    // enumeration order (the audit concern: sort-after-materialize let
    // enumeration order reach which keys survive).
    let all: Vec<String> = (0..500).map(|i| format!("k{i:04}")).collect();
    let mut reversed = all.clone();
    reversed.reverse();

    let v1 = FakeRegistry::default().with_subkeys(
        root(UninstallView::Hklm64),
        &all.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let v2 = FakeRegistry::default().with_subkeys(
        root(UninstallView::Hklm64),
        &reversed.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );

    let kept1 = v1.subkeys_bounded(root(UninstallView::Hklm64), 50);
    let kept2 = v2.subkeys_bounded(root(UninstallView::Hklm64), 50);
    assert_eq!(kept1.keys, kept2.keys);
    assert_eq!(kept1.keys.len(), 50);
    assert_eq!(kept1.truncated, 450);
    assert_eq!(kept1.keys[0], "k0000");
    assert_eq!(kept1.keys[49], "k0049");
}

#[test]
fn bnd_prober_topk_keeps_canonical_smallest_and_counts_exactly() {
    // Same contract for the footprint prober: bounded memory, exact
    // overflow, canonically-smallest kept set under any enumeration order.
    let mut names: Vec<String> = (0..5_000).map(|i| format!("dir-{i:05}")).collect();
    names.reverse();
    let fs = FakeFs::default().with_dirs(
        "C:/root",
        &names.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let listing = fs.children_bounded(Path::new("C:/root"), 25);
    assert_eq!(listing.names.len(), 25);
    assert_eq!(listing.overflow, 4_975);
    assert_eq!(listing.names[0], PathBuf::from("dir-00000"));
    assert_eq!(listing.names[24], PathBuf::from("dir-00024"));

    let forward: Vec<String> = (0..5_000).map(|i| format!("dir-{i:05}")).collect();
    let fs2 = FakeFs::default().with_dirs(
        "C:/root",
        &forward.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let listing2 = fs2.children_bounded(Path::new("C:/root"), 25);
    assert_eq!(listing, listing2, "enumeration order must not matter");
}

#[test]
fn bnd_footprint_admission_bounds_working_set_deterministically() {
    // Far more candidates than max_records: the published set is the
    // canonically-first max_records distinct keys, overflow is exact, and
    // admission (not a final truncate) is what bounds the working set.
    fn app(name: &str) -> ApplicationRecord {
        ApplicationRecord {
            id: ApplicationId::derive(name, Some("Vendor")),
            name: name.to_string(),
            version: None,
            publisher: Some("Vendor".to_string()),
            install_location: None,
            install_date: None,
            estimated_size_bytes: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            install_source: None,
            source: ApplicationSource::RegistryUninstall,
            kind: PackageKind::Installed,
            system_component: false,
            observed_in_views: vec![],
        }
    }
    let apps: Vec<ApplicationRecord> = (0..50).map(|i| app(&format!("App{i:03}"))).collect();
    let dirs: Vec<String> = (0..50).map(|i| format!("App{i:03}")).collect();
    let fs = FakeFs::default().with_dirs(
        "C:/Users/u/AppData/Local",
        &dirs.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let roots = KnownRoots {
        local_app_data: Some(PathBuf::from("C:/Users/u/AppData/Local")),
        ..KnownRoots::default()
    };
    let limits = DiscoveryLimits {
        max_records: 10,
        ..DiscoveryLimits::default()
    };
    let report = discover_footprints(&apps, &roots, &fs, &limits);
    assert_eq!(report.candidates.len(), 10);
    assert_eq!(report.candidates_truncated, 40);
    let first_path = report.candidates[0].path.as_os_str().as_encoded_bytes();
    let last_path = report.candidates[9].path.as_os_str().as_encoded_bytes();
    assert!(
        first_path <= last_path,
        "published set is canonically ordered"
    );
}
