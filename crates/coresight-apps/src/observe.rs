//! Typed, read-only filesystem observation for the application-intelligence
//! layer (Phase 6.2).
//!
//! Every filesystem read the layer performs goes through
//! [`crate::footprint::PathProber`]; this module defines the *facts* those
//! reads produce and the production prober ([`PlatformPathProber`]) built on
//! the engine's established [`PlatformFs`] boundary (so no second
//! filesystem abstraction exists, and the engine's no-follow content
//! contract applies unchanged).
//!
//! ## Honesty (Phase 6.1 rules preserved)
//!
//! Each observation carries an explicit [`AccessState`]:
//!
//! * `denied != empty` — a denied directory is
//!   [`AccessState::ExistsButInaccessible`], never an empty listing;
//! * a payload (`entries` / `bytes`) exists **only** when the state is a
//!   completed read ([`AccessState::is_read`]); every other state carries a
//!   note and no payload (constructors enforce this; see
//!   [`DirectoryObservation::is_well_formed`]);
//! * a prober that cannot service a request reports
//!   [`AccessState::Unsupported`] — never an empty success.
//!
//! ## Paths
//!
//! Paths stay [`PathBuf`] end to end. Canonical ordering is by the platform's
//! encoded bytes ([`PathKey`]) — never a lossy `to_string_lossy` conversion.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

use coresight_capabilities::access::AccessState;
use coresight_engine::platform::{ContentError, ContentOutcome, FsKind, PlatformFs};
use coresight_identity::ObjectIdentity;
use serde::{Deserialize, Serialize};

use crate::bounded::BoundedTopK;
use crate::footprint::{BoundedListing, PathProber};

/// A path with the canonical total order used everywhere in this crate:
/// the platform-encoded bytes. Lossless and platform-stable within a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PathKey(pub PathBuf);

impl PathKey {
    pub fn bytes(&self) -> &[u8] {
        self.0.as_os_str().as_encoded_bytes()
    }
}

impl PartialOrd for PathKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bytes().cmp(other.bytes())
    }
}

/// Link-aware kind of one filesystem entry (a link is never a `Dir`/`File`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProbedKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl From<FsKind> for ProbedKind {
    fn from(k: FsKind) -> Self {
        match k {
            FsKind::File => ProbedKind::File,
            FsKind::Dir => ProbedKind::Dir,
            FsKind::Symlink => ProbedKind::Symlink,
            FsKind::Other => ProbedKind::Other,
        }
    }
}

/// One entry seen in a directory listing (name-level facts only; identity
/// is observed on demand with [`PathProber::stat`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedEntry {
    pub path: PathBuf,
    pub kind: ProbedKind,
}

/// A bounded directory listing with an explicit access state.
///
/// Invariants (see [`Self::is_well_formed`]): `entries` is non-empty only
/// for [`AccessState::ReadSucceeded`]; [`AccessState::Empty`] has no
/// entries and zero overflow; every non-read state has no entries and no
/// overflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryObservation {
    pub access: AccessState,
    /// The canonically-smallest entries (≤ the requested max), ascending.
    pub entries: Vec<ListedEntry>,
    /// Entries visited beyond the kept set (exact). Memory is O(max).
    pub overflow: u64,
    pub note: Option<String>,
}

impl DirectoryObservation {
    /// A completed read. `Empty` iff nothing was seen at all.
    pub fn read(entries: Vec<ListedEntry>, overflow: u64) -> Self {
        let access = if entries.is_empty() && overflow == 0 {
            AccessState::Empty
        } else {
            AccessState::ReadSucceeded
        };
        DirectoryObservation {
            access,
            entries,
            overflow,
            note: None,
        }
    }

    fn without_payload(access: AccessState, note: impl Into<String>) -> Self {
        DirectoryObservation {
            access,
            entries: Vec::new(),
            overflow: 0,
            note: Some(note.into()),
        }
    }

    pub fn does_not_exist() -> Self {
        DirectoryObservation {
            access: AccessState::DoesNotExist,
            entries: Vec::new(),
            overflow: 0,
            note: None,
        }
    }

    pub fn inaccessible(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::ExistsButInaccessible, note)
    }

    pub fn failed(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::Failed, note)
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::Unsupported, note)
    }

    /// The observation invariant: a non-read state cannot contain payload.
    pub fn is_well_formed(&self) -> bool {
        match self.access {
            AccessState::ReadSucceeded => true,
            AccessState::Empty => self.entries.is_empty() && self.overflow == 0,
            _ => self.entries.is_empty() && self.overflow == 0,
        }
    }
}

