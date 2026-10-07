//! Phase 6.2 integration test matrix: the application-intelligence layer.
//!
//! Every test here is portable (no OS-gated imports) so the whole matrix runs
//! on all three CI platforms. Categories:
//!
//! * identity (object identity width; logical application identity)
//! * provider/footprint determinism under permutation
//! * evidence correlation ceilings
//! * shared / conflicting ownership
//! * access-state semantics (denied != empty != missing)
//! * hostile bounds (10k directories, 50k entries, 5k candidates)
//! * invariants (commutativity, idempotence, boundedness, safety)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coresight_apps::{
    analyze, assess, can_authorize_execution, discover_footprints, merge_inventory, ApplicationId,
    ApplicationRecord, ApplicationSource, BoundedListing, CandidateBlocker, CandidateKind,
    Confidence, CorrelationGroup, DiscoveryLimits, EvidenceAccumulator, EvidenceKind,
    EvidenceSource, EvidenceStrength, FootprintReport, KnownRoots, MatchedAttribute,
    ObservedArtifact, OwnershipAssessment, OwnershipEvidence, PackageKind, PathProber, ProbedKind,
    ProviderOutcome, RelationKind, SharedStatus, SourceCoverage,
};
use coresight_capabilities::access::AccessState;
use coresight_identity::ObjectIdentity;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct FakeProber {
    dirs: BTreeMap<PathBuf, Vec<PathBuf>>,
    files: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl FakeProber {
    fn with_dirs(mut self, parent: &str, children: &[&str]) -> Self {
        self.dirs.insert(
            PathBuf::from(parent),
            children.iter().map(PathBuf::from).collect(),
        );
        self
    }
}

impl PathProber for FakeProber {
    fn children_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
        let mut set = std::collections::BTreeSet::new();
        let mut overflow = 0u64;
        for name in self.dirs.get(dir).cloned().unwrap_or_default() {
            coresight_apps::offer_path(&mut set, max, name, &mut overflow);
        }
        BoundedListing {
            names: set.into_iter().collect(),
            overflow,
        }
    }
    fn entries_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
        let mut set = std::collections::BTreeSet::new();
        let mut overflow = 0u64;
        for name in self.files.get(dir).cloned().unwrap_or_default() {
            coresight_apps::offer_path(&mut set, max, name, &mut overflow);
        }
        BoundedListing {
            names: set.into_iter().collect(),
            overflow,
        }
    }
}

fn app(name: &str, publisher: Option<&str>, source: ApplicationSource) -> ApplicationRecord {
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
        source: source.clone(),
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
        bundle_identifier: None,
        executable_path: None,
        provenance: vec![source],
    }
}

fn outcome(records: Vec<ApplicationRecord>) -> ProviderOutcome {
    ProviderOutcome {
        records,
        coverage: SourceCoverage::complete("fixture"),
    }
}

fn artifact(path: &str, attributed: Vec<(ApplicationId, EvidenceKind)>) -> ObservedArtifact {
    ObservedArtifact {
        path: PathBuf::from(path),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity {
            volume: 1,
            file_id: 100,
            file_id_hi: None,
        }),
        size: Some(1),
        attributed_to: attributed,
    }
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[test]
fn object_identity_widths_stay_distinct_through_analysis() {
    let a = app("Wide", None, ApplicationSource::RegistryUninstall);
    let narrow = ObservedArtifact {
        path: PathBuf::from("/n"),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity::narrow(1, 2)),
        size: None,
        attributed_to: vec![(a.id.clone(), EvidenceKind::InstallLocation)],
    };
    let wide = ObservedArtifact {
        path: PathBuf::from("/w"),
        kind: ProbedKind::File,
        identity: Some(ObjectIdentity {
            volume: 1,
            file_id: 2,
            file_id_hi: Some(9),
        }),
        size: None,
        attributed_to: vec![(a.id.clone(), EvidenceKind::InstallLocation)],
    };
    let out = analyze(&[a], &[narrow, wide], &DiscoveryLimits::default());
    assert_eq!(out.artifacts.len(), 2);
    let n = &out.artifacts[0].identity.unwrap();
    let w = &out.artifacts[1].identity.unwrap();
    assert_eq!(*n, ObjectIdentity::narrow(1, 2), "narrow never widened");
    assert_eq!(
        *w,
        ObjectIdentity {
            volume: 1,
            file_id: 2,
            file_id_hi: Some(9)
        }
    );
    assert_ne!(*n, *w, "same low pair, different high bits are DISTINCT");
}

