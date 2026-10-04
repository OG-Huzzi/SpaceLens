//! Windows application inventory tests using a fake registry view —
//! multiple views, duplicates, malformed metadata, system components,
//! Appx honesty, conflicting records.

use std::collections::BTreeMap;
use std::path::PathBuf;

use coresight_apps::{
    merge_inventory, ApplicationProvider, DiscoveryLimits, PackageKind, ProviderError,
    RegistryValue, RegistryView, UninstallView, Win32UninstallEnumerator, WindowsAppxProvider,
};

#[derive(Default)]
struct FakeRegistry {
    keys: BTreeMap<String, Vec<String>>,
    values: BTreeMap<(String, String), RegistryValue>,
}

impl FakeRegistry {
    fn with_subkeys(mut self, key: &str, subs: &[&str]) -> Self {
        self.keys.insert(
            key.to_string(),
            subs.iter().map(|s| s.to_string()).collect(),
        );
        self
    }
    fn with_value(mut self, key: &str, name: &str, v: RegistryValue) -> Self {
        self.values.insert((key.to_string(), name.to_string()), v);
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
}

fn key(view: UninstallView, sub: &str) -> String {
    match view {
        UninstallView::Hklm64 => {
            format!("HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{sub}")
        }
        UninstallView::Hklm32 => format!(
            "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{sub}"
        ),
        UninstallView::Hkcu => {
            format!("HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{sub}")
        }
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

#[test]
fn multiple_views_merge_duplicate_entries() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["AppX64"])
        .with_subkeys(root(UninstallView::Hklm32), &["AppX64"])
        .with_subkeys(root(UninstallView::Hkcu), &["UserApp"])
        .with_value(
            &key(UninstallView::Hklm64, "AppX64"),
            "DisplayName",
            RegistryValue::Sz("Example App".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "AppX64"),
            "Publisher",
            RegistryValue::Sz("Vendor".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "AppX64"),
            "DisplayVersion",
            RegistryValue::Sz("1.0".into()),
        )
        .with_value(
            &key(UninstallView::Hklm32, "AppX64"),
            "DisplayName",
            RegistryValue::Sz("Example App".into()),
        )
        .with_value(
            &key(UninstallView::Hklm32, "AppX64"),
            "Publisher",
            RegistryValue::Sz("Vendor".into()),
        )
        .with_value(
            &key(UninstallView::Hkcu, "UserApp"),
            "DisplayName",
            RegistryValue::Sz("User App".into()),
        )
        .with_value(
            &key(UninstallView::Hkcu, "UserApp"),
            "Publisher",
            RegistryValue::Sz("Vendor".into()),
        );

    let enumerator = Win32UninstallEnumerator::new(fake);
    let records = enumerator.enumerate().unwrap();
    let inventory = merge_inventory(
        vec![(
            records,
            coresight_apps::SourceCoverage {
                source: "win32-uninstall".into(),
                enumerated: true,
                note: None,
            },
        )],
        &DiscoveryLimits::default(),
    );
    assert_eq!(inventory.records.len(), 2);
    let example = inventory
        .records
        .iter()
        .find(|r| r.name == "Example App")
        .unwrap();
    // Duplicate across views merged, provenance preserved.
    assert_eq!(example.observed_in_views.len(), 2);
}

#[test]
fn missing_metadata_is_tolerated() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["Bare"])
        .with_value(
            &key(UninstallView::Hklm64, "Bare"),
            "DisplayName",
            RegistryValue::Sz("Bare App".into()),
        );

    let enumerator = Win32UninstallEnumerator::new(fake);
    let records = enumerator.enumerate().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].version.is_none());
    assert!(records[0].publisher.is_none());
    assert!(records[0].install_location.is_none());
    assert_eq!(records[0].kind, PackageKind::Installed);
}

#[test]
fn missing_display_name_is_skipped() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["NoName"])
        .with_value(
            &key(UninstallView::Hklm64, "NoName"),
            "Publisher",
            RegistryValue::Sz("Vendor".into()),
        );

    let records = Win32UninstallEnumerator::new(fake).enumerate().unwrap();
    assert!(records.is_empty());
}

#[test]
fn system_component_flag_produces_system_component_kind() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["Sys"])
        .with_value(
            &key(UninstallView::Hklm64, "Sys"),
            "DisplayName",
            RegistryValue::Sz("OS Feature".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "Sys"),
            "SystemComponent",
            RegistryValue::Dword(1),
        );

    let records = Win32UninstallEnumerator::new(fake).enumerate().unwrap();
    assert_eq!(records[0].kind, PackageKind::SystemComponent);
    assert!(records[0].system_component);
}

#[test]
fn malformed_estimated_size_is_treated_as_unknown() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hklm64), &["Odd"])
        .with_value(
            &key(UninstallView::Hklm64, "Odd"),
            "DisplayName",
            RegistryValue::Sz("Odd App".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "Odd"),
            "EstimatedSize",
            RegistryValue::Sz("not-a-number".into()),
        );

    let records = Win32UninstallEnumerator::new(fake).enumerate().unwrap();
    assert!(records[0].estimated_size_bytes.is_none());
}

#[test]
fn appx_provider_is_honestly_unsupported_not_silently_empty() {
    let err = WindowsAppxProvider.enumerate().unwrap_err();
    assert!(matches!(err, ProviderError::Unsupported(_)));
}

#[test]
fn duplicate_registry_entries_collapse_preferred_complete() {
    let fake = FakeRegistry::default()
        .with_subkeys(root(UninstallView::Hkcu), &["A"])
        .with_subkeys(root(UninstallView::Hklm64), &["A"])
        .with_value(
            &key(UninstallView::Hkcu, "A"),
            "DisplayName",
            RegistryValue::Sz("App".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "A"),
            "DisplayName",
            RegistryValue::Sz("App".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "A"),
            "DisplayVersion",
            RegistryValue::Sz("2.0".into()),
        )
        .with_value(
            &key(UninstallView::Hklm64, "A"),
            "InstallLocation",
            RegistryValue::Sz("C:\\Program Files\\App".into()),
        );

    let records = Win32UninstallEnumerator::new(fake).enumerate().unwrap();
    let inventory = merge_inventory(
        vec![(
            records,
            coresight_apps::SourceCoverage {
                source: "win32-uninstall".into(),
                enumerated: true,
                note: None,
            },
        )],
        &DiscoveryLimits::default(),
    );
    assert_eq!(inventory.records.len(), 1);
    assert_eq!(inventory.records[0].version.as_deref(), Some("2.0"));
    assert_eq!(
        inventory.records[0].install_location,
        Some(PathBuf::from("C:\\Program Files\\App"))
    );
}

#[cfg(windows)]
#[test]
fn real_registry_enumeration_is_deterministic_and_nonfabricated() {
    use coresight_apps::Win32RegistryView;

    let enumerator = Win32UninstallEnumerator::new(Win32RegistryView::new());
    let a = enumerator.enumerate().unwrap();
    let b = enumerator.enumerate().unwrap();
    assert_eq!(a, b);
    for r in &a {
        assert!(!r.name.trim().is_empty());
        assert!(matches!(
            r.source,
            coresight_apps::ApplicationSource::RegistryUninstall
        ));
    }
    let inv = merge_inventory(
        vec![(
            a,
            coresight_apps::SourceCoverage {
                source: "win32-uninstall".into(),
                enumerated: true,
                note: None,
            },
        )],
        &DiscoveryLimits::default(),
    );
    // Deterministic ordering.
    let names: Vec<&str> = inv.records.iter().map(|r| r.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(names, sorted);
}
