//! Platform-neutral application domain model (Phase 6).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable application identifier: content-derived from
/// (name, publisher) — deterministic for identical input.
///
/// ## The identity rule (explicit, deterministic)
///
/// A **logical application is identified by its normalized
/// (name, publisher) pair alone**. The discovery source is PROVENANCE,
/// never identity: the same application observed through several sources
/// (Win32 uninstall views, MSIX/AppX, filesystem presence) is ONE logical
/// application with unioned provenance — this is exactly what
/// [`crate::discovery::merge_inventory`] keys on, so the id, the merge
/// key, provenance, source coverage, footprint associations, and any
/// future persistence key all agree. Consequence: two records with the
/// same normalized (name, publisher) from different sources share one
/// [`ApplicationId`]; different names or publishers are different
/// applications. Two simultaneous DISTINCT installations with identical
/// name and publisher are represented as one logical application — the
/// indistinguishability is documented, not hidden.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(pub String);

impl ApplicationId {
    /// Derive a stable id from normalized name + publisher, so the same
    /// logical application yields the same id across runs AND across
    /// sources.
    pub fn derive(name: &str, publisher: Option<&str>) -> Self {
        use sha2::Digest;
        let key = format!(
            "{}|{}",
            name.trim().to_lowercase(),
            publisher.unwrap_or("").trim().to_lowercase(),
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
    /// Which sources were actually enumerated (honest coverage report),
    /// canonically ordered.
    pub sources: Vec<SourceCoverage>,
    /// Records dropped because a limit was hit (exact count).
    pub records_truncated: u64,
    /// Records rejected because a declared limit (name length) was
    /// violated — rejected rather than truncated, because a truncated
    /// name would derive a different [`ApplicationId`]. Exact count.
    #[serde(default)]
    pub records_rejected: u64,
}

/// How a discovery source fared. A source that could not be read is
/// NEVER reported as an empty success: every provider states one of
/// these five outcomes explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceStatus {
    /// Every declared view/scope of the source was enumerated.
    Complete,
    /// Some views/scopes were enumerated; at least one failed, was
    /// unavailable, or was truncated. The coverage note names them.
    Partial,
    /// The provider cannot run on this platform/build (e.g. MSIX on a
    /// non-Windows host). Never an empty success.
    Unsupported,
    /// The provider ran and the source failed.
    Failed,
    /// The source is absent in this machine state (e.g. a registry root
    /// that does not exist) — distinct from "enumerated and found none".
    Unavailable,
}

/// Which subsystem was enumerated and how it fared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCoverage {
    pub source: String,
    pub status: SourceStatus,
    /// Explicit detail when the status is not `Complete` — an
    /// implementation gap or a failed/unavailable view is reported,
    /// never silently empty.
    pub note: Option<String>,
}

impl SourceCoverage {
    pub fn complete(source: &str) -> Self {
        SourceCoverage {
            source: source.to_string(),
            status: SourceStatus::Complete,
            note: None,
        }
    }

    pub fn with_status(source: &str, status: SourceStatus, note: Option<String>) -> Self {
        SourceCoverage {
            source: source.to_string(),
            status,
            note,
        }
    }
}

/// Bounded work knobs for discovery. Every limit is explicit and
/// deterministic; overflow is counted exactly, never silently dropped.
#[derive(Debug, Clone)]
pub struct DiscoveryLimits {
    /// Maximum records/candidates a single merged result may publish.
    pub max_records: usize,
    /// Maximum accepted inventory-record name length. Longer names are
    /// rejected (not truncated — their id would change) and counted.
    pub max_inventory_name_len: usize,
    /// Maximum evidence items retained per footprint candidate. Overflow
    /// is counted exactly.
    pub max_evidence_per_candidate: usize,
    /// Maximum directory children examined per probed root. Children are
    /// canonically ordered before capping, so selection is deterministic.
    pub max_children_per_root: usize,
    /// Maximum applications probed for footprints per scan.
    pub max_apps_probed: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        DiscoveryLimits {
            max_records: 4096,
            max_inventory_name_len: 512,
            max_evidence_per_candidate: 16,
            max_children_per_root: 4096,
            max_apps_probed: 4096,
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
