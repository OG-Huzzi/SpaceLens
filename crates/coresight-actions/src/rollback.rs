//! Failure-safe restoration of a partially completed relocation.
//!
//! This mirrors `coresight-history`'s store-quarantine discipline: a
//! multi-file move records every `(original, moved)` pair as it succeeds, and
//! a failure restores them in reverse order so the caller's error can promise
//! either "everything is back" or name precisely what is not.
//!
//! ## Restore never overwrites either
//!
//! A restore is a `rename` from the destination back to the original path. If
//! something now exists at the original path — the user recreated the file, a
//! backup restored it, a test planted a blocker — that entry is **not**
//! replaced. On Unix `rename` would silently overwrite it; the explicit
//! existence check before every restore is what prevents that. The restore
//! then fails and the entry is reported as unrestored, with its current
//! location, instead of being claimed as recovered.
//!
//! ## What is not guaranteed
//!
//! A restore can fail for reasons outside this crate's control: the
//! destination was removed by another process, the original directory was
//! deleted, permissions changed, the volumes were separated. Every one of
//! those becomes a [`RollbackFailure`] naming the path that is still out of
//! place. No failure is ever swallowed into a successful-looking result.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::trash::MovedFile;

/// One entry that should have been restored and was not.
///
/// The names are deliberately literal about location: `original` is the path
/// the entry came from and `quarantined` is where it is now (a trash
/// directory or a quarantine directory — the same field serves both because a
/// caller facing a half-restored set needs one answer: *where is my file?*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackFailure {
    /// The path the entry came from and could not be put back to.
    pub original: PathBuf,
    /// The path the entry occupies now.
    pub quarantined: PathBuf,
    /// Why the restore did not happen, in the filesystem's own words where
    /// there are any.
    pub detail: String,
}

/// The result of one reverse pass over the recorded moves.
#[derive(Debug)]
pub(crate) struct RollbackOutcome {
    /// How many entries are confirmed back at their original path.
    pub(crate) restored: usize,
    /// Every entry that is not.
    pub(crate) failures: Vec<RollbackFailure>,
}

impl RollbackOutcome {
    /// True when every recorded move was undone.
    pub(crate) fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Restore every recorded move, newest first.
///
/// Reverse order matters: the move order is main file, `-wal`, `-shm`, so the
/// reverse pass puts the sidecars back before the database whose name they
/// belong to. It also means a failure part-way leaves the set in the same
/// "prefix moved" state the caller can reason about.
pub(crate) fn restore_all(moved: &[MovedFile]) -> RollbackOutcome {
    let mut restored = 0usize;
    let mut failures = Vec::new();
    for record in moved.iter().rev() {
        match restore_one(record) {
            Ok(()) => restored += 1,
            Err(detail) => failures.push(RollbackFailure {
                original: record.source.clone(),
                quarantined: record.destination.clone(),
                detail,
            }),
        }
    }
    RollbackOutcome { restored, failures }
}

/// Put one entry back, refusing to overwrite anything that appeared at the
/// original path while it was away.
fn restore_one(record: &MovedFile) -> Result<(), String> {
    match fs::symlink_metadata(&record.source) {
        Ok(_) => {
            return Err(
                "an entry now exists at the original path, so it was not replaced".to_string(),
            )
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("the original path could not be inspected: {e}")),
    }
    fs::rename(&record.destination, &record.source)
        .map_err(|e| format!("the entry could not be moved back: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_move_set_restores_nothing_and_reports_nothing_missing() {
        let outcome = restore_all(&[]);
        assert_eq!(outcome.restored, 0);
        assert!(outcome.is_complete());
    }
}
