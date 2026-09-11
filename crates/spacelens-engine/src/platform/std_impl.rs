//! std-based `PlatformFs` implementation shared by every OS.
//!
//! All OS-specific behavior sits in the `unix` / `windows` helper modules and
//! is selected by `cfg` **inside this file only** — the shared scanner never
//! branches on the OS (docs/CROSS_PLATFORM.md).
//!
//! Phase 3.1 content-opening contract (docs/IDENTITY.md §link safety):
//! `read_content` opens the observed path with **no-follow** semantics —
//! Unix `open(O_NOFOLLOW | O_NONBLOCK)`, Windows
//! `FILE_FLAG_OPEN_REPARSE_POINT` + handle inspection — so a path that
//! became a symlink/junction/reparse point after observation is refused
//! with [`ContentError::UnexpectedLink`] and its target is never touched.
//! Every fact the consumer sees (identity, length, change timestamps) is
//! taken from the open handle: fstat semantics, never a second path
//! resolution.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::{
    ChildInfo, ContentError, ContentOutcome, ContentReader, FsKind, HandleStat, MetadataInfo,
    PlatformFs,
};
use crate::error::ErrorCategory;
use crate::identity::FileIdentity;

// The per-OS helper modules live beside this file, not beneath it.
#[cfg(unix)]
#[path = "unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "windows.rs"]
mod windows;

/// Shared, std-only implementation used as the default platform.
pub struct StdFs;

/// Bounded streaming reader over one open file handle.
struct StdContentReader {
    file: fs::File,
    identity: FileIdentity,
    /// Last read error, if any. When the consumer callback returns `Err`,
    /// this distinguishes a genuine read failure (`Some`) from a deliberate
    /// consumer abort (`None`) — typed error semantics without a second
    /// error channel.
    last_read_error: Option<io::Error>,
}

impl ContentReader for StdContentReader {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        // EOF is `Ok(None)` per the trait contract (a 0-byte read is EOF
        // for an at-end handle position — callers must not need to
        // special-case `Some(0)`).
        match self.file.read(buf) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(n)),
            Err(e) => {
                self.last_read_error = Some(e);
                Err(io::Error::other(
                    "content read failed (see last_read_error)",
                ))
            }
        }
    }
    fn file_identity(&self) -> FileIdentity {
        self.identity
    }

    fn pre_stat(&self) -> io::Result<HandleStat> {
        handle_stat(&self.file)
    }

    fn post_stat(&self) -> io::Result<HandleStat> {
        handle_stat(&self.file)
    }
}

