//! Windows-specific pieces of the `StdFs` trait implementation.

use std::fs;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Foundation::{ERROR_SYMLINK_NOT_SUPPORTED, FILETIME};
use windows_sys::Win32::Storage::FileSystem::{
    FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
    BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT,
};

use super::{ContentError, HandleStat, MetadataInfo};
use crate::error::ErrorCategory;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;
// Reparse-related open failures on some filesystem stacks (a path that
// cannot be opened as itself because the tag is unsupported).
const ERROR_INVALID_REPARSE_DATA: i32 = 4392;
const ERROR_REPARSE_POINT_ENCOUNTERED: i32 = 4395;

pub(super) fn fill_platform_fields(md: &std::fs::Metadata, info: &mut MetadataInfo) {
    use std::os::windows::fs::MetadataExt;
    let attrs = md.file_attributes();
    info.reparse = attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    info.hidden = attrs & FILE_ATTRIBUTE_HIDDEN != 0;
    // std does not expose allocated size / file index on stable for path
    // stats (the `windows_by_handle` extension is unstable); `None` is the
    // honest value. Handle-time identity IS available through
    // `GetFileInformationByHandle` and is used by the content boundary.
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

/// `FILE_FLAG_OPEN_REPARSE_POINT`: open the reparse point *itself*, never
/// traverse to its target. Combined with the immediate handle inspection
/// below, a path that became a symlink/junction/mount point after
/// observation is refused as [`ContentError::UnexpectedLink`] — the target
/// is never touched.
///
/// `FILE_FLAG_BACKUP_SEMANTICS` is required to open directory handles (a
/// replaced-by-directory path must be *typed* from handle attributes, not
/// guessed from an error code) and has no effect on regular-file opens.
pub(super) fn open_no_follow(path: &Path) -> Result<fs::File, ContentError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(map_open_err)?;
    inspect_handle(file)
}

/// Read BY_HANDLE information from the open handle and classify the object
/// it names: only a plain regular file (no reparse bit, no directory bit)
/// may be hashed. On a reparse handle `GetFileInformationByHandle` reports
/// the reparse point's own attributes — exactly what the inspection needs.
fn inspect_handle(file: fs::File) -> Result<fs::File, ContentError> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // BOOL FALSE == 0.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        // Cannot classify the open object: refuse it. Never guess.
        return Err(ContentError::OpenFailed(io::Error::other(
            "open refused: object at the observed path could not be classified",
        )));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ContentError::UnexpectedLink);
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(ContentError::NotRegularFile);
    }
    Ok(file)
}

/// Map Windows open failures onto typed content errors. A path that is now
/// a reparse point opens *successfully* with `OPEN_REPARSE_POINT` (that is
/// the point of the flag) — the refusal happens by handle inspection, not
/// by error code. The mappings below cover opens that fail with a
/// link-ish cause before inspection can run; everything else keeps its
/// native error.
fn map_open_err(e: io::Error) -> ContentError {
    const SYMLINK_NOT_SUPPORTED: i32 = ERROR_SYMLINK_NOT_SUPPORTED as i32;
    match e.raw_os_error() {
        Some(SYMLINK_NOT_SUPPORTED)
        | Some(ERROR_INVALID_REPARSE_DATA)
        | Some(ERROR_REPARSE_POINT_ENCOUNTERED) => ContentError::UnexpectedLink,
        _ => ContentError::OpenFailed(e),
    }
}

/// Handle-proven length + change timestamps. Length and mtime come from
/// BY_HANDLE_FILE_INFORMATION; the **NTFS change time** comes from
/// `GetFileInformationByHandleEx(FileBasicInfo)` — it moves on rewrites
/// (including same-length rewrites that preserve mtime) and cannot be set
/// directly from userspace, making it the strongest mid-read mutation
/// signal Windows offers. Filesystems that do not maintain it report 0,
/// honestly mapped to `None` (the mutation check then relies on the
/// length/mtime brackets — docs/IDENTITY.md records the limitation).
pub(super) fn handle_stat(file: &fs::File) -> io::Result<HandleStat> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        // Degrade honestly to std's handle stat.
        let md = file.metadata()?;
        return Ok(HandleStat {
            len: md.len(),
            modified: md.modified().ok(),
            changed: None,
        });
    }
    let mut changed = None;
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileBasicInfo,
            &mut basic as *mut FILE_BASIC_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if ok != 0 {
        changed = largeint_to_systemtime(basic.ChangeTime);
    }
    Ok(HandleStat {
        len: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        modified: filetime_to_systemtime(info.ftLastWriteTime),
        changed,
    })
}

/// Win32 FILETIME (100ns units since 1601-01-01) → `SystemTime`; `None`
/// when the OS recorded nothing (0) or the value overflows.
fn filetime_to_systemtime(ft: FILETIME) -> Option<SystemTime> {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    filetime_ticks_to_systemtime(ticks)
}

/// FILE_BASIC_INFO times are LARGE_INTEGER (100ns since 1601, may be 0).
fn largeint_to_systemtime(t: i64) -> Option<SystemTime> {
    if t <= 0 {
        return None;
    }
    filetime_ticks_to_systemtime(t as u64)
}

fn filetime_ticks_to_systemtime(ticks: u64) -> Option<SystemTime> {
    const TICKS_PER_SEC: u64 = 10_000_000;
    const EPOCH_SHIFT_SECS: u64 = 11_644_473_600; // 1601 → 1970
    let secs = ticks / TICKS_PER_SEC;
    let sub100ns = (ticks % TICKS_PER_SEC) as u32;
    let unix_secs = secs.checked_sub(EPOCH_SHIFT_SECS)?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(unix_secs, sub100ns * 100))
}
