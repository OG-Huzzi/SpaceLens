//! Production [`RegistryView`] reading the real Windows registry via
//! `windows-sys` (no extra dependencies, matching the engine's choice).

#[cfg(windows)]
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_QWORD, REG_SZ,
};

#[cfg(windows)]
use crate::windows_discovery::{
    decode_registry_string, split_hive_path, RegistryHive, RegistryValue, RegistryView,
};

#[cfg(windows)]
pub struct Win32RegistryView;

#[cfg(windows)]
impl Win32RegistryView {
    pub fn new() -> Self {
        Win32RegistryView
    }

    fn split_hive(key: &str) -> Option<(HKEY, String)> {
        split_hive_path(key).map(|(hive, rest)| {
            let hkey = match hive {
                RegistryHive::Hklm => HKEY_LOCAL_MACHINE,
                RegistryHive::Hkcu => HKEY_CURRENT_USER,
            };
            (hkey, rest.to_string())
        })
    }

    fn open(key: &str) -> Option<HKEY> {
        let (hive, subkey) = Self::split_hive(key)?;
        let wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: null-terminated UTF-16 subkey; handle written only on
        // success; caller closes via RegCloseKey on every path.
        let status = unsafe { RegOpenKeyExW(hive, wide.as_ptr(), 0, KEY_READ, &mut handle) };
        if status == 0 {
            Some(handle)
        } else {
            None
        }
    }

    fn query(handle: HKEY, name: &str) -> Option<RegistryValue> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut kind = 0u32;
        let mut size = 0u32;
        // SAFETY: NULL data with valid size out-param is the documented
        // size-probe form of RegQueryValueExW.
        let status = unsafe {
            RegQueryValueExW(
                handle,
                wide.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != 0 || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        // SAFETY: buffer is exactly `size`; RegQueryValueExW writes at
        // most size bytes and updates size.
        let status = unsafe {
            RegQueryValueExW(
                handle,
                wide.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        buf.truncate(size as usize);
        match kind {
            REG_SZ | REG_EXPAND_SZ => {
                let s = decode_registry_string(&buf);
                if kind == REG_SZ {
                    Some(RegistryValue::Sz(s))
                } else {
                    Some(RegistryValue::ExpandSz(s))
                }
            }
            REG_DWORD => {
                if buf.len() >= 4 {
                    Some(RegistryValue::Dword(u32::from_le_bytes([
                        buf[0], buf[1], buf[2], buf[3],
                    ])))
                } else {
                    None
                }
            }
            REG_QWORD => {
                if buf.len() >= 8 {
                    Some(RegistryValue::Qword(u64::from_le_bytes([
                        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
                    ])))
                } else {
                    None
                }
            }
            _ => Some(RegistryValue::Binary(buf)),
        }
    }
}

#[cfg(windows)]
impl Default for Win32RegistryView {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl RegistryView for Win32RegistryView {
    /// Honest, BOUNDED subkey walk: names stream through a bounded top-K
    /// set (at most `max` canonically-smallest names are retained — O(max)
    /// memory regardless of key count). A name too long for the fixed
    /// 260-unit buffer (`ERROR_MORE_DATA`, 234) is skipped and COUNTED;
    /// any other error short of `ERROR_NO_MORE_ITEMS` (259) marks the
    /// enumeration incomplete. Neither is silently swallowed into a clean
    /// empty list, and nothing beyond the bound is materialized.
    fn subkeys_bounded(
        &self,
        key: &str,
        max: usize,
    ) -> crate::windows_discovery::SubkeyEnumeration {
        const ERROR_NO_MORE_ITEMS: u32 = 259;
        const ERROR_MORE_DATA: u32 = 234;
        let Some(handle) = Self::open(key) else {
            return crate::windows_discovery::SubkeyEnumeration::default();
        };
        let mut out = crate::windows_discovery::SubkeyEnumeration::default();
        let mut kept = std::collections::BTreeSet::new();
        let mut index = 0u32;
        loop {
            let mut name = vec![0u16; 260];
            let mut len = 260u32;
            // SAFETY: fixed buffer; RegEnumKeyExW writes up to len chars
            // and updates len on success.
            let status = unsafe {
                RegEnumKeyExW(
                    handle,
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status == ERROR_MORE_DATA {
                // The key exists but its name exceeds the buffer: skip
                // THIS key and continue with the next index.
                out.skipped_oversized += 1;
                index += 1;
                continue;
            }
            if status != 0 {
                out.incomplete = true;
                break;
            }
            // Key names located via lossy UTF-16; used only to open the
            // subkey, never as filesystem identity.
            let sub = String::from_utf16_lossy(&name[..len as usize]);
            crate::windows_discovery::offer_name(&mut kept, max, sub, &mut out.truncated);
            index += 1;
        }
        out.keys = kept.into_iter().collect();
        // SAFETY: handle is open and owned here.
        unsafe { RegCloseKey(handle) };
        out
    }

    fn get_value(&self, key: &str, name: &str) -> Option<RegistryValue> {
        let handle = Self::open(key)?;
        let value = Self::query(handle, name);
        // SAFETY: handle is open and owned here.
        unsafe { RegCloseKey(handle) };
        value
    }

    /// Precise existence probe: the key opens or it does not (an empty
    /// root is `true`; an absent root is `false`).
    fn key_present(&self, key: &str) -> bool {
        match Self::open(key) {
            Some(handle) => {
                // SAFETY: handle is open and owned here.
                unsafe { RegCloseKey(handle) };
                true
            }
            None => false,
        }
    }
}
