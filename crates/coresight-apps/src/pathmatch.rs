//! Lossless semantic path matching (Phase 6.3 hardening).
//!
//! Name and extension checks in the intelligence layer decide application
//! identity, executable candidacy, and bundle/desktop-entry discovery. Those
//! decisions must never depend on replacement characters introduced by a
//! lossy decoding: two distinct non-UTF-8 names can decode to the SAME
//! display string, so a lossy matching key can fabricate agreement where the
//! bytes disagree.
//!
//! The rules in this module are therefore:
//!
//! * semantic comparisons against ASCII-defined constants (`"MacOS"`,
//!   `"Contents"`, `"exe"`, `"app"`, `"lnk"`, `"desktop"`) run on the
//!   platform-encoded bytes, never on a decoded `str`;
//! * decoding a path component to text for name agreement is STRICT
//!   ([`str::from_utf8`]-equivalent via [`OsStr::to_str`]): a component the
//!   platform did not encode as UTF-8 yields `None` ("cannot interpret"),
//!   which callers treat as "does not match" rather than as an empty or
//!   replacement-character name;
//! * extension *selection* keeps [`Path::extension`] semantics exactly
//!   (including its dotfile rule); only the *comparison* is byte-level.

use std::path::Path;

/// The final component of `path` as text, or `None` when the platform did
/// not encode it as UTF-8. `None` means "cannot interpret" — never an empty
/// name, never a replacement-character name.
pub fn file_name_str(path: &Path) -> Option<&str> {
    path.file_name().and_then(|n| n.to_str())
}

/// The final component without its extension as text, or `None` when the
/// platform did not encode it as UTF-8.
pub fn file_stem_str(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|s| s.to_str())
}

/// `true` when the final component's bytes equal `want` exactly.
/// Byte-exact: non-UTF-8 components simply never equal an ASCII constant.
pub fn file_name_is_ascii(path: &Path, want: &[u8]) -> bool {
    path.file_name()
        .map(|n| n.as_encoded_bytes() == want)
        .unwrap_or(false)
}

/// `true` when the path's extension (selected with [`Path::extension`]
/// semantics) equals `want` under ASCII case folding. Byte-level: no
/// decoding of the arbitrary file name ever takes place.
pub fn extension_is_ascii(path: &Path, want: &[u8]) -> bool {
    path.extension()
        .map(|e| ascii_eq_ignore_case(e.as_encoded_bytes(), want))
        .unwrap_or(false)
}

/// ASCII case-insensitive byte equality. Both sides are expected to be
/// short ASCII constants on at least one side; non-ASCII bytes compare
/// exactly (they never fold).
pub fn ascii_eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ascii_constants_match_without_decoding() {
        assert!(extension_is_ascii(Path::new("/a/tool.exe"), b"exe"));
        assert!(extension_is_ascii(Path::new("/a/tool.EXE"), b"exe"));
        assert!(extension_is_ascii(Path::new("/a/x.APP"), b"app"));
        assert!(!extension_is_ascii(Path::new("/a/tool.exe"), b"com"));
        assert!(!extension_is_ascii(Path::new("/a/tool"), b"exe"));
        assert!(file_name_is_ascii(Path::new("/x/MacOS"), b"MacOS"));
        assert!(!file_name_is_ascii(Path::new("/x/macos"), b"MacOS"));
    }

    #[test]
    fn non_utf8_components_are_uninterpretable_never_lossy() {
        use std::ffi::OsString;
        // WTF-8 for an unpaired UTF-16 surrogate is a valid platform
        // encoding on Windows and non-UTF-8 arbitrary bytes on Unix. Avoid
        // invalid constructor input on Windows.
        let raw = b"/data/\xed\xa0\x80/tool";
        let p = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw.to_vec()) });
        // Strict decoding refuses the component: it must not become a
        // replacement-character name that could match something else.
        assert_eq!(file_name_str(&p), Some("tool"));
        let mut raw_dir = b"/data/".to_vec();
        raw_dir.extend_from_slice(&[0xed, 0xa0, 0x80]);
        let d = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw_dir.clone()) });
        assert_eq!(file_name_str(&d), None);
        assert!(!file_name_is_ascii(&d, b"data"));
        assert!(std::str::from_utf8(raw_dir.as_slice()).is_err());
        // A non-UTF-8 extension never equals an ASCII-defined suffix.
        let raw_ext = b"/data/tool.e\xed\xa0\x80";
        let e = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw_ext.to_vec()) });
        assert!(!extension_is_ascii(&e, b"exe"));
    }

    #[test]
    fn dotfile_extension_keeps_path_semantics() {
        // `Path::extension` reports no extension for a leading-dot stem;
        // the byte-level comparison preserves that selection rule.
        assert!(!extension_is_ascii(Path::new("/a/.lnk"), b"lnk"));
        assert!(extension_is_ascii(Path::new("/a/x.lnk"), b"lnk"));
    }
}
