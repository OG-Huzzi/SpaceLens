//! Windows-specific pieces of the `StdFs` trait implementation.
//!
//! Phase 3.2 adds **scan-time object identity**: a query-only handle
//! (`CreateFileW` with `FILE_READ_ATTRIBUTES`, share-all, opened
//! `OPEN_REPARSE_POINT` so a reparse point's own handle is inspected, never
//! its target) yields `FILE_ID_INFO` — the (volume serial, 128-bit file id)
//! pair that identifies the filesystem object rather than the path. On
//! NTFS the file id embeds the MFT record reference *including its sequence
//! number*, which increments every time a record is freed and reused — the
//! strongest reuse-resistant object identity Windows exposes to user mode.
//! Filesystems that do not support `FileIdInfo` fall back to the 64-bit
//! `BY_HANDLE_FILE_INFORMATION` index; where even that fails, identity
//! degrades honestly to `None` (never fabricated).

use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SYMLINK_NOT_SUPPORTED, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileBasicInfo, FileIdInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use super::{ContentError, HandleStat, MetadataInfo};
use crate::error::ErrorCategory;
use crate::identity::FileIdentity;

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
    // Allocated size is not exposed by std path-stats; the object identity
    // fields are filled by `identity_via_query_handle` (see `StdFs::metadata`).
    info.allocated = None;
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

/// Open a query-only handle on `path` and read the object identity from it.
///
/// `FILE_READ_ATTRIBUTES` (no data access, share-everything) succeeds on
/// files whose bytes are locked, and `FILE_FLAG_OPEN_REPARSE_POINT` means a
/// reparse point is opened *itself* — the scan observes the entry at the
/// path, never the link target (matching the scanner's record-only link
/// policy). `None` = the OS refused the query (ACL/ vanished/volume quirks):
/// the caller keeps `None` — identity is never fabricated.
pub(super) fn identity_via_query_handle(path: &Path) -> Option<FileIdentity> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a null-terminated UTF-16 path owned for the call;
    // no other parameter is retained. The returned handle (if any) is
    // closed on every path below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            INVALID_HANDLE_VALUE,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let identity = handle_identity_from(handle);
    // SAFETY: `handle` is a valid open handle owned by this call.
    unsafe { CloseHandle(handle) };
    identity
}

/// Object identity from an open raw HANDLE: `FILE_ID_INFO` first (128-bit
/// file id + 64-bit volume serial), falling back to the 64-bit
/// `BY_HANDLE_FILE_INFORMATION` index on filesystems that predate
/// `FileIdInfo`. The derivation is identical for scan-time and hash-time
/// identity, so comparisons are always like-for-like.
fn handle_identity_from(handle: HANDLE) -> Option<FileIdentity> {
    let mut id_info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    // BOOL FALSE == 0.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            &mut id_info as *mut FILE_ID_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok != 0 {
        let mut lo = [0u8; 8];
        let mut hi = [0u8; 8];
        lo.copy_from_slice(&id_info.FileId.Identifier[..8]);
        hi.copy_from_slice(&id_info.FileId.Identifier[8..]);
        let hi = u64::from_le_bytes(hi);
        return Some(FileIdentity {
            device: Some(id_info.VolumeSerialNumber),
            inode: Some(u64::from_le_bytes(lo)),
            file_id_hi: Some(hi),
            // FileIdInfo does not carry the link count; the hard-link
            // accounting uses the (volume, id) pair, so the count is not
            // needed for identity and is left unproven here.
            link_count: None,
        });
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    if ok == 0 {
        return None;
    }
    Some(FileIdentity {
        device: Some(u64::from(info.dwVolumeSerialNumber)),
        inode: Some((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)),
        file_id_hi: None,
        link_count: Some(u64::from(info.nNumberOfLinks)),
    })
}

/// Hash-time object identity from an already-open std file handle. Same
/// derivation as [`identity_via_query_handle`] — the identity layer
/// compares the two directly.
pub(super) fn handle_identity(file: &fs::File) -> io::Result<FileIdentity> {
    Ok(handle_identity_from(file.as_raw_handle() as _).unwrap_or_else(FileIdentity::unknown))
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
///
/// The intermediate-path guard is NOT folded into this open: it is
/// boundary-aware ([`validate_chain_below`], called through
/// `PlatformFs::validate_path_chain`) because components above the scan's
/// caller-chosen root may legitimately traverse OS-level reparse points
/// (profile-folder junctions, `/var`-style prefixes) and must not be
/// refused by the engine.
pub(super) fn open_no_follow(path: &Path) -> Result<fs::File, ContentError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(map_open_err)?;
    inspect_handle(file)
}

