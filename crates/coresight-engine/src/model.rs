//! Typed filesystem metadata model.
//!
//! Every field the OS may not provide is an explicit `Option` — the model
//! never fabricates values. Sizes are `u64` (multi-terabyte safe); timestamps
//! are [`SystemTime`] to avoid lossy conversion this early in the pipeline.
//! Paths are stored as [`PathBuf`] and remain platform-correct.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// What a discovered entry is. Distinctions the platform layer can actually
/// prove are kept; everything else is [`EntryKind::Other`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Real directory (never a link).
    Dir,
    /// A link (symlink, Windows junction, or other reparse point) that was
    /// recorded but NOT recursed into. See `SymlinkPolicy`.
    Link(LinkInfo),
    /// Anything the platform reports that is not file/dir/link (sockets,
    /// FIFOs, device nodes, …).
    Other,
}

/// Link details recorded without recursion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    /// Link classification as far as the platform could determine it.
    pub kind: LinkKind,
    /// Link target if the platform could read it. `None` for broken links
    /// whose target is unreadable — this is an explicit state, not an error.
    pub target: Option<PathBuf>,
    /// True when the link is broken: the target could not be resolved.
    pub broken: bool,
}

/// Platform-level link classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    /// POSIX symbolic link.
    Symlink,
    /// Windows junction / mount point / any reparse point. The precise tag is
    /// a Phase-2+ refinement; behavior (never follow blindly) is identical.
    Reparse,
    /// The platform reports a link but cannot classify it further.
    Unknown,
}

/// One discovered filesystem entry, normalized.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    /// Scan-scoped sequential identifier (stable within one scan only).
    pub id: u64,
    /// [`FsEntry::id`] of the containing directory; `None` for the scan root.
    pub parent_id: Option<u64>,
    /// Full path, platform-correct. Treated as opaque data — never executed,
    /// never shell-interpolated.
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Logical size in bytes. Always `0` for directories.
    pub size: u64,
    /// On-disk allocated size where the platform reports it (sparse files,
    /// block-based filesystems). `None` = platform does not expose it.
    pub allocated_size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// Creation time where the platform provides it.
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Metadata-change timestamp (Unix `st_ctime`) where the scanner
    /// observed it. `None` on Windows path-stats (the NTFS ChangeTime is
    /// not exposed by std's stat surface) and on filesystems that do not
    /// maintain it. The identity layer compares this with the handle-proven
    /// change time to catch same-length rewrites that preserve mtime.
    pub changed: Option<SystemTime>,
    /// Filesystem/device identity where the platform provides it (Unix `st_dev`).
    pub device: Option<u64>,
    /// Inode/file identity where the platform provides it (Unix `st_ino`;
    /// Windows low 64 bits of the 128-bit file identifier — on NTFS the MFT
    /// record reference including its sequence number, so a freed-and-reused
    /// record produces a different identity). Captured through a
    /// query-only handle (Phase 3.2) on every supported platform.
    pub inode: Option<u64>,
    /// High 64 bits of a >64-bit file identifier (Windows `FILE_ID_INFO`,
    /// non-zero on ReFS-class filesystems). `None` = no wider identifier
    /// proven; the identity layer compares only what both sides proved.
    pub file_id_hi: Option<u64>,
    /// Platform-specific hidden determination (Windows `FILE_ATTRIBUTE_HIDDEN`,
    /// Unix dot-name convention).
    pub hidden: bool,
    /// Set when the entry was discovered but its metadata could not be read
    /// (e.g. vanished between listing and stat, or permission denied).
    /// The entry is still reported; the error is also tallied in the summary.
    pub error: Option<ErrorCategoryRef>,
}

/// Serializable reference to [`error::ErrorCategory`] used inside entries
/// without pulling the richer struct (which carries an OS message) into RAM
/// for every entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCategoryRef {
    PermissionDenied,
    NotFound,
    InUse,
    BrokenLink,
    MetadataUnavailable,
    Unsupported,
    Transient,
    Other,
}

impl From<crate::error::ErrorCategory> for ErrorCategoryRef {
    fn from(c: crate::error::ErrorCategory) -> Self {
        match c {
            crate::error::ErrorCategory::PermissionDenied => Self::PermissionDenied,
            crate::error::ErrorCategory::NotFound => Self::NotFound,
            crate::error::ErrorCategory::InUse => Self::InUse,
            crate::error::ErrorCategory::BrokenLink => Self::BrokenLink,
            crate::error::ErrorCategory::MetadataUnavailable => Self::MetadataUnavailable,
            crate::error::ErrorCategory::Unsupported => Self::Unsupported,
            crate::error::ErrorCategory::Transient => Self::Transient,
            crate::error::ErrorCategory::Other => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_sizes_survive_serialization() {
        // > 4 GiB logical size must round-trip exactly (u64, no f32/i32 traps).
        let entry = FsEntry {
            id: 7,
            parent_id: None,
            path: PathBuf::from("/fixture/huge.bin"),
            kind: EntryKind::File,
            size: 6_442_450_944, // 6 GiB + 1
            allocated_size: Some(1_073_741_824),
            modified: None,
            created: None,
            accessed: None,
            changed: None,
            device: None,
            inode: None,
            file_id_hi: None,
            hidden: false,
            error: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(
            json.contains("6442450944"),
            "size must not be truncated: {json}"
        );
        let back: FsEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.size, 6_442_450_944);
    }

    #[test]
    fn entries_serialize_camel_case() {
        let entry = FsEntry {
            id: 1,
            parent_id: Some(0),
            path: PathBuf::from("x"),
            kind: EntryKind::Link(LinkInfo {
                kind: LinkKind::Reparse,
                target: Some(PathBuf::from("t")),
                broken: false,
            }),
            size: 0,
            allocated_size: None,
            modified: None,
            created: None,
            accessed: None,
            changed: None,
            device: None,
            inode: None,
            file_id_hi: None,
            hidden: false,
            error: Some(ErrorCategoryRef::NotFound),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"allocatedSize\""));
        assert!(json.contains("\"NOT_FOUND\""));
        assert!(json.contains("\"reparse\""));
    }
}
