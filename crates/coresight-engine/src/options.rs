//! Scan request configuration.

use std::time::Duration;

/// How the scanner treats symlinks, junctions, mount points and other reparse
/// points. The default is deliberately conservative: **record, never follow**.
///
/// Rationale (docs/CROSS_PLATFORM.md, docs/SECURITY_AND_SAFETY.md): links are
/// the only source of filesystem cycles on NTFS/APFS/ext4 volumes, and
/// following them blindly risks infinite traversal, double counting, and
/// scanning into unrelated volumes. A broken or inaccessible link is recorded
/// as such and never fails the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    /// Record links as [`crate::model::EntryKind::Link`] with their target
    /// (when readable) and never recurse into them. **Default.**
    RecordOnly,
    /// Additionally recurse into directory links whose target the platform
    /// can identify with a stable `(device, inode)` identity. Cycle-safe: a
    /// directory already visited under that identity is never entered twice.
    /// Links without a stable identity are recorded, not followed.
    /// Must be enabled explicitly; never the default.
    FollowWithCycleGuard,
}

/// Scan configuration. Root selection is always explicit — the engine never
/// picks roots or volumes by itself.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Worker threads for directory traversal. Bounded by design; the default
    /// is small (see [`default_threads`]) so an 8 GB machine is never
    /// saturated. Files are never given their own tasks.
    pub threads: usize,
    pub symlink_policy: SymlinkPolicy,
    /// Hard traversal depth cap. `None` = unlimited (safe: real directories
    /// form a finite DAG once links are not followed). A cap can be set by
    /// callers who want belt-and-braces protection.
    pub max_depth: Option<u32>,
    /// Emit [`crate::progress::ScanEvent::Entry`] for every discovered entry.
    /// Turning this off yields counters-only scans (cheapest mode).
    pub emit_entries: bool,
    /// Minimum interval between `Progress` events. The contract caps progress
    /// at ≤4 events/sec; the default of 250 ms matches exactly.
    pub progress_interval: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            threads: default_threads(),
            symlink_policy: SymlinkPolicy::RecordOnly,
            max_depth: None,
            emit_entries: true,
            progress_interval: Duration::from_millis(250),
        }
    }
}

/// Conservative default worker count for bounded traversal.
///
/// Deliberately NOT `available_parallelism`: metadata scanning is IO-bound,
/// and this product must stay practical on low-memory machines. Four workers
/// give concurrency without unbounded IO depth. Callers may override.
pub fn default_threads() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    cpus.clamp(2, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative() {
        let opts = ScanOptions::default();
        assert_eq!(opts.symlink_policy, SymlinkPolicy::RecordOnly);
        assert_eq!(opts.max_depth, None);
        assert!(opts.threads <= 4, "default threads must stay bounded");
        assert_eq!(opts.progress_interval, Duration::from_millis(250));
    }

    #[test]
    fn default_threads_never_exceeds_four() {
        assert!(default_threads() >= 2 && default_threads() <= 4);
    }
}
