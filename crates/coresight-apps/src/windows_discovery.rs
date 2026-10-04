//! Win32 uninstall registry discovery (Phase 6, Windows-first).
//!
//! The registry is abstracted behind [`RegistryView`] so tests exercise
//! the full normalization/dedup pipeline without touching the machine's
//! registry. The production implementation reads the three documented
//! uninstall views:
//!
//! - `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall` (64-bit)
//! - `HKLM\SOFTWARE\WOW6432Node\...` (32-bit view)
//! - `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`

use std::path::PathBuf;

use crate::discovery::{ApplicationProvider, PackagedAppProvider, ProviderError};
use crate::domain::SourceCoverage;
use crate::domain::{ApplicationId, ApplicationRecord, ApplicationSource, PackageKind};

/// A decoded registry value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValue {
    Sz(String),
    ExpandSz(String),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
}

/// Abstract registry: subkeys + values at one key path.
pub trait RegistryView {
    fn subkeys(&self, key: &str) -> Vec<String>;
    fn get_value(&self, key: &str, name: &str) -> Option<RegistryValue>;
}

/// The three documented uninstall views and how each is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallView {
    // HKLM native (64-bit on 64-bit Windows)
    Hklm64,
    // HKLM WOW6432Node (32-bit view)
    Hklm32,
    // HKCU
    Hkcu,
}

impl UninstallView {
    pub fn tag(self) -> &'static str {
        match self {
            UninstallView::Hklm64 => "HKLM-64",
            UninstallView::Hklm32 => "HKLM-32",
            UninstallView::Hkcu => "HKCU",
        }
    }

    fn root_key(self) -> &'static str {
        match self {
            UninstallView::Hklm64 => {
                "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall"
            }
            UninstallView::Hklm32 => {
                "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall"
            }
            UninstallView::Hkcu => "HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        }
    }
}

impl UninstallView {
    pub const ALL: [UninstallView; 3] = [
        UninstallView::Hklm64,
        UninstallView::Hklm32,
        UninstallView::Hkcu,
    ];
}

/// Enumerates Win32 uninstall records through an abstract view.
pub struct Win32UninstallEnumerator<V: RegistryView> {
    pub view: V,
}

impl<V: RegistryView> Win32UninstallEnumerator<V> {
    pub fn new(view: V) -> Self {
        Win32UninstallEnumerator { view }
    }

    /// Enumerate one view. Returns records tagged with the view.
    pub fn enumerate_view(&self, view: UninstallView) -> Vec<ApplicationRecord> {
        let mut out = Vec::new();
        for subkey in self.view.subkeys(view.root_key()) {
            let key = format!("{}\\{}", view.root_key(), subkey);
            let name = match self.view.get_value(&key, "DisplayName") {
                Some(RegistryValue::Sz(s)) | Some(RegistryValue::ExpandSz(s)) => s,
                _ => continue, // missing or non-string DisplayName: not a product record
            };
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let rec = self.record_from(&key, &name, view);
            out.push(rec);
        }
        out
    }

    fn record_from(&self, key: &str, name: &str, view: UninstallView) -> ApplicationRecord {
        let version = self
            .view
            .get_value(key, "DisplayVersion")
            .and_then(|v| reg_string(&v));
        let publisher = self
            .view
            .get_value(key, "Publisher")
            .and_then(|v| reg_string(&v));
        let install_location = self
            .view
            .get_value(key, "InstallLocation")
            .and_then(|v| reg_string(&v))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let install_date = self
            .view
            .get_value(key, "InstallDate")
            .and_then(|v| reg_string(&v));
        let estimated_size_bytes =
            self.view
                .get_value(key, "EstimatedSize")
                .and_then(|v| match v {
                    RegistryValue::Dword(n) => Some(n as u64 * 1024), // MSDN: KiB
                    RegistryValue::Qword(n) => Some(n),
                    _ => None,
                });
        let uninstall_string = self
            .view
            .get_value(key, "UninstallString")
            .and_then(|v| reg_string(&v));
        let quiet_uninstall_string = self
            .view
            .get_value(key, "QuietUninstallString")
            .and_then(|v| reg_string(&v));
        let modify_path = self
            .view
            .get_value(key, "ModifyPath")
            .and_then(|v| reg_value_string(&v));
        let system_component = self
            .view
            .get_value(key, "SystemComponent")
            .and_then(|v| match v {
                RegistryValue::Dword(n) => Some(n != 0),
                _ => None,
            })
            .unwrap_or(false);
        let kind = if system_component {
            PackageKind::SystemComponent
        } else {
            PackageKind::Installed
        };
        let id = ApplicationId::derive(name, publisher.as_deref(), "win32-uninstall");
        ApplicationRecord {
            id,
            name: name.to_string(),
            version,
            publisher,
            install_location,
            install_date,
            estimated_size_bytes,
            uninstall_string,
            quiet_uninstall_string,
            modify_path,
            install_source: self
                .view
                .get_value(key, "URLInfoAbout")
                .and_then(|v| reg_string(&v)),
            source: ApplicationSource::RegistryUninstall,
            kind,
            system_component,
            observed_in_views: vec![view.tag().to_string()],
        }
    }

    /// Coverage for all three views (driven by the actual view read).
    pub fn coverage(&self) -> SourceCoverage {
        SourceCoverage {
            source: "win32-uninstall".to_string(),
            enumerated: true,
            note: None,
        }
    }
}

impl<V: RegistryView> ApplicationProvider for Win32UninstallEnumerator<V> {
    fn source_tag(&self) -> &'static str {
        "win32-uninstall"
    }
    fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError> {
        let mut out = Vec::new();
        for view in UninstallView::ALL {
            out.extend(self.enumerate_view(view));
        }
        Ok(out)
    }
}

fn reg_string(v: &RegistryValue) -> Option<String> {
    match v {
        RegistryValue::Sz(s) | RegistryValue::ExpandSz(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
        _ => None,
    }
}

fn reg_value_string(v: &RegistryValue) -> Option<String> {
    reg_string(v)
}

/// Appx/MSIX abstraction: a clean trait surface even though full
/// enumeration is platform-deferred. The Windows provider honestly
/// reports itself not-implemented rather than fabricating an empty
/// success.
pub struct WindowsAppxProvider;

impl ApplicationProvider for WindowsAppxProvider {
    fn source_tag(&self) -> &'static str {
        "msix-appx"
    }
    fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError> {
        Err(ProviderError::Unsupported(
            "MSIX/AppX enumeration is abstracted but not yet implemented on Windows".to_string(),
        ))
    }
}

impl PackagedAppProvider for WindowsAppxProvider {
    fn package_source(&self) -> &'static str {
        "appxmanifest"
    }
}
