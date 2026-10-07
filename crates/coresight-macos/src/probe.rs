//! The raw filesystem facts behind a macOS source observation.
//!
//! Platform behavior hides behind [`MacFileProbe`]; the fact→state mapping
//! lives in [`crate::observation`] as shared, platform-independent logic so
//! it is testable on every platform with scripted probes.

use std::fs;
use std::io;
use std::path::Path;

/// What link-aware metadata said about one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Dir,
    File,
    Symlink,
    Other,
}

/// Bounded child count from one directory listing. Exact accounting: a
/// truncated listing is flagged, never silently capped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildCount {
    pub count: u64,
    pub truncated: bool,
}

/// The filesystem facts an observation needs. Read-only by contract: no
/// content access, no recursion, no writes.
pub trait MacFileProbe: Send + Sync {
    /// Link-aware metadata (lstat semantics): describes the entry itself.
    fn metadata(&self, path: &Path) -> io::Result<ProbeKind>;

    /// List up to `max` children; `truncated` is true when more exist.
    /// An `Err` means the directory could not be read (including a
    /// mid-iteration failure — honesty over partial answers).
    fn list_children_bounded(&self, path: &Path, max: u64) -> io::Result<ChildCount>;
}

/// The std-based probe (production).
pub struct StdProbe;

impl MacFileProbe for StdProbe {
    fn metadata(&self, path: &Path) -> io::Result<ProbeKind> {
        let md = fs::symlink_metadata(path)?;
        let ft = md.file_type();
        Ok(if ft.is_symlink() {
            ProbeKind::Symlink
        } else if ft.is_dir() {
            ProbeKind::Dir
        } else if ft.is_file() {
            ProbeKind::File
        } else {
            ProbeKind::Other
        })
    }

    fn list_children_bounded(&self, path: &Path, max: u64) -> io::Result<ChildCount> {
        let rd = fs::read_dir(path)?;
        let mut count: u64 = 0;
        let mut truncated = false;
        for entry in rd {
            entry?;
            if count < max {
                count += 1;
            } else {
                // One entry beyond the bound proves truncation exactly.
                truncated = true;
                break;
            }
        }
        Ok(ChildCount { count, truncated })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_listing_counts_exactly_and_flags_truncation() {
        let dir = tempfile::tempdir().expect("temp fixture");
        for i in 0..5 {
            fs::write(dir.path().join(format!("f{i}.txt")), b"x").expect("fixture file");
        }
        let probe = StdProbe;
        let full = probe
            .list_children_bounded(dir.path(), 10)
            .expect("listing works");
        assert_eq!(
            full,
            ChildCount {
                count: 5,
                truncated: false
            }
        );

        let capped = probe
            .list_children_bounded(dir.path(), 3)
            .expect("listing works");
        assert_eq!(
            capped,
            ChildCount {
                count: 3,
                truncated: true
            }
        );
    }

    #[test]
    fn metadata_maps_kinds_without_links() {
        let dir = tempfile::tempdir().expect("temp fixture");
        fs::write(dir.path().join("file.txt"), b"x").expect("fixture file");
        let probe = StdProbe;
        assert_eq!(
            probe.metadata(&dir.path().join("file.txt")).unwrap(),
            ProbeKind::File
        );
        assert_eq!(probe.metadata(dir.path()).unwrap(), ProbeKind::Dir);
        assert_eq!(
            probe
                .metadata(&dir.path().join("missing"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
