//! Permission-aware observation of the macOS source catalog
//! (Phase 6.1, Tasks 4 and 6).
//!
//! The fact→state mapping ([`observe_path`]) is shared, platform-independent
//! logic: it is tested on every platform with scripted probes. Real
//! observation runs only on macOS; non-macOS hosts report every source as
//! [`AccessState::Unsupported`] — never as empty or absent.

use std::io;
use std::path::{Path, PathBuf};

use coresight_capabilities::access::AccessState;
use serde::{Deserialize, Serialize};

use crate::catalog::{
    MacSourceId, MacSourceSpec, SourceAccess, SourceAvailability, SourceLocation, SOURCES,
};
use crate::probe::{MacFileProbe, ProbeKind};

/// Bounds for one observation. Deterministic; truncation is flagged, never
/// hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeLimits {
    pub max_children_per_source: u64,
}

impl Default for ProbeLimits {
    fn default() -> Self {
        ProbeLimits {
            max_children_per_source: 4096,
        }
    }
}

/// One source's observation: the honest access state plus bounded counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceObservation {
    pub source: MacSourceId,
    pub access: AccessState,
    /// Children seen; `Some` only for `ReadSucceeded`/`Empty`.
    pub children_seen: Option<u64>,
    pub children_truncated: bool,
    pub note: Option<String>,
}

/// Map raw probe facts onto the honest access states. Deterministic and
/// platform-independent.
///
/// Key honesty decisions:
/// - metadata `NotFound` → [`AccessState::DoesNotExist`] (proven absence);
/// - metadata denied → [`AccessState::Failed`]: existence could NOT be
///   proven, so claiming exists-but-inaccessible would overclaim;
/// - listing denied after a successful stat →
///   [`AccessState::ExistsButInaccessible`] (existence proven, read denied);
/// - an empty successful listing → [`AccessState::Empty`], a fact distinct
///   from every failure state;
/// - links are never followed; a symlink at the source path is `Failed`
///   with an explanatory note, never traversed.
pub fn observe_path(
    probe: &dyn MacFileProbe,
    path: &Path,
    limits: &ProbeLimits,
) -> (AccessState, Option<u64>, bool, Option<String>) {
    match probe.metadata(path) {
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                (AccessState::DoesNotExist, None, false, None)
            }
            _ => (
                AccessState::Failed,
                None,
                false,
                Some(format!("metadata unavailable: {err}")),
            ),
        },
        Ok(kind) => match kind {
            ProbeKind::Dir => {
                match probe.list_children_bounded(path, limits.max_children_per_source) {
                    Ok(cc) => {
                        let state = if cc.truncated || cc.count > 0 {
                            AccessState::ReadSucceeded
                        } else {
                            AccessState::Empty
                        };
                        (state, Some(cc.count), cc.truncated, None)
                    }
                    Err(err) if err.kind() == io::ErrorKind::PermissionDenied => (
                        AccessState::ExistsButInaccessible,
                        None,
                        false,
                        Some("directory listing denied".to_string()),
                    ),
                    Err(err) => (
                        AccessState::Failed,
                        None,
                        false,
                        Some(format!("listing failed: {err}")),
                    ),
                }
            }
            ProbeKind::Symlink => (
                AccessState::Failed,
                None,
                false,
                Some("path is a link; links are never followed".to_string()),
            ),
            ProbeKind::File | ProbeKind::Other => (
                AccessState::Failed,
                None,
                false,
                Some("expected a directory".to_string()),
            ),
        },
    }
}

/// Resolve a catalog location against a home directory. Home-relative
/// sources carry the `~/` display form; the prefix is resolved here.
/// `None` for mechanism sources (they are never probed).
fn resolve(location: SourceLocation, home: &Path) -> Option<PathBuf> {
    match location {
        SourceLocation::Absolute { path } => Some(PathBuf::from(path)),
        SourceLocation::HomeRelative { relative } => {
            let rel = relative.strip_prefix("~/").unwrap_or(relative);
            Some(home.join(rel))
        }
        SourceLocation::Mechanism { .. } => None,
    }
}

