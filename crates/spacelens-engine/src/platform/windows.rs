//! Windows-specific pieces of the `StdFs` trait implementation.

use std::io;

use super::MetadataInfo;
use crate::error::ErrorCategory;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

pub(super) fn fill_platform_fields(md: &std::fs::Metadata, info: &mut MetadataInfo) {
    use std::os::windows::fs::MetadataExt;
    let attrs = md.file_attributes();
    info.reparse = attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    info.hidden = attrs & FILE_ATTRIBUTE_HIDDEN != 0;
    // std does not expose allocated size / file index without extra FFI;
    // `None` is the honest value. Revisit with `GetFileInformationByHandle`
    // when app attribution (Phase 2+) needs file IDs.
    info.allocated = None;
    info.device = None;
    info.inode = None;
}

pub(super) fn categorize(err: &io::Error) -> ErrorCategory {
    match err.raw_os_error() {
        Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION) => ErrorCategory::InUse,
        _ => match err.kind() {
            io::ErrorKind::PermissionDenied => ErrorCategory::PermissionDenied,
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => ErrorCategory::NotFound,
            io::ErrorKind::Interrupted => ErrorCategory::Transient,
            _ => ErrorCategory::Other,
        },
    }
}

pub(super) fn is_hidden(md: &MetadataInfo) -> bool {
    md.hidden
}
