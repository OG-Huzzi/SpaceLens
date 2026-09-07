//! std-based `PlatformFs` implementation shared by every OS.
//!
//! All OS-specific behavior sits in the `unix` / `windows` helper modules and
//! is selected by `cfg` **inside this trait impl only** — the shared scanner
//! never branches on the OS (docs/CROSS_PLATFORM.md).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{ChildInfo, FsKind, MetadataInfo, PlatformFs};
use crate::error::ErrorCategory;

// The per-OS helper modules live beside this file, not beneath it.
#[cfg(unix)]
#[path = "unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "windows.rs"]
mod windows;

/// Shared, std-only implementation used as the default platform.
pub struct StdFs;

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
}

/// The default platform instance for production use.
pub fn std_fs() -> &'static dyn PlatformFs {
    &StdFs
}
