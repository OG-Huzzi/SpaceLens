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

use crate::discovery::{ApplicationProvider, PackagedAppProvider, ProviderError, ProviderOutcome};
use crate::domain::SourceCoverage;
use crate::domain::{
    ApplicationId, ApplicationRecord, ApplicationSource, PackageKind, SourceStatus,
};

/// A decoded registry value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValue {
    Sz(String),
    ExpandSz(String),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
}

/// Registry hive a CoreSight key path refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryHive {
    /// `HKLM\` — the local-machine hive.
    Hklm,
    /// `HKCU\` — the current-user hive.
    Hkcu,
}

/// Split a CoreSight registry path into its hive and subkey.
///
/// Only the documented `HKLM\` and `HKCU\` roots are recognized, and the
/// prefix match is case-sensitive (unchanged semantics). Anything else
/// returns `None`.
pub fn split_hive_path(key: &str) -> Option<(RegistryHive, &str)> {
    if let Some(rest) = key.strip_prefix("HKLM\\") {
        Some((RegistryHive::Hklm, rest))
    } else {
        key.strip_prefix("HKCU\\")
            .map(|rest| (RegistryHive::Hkcu, rest))
    }
}

/// Decode `REG_SZ`/`REG_EXPAND_SZ` bytes: little-endian UTF-16 with all
/// trailing NUL units trimmed; unpaired surrogates become U+FFFD.
///
/// This is presentation-metadata decoding only — never filesystem
/// identity. A trailing odd byte (malformed registry data) is ignored,
/// preserving the previous `chunks_exact(2)` behavior.
pub fn decode_registry_string(bytes: &[u8]) -> String {
    let mut chars: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        chars.push(u16::from_le_bytes(*pair));
    }
    while chars.last() == Some(&0) {
        chars.pop();
    }
    String::from_utf16_lossy(&chars)
}

/// Result of enumerating subkeys, honest about skips, truncation, and
/// incompleteness.
///
/// - `keys` holds at most the requested `max` names, canonically ascending
///   and deduplicated — the canonically-FIRST names, so the examined
///   subset never depends on the platform's enumeration order.
/// - `truncated` is the EXACT count of subkeys visited beyond the kept
///   set. `skipped_oversized` counts keys whose name exceeded the
///   platform's enumeration buffer.
/// - `incomplete` is true when enumeration stopped before the natural end
///   (an OS error): the kept keys are then a partial view and the
///   coverage must say so.
///
/// BOUNDEDNESS: the enumeration itself is bounded — an implementation
/// must NOT materialize the full subkey list before capping. Working
/// memory is O(max) names regardless of how many subkeys the key holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubkeyEnumeration {
    pub keys: Vec<String>,
    /// Subkeys skipped because their name exceeded the platform's
    /// enumeration buffer. Counted exactly — never silently dropped.
    pub skipped_oversized: u64,
    /// Subkeys visited beyond the kept set (exact; the output bound is
    /// real memory, not a post-hoc truncate).
    pub truncated: u64,
    /// True when enumeration stopped before the natural end (an OS
    /// error). The returned keys are then a partial view and the
    /// coverage must say so.
    pub incomplete: bool,
}

/// Streaming bounded-name insertion: keeps the `max` canonically-smallest
/// names in `set`; everything else increments `overflow`. O(max) memory,
/// call-order independent (the kept set is always the canonically-smallest
/// names seen). Implementors of [`RegistryView`] use this to enumerate
/// without materializing the full key list.
pub fn offer_name(
    set: &mut std::collections::BTreeSet<String>,
    max: usize,
    name: String,
    overflow: &mut u64,
) {
    if max == 0 {
        *overflow += 1;
        return;
    }
    if set.contains(&name) {
        return; // duplicates cannot occur from a live registry; be exact anyway
    }
    if set.len() < max {
        set.insert(name);
        return;
    }
    let largest = match set.iter().next_back() {
        Some(l) => l.clone(),
        None => {
            set.insert(name);
            return;
        }
    };
    if name < largest {
        set.remove(&largest);
        *overflow += 1;
        set.insert(name);
    } else {
        *overflow += 1;
    }
}

/// Abstract registry: values at one key path plus BOUNDED subkey
/// enumeration.
///
/// Implementations must stream: `subkeys_bounded` keeps at most `max`
/// canonically-smallest subkey names with exact truncation accounting, so
/// a hostile registry cannot balloon working memory
/// (`max_subkeys_per_view` bounds MEMORY, not merely the published list).
pub trait RegistryView {
    /// Bounded, canonically-ordered subkey enumeration of `key`.
    fn subkeys_bounded(&self, key: &str, max: usize) -> SubkeyEnumeration;
    fn get_value(&self, key: &str, name: &str) -> Option<RegistryValue>;
    /// Whether the key exists at all. Absent roots are `Unavailable`,
    /// which is NOT the same as an enumerated-empty root. Bounded by
    /// definition (asks for one subkey).
    fn key_present(&self, key: &str) -> bool {
        !self.subkeys_bounded(key, 1).keys.is_empty() || self.get_value(key, "").is_some()
    }
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
    /// Hard per-view bound on subkeys examined. Subkeys are canonically
    /// ordered before the cap, so the examined subset is deterministic;
    /// overflow is counted exactly and reported as `Partial` coverage.
    pub max_subkeys_per_view: usize,
}

