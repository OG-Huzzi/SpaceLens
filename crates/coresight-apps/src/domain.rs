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
///
/// ## The identity encoding (collision-free by construction)
///
/// The id hashes a **length-prefixed** encoding of the normalized pair:
///
/// ```text
/// <len(name) as fixed 8-byte big-endian> <name bytes>
/// <len(publisher) as fixed 8-byte big-endian> <publisher bytes>
/// ```
///
/// A length prefix is self-delimiting, so the encoding is injective:
/// no name/publisher content can forge a component boundary. The
/// Phase 6.4 encoding (`"{name}|{publisher}"`) was ambiguous —
/// `("A|B", "C")` and `("A", "B|C")` hashed identically, so two genuinely
/// different logical applications shared one id. That is repaired here
/// (Workstream A, Phase 6.4.1); see [`ApplicationId::COMPAT_NOTE`].
///
/// Normalization is unchanged and defined by [`normalized_pair`]:
/// name trimmed and lowercased, publisher `None` treated as the empty
/// string, trimmed and lowercased. Because normalization collapses all
/// whitespace-trimmed and case-folded spellings to one form, identity is
/// deliberately insensitive to case and surrounding whitespace, and
/// sensitive to *interior* characters. Unicode is hashed as its UTF-8
/// bytes (no Unicode normalization is applied — that would be a second,
/// hidden rule, so NFC and NFD spellings stay distinct by design).
///
/// ## Compatibility
///
/// Changing the derivation changes every derived id, so ids persisted by
/// the Phase 6.4 (schema v5) build would no longer verify against their
/// stored name/publisher. Persisted rows are therefore **re-keyed, never
/// globally replaced**: the `v5 → v6` migration recomputes each row's id
/// from its OWN stored facts (one fact at a time), so a single legacy id
/// that represented several distinct pairs splits back into distinct ids
/// and every child row follows its parent. See
/// [`ApplicationId::COMPAT_NOTE`] for the full rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(pub String);

impl ApplicationId {
    /// Version tag of the identity encoding. `1` was the Phase 6.4
    /// delimiter-joined encoding (ambiguous at component boundaries);
    /// `2` is the collision-free length-prefixed encoding. The tag is
    /// recorded in [`ConfigFingerprint::app_snapshot_schema`]'s sibling
    /// provenance and used by the migration to decide which rows need
    /// re-keying, so old and new ids are never silently mixed.
    pub const ID_ENCODING_VERSION: u32 = 2;

    /// The legacy (Phase 6.4) encoding: `"{name}|{publisher}"`. Retained
    /// ONLY so the migration can recognize and re-key legacy ids;
    /// never used to derive a new id.
    #[doc(hidden)]
    pub fn legacy_derivation_key(name: &str, publisher: Option<&str>) -> String {
        format!(
            "{}|{}",
            name.trim().to_lowercase(),
            publisher.unwrap_or("").trim().to_lowercase(),
        )
    }

    /// Human/tool-readable summary of the compatibility rule, so callers
    /// and the migration agree without reading the implementation.
    pub const COMPAT_NOTE: &'static str = "\
Application identity is the SHA-256 of the length-prefixed normalized \
(name, publisher) pair (encoding version 2). Encoding version 1 joined the \
same pair with a '|' delimiter, which is ambiguous: ('A|B','C') and \
('A','B|C') produced the same id. Ids persisted under version 1 are \
re-keyed per stored fact by the v5→v6 migration — never globally replaced \
— so a legacy id that covered several distinct pairs splits correctly and \
its child rows follow it. Deriving or reading an id never consults the \
discovery source: source is provenance, never identity.";

    /// Derive a stable id from normalized name + publisher, so the same
    /// logical application yields the same id across runs AND across
    /// sources.
    ///
    /// See the type docs for the encoding and the compatibility rule.
    pub fn derive(name: &str, publisher: Option<&str>) -> Self {
        use sha2::Digest;
        let (name, publisher) = normalized_pair(name, publisher);
        let mut input = Vec::new();
        push_component(&mut input, &name);
        push_component(&mut input, &publisher);
        let digest = sha2::Sha256::digest(&input);
        ApplicationId(format!("app-{}", hex8(&digest[..])))
    }
}

/// The canonical normalized identity pair: `(name, publisher)` both
/// trimmed and lowercased, with a missing publisher represented as the
/// empty string. This is the ONE normalization rule; both
/// [`ApplicationId::derive`] and [`crate::discovery::merge_inventory`]'s
/// merge key are defined by it, so the id and the merge key can never
/// disagree.
pub(crate) fn normalized_pair(name: &str, publisher: Option<&str>) -> (String, String) {
    (
        name.trim().to_lowercase(),
        publisher.unwrap_or("").trim().to_lowercase(),
    )
}

/// Append one self-delimiting component: its byte length as a fixed
/// 8-byte big-endian prefix, then the bytes. Because the prefix states
/// exactly how many bytes belong to this component, no interior byte
/// sequence can be mistaken for a boundary — the pair encoding is
/// injective for all inputs, including embedded separators, NUL bytes,
/// and non-UTF-8-compatible scalars.
fn push_component(out: &mut Vec<u8>, value: &str) {
    let len = value.len() as u64;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
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
    /// macOS application bundle (*.app/Contents/Info.plist). Local file
    /// metadata only.
    BundleInfoPlist,
    /// Freedesktop .desktop entry (Linux/BSD). Local file metadata only.
    DesktopEntry,
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
    /// Bundle / package identifier declared by the application's own
    /// metadata (macOS CFBundleIdentifier, MSIX package family name).
    /// None = the source declares none.
    #[serde(default)]
    pub bundle_identifier: Option<String>,
    /// Executable path EXACTLY as the source metadata recorded it
    /// (e.g. Win32 DisplayIcon, .desktop absolute Exec, bundle
    /// CFBundleExecutable resolved inside the bundle). Lossless; never
    /// verified to exist by the record itself.
    #[serde(default)]
    pub executable_path: Option<PathBuf>,
    /// Every source this logical application was observed through
    /// (provenance, canonically ordered and deduplicated by
    /// [crate::discovery::merge_inventory]). Never part of identity.
    #[serde(default)]
    pub provenance: Vec<ApplicationSource>,
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
    /// Maximum directories listed in one intelligence observation
    /// (Phase 6.2). Exceeding it is counted, never silent.
    pub max_directories: usize,
    /// Maximum directory entries visited across one intelligence
    /// observation (Phase 6.2).
    pub max_entries: usize,
    /// Maximum directory depth below a probed root (Phase 6.2).
    pub max_depth: usize,
    /// Maximum bytes of file metadata (plist, desktop entry, ...) read in
    /// one observation (Phase 6.2).
    pub max_metadata_bytes: u64,
    /// Maximum candidate install roots retained per application.
    pub max_roots_per_app: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        DiscoveryLimits {
            max_records: 4096,
            max_inventory_name_len: 512,
            max_evidence_per_candidate: 16,
            max_children_per_root: 4096,
            max_apps_probed: 4096,
            max_directories: 4096,
            max_entries: 262_144,
            max_depth: 3,
            max_metadata_bytes: 8 * 1024 * 1024,
            max_roots_per_app: 8,
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
