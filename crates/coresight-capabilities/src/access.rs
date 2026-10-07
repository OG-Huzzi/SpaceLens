//! Path-access truth model (Phase 6.1, Task 6 — macOS permission model).
//!
//! macOS may deny access to locations even when the path exists. These
//! states make that honesty structural: permission denial is NEVER
//! collapsed into an empty result, and "unsupported" is never confused
//! with "does not exist".
//!
//! Rules every consumer must respect:
//!
//! - [`AccessState::Empty`] is a SUCCESSFUL read that found nothing — a
//!   fact about the contents.
//! - [`AccessState::ExistsButInaccessible`] proves existence (metadata
//!   succeeded) while the read was denied — the contents are UNKNOWN.
//! - [`AccessState::Failed`] means the attempt errored in a way that may
//!   leave even existence unproven — the contents are UNKNOWN.
//! - [`AccessState::Unsupported`] means this build cannot service the
//!   source at all — the contents are UNKNOWN by construction.
//!
//! No privilege escalation and no security circumvention ever: a denied
//! location stays denied and is reported as such.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccessState {
    /// Read successfully; the count facts travel with the observation.
    ReadSucceeded,
    /// Read successfully and found nothing — a fact, not a failure.
    Empty,
    /// Proven absent.
    DoesNotExist,
    /// Exists (proven by metadata) but the OS denied reading it.
    ExistsButInaccessible,
    /// The source concept does not apply in this environment/configuration.
    NotApplicable,
    /// This build cannot service the source at all (no API surface, other
    /// platform).
    Unsupported,
    /// Attempted and failed; existence may be unprovable (e.g. access
    /// denied before metadata could answer).
    Failed,
}

impl AccessState {
    pub const ALL: [AccessState; 7] = [
        AccessState::ReadSucceeded,
        AccessState::Empty,
        AccessState::DoesNotExist,
        AccessState::ExistsButInaccessible,
        AccessState::NotApplicable,
        AccessState::Unsupported,
        AccessState::Failed,
    ];

    /// True only when a read completed, with or without content. Every
    /// other state leaves the contents UNKNOWN — never "empty".
    pub fn is_read(self) -> bool {
        matches!(self, AccessState::ReadSucceeded | AccessState::Empty)
    }

    /// True when this state proves the path exists.
    pub fn proves_existence(self) -> bool {
        matches!(
            self,
            AccessState::ReadSucceeded | AccessState::Empty | AccessState::ExistsButInaccessible
        )
    }

    /// True when this state proves the path does not exist.
    pub fn proves_absence(self) -> bool {
        matches!(self, AccessState::DoesNotExist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_states_are_distinct() {
        for (i, a) in AccessState::ALL.iter().enumerate() {
            for b in &AccessState::ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} and {b:?} must stay distinct");
            }
        }
    }

    #[test]
    fn permission_denial_is_never_an_empty_result() {
        // The anti-pattern this model exists to prevent.
        assert_ne!(AccessState::ExistsButInaccessible, AccessState::Empty);
        assert_ne!(AccessState::Failed, AccessState::Empty);
        assert_ne!(AccessState::Unsupported, AccessState::Empty);
        assert!(!AccessState::ExistsButInaccessible.is_read());
        assert!(!AccessState::Failed.is_read());
        assert!(!AccessState::Unsupported.is_read());
    }

    #[test]
    fn existence_and_absence_claims_are_exact() {
        assert!(AccessState::ReadSucceeded.proves_existence());
        assert!(AccessState::Empty.proves_existence());
        assert!(AccessState::ExistsButInaccessible.proves_existence());
        assert!(!AccessState::Failed.proves_existence());
        assert!(!AccessState::DoesNotExist.proves_existence());
        assert!(AccessState::DoesNotExist.proves_absence());
        assert!(!AccessState::Failed.proves_absence());
        assert!(!AccessState::Unsupported.proves_absence());
    }

    #[test]
    fn only_reads_are_reads() {
        for state in AccessState::ALL {
            assert_eq!(
                state.is_read(),
                matches!(state, AccessState::ReadSucceeded | AccessState::Empty),
                "{state:?}"
            );
        }
    }
}