#[test]
fn missing_object_identity_is_explicit_and_never_fabricated() {
    let a = app("NoId", None, ApplicationSource::RegistryUninstall);
    let art = ObservedArtifact {
        path: PathBuf::from("/x"),
        kind: ProbedKind::File,
        identity: None,
        size: None,
        attributed_to: vec![(a.id.clone(), EvidenceKind::InstallLocation)],
    };
    let out = analyze(&[a], &[art], &DiscoveryLimits::default());
    assert!(out.artifacts[0].identity.is_none());
    assert!(out.candidates[0]
        .blockers
        .contains(&CandidateBlocker::UnprovenObjectIdentity));
}

#[test]
fn logical_application_identity_ignores_the_source() {
    let win = app(
        "Example",
        Some("Vendor"),
        ApplicationSource::RegistryUninstall,
    );
    let mac = app(
        "Example",
        Some("Vendor"),
        ApplicationSource::BundleInfoPlist,
    );
    assert_eq!(win.id, mac.id, "source is provenance, not identity");
    let inv = merge_inventory(
        vec![outcome(vec![win]), outcome(vec![mac])],
        &DiscoveryLimits::default(),
    );
    assert_eq!(inv.records.len(), 1, "same logical application");
    let prov = &inv.records[0].provenance;
    assert!(prov.contains(&ApplicationSource::RegistryUninstall));
    assert!(prov.contains(&ApplicationSource::BundleInfoPlist));
}

#[test]
fn different_publisher_or_name_is_a_different_application() {
    let a = app(
        "Example",
        Some("Vendor A"),
        ApplicationSource::RegistryUninstall,
    );
    let b = app(
        "Example",
        Some("Vendor B"),
        ApplicationSource::RegistryUninstall,
    );
    let c = app(
        "Other",
        Some("Vendor A"),
        ApplicationSource::RegistryUninstall,
    );
    assert_ne!(a.id, b.id);
    assert_ne!(a.id, c.id);
    let inv = merge_inventory(vec![outcome(vec![a, b, c])], &DiscoveryLimits::default());
    assert_eq!(inv.records.len(), 3);
}

#[test]
fn missing_publisher_merges_with_itself_only() {
    let a = app("Example", None, ApplicationSource::RegistryUninstall);
    let b = app(
        "Example",
        Some("Vendor"),
        ApplicationSource::RegistryUninstall,
    );
    assert_ne!(a.id, b.id);
    let inv = merge_inventory(vec![outcome(vec![a, b])], &DiscoveryLimits::default());
    assert_eq!(inv.records.len(), 2);
}

#[test]
fn merge_is_commutative_and_idempotent() {
    let a = app("Alpha", Some("V"), ApplicationSource::RegistryUninstall);
    let b = app("Beta", Some("V"), ApplicationSource::PackagedApp);
    let forward = merge_inventory(
        vec![outcome(vec![a.clone(), b.clone()]), outcome(vec![])],
        &DiscoveryLimits::default(),
    );
    let backward = merge_inventory(
        vec![outcome(vec![]), outcome(vec![b.clone(), a.clone()])],
        &DiscoveryLimits::default(),
    );
    assert_eq!(
        forward.records, backward.records,
        "merge(a,b) == merge(b,a)"
    );
    let once = merge_inventory(vec![outcome(vec![a.clone()])], &DiscoveryLimits::default());
    let twice = merge_inventory(
        vec![outcome(vec![a.clone()]), outcome(vec![a.clone()])],
        &DiscoveryLimits::default(),
    );
    assert_eq!(
        once.records, twice.records,
        "idempotent for identical input"
    );
}

// ---------------------------------------------------------------------------
// Evidence correlation ceiling
// ---------------------------------------------------------------------------

fn name_derived_evidence(path: &str) -> OwnershipEvidence {
    OwnershipEvidence::new(
        EvidenceKind::DirectoryNameSimilarity,
        EvidenceSource::FilesystemPathHeuristic,
        EvidenceStrength::Direct, // deliberately over-claimed
        CorrelationGroup::NameDerived,
        coresight_apps::AssociationScope::ThisMachine,
        PathBuf::from(path),
        MatchedAttribute::ApplicationName,
        Some("Example".into()),
    )
}

