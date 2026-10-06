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
    discover_footprints, merge_inventory, ApplicationId, ApplicationProvider, ApplicationRecord,
    ApplicationSource, Confidence, DiscoveryLimits, FootprintKind, KnownRoots, OwnershipStrength,
    PackageKind, PackagedAppProvider, PathProber, ProviderOutcome, RegistryValue, RegistryView,
    SourceCoverage, SourceStatus, UninstallView, Win32UninstallEnumerator, WindowsAppxProvider,
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
    fn subkeys(&self, key: &str) -> Vec<String> {
        self.keys.get(key).cloned().unwrap_or_default()
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

fn root(view: UninstallView) -> &'static str {
    match view {
        UninstallView::Hklm64 => "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        UninstallView::Hklm32 => {
            "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall"
        }
        UninstallView::Hkcu => "HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
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
    fn children(&self, dir: &Path) -> Vec<PathBuf> {
        self.dirs.get(dir).cloned().unwrap_or_default()
    }
    fn entries(&self, dir: &Path) -> Vec<PathBuf> {
        self.dirs.get(dir).cloned().unwrap_or_default()
    }
}

fn app(name: &str, publisher: Option<&str>) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, publisher, "win32-uninstall"),
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
    let a = ApplicationId::derive("Example App", Some("Vendor"), "win32-uninstall");
    let b = ApplicationId::derive("Example App", Some("Vendor"), "win32-uninstall");
    assert_eq!(a, b);
    // Case/whitespace normalization is the documented normalization.
    let c = ApplicationId::derive("  example app ", Some("VENDOR"), "win32-uninstall");
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
    let a = ApplicationId::derive("Setup", Some("Vendor A"), "win32-uninstall");
    let b = ApplicationId::derive("Setup", Some("Vendor B"), "win32-uninstall");
    assert_ne!(a, b);
    // Missing publisher must not collide with a named publisher.
    let c = ApplicationId::derive("Setup", None, "win32-uninstall");
    assert_ne!(a, c);
    assert_ne!(b, c);
}

#[test]
fn a2_same_name_different_source_is_a_different_identity() {
    let a = ApplicationId::derive("Example", None, "win32-uninstall");
    let b = ApplicationId::derive("Example", None, "msix-appx");
    assert_ne!(
        a, b,
        "same name from different sources cannot share identity"
    );
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
    long.id = ApplicationId::derive(&long.name, Some("Vendor"), "win32-uninstall");
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
        .map(|i| format!("C:\\ProgramData\\App{i:02}"))
        .collect();
    let child_refs: Vec<&str> = children.iter().map(String::as_str).collect();
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &child_refs);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
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
        "C:\\ProgramData",
        &[
            "C:\\ProgramData\\Alpha",
            "C:\\ProgramData\\Alpha2",
            "C:\\ProgramData\\AlphaCache",
        ],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
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
        fn subkeys(&self, key: &str) -> Vec<String> {
            self.inner.subkeys(key)
        }
        fn get_value(&self, key: &str, name: &str) -> Option<RegistryValue> {
            self.inner.get_value(key, name)
        }
        fn key_present(&self, _key: &str) -> bool {
            true
        }
        fn subkeys_detailed(&self, key: &str) -> coresight_apps::SubkeyEnumeration {
            coresight_apps::SubkeyEnumeration {
                keys: self.inner.subkeys(key),
                skipped_oversized: 2,
                incomplete: false,
            }
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
    assert!(note.contains('2'), "exact skip count reported: {note}");
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
        "C:\\ProgramData",
        &[
            "C:\\ProgramData\\Alpha",
            "C:\\ProgramData\\Beta",
            "C:\\ProgramData\\Gamma",
        ],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
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
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\Steam"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
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
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\Shared"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
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
        "C:\\ProgramData",
        &[
            "C:\\ProgramData\\Microsoft",
            "C:\\ProgramData\\Microsoft Visual C++ Redistributable",
        ],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
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
    a.install_location = Some(PathBuf::from("C:\\Program Files\\VideoLAN\\VLC"));
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