/// Metadata of one path (identity is the canonical [`ObjectIdentity`], with
/// its high bits, or `None` when the platform could not prove it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathObservation {
    pub access: AccessState,
    pub kind: Option<ProbedKind>,
    pub identity: Option<ObjectIdentity>,
    pub size: Option<u64>,
    pub note: Option<String>,
}

impl PathObservation {
    pub fn present(kind: ProbedKind, identity: Option<ObjectIdentity>, size: Option<u64>) -> Self {
        PathObservation {
            access: AccessState::ReadSucceeded,
            kind: Some(kind),
            identity,
            size,
            note: None,
        }
    }

    fn without_payload(access: AccessState, note: Option<String>) -> Self {
        PathObservation {
            access,
            kind: None,
            identity: None,
            size: None,
            note,
        }
    }

    pub fn does_not_exist() -> Self {
        Self::without_payload(AccessState::DoesNotExist, None)
    }

    pub fn failed(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::Failed, Some(note.into()))
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::Unsupported, Some(note.into()))
    }

    pub fn is_well_formed(&self) -> bool {
        self.access.is_read()
            || (self.kind.is_none() && self.identity.is_none() && self.size.is_none())
    }
}

/// A bounded file read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileObservation {
    pub access: AccessState,
    /// At most the requested byte bound; empty unless the state is a read.
    pub bytes: Vec<u8>,
    /// More bytes existed than the bound allowed (exact: one byte beyond
    /// the bound was observed).
    pub truncated: bool,
    pub note: Option<String>,
}

impl FileObservation {
    pub fn read(bytes: Vec<u8>, truncated: bool) -> Self {
        let access = if bytes.is_empty() && !truncated {
            AccessState::Empty
        } else {
            AccessState::ReadSucceeded
        };
        FileObservation {
            access,
            bytes,
            truncated,
            note: None,
        }
    }

    fn without_payload(access: AccessState, note: Option<String>) -> Self {
        FileObservation {
            access,
            bytes: Vec::new(),
            truncated: false,
            note,
        }
    }

    pub fn does_not_exist() -> Self {
        Self::without_payload(AccessState::DoesNotExist, None)
    }

    pub fn inaccessible(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::ExistsButInaccessible, Some(note.into()))
    }

    pub fn failed(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::Failed, Some(note.into()))
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Self::without_payload(AccessState::Unsupported, Some(note.into()))
    }

    pub fn is_well_formed(&self) -> bool {
        self.access.is_read() || (self.bytes.is_empty() && !self.truncated)
    }
}

/// Production prober over the engine's [`PlatformFs`] boundary. Read-only:
/// bounded streaming directory listing, link-aware metadata, and the
/// engine's no-follow bounded content read. No writes, no recursion, no
/// subprocesses, no network.
pub struct PlatformPathProber<'a> {
    fs: &'a dyn PlatformFs,
}

impl<'a> PlatformPathProber<'a> {
    pub fn new(fs: &'a dyn PlatformFs) -> Self {
        PlatformPathProber { fs }
    }

    /// The prober for the current host's standard filesystem.
    pub fn host() -> PlatformPathProber<'static> {
        PlatformPathProber {
            fs: coresight_engine::platform::std_fs(),
        }
    }

    /// Streaming, bounded list of `dir` filtered by `keep`; memory O(max).
    fn stream_listing(
        &self,
        dir: &Path,
        max: usize,
        keep: impl Fn(ProbedKind) -> bool,
    ) -> io::Result<BoundedTopK<PathKey, ProbedKind>> {
        let mut top: BoundedTopK<PathKey, ProbedKind> = BoundedTopK::new(max);
        self.fs.read_dir_entries(dir, &mut |child| {
            let kind = if child.is_symlink {
                ProbedKind::Symlink
            } else if child.is_dir {
                ProbedKind::Dir
            } else {
                ProbedKind::File
            };
            if keep(kind) {
                top.offer(PathKey(dir.join(&child.name)), kind, |_, _| false);
            }
            true
        })?;
        Ok(top)
    }
}

/// Map an I/O failure of an operation on a path whose existence was already
/// PROVEN by metadata. Denial after a proven stat is "exists but
/// inaccessible"; anything else is `Failed`.
fn access_after_proven_existence(err: &io::Error) -> AccessState {
    if err.kind() == io::ErrorKind::PermissionDenied {
        AccessState::ExistsButInaccessible
    } else {
        AccessState::Failed
    }
}

