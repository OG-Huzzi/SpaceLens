//! The macOS discovery source catalog (Phase 6.1, Task 4).
//!
//! The potential sources are classified — never blindly scanned — along
//! four explicit dimensions, covering exactly the buckets Phase 6.1 asks
//! for: safe to read now / requires special permissions / potentially
//! sensitive / unsupported for now / destructive if modified / deferred to
//! later phases.
//!
//! This is a POLICY TABLE: pure data, deterministic, testable on every
//! platform. No entry in this table is probed for content in this phase —
//! `Probed` sources get a bounded directory-listing observation only.

use serde::{Deserialize, Serialize};

use coresight_capabilities::capability::CapabilityId;

/// The macOS discovery sources this architecture recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MacSourceId {
    ApplicationsDir,
    UserApplications,
    UserAppSupport,
    UserCaches,
    UserLogs,
    UserContainers,
    UserGroupContainers,
    UserPreferences,
    UserLaunchAgents,
    SystemLaunchAgents,
    SystemLaunchDaemons,
    LoginItems,
    TccProtectedUserData,
    MountedVolumes,
    ApfsVolumeInfo,
}

impl MacSourceId {
    pub const ALL: [MacSourceId; 15] = [
        MacSourceId::ApplicationsDir,
        MacSourceId::UserApplications,
        MacSourceId::UserAppSupport,
        MacSourceId::UserCaches,
        MacSourceId::UserLogs,
        MacSourceId::UserContainers,
        MacSourceId::UserGroupContainers,
        MacSourceId::UserPreferences,
        MacSourceId::UserLaunchAgents,
        MacSourceId::SystemLaunchAgents,
        MacSourceId::SystemLaunchDaemons,
        MacSourceId::LoginItems,
        MacSourceId::TccProtectedUserData,
        MacSourceId::MountedVolumes,
        MacSourceId::ApfsVolumeInfo,
    ];

    /// Stable machine-readable id (kebab-case).
    pub fn as_str(self) -> &'static str {
        match self {
            MacSourceId::ApplicationsDir => "applications-dir",
            MacSourceId::UserApplications => "user-applications",
            MacSourceId::UserAppSupport => "user-app-support",
            MacSourceId::UserCaches => "user-caches",
            MacSourceId::UserLogs => "user-logs",
            MacSourceId::UserContainers => "user-containers",
            MacSourceId::UserGroupContainers => "user-group-containers",
            MacSourceId::UserPreferences => "user-preferences",
            MacSourceId::UserLaunchAgents => "user-launch-agents",
            MacSourceId::SystemLaunchAgents => "system-launch-agents",
            MacSourceId::SystemLaunchDaemons => "system-launch-daemons",
            MacSourceId::LoginItems => "login-items",
            MacSourceId::TccProtectedUserData => "tcc-protected-user-data",
            MacSourceId::MountedVolumes => "mounted-volumes",
            MacSourceId::ApfsVolumeInfo => "apfs-volume-info",
        }
    }

    /// The capability contracts this source can feed.
    pub fn related_capabilities(self) -> &'static [CapabilityId] {
        match self {
            MacSourceId::ApplicationsDir | MacSourceId::UserApplications => &[
                CapabilityId::ApplicationInventory,
                CapabilityId::SoftwareManagement,
                CapabilityId::StorageAnalysis,
            ],
            MacSourceId::UserAppSupport => &[
                CapabilityId::ApplicationFootprint,
                CapabilityId::StorageAnalysis,
            ],
            MacSourceId::UserCaches => &[
                CapabilityId::StorageAnalysis,
                CapabilityId::ApplicationFootprint,
                CapabilityId::PrivacyHousekeeping,
            ],
            MacSourceId::UserLogs => &[
                CapabilityId::StorageAnalysis,
                CapabilityId::PrivacyHousekeeping,
            ],
            MacSourceId::UserContainers | MacSourceId::UserGroupContainers => &[
                CapabilityId::ApplicationFootprint,
                CapabilityId::PrivacyHousekeeping,
                CapabilityId::StorageAnalysis,
            ],
            MacSourceId::UserPreferences => &[
                CapabilityId::ApplicationFootprint,
                CapabilityId::PrivacyHousekeeping,
            ],
            MacSourceId::UserLaunchAgents
            | MacSourceId::SystemLaunchAgents
            | MacSourceId::SystemLaunchDaemons => {
                &[CapabilityId::LaunchAgents, CapabilityId::StartupItems]
            }
            MacSourceId::LoginItems => &[CapabilityId::StartupItems],
            MacSourceId::TccProtectedUserData => &[CapabilityId::PrivacyHousekeeping],
            MacSourceId::MountedVolumes => &[
                CapabilityId::VolumeSystemInventory,
                CapabilityId::StorageAnalysis,
            ],
            MacSourceId::ApfsVolumeInfo => &[
                CapabilityId::VolumeSystemInventory,
                CapabilityId::StorageAnalysis,
            ],
        }
    }
}

