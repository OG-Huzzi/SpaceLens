//! Semantic category taxonomy (Phase 2).
//!
//! These are engine-internal semantic categories, NOT UI labels. The UI may
//! group or rename them later (e.g. show "Videos" for `UserData::Media::Video`);
//! the engine contract stays stable and versioned (`spacelens.v1.*` naming
//! convention, docs/API_CONTRACTS.md).
//!
//! `Unknown` and `Other` are deliberately distinct:
//! - [`Category::Unknown`] = insufficient evidence to classify confidently.
//! - [`Category::Other`] = understood enough, but no more useful primary
//!   category applies. Never a dumping ground for failure to classify.

use serde::{Deserialize, Serialize};

/// Primary semantic categories. Order matters: it is the deterministic
/// aggregation/report order (stable IPC output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Category {
    Applications,
    Games,
    Documents,
    Images,
    Video,
    Audio,
    Downloads,
    Archives,
    Development,
    TemporaryData,
    Cache,
    Logs,
    Backups,
    SystemData,
    UserData,
    Other,
    Unknown,
}

impl Category {
    /// Stable IPC identifier (contract surface, never renamed inside v1).
    pub fn code(self) -> &'static str {
        match self {
            Category::Applications => "APPLICATIONS",
            Category::Games => "GAMES",
            Category::Documents => "DOCUMENTS",
            Category::Images => "IMAGES",
            Category::Video => "VIDEO",
            Category::Audio => "AUDIO",
            Category::Downloads => "DOWNLOADS",
            Category::Archives => "ARCHIVES",
            Category::Development => "DEVELOPMENT",
            Category::TemporaryData => "TEMPORARY_DATA",
            Category::Cache => "CACHE",
            Category::Logs => "LOGS",
            Category::Backups => "BACKUPS",
            Category::SystemData => "SYSTEM_DATA",
            Category::UserData => "USER_DATA",
            Category::Other => "OTHER",
            Category::Unknown => "UNKNOWN",
        }
    }

    /// All categories in the canonical (report/aggregation) order.
    pub const ALL: [Category; 17] = [
        Category::Applications,
        Category::Games,
        Category::Documents,
        Category::Images,
        Category::Video,
        Category::Audio,
        Category::Downloads,
        Category::Archives,
        Category::Development,
        Category::TemporaryData,
        Category::Cache,
        Category::Logs,
        Category::Backups,
        Category::SystemData,
        Category::UserData,
        Category::Other,
        Category::Unknown,
    ];

    /// Categories that are "terminal buckets" rather than semantic subjects.
    pub fn is_bucket(self) -> bool {
        matches!(self, Category::Other | Category::Unknown)
    }
}

/// Subcategory refinement. Deliberately small and evidence-backed only —
/// quality over quantity (master prompt §41).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Subcategory {
    /// Executable/package installer (setup.exe, .msi, .dmg, .pkg, .deb, .rpm…).
    Installer,
    /// Compressed archive (.zip, .tar.gz, .7z, …).
    Archive,
    /// Disk image (.iso, .img).
    DiskImage,
    /// Runtime/dependency directory (node_modules, target/, venv, .git).
    DependencyTree,
    /// Build output (target/debug, dist, build, …).
    BuildOutput,
    /// Version-control internals (.git).
    VcsInternals,
    /// Log file (.log, .log.1 rotation).
    LogFile,
}

impl Subcategory {
    /// Stable IPC identifier.
    pub fn code(self) -> &'static str {
        match self {
            Subcategory::Installer => "INSTALLER",
            Subcategory::Archive => "ARCHIVE",
            Subcategory::DiskImage => "DISK_IMAGE",
            Subcategory::DependencyTree => "DEPENDENCY_TREE",
            Subcategory::BuildOutput => "BUILD_OUTPUT",
            Subcategory::VcsInternals => "VCS_INTERNALS",
            Subcategory::LogFile => "LOG_FILE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in Category::ALL {
            let code = c.code();
            assert!(!code.is_empty());
            assert!(seen.insert(code), "duplicate category code: {code}");
        }
        assert_eq!(Category::ALL.len(), 17);
    }

    #[test]
    fn unknown_and_other_are_distinct_buckets() {
        assert_ne!(Category::Unknown, Category::Other);
        assert!(Category::Unknown.is_bucket());
        assert!(Category::Other.is_bucket());
        assert!(!Category::Cache.is_bucket());
    }

    #[test]
    fn subcategory_codes_unique() {
        let subs = [
            Subcategory::Installer,
            Subcategory::Archive,
            Subcategory::DiskImage,
            Subcategory::DependencyTree,
            Subcategory::BuildOutput,
            Subcategory::VcsInternals,
            Subcategory::LogFile,
        ];
        let mut seen = std::collections::HashSet::new();
        for s in subs {
            assert!(seen.insert(s.code()));
        }
    }

    #[test]
    fn serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_string(&Category::TemporaryData).unwrap(),
            "\"TEMPORARY_DATA\""
        );
        assert_eq!(
            serde_json::to_string(&Subcategory::Installer).unwrap(),
            "\"INSTALLER\""
        );
    }
}
