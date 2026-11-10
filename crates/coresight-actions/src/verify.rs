//! Post-execution verification: what actually arrived, and how big it is.
//!
//! ## Why a separate step
//!
//! [`ExecutionReport::freed_bytes`](crate::ExecutionReport::freed_bytes) is
//! measured at the sources *before* each rename. It is the executor's claim
//! about what it moved, and a claim is not evidence. This module re-stats each
//! recorded destination and answers three separate questions:
//!
//! * does the destination exist? ([`VerificationReport::missing`])
//! * does its size still match what was recorded? ([`VerificationReport::size_mismatches`])
//! * how many entries pass both checks, and how many bytes do they account for?
//!   ([`VerificationReport::verified`], [`VerificationReport::verified_bytes`])
//!
//! **Only `verified_bytes` may be presented to a user as "these bytes were
//! moved".** A record that is missing or has the wrong size is excluded from
//! it. A report that is not complete must be shown as incomplete — the caller
//! decides the wording, but there is no API here that turns a failure into a
//! success.
//!
//! ## Limits
//!
//! * Verification does not prove the **source** is gone; it proves the
//!   destination holds the recorded bytes. (The executor's only move
//!   primitive is `rename`, which removes the source name in the same
//!   operation, but a caller that wants that postcondition must stat the
//!   source itself.)
//! * A directory's size is the recursive total of its regular files, measured
//!   best effort; an unreadable subtree contributes zero on both sides, so a
//!   permission change deep inside a moved directory is not detected.
//! * Verification is a point-in-time observation. A file can change the
//!   instant after it is measured; that is why the report carries the numbers
//!   it compared rather than a boolean.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::execute::ExecutionReport;
use crate::trash::measure_entry;

/// The result of re-checking every moved entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    /// How many entries exist at their destination with the recorded size.
    pub verified: usize,
    /// Destinations that do not exist (or cannot be inspected).
    pub missing: Vec<PathBuf>,
    /// Destinations whose size differs from the record:
    /// `(destination, recorded, observed)`.
    pub size_mismatches: Vec<(PathBuf, u64, u64)>,
    /// The bytes accounted for by the entries in `verified` only.
    ///
    /// This is the only byte total that may be called verified. It is zero for
    /// a fully missing set, and it never includes a mismatch.
    pub verified_bytes: u64,
}

impl VerificationReport {
    /// True when every recorded entry was found at its destination with the
    /// recorded size.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.size_mismatches.is_empty()
    }
}

/// Re-stat every destination in `report` and compare it with the record.
///
/// Never panics, never aborts on the first problem: a caller needs the whole
/// picture, not the first missing file.
pub fn verify(report: &ExecutionReport) -> VerificationReport {
    let mut verified = 0usize;
    let mut missing = Vec::new();
    let mut size_mismatches = Vec::new();
    let mut verified_bytes = 0u64;

    for record in &report.moved {
        match measure_entry(&record.destination) {
            Ok(observed) if observed == record.bytes => {
                verified += 1;
                verified_bytes = verified_bytes.saturating_add(observed);
            }
            Ok(observed) => {
                size_mismatches.push((record.destination.clone(), record.bytes, observed));
            }
            Err(_) => missing.push(record.destination.clone()),
        }
    }

    VerificationReport {
        verified,
        missing,
        size_mismatches,
        verified_bytes,
    }
}
