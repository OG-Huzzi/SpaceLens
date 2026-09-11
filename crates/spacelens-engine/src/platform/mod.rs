//! Platform abstraction: the trait boundary between the shared scanner and
//! per-OS behavior (docs/CROSS_PLATFORM.md).
//!
//! Shared scanner code is generic over these traits and NEVER branches on the
//! target OS; only the implementations in this module tree do. Names follow
//! the Phase-0 contract: `PlatformFs`, `DriveInfo`, `SysDirs` (the `Trash`
//! trait belongs to the cleanup phases and is intentionally absent here).
//!
//! Phase 3.1 tightens the **content-access contract** (see
//! [`PlatformFs::read_content`]): opens are no-follow, handles prove their
//! own object identity and mutable state, and the caller's observed-vs-opened
//! and pre/post-read consistency checks run on *handle-proven* facts — never
//! on a second path resolution that could describe a different object.

mod drives;
mod std_impl;

pub use drives::{drive_info, sys_dirs};
pub use std_impl::{std_fs, StdFs};

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::ErrorCategory;
use crate::identity::FileIdentity;

/// Handle-proven mutable state of the open file object, taken from the open
/// handle itself (fstat semantics) — not from a second path lookup.
///
/// Used by the identity layer to bracket a content read: the length and
/// change timestamps observed before and after the read must agree, or the
/// digest is not proven to describe one stable state of the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleStat {
    /// Logical length in bytes (`st_size` / `EndOfFile`).
    pub len: u64,
    /// Modification time where the OS reports one for the object.
    /// `None` is honest: some filesystems do not maintain it.
    pub modified: Option<SystemTime>,
    /// Metadata-change timestamp (Unix `st_ctime`; Windows
    /// `ChangeTime` via `GetFileInformationByHandle`) where available.
    /// Distinguishes a rewrite that preserves mtime from a genuine
    /// unchanged object on platforms that maintain it.
    pub changed: Option<SystemTime>,
}

/// Streaming reader handed to [`PlatformFs::read_content`] callbacks.
///
/// The implementation reads bounded chunks from one OS file handle opened
/// with no-follow semantics. Every method describes **that handle's object**
/// — the same object for the whole callback. A read is retryable exactly when
/// it yields `Interrupted`; any other `Error` is terminal for that file. The
/// consumer must not retain the borrowed chunk across calls.
pub trait ContentReader {
    /// Read the next bounded chunk. Returns `Ok(None)` at end of file.
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>>;

    /// Identity of the underlying file object, proven from the open handle.
    /// This describes the object the bytes are actually read from — the only
    /// trustworthy identity in Phase 3.1 (a path may have been swapped
    /// between observation and open).
    fn file_identity(&self) -> FileIdentity;

    /// Handle-proven mutable state (length + change timestamps), taken
    /// BEFORE the first byte is read. Never a path stat.
    fn pre_stat(&self) -> io::Result<HandleStat>;

    /// Handle-proven mutable state, taken AFTER the last byte was read.
    /// Comparing against [`Self::pre_stat`] brackets the content read: any
    /// disagreement means the bytes hashed did not describe one stable
    /// state of the object.
    fn post_stat(&self) -> io::Result<HandleStat>;

    /// Raw file length as the OS reports it for the open handle.
    fn file_len(&self) -> io::Result<u64> {
        Ok(self.pre_stat()?.len)
    }
}

/// Outcome of [`PlatformFs::read_content`]. The error variant preserves the
/// native error so the platform's own categorization (sharing violation,
/// permission denied, vanished file, …) applies unchanged.
///
/// Callback failures use a separate signal so the caller can distinguish
/// "the file could not be read" from "the consumer stopped the read".
pub enum ContentOutcome<'a> {
    /// The handle was opened successfully; drive the read through `feed`.
    /// Returning `Err` from the callback aborts and yields
    /// [`ContentError::Aborted`].
    Opened(&'a mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>),
    /// The handle could not be opened.
    Failed(io::Error),
}

