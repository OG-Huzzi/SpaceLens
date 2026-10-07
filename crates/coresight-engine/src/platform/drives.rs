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
    use std::ffi::OsString;

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
            // Read as raw bytes: a mount path is an arbitrary byte string,
            // and the project's path-losslessness guarantee forbids UTF-8
            // round-trips on paths (a read_to_string would silently drop
            // the whole table the moment any path is not valid UTF-8).
            match std::fs::read("/proc/mounts") {
                Ok(bytes) => Ok(bytes
                    .split(|b| *b == b'\n')
                    .filter_map(parse_mount_line)
                    .collect()),
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

    /// Parse one /proc/mounts line into a volume entry. `None` for malformed
    /// lines and pseudo filesystems (filtered by policy, not by hope).
    fn parse_mount_line(line: &[u8]) -> Option<VolumeInfo> {
        let mut fields = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|f| !f.is_empty());
        let (Some(_dev), Some(mount), Some(fs_type)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return None;
        };
        // Filesystem type names are kernel ASCII identifiers; a non-UTF-8
        // one is reported as `None` (honest unknown), never mangled.
        let fs_type = std::str::from_utf8(fs_type).ok();
        if fs_type.map(is_pseudo).unwrap_or(false) {
            return None;
        }
        let network = matches!(fs_type, Some("nfs") | Some("cifs") | Some("smbfs"));
        Some(VolumeInfo {
            id: None,
            root: PathBuf::from(unescape_mount_path(mount)),
            label: None,
            fs_type: fs_type.map(str::to_string),
            kind: if network {
                VolumeKind::Network
            } else {
                VolumeKind::Unknown
            },
            // statvfs needs a libc dependency; Phase 1 reports
            // Unix capacity as unknown. Owned follow-up.
            capacity: None,
            available: None,
        })
    }

    /// Decode the octal escapes /proc/mounts uses (`\040` space, `\134`
    /// backslash, `\011` tab, `\012` newline) at the BYTE level and build
    /// the mount path losslessly. The kernel's escape set is ASCII, but the
    /// path bytes around the escapes are arbitrary: a lossy UTF-8 decode
    /// here could collapse two distinct mounts onto one fabricated path
    /// (U+FFFD), violating the project's path-losslessness guarantee. Octal
    /// escapes ≥ 128 stay literal, as before (the kernel never emits them).
    fn unescape_mount_path(bytes: &[u8]) -> OsString {
        use std::os::unix::ffi::OsStringExt;
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
        OsString::from_vec(out)
    }

    pub(super) fn drive_info() -> &'static dyn DriveInfo {
        &UnixDrives
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::PathBuf;

        fn decoded(bytes: &[u8]) -> PathBuf {
            PathBuf::from(unescape_mount_path(bytes))
        }

        #[test]
        fn mount_path_unescaping_decodes_octal_escapes() {
            assert_eq!(
                decoded(b"/mnt/with\\040space"),
                PathBuf::from("/mnt/with space")
            );
            assert_eq!(decoded(b"/plain"), PathBuf::from("/plain"));
            assert_eq!(decoded(b"/tab\\011sep"), PathBuf::from("/tab\tsep"));
            // Incomplete escape stays literal.
            assert_eq!(decoded(b"/trailing\\04"), PathBuf::from("/trailing\\04"));
        }

        #[test]
        fn mount_path_decoding_is_lossless_for_non_utf8_bytes() {
            use std::os::unix::ffi::OsStrExt;
            // A mount path is an arbitrary byte string: every byte must
            // survive exactly (no U+FFFD substitution, no UTF-8 round-trip).
            let raw: &[u8] = b"/mnt/\xff\xfe-dir";
            let first = unescape_mount_path(raw);
            assert_eq!(first.as_os_str().as_bytes(), raw);
            // Distinct byte strings must stay distinct paths.
            let other: &[u8] = b"/mnt/\xff\xff-dir";
            assert_ne!(first, unescape_mount_path(other));
        }

        #[test]
        fn mount_line_parsing_decodes_and_filters() {
            let vol = parse_mount_line(b"/dev/sda1 /mnt/with\\040space ext4 rw 0 0")
                .expect("a plain ext4 line parses");
            assert_eq!(vol.root, PathBuf::from("/mnt/with space"));
            assert_eq!(vol.fs_type.as_deref(), Some("ext4"));
            assert_eq!(vol.kind, VolumeKind::Unknown);

            assert!(
                parse_mount_line(b"proc /proc proc rw 0 0").is_none(),
                "pseudo filesystems are filtered"
            );
            assert!(
                parse_mount_line(b"").is_none(),
                "malformed lines are dropped"
            );

            let nfs =
                parse_mount_line(b"host:/share /mnt/nas nfs rw 0 0").expect("an nfs line parses");
            assert_eq!(nfs.kind, VolumeKind::Network);
        }

        #[test]
        fn mount_line_with_invalid_utf8_keeps_entry_honestly() {
            // A non-UTF-8 filesystem type is not a pseudo filesystem and
            // must not mangle the entry: fs_type becomes `None`, the mount
            // path survives byte-exactly.
            use std::os::unix::ffi::OsStrExt;
            let line: &[u8] = b"/dev/sdb1 /mnt/ok \xff\xfe-type rw 0 0";
            let vol = parse_mount_line(line).expect("the entry is kept");
            assert_eq!(vol.fs_type, None);
            assert_eq!(vol.root.as_os_str().as_bytes(), b"/mnt/ok" as &[u8]);
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

// Platform behavior lives in cfg-selected modules — the shared `SysDirs`
// impl never branches at runtime (docs/CROSS_PLATFORM.md). Mirrors the
// `drive_info()` selection pattern above.
#[cfg(windows)]
mod windows_dirs {
    use super::*;

    pub(super) fn home() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

#[cfg(unix)]
mod unix_dirs {
    use super::*;

    pub(super) fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

impl SysDirs for StdDirs {
    fn home(&self) -> Option<PathBuf> {
        // Deliberately not std::env::home_dir (its behavior differs across
        // versions); read the canonical per-OS env var directly.
        #[cfg(windows)]
        return windows_dirs::home();
        #[cfg(unix)]
        return unix_dirs::home();
        #[cfg(not(any(unix, windows)))]
        return None;
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