#[test]
fn correlated_name_heuristics_cannot_inflate_confidence() {
    let mut acc = EvidenceAccumulator::new(16);
    // Three name-derived "signals" — all from the same root.
    acc.offer(name_derived_evidence("/a"));
    acc.offer(OwnershipEvidence::new(
        EvidenceKind::FilenameSimilarity,
        EvidenceSource::FilesystemPathHeuristic,
        EvidenceStrength::Direct,
        CorrelationGroup::NameDerived,
        coresight_apps::AssociationScope::ThisMachine,
        PathBuf::from("/b"),
        MatchedAttribute::ApplicationName,
        Some("Example".into()),
    ));
    acc.offer(OwnershipEvidence::new(
        EvidenceKind::PublisherDirectory,
        EvidenceSource::FilesystemPathHeuristic,
        EvidenceStrength::Direct,
        CorrelationGroup::NameDerived,
        coresight_apps::AssociationScope::ThisMachine,
        PathBuf::from("/c"),
        MatchedAttribute::Publisher,
        Some("Vendor".into()),
    ));
    assert_eq!(
        acc.assessment(),
        OwnershipAssessment::Weak,
        "one correlation group is one vote, however many items it has"
    );
}

#[test]
fn construction_clamps_overclaimed_strength() {
    let e = name_derived_evidence("/a");
    assert_eq!(
        e.strength,
        EvidenceStrength::Weak,
        "the group ceiling clamps at construction"
    );
}

#[test]
fn independent_groups_corroborate_only_to_moderate() {
    let mut m = BTreeMap::new();
    m.insert(CorrelationGroup::NameDerived, EvidenceStrength::Weak);
    assert_eq!(assess(&m), OwnershipAssessment::Weak);
    m.insert(
        CorrelationGroup::InstallRootStructure,
        EvidenceStrength::Moderate,
    );
    assert_eq!(
        assess(&m),
        OwnershipAssessment::Moderate,
        "two independent groups lift Weak to Moderate and no further"
    );
    let mut strong = BTreeMap::new();
    strong.insert(
        CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
        EvidenceStrength::Direct,
    );
    assert_eq!(assess(&strong), OwnershipAssessment::Direct);
}

// ---------------------------------------------------------------------------
// Shared / conflicting ownership
// ---------------------------------------------------------------------------

#[test]
fn two_strong_claims_preserve_the_conflict() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let b = app("Beta", None, ApplicationSource::RegistryUninstall);
    let arts = vec![artifact(
        "/contested",
        vec![
            (a.id.clone(), EvidenceKind::InstallLocation),
            (b.id.clone(), EvidenceKind::InstallLocation),
        ],
    )];
    let out = analyze(&[a, b], &arts, &DiscoveryLimits::default());
    assert_eq!(out.artifacts[0].status, SharedStatus::Conflicting);
    assert_eq!(out.artifacts[0].claimants.len(), 2);
    assert!(out
        .relationships
        .iter()
        .all(|r| r.kind == RelationKind::Conflicting));
}

#[test]
fn adding_a_second_provider_never_overwrites_the_first_owner() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let b = app("Beta", None, ApplicationSource::PackagedApp);
    let only_a = vec![artifact(
        "/x",
        vec![(a.id.clone(), EvidenceKind::InstallLocation)],
    )];
    let both = vec![artifact(
        "/x",
        vec![
            (a.id.clone(), EvidenceKind::InstallLocation),
            (b.id.clone(), EvidenceKind::PackageIdentity),
        ],
    )];
    let one = analyze(
        std::slice::from_ref(&a),
        &only_a,
        &DiscoveryLimits::default(),
    );
    assert_eq!(one.artifacts[0].status, SharedStatus::Exclusive);
    let two = analyze(&[a, b], &both, &DiscoveryLimits::default());
    assert_eq!(two.artifacts[0].status, SharedStatus::Conflicting);
    assert_eq!(
        two.artifacts[0].claimants.len(),
        2,
        "a later claimant never replaces an earlier owner"
    );
}

#[test]
fn containment_alone_is_contains_never_owns() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let arts = vec![artifact(
        "/root/file",
        vec![(a.id.clone(), EvidenceKind::InstallRootContainment)],
    )];
    let out = analyze(&[a], &arts, &DiscoveryLimits::default());
    assert_eq!(
        out.relationships[0].kind,
        RelationKind::Contains,
        "structural containment is Contains, not Owns"
    );
    // It may be credible that the artifact is CONTAINED, but the assessment
    // never reaches the strength that would assert ownership.
    assert!(out.relationships[0].assessment <= OwnershipAssessment::Moderate);
    assert_ne!(out.relationships[0].assessment, OwnershipAssessment::Direct);
}

