//! Semantic category taxonomy (Phase 2).
//!
//! These are engine-internal semantic categories, NOT UI labels. The UI may
//! group or rename them later (e.g. show "Videos" for `UserData::Media::Video`);
//! the engine contract stays stable and versioned (`spacelens.v1.*` naming
//! convention, docs/API_CONTRACTS.md).
//!
//! `Unknown` and `Other` are deliberately distinct — see
//! docs/CLASSIFICATION.md for the full contract:
//! - [`Category::Unknown`] = SpaceLens cannot establish even a basic semantic
//!   interpretation of the observation (unusable name, incomplete metadata,
//!   uninterpretable entry kind).
//! - [`Category::Other`] = the entry is understood at a basic level (a regular
//!   file, a directory) but no more useful primary category applies. Never a
//!   dumping ground for failure to classify.

use serde::{Deserialize, Serialize};

/// Primary semantic categories. Order matters: it is the deterministic
/// aggregation/report order (stable IPC output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Category {
    /// Installed application *code* (Program Files, /Applications, /opt).
    Applications,
    /// Data owned by an application: per-user (AppData, ~/Library/Application
    /// Support, ~/.config) or machine-wide (ProgramData). Deliberately NOT
    /// [`Category::Applications`] — SpaceLens must be able to say "this is how
    /// much space this application uses" without conflating the program with
    /// the data it produced.
    ApplicationData,
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
            Category::ApplicationData => "APPLICATION_DATA",
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

    /// Number of primary categories. Kept explicit so aggregation arrays and
    /// `Category::ALL` cannot silently drift apart.
    pub const COUNT: usize = 18;

    /// All categories in the canonical (report/aggregation) order.
    pub const ALL: [Category; 18] = [
        Category::Applications,
        Category::ApplicationData,
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
        assert_eq!(Category::ALL.len(), 18);
        assert_eq!(Category::ALL.len(), Category::COUNT);
    }

    #[test]
    fn application_data_is_not_applications() {
        // Finding 2: installation location and application-owned data must
        // never be conflated — "how much space does this app use?" depends on
        // it. They are distinct codes and only one is an application.
        assert_ne!(Category::Applications, Category::ApplicationData);
        assert_eq!(Category::ApplicationData.code(), "APPLICATION_DATA");
    }

    #[test]
    fn unknown_and_other_are_distinct_buckets() {
        assert_ne!(Category::Unknown, Category::Other);
        assert!(Category::Unknown.is_bucket());
        assert!(Category::Other.is_bucket());
        assert!(!Category::Cache.is_bucket());
        assert!(!Category::ApplicationData.is_bucket());
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
