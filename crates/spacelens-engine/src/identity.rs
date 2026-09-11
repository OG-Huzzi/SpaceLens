//! File identity — which filesystem object a path refers to.
//!
//! Distinct from **content identity** (Phase 3, `spacelens-identity`): two
//! paths can refer to the *same file object* (hard links) while two different
//! objects can hold identical bytes. Both concepts are needed; conflating
//! them would double-count hard-linked storage (docs/IDENTITY.md).
//!
//! Wherever the platform cannot prove the fields, they are `None` — the model
//! never fabricates identity.

use serde::{Deserialize, Serialize};

/// Identity of the underlying file object, as proven by the OS while a
/// handle to it is open. `nlink` counts the paths referring to the object.
///
/// Windows identity is `(volume serial, 128-bit file id)`: `inode` carries
/// the low 64 bits (on NTFS the MFT record reference *including* the
/// sequence number, which increments on record reuse) and `file_id_hi` the
/// high 64 bits (non-zero on ReFS-class filesystems). Unix identity is
/// `(st_dev, st_ino)` with `file_id_hi = None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    /// Volume/filesystem identity (Unix `st_dev`, Windows volume serial
    /// number). `None` = not provable.
    pub device: Option<u64>,
    /// File identity within the volume (Unix `st_ino`, Windows low 64 bits
    /// of the `FILE_ID_INFO.FileId` 128-bit identifier / the 64-bit file
    /// index). `None` = not provable.
    pub inode: Option<u64>,
    /// High 64 bits of a >64-bit file identifier (Windows `FILE_ID_INFO`
    /// on ReFS-class filesystems). `None` = the platform proved no wider
    /// identifier — never fabricated.
    pub file_id_hi: Option<u64>,
    /// Number of directory entries referring to this object (Unix `st_nlink`,
    /// Windows `nNumberOfLinks`). `None` = not provable.
    pub link_count: Option<u64>,
}

impl FileIdentity {
    /// The identity when the platform could prove nothing.
    pub fn unknown() -> Self {
        FileIdentity {
            device: None,
            inode: None,
            file_id_hi: None,
            link_count: None,
        }
    }

    /// `Some` only when the platform proved at least the (device, inode) pair.
    pub fn is_stable(&self) -> bool {
        self.device.is_some() && self.inode.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stability_requires_both_fields() {
        assert!(FileIdentity {
            device: Some(1),
            inode: Some(2),
            file_id_hi: None,
            link_count: Some(1)
        }
        .is_stable());
        assert!(!FileIdentity {
            device: None,
            inode: Some(2),
            file_id_hi: Some(0),
            link_count: None
        }
        .is_stable());
        assert!(!FileIdentity {
            device: Some(1),
            inode: None,
            file_id_hi: None,
            link_count: None
        }
        .is_stable());
        assert_eq!(
            FileIdentity::unknown(),
            FileIdentity {
                device: None,
                inode: None,
                file_id_hi: None,
                link_count: None,
            }
        );
    }
}