#[test]
fn weak_and_strong_together_are_exclusive_not_conflicting() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let b = app("Beta", None, ApplicationSource::RegistryUninstall);
    let arts = vec![artifact(
        "/mixed",
        vec![
            (a.id.clone(), EvidenceKind::InstallLocation),
            (b.id.clone(), EvidenceKind::FilenameSimilarity),
        ],
    )];
    let out = analyze(&[a, b], &arts, &DiscoveryLimits::default());
    assert_eq!(out.artifacts[0].status, SharedStatus::Exclusive);
}

// ---------------------------------------------------------------------------
// Access-state semantics
// ---------------------------------------------------------------------------

#[test]
fn denied_is_never_empty_and_never_missing() {
    let prober = FakeProber::default();
    // A prober that cannot service the request reports Unsupported — never a
    // silent empty listing. (The trait default is honest by construction.)
    let obs = prober.list_dir(Path::new("/denied"), 4);
    assert_eq!(obs.access, AccessState::Unsupported);
    assert_ne!(obs.access, AccessState::Empty);

    // A typed observation carrying a denial has no payload and stays distinct
    // from both an empty read and a proven absence.
    let denied = coresight_apps::DirectoryObservation::inaccessible("denied");
    let empty = coresight_apps::DirectoryObservation::read(Vec::new(), 0);
    let missing = coresight_apps::DirectoryObservation::does_not_exist();
    assert_eq!(denied.access, AccessState::ExistsButInaccessible);
    assert_eq!(empty.access, AccessState::Empty);
    assert_eq!(missing.access, AccessState::DoesNotExist);
    assert_ne!(denied.access, empty.access);
    assert_ne!(denied.access, missing.access);
    assert_ne!(empty.access, missing.access);
    assert!(denied.is_well_formed(), "no payload under a denial");
    assert!(empty.is_well_formed());
}

#[test]
fn inaccessible_states_carry_no_payload() {
    for obs in [
        coresight_apps::DirectoryObservation::inaccessible("d"),
        coresight_apps::DirectoryObservation::failed("f"),
        coresight_apps::DirectoryObservation::unsupported("u"),
        coresight_apps::DirectoryObservation::does_not_exist(),
    ] {
        assert!(obs.entries.is_empty());
        assert_eq!(obs.overflow, 0);
        assert!(obs.is_well_formed());
    }
}

#[test]
fn unobserved_file_states_carry_no_bytes() {
    for obs in [
        coresight_apps::FileObservation::inaccessible("d"),
        coresight_apps::FileObservation::failed("f"),
        coresight_apps::FileObservation::unsupported("u"),
        coresight_apps::FileObservation::does_not_exist(),
    ] {
        assert!(obs.bytes.is_empty());
        assert!(!obs.truncated);
        assert!(obs.is_well_formed());
    }
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn footprint_output_is_identical_under_every_permutation() {
    let apps: Vec<ApplicationRecord> = (0..6)
        .map(|i| {
            app(
                &format!("App{i}"),
                Some("Vendor"),
                ApplicationSource::RegistryUninstall,
            )
        })
        .collect();
    let dirs: Vec<String> = (0..6).map(|i| format!("C:/ProgramData/App{i}")).collect();
    let fs = FakeProber::default().with_dirs(
        "C:/ProgramData",
        &dirs.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..KnownRoots::default()
    };
    let baseline = discover_footprints(&apps, &roots, &fs, &DiscoveryLimits::default());

    // Every rotation of the app order must give the same canonical report.
    for shift in 0..apps.len() {
        let mut rotated = apps.clone();
        rotated.rotate_left(shift);
        let got = discover_footprints(&rotated, &roots, &fs, &DiscoveryLimits::default());
        assert_eq!(baseline, got, "rotation {shift} changed the report");
    }
    let mut reversed = apps.clone();
    reversed.reverse();
    assert_eq!(
        baseline,
        discover_footprints(&reversed, &roots, &fs, &DiscoveryLimits::default())
    );
}

#[test]
fn footprint_evidence_order_is_canonical() {
    let a = app(
        "Spotify",
        Some("Spotify AB"),
        ApplicationSource::RegistryUninstall,
    );
    let fs = FakeProber::default()
        .with_dirs("C:/ProgramData", &["C:/ProgramData/Spotify AB"])
        .with_dirs(
            "C:/ProgramData/Spotify AB",
            &["C:/ProgramData/Spotify AB/Spotify"],
        );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..KnownRoots::default()
    };
    let out = discover_footprints(&[a], &roots, &fs, &DiscoveryLimits::default());
    for cand in &out.candidates {
        let mut sorted = cand.evidence.clone();
        sorted.sort();
        assert_eq!(sorted, cand.evidence, "evidence is canonically ordered");
    }
}

