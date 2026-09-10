//! The eligibility contract — exactly what may be hashed (Phase 3 STEP 9).
//!
//! The hashing layer trusts the *observation*, never the name. An entry is
//! hashable only when the observer proved it a regular file with readable
//! metadata and no recorded error. Everything else is typed as ineligible
//! with a reason, so callers can see why — and so zero-byte handling,
//! hard-link semantics, and hostile inputs have one place to be decided.

use serde::{Deserialize, Serialize};

use spacelens_engine::{EntryKind, ErrorCategoryRef, FsEntry};

/// Why an entry is not a hashing candidate. Enumerated so tests can pin the
/// full contract; `Hash` failures are separate (a file may be eligible yet
/// fail during hashing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IneligibleReason {
    /// The entry is a real directory. Directories are never hashed.
    Directory,
    /// A symlink/junction/reparse point. Links are recorded, never followed
    /// (Phase 1 rule, preserved here): their content belongs to the target,
    /// which the observer either saw separately or saw not at all.
    Link,
    /// A special node (socket, FIFO, device, …). Not file content.
    Special,
    /// The observer could not fully stat the entry (`FsEntry::error`); its
    /// size — and therefore its candidacy — is unknown.
    ObservationError {
        #[serde(rename = "errorCategory")]
        category: ErrorCategoryRef,
    },
}

impl IneligibleReason {
    pub fn code(self) -> &'static str {
        match self {
            IneligibleReason::Directory => "DIRECTORY",
            IneligibleReason::Link => "LINK",
            IneligibleReason::Special => "SPECIAL_ENTRY",
            IneligibleReason::ObservationError { .. } => "OBSERVATION_ERROR",
        }
    }
}

/// The eligibility verdict for one entry. `Eligible` carries everything the
/// pipeline needs without re-reading the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// A regular file that may become a hashing candidate.
    Eligible,
    /// Not hashable, with the typed reason.
    Ineligible(IneligibleReason),
}

impl Eligibility {
    /// Classify one observed entry. Pure function — testable without a disk.
    pub fn of(entry: &FsEntry) -> Self {
        match &entry.kind {
            EntryKind::File => {
                if let Some(category) = entry.error {
                    // Unreliable metadata (size unknown or stale): never
                    // candidate. The failure is already tallied by the scan.
                    return Eligibility::Ineligible(IneligibleReason::ObservationError {
                        category,
                    });
                }
                Eligibility::Eligible
            }
            EntryKind::Dir => Eligibility::Ineligible(IneligibleReason::Directory),
            EntryKind::Link(_) => Eligibility::Ineligible(IneligibleReason::Link),
            EntryKind::Other => Eligibility::Ineligible(IneligibleReason::Special),
        }
    }
}

/// Tallies from the ingest stage. Bounded (fixed-width counters); enables
/// callers to verify "every entry accounted for" exactly once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibilityStats {
    /// Entries examined (every streamed entry passes through exactly once).
    pub examined: u64,
    pub eligible_files: u64,
    pub dirs: u64,
    pub links: u64,
    pub special: u64,
    pub observation_errors: u64,
    /// Eligible files whose size was zero (they become candidates only under
    /// the zero-byte policy).
    pub zero_byte_files: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacelens_engine::{LinkInfo, LinkKind};
    use std::path::PathBuf;

    fn entry(kind: EntryKind, error: Option<ErrorCategoryRef>) -> FsEntry {
        FsEntry {
            id: 1,
            parent_id: None,
            path: PathBuf::from("x"),
            kind,
            size: 0,
            allocated_size: None,
            modified: None,
            created: None,
            accessed: None,
            device: None,
            inode: None,
            hidden: false,
            error,
        }
    }

    #[test]
    fn only_clean_regular_files_are_eligible() {
        assert_eq!(
            Eligibility::of(&entry(EntryKind::File, None)),
            Eligibility::Eligible
        );

        assert_eq!(
            Eligibility::of(&entry(EntryKind::Dir, None)),
            Eligibility::Ineligible(IneligibleReason::Directory)
        );
        assert_eq!(
            Eligibility::of(&entry(
                EntryKind::Link(LinkInfo {
                    kind: LinkKind::Symlink,
                    target: None,
                    broken: false
                }),
                None
            )),
            Eligibility::Ineligible(IneligibleReason::Link)
        );
        assert_eq!(
            Eligibility::of(&entry(EntryKind::Other, None)),
            Eligibility::Ineligible(IneligibleReason::Special)
        );
        assert_eq!(
            Eligibility::of(&entry(
                EntryKind::File,
                Some(ErrorCategoryRef::PermissionDenied)
            )),
            Eligibility::Ineligible(IneligibleReason::ObservationError {
                category: ErrorCategoryRef::PermissionDenied
            })
        );
    }

    #[test]
    fn reason_codes_are_stable() {
        assert_eq!(IneligibleReason::Directory.code(), "DIRECTORY");
        assert_eq!(IneligibleReason::Link.code(), "LINK");
        assert_eq!(IneligibleReason::Special.code(), "SPECIAL_ENTRY");
        assert_eq!(
            IneligibleReason::ObservationError {
                category: ErrorCategoryRef::Other
            }
            .code(),
            "OBSERVATION_ERROR"
        );
    }
}