impl PathProber for PlatformPathProber<'_> {
    fn children_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
        match self.stream_listing(dir, max, |k| k == ProbedKind::Dir) {
            Ok(top) => {
                let (items, overflow) = top.into_sorted();
                BoundedListing {
                    names: items.into_iter().map(|(k, _)| k.0).collect(),
                    overflow,
                }
            }
            Err(_) => BoundedListing::default(),
        }
    }

    fn entries_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
        match self.stream_listing(dir, max, |_| true) {
            Ok(top) => {
                let (items, overflow) = top.into_sorted();
                BoundedListing {
                    names: items.into_iter().map(|(k, _)| k.0).collect(),
                    overflow,
                }
            }
            Err(_) => BoundedListing::default(),
        }
    }

    fn list_dir(&self, dir: &Path, max: usize) -> DirectoryObservation {
        match self.fs.metadata(dir) {
            Err(err) => {
                return match err.kind() {
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                        DirectoryObservation::does_not_exist()
                    }
                    // Existence could not be proven: Failed, not
                    // "exists but inaccessible" (no overclaiming).
                    _ => DirectoryObservation::failed(format!("metadata unavailable: {err}")),
                };
            }
            Ok(md) => {
                if md.kind != FsKind::Dir {
                    return DirectoryObservation::failed(
                        "path is not a directory (links are never followed)",
                    );
                }
            }
        }
        match self.stream_listing(dir, max, |_| true) {
            Ok(top) => {
                let (items, overflow) = top.into_sorted();
                DirectoryObservation::read(
                    items
                        .into_iter()
                        .map(|(k, kind)| ListedEntry { path: k.0, kind })
                        .collect(),
                    overflow,
                )
            }
            Err(err) => match access_after_proven_existence(&err) {
                AccessState::ExistsButInaccessible => {
                    DirectoryObservation::inaccessible("directory listing denied")
                }
                _ => DirectoryObservation::failed(format!("listing failed: {err}")),
            },
        }
    }

    fn stat(&self, path: &Path) -> PathObservation {
        match self.fs.metadata(path) {
            Ok(md) => {
                let identity = match (md.device, md.inode) {
                    (Some(volume), Some(file_id)) => Some(ObjectIdentity {
                        volume,
                        file_id,
                        file_id_hi: md.file_id_hi,
                    }),
                    // Partial identity is never promoted: a high part
                    // without the low pair proves nothing usable.
                    _ => None,
                };
                PathObservation::present(md.kind.into(), identity, Some(md.size))
            }
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                    PathObservation::does_not_exist()
                }
                _ => PathObservation::failed(format!("metadata unavailable: {err}")),
            },
        }
    }

    fn read_file_bounded(&self, path: &Path, max_bytes: u64) -> FileObservation {
        match self.fs.metadata(path) {
            Err(err) => {
                return match err.kind() {
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                        FileObservation::does_not_exist()
                    }
                    _ => FileObservation::failed(format!("metadata unavailable: {err}")),
                };
            }
            Ok(md) if md.kind != FsKind::File => {
                return FileObservation::failed("path is not a regular file");
            }
            Ok(_) => {}
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut feed =
            |reader: &mut dyn coresight_engine::platform::ContentReader| -> io::Result<()> {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read_chunk(&mut buf)? {
                        None => return Ok(()),
                        Some(n) => {
                            let room = (max_bytes as usize).saturating_sub(bytes.len());
                            if n > room {
                                bytes.extend_from_slice(&buf[..room]);
                                // One byte beyond the bound proves truncation.
                                truncated = true;
                                return Ok(());
                            }
                            bytes.extend_from_slice(&buf[..n]);
                        }
                    }
                }
            };
        match self
            .fs
            .read_content(path, ContentOutcome::Opened(&mut feed))
        {
            Ok(()) => FileObservation::read(bytes, truncated),
            Err(ContentError::OpenFailed(err)) | Err(ContentError::ReadFailed(err)) => {
                match access_after_proven_existence(&err) {
                    AccessState::ExistsButInaccessible => {
                        FileObservation::inaccessible("file read denied")
                    }
                    _ => FileObservation::failed(format!("read failed: {err}")),
                }
            }
            Err(other) => FileObservation::failed(other.message()),
        }
    }
}