/// Where a source lives. Home-relative sources carry the human `~/` form;
/// resolution against a home directory happens in `observation::resolve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceLocation {
    Absolute {
        path: &'static str,
    },
    HomeRelative {
        relative: &'static str,
    },
    /// An OS mechanism with no single filesystem path.
    Mechanism {
        description: &'static str,
    },
}

/// Whether and how this phase's architecture permits reading the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceAccess {
    /// Plain, permission-free reads. In this phase that means a bounded
    /// directory listing only — never content reads.
    ReadableNow,
    /// Reading requires a macOS privacy grant (TCC Full Disk Access) that
    /// CoreSight must never assume, request implicitly, or bypass.
    RequiresFullDiskAccess,
    /// No supported read path in this build (no std API surface; no
    /// subprocesses are allowed).
    UnsupportedForNow,
}

/// How much private information the source's names/metadata expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Sensitivity {
    Public,
    Sensitive,
    PrivacySensitive,
}

/// What MODIFYING the source would risk. Classification for FUTURE phases:
/// this build performs no modifications at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ModificationRisk {
    /// Modification is not part of any current capability.
    ReadOnlyOnly,
    /// Modifying can lose state that is usually recoverable.
    RecoverableLoss,
    /// Modifying can destroy data the user cannot get back.
    DestructiveLoss,
    /// Destructive AND requires administrator rights to modify.
    PrivilegedDestructiveLoss,
}

/// Whether a source is observed in this phase or intentionally deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceAvailability {
    /// A bounded, read-only listing observation is active.
    Probed,
    /// Recognized but intentionally not probed in this phase.
    Deferred,
}

/// One macOS discovery source and its safety classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacSourceSpec {
    pub id: MacSourceId,
    pub location: SourceLocation,
    pub access: SourceAccess,
    pub sensitivity: Sensitivity,
    pub if_modified: ModificationRisk,
    pub availability: SourceAvailability,
    pub note: &'static str,
}