#[test]
fn analysis_is_identical_under_arrival_permutation() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let b = app("Beta", None, ApplicationSource::PackagedApp);
    let arts = vec![
        artifact(
            "/z",
            vec![
                (a.id.clone(), EvidenceKind::InstallLocation),
                (b.id.clone(), EvidenceKind::FilenameSimilarity),
            ],
        ),
        artifact("/a", vec![(b.id.clone(), EvidenceKind::PackageIdentity)]),
    ];
    let mut reversed = arts.clone();
    reversed.reverse();
    let forward = analyze(&[a.clone(), b.clone()], &arts, &DiscoveryLimits::default());
    let backward = analyze(&[b, a], &reversed, &DiscoveryLimits::default());
    assert_eq!(forward, backward);
}

// ---------------------------------------------------------------------------
// Bounds (hostile fixtures)
// ---------------------------------------------------------------------------

#[test]
fn hostile_registry_view_is_bounded_and_exactly_counted() {
    // 10,000 subkeys against a 50-name bound.
    use coresight_apps::{
        offer_name, RegistryValue, RegistryView, SubkeyEnumeration, Win32UninstallEnumerator,
    };

    #[derive(Default)]
    struct HostileRegistry {
        subs: BTreeMap<String, Vec<String>>,
    }
    impl RegistryView for HostileRegistry {
        fn subkeys_bounded(&self, key: &str, max: usize) -> SubkeyEnumeration {
            let mut set = std::collections::BTreeSet::new();
            let mut truncated = 0u64;
            for name in self.subs.get(key).cloned().unwrap_or_default() {
                offer_name(&mut set, max, name, &mut truncated);
            }
            SubkeyEnumeration {
                keys: set.into_iter().collect(),
                skipped_oversized: 0,
                truncated,
                incomplete: false,
            }
        }
        fn get_value(&self, _key: &str, _name: &str) -> Option<RegistryValue> {
            None
        }
    }

    let root = "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    let registry = HostileRegistry {
        subs: BTreeMap::from([(
            root.to_string(),
            (0..10_000).map(|i| format!("key-{i:05}")).collect(),
        )]),
    };
    let enumerator = Win32UninstallEnumerator::new(registry).with_max_subkeys_per_view(50);
    let enum_out = enumerator.view.subkeys_bounded(root, 50);
    assert_eq!(enum_out.keys.len(), 50, "only the bound is materialized");
    assert_eq!(enum_out.truncated, 9_950, "overflow counted exactly");
}

#[test]
fn hostile_footprint_scan_retains_only_the_bound() {
    // A hostile 10,000-directory root scanned for many applications. The
    // per-root child bound keeps the WORK bounded (the prober never
    // materializes the whole directory) and the published candidate set is
    // capped at max_records with exact overflow accounting.
    let apps: Vec<ApplicationRecord> = (0..200)
        .map(|i| {
            app(
                &format!("App{i:04}"),
                Some("Vendor"),
                ApplicationSource::RegistryUninstall,
            )
        })
        .collect();
    let dirs: Vec<String> = (0..10_000)
        .map(|i| format!("C:/ProgramData/App{i:04}"))
        .collect();
    let fs = FakeProber::default().with_dirs(
        "C:/ProgramData",
        &dirs.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:/ProgramData")),
        ..KnownRoots::default()
    };
    let limits = DiscoveryLimits {
        max_records: 128,
        max_children_per_root: 256,
        ..DiscoveryLimits::default()
    };
    let report: FootprintReport = discover_footprints(&apps, &roots, &fs, &limits);
    assert!(
        report.candidates.len() <= 128,
        "retained {} > bound",
        report.candidates.len()
    );
    assert!(
        report.children_truncated > 0,
        "the child bound really applied"
    );
    // Same facts, same result — the hostile scan stays deterministic.
    let again = discover_footprints(&apps, &roots, &fs, &limits);
    assert_eq!(report, again);
}

