//! Capability taxonomy: the seven product pillars, the typed capability
//! contracts, and the honest status of each (docs/MACOS_ARCHITECTURE.md).
//!
//! The status table is a truthfulness contract, not decoration: a status of
//! [`CapabilityStatus::Implemented`] claims the capability exists in this
//! build and is exercised by tests on every CI platform; anything less is
//! [`Partial`], [`Planned`], or [`Deferred`]. Tests pin the exact statuses
//! so an overclaim is a visible diff.

use serde::{Deserialize, Serialize};

/// The product pillars. CoreSight is a macOS system intelligence +
/// power-tools application; storage intelligence is one pillar, not the
/// whole product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Pillar {
    Storage,
    Applications,
    System,
    Privacy,
    Performance,
    SoftwareManagement,
    History,
}

impl Pillar {
    pub const ALL: [Pillar; 7] = [
        Pillar::Storage,
        Pillar::Applications,
        Pillar::System,
        Pillar::Privacy,
        Pillar::Performance,
        Pillar::SoftwareManagement,
        Pillar::History,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pillar::Storage => "Storage Intelligence",
            Pillar::Applications => "Application Intelligence",
            Pillar::System => "System Intelligence",
            Pillar::Privacy => "Privacy / Housekeeping",
            Pillar::Performance => "Performance / Diagnostics",
            Pillar::SoftwareManagement => "Software Management",
            Pillar::History => "History / Forensics",
        }
    }
}

/// Stable identifiers for the capability contracts. Each capability reports
/// through the honest-state envelopes ([`crate::Observation`],
/// [`crate::CapabilityReport`]); the concrete payload types live with their
/// domain crates and are bound to these contracts by adapters in later
/// phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CapabilityId {
    StorageAnalysis,
    ApplicationInventory,
    ApplicationFootprint,
    StartupItems,
    LaunchAgents,
    VolumeSystemInventory,
    Diagnostics,
    HistoricalObservations,
    PrivacyHousekeeping,
    SoftwareManagement,
}

impl CapabilityId {
    pub const ALL: [CapabilityId; 10] = [
        CapabilityId::StorageAnalysis,
        CapabilityId::ApplicationInventory,
        CapabilityId::ApplicationFootprint,
        CapabilityId::StartupItems,
        CapabilityId::LaunchAgents,
        CapabilityId::VolumeSystemInventory,
        CapabilityId::Diagnostics,
        CapabilityId::HistoricalObservations,
        CapabilityId::PrivacyHousekeeping,
        CapabilityId::SoftwareManagement,
    ];

    /// Stable machine-readable id (kebab-case), fixed for IPC use.
    pub fn as_str(self) -> &'static str {
        match self {
            CapabilityId::StorageAnalysis => "storage-analysis",
            CapabilityId::ApplicationInventory => "application-inventory",
            CapabilityId::ApplicationFootprint => "application-footprint",
            CapabilityId::StartupItems => "startup-items",
            CapabilityId::LaunchAgents => "launch-agents",
            CapabilityId::VolumeSystemInventory => "volume-system-inventory",
            CapabilityId::Diagnostics => "diagnostics",
            CapabilityId::HistoricalObservations => "historical-observations",
            CapabilityId::PrivacyHousekeeping => "privacy-housekeeping",
            CapabilityId::SoftwareManagement => "software-management",
        }
    }

    pub fn pillar(self) -> Pillar {
        match self {
            CapabilityId::StorageAnalysis => Pillar::Storage,
            CapabilityId::ApplicationInventory | CapabilityId::ApplicationFootprint => {
                Pillar::Applications
            }
            CapabilityId::StartupItems | CapabilityId::LaunchAgents => Pillar::System,
            CapabilityId::VolumeSystemInventory => Pillar::System,
            CapabilityId::Diagnostics => Pillar::Performance,
            CapabilityId::HistoricalObservations => Pillar::History,
            CapabilityId::PrivacyHousekeeping => Pillar::Privacy,
            CapabilityId::SoftwareManagement => Pillar::SoftwareManagement,
        }
    }
}

/// How much of a capability exists in this build. Definitions:
///
/// - `Implemented` — end to end in this build, exercised by tests on every
///   CI platform.
/// - `Partial` — real implementation exists but is platform-limited or has
///   known gaps with honest contracts.
/// - `Planned` — typed contract only; no provider yet.
/// - `Deferred` — intentionally not implemented until a separately
///   authorized phase (safety-sensitive; docs/SECURITY_AND_SAFETY.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CapabilityStatus {
    Implemented,
    Partial,
    Planned,
    Deferred,
}

/// One capability's contract and its honest status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityContract {
    pub id: CapabilityId,
    pub pillar: Pillar,
    pub status: CapabilityStatus,
    pub note: &'static str,
}

