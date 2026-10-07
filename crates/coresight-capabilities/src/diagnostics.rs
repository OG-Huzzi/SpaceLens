//! Typed contract for diagnostic signals (Phase 6.1, contract F). Model
//! only — no signal producer exists yet. A diagnostic signal EXPLAINS; it
//! never acts. Every claim carries evidence strings naming its sources.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticSignalKind {
    StoragePressure,
    StartupBurden,
    ResourcePressure,
    SystemHealth,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticSeverity {
    Info,
    Notice,
    Warning,
    Critical,
    Unknown,
}

/// One root-cause-oriented diagnostic: what was observed, how severe, and
/// the evidence behind the claim. Advisory only — a signal cannot execute
/// anything (see [`crate::safety`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSignal {
    pub kind: DiagnosticSignalKind,
    pub severity: DiagnosticSeverity,
    pub summary: String,
    /// Provenance strings: every claim names where it came from.
    pub evidence: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let signal = DiagnosticSignal {
            kind: DiagnosticSignalKind::StoragePressure,
            severity: DiagnosticSeverity::Warning,
            summary: "Developer caches grew 12 GB in 30 days".into(),
            evidence: vec!["storage-analysis:caches".into()],
        };
        let json = serde_json::to_string(&signal).unwrap();
        assert!(json.contains("STORAGE_PRESSURE"));
        let back: DiagnosticSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, signal);
    }
}