/// The catalog, in canonical order. Deterministic: the same build always
/// returns the same sequence.
pub const SOURCES: [MacSourceSpec; 15] = [
    MacSourceSpec {
        id: MacSourceId::ApplicationsDir,
        location: SourceLocation::Absolute {
            path: "/Applications",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Public,
        if_modified: ModificationRisk::PrivilegedDestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "System-wide application bundles; removal belongs to a future uninstall \
               capability behind the safety gate, never to a plain delete.",
    },
    MacSourceSpec {
        id: MacSourceId::UserApplications,
        location: SourceLocation::HomeRelative {
            relative: "~/Applications",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Public,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "User-scoped applications; often absent — absence is reported as DOES NOT EXIST, \
               never filled in.",
    },
    MacSourceSpec {
        id: MacSourceId::UserAppSupport,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/Application Support",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "Application data; deletion can lose user state (profiles, in-app documents).",
    },
    MacSourceSpec {
        id: MacSourceId::UserCaches,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/Caches",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::RecoverableLoss,
        availability: SourceAvailability::Probed,
        note: "Regenerable caches; cleanup remains safety-gated and is NOT implemented in this \
               phase.",
    },
    MacSourceSpec {
        id: MacSourceId::UserLogs,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/Logs",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::RecoverableLoss,
        availability: SourceAvailability::Probed,
        note: "Application logs; may contain private paths and activity traces.",
    },
    MacSourceSpec {
        id: MacSourceId::UserContainers,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/Containers",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::PrivacySensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "Sandboxed app containers; container names reveal installed apps and contents may \
               hold private app data.",
    },
    MacSourceSpec {
        id: MacSourceId::UserGroupContainers,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/Group Containers",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::PrivacySensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "Shared sandbox groups spanning one team's apps.",
    },
    MacSourceSpec {
        id: MacSourceId::UserPreferences,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/Preferences",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "Preference plists; contents are user configuration — only names are listed, \
               never contents.",
    },
    MacSourceSpec {
        id: MacSourceId::UserLaunchAgents,
        location: SourceLocation::HomeRelative {
            relative: "~/Library/LaunchAgents",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "User launch agents; disabling one is a startup-behavior change reserved for a \
               later authorized phase.",
    },
    MacSourceSpec {
        id: MacSourceId::SystemLaunchAgents,
        location: SourceLocation::Absolute {
            path: "/Library/LaunchAgents",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::PrivilegedDestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "Machine-wide launch agents; writing requires administrator rights.",
    },
    MacSourceSpec {
        id: MacSourceId::SystemLaunchDaemons,
        location: SourceLocation::Absolute {
            path: "/Library/LaunchDaemons",
        },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::PrivilegedDestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "System daemons; modifying affects all users and requires administrator rights.",
    },
    MacSourceSpec {
        id: MacSourceId::LoginItems,
        location: SourceLocation::Mechanism {
            description: "Background Task Management / login-item records",
        },
        access: SourceAccess::UnsupportedForNow,
        sensitivity: Sensitivity::Sensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Deferred,
        note: "Login items need an OS API surface (SMAppService/BTM records); no std-reachable \
               read and no subprocesses allowed — deferred to a later authorized phase.",
    },
    MacSourceSpec {
        id: MacSourceId::TccProtectedUserData,
        location: SourceLocation::Mechanism {
            description: "TCC-protected user data (Mail, Messages, Safari, Photos, ...)",
        },
        access: SourceAccess::RequiresFullDiskAccess,
        sensitivity: Sensitivity::PrivacySensitive,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Deferred,
        note: "Never probed without an explicit, user-granted Full Disk Access consent; this \
               build never requests or bypasses it.",
    },
    MacSourceSpec {
        id: MacSourceId::MountedVolumes,
        location: SourceLocation::Absolute { path: "/Volumes" },
        access: SourceAccess::ReadableNow,
        sensitivity: Sensitivity::Public,
        if_modified: ModificationRisk::DestructiveLoss,
        availability: SourceAvailability::Probed,
        note: "Mount points; each volume is attributed to itself, never double-counted into the \
               parent.",
    },
    MacSourceSpec {
        id: MacSourceId::ApfsVolumeInfo,
        location: SourceLocation::Mechanism {
            description: "APFS container/volume/snapshot metadata",
        },
        access: SourceAccess::UnsupportedForNow,
        sensitivity: Sensitivity::Public,
        if_modified: ModificationRisk::PrivilegedDestructiveLoss,
        availability: SourceAvailability::Deferred,
        note: "Needs APFS-aware APIs (no std surface; no subprocesses allowed). Time Machine \
               snapshots must never be touched.",
    },
];

/// Look up one source's spec.
pub fn source(id: MacSourceId) -> &'static MacSourceSpec {
    SOURCES
        .iter()
        .find(|s| s.id == id)
        .expect("every MacSourceId is registered in SOURCES")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_canonical_and_unique() {
        let mut ids: Vec<&str> = MacSourceId::ALL.iter().map(|s| s.as_str()).collect();
        let unique = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), unique, "source ids must be unique");
        for id in ids {
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "ids are kebab-case: {id}"
            );
        }
        // The canonical order is pinned so consumers can rely on it.
        assert_eq!(
            SOURCES.iter().map(|s| s.id).collect::<Vec<_>>(),
            MacSourceId::ALL.to_vec()
        );
    }

    #[test]
    fn every_spec_is_complete_and_honest() {
        for spec in SOURCES.iter() {
            assert!(!spec.note.is_empty());
            // Only permission-free sources are probed in this phase.
            assert_eq!(
                spec.availability == SourceAvailability::Probed,
                spec.access == SourceAccess::ReadableNow,
                "{:?}: probed ⇔ readable-now",
                spec.id
            );
            match spec.location {
                SourceLocation::Absolute { path } => assert!(path.starts_with('/')),
                SourceLocation::HomeRelative { relative } => {
                    assert!(relative.starts_with("~/"), "{relative}")
                }
                SourceLocation::Mechanism { description } => assert!(!description.is_empty()),
            }
        }
    }

    #[test]
    fn deferred_sources_never_claim_reads() {
        for spec in SOURCES.iter() {
            if spec.availability == SourceAvailability::Deferred {
                assert_ne!(
                    spec.access,
                    SourceAccess::ReadableNow,
                    "{:?} is deferred and must not be read",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn related_capabilities_exist() {
        for spec in SOURCES.iter() {
            let related = spec.id.related_capabilities();
            assert!(
                !related.is_empty(),
                "{:?} maps to at least one capability",
                spec.id
            );
            for cap in related {
                assert!(
                    CapabilityId::ALL.contains(cap),
                    "{spec:?} references a registered capability"
                );
            }
        }
    }

    #[test]
    fn core_capability_mapping_is_pinned() {
        assert_eq!(
            source(MacSourceId::UserLaunchAgents)
                .id
                .related_capabilities(),
            [CapabilityId::LaunchAgents, CapabilityId::StartupItems]
        );
        assert_eq!(
            source(MacSourceId::ApplicationsDir)
                .id
                .related_capabilities(),
            [
                CapabilityId::ApplicationInventory,
                CapabilityId::SoftwareManagement,
                CapabilityId::StorageAnalysis
            ]
        );
    }
}
