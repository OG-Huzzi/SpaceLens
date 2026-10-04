//! Drive / volume identification and OS standard directories.
//!
//! Windows uses the Win32 volume APIs via `windows-sys` (thin FFI
//! declarations, Microsoft-maintained). Linux parses `/proc/mounts`; macOS
//! has no std-reachable mount table without a `libc` dependency, so it
//! honestly reports only the root volume in Phase 1 (documented limitation).

use std::io;
use std::path::PathBuf;

use super::{DriveInfo, SysDirs, VolumeInfo, VolumeKind};

// ---------------------------------------------------------------------------
// DriveInfo
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_drives {
    use super::*;

    struct WindowsDrives;

    /// GetDriveTypeW return values.
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl DriveInfo for WindowsDrives {
        fn list_volumes(&self) -> io::Result<Vec<VolumeInfo>> {
            #[link(name = "kernel32")]
            extern "system" {
                // SAFETY: FFI declarations for kernel32 volume APIs taking
                // wide-string inputs and caller-owned output buffers only.
                fn GetLogicalDrives() -> u32;
                fn GetDriveTypeW(root: *const u16) -> u32;
                fn GetDiskFreeSpaceExW(
                    dir: *const u16,
                    free_caller: *mut u64,
                    total: *mut u64,
                    free_total: *mut u64,
                ) -> i32;
                fn GetVolumeInformationW(
                    root: *const u16,
                    label: *mut u16,
                    label_len: u32,
                    serial: *mut u32,
                    max_comp_len: *mut u32,
                    fs_flags: *mut u32,
                    fs_buf: *mut u16,
                    fs_len: u32,
                ) -> i32;
            }

            let mask = unsafe { GetLogicalDrives() };
            let mut out = Vec::new();
            for i in 0..26u32 {
                if mask & (1 << i) == 0 {
                    continue;
                }
                let letter = (b'A' + i as u8) as char;
                let root = format!("{letter}:\\");
                let root_w = to_wide(&root);

                let kind = match unsafe { GetDriveTypeW(root_w.as_ptr()) } {
                    DRIVE_REMOVABLE => VolumeKind::Removable,
                    DRIVE_FIXED => VolumeKind::Internal,
                    DRIVE_REMOTE => VolumeKind::Network,
                    _ => VolumeKind::Unknown,
                };

                let (mut capacity, mut available) = (0u64, 0u64);
                let space_ok = unsafe {
                    GetDiskFreeSpaceExW(
                        root_w.as_ptr(),
                        std::ptr::null_mut(),
                        &mut capacity,
                        &mut available,
                    )
                } != 0;

                let mut label_buf = [0u16; 261];
                let mut fs_buf = [0u16; 64];
                let mut serial = 0u32;
                let info_ok = unsafe {
                    GetVolumeInformationW(
                        root_w.as_ptr(),
                        label_buf.as_mut_ptr(),
                        label_buf.len() as u32,
                        &mut serial,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        fs_buf.as_mut_ptr(),
                        fs_buf.len() as u32,
                    )
                } != 0;

                let label = if info_ok {
                    let end = label_buf.iter().position(|&c| c == 0).unwrap_or(0);
                    Some(String::from_utf16_lossy(&label_buf[..end]))
                } else {
                    None
                };
                let fs_type = if info_ok {
                    let end = fs_buf.iter().position(|&c| c == 0).unwrap_or(0);
                    Some(String::from_utf16_lossy(&fs_buf[..end]))
                } else {
                    None
                };
                // CD-ROM drives with no media report no space: keep them
                // listed with explicit `None`s instead of failing the listing.
                out.push(VolumeInfo {
                    id: info_ok.then(|| format!("vol-{serial:08X}")),
                    root: PathBuf::from(&root),
                    label,
                    fs_type,
                    kind,
                    capacity: space_ok.then_some(capacity),
                    available: space_ok.then_some(available),
                });
            }
            Ok(out)
        }
    }

    pub(super) fn drive_info() -> &'static dyn DriveInfo {
        &WindowsDrives
    }
}

#[cfg(unix)]
mod unix_drives {
    use super::*;

    struct UnixDrives;

    /// Pseudo-filesystems that are kernel/memory views, not storage to scan.
    /// Kept explicit; anything not listed is reported.
    const PSEUDO_FS: [&str; 16] = [
        "proc",
        "sysfs",
        "devtmpfs",
        "devpts",
        "tmpfs",
        "securityfs",
        "cgroup",
        "cgroup2",
        "pstore",
        "bpf",
        "debugfs",
        "tracefs",
        "configfs",
        "hugetlbfs",
        "mqueue",
        "efivarfs",
    ];

    fn is_pseudo(fs_type: &str) -> bool {
        PSEUDO_FS.contains(&fs_type)
    }

