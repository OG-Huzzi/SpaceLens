//! Unix-specific pieces of the `StdFs` trait implementation.
//!
//! No `libc` dependency: Rust's std already maps per-OS errno values into
//! `io::ErrorKind`, including the granular variants (`FilesystemLoop`,
//! `NotADirectory`, …) stabilized well before this crate's MSRV. Raw-code
//! matching is deliberately avoided here because errno numbers diverge
//! between Linux and macOS.

use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::MetadataExt;

use super::MetadataInfo;
use crate::error::ErrorCategory;

pub(super) fn fill_platform_fields(md: &std::fs::Metadata, info: &mut MetadataInfo) {
    info.device = Some(md.dev());
    info.inode = Some(md.ino());
    // Allocated size: st_blocks is always in 512-byte units on all Unixes.
    info.allocated = Some(md.blocks().saturating_mul(512));
}

pub(super) fn categorize(err: &io::Error) -> ErrorCategory {
    match err.kind() {
        io::ErrorKind::PermissionDenied => ErrorCategory::PermissionDenied,
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => ErrorCategory::NotFound,
        io::ErrorKind::Interrupted => ErrorCategory::Transient,
        _ => {
            // EIO (5), EBUSY (16), EAGAIN (11) are identical across Linux,
            // macOS and the BSDs — the only raw codes we match on Unix.
            // (ELOOP is intentionally not matched here: its raw value
            // differs per OS and it cannot occur under lstat semantics;
            // a stray FilesystemLoop degrades to `Other`.)
            match err.raw_os_error() {
                Some(5) | Some(16) | Some(11) => ErrorCategory::Transient,
                _ => ErrorCategory::Other,
            }
        }
    }
}

pub(super) fn is_hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".")
}
