//! Platform abstraction: the trait boundary between the shared scanner and
//! per-OS behavior (docs/CROSS_PLATFORM.md).
//!
//! Shared scanner code is generic over these traits and NEVER branches on the
//! target OS; only the implementations in this module tree do. Names follow
//! the Phase-0 contract: `PlatformFs`, `DriveInfo`, `SysDirs` (the `Trash`
//! trait belongs to the cleanup phases and is intentionally absent here).

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

/// Streaming reader handed to [`PlatformFs::read_content`] callbacks.
///
/// The implementation reads bounded chunks from an OS file handle. A read is
/// retryable exactly when it yields [`ReadChunk::Interrupted`]; any `Error`
/// is terminal for that file. The consumer must not retain the borrowed
/// chunk across calls.
pub trait ContentReader {
    /// Read the next bounded chunk. Returns `Ok(None)` at end of file.
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>>;

    /// Identity of the underlying file object, proven from the open handle
    /// while the content is being read. Prefer this over any pre-open stat:
    /// it describes the object the bytes actually came from.
    fn file_identity(&self) -> FileIdentity;

    /// Raw file length as the OS reports it for the open handle.
    fn file_len(&self) -> io::Result<u64>;
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

/// Why [`PlatformFs::read_content`] did not deliver content.
#[derive(Debug)]
pub enum ContentError {
    /// The handle could not be opened (preserves the original error).
    OpenFailed(io::Error),
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
    /// Device / volume identity (Unix `st_dev`).
    pub device: Option<u64>,
    /// File identity (Unix `st_ino`).
    pub inode: Option<u64>,
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
    /// Implementations must open with metadata-following disabled or proved
    /// equivalent (the entry was observed as [`crate::model::EntryKind::File`]
    /// at scan time), stream in bounded chunks without buffering whole files,
    /// and never spawn processes, touch the network, or traverse directories.
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
