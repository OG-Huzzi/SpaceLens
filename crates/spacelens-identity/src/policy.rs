//! Policy knobs with documented defaults (Phase 3 STEPS 4–6, 13).
//!
//! Everything here exists to keep the pipeline *bounded and honest*:
//! candidate staging, group reporting, worker counts, and the mutation
//! policy live behind explicit, testable values rather than magic numbers
//! buried in the algorithm.

use serde::{Deserialize, Serialize};
use std::path::Path;

use spacelens_engine::platform::{ContentOutcome, ContentReader};

/// Hard cap on how many members of one duplicate group the report keeps
/// inline. A hostile tree can put millions of paths into one content group
/// (a million zero-byte files); the *counts* stay exact, only the member
/// detail is capped. This is reporting, not grouping.
pub const DEFAULT_MAX_GROUP_MEMBERS_REPORTED: usize = 64;

/// Hard cap on how many candidates per size group are hashed. Bounding this
/// keeps memory flat under the most hostile input (millions of same-size
/// files); overflow is counted and reported, never silently dropped.
pub const DEFAULT_MAX_CANDIDATES_PER_GROUP: usize = 1_000_000;

/// Bounded default worker pool. The scanner's model: metadata/content work
/// is I/O-bound, an 8 GB machine is never saturated, callers may override.
/// Never thread-per-file; never unbounded.
pub fn default_hash_threads() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    cpus.clamp(2, 4)
}

/// Policy for files whose content changes while being read.
///
/// A mid-read change means the digest belongs to no single state of the
/// file: publishing it could create a false duplicate relationship. The
/// policy chosen here (and pinned by tests) is **reject**: the candidate is
/// excluded and a typed [`crate::error::HashFailureKind::Changed`] failure
/// is recorded. Retrying is deliberately not automatic — a churning file is
/// unlikely to be stable on the next read either, and a deterministic
/// single-pass result is easier to explain and to verify.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationPolicy {
    /// Reject any file whose observed metadata (kind, size) or length does
    /// not match exactly what was read. **Default and only implemented
    /// policy** — honesty over throughput.
    #[default]
    Reject,
}

/// How a reader factory obtains a [`ContentReader`] for one path.
///
/// The duplicate engine never crawls the filesystem and never opens files by
/// itself: it asks the platform boundary. `DefaultReaderFactory` forwards to
/// `PlatformFs::read_content`; tests inject factories over in-memory data.
pub trait ContentReaderFactory: Send + Sync {
    /// Drive one file's content through `feed`, or fail with a typed error.
    fn read(
        &self,
        path: &Path,
        feed: &mut dyn FnMut(&mut dyn ContentReader) -> std::io::Result<()>,
    ) -> Result<(), spacelens_engine::platform::ContentError>;
}

/// Production factory: forwards to the engine's `PlatformFs` content
/// boundary. Holds no state; content access stays behind the same platform
/// abstraction as every other filesystem operation.
pub struct DefaultReaderFactory<'p> {
    platform: &'p dyn spacelens_engine::PlatformFs,
}

impl<'p> DefaultReaderFactory<'p> {
    pub fn new(platform: &'p dyn spacelens_engine::PlatformFs) -> Self {
        DefaultReaderFactory { platform }
    }
}

impl ContentReaderFactory for DefaultReaderFactory<'_> {
    fn read(
        &self,
        path: &Path,
        feed: &mut dyn FnMut(&mut dyn ContentReader) -> std::io::Result<()>,
    ) -> Result<(), spacelens_engine::platform::ContentError> {
        self.platform
            .read_content(path, ContentOutcome::Opened(feed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_reject() {
        assert_eq!(MutationPolicy::default(), MutationPolicy::Reject);
    }
    #[test]
    fn defaults_are_bounded() {
        const {
            assert!(DEFAULT_MAX_GROUP_MEMBERS_REPORTED >= 2);
            assert!(DEFAULT_MAX_CANDIDATES_PER_GROUP >= DEFAULT_MAX_GROUP_MEMBERS_REPORTED);
        }
        let t = default_hash_threads();
        assert!(
            (2..=4).contains(&t),
            "default hash threads must be bounded: {t}"
        );
    }
}
