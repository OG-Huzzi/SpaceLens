//! Content hashing — the single source of content identity.
//!
//! Algorithm: **SHA-256** ([`HashAlgorithm::Sha256`]), from the RustCrypto
//! `sha2` crate (pure Rust, MIT OR Apache-2.0, no transitive runtime
//! dependencies beyond `digest`/`block-buffer`/`generic-array`/`typenum`/
//! `cpufeatures`; actively maintained). Chosen because:
//! - the Phase 0 architecture contract already names SHA-256 for hashing
//!   (docs/ARCHITECTURE.md),
//! - it is a well-established, collision-resistant cryptographic hash — no
//!   custom cryptography is written here,
//! - pure Rust keeps the Windows/macOS/Linux CI matrix identical.
//!
//! Output representation: [`ContentHash`] — the raw 32-byte digest, strongly
//! typed so a digest can never be mistaken for a hash of a different
//! algorithm or a truncated prefix. Hex/IPC rendering happens only at the
//! contract boundary (`as_hex`).
//!
//! **Compatibility:** a content identity is only meaningful under the
//! algorithm that produced it. Changing the algorithm invalidates every
//! persisted identity; the algorithm tag travels inside
//! [`spacelens_engine::FsEntry`]-adjacent result types
//! (`HashResult::algorithm`) so a future change forces a visible contract
//! break instead of a silent mismatch.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bytes fed to the OS per read call. Bounded so no allocation ever scales
/// with file size, and large enough that syscall overhead is negligible.
pub const HASH_CHUNK_LEN: usize = 64 * 1024;

/// The hashing algorithm that produced a [`ContentHash`]. Part of the
/// identity contract: persisted identities are meaningless without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HashAlgorithm {
    Sha256,
}

impl HashAlgorithm {
    /// Digest length in bytes.
    pub fn digest_len(self) -> usize {
        match self {
            HashAlgorithm::Sha256 => 32,
        }
    }

    /// Stable IPC tag (`spacelens.v1.identity.*` contracts).
    pub fn tag(self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "sha256",
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// Cryptographic digest of file content. Identity of the *bytes*, not of the
/// path or the file object (see crate docs). Ordered byte-wise so identity
/// keys can live in ordered maps (deterministic grouping).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Byte length of a digest under the current algorithm.
    pub const LEN: usize = 32;

    /// The identity of empty content. All zero-byte files share it — this is
    /// correct (their content is identical) and the pipeline handles the
    /// resulting large groups explicitly (docs/IDENTITY.md §zero-byte).
    pub fn empty() -> Self {
        ContentHash(Sha256::digest([]).into())
    }

    /// Hash a byte slice (tests and small buffers; the file path is
    /// `hash_file`/`hash_reader`).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        ContentHash(Sha256::digest(bytes).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, for IPC/display only — never used as identity by the
    /// engine itself.
    pub fn as_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContentHash(")?;
        f.write_str(&self.as_hex())?;
        f.write_str(")")
    }
}

/// Streaming hasher over any byte source. Memory use is one caller-supplied
/// chunk buffer plus the fixed SHA-256 state — never proportional to
/// content length.
pub struct ContentHasher {
    inner: Sha256,
    bytes_read: u64,
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentHasher {
    pub fn new() -> Self {
        ContentHasher {
            inner: Sha256::new(),
            bytes_read: 0,
        }
    }

    /// Feed one chunk. Public so tests can simulate arbitrary byte streams;
    /// the pipeline drives it through `update_reader`.
    pub fn update(&mut self, bytes: &[u8]) {
        self.inner.update(bytes);
        self.bytes_read = self.bytes_read.saturating_add(bytes.len() as u64);
    }

    /// Total bytes ingested so far (observational; also used by the mutation
    /// policy to detect length changes mid-read).
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Finish and emit the digest plus the total length actually read.
    pub fn finalize(self) -> (ContentHash, u64) {
        (ContentHash(self.inner.finalize().into()), self.bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_answers() {
        // NIST FIPS 180-2 / NIST CSRC test vectors for SHA-256.
        let (h, n) = {
            let mut hasher = ContentHasher::new();
            hasher.update(b"abc");
            hasher.finalize()
        };
        assert_eq!(n, 3);
        assert_eq!(
            h.as_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let (h, n) = {
            let mut hasher = ContentHasher::new();
            hasher.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
            hasher.finalize()
        };
        assert_eq!(n, 56);
        assert_eq!(
            h.as_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn empty_content_identity() {
        let (h, n) = ContentHasher::new().finalize();
        assert_eq!(n, 0);
        assert_eq!(h, ContentHash::empty());
        assert_eq!(
            h.as_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn chunking_does_not_change_digest() {
        // Same bytes in different chunk boundaries must produce the same
        // identity (streaming == one-shot).
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let one_shot = ContentHash::from_bytes(&data);

        let mut h = ContentHasher::new();
        let mut rest = &data[..];
        while !rest.is_empty() {
            let take = take_varied(rest.len());
            h.update(&rest[..take]);
            rest = &rest[take..];
        }
        let (streamed, n) = h.finalize();
        assert_eq!(n, data.len() as u64);
        assert_eq!(streamed, one_shot);
    }

    fn take_varied(len: usize) -> usize {
        // Deliberately irregular to cross chunk-boundary logic.
        match len % 4 {
            0 => 1,
            1 => 7_777.min(len),
            2 => 65_536.min(len),
            _ => len,
        }
    }

    #[test]
    fn different_bytes_different_identity() {
        assert_ne!(
            ContentHash::from_bytes(b"content A"),
            ContentHash::from_bytes(b"content B")
        );
    }

    #[test]
    fn hex_is_lowercase_and_full_length() {
        let h = ContentHash::from_bytes(b"abc");
        let hex = h.as_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, hex.to_lowercase());
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
