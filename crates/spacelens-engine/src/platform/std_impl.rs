//! std-based `PlatformFs` implementation shared by every OS.
//!
//! All OS-specific behavior sits in the `unix` / `windows` helper modules and
//! is selected by `cfg` **inside this trait impl only** — the shared scanner
//! never branches on the OS (docs/CROSS_PLATFORM.md).

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::{
    ChildInfo, ContentError, ContentOutcome, ContentReader, FsKind, MetadataInfo, PlatformFs,
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
        self.file.read(buf).map(Some).map_err(|e| {
            self.last_read_error = Some(e);
            io::Error::other("content read failed (see last_read_error)")
        })
    }

    fn file_identity(&self) -> FileIdentity {
        self.identity
    }

    fn file_len(&self) -> io::Result<u64> {
        self.file.metadata().map(|m| m.len())
    }
}

/// Identity proven from the open handle itself (fstat-equivalent). This
/// describes the object the bytes are actually read from — not a path that
/// could have been swapped between scan time and hash time.
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
        // BOOL FALSE == 0. On failure identity degrades explicitly to
        // unknown — never fabricated.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
        if ok == 0 {
            return Ok(FileIdentity {
                device: None,
                inode: None,
                link_count: None,
            });
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
        Ok(FileIdentity {
            device: None,
            inode: None,
            link_count: None,
        })
    }
}

fn open_and_stream(
    path: &Path,
    feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
) -> Result<(), ContentError> {
    let file = fs::File::open(path).map_err(ContentError::OpenFailed)?;
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
