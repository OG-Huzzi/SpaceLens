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

use std::ffi::OsString;
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
        let os = os_string_from_encoded(&bytes)?;
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
    /// `e:`-tagged bytes are not a valid platform path encoding.
    InvalidEncoding,
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
            PathDecodeError::InvalidEncoding => {
                write!(
                    f,
                    "stored path: invalid platform encoding in e:-tagged value"
                )
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

#[cfg(unix)]
fn os_string_from_encoded(bytes: &[u8]) -> Result<OsString, PathDecodeError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(bytes.to_vec()))
}

#[cfg(windows)]
fn os_string_from_encoded(bytes: &[u8]) -> Result<OsString, PathDecodeError> {
    use std::os::windows::ffi::OsStringExt;
    let wide = decode_wtf8(bytes).ok_or(PathDecodeError::InvalidEncoding)?;
    Ok(OsString::from_wide(&wide))
}

#[cfg(not(any(unix, windows)))]
fn os_string_from_encoded(bytes: &[u8]) -> Result<OsString, PathDecodeError> {
    let text = std::str::from_utf8(bytes).map_err(|_| PathDecodeError::InvalidEncoding)?;
    Ok(OsString::from(text))
}

/// Decode canonical WTF-8 (UTF-8 plus unpaired surrogate code points) into
/// UTF-16 without invoking an unsafe OS-string constructor. Adjacent encoded
/// high+low surrogates are rejected: a valid pair must use its scalar UTF-8
/// encoding, as produced by `OsStr::as_encoded_bytes`.
#[cfg(windows)]
fn decode_wtf8(bytes: &[u8]) -> Option<Vec<u16>> {
    let mut wide = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut previous_high_surrogate = false;
    while index < bytes.len() {
        let first = bytes[index];
        let (code_point, width) = match first {
            0x00..=0x7f => (u32::from(first), 1),
            0xc2..=0xdf => {
                let second = *bytes.get(index + 1)?;
                if !(0x80..=0xbf).contains(&second) {
                    return None;
                }
                ((u32::from(first & 0x1f) << 6) | u32::from(second & 0x3f), 2)
            }
            0xe0..=0xef => {
                let second = *bytes.get(index + 1)?;
                let third = *bytes.get(index + 2)?;
                let second_valid = match first {
                    0xe0 => (0xa0..=0xbf).contains(&second),
                    _ => (0x80..=0xbf).contains(&second),
                };
                if !second_valid || !(0x80..=0xbf).contains(&third) {
                    return None;
                }
                (
                    (u32::from(first & 0x0f) << 12)
                        | (u32::from(second & 0x3f) << 6)
                        | u32::from(third & 0x3f),
                    3,
                )
            }
            0xf0..=0xf4 => {
                let second = *bytes.get(index + 1)?;
                let third = *bytes.get(index + 2)?;
                let fourth = *bytes.get(index + 3)?;
                let second_valid = match first {
                    0xf0 => (0x90..=0xbf).contains(&second),
                    0xf4 => (0x80..=0x8f).contains(&second),
                    _ => (0x80..=0xbf).contains(&second),
                };
                if !second_valid
                    || !(0x80..=0xbf).contains(&third)
                    || !(0x80..=0xbf).contains(&fourth)
                {
                    return None;
                }
                (
                    (u32::from(first & 0x07) << 18)
                        | (u32::from(second & 0x3f) << 12)
                        | (u32::from(third & 0x3f) << 6)
                        | u32::from(fourth & 0x3f),
                    4,
                )
            }
            _ => return None,
        };
        if previous_high_surrogate && (0xdc00..=0xdfff).contains(&code_point) {
            return None;
        }
        previous_high_surrogate = (0xd800..=0xdbff).contains(&code_point);
        if code_point <= 0xffff {
            wide.push(code_point as u16);
        } else {
            let supplementary = code_point - 0x1_0000;
            wide.push(0xd800 | (supplementary >> 10) as u16);
            wide.push(0xdc00 | (supplementary & 0x3ff) as u16);
        }
        index += width;
    }
    Some(wide)
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
        // The unpaired-surrogate WTF-8 sequence is valid platform encoding
        // on Windows and non-UTF-8 arbitrary bytes on Unix. Never feed invalid
        // encoded bytes to the unsafe constructor on Windows.
        let cases: Vec<&[u8]> = vec![b"plain", b"\xed\xa0\x80"];
        for bytes in cases {
            let os = os_string_from_encoded(bytes).unwrap();
            let p = Path::new(&os);
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

    #[cfg(windows)]
    #[test]
    fn malformed_wtf8_is_rejected_before_path_construction() {
        for stored in ["e:ff", "e:f08080", "e:eda080edb080"] {
            assert_eq!(decode(stored), Err(PathDecodeError::InvalidEncoding));
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