impl<V: RegistryView> Win32UninstallEnumerator<V> {
    pub fn new(view: V) -> Self {
        Win32UninstallEnumerator {
            view,
            max_subkeys_per_view: DEFAULT_MAX_SUBKEYS_PER_VIEW,
        }
    }

    pub fn with_max_subkeys_per_view(mut self, max: usize) -> Self {
        self.max_subkeys_per_view = max;
        self
    }

    /// Enumerate one view. Returns records tagged with the view.
    pub fn enumerate_view(&self, view: UninstallView) -> Vec<ApplicationRecord> {
        self.enumerate_view_outcome(view).records
    }

    /// Enumerate one view, reporting whether the view root existed:
    /// an absent root is `Unavailable` (no installer records live
    /// there), which is NOT the same as an enumerated-empty root.
    fn enumerate_view_outcome(&self, view: UninstallView) -> ProviderViewOutcome {
        let root = view.root_key();
        if !self.view.key_present(root) {
            return ProviderViewOutcome {
                records: Vec::new(),
                unavailable: true,
                truncated_subkeys: 0,
                enumeration_incomplete: false,
            };
        }
        // The enumeration is bounded AT THE SOURCE: at most
        // `max_subkeys_per_view` canonically-first names are materialized,
        // so a registry with millions of keys cannot balloon working
        // memory. Truncation/skips are exact and reported as `Partial`.
        let enumeration = self.view.subkeys_bounded(root, self.max_subkeys_per_view);
        let truncated_subkeys = enumeration.truncated + enumeration.skipped_oversized;
        let mut out = Vec::new();
        for subkey in &enumeration.keys {
            let key = format!("{}\\{}", root, subkey);
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
        ProviderViewOutcome {
            records: out,
            unavailable: false,
            truncated_subkeys,
            enumeration_incomplete: enumeration.incomplete,
        }
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
        let id = ApplicationId::derive(name, publisher.as_deref());
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

    /// Coverage for all three views (driven by the actual view reads).
    /// A view whose root does not exist on this machine is reported in
    /// the note — the honest "this machine has no 32-bit view" fact,
    /// distinct from a read that found nothing.
    pub fn coverage(&self) -> SourceCoverage {
        self.enumerate_outcome().coverage
    }
}

/// One view's enumeration result, with the facts coverage is derived
/// from (the view tag itself stays with the caller's loop).
struct ProviderViewOutcome {
    records: Vec<ApplicationRecord>,
    unavailable: bool,
    truncated_subkeys: u64,
    enumeration_incomplete: bool,
}

/// Default per-view subkey bound: comfortably above any real machine's
/// uninstall key count (thousands on heavily-provisioned systems), small
/// enough that a hostile registry cannot make discovery unbounded.
pub const DEFAULT_MAX_SUBKEYS_PER_VIEW: usize = 16_384;

impl<V: RegistryView> ApplicationProvider for Win32UninstallEnumerator<V> {
    fn source_tag(&self) -> &'static str {
        "win32-uninstall"
    }
    fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError> {
        Ok(self.enumerate_outcome().records)
    }

    fn enumerate_outcome(&self) -> ProviderOutcome {
        let mut records = Vec::new();
        let mut missing_views: Vec<&'static str> = Vec::new();
        let mut seen_views: Vec<&'static str> = Vec::new();
        let mut truncated_total = 0u64;
        let mut incomplete_views: Vec<&'static str> = Vec::new();
        for view in UninstallView::ALL {
            let outcome = self.enumerate_view_outcome(view);
            if outcome.unavailable {
                missing_views.push(view.tag());
            } else {
                seen_views.push(view.tag());
            }
            truncated_total += outcome.truncated_subkeys;
            if outcome.enumeration_incomplete {
                incomplete_views.push(view.tag());
            }
            records.extend(outcome.records);
        }
        // Every expected view absent (and no records): the whole source
        // is unavailable on this machine — never a "successfully empty"
        // inventory.
        let coverage = if seen_views.is_empty() {
            SourceCoverage::with_status(
                "win32-uninstall",
                SourceStatus::Unavailable,
                Some(format!(
                    "none of the three uninstall views exist on this machine (missing: {})",
                    missing_views.join(", ")
                )),
            )
        } else if missing_views.is_empty() && truncated_total == 0 && incomplete_views.is_empty() {
            SourceCoverage::complete("win32-uninstall")
        } else {
            let mut notes = Vec::new();
            if !missing_views.is_empty() {
                notes.push(format!(
                    "absent on this machine: {}",
                    missing_views.join(", ")
                ));
            }
            if truncated_total > 0 {
                notes.push(format!(
                    "{truncated_total} subkeys were skipped (bound or oversized name)"
                ));
            }
            if !incomplete_views.is_empty() {
                notes.push(format!(
                    "enumeration stopped early in: {}",
                    incomplete_views.join(", ")
                ));
            }
            SourceCoverage::with_status(
                "win32-uninstall",
                SourceStatus::Partial,
                Some(format!(
                    "views read: {}; {}",
                    seen_views.join(", "),
                    notes.join("; ")
                )),
            )
        };
        ProviderOutcome { records, coverage }
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