/// Handle-proven length + change timestamps from the open handle only
/// (fstat / BY_HANDLE_FILE_INFORMATION). Fields the OS does not maintain
/// are `None` — never fabricated.
fn handle_stat(file: &fs::File) -> io::Result<HandleStat> {
    #[cfg(unix)]
    {
        unix::handle_stat(file)
    }
    #[cfg(windows)]
    {
        windows::handle_stat(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let md = file.metadata()?;
        Ok(HandleStat {
            len: md.len(),
            modified: md.modified().ok(),
            changed: None,
        })
    }
}

/// Identity proven from the open handle itself (fstat-equivalent). This
/// describes the object the bytes are actually read from — not a path that
/// could have been swapped between scan time and hash time. On failure
/// identity degrades explicitly to unknown — never fabricated.
fn handle_identity(file: &fs::File) -> io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let md = file.metadata()?;
        Ok(FileIdentity {
            device: Some(md.dev()),
            inode: Some(md.ino()),
            link_count: Some(md.nlink()),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // BOOL FALSE == 0.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
        if ok == 0 {
            return Ok(FileIdentity::unknown());
        }
        Ok(FileIdentity {
            device: Some(u64::from(info.dwVolumeSerialNumber)),
            inode: Some((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)),
            link_count: Some(u64::from(info.nNumberOfLinks)),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(FileIdentity::unknown())
    }
}

/// Open the observed path with **no-follow** semantics, or refuse.
///
/// Returns the open regular-file handle. A path that resolves to a
/// symlink/junction/reparse point yields [`ContentError::UnexpectedLink`] —
/// the target is never touched. A non-regular, non-link object (FIFO,
/// socket, device, directory) yields [`ContentError::NotRegularFile`].
fn open_no_follow(path: &Path) -> Result<fs::File, ContentError> {
    #[cfg(unix)]
    {
        unix::open_no_follow(path)
    }
    #[cfg(windows)]
    {
        windows::open_no_follow(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        // No platform-specific no-follow primitive: degrade to refusing
        // links by stat first, honest about the residual final-component race.
        let md = fs::symlink_metadata(path).map_err(ContentError::OpenFailed)?;
        if md.file_type().is_symlink() {
            Err(ContentError::UnexpectedLink)
        } else {
            fs::File::open(path).map_err(ContentError::OpenFailed)
        }
    }
}

fn open_and_stream(
    path: &Path,
    feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
) -> Result<(), ContentError> {
    let file = open_no_follow(path)?;
    let identity = handle_identity(&file).map_err(ContentError::ReadFailed)?;
    let mut reader = StdContentReader {
        file,
        identity,
        last_read_error: None,
    };
    // A consumer callback error is `Aborted` ONLY when the consumer stopped
    // deliberately: it returns `Interrupted` AND no read failed underneath.
    // A consistency-check failure (any other kind, or a genuine read error
    // recorded earlier) keeps its own type — a per-file failure, never a
    // cancellation.
    if let Err(consumer_err) = feed(&mut reader) {
        let aborted =
            reader.last_read_error.is_none() && consumer_err.kind() == io::ErrorKind::Interrupted;
        return Err(if aborted {
            ContentError::Aborted
        } else {
            match reader.last_read_error.take() {
                Some(read_err) => ContentError::ReadFailed(read_err),
                None => ContentError::ReadFailed(consumer_err),
            }
        });
    }
    Ok(())
}

impl PlatformFs for StdFs {
    fn read_dir_entries(
        &self,
        dir: &Path,
        visit: &mut dyn FnMut(ChildInfo) -> bool,
    ) -> io::Result<()> {
        let rd = fs::read_dir(dir)?;
        for entry in rd {
            // A child vanishing between listing and file_type is normal on
            // live filesystems: skip it, don't fail the directory.
            let Ok(entry) = entry else { continue };
            let Ok(ft) = entry.file_type() else { continue };
            let cont = visit(ChildInfo {
                name: entry.file_name(),
                is_dir: ft.is_dir(),
                is_symlink: ft.is_symlink(),
            });
            if !cont {
                break;
            }
        }
        Ok(())
    }

    fn metadata(&self, path: &Path) -> io::Result<MetadataInfo> {
        // symlink_metadata = lstat semantics: describes the link itself.
        let md = fs::symlink_metadata(path)?;
        let ft = md.file_type();
        let kind = if ft.is_symlink() {
            FsKind::Symlink
        } else if ft.is_dir() {
            FsKind::Dir
        } else if ft.is_file() {
            FsKind::File
        } else {
            FsKind::Other
        };
        let mut info = MetadataInfo {
            kind,
            size: md.len(),
            allocated: None,
            modified: md.modified().ok(),
            created: md.created().ok(),
            accessed: md.accessed().ok(),
            changed: None,
            device: None,
            inode: None,
            reparse: false,
            hidden: false,
        };
        #[cfg(unix)]
        unix::fill_platform_fields(&md, &mut info);
        #[cfg(windows)]
        windows::fill_platform_fields(&md, &mut info);
        Ok(info)
    }

    fn read_link_target(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn categorize_error(&self, err: &io::Error) -> ErrorCategory {
        #[cfg(unix)]
        return unix::categorize(err);
        #[cfg(windows)]
        return windows::categorize(err);
        #[cfg(not(any(unix, windows)))]
        let _ = err;
        #[cfg(not(any(unix, windows)))]
        return ErrorCategory::Other;
    }

    fn is_hidden(&self, name: &OsStr, md: &MetadataInfo) -> bool {
        #[cfg(windows)]
        {
            let _ = name;
            windows::is_hidden(md)
        }
        #[cfg(unix)]
        {
            let _ = md;
            unix::is_hidden(name)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (name, md);
            false
        }
    }

    fn read_content(&self, path: &Path, outcome: ContentOutcome<'_>) -> Result<(), ContentError> {
        match outcome {
            ContentOutcome::Opened(feed) => open_and_stream(path, feed),
            ContentOutcome::Failed(err) => Err(ContentError::OpenFailed(err)),
        }
    }
}

/// The default platform instance for production use.
pub fn std_fs() -> &'static dyn PlatformFs {
    &StdFs
}
