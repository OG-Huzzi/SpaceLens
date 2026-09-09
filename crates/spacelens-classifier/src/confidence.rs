//! Deterministic confidence model.
//!
//! Confidence is one of four semantic bands — never fake numeric precision
//! (master prompt §10).
//!
//! # The single confidence policy
//!
//! Every rule in the table carries a [`RuleKind`]. The kind — not the rule's
//! author, not a scattered special case — determines the hard ceiling that
//! mechanically clamps the rule's base confidence:
//!
//! | [`RuleKind`]        | Hard ceiling                                     |
//! |---------------------|--------------------------------------------------|
//! | [`RuleKind::Authoritative`] | [`Confidence::AUTHORITATIVE_CAP`] (`High`)     |
//! | [`RuleKind::Extension`]     | [`Confidence::EXTENSION_ONLY_CAP`] (`Medium`)  |
//! | [`RuleKind::Heuristic`]     | [`Confidence::HEURISTIC_CAP`] (`Low`), or [`Confidence::HEURISTIC_CORROBORATED_CAP`] (`Medium`) when corroborated |
//!
//! The clamp is applied in one place (`classify()`), after the winner is
//! known, before any context adjustment. Nothing can bypass it:
//!
//! 1. **Extension-only evidence can never exceed `Medium`.** A filename
//!    extension alone is weak evidence, and no amount of corroboration turns
//!    it into location knowledge.
//! 2. **A pure heuristic can never exceed `Low`.** A bare directory name such
//!    as `build` or `cache` is a guess; only corroboration by authoritative
//!    location knowledge or by a classified parent may lift it to `Medium`.
//! 3. **Authoritative location knowledge may reach `High`.** A rooted,
//!    platform-specific location (`Program Files`, `/usr`, `~/Library/Caches`)
//!    is knowledge, not a guess.
//!
//! Parent context may raise a winning confidence by **at most one band**, and
//! is then clamped back to the ceiling — so context can never manufacture
//! `High` for an extension match or `Medium` for an uncorroborated guess.
//!
//! A *location-gated* rule (see `rules::RuleGate`) is the one deliberate
//! exception: it may only win when authoritative location knowledge already
//! corroborates it, so at win time the location — not the name — supplies the
//! confidence, and the ceiling becomes `High`.

use serde::{Deserialize, Serialize};

/// How a rule knows what it knows. This is the only input to the confidence
/// ceiling; it is carried by every rule so policy is data, not scattered
/// special cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuleKind {
    /// Rooted platform/location knowledge (or a name signal that is only
    /// eligible when such knowledge is present).
    Authoritative,
    /// A filename-extension table match.
    Extension,
    /// A bare-name guess (`build`, `cache`, `tmp`, `setup`, …).
    Heuristic,
}

impl RuleKind {
    /// Stable IPC identifier.
    pub fn code(self) -> &'static str {
        match self {
            RuleKind::Authoritative => "AUTHORITATIVE",
            RuleKind::Extension => "EXTENSION",
            RuleKind::Heuristic => "HEURISTIC",
        }
    }

    /// The hard ceiling for a rule of this kind when it is **not**
    /// corroborated. See [`Confidence::cap_for`].
    pub const fn cap(self) -> Confidence {
        Confidence::cap_for(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Confidence {
    /// No meaningful evidence at all.
    Unknown,
    /// Weak heuristic only (bare well-known directory names, single weak
    /// filename signal).
    Low,
    /// Reasonable evidence (extension match, or a heuristic corroborated by
    /// authoritative location knowledge).
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

    /// Maximum confidence an extension-table classification may ever reach.
    /// Absolute: corroboration and parent context cannot lift it.
    pub const EXTENSION_ONLY_CAP: Confidence = Confidence::Medium;

    /// Maximum confidence a *pure* (uncorroborated) heuristic may reach.
    pub const HEURISTIC_CAP: Confidence = Confidence::Low;

    /// Maximum confidence a heuristic reaches once authoritative location
    /// knowledge (or a classified parent) corroborates it.
    pub const HEURISTIC_CORROBORATED_CAP: Confidence = Confidence::Medium;

    /// Maximum confidence authoritative location knowledge may reach.
    pub const AUTHORITATIVE_CAP: Confidence = Confidence::High;

    /// The hard ceiling for a rule kind. Single source of truth — the policy
    /// itself (never duplicated in the rule table or in `classify()`).
    pub const fn cap_for(kind: RuleKind) -> Confidence {
        match kind {
            RuleKind::Authoritative => Confidence::AUTHORITATIVE_CAP,
            RuleKind::Extension => Confidence::EXTENSION_ONLY_CAP,
            RuleKind::Heuristic => Confidence::HEURISTIC_CAP,
        }
    }

    /// Raise at most one band (`Unknown→Low`, `Low→Medium`, `Medium→High`),
    /// then clamp to `cap`. This is the only way context may influence
    /// confidence, and it can never escape the ceiling.
    pub fn raise_one_band_capped(self, cap: Confidence) -> Confidence {
        let raised = match self {
            Confidence::Unknown => Confidence::Low,
            Confidence::Low => Confidence::Medium,
            Confidence::Medium | Confidence::High => Confidence::High,
        };
        if raised > cap {
            cap
        } else {
            raised
        }
    }
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
        assert_eq!(Confidence::AUTHORITATIVE_CAP, Confidence::High);
        assert!(Confidence::High > Confidence::EXTENSION_ONLY_CAP);
        assert!(Confidence::Medium > Confidence::HEURISTIC_CAP);
    }

    #[test]
    fn cap_for_matches_each_kind() {
        assert_eq!(RuleKind::Authoritative.cap(), Confidence::High);
        assert_eq!(RuleKind::Extension.cap(), Confidence::Medium);
        assert_eq!(RuleKind::Heuristic.cap(), Confidence::Low);
    }

    #[test]
    fn raise_one_band_never_exceeds_cap() {
        // Extension ceiling: even repeated raises cannot pass Medium.
        let mut c = Confidence::Low;
        for _ in 0..5 {
            c = c.raise_one_band_capped(Confidence::EXTENSION_ONLY_CAP);
        }
        assert_eq!(c, Confidence::Medium);

        // Heuristic ceiling: an uncorroborated guess stays Low.
        assert_eq!(
            Confidence::Low.raise_one_band_capped(Confidence::HEURISTIC_CAP),
            Confidence::Low
        );

        // Heuristic with room: Low -> Medium, then clamped.
        assert_eq!(
            Confidence::Low.raise_one_band_capped(Confidence::HEURISTIC_CORROBORATED_CAP),
            Confidence::Medium
        );
        assert_eq!(
            Confidence::Medium.raise_one_band_capped(Confidence::HEURISTIC_CORROBORATED_CAP),
            Confidence::Medium
        );

        // Authoritative ceiling allows High.
        assert_eq!(
            Confidence::Medium.raise_one_band_capped(Confidence::AUTHORITATIVE_CAP),
            Confidence::High
        );
    }

    #[test]
    fn rule_kind_codes_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in [
            RuleKind::Authoritative,
            RuleKind::Extension,
            RuleKind::Heuristic,
        ] {
            assert!(seen.insert(k.code()));
        }
    }
}
