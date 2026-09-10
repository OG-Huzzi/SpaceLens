//! Typed hashing failures — Phase 3's per-file error semantics.
//!
//! A file that cannot be hashed is never an empty hash and never a silent
//! skip: it becomes a [`HashFailure`] carried in the report, with a
//! platform-categorized [`HashFailureKind`]. Failed hashing never creates or
//! destroys a duplicate relationship — the file is simply absent from
//! grouping (docs/IDENTITY.md §error semantics).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use spacelens_engine::ErrorCategory;

/// What kind of failure produced a [`HashFailure`]. `Hash` errors are
/// engine-categorized (permission, sharing violation, vanished, …);
/// `Policy` errors are the pipeline's own contract rejections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum HashFailureKind {
    /// The file could not be opened or read; `ErrorCategory` on the failure
    /// carries the platform's typed cause (permission denied, in use, …).
    Hash { category: ErrorCategory },
    /// The file's kind changed between observation and hashing (e.g. it was
    /// replaced by a directory or a link). Never hashed through the new type.
    Changed,
    /// The file vanished between observation and hashing (or mid-hash, per
    /// the mutation policy).
    Vanished,
    /// The content read was cancelled by the consumer.
    Cancelled,
    /// The platform implementation does not expose content access.
    Unsupported,
}

/// One file's hashing failure: path + typed kind + human-readable message
/// (preserved OS message where present). Paths appear here for operator
/// diagnosis only — the detail list is capped for bounded memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashFailure {
    pub path: PathBuf,
    pub kind: HashFailureKind,
    pub message: String,
}

impl HashFailure {
    pub(crate) fn new(path: PathBuf, kind: HashFailureKind, message: impl Into<String>) -> Self {
        HashFailure {
            path,
            kind,
            message: message.into(),
        }
    }
}

/// Crate-local error type for hashing a single file.
#[derive(Debug)]
pub enum HashError {
    /// Typed failure to record for this file (pipeline continues).
    Failure(HashFailure),
    /// The consumer requested cancellation; the pipeline unwinds and reports
    /// a `Cancelled` terminal state. Never recorded as a file failure.
    Cancelled,
}

impl From<HashFailure> for HashError {
    fn from(f: HashFailure) -> Self {
        HashError::Failure(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_kinds_serialize_with_tag() {
        let f = HashFailure::new(
            PathBuf::from("p"),
            HashFailureKind::Hash {
                category: ErrorCategory::PermissionDenied,
            },
            "denied",
        );
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"kind\":\"hash\""), "{json}");
        assert!(
            json.contains("\"category\":\"PERMISSION_DENIED\""),
            "{json}"
        );
    }

    #[test]
    fn kinds_are_distinguishable() {
        assert_ne!(
            HashFailureKind::Changed,
            HashFailureKind::Vanished,
            "a policy-rejected mutation must never be reported as a vanish"
        );
    }
}
