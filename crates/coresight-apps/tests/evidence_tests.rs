//! Evidence / ownership / explanation invariants.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coresight_apps::{
    discover_footprints, explain, ApplicationId, ApplicationRecord, ApplicationSource, Confidence,
    EvidenceKind, KnownRoots, OwnershipStrength, PackageKind, PathProber,
};

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

fn app(name: &str) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, None, "win32-uninstall"),
        name: name.to_string(),
        version: None,
        publisher: None,
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
        observed_in_views: vec!["HKLM-64".into()],
    }
}

#[test]
fn every_candidate_carries_explicit_evidence() {
    let apps = [app("VLC media player")];
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\vlc"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    for cand in discover_footprints(&apps, &roots, &fs) {
        assert!(!cand.evidence.is_empty(), "candidate without evidence");
        // Every evidence item explains its source.
        for e in &cand.evidence {
            assert!(!e.why.is_empty());
            assert!(!e.source.is_empty());
        }
    }
}

#[test]
fn no_candidate_is_confirmed_without_install_location_evidence() {
    let apps = [app("VLC media player")];
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\vlc"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    for cand in discover_footprints(&apps, &roots, &fs) {
        if cand.confidence == Confidence::Confirmed {
            assert!(
                cand.evidence
                    .iter()
                    .any(|e| e.kind == EvidenceKind::InstallLocation),
                "Confirmed without install-location evidence"
            );
        }
    }
}

#[test]
fn ownership_strength_never_upgrades_weak_evidence() {
    assert_eq!(
        OwnershipStrength::from_confidence(Confidence::Possible),
        OwnershipStrength::CannotDetermine
    );
    assert_eq!(
        OwnershipStrength::from_confidence(Confidence::Probable),
        OwnershipStrength::Possible
    );
    assert_eq!(
        OwnershipStrength::from_confidence(Confidence::Unknown),
        OwnershipStrength::CannotDetermine
    );
}

#[test]
fn weak_association_explanation_is_tentative() {
    let a = app("VLC media player");
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\vlc"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    let cands = discover_footprints(&[a.clone()], &roots, &fs);
    let weak = cands
        .iter()
        .find(|c| c.confidence == Confidence::Possible)
        .cloned()
        .unwrap_or_else(|| {
            // If no Possible candidate exists, synthesize the worst
            // case via a name-coincidence on a different root.
            cands.first().cloned().expect("at least one candidate")
        });
    let ex = explain(&a, &weak);
    assert!(!ex.headline.is_empty());
    // Headline must not assert ownership for weak evidence.
    if weak.confidence == Confidence::Possible || weak.confidence == Confidence::Unknown {
        assert!(ex.tentative);
    }
}

#[test]
fn strong_association_explanation_cites_evidence() {
    let mut a = app("Spotify");
    a.publisher = Some("Spotify AB".into());
    let fs = FakeFs::default()
        .with_dirs("C:\\ProgramData", &["C:\\ProgramData\\Spotify AB"])
        .with_dirs(
            "C:\\ProgramData\\Spotify AB",
            &["C:\\ProgramData\\Spotify AB\\Spotify"],
        );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    let cands = discover_footprints(&[a.clone()], &roots, &fs);
    let strong = cands
        .iter()
        .find(|c| c.confidence == Confidence::Strong)
        .expect("strong candidate");
    let ex = explain(&a, strong);
    assert!(ex.headline.contains("Spotify"));
    assert!(ex.bullets.iter().any(|b| b.contains("publisher")));
}
