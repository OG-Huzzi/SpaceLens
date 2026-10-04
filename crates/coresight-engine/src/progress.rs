//! Typed progress reporting.
//!
//! Events are structured values, never human-readable strings — presentation
//! belongs to the UI. `Progress` events are throttled to
//! `ScanOptions::progress_interval` (default 250 ms ⇒ ≤4/sec, the contract
//! cap in docs/PERFORMANCE.md) plus one always-final event, so a 1M-file scan
//! cannot flood the IPC channel.

use serde::{Deserialize, Serialize};

use crate::model::FsEntry;
use crate::summary::ScanSummary;

/// Scan phase, for typed progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Preparing,
    Walking,
    Finalizing,
}

/// Immutable counter snapshot for progress display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressSnapshot {
    pub files_seen: u64,
    pub dirs_seen: u64,
    pub bytes_seen: u64,
    pub errors_seen: u64,
    /// Elapsed since scan start, in milliseconds. Observational only — never
    /// a pass/fail criterion.
    pub elapsed_ms: u64,
}

/// Events emitted by a scan. The stream always starts with `Started`, ends
/// with exactly one of `Completed` / `Cancelled` / `Failed`, and may contain
/// any number of `Entry` / `Progress` events in between.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ScanEvent {
    Started,
    Entry(Box<FsEntry>),
    Progress(ProgressSnapshot),
    Completed(Box<ScanSummary>),
    Cancelled(Box<ScanSummary>),
    Failed(Box<ScanSummary>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_tag() {
        let json = serde_json::to_string(&ScanEvent::Started).unwrap();
        assert_eq!(json, r#"{"type":"started"}"#);
        let json = serde_json::to_string(&ScanEvent::Progress(ProgressSnapshot {
            files_seen: 12,
            ..Default::default()
        }))
        .unwrap();
        assert!(json.contains("\"filesSeen\":12"), "{json}");
    }
}
