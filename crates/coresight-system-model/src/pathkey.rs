//! Lossless artifact keys.
//!
//! An artifact node needs a stable, orderable, **lossless** key. A display
//! string is not acceptable: on Unix a path is arbitrary bytes, and on
//! Windows it is arbitrary UTF-16 that may contain unpaired surrogates.
//!
//! The key is therefore built from the platform's own lossless encoding
//! ([`OsStr::as_encoded_bytes`], the same primitive the rest of CoreSight
//! uses for canonical ordering) rendered as lowercase hex:
//!
//! ```text
//! art-<hex(bytes)>
//! ```
//!
//! Properties that matter:
//!
//! * **Lossless** — the key is a bijection of the encoded path bytes; two
//!   paths share a key iff their bytes are identical.
//! * **Orderable** — hex encoding preserves the byte order, so sorting keys
//!   sorts paths by bytes exactly like `PathKey` does.
//! * **Stable** — no hashing, so there is no collision risk and no
//!   dependence on hasher state.
//! * **Not an identity** — the key identifies a *path*, never a filesystem
//!   object. Object identity stays [`coresight_identity::ObjectIdentity`].

use std::fmt;
use std::path::{Path, PathBuf};

/// The canonical lossless key of one observed path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactKey(String);

impl ArtifactKey {
    /// Key for a path: `art-` + lowercase hex of the platform-encoded bytes.
    pub fn of(path: &Path) -> Self {
        let bytes = path.as_os_str().as_encoded_bytes();
        let mut out = String::with_capacity(4 + bytes.len() * 2);
        out.push_str("art-");
        for b in bytes {
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0f));
        }
        ArtifactKey(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Decode back to the exact original path bytes. Never used for
    /// semantics (the model keeps the real `PathBuf`), but it makes the
    /// losslessness of the encoding testable.
    pub fn decode(&self) -> Option<PathBuf> {
        let hex = self.0.strip_prefix("art-")?;
        if !hex.len().is_multiple_of(2) {
            return None;
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let raw = hex.as_bytes();
        for pair in raw.as_chunks::<2>().0 {
            let hi = hex_value(pair[0])?;
            let lo = hex_value(pair[1])?;
            bytes.push((hi << 4) | lo);
        }
        Some(path_from_bytes(bytes))
    }
}

impl fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex_digit(v: u8) -> char {
    match v {
        0..=9 => (b'0' + v) as char,
        _ => (b'a' + (v - 10)) as char,
    }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Rebuild a path from raw platform-encoded bytes.
///
/// `OsString::from_encoded_bytes_unchecked` is the exact, platform-neutral
/// inverse of `as_encoded_bytes` on every host (POSIX bytes on Unix, WTF-8
/// on Windows): no platform branching is needed, and no lossy conversion
/// occurs. The model never relies on this for semantics — `decode` exists
/// only to prove the key encoding round-trips.
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(bytes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_lossless_for_ordinary_paths() {
        for p in [
            "/usr/local/bin/tool",
            "C:/Program Files/App/app.exe",
            "/tmp/a b/c",
        ] {
            let key = ArtifactKey::of(Path::new(p));
            assert!(key.as_str().starts_with("art-"));
            assert_eq!(key.decode().as_deref(), Some(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn key_order_matches_byte_order() {
        let mut paths = [
            Path::new("/b"),
            Path::new("/a"),
            Path::new("/a/child"),
            Path::new("/A"),
        ]
        .map(|p| (ArtifactKey::of(p), p.to_path_buf()))
        .to_vec();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        let by_bytes: Vec<PathBuf> = {
            let mut v: Vec<PathBuf> =
                vec!["/b".into(), "/a".into(), "/a/child".into(), "/A".into()];
            v.sort_by(|a, b| {
                a.as_os_str()
                    .as_encoded_bytes()
                    .cmp(b.as_os_str().as_encoded_bytes())
            });
            v
        };
        assert_eq!(
            paths.into_iter().map(|(_, p)| p).collect::<Vec<_>>(),
            by_bytes
        );
    }

    #[test]
    fn identical_bytes_share_a_key_and_different_bytes_do_not() {
        assert_eq!(
            ArtifactKey::of(Path::new("/a/b")),
            ArtifactKey::of(Path::new("/a/b"))
        );
        assert_ne!(
            ArtifactKey::of(Path::new("/a/b")),
            ArtifactKey::of(Path::new("/a/c"))
        );
        // Case variants are DISTINCT paths (no case folding).
        assert_ne!(
            ArtifactKey::of(Path::new("/A")),
            ArtifactKey::of(Path::new("/a"))
        );
    }

    /// Non-UTF-8 paths must round-trip byte-exactly. The three-byte
    /// surrogate encoding is valid WTF-8 on Windows and an arbitrary
    /// non-UTF-8 path component on Unix, so the unsafe constructor's
    /// platform-encoding precondition holds on every CI platform.
    #[test]
    fn non_utf8_paths_round_trip_exactly() {
        use std::ffi::OsString;
        let mut raw = b"/data/".to_vec();
        raw.extend_from_slice(&[0xed, 0xa0, 0x80]);
        raw.extend_from_slice(b"/name");
        let p = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw.clone()) });
        let key = ArtifactKey::of(&p);
        assert_eq!(
            key.decode().unwrap().as_os_str().as_encoded_bytes(),
            raw.as_slice(),
            "the key must reproduce the exact bytes"
        );
        assert!(
            std::str::from_utf8(raw.as_slice()).is_err(),
            "the test bytes must be non-UTF-8"
        );
        // Another valid encoded unpaired surrogate has a distinct key.
        let other_bytes = b"/data/\xed\xa0\x81/name";
        let other =
            PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(other_bytes.to_vec()) });
        assert_ne!(key, ArtifactKey::of(&other));
    }

    #[test]
    fn keys_are_not_identity() {
        // Two different paths must never produce the same key.
        let a = ArtifactKey::of(Path::new("/a"));
        let b = ArtifactKey::of(Path::new("/b"));
        assert_ne!(a, b);
        // And the key is explicitly path-derived, never object-derived.
        assert!(a.as_str().starts_with("art-"));
    }
}
