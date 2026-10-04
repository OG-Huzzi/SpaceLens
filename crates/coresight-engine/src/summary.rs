//! Typed scan outcomes.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::error::{ErrorCategory, ScanErrorReport};

/// Final scan state. A cancelled scan is a *successful cancellation*, not an
/// error; `Failed` is reserved for the scan being unable to proceed at all
/// (e.g. the root itself was unreadable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanStatus {
    Completed,
    Cancelled,
    Failed,
}

/// Aggregate result of one scan. Entries are streamed to the caller and NOT
/// retained here — memory stays flat against tree size (docs/PERFORMANCE.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub status: ScanStatus,
    /// Requested root, as given (never canonicalized away).
    pub root: PathBuf,
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub other_entries: u64,
    /// Total logical bytes of files successfully statted.
    pub bytes: u64,
    /// Total allocated bytes over entries where the platform reports them
    /// (`None` if the platform never reports allocated sizes).
    pub allocated_bytes: Option<u64>,
    /// Entries discovered but not fully statted (their error is tallied).
    pub entries_with_errors: u64,
    pub errors: ScanErrorReport,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    /// True when traversal hit the optional depth cap.
    pub depth_capped: bool,
}

impl ScanSummary {
    /// Per-category error counts, ordered for stable IPC output.
    pub fn error_count(&self, category: ErrorCategory) -> u64 {
        self.errors
            .counts
            .iter()
            .find(|(c, _)| *c == category)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_count_lookups_are_exact() {
        let mut report = ScanErrorReport::new();
        report.record(crate::error::ScanError {
            category: ErrorCategory::PermissionDenied,
            path: PathBuf::from("p"),
            message: "m".into(),
            raw_os: None,
        });
        let summary = ScanSummary {
            status: ScanStatus::Completed,
            root: PathBuf::from("/"),
            files: 0,
            dirs: 0,
            links: 0,
            other_entries: 0,
            bytes: 0,
            allocated_bytes: None,
            entries_with_errors: 1,
            errors: report,
            started_at: SystemTime::UNIX_EPOCH,
            finished_at: SystemTime::UNIX_EPOCH,
            depth_capped: false,
        };
        assert_eq!(summary.error_count(ErrorCategory::PermissionDenied), 1);
        assert_eq!(summary.error_count(ErrorCategory::NotFound), 0);
    }
}