/// Observe one catalog source. Non-probed sources (deferred, permission-
/// gated, or unsupported mechanisms) report [`AccessState::Unsupported`]
/// with the catalog's reason — never an empty success.
pub fn observe_source(
    probe: &dyn MacFileProbe,
    spec: &MacSourceSpec,
    home: &Path,
    limits: &ProbeLimits,
) -> SourceObservation {
    if spec.availability != SourceAvailability::Probed || spec.access != SourceAccess::ReadableNow {
        return SourceObservation {
            source: spec.id,
            access: AccessState::Unsupported,
            children_seen: None,
            children_truncated: false,
            note: Some(spec.note.to_string()),
        };
    }
    match resolve(spec.location, home) {
        Some(path) => {
            let (access, children, truncated, note) = observe_path(probe, &path, limits);
            SourceObservation {
                source: spec.id,
                access,
                children_seen: children,
                children_truncated: truncated,
                note,
            }
        }
        None => SourceObservation {
            source: spec.id,
            access: AccessState::Unsupported,
            children_seen: None,
            children_truncated: false,
            note: Some("mechanism source has no probed path".to_string()),
        },
    }
}

/// Observe the whole catalog against one home directory, in canonical
/// (catalog) order. Deterministic for identical inputs.
pub fn observe_with_probe(
    probe: &dyn MacFileProbe,
    home: &Path,
    limits: &ProbeLimits,
) -> Vec<SourceObservation> {
    SOURCES
        .iter()
        .map(|spec| observe_source(probe, spec, home, limits))
        .collect()
}

/// Production entry point for the current host.
#[cfg(target_os = "macos")]
pub fn observe_host_sources(home: &Path, limits: &ProbeLimits) -> Vec<SourceObservation> {
    observe_with_probe(&crate::probe::StdProbe, home, limits)
}

