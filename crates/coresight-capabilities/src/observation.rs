//! The honesty envelope for every capability result (Phase 6.1, Task 3).
//!
//! Every result must distinguish: observed / inferred / unsupported /
//! unavailable / failed. The state is part of the type: an `Unsupported`,
//! `Unavailable`, or `Failed` observation structurally cannot carry a
//! payload, so unavailable data can never masquerade as "nothing found".
//! An `Observed` payload may legitimately be an empty collection — that is
//! a fact, and it is a different fact from "we could not look".

use serde::{Deserialize, Serialize};

/// Provenance of a capability result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservationState {
    /// Read directly from the system.
    Observed,
    /// Derived from observed evidence; the basis travels with the value and
    /// must never be presented as a direct read.
    Inferred,
    /// This build/host cannot service the source at all (no API surface,
    /// other platform). Never an empty success.
    Unsupported,
    /// The source is absent in this machine state (e.g. an unmounted
    /// volume, a registry root that does not exist). Never an empty success.
    Unavailable,
    /// Attempted and failed; the note names why. Never an empty success.
    Failed,
}

/// One capability result. The state is part of the type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum Observation<T> {
    Observed { value: T },
    Inferred { value: T, basis: String },
    Unsupported { note: String },
    Unavailable { note: String },
    Failed { note: String },
}

impl<T> Observation<T> {
    pub fn observed(value: T) -> Self {
        Observation::Observed { value }
    }

    pub fn inferred(value: T, basis: impl Into<String>) -> Self {
        Observation::Inferred {
            value,
            basis: basis.into(),
        }
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Observation::Unsupported { note: note.into() }
    }

    pub fn unavailable(note: impl Into<String>) -> Self {
        Observation::Unavailable { note: note.into() }
    }

    pub fn failed(note: impl Into<String>) -> Self {
        Observation::Failed { note: note.into() }
    }

    pub fn state(&self) -> ObservationState {
        match self {
            Observation::Observed { .. } => ObservationState::Observed,
            Observation::Inferred { .. } => ObservationState::Inferred,
            Observation::Unsupported { .. } => ObservationState::Unsupported,
            Observation::Unavailable { .. } => ObservationState::Unavailable,
            Observation::Failed { .. } => ObservationState::Failed,
        }
    }

    /// The payload, when the state carries one (`Observed`/`Inferred`).
    /// `None` for unsupported/unavailable/failed — callers must handle the
    /// absence explicitly, which is exactly the point.
    pub fn value(&self) -> Option<&T> {
        match self {
            Observation::Observed { value } | Observation::Inferred { value, .. } => Some(value),
            _ => None,
        }
    }

    /// The explanation carried by a non-observed state.
    pub fn note(&self) -> Option<&str> {
        match self {
            Observation::Unsupported { note }
            | Observation::Unavailable { note }
            | Observation::Failed { note } => Some(note),
            _ => None,
        }
    }

    /// The evidence basis of an `Inferred` observation.
    pub fn basis(&self) -> Option<&str> {
        match self {
            Observation::Inferred { basis, .. } => Some(basis),
            _ => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Observation<U> {
        match self {
            Observation::Observed { value } => Observation::Observed { value: f(value) },
            Observation::Inferred { value, basis } => Observation::Inferred {
                value: f(value),
                basis,
            },
            Observation::Unsupported { note } => Observation::Unsupported { note },
            Observation::Unavailable { note } => Observation::Unavailable { note },
            Observation::Failed { note } => Observation::Failed { note },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_observed_states_carry_no_value() {
        assert!(Observation::<Vec<u8>>::unsupported("x").value().is_none());
        assert!(Observation::<Vec<u8>>::unavailable("x").value().is_none());
        assert!(Observation::<Vec<u8>>::failed("x").value().is_none());
    }

    #[test]
    fn empty_observed_is_a_fact_distinct_from_unavailable() {
        let empty: Observation<Vec<u8>> = Observation::observed(Vec::new());
        let absent: Observation<Vec<u8>> = Observation::unavailable("source not mounted");
        assert_ne!(empty.state(), absent.state());
        assert_eq!(empty.value(), Some(&Vec::new()));
        assert_eq!(absent.value(), None);
        // The core Phase 6.1 honesty rule: unavailable data must never
        // masquerade as "nothing found".
        assert_ne!(empty, absent);
    }

    #[test]
    fn inferred_carries_its_basis() {
        let o: Observation<u32> = Observation::inferred(7, "derived from bundle presence");
        assert_eq!(o.state(), ObservationState::Inferred);
        assert_eq!(o.value(), Some(&7));
        assert_eq!(o.basis(), Some("derived from bundle presence"));
    }

    #[test]
    fn notes_travel_with_non_observed_states() {
        assert_eq!(
            Observation::<u8>::failed("read error").note(),
            Some("read error")
        );
        assert_eq!(Observation::<u8>::observed(1).note(), None);
    }

    #[test]
    fn map_preserves_state() {
        let o: Observation<Vec<u8>> = Observation::observed(vec![1, 2]);
        assert_eq!(o.map(|v| v.len()), Observation::observed(2));
        let f: Observation<Vec<u8>> = Observation::failed("nope");
        assert_eq!(f.map(|v| v.len()), Observation::<usize>::failed("nope"));
    }

    #[test]
    fn serde_round_trip_preserves_the_variant() {
        let observed: Observation<Vec<u32>> = Observation::observed(vec![1]);
        let json = serde_json::to_string(&observed).unwrap();
        assert!(json.contains("observed"));
        let back: Observation<Vec<u32>> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, observed);

        for (o, tag) in [
            (Observation::<u8>::unsupported("u"), "unsupported"),
            (Observation::<u8>::unavailable("n/a"), "unavailable"),
            (Observation::<u8>::failed("f"), "failed"),
            (Observation::<u8>::inferred(1, "b"), "inferred"),
        ] {
            let json = serde_json::to_string(&o).unwrap();
            assert!(json.contains(tag), "{json} must mention {tag}");
            let back: Observation<u8> = serde_json::from_str(&json).unwrap();
            assert_eq!(back, o);
        }
    }

    #[test]
    fn deserialization_cannot_smuggle_a_value_into_unsupported() {
        // The tagged enum has no representation for "unsupported with a
        // value": the shape itself cannot exist on the wire.
        let json = r#"{"state":"unsupported","note":"n","value":1}"#;
        let back: Result<Observation<u8>, _> = serde_json::from_str(json);
        assert!(back.is_err(), "unknown fields must not decode");
    }
}