/// Why [`PlatformFs::read_content`] did not deliver content. The error
/// variants preserve the native error so the platform's own categorization
/// (sharing violation, permission denied, vanished file, …) applies
/// unchanged.
///
/// Callback failures use a separate signal so the caller can distinguish
/// "the file could not be read" from "the consumer stopped the read".
#[derive(Debug)]
pub enum ContentError {
    /// The handle could not be opened (preserves the original error).
    OpenFailed(io::Error),
    /// The open resolved to a link/reparse point where a regular file was
    /// required (no-follow contract). **No target was opened and no content
    /// was touched** — the original `openat(..., O_NOFOLLOW)`/Windows
    /// no-reparse open refused the traversal, so a hostile swap of the
    /// observed path cannot reach its target through this boundary.
    UnexpectedLink,
    /// The open resolved to a **non-regular, non-link object** (FIFO,
    /// socket, device node, directory) where a regular file was observed.
    /// Caught by post-open handle inspection (fstat / handle attributes) —
    /// the kind changed under the observed path.
    NotRegularFile,
    /// A read failed mid-stream (preserves the original error).
    ReadFailed(io::Error),
    /// The consumer's callback aborted the read deliberately (e.g.
    /// cancellation). Never an OS failure.
    Aborted,
}

impl From<ContentError> for io::Error {
    fn from(e: ContentError) -> Self {
        match e {
            ContentError::OpenFailed(err) | ContentError::ReadFailed(err) => err,
            ContentError::UnexpectedLink => io::Error::other(
                "open refused: path resolved to a link where a regular file was required",
            ),
            ContentError::NotRegularFile => io::Error::other(
                "open refused: path resolved to a non-regular object where a file was observed",
            ),
            ContentError::Aborted => io::Error::new(
                io::ErrorKind::Interrupted,
                "content read aborted by consumer",
            ),
        }
    }
}

impl ContentError {
    pub fn message(&self) -> String {
        match self {
            ContentError::OpenFailed(err) => err.to_string(),
            ContentError::UnexpectedLink => {
                "open refused: path is now a link (no-follow contract)".to_string()
            }
            ContentError::NotRegularFile => {
                "open refused: path is no longer a regular file".to_string()
            }
            ContentError::ReadFailed(err) => err.to_string(),
            ContentError::Aborted => "aborted by consumer".to_string(),
        }
    }
}

/// File kind as reported by the platform metadata call (link-aware: a
/// symlink/junction is `Symlink`, never `Dir`/`File`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// Raw metadata as the platform reports it. Fields the OS does not provide
/// stay `None`; the engine never fabricates values.
#[derive(Debug, Clone)]
pub struct MetadataInfo {
    pub kind: FsKind,
    /// Logical size (files). Directories report the platform's value; the
    /// engine forces directory logical size to 0 in the normalized model.
    pub size: u64,
    /// Allocated size (blocks × block size, sparse-aware) where available.
    pub allocated: Option<u64>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Metadata-change timestamp (Unix `st_ctime`) where the platform
    /// provides it from a **path stat**. Windows path-stats cannot read the
    /// NTFS ChangeTime, so it stays `None` there — honest, never fabricated.
    /// This is the observation-side twin of [`HandleStat::changed`]; the
    /// identity layer compares the two to catch same-length rewrites that
    /// preserve mtime.
    pub changed: Option<SystemTime>,
    /// Device / volume identity (Unix `st_dev`; Windows volume serial
    /// number). Phase 3.2: Windows scan-time identity is captured through a
    /// query-only handle (`FILE_ID_INFO`), so it is provable on all
    /// supported platforms. `None` where the OS refused the query — never
    /// fabricated.
    pub device: Option<u64>,
    /// File identity (Unix `st_ino`; Windows low 64 bits of the 128-bit
    /// `FILE_ID_INFO.FileId` — on NTFS the MFT record reference including
    /// its sequence number). `None` = not provable.
    pub inode: Option<u64>,
    /// High 64 bits of a >64-bit file identifier (Windows `FILE_ID_INFO`
    /// on ReFS-class filesystems). `None` = no wider identifier proven.
    pub file_id_hi: Option<u64>,
    /// Windows reparse-point flag (junctions, symlinks, mount points).
    pub reparse: bool,
    /// Windows `FILE_ATTRIBUTE_HIDDEN` bit; always `false` on other OSes
    /// (Unix hidden detection is name-based and lives in `is_hidden`).
    pub hidden: bool,
}

/// A child as observed during directory listing (no extra stat required).
#[derive(Debug, Clone)]
pub struct ChildInfo {
    pub name: OsString,
    pub is_dir: bool,
    pub is_symlink: bool,
}