/// Non-macOS hosts cannot service macOS discovery sources. Every source is
/// reported UNSUPPORTED — never as an empty or absent result — so a
/// non-Mac run can never masquerade as "nothing found on this Mac".
#[cfg(not(target_os = "macos"))]
pub fn observe_host_sources(_home: &Path, _limits: &ProbeLimits) -> Vec<SourceObservation> {
    SOURCES
        .iter()
        .map(|spec| SourceObservation {
            source: spec.id,
            access: AccessState::Unsupported,
            children_seen: None,
            children_truncated: false,
            note: Some(format!(
                "macOS discovery sources are only observable on macOS ({})",
                spec.note
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::ChildCount;
    use std::collections::HashMap;

    /// Scripted probe for deterministic tests on every platform. Errors are
    /// stored as `ErrorKind` (io::Error is not Clone) and rebuilt on access.
    struct FakeProbe {
        meta: HashMap<PathBuf, Result<ProbeKind, io::ErrorKind>>,
        list: HashMap<PathBuf, Result<ChildCount, io::ErrorKind>>,
    }

    impl FakeProbe {
        fn new() -> Self {
            FakeProbe {
                meta: HashMap::new(),
                list: HashMap::new(),
            }
        }
        fn with_dir(mut self, path: &Path, children: ChildCount) -> Self {
            self.meta.insert(path.to_path_buf(), Ok(ProbeKind::Dir));
            self.list.insert(path.to_path_buf(), Ok(children));
            self
        }
        fn with_meta(mut self, path: &Path, kind: ProbeKind) -> Self {
            self.meta.insert(path.to_path_buf(), Ok(kind));
            self
        }
        fn with_meta_err(mut self, path: &Path, kind: io::ErrorKind) -> Self {
            self.meta.insert(path.to_path_buf(), Err(kind));
            self
        }
        fn with_list_err(mut self, path: &Path, kind: io::ErrorKind) -> Self {
            self.list.insert(path.to_path_buf(), Err(kind));
            self
        }
    }

    impl MacFileProbe for FakeProbe {
        fn metadata(&self, path: &Path) -> io::Result<ProbeKind> {
            match self.meta.get(path) {
                Some(Ok(kind)) => Ok(*kind),
                Some(Err(kind)) => Err(io::Error::new(*kind, "scripted")),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "scripted")),
            }
        }
        fn list_children_bounded(&self, path: &Path, max: u64) -> io::Result<ChildCount> {
            let _ = max;
            match self.list.get(path) {
                Some(Ok(cc)) => Ok(*cc),
                Some(Err(kind)) => Err(io::Error::new(*kind, "scripted")),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "scripted")),
            }
        }
    }

    const LIMITS: ProbeLimits = ProbeLimits {
        max_children_per_source: 100,
    };

    #[test]
    fn a_read_dir_with_children_is_read_succeeded() {
        let path = Path::new("/fixture/apps");
        let probe = FakeProbe::new().with_dir(
            path,
            ChildCount {
                count: 3,
                truncated: false,
            },
        );
        let (state, children, truncated, note) = observe_path(&probe, path, &LIMITS);
        assert_eq!(state, AccessState::ReadSucceeded);
        assert_eq!(children, Some(3));
        assert!(!truncated);
        assert_eq!(note, None);
    }

    #[test]
    fn an_empty_listing_is_empty_not_missing() {
        let path = Path::new("/fixture/empty");
        let probe = FakeProbe::new().with_dir(
            path,
            ChildCount {
                count: 0,
                truncated: false,
            },
        );
        let (state, children, _, _) = observe_path(&probe, path, &LIMITS);
        assert_eq!(state, AccessState::Empty);
        assert_eq!(children, Some(0));
        // The anti-pattern: Empty is not DoesNotExist, not Failed, not
        // ExistsButInaccessible.
        assert_ne!(state, AccessState::DoesNotExist);
        assert_ne!(state, AccessState::Failed);
        assert_ne!(state, AccessState::ExistsButInaccessible);
    }

    #[test]
    fn a_missing_path_is_proven_absent() {
        let probe = FakeProbe::new();
        let (state, children, _, _) = observe_path(&probe, Path::new("/fixture/gone"), &LIMITS);
        assert_eq!(state, AccessState::DoesNotExist);
        assert_eq!(children, None);
    }

    #[test]
    fn listing_denied_after_stat_is_exists_but_inaccessible() {
        let path = Path::new("/fixture/protected");
        let probe = FakeProbe::new()
            .with_dir(
                path,
                ChildCount {
                    count: 0,
                    truncated: false,
                },
            )
            .with_list_err(path, io::ErrorKind::PermissionDenied);
        let (state, children, _, note) = observe_path(&probe, path, &LIMITS);
        assert_eq!(state, AccessState::ExistsButInaccessible);
        assert_eq!(children, None);
        assert!(note.is_some());
        // Never collapsed into empty.
        assert!(!state.is_read());
    }

    #[test]
    fn metadata_denied_is_failed_not_inaccessible() {
        // We could not stat the path: existence is UNPROVEN. Claiming
        // "exists but inaccessible" would overclaim.
        let path = Path::new("/fixture/denied-stat");
        let probe = FakeProbe::new().with_meta_err(path, io::ErrorKind::PermissionDenied);
        let (state, children, _, note) = observe_path(&probe, path, &LIMITS);
        assert_eq!(state, AccessState::Failed);
        assert_eq!(children, None);
        assert!(note.is_some());
        assert_ne!(state, AccessState::ExistsButInaccessible);
        assert_ne!(state, AccessState::Empty);
    }

    #[test]
    fn links_are_never_followed() {
        let path = Path::new("/fixture/link");
        let probe = FakeProbe::new().with_meta(path, ProbeKind::Symlink);
        let (state, _, _, note) = observe_path(&probe, path, &LIMITS);
        assert_eq!(state, AccessState::Failed);
        assert!(note.unwrap().contains("never followed"));
    }

    #[test]
    fn a_file_where_a_dir_is_expected_fails_honestly() {
        let path = Path::new("/fixture/not-a-dir");
        let probe = FakeProbe::new().with_meta(path, ProbeKind::File);
        let (state, _, _, note) = observe_path(&probe, path, &LIMITS);
        assert_eq!(state, AccessState::Failed);
        assert!(note.unwrap().contains("expected a directory"));
    }

    #[test]
    fn observation_is_deterministic() {
        let path = Path::new("/fixture/apps");
        let probe = FakeProbe::new().with_dir(
            path,
            ChildCount {
                count: 7,
                truncated: false,
            },
        );
        let first = observe_path(&probe, path, &LIMITS);
        let second = observe_path(&probe, path, &LIMITS);
        assert_eq!(first, second);
    }

    #[test]
    fn deferred_and_gated_sources_report_unsupported() {
        let home = Path::new("/fixture/home");
        let probe = FakeProbe::new();
        for id in [
            MacSourceId::LoginItems,
            MacSourceId::TccProtectedUserData,
            MacSourceId::ApfsVolumeInfo,
        ] {
            let obs = observe_source(&probe, crate::catalog::source(id), home, &LIMITS);
            assert_eq!(obs.access, AccessState::Unsupported, "{id:?}");
            assert_eq!(obs.children_seen, None);
            assert!(obs.note.is_some());
        }
    }

    #[test]
    fn full_catalog_observation_is_well_formed_and_deterministic() {
        let home = Path::new("/fixture/home");
        let probe = FakeProbe::new()
            .with_dir(
                Path::new("/Applications"),
                ChildCount {
                    count: 12,
                    truncated: false,
                },
            )
            .with_dir(
                Path::new("/fixture/home/Library/Caches"),
                ChildCount {
                    count: 0,
                    truncated: false,
                },
            )
            .with_dir(
                Path::new("/fixture/home/Library/Containers"),
                ChildCount {
                    count: 0,
                    truncated: false,
                },
            )
            .with_list_err(
                Path::new("/fixture/home/Library/Containers"),
                io::ErrorKind::PermissionDenied,
            );
        let first = observe_with_probe(&probe, home, &LIMITS);
        let second = observe_with_probe(&probe, home, &LIMITS);
        assert_eq!(first, second, "deterministic for identical inputs");
        assert_eq!(first.len(), SOURCES.len());
        assert_eq!(first[0].source, MacSourceId::ApplicationsDir);

        for obs in &first {
            let spec = crate::catalog::source(obs.source);
            if spec.availability == SourceAvailability::Probed {
                assert_ne!(
                    obs.access,
                    AccessState::Unsupported,
                    "probed sources are really probed"
                );
                if let Some(count) = obs.children_seen {
                    assert!(
                        matches!(obs.access, AccessState::ReadSucceeded | AccessState::Empty),
                        "children_seen only travels with completed reads"
                    );
                    let _ = count;
                }
            } else {
                assert_eq!(obs.access, AccessState::Unsupported);
                assert_eq!(obs.children_seen, None);
            }
        }
        let containers = first
            .iter()
            .find(|o| o.source == MacSourceId::UserContainers)
            .unwrap();
        assert_eq!(containers.access, AccessState::ExistsButInaccessible);
        let caches = first
            .iter()
            .find(|o| o.source == MacSourceId::UserCaches)
            .unwrap();
        assert_eq!(caches.access, AccessState::Empty);
    }

    #[test]
    fn home_relative_locations_resolve_under_home() {
        let home = Path::new("/fixture/home");
        assert_eq!(
            resolve(
                SourceLocation::HomeRelative {
                    relative: "~/Library/Caches"
                },
                home
            ),
            Some(PathBuf::from("/fixture/home/Library/Caches"))
        );
        assert_eq!(
            resolve(
                SourceLocation::Absolute {
                    path: "/Applications"
                },
                home
            ),
            Some(PathBuf::from("/Applications"))
        );
        assert_eq!(
            resolve(SourceLocation::Mechanism { description: "x" }, home),
            None
        );
    }

    #[test]
    fn source_observations_round_trip_through_serde() {
        let obs = SourceObservation {
            source: MacSourceId::UserCaches,
            access: AccessState::ReadSucceeded,
            children_seen: Some(41),
            children_truncated: false,
            note: None,
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("USER_CACHES"));
        assert!(json.contains("READ_SUCCEEDED"));
        let back: SourceObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_hosts_report_every_source_unsupported() {
        let obs = observe_host_sources(Path::new("/unused"), &ProbeLimits::default());
        assert_eq!(obs.len(), SOURCES.len());
        for o in &obs {
            assert_eq!(o.access, AccessState::Unsupported);
            assert_eq!(o.children_seen, None);
            assert!(o.note.as_deref().unwrap_or_default().contains("macOS"));
        }
        // The core honesty rule: unsupported is not "empty", not "absent".
        assert_ne!(obs[0].access, AccessState::Empty);
        assert_ne!(obs[0].access, AccessState::DoesNotExist);
    }
}
