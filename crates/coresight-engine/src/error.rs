//! Typed scan errors.
//!
//! Internal errors are rich Rust values; the stable IPC codes they map to are
//! the contract surface (docs/API_CONTRACTS.md `{ code, message, detail }`).
//! The engine never turns recoverable per-entry errors into scan failures:
//! errors are recorded, categorized, tallied, and the scan continues.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable error categories. These map 1:1 to IPC error codes later; the
/// wording of human messages is a UI concern and does not live here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCategory {
    /// Access was denied (ACLs, TCC, EACCES/EPERM). Recoverable: skip + tally.
    PermissionDenied,
    /// The entry disappeared mid-scan (ENOENT and equivalents). Expected on
    /// live filesystems; never fatal.
    NotFound,
    /// The entry is locked/in use (Windows `ERROR_SHARING_VIOLATION` and kin).
    InUse,
    /// A link's target could not be resolved. Recorded, never fatal.
    BrokenLink,
    /// Metadata was readable in part but malformed/incomplete.
    MetadataUnavailable,
    /// The platform/filesystem does not support the operation (e.g. following
    /// a link with no stable identity under `SymlinkPolicy::Follow`).
    Unsupported,
    /// Transient IO condition (device busy, I/O error) that may succeed later.
    Transient,
    /// Anything not covered above; raw OS error is preserved when present.
    Other,
}

impl ErrorCategory {
    /// Stable IPC error code (the `code` field of `{ code, message, detail }`).
    pub fn code(self) -> &'static str {
        match self {
            ErrorCategory::PermissionDenied => "PERMISSION_DENIED",
            ErrorCategory::NotFound => "ENTRY_NOT_FOUND",
            ErrorCategory::InUse => "ENTRY_IN_USE",
            ErrorCategory::BrokenLink => "BROKEN_LINK",
            ErrorCategory::MetadataUnavailable => "METADATA_UNAVAILABLE",
            ErrorCategory::Unsupported => "OPERATION_UNSUPPORTED",
            ErrorCategory::Transient => "TRANSIENT_IO",
            ErrorCategory::Other => "SCAN_ENTRY_ERROR",
        }
    }
}

/// A concrete recoverable error observed while scanning one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub category: ErrorCategory,
    /// Path the error occurred on. Only kept for the bounded per-scan report;
    /// never logged wholesale (privacy: paths may be sensitive).
    pub path: PathBuf,
    /// Underlying OS message, preserved for diagnosis. Not shown to users by
    /// default; the UI uses `category.code()`.
    pub message: String,
    /// Raw OS error code when the platform provided one.
    pub raw_os: Option<i32>,
}

impl ScanError {
    pub fn new(category: ErrorCategory, path: PathBuf, err: &std::io::Error) -> Self {
        ScanError {
            category,
            path,
            message: err.to_string(),
            raw_os: err.raw_os_error(),
        }
    }
}

/// Bounded error report attached to a finished scan. The full set of errors
/// can be arbitrarily large on hostile trees, so the report keeps exact
/// per-category counts plus at most `max_detail` example errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanErrorReport {
    /// Exact count per category — never capped.
    pub counts: Vec<(ErrorCategory, u64)>,
    /// Up to `max_detail` example errors (bounded memory).
    pub detail: Vec<ScanError>,
}

impl ScanErrorReport {
    /// Detail cap. Generous enough for diagnosis, small enough that a
    /// pathological tree cannot balloon memory.
    pub const MAX_DETAIL: usize = 256;

    pub fn new() -> Self {
        ScanErrorReport {
            counts: Vec::new(),
            detail: Vec::new(),
        }
    }

    pub fn record(&mut self, err: ScanError) {
        match self.counts.iter_mut().find(|(c, _)| *c == err.category) {
            Some((_, n)) => *n += 1,
            None => self.counts.push((err.category, 1)),
        }
        if self.detail.len() < Self::MAX_DETAIL {
            self.detail.push(err);
        }
    }

    pub fn total_errors(&self) -> u64 {
        self.counts.iter().map(|c| c.1).sum()
    }
}

impl Default for ScanErrorReport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(ErrorCategory::PermissionDenied.code(), "PERMISSION_DENIED");
        assert_eq!(ErrorCategory::InUse.code(), "ENTRY_IN_USE");
        assert_eq!(ErrorCategory::NotFound.code(), "ENTRY_NOT_FOUND");
    }

    #[test]
    fn report_counts_exactly_but_caps_detail() {
        let mut report = ScanErrorReport::new();
        for i in 0..(ScanErrorReport::MAX_DETAIL as u64 + 50) {
            report.record(ScanError {
                category: if i % 2 == 0 {
                    ErrorCategory::PermissionDenied
                } else {
                    ErrorCategory::NotFound
                },
                path: PathBuf::from(format!("p{i}")),
                message: "m".into(),
                raw_os: None,
            });
        }
        assert_eq!(
            report.total_errors(),
            ScanErrorReport::MAX_DETAIL as u64 + 50
        );
        let denied = report
            .counts
            .iter()
            .find(|c| c.0 == ErrorCategory::PermissionDenied)
            .unwrap();
        assert_eq!(
            denied.1,
            (ScanErrorReport::MAX_DETAIL as u64 + 50).div_ceil(2)
        );
        assert_eq!(report.detail.len(), ScanErrorReport::MAX_DETAIL);
    }
}
