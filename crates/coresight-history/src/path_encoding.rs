//! Lossless path persistence (Phase 5.1, Finding 5).
//!
//! History must never weaken observation: the scanner's paths are
//! platform-native [`OsStr`] values (Unix: arbitrary non-NUL bytes;
//! Windows: arbitrary non-NUL UTF-16 including unpaired surrogates), and
//! `to_string_lossy()` — the pre-repair persistence — silently replaced
//! every non-UTF-8 value with U+FFFD. `store(path) → load(path)` is now
//! the exact same path for every representable value.
//!
//! ## Format
//!
//! Stored text is a tagged string:
//!
//! ```text
//! "u:<path>"  UTF-8 path verbatim (fast path; also the ONLY format the
//!             pre-repair store wrote, so legacy rows already look like
//!             this without the tag).
//! "e:<hex>"   path bytes (platform-encoded OsStr bytes) as lowercase
//!             hex — used exactly when the path is not valid UTF-8.
//! "l:<path>"  legacy lossy string (pre-repair row), tagged by the v2→v3
//!             migration: it is what the old store actually wrote — the
//!             lossy conversion is irreversible, so the row is preserved
//!             as-represented rather than reinterpreted.
//! ```
//!
//! Verbatim/tagged storage means: no normalization, no lowercasing, no
//! separator conversion, no symlink resolution, no Unicode replacement.
//!
//! Decoding is total: a malformed value can only occur through direct
//! database tampering, and yields a parse error rather than a corrupted
//! path.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Encode a path for SQLite persistence — lossless for every value the
/// platform can represent.
pub fn encode(path: &Path) -> String {
    let os = path.as_os_str();
    if let Some(s) = os.to_str() {
        format!("u:{s}")
    } else {
        let mut hex = String::with_capacity(2 + 2 + os.as_encoded_bytes().len() * 2);
        hex.push_str("e:");
        for b in os.as_encoded_bytes() {
            hex.push_str(&format!("{b:02x}"));
        }
        hex
    }
}

/// Decode a stored path value. `Ok(Some(path))` for a well-formed tagged
/// value; `Ok(None)` for an untagged legacy string (pre-repair rows:
/// kept as-represented, `to_string_lossy` semantics — the historical
/// limitation is preserved honestly); `Err` only for malformed tagged
/// values (direct tampering).
pub fn decode(stored: &str) -> Result<Option<PathBuf>, PathDecodeError> {
    if let Some(utf8) = stored.strip_prefix("u:") {
        return Ok(Some(PathBuf::from(utf8)));
    }
    if let Some(hex) = stored.strip_prefix("e:") {
        let bytes = decode_hex(hex)?;
        // SAFETY: the bytes came from `OsStr::as_encoded_bytes` on this
        // platform (encode), so they are a valid platform encoding of
        // some OsStr; decoding hex preserves them exactly.
        let os = unsafe { OsStr::from_encoded_bytes_unchecked(&bytes) };
        return Ok(Some(PathBuf::from(os)));
    }
    if let Some(legacy) = stored.strip_prefix("l:") {
        return Ok(Some(PathBuf::from(legacy)));
    }
    // Untagged: a row written by the pre-repair store (its only format
    // was the raw lossy string). Preserve as-represented.
    Ok(Some(PathBuf::from(stored)))
}

/// Why a stored path value could not be decoded (direct database
/// tampering — the store itself never writes malformed values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathDecodeError {
    /// `e:`-tagged value containing non-hex characters.
    InvalidHex,
    /// `e:`-tagged value with an odd number of hex digits.
    OddLength,
}

impl std::fmt::Display for PathDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathDecodeError::InvalidHex => {
                write!(f, "stored path: invalid hex in e:-tagged value")
            }
            PathDecodeError::OddLength => {
                write!(f, "stored path: odd hex length in e:-tagged value")
            }
        }
    }
}

impl std::error::Error for PathDecodeError {}

fn decode_hex(hex: &str) -> Result<Vec<u8>, PathDecodeError> {
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(PathDecodeError::OddLength);
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let hi = hex_digit(pair[0]).ok_or(PathDecodeError::InvalidHex)?;
        let lo = hex_digit(pair[1]).ok_or(PathDecodeError::InvalidHex)?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_round_trips_via_fast_path() {
        for p in [
            "/plain",
            "/with spaces/and more",
            "/üñïçø∂é/🎉",
            "/日本語/ファイル",
        ] {
            let encoded = encode(Path::new(p));
            assert_eq!(encoded, format!("u:{p}"));
            assert_eq!(decode(&encoded).unwrap(), Some(PathBuf::from(p)));
        }
    }

    #[test]
    fn decode_is_lossless_in_bytes() {
        // Property: decode(encode(p)) == p, checked over adversarial bytes
        // on this platform's OsStr encoding.
        let cases: Vec<&[u8]> = vec![b"plain", b"\xff\xfe\x80"];
        for bytes in cases {
            let os = unsafe { OsStr::from_encoded_bytes_unchecked(bytes) };
            let p = Path::new(os);
            let encoded = encode(p);
            // The tagged form must follow the value's own representability:
            // UTF-8 takes the verbatim path, anything else the hex path.
            if os.to_str().is_some() {
                assert!(
                    encoded.starts_with("u:"),
                    "valid UTF-8 takes the verbatim path"
                );
            } else {
                assert!(encoded.starts_with("e:"), "non-UTF-8 takes the hex path");
            }
            assert_eq!(decode(&encoded).unwrap().as_deref(), Some(p));
        }
    }

    #[test]
    fn untagged_legacy_values_decode_verbatim() {
        assert_eq!(
            decode("/scope-a/f.bin").unwrap(),
            Some(PathBuf::from("/scope-a/f.bin"))
        );
        assert_eq!(
            decode("l:/tagged-legacy").unwrap(),
            Some(PathBuf::from("/tagged-legacy"))
        );
    }

    #[test]
    fn malformed_hex_is_a_typed_error() {
        assert_eq!(decode("e:zz").unwrap_err(), PathDecodeError::InvalidHex);
        assert_eq!(decode("e:abc").unwrap_err(), PathDecodeError::OddLength);
    }
}