/// Phase 3.2 intermediate-path guard (Windows twin of the Unix
/// symlink-ancestor validation): every ancestor component **at or below the
/// `boundary`** must still be a plain directory. The boundary itself is
/// included — it is the deepest common ancestor of the run's staged
/// candidates, a directory the scanner observed, so a swap of it is as
/// hostile as a swap deeper down. Each ancestor is opened
/// `OPEN_REPARSE_POINT` (its own handle, never its target) and rejected on
/// the reparse attribute — deterministic, no error-code guessing. A
/// hostile junction/symlink ancestor is refused `UnexpectedLink`; an
/// ancestor that cannot be opened (ACL) is not treated as a link — the
/// final open and the identity comparison remain the gates.
pub(super) fn validate_chain_below(path: &Path, boundary: &Path) -> Result<(), ContentError> {
    let boundary_depth = component_depth(boundary);
    for ancestor in path.ancestors() {
        if component_depth(ancestor) < boundary_depth {
            break;
        }
        if ancestor == path {
            continue;
        }
        let wide: Vec<u16> = ancestor
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is a null-terminated UTF-16 path owned for the
        // call; the handle is closed on every path below.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                INVALID_HANDLE_VALUE,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // Unopenable ancestor (ACL, vanished): not evidence of a link.
            // The final open and the identity comparison remain the gates.
            continue;
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // BOOL FALSE == 0.
        let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
        // SAFETY: `handle` is a valid open handle owned by this call.
        unsafe { CloseHandle(handle) };
        if ok != 0 && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(ContentError::UnexpectedLink);
        }
    }
    Ok(())
}

/// Number of path components (a cheap depth measure for the boundary
/// comparison; consistent within one platform's path form).
fn component_depth(p: &Path) -> usize {
    p.components().count()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_time_identity_is_proven_for_regular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("identity.bin");
        std::fs::write(&p, b"identity-fixture").unwrap();
        let identity = identity_via_query_handle(&p)
            .expect("a plain tempdir file must yield object identity on Windows");
        let (volume, inode) = match (identity.device, identity.inode) {
            (Some(v), Some(i)) => (v, i),
            other => panic!("identity fields must be proven: {other:?}"),
        };
        assert_ne!(volume, 0, "volume serial is never zero for a real volume");
        // Same path, second query: the same live object must give the same
        // identity (stability over the scan/hash window).
        let again = identity_via_query_handle(&p).unwrap();
        assert_eq!((again.device, again.inode), (Some(volume), Some(inode)));
        if let (Some(h1), Some(h2)) = (identity.file_id_hi, again.file_id_hi) {
            assert_eq!(h1, h2, "high file-id bits must be stable");
        }
    }

    #[test]
    fn scan_time_identity_is_proven_for_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir-with-identity");
        std::fs::create_dir(&dir).unwrap();
        let identity = identity_via_query_handle(&dir)
            .expect("a plain directory must yield object identity on Windows");
        assert!(identity.device.is_some() && identity.inode.is_some());
        // Distinct objects must never share an identity.
        let file = tmp.path().join("a-file.bin");
        std::fs::write(&file, b"x").unwrap();
        let file_identity = identity_via_query_handle(&file).unwrap();
        assert_ne!(
            (identity.device, identity.inode),
            (file_identity.device, file_identity.inode),
            "a directory and a file are different objects"
        );
    }

    #[test]
    fn identity_distinguishes_two_files_and_matches_a_hard_link() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"two").unwrap();
        let ia = identity_via_query_handle(&a).unwrap();
        let ib = identity_via_query_handle(&b).unwrap();
        assert_ne!(
            (ia.device, ia.inode),
            (ib.device, ib.inode),
            "two distinct files must have distinct identities"
        );
        // Hard link: same object, same identity.
        let alias = tmp.path().join("alias.bin");
        if std::fs::hard_link(&a, &alias).is_err() {
            eprintln!("skipping: host refused hard-link creation");
            return;
        }
        let ialias = identity_via_query_handle(&alias).unwrap();
        assert_eq!(
            (ia.device, ia.inode),
            (ialias.device, ialias.inode),
            "a hard link is the same object: identical identity"
        );
    }

    #[test]
    fn query_handle_on_missing_path_is_none_not_fabricated() {
        let tmp = tempfile::tempdir().unwrap();
        let ghost = tmp.path().join("does-not-exist.bin");
        assert!(identity_via_query_handle(&ghost).is_none());
    }
}
