//! Real-host observation (macOS CI only): the catalog observation path
//! runs against the actual runner filesystem with a synthetic home
//! directory. Read-only, bounded, no user data.

#![cfg(target_os = "macos")]

use std::path::Path;

use coresight_capabilities::access::AccessState;
use coresight_macos::{
    observe_with_probe, ProbeLimits, SourceAccess, SourceAvailability, SourceLocation, StdProbe,
    SOURCES,
};

#[test]
fn host_observation_is_well_formed() {
    // Synthetic home: never a real user directory.
    let home = tempfile::tempdir().expect("temp fixture");
    let limits = ProbeLimits::default();
    let observations = observe_with_probe(&StdProbe, home.path(), &limits);

    assert_eq!(observations.len(), SOURCES.len());
    for obs in &observations {
        let spec = SOURCES.iter().find(|s| s.id == obs.source).unwrap();
        if spec.availability == SourceAvailability::Deferred {
            assert_eq!(obs.access, AccessState::Unsupported, "{:?}", spec.id);
            assert_eq!(obs.children_seen, None);
            continue;
        }
        // Probed, permission-free sources were really probed on this host.
        assert_ne!(obs.access, AccessState::Unsupported, "{:?}", spec.id);
        match (spec.location, spec.access) {
            (SourceLocation::HomeRelative { .. }, SourceAccess::ReadableNow) => {
                // A fresh temp dir has none of the home-relative sources:
                // absence is proven, never filled in.
                assert_eq!(obs.access, AccessState::DoesNotExist, "{:?}", spec.id);
                assert_eq!(obs.children_seen, None);
            }
            (SourceLocation::Absolute { .. }, SourceAccess::ReadableNow) => {
                assert!(
                    matches!(
                        obs.access,
                        AccessState::ReadSucceeded
                            | AccessState::Empty
                            | AccessState::DoesNotExist
                            | AccessState::ExistsButInaccessible
                            | AccessState::Failed
                    ),
                    "{:?}: honest concrete state, got {:?}",
                    spec.id,
                    obs.access
                );
                if let Some(count) = obs.children_seen {
                    assert!(matches!(
                        obs.access,
                        AccessState::ReadSucceeded | AccessState::Empty
                    ));
                    let _ = count;
                }
            }
            (SourceLocation::Mechanism { .. }, _) => {
                assert_eq!(obs.access, AccessState::Unsupported, "{:?}", spec.id);
            }
            _ => {}
        }
    }
}

#[test]
fn host_observation_is_deterministic() {
    let home = tempfile::tempdir().expect("temp fixture");
    let limits = ProbeLimits::default();
    let first = observe_with_probe(&StdProbe, home.path(), &limits);
    let second = observe_with_probe(&StdProbe, home.path(), &limits);
    // Listing counts of live system directories can shift in principle, so
    // determinism is asserted on the state sequence, not volatile counts.
    let states_a: Vec<AccessState> = first.iter().map(|o| o.access).collect();
    let states_b: Vec<AccessState> = second.iter().map(|o| o.access).collect();
    assert_eq!(states_a, states_b);
}

#[test]
fn user_home_sources_under_a_real_home_report_honestly() {
    // A real, temporary home with a planted cache directory proves the
    // home-relative resolution against the actual macOS filesystem.
    let home = tempfile::tempdir().expect("temp fixture");
    let caches = home.path().join("Library/Caches");
    std::fs::create_dir_all(&caches).expect("fixture dir");
    std::fs::write(caches.join("fixture-cache"), b"x").expect("fixture file");

    let limits = ProbeLimits::default();
    let observations = observe_with_probe(&StdProbe, home.path(), &limits);
    let caches_obs = observations
        .iter()
        .find(|o| o.source == coresight_macos::MacSourceId::UserCaches)
        .expect("user-caches is observed");
    assert_eq!(caches_obs.access, AccessState::ReadSucceeded);
    assert_eq!(caches_obs.children_seen, Some(1));
    assert!(Path::new(&caches).is_dir());
}
