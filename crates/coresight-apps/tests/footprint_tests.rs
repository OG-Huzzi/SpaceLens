//! Footprint discovery tests: confirmed install location, user data,
//! cache, logs, ambiguous directories, shared runtime, unrelated
//! same-name directory, missing install location.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coresight_apps::{
    discover_footprints, normalize_name, ApplicationId, ApplicationRecord, ApplicationSource,
    Confidence, FootprintKind, KnownRoots, PackageKind, PathProber,
};

#[derive(Default)]
struct FakeFs {
    dirs: BTreeMap<PathBuf, Vec<PathBuf>>,
    entries: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl FakeFs {
    fn with_dirs(mut self, parent: &str, children: &[&str]) -> Self {
        self.dirs.insert(
            PathBuf::from(parent),
            children.iter().map(PathBuf::from).collect(),
        );
        self
    }
    fn with_entries(mut self, parent: &str, children: &[&str]) -> Self {
        self.entries.insert(
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
        self.entries.get(dir).cloned().unwrap_or_default()
    }
}

fn app(name: &str, publisher: &str, install_location: Option<&str>) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, Some(publisher), "win32-uninstall"),
        name: name.to_string(),
        version: Some("1.0".into()),
        publisher: Some(publisher.to_string()),
        install_location: install_location.map(PathBuf::from),
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
fn confirmed_install_location_candidate() {
    let apps = [app(
        "VLC media player",
        "VideoLAN",
        Some("C:\\Program Files\\VideoLAN\\VLC"),
    )];
    let out = discover_footprints(&apps, &KnownRoots::default(), &FakeFs::default());
    let inst = out
        .iter()
        .find(|c| c.kind == FootprintKind::InstallationDirectory)
        .expect("install dir candidate");
    assert_eq!(inst.confidence, Confidence::Confirmed);
    assert!(!inst.evidence.is_empty());
}

#[test]
fn user_data_under_standard_root_is_probable() {
    let apps = [app("VLC media player", "VideoLAN", None)];
    let fs = FakeFs::default().with_dirs(
        "C:\\Users\\U\\AppData\\Roaming",
        &["C:\\Users\\U\\AppData\\Roaming\\vlc"],
    );
    let roots = KnownRoots {
        roaming_app_data: Some(PathBuf::from("C:\\Users\\U\\AppData\\Roaming")),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    let cand = out
        .iter()
        .find(|c| c.kind == FootprintKind::UserData)
        .expect("user data candidate");
    assert_eq!(cand.confidence, Confidence::Probable);
    assert!(cand
        .evidence
        .iter()
        .any(|e| e.kind == coresight_apps::EvidenceKind::KnownApplicationDirectory));
}

#[test]
fn publisher_directory_with_app_child_is_strong() {
    let apps = [app("Spotify", "Spotify AB", None)];
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\Spotify AB"]);
    let fs = fs.with_dirs(
        "C:\\ProgramData\\Spotify AB",
        &["C:\\ProgramData\\Spotify AB\\Spotify"],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    let cand = out
        .iter()
        .find(|c| c.path == PathBuf::from("C:\\ProgramData\\Spotify AB\\Spotify"))
        .expect("strong candidate");
    assert_eq!(cand.confidence, Confidence::Strong);
}

#[test]
fn cache_and_logs_are_typed() {
    let apps = [app("MyApp", "Vendor", None)];
    let fs = FakeFs::default().with_dirs(
        "C:\\Users\\U\\AppData\\Local",
        &[
            "C:\\Users\\U\\AppData\\Local\\MyAppCache",
            "C:\\Users\\U\\AppData\\Local\\MyAppLogs",
        ],
    );
    let roots = KnownRoots {
        local_app_data: Some(PathBuf::from("C:\\Users\\U\\AppData\\Local")),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    assert!(out.iter().any(|c| c.kind == FootprintKind::Cache));
    assert!(out.iter().any(|c| c.kind == FootprintKind::Logs));
}

#[test]
fn unrelated_same_name_directory_is_possible_not_confirmed() {
    let apps = [app("VLC media player", "VideoLAN", None)];
    let fs = FakeFs::default().with_dirs(
        "C:\\Users\\U\\AppData\\Roaming",
        &["C:\\Users\\U\\AppData\\Roaming\\vlc"],
    );
    let roots = KnownRoots {
        roaming_app_data: Some(PathBuf::from("C:\\Users\\U\\AppData\\Roaming")),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    let cand = out.iter().find(|c| c.path.ends_with("vlc")).unwrap();
    // A name-only match must never be Confirmed.
    assert!(cand.confidence <= Confidence::Probable);
    assert_ne!(cand.confidence, Confidence::Confirmed);
}

#[test]
fn shared_runtime_directory_does_not_claim_ownership() {
    // Shared runtime "VC++ Redist" has install location but its
    // cache-like dir under ProgramData is only Probable — never
    // Confirmed ownership of a random same-named dir.
    let apps = [app("Microsoft VC++ Redistributable", "Microsoft", None)];
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\Microsoft DCOM"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    // No candidate may claim Confirmed from name coincidence.
    assert!(out.iter().all(|c| c.confidence != Confidence::Confirmed));
}

#[test]
fn missing_install_location_still_scans_standard_roots() {
    let apps = [app("Portable Tool", "Vendor", None)];
    let fs = FakeFs::default().with_dirs("C:\\ProgramData", &["C:\\ProgramData\\Portable Tool"]);
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    assert!(out.iter().any(|c| c.kind == FootprintKind::UserData));
}

#[test]
fn shortcut_entries_are_detected() {
    let apps = [app("Example", "Vendor", None)];
    let fs = FakeFs::default().with_entries(
        "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs",
        &["C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Example.lnk"],
    );
    let roots = KnownRoots {
        start_menu_programs: Some(PathBuf::from(
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs",
        )),
        ..Default::default()
    };
    let out = discover_footprints(&apps, &roots, &fs);
    assert!(out.iter().any(|c| c.kind == FootprintKind::ShortcutEntry));
}

#[test]
fn normalize_name_collapses_separators() {
    assert_eq!(normalize_name("Spotify_AB"), "spotify ab");
    assert_eq!(normalize_name("  VLC  media  player "), "vlc media player");
    assert_eq!(normalize_name("My-App.Name"), "my app name");
}

#[test]
fn determinism_same_input_same_order() {
    let apps = [app("Alpha", "Vendor", None), app("Beta", "Vendor", None)];
    let fs = FakeFs::default().with_dirs(
        "C:\\ProgramData",
        &["C:\\ProgramData\\Alpha", "C:\\ProgramData\\Beta"],
    );
    let roots = KnownRoots {
        program_data: Some(PathBuf::from("C:\\ProgramData")),
        ..Default::default()
    };
    let a = discover_footprints(&apps, &roots, &fs);
    let b = discover_footprints(&apps, &roots, &fs);
    assert_eq!(a, b);
}
