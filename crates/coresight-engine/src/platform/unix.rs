//! Unix-specific pieces of the `StdFs` trait implementation.
//!
//! Error categorization needs no `libc`: Rust's std already maps per-OS
//! errno values into `io::ErrorKind`, and raw-code matching is avoided
//! because errno numbers diverge between Linux and macOS.
//!
//! The Phase 3.1 **no-follow content open** is the one deliberate `libc`
//! binding: `O_NOFOLLOW`/`O_NONBLOCK`/`O_CLOEXEC` and `ELOOP`/`ENXIO` have
//! different raw values on every Unix flavor (Linux `O_NOFOLLOW` is
//! `0o400000`, macOS's is `0o100`) — hand-rolled constants would silently
//! apply the wrong flags on an unlisted target. `libc` is bindings-only
//! (no transitive dependencies, maintained by the Rust project) and is
//! used solely for these platform constants and errno names; no syscalls
//! are hand-rolled.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{ContentError, HandleStat, MetadataInfo};
use crate::error::ErrorCategory;

pub(super) fn fill_platform_fields(md: &fs::Metadata, info: &mut MetadataInfo) {
    info.device = Some(md.dev());
    info.inode = Some(md.ino());
    // Allocated size: st_blocks is always in 512-byte units on all Unixes.
    info.allocated = Some(md.blocks().saturating_mul(512));
    // Observation-side change time (st_ctime): twin of HandleStat::changed.
    info.changed = ctime_of(md);
}

pub(super) fn categorize(err: &io::Error) -> ErrorCategory {
    match err.kind() {
        io::ErrorKind::PermissionDenied => ErrorCategory::PermissionDenied,
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => ErrorCategory::NotFound,
        io::ErrorKind::Interrupted => ErrorCategory::Transient,
        _ => {
            // EIO (5), EBUSY (16), EAGAIN (11) are identical across Linux,
            // macOS and the BSDs — the only raw codes we match on Unix.
            // (ELOOP is handled by the no-follow open as UnexpectedLink and
            // cannot occur under lstat semantics; a stray FilesystemLoop
            // degrades to `Other`.)
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

/// Open the observed path with **no-follow** semantics (Phase 3.1 content
/// contract, docs/IDENTITY.md §link safety).
///
/// - `O_NOFOLLOW`: a final component that became a symlink after observation
///   is refused with `ELOOP` → [`ContentError::UnexpectedLink`]. The link's
///   target is never touched.
/// - `O_NONBLOCK`: a path that became a FIFO would otherwise block a hashing
///   worker forever on `open(O_RDONLY)`; with the flag the open succeeds
///   immediately and the fstat inspection below rejects it.
/// - `O_CLOEXEC`: hygiene; the engine spawns no processes.
///
/// The intermediate-path guard is NOT folded into this open: it is
/// boundary-aware ([`validate_chain_below`], called through
/// `PlatformFs::validate_path_chain`) because components above the scan's
/// caller-chosen root may legitimately traverse OS-level symlinks (macOS
/// `/var`, `/tmp` → `/private/tmp`) and must not be refused by the engine.
///
/// The observed-vs-opened `(st_dev, st_ino)` comparison in the identity
/// layer remains the authoritative proof that the hashed object is the
/// observed object; the chain guard is the deterministic structural check
/// that runs before it.
pub(super) fn open_no_follow(path: &Path) -> Result<fs::File, ContentError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(map_open_err)?;
    // Post-open inspection: only a regular file may be hashed. std's
    // `FileType::is_file` is S_ISREG. This also rejects a directory
    // (which Unix happily opens O_RDONLY).
    match file.metadata() {
        Ok(md) if md.file_type().is_file() => Ok(file),
        Ok(_) => Err(ContentError::NotRegularFile),
        Err(e) => Err(ContentError::OpenFailed(e)),
    }
}

/// Phase 3.2 intermediate-path guard (Unix twin of the Windows
/// reparse-ancestor validation): every ancestor component **at or below the
/// `boundary`** must still be a plain directory. The boundary itself is
/// included — it is the deepest common ancestor of the run's staged
/// candidates, a directory the scanner observed, so a swap of it is as
/// hostile as a swap deeper down. Detection is via `symlink_metadata` — the
/// link is named by the inode type (`S_ISLNK`) with no errno
/// interpretation involved — so the refusal is deterministic on every Unix
/// flavor. A hostile symlink ancestor is refused `UnexpectedLink`; other
/// ancestor failures (ACL, vanished) degrade to the final open's
/// authoritative error, and the final object's identity comparison remains
/// the authoritative proof.
pub(super) fn validate_chain_below(path: &Path, boundary: &Path) -> Result<(), ContentError> {
    let boundary_depth = component_depth(boundary);
    for ancestor in path.ancestors() {
        // The path itself has no ancestor role; everything from its parent
        // down to (and including) the boundary is validated.
        if component_depth(ancestor) < boundary_depth {
            break;
        }
        if ancestor == path {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(md) if md.file_type().is_symlink() => {
                return Err(ContentError::UnexpectedLink);
            }
            Ok(_) => {}
            // Ancestor unreadable (ACL/vanished): not evidence of a link.
            // The final open and the identity comparison remain the gates.
            Err(_) => continue,
        }
    }
    Ok(())
}

/// Number of path components (a cheap, allocation-free depth measure for
/// the boundary comparison; consistent within one platform's path form).
fn component_depth(p: &Path) -> usize {
    p.components().count()
}

/// Map no-follow open failures onto typed content errors.
fn map_open_err(e: io::Error) -> ContentError {
    if e.raw_os_error() == Some(libc::ELOOP) {
        return ContentError::UnexpectedLink;
    }
    if e.raw_os_error() == Some(libc::ENXIO) {
        // Opening a named socket (or an unattached device) yields ENXIO:
        // the object at the path is not the observed regular file.
        return ContentError::NotRegularFile;
    }
    ContentError::OpenFailed(e)
}

/// Handle-proven length + change timestamps (fstat on the open handle).
/// `st_ctime` is the inode change time: it moves on content rewrites *and*
/// when mtime is deliberately preserved (`utimensat` updates ctime), and
/// cannot be set independently from userspace — the strongest mid-read
/// mutation signal Unix offers.
pub(super) fn handle_stat(file: &fs::File) -> io::Result<HandleStat> {
    let md = file.metadata()?;
    Ok(HandleStat {
        len: md.size(),
        modified: md.modified().ok(),
        changed: ctime_of(&md),
    })
}

/// `MetadataExt::ctime` (seconds) + `ctime_nsec` → `SystemTime`, `None`
/// when the platform reports none or the value overflows.
fn ctime_of(md: &fs::Metadata) -> Option<SystemTime> {
    let secs = md.ctime();
    let nanos = md.ctime_nsec().clamp(0, 999_999_999) as u32;
    if secs >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos))
    } else {
        // Pre-epoch change times (clock skew): sub-second precision cannot
        // survive SystemTime arithmetic below the epoch — whole seconds
        // only, still exact enough for the equality brackets.
        SystemTime::UNIX_EPOCH.checked_sub(Duration::from_secs(secs.unsigned_abs()))
    }
}