    impl DriveInfo for UnixDrives {
        fn list_volumes(&self) -> io::Result<Vec<VolumeInfo>> {
            // Linux: /proc/mounts (kernel-provided; no subprocess involved).
            match std::fs::read_to_string("/proc/mounts") {
                Ok(text) => {
                    let mut out = Vec::new();
                    for line in text.lines() {
                        let mut fields = line.split_whitespace();
                        let (Some(_dev), Some(mount), Some(fs_type)) =
                            (fields.next(), fields.next(), fields.next())
                        else {
                            continue;
                        };
                        if is_pseudo(fs_type) {
                            continue;
                        }
                        let mount = unescape_mount_path(mount);
                        out.push(VolumeInfo {
                            id: None,
                            root: PathBuf::from(mount),
                            label: None,
                            fs_type: Some(fs_type.to_string()),
                            kind: if fs_type == "nfs" || fs_type == "cifs" || fs_type == "smbfs" {
                                VolumeKind::Network
                            } else {
                                VolumeKind::Unknown
                            },
                            // statvfs needs a libc dependency; Phase 1 reports
                            // Unix capacity as unknown. Owned follow-up.
                            capacity: None,
                            available: None,
                        });
                    }
                    Ok(out)
                }
                Err(_) => {
                    // Non-Linux Unix (e.g. macOS): no std-reachable mount
                    // table. Report the root honestly and nothing else.
                    Ok(vec![VolumeInfo {
                        id: None,
                        root: PathBuf::from("/"),
                        label: None,
                        fs_type: None,
                        kind: VolumeKind::Unknown,
                        capacity: None,
                        available: None,
                    }])
                }
            }
        }
    }

    /// Decode the octal escapes /proc/mounts uses (`\040` space, `\134`
    /// backslash, `\011` tab, `\012` newline) so paths stay correct.
    fn unescape_mount_path(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 3 < bytes.len() {
                let digits = &bytes[i + 1..i + 4];
                if digits.iter().all(|b| b.is_ascii_digit()) {
                    let oct = (digits[0] - b'0') as u32 * 64
                        + (digits[1] - b'0') as u32 * 8
                        + (digits[2] - b'0') as u32;
                    if oct < 128 {
                        out.push(oct as u8);
                        i += 4;
                        continue;
                    }
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub(super) fn drive_info() -> &'static dyn DriveInfo {
        &UnixDrives
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn mount_path_unescaping_decodes_octal_escapes() {
            assert_eq!(
                unescape_mount_path("/mnt/with\\040space"),
                "/mnt/with space"
            );
            assert_eq!(unescape_mount_path("/plain"), "/plain");
            assert_eq!(unescape_mount_path("/tab\\011sep"), "/tab\tsep");
            // Incomplete escape stays literal.
            assert_eq!(unescape_mount_path("/trailing\\04"), "/trailing\\04");
        }
    }
}

/// Default `DriveInfo` instance for the current platform.
pub fn drive_info() -> &'static dyn DriveInfo {
    #[cfg(windows)]
    return windows_drives::drive_info();
    #[cfg(unix)]
    return unix_drives::drive_info();
    #[cfg(not(any(unix, windows)))]
    unimplemented!("no DriveInfo implementation for this platform");
}

// ---------------------------------------------------------------------------
// SysDirs
// ---------------------------------------------------------------------------

struct StdDirs;

impl SysDirs for StdDirs {
    fn home(&self) -> Option<PathBuf> {
        // Deliberately not std::env::home_dir (its behavior differs across
        // versions); read the canonical env vars directly.
        if cfg!(windows) {
            std::env::var_os("USERPROFILE").map(PathBuf::from)
        } else {
            std::env::var_os("HOME").map(PathBuf::from)
        }
    }

    fn temp(&self) -> PathBuf {
        std::env::temp_dir()
    }
}

/// Default `SysDirs` instance for the current platform.
pub fn sys_dirs() -> &'static dyn SysDirs {
    &StdDirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_and_temp_resolve() {
        let dirs = sys_dirs();
        let temp = dirs.temp();
        assert!(temp.is_absolute());
        // HOME may legitimately be unset in exotic CI sandboxes; assert only
        // that the call never panics and is absolute when present.
        if let Some(h) = dirs.home() {
            assert!(h.is_absolute());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_lists_at_least_c_drive() {
        let vols = drive_info().list_volumes().unwrap();
        assert!(
            !vols.is_empty(),
            "a booted Windows machine has at least C:\\"
        );
        let c = vols
            .iter()
            .find(|v| v.root.as_os_str() == "C:\\")
            .expect("C:\\ must be listed");
        assert!(
            c.capacity.unwrap_or(0) > 0,
            "C:\\ capacity must be readable"
        );
        assert_eq!(c.kind, VolumeKind::Internal);
        assert!(c.id.is_some(), "volume serial should be readable on NTFS");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_lists_root_without_pseudo_filesystems() {
        use std::path::Path;
        let vols = drive_info().list_volumes().unwrap();
        assert!(!vols.is_empty());
        assert!(vols.iter().any(|v| v.root == Path::new("/")));
        assert!(
            !vols.iter().any(|v| v.fs_type.as_deref() == Some("proc")),
            "pseudo filesystems must be filtered"
        );
    }
}
