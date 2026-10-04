//! Platform-neutral application domain model (Phase 6).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable application identifier: content-derived from
/// (name, publisher, source) — deterministic for identical input.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(pub String);

impl ApplicationId {
    /// Derive a stable id from normalized name + publisher + source tag,
    /// so the same logical application yields the same id across runs.
    pub fn derive(name: &str, publisher: Option<&str>, source: &str) -> Self {
        use sha2::Digest;
        let key = format!(
            "{}|{}|{}",
            name.trim().to_lowercase(),
            publisher.unwrap_or("").trim().to_lowercase(),
            source
        );
        let digest = sha2::Sha256::digest(key.as_bytes());
        ApplicationId(format!("app-{}", hex8(&digest[..])))
    }
}

fn hex8(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What kind of software presence this record describes. Platform
/// inventory must distinguish them so a shared runtime is never
/// mistaken for a product the user installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PackageKind {
    /// A user-installed product.
    Installed,
    /// A self-contained portable presence (no installer record).
    Portable,
    /// An OS component that may be an optional feature or driver.
    SystemComponent,
    /// A dependency/runtime shared by products.
    SharedRuntime,
    /// Another product's bundled component that should not be
    /// treated as independently removable.
    DependentComponent,
    Unknown,
}

/// Where the inventory record came from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApplicationSource {
    /// Win32 uninstall registry key, one of the enumerated views.
    RegistryUninstall,
    /// Modern packaged app (MSIX/AppX): abstracted; Windows
    /// enumeration is provided by [`crate::discovery::PackagedAppProvider`].
    PackagedApp,
    /// Detected only by filesystem presence (portable app) — recorded
    /// only when the caller supplies filesystem evidence.
    FilesystemPresence,
}

/// One inventory record: what the machine says about an application.
/// Every field honest about unknowns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRecord {
    pub id: ApplicationId,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub install_location: Option<PathBuf>,
    pub install_date: Option<String>,
    pub estimated_size_bytes: Option<u64>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub modify_path: Option<String>,
    pub install_source: Option<String>,
    pub source: ApplicationSource,
    pub kind: PackageKind,
    /// True only when the source explicitly flags OS-shipped software
    /// (e.g. `SystemComponent=1` under the uninstall key).
    pub system_component: bool,
    /// All raw registry views this record appeared in (provenance).
    pub observed_in_views: Vec<String>,
}

/// Normalized, deduplicated inventory result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// Canonically ordered by (name, publisher, id) — deterministic.
    pub records: Vec<ApplicationRecord>,
    /// Which sources were actually enumerated (honest coverage report).
    pub sources: Vec<SourceCoverage>,
    /// Records dropped because a limit was hit (exact count).
    pub records_truncated: u64,
}

/// Which subsystem was enumerated and how it fared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCoverage {
    pub source: String,
    pub enumerated: bool,
    /// Explicit note when enumeration failed or was not attempted —
    /// an implementation gap is reported, never silently empty.
    pub note: Option<String>,
}

/// Bounded work knobs for discovery.
#[derive(Debug, Clone)]
pub struct DiscoveryLimits {
    pub max_records: usize,
    pub max_inventory_name_len: usize,
    pub max_evidence_per_candidate: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        DiscoveryLimits {
            max_records: 4096,
            max_inventory_name_len: 512,
            max_evidence_per_candidate: 16,
        }
    }
}

/// Human-readable application version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationVersion(pub String);

/// Normalized publisher string (trimmed; preserves original case).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Publisher(pub String);
