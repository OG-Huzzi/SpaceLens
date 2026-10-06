//! CoreSight application intelligence — Phase 6.
//!
//! Turns low-level engines into a unified understanding of installed
//! software: platform-neutral application domain, provider-based
//! discovery (Windows Win32 uninstall registry first; MSIX/AppX
//! abstracted), evidence-backed footprint candidates, honest
//! ownership relationships, and human-readable explanations.
//!
//! Privacy: all intelligence is local. No telemetry, no network,
//! no AI substitution for evidence.

pub mod discovery;
pub mod domain;
pub mod evidence;
pub mod explain;
pub mod footprint;
pub mod relationships;
pub mod windows_discovery;

#[cfg(windows)]
pub mod win32_registry;

#[cfg(not(windows))]
pub mod win32_registry {
    //! Non-Windows stub: no production registry.
    use crate::discovery::{ApplicationProvider, ProviderError};
    use crate::domain::ApplicationRecord;
    use crate::windows_discovery::{RegistryValue, RegistryView};

    pub struct Win32RegistryView;

    impl Win32RegistryView {
        pub fn new() -> Self {
            Win32RegistryView
        }
    }

    impl Default for Win32RegistryView {
        fn default() -> Self {
            Self::new()
        }
    }

    impl RegistryView for Win32RegistryView {
        fn subkeys(&self, _key: &str) -> Vec<String> {
            Vec::new()
        }
        fn get_value(&self, _key: &str, _name: &str) -> Option<RegistryValue> {
            None
        }
    }

    impl ApplicationProvider for Win32RegistryView {
        fn source_tag(&self) -> &'static str {
            "win32-uninstall"
        }
        fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError> {
            Err(ProviderError::Unsupported(
                "Win32 registry enumeration requires Windows".to_string(),
            ))
        }
    }
}

pub use discovery::{
    merge_inventory, ApplicationProvider, PackagedAppProvider, ProviderError, ProviderOutcome,
};
pub use domain::{
    ApplicationId, ApplicationRecord, ApplicationSource, DiscoveryLimits, Inventory, PackageKind,
    SourceCoverage, SourceStatus,
};
pub use evidence::{AssociationScope, Confidence, EvidenceKind, FootprintEvidence};
pub use explain::{explain, Explanation};
pub use footprint::{
    discover_footprints, normalize_name, FootprintCandidate, FootprintKind, FootprintReport,
    KnownRoots, PathProber,
};
pub use relationships::{AppAssociation, AssociationKind, OwnershipStrength};
pub use windows_discovery::{
    decode_registry_string, split_hive_path, RegistryHive, RegistryValue, RegistryView,
    SubkeyEnumeration, UninstallView, Win32UninstallEnumerator, WindowsAppxProvider,
    DEFAULT_MAX_SUBKEYS_PER_VIEW,
};

pub use win32_registry::Win32RegistryView;