/// The capability registry. Notes name platforms honestly — never overclaim.
pub const CONTRACTS: [CapabilityContract; 10] = [
    CapabilityContract {
        id: CapabilityId::StorageAnalysis,
        pillar: Pillar::Storage,
        status: CapabilityStatus::Partial,
        note: "Scanner, classifier, duplicate identity and history are implemented and CI-tested \
               on Windows/Linux/macOS; APFS-specific facts (clones, purgeable space, snapshots) \
               are pending.",
    },
    CapabilityContract {
        id: CapabilityId::ApplicationInventory,
        pillar: Pillar::Applications,
        status: CapabilityStatus::Partial,
        note: "Windows Win32-uninstall providers implemented and CI-tested; macOS bundle \
               (Info.plist) and Linux .desktop filesystem-native providers implemented \
               (Phase 6.2) but runtime-validated only on their own OS. MSIX/AppX and distro \
               package databases are reported Unsupported, never as empty inventories.",
    },
    CapabilityContract {
        id: CapabilityId::ApplicationFootprint,
        pillar: Pillar::Applications,
        status: CapabilityStatus::Partial,
        note: "Cross-platform footprint discovery, install-root detection, evidence-grouped \
               ownership with a documented correlation ceiling, shared/conflict detection and \
               inert read-only candidates are implemented and CI-tested (Phase 6.2). Real-world \
               platform runtime validation beyond synthetic fixtures is still pending.",
    },
    CapabilityContract {
        id: CapabilityId::StartupItems,
        pillar: Pillar::System,
        status: CapabilityStatus::Planned,
        note: "Typed contract only ([`crate::StartupItem`]); no discovery provider yet. macOS \
               login-item mechanisms need an OS API surface and are deferred in the catalog.",
    },
    CapabilityContract {
        id: CapabilityId::LaunchAgents,
        pillar: Pillar::System,
        status: CapabilityStatus::Planned,
        note: "Typed contract only; the macOS source catalog classifies the launchd directories \
               (user/system agents and daemons) for a later provider.",
    },
    CapabilityContract {
        id: CapabilityId::VolumeSystemInventory,
        pillar: Pillar::System,
        status: CapabilityStatus::Partial,
        note: "Volume listing implemented (Windows full; Linux /proc/mounts; macOS root-only \
               limitation); APFS container/snapshot facts deferred.",
    },
    CapabilityContract {
        id: CapabilityId::Diagnostics,
        pillar: Pillar::Performance,
        status: CapabilityStatus::Planned,
        note: "Typed contract only ([`crate::DiagnosticSignal`]); no signal producer yet.",
    },
    CapabilityContract {
        id: CapabilityId::HistoricalObservations,
        pillar: Pillar::History,
        status: CapabilityStatus::Implemented,
        note: "coresight-history implemented and audited (Phase 5/5.1): persisted runs, typed \
               change events, strict corruption rejection.",
    },
    CapabilityContract {
        id: CapabilityId::PrivacyHousekeeping,
        pillar: Pillar::Privacy,
        status: CapabilityStatus::Deferred,
        note: "Observation contracts only; ANY action against privacy-sensitive data requires a \
               separately authorized phase and the safety gate (docs/SECURITY_AND_SAFETY.md).",
    },
    CapabilityContract {
        id: CapabilityId::SoftwareManagement,
        pillar: Pillar::SoftwareManagement,
        status: CapabilityStatus::Planned,
        note: "Inert ownership/relationship evidence and read-only recommendation candidates \
               exist (coresight-apps, Phase 6.2); no uninstall, cleanup, or startup-disabling \
               execution exists anywhere in this build, and no executor API is planned here.",
    },
];

/// Look up one capability's contract.
pub fn contract(id: CapabilityId) -> &'static CapabilityContract {
    CONTRACTS
        .iter()
        .find(|c| c.id == id)
        .expect("every CapabilityId is registered in CONTRACTS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_ids_are_unique_and_stable() {
        let mut ids: Vec<&str> = CapabilityId::ALL.iter().map(|c| c.as_str()).collect();
        ids.sort();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "kebab-case ids must be unique");
        for id in ids {
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "ids are kebab-case: {id}"
            );
        }
    }

    #[test]
    fn every_pillar_has_at_least_one_contract() {
        for pillar in Pillar::ALL {
            assert!(
                CapabilityId::ALL.iter().any(|c| c.pillar() == pillar),
                "pillar {} has no contract",
                pillar.title()
            );
        }
    }

    #[test]
    fn pillar_metadata_is_consistent() {
        for id in CapabilityId::ALL {
            let c = contract(id);
            assert_eq!(c.id, id);
            assert_eq!(c.pillar, id.pillar());
            assert!(!c.note.is_empty(), "every contract carries an honest note");
        }
    }

    #[test]
    fn statuses_are_pinned_honest() {
        // Pinned so that overclaiming is a visible, deliberate diff.
        let expected = [
            (CapabilityId::StorageAnalysis, CapabilityStatus::Partial),
            (
                CapabilityId::ApplicationInventory,
                CapabilityStatus::Partial,
            ),
            (
                CapabilityId::ApplicationFootprint,
                CapabilityStatus::Partial,
            ),
            (CapabilityId::StartupItems, CapabilityStatus::Planned),
            (CapabilityId::LaunchAgents, CapabilityStatus::Planned),
            (
                CapabilityId::VolumeSystemInventory,
                CapabilityStatus::Partial,
            ),
            (CapabilityId::Diagnostics, CapabilityStatus::Planned),
            (
                CapabilityId::HistoricalObservations,
                CapabilityStatus::Implemented,
            ),
            (
                CapabilityId::PrivacyHousekeeping,
                CapabilityStatus::Deferred,
            ),
            (CapabilityId::SoftwareManagement, CapabilityStatus::Planned),
        ];
        for (id, status) in expected {
            assert_eq!(contract(id).status, status, "status drifted for {id:?}");
        }
    }

    #[test]
    fn destructive_adjacent_contracts_are_never_implemented() {
        // The deferred/planned contracts include every capability whose
        // full realization would involve state-changing actions.
        for id in [
            CapabilityId::PrivacyHousekeeping,
            CapabilityId::SoftwareManagement,
        ] {
            assert_ne!(
                contract(id).status,
                CapabilityStatus::Implemented,
                "{id:?} must not claim implementation"
            );
        }
    }
}