/// Filesystem operations the scanner needs, per OS.
///
/// `metadata` has link-aware ("lstat") semantics: it describes the entry
/// itself, never its target — the scanner decides recursion, not the OS.
pub trait PlatformFs: Send + Sync {
    /// List the children of `dir`, invoking `visit` for each child as it is
    /// streamed out of the OS. Returning `false` from `visit` stops early
    /// (cancellation). An `Err` means the directory itself is unreadable.
    ///
    /// Implementations must stream: they must not buffer the whole directory
    /// before calling `visit` (a directory may hold a million entries).
    fn read_dir_entries(
        &self,
        dir: &Path,
        visit: &mut dyn FnMut(ChildInfo) -> bool,
    ) -> io::Result<()>;

    /// Link-aware metadata for one entry.
    fn metadata(&self, path: &Path) -> io::Result<MetadataInfo>;

    /// Read a link's target path. Does not resolve the target's existence.
    fn read_link_target(&self, path: &Path) -> io::Result<PathBuf>;

    /// Map an OS error onto a stable [`ErrorCategory`]. This is platform
    /// behavior (raw code meanings differ per OS) and lives behind the trait.
    fn categorize_error(&self, err: &io::Error) -> ErrorCategory;

    /// Platform hidden-entry rule (Windows attributes vs Unix dot-names).
    fn is_hidden(&self, name: &OsStr, md: &MetadataInfo) -> bool;

    /// Open the file at `path` for bounded streaming reads and call `outcome`
    /// with the result. This is Phase 3's single content-access boundary:
    /// the hashing layer consumes observed entries and never crawls the
    /// filesystem itself, so content access must flow through the platform
    /// abstraction like every other operation.
    ///
    /// **Phase 3.1 contract (link-safety):** implementations MUST open with
    /// metadata-following disabled — Unix `openat(parent, name, O_NOFOLLOW)`
    /// (the parent directory is opened `O_PATH`, so only the final component
    /// is re-resolved, and it is re-resolved against refusal); Windows
    /// `FILE_FLAG_OPEN_REPARSE_POINT` (a reparse point is opened *itself*,
    /// never traversed to its target; the handle is then inspected with
    /// `GetFileInformationByHandle` and rejected unless it is a plain
    /// regular file). A path that became a link/reparse point after
    /// observation fails with [`ContentError::UnexpectedLink`]; no content
    /// beyond the link is ever touched. The race that remains on the final
    /// component (path swapped to a *different regular file* after the
    /// no-follow open) is closed by the identity layer comparing
    /// observed-time object identity with handle identity where the
    /// platform provides it (Unix `st_dev`/`st_ino`; Windows scan-time
    /// identity is honestly unavailable — std's path-stat surface does not
    /// expose it — and the comparison degrades to length + change-time
    /// brackets).
    ///
    /// **Handle-proven facts only:** the [`ContentReader`] handed to the
    /// consumer exposes identity and pre/post state from the open handle —
    /// never a second path resolution. Implementations stream in bounded
    /// chunks without buffering whole files, and never spawn processes,
    /// touch the network, or traverse directories.
    fn read_content(&self, path: &Path, outcome: ContentOutcome<'_>) -> Result<(), ContentError> {
        // Default: no content access. The engine core is usable without it;
        // platforms that cannot read content degrade explicitly.
        let _ = (path, outcome);
        Err(ContentError::OpenFailed(io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform implementation does not expose file content",
        )))
    }
}

/// One volume / mount the platform exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Stable volume identity where the platform provides one (Windows
    /// volume serial number; `None` elsewhere in Phase 1).
    pub id: Option<String>,
    /// Root / mount path.
    pub root: PathBuf,
    pub label: Option<String>,
    pub fs_type: Option<String>,
    pub kind: VolumeKind,
    pub capacity: Option<u64>,
    pub available: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    Internal,
    Removable,
    Network,
    Unknown,
}

/// Drive / volume identification foundation (docs §18). No UI, no automatic
/// scanning — callers decide what to do with the list.
pub trait DriveInfo: Send + Sync {
    /// Volumes currently visible to the platform. Empty/erroneous entries are
    /// skipped; a volume that fails its capacity query is still listed with
    /// `capacity: None`.
    fn list_volumes(&self) -> io::Result<Vec<VolumeInfo>>;
}

/// OS standard directory locations the engine may need (temp fixtures, app
/// data placement decisions made by the shell, not the engine).
pub trait SysDirs: Send + Sync {
    fn home(&self) -> Option<PathBuf>;
    fn temp(&self) -> PathBuf;
}
