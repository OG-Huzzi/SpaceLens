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

pub mod analysis;
pub mod bounded;
pub mod discovery;
pub mod domain;
pub mod evidence;
pub mod explain;
pub mod footprint;
pub mod observe;
pub mod ownership;
pub mod pathmatch;
pub mod relationships;
pub mod roots;
pub mod sources;
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
        fn subkeys_bounded(
            &self,
            _key: &str,
            _max: usize,
        ) -> crate::windows_discovery::SubkeyEnumeration {
            crate::windows_discovery::SubkeyEnumeration::default()
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

pub use analysis::{
    analyze, assess, can_authorize_execution, executable_evidence, AnalysisTruncation,
    ApplicationAnalysis, ApplicationRelationship, ArtifactClaimant, ArtifactOwnership,
    CandidateBlocker, CandidateKind, ObservedArtifact, OwnershipCandidate, RelationKind,
    SharedStatus,
};
pub use bounded::{Admission, BoundedTopK};
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
    discover_footprints, normalize_name, offer_path, BoundedListing, FootprintCandidate,
    FootprintKind, FootprintReport, KnownRoots, PathProber,
};
pub use observe::{
    DirectoryObservation, FileObservation, ListedEntry, PathKey, PathObservation,
    PlatformPathProber, ProbedKind,
};
pub use ownership::{
    assess_groups, CorrelationGroup, EvidenceAccumulator, EvidenceSource, EvidenceStrength,
    MatchedAttribute, OwnershipAssessment, OwnershipEvidence,
};
pub use pathmatch::{
    ascii_eq_ignore_case, extension_is_ascii, file_name_is_ascii, file_name_str, file_stem_str,
};
pub use relationships::{AppAssociation, AssociationKind, OwnershipStrength};
pub use roots::{
    associate_executable, bundle_root_of, containing_root, detect_install_roots, path_within,
    ExecutableAssociation, ExecutableStatus, InstallRoot, ProgramRoots, RootDetectionCounts,
    RootSignal,
};
pub use sources::{
    parse_desktop_entry, parse_info_plist, record_from_bundle, record_from_desktop_entry,
    BundleMetadata, BundlePlistProvider, DesktopEntryMetadata, DesktopEntryProvider,
};
pub use windows_discovery::{
    decode_registry_string, offer_name, split_hive_path, RegistryHive, RegistryValue, RegistryView,
    SubkeyEnumeration, UninstallView, Win32UninstallEnumerator, WindowsAppxProvider,
    DEFAULT_MAX_SUBKEYS_PER_VIEW,
};

pub use win32_registry::Win32RegistryView;

#[cfg(test)]
mod architecture_guard_tests {
    use std::fs;
    use std::path::Path;

    /// The Phase 6.2 intelligence layer is SHARED code: it must contain no
    /// platform conditionals (OS behavior belongs in the cfg-selected
    /// modules), no subprocess/network capability, and no lossy path
    /// decoding in semantic logic.
    ///
    /// The needles are assembled at runtime so this test's own source does
    /// not match them. Comment lines are stripped before matching so prose
    /// about a forbidden pattern does not trip the guard.
    #[test]
    fn intelligence_layer_is_platform_neutral_and_inert() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let shared = [
            "analysis.rs",
            "bounded.rs",
            "footprint.rs",
            "observe.rs",
            "ownership.rs",
            "pathmatch.rs",
            "roots.rs",
            "sources.rs",
        ];
        let mut forbidden: Vec<String> = Vec::new();
        for word in ["target_os", "windows", "unix"] {
            forbidden.push(format!("cfg!( {word}").replace(' ', ""));
            forbidden.push(format!("#[cfg( {word}").replace(' ', ""));
        }
        for needle in [
            "std::process",
            "std::net",
            "Command::new",
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "to_str().unwrap_or_default",
            "to_str().unwrap()",
        ] {
            forbidden.push(needle.to_string());
        }
        // Assembled so this source never contains them contiguously.
        forbidden.push(["to_string", "_lossy"].concat());
        forbidden.push(["from_utf8", "_lossy"].concat());
        forbidden.push(["from_utf16", "_lossy"].concat());
        forbidden.push(["std::fs", "::remove_file"].concat());
        forbidden.push(["std::fs", "::remove_dir"].concat());
        forbidden.push(["Open", "Options"].concat());

        for file in shared {
            let path = src.join(file);
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()));
            // The guard test itself exempts lines inside `#[cfg(test)]`
            // blocks by construction: the needles are runtime-assembled.
            let code: Vec<&str> = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect();
            for needle in &forbidden {
                assert!(
                    !code.iter().any(|l| l.contains(needle.as_str())),
                    "{needle:?} found in {}",
                    path.display()
                );
            }
        }
    }
}
