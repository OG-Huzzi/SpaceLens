//! The per-capability report envelope (Phase 6.1, Task 3).
//!
//! A capability reports one [`CapabilityOutcome`] per discovery source. The
//! coverage helpers make the honesty rule mechanical: an unavailable or
//! failed source can never let a report claim completeness, and an empty
//! report is NOT complete — it means the capability was never attempted.

use serde::{Deserialize, Serialize};

use crate::capability::CapabilityId;
use crate::observation::{Observation, ObservationState};

/// One source's result inside a capability report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityOutcome<T> {
    pub source: String,
    pub observation: Observation<T>,
}

/// A capability's full report: outcomes per source, in the caller's
/// canonical order (callers keep sources in canonical order; this type
/// never reorders, so determinism is visible at the call site).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport<T> {
    pub capability: CapabilityId,
    pub outcomes: Vec<CapabilityOutcome<T>>,
}

/// Exact per-state counts across the report's sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub observed: usize,
    pub inferred: usize,
    pub unsupported: usize,
    pub unavailable: usize,
    pub failed: usize,
}

impl<T> CapabilityReport<T> {
    pub fn new(capability: CapabilityId) -> Self {
        CapabilityReport {
            capability,
            outcomes: Vec::new(),
        }
    }

    pub fn push(&mut self, source: impl Into<String>, observation: Observation<T>) {
        self.outcomes.push(CapabilityOutcome {
            source: source.into(),
            observation,
        });
    }

    pub fn coverage(&self) -> Coverage {
        let mut coverage = Coverage {
            observed: 0,
            inferred: 0,
            unsupported: 0,
            unavailable: 0,
            failed: 0,
        };
        for outcome in &self.outcomes {
            match outcome.observation.state() {
                ObservationState::Observed => coverage.observed += 1,
                ObservationState::Inferred => coverage.inferred += 1,
                ObservationState::Unsupported => coverage.unsupported += 1,
                ObservationState::Unavailable => coverage.unavailable += 1,
                ObservationState::Failed => coverage.failed += 1,
            }
        }
        coverage
    }

    /// True only when at least one source was attempted AND every source
    /// observed or inferred. An empty report is NOT complete.
    pub fn is_complete(&self) -> bool {
        !self.outcomes.is_empty()
            && self.outcomes.iter().all(|o| {
                matches!(
                    o.observation.state(),
                    ObservationState::Observed | ObservationState::Inferred
                )
            })
    }

    /// The sources currently in one state.
    pub fn sources_in_state(&self, state: ObservationState) -> Vec<&str> {
        self.outcomes
            .iter()
            .filter(|o| o.observation.state() == state)
            .map(|o| o.source.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> CapabilityReport<Vec<String>> {
        let mut report = CapabilityReport::new(CapabilityId::StartupItems);
        report.push(
            "user-launch-agents",
            Observation::observed(vec!["a".to_string()]),
        );
        report.push(
            "system-launch-daemons",
            Observation::unavailable("volume not mounted"),
        );
        report
    }

    #[test]
    fn coverage_counts_every_state_exactly() {
        let coverage = sample_report().coverage();
        assert_eq!(
            coverage,
            Coverage {
                observed: 1,
                inferred: 0,
                unsupported: 0,
                unavailable: 1,
                failed: 0
            }
        );
    }

    #[test]
    fn unavailable_sources_block_completeness() {
        let report = sample_report();
        assert!(!report.is_complete());
        assert_eq!(
            report.sources_in_state(ObservationState::Unavailable),
            vec!["system-launch-daemons"]
        );
    }

    #[test]
    fn an_empty_report_is_not_complete() {
        let report: CapabilityReport<String> = CapabilityReport::new(CapabilityId::StartupItems);
        assert!(!report.is_complete(), "not attempted is not complete");
        assert_eq!(report.coverage().observed, 0);
    }

    #[test]
    fn serde_round_trip() {
        let report = sample_report();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("STARTUP_ITEMS"));
        let back: CapabilityReport<Vec<String>> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }
}
