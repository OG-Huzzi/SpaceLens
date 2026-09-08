//! Deterministic confidence model.
//!
//! Confidence is one of four semantic bands — never fake numeric precision
//! (master prompt §10). Two hard rules are enforced by tests, not just
//! convention:
//!
//! 1. **Extension-only evidence can never exceed [`Confidence::Medium`].**
//!    A filename extension alone is weak evidence (§2 rule 29/30).
//! 2. **Pure heuristics (bare directory names like `build`) can never exceed
//!    [`Confidence::Low`].**
//!
//! Aggregation takes the *strongest* band present in the winning rule's own
//! evidence — context can raise a heuristic, but a weak rule can never
//! masquerade as certainty on its own.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Confidence {
    /// No meaningful evidence.
    Unknown,
    /// Weak heuristic only (bare well-known directory names, single weak
    /// filename signal).
    Low,
    /// Reasonable evidence (extension match, or strong location + weak
    /// corroboration).
    Medium,
    /// Authoritative evidence (known application/system/cache location,
    /// canonical directory with corroborating signals).
    High,
}

impl Confidence {
    pub fn code(self) -> &'static str {
        match self {
            Confidence::Unknown => "UNKNOWN",
            Confidence::Low => "LOW",
            Confidence::Medium => "MEDIUM",
            Confidence::High => "HIGH",
        }
    }

    /// The maximum confidence an extension-only classification may reach.
    pub const EXTENSION_ONLY_CAP: Confidence = Confidence::Medium;

    /// The maximum confidence a bare heuristic may reach.
    pub const HEURISTIC_CAP: Confidence = Confidence::Low;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_is_semantic() {
        assert!(Confidence::Unknown < Confidence::Low);
        assert!(Confidence::Low < Confidence::Medium);
        assert!(Confidence::Medium < Confidence::High);
    }

    #[test]
    fn caps_are_correct() {
        assert_eq!(Confidence::EXTENSION_ONLY_CAP, Confidence::Medium);
        assert_eq!(Confidence::HEURISTIC_CAP, Confidence::Low);
        assert!(Confidence::High > Confidence::EXTENSION_ONLY_CAP);
        assert!(Confidence::Medium > Confidence::HEURISTIC_CAP);
    }
}