#[test]
fn analysis_bounds_every_collection_including_evidence() {
    let a = app("Bulk", None, ApplicationSource::RegistryUninstall);
    let arts: Vec<ObservedArtifact> = (0..5_000)
        .map(|i| {
            artifact(
                &format!("/bulk/f{i:05}"),
                vec![(a.id.clone(), EvidenceKind::InstallLocation)],
            )
        })
        .collect();
    let limits = DiscoveryLimits {
        max_records: 64,
        max_evidence_per_candidate: 4,
        ..DiscoveryLimits::default()
    };
    let out = analyze(&[a], &arts, &limits);
    assert!(out.relationships.len() <= 64);
    assert!(out.artifacts.len() <= 64);
    assert!(out.candidates.len() <= 64);
    for rel in &out.relationships {
        assert!(rel.evidence.len() <= 4, "evidence payload is bounded");
    }
    assert_eq!(out.truncated.relationships_truncated, 5_000 - 64);
}

#[test]
fn hostile_prober_topk_keeps_the_canonical_subset() {
    let names: Vec<String> = (0..50_000).map(|i| format!("dir-{i:05}")).collect();
    let fs = FakeProber::default().with_dirs(
        "C:/root",
        &names.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let listing = fs.children_bounded(Path::new("C:/root"), 100);
    assert_eq!(listing.names.len(), 100);
    assert_eq!(listing.overflow, 49_900);
    assert_eq!(listing.names[0], PathBuf::from("dir-00000"));
    assert_eq!(listing.names[99], PathBuf::from("dir-00099"));
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

#[test]
fn boundedness_invariant_holds_for_every_limit() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let arts: Vec<ObservedArtifact> = (0..200)
        .map(|i| {
            artifact(
                &format!("/f{i:04}"),
                vec![(a.id.clone(), EvidenceKind::InstallLocation)],
            )
        })
        .collect();
    for limit in [0usize, 1, 7, 200] {
        let limits = DiscoveryLimits {
            max_records: limit,
            ..DiscoveryLimits::default()
        };
        let out = analyze(std::slice::from_ref(&a), &arts, &limits);
        assert!(out.relationships.len() <= limit);
        assert!(out.artifacts.len() <= limit);
        assert!(out.candidates.len() <= limit);
    }
}

#[test]
fn analysis_can_never_authorize_execution() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let arts = vec![artifact(
        "/f",
        vec![(a.id.clone(), EvidenceKind::InstallLocation)],
    )];
    let out = analyze(&[a], &arts, &DiscoveryLimits::default());
    assert!(!can_authorize_execution(&out));
    // Every candidate records the no-executor blocker.
    for c in &out.candidates {
        assert!(c
            .blockers
            .contains(&CandidateBlocker::NoExecutorInThisPhase));
    }
}

#[test]
fn every_candidate_carries_structured_evidence_not_just_prose() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let arts = vec![artifact(
        "/f",
        vec![(a.id.clone(), EvidenceKind::InstallLocation)],
    )];
    let out = analyze(&[a], &arts, &DiscoveryLimits::default());
    for c in &out.candidates {
        assert!(!c.evidence.is_empty(), "candidate without evidence");
        for e in &c.evidence {
            // The machine-readable facts, not only the rendered sentence.
            assert_eq!(e.observed_path, PathBuf::from("/f"));
            let _: EvidenceKind = e.kind;
            let _: EvidenceSource = e.source;
            let _: EvidenceStrength = e.strength;
            let _: CorrelationGroup = e.correlation_group.clone();
            let _: MatchedAttribute = e.matched_attribute;
            assert!(!e.render().is_empty(), "and it can be rendered");
        }
    }
}

#[test]
fn shared_artifacts_are_never_presented_as_exclusively_owned() {
    let a = app("Alpha", None, ApplicationSource::RegistryUninstall);
    let b = app("Beta", None, ApplicationSource::PackagedApp);
    let arts = vec![artifact(
        "/shared/lib",
        vec![
            (a.id.clone(), EvidenceKind::InstallLocation),
            (b.id.clone(), EvidenceKind::PackageIdentity),
        ],
    )];
    let out = analyze(&[a, b], &arts, &DiscoveryLimits::default());
    assert_ne!(out.artifacts[0].status, SharedStatus::Exclusive);
    let shared = out
        .candidates
        .iter()
        .filter(|c| c.kind == CandidateKind::SharedArtifact)
        .count();
    assert_eq!(shared, 2, "both claims are reported as shared artifacts");
    for c in out
        .candidates
        .iter()
        .filter(|c| c.kind == CandidateKind::SharedArtifact)
    {
        assert!(
            c.blockers.contains(&CandidateBlocker::SharedArtifact)
                || c.blockers.contains(&CandidateBlocker::ConflictingOwnership)
        );
        assert_ne!(c.confidence, Confidence::Confirmed);
    }
}
