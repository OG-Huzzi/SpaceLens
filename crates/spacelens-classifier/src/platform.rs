//! Platform identity for classification.
//!
//! The core rule engine is platform-neutral: platform knowledge enters as a
//! *data* field ([`Platform`]) on the classification input, never as `cfg!`
//! branches scattered through shared logic (docs/CROSS_PLATFORM.md). Exactly
//! one `cfg!` site exists in this crate — [`Platform::current`] — mirroring
//! the Phase 1 engine's discipline.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Platform {
    Windows,
    Mac,
    Linux,
}

impl Platform {
    /// The only `cfg!` site in this crate.
    pub fn current() -> Platform {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        }
    }

    /// Whether filesystem name comparisons on this platform are
    /// case-insensitive. Windows: yes (NTFS default). macOS: APFS/HFS+ are
    /// case-insensitive by default. Linux: conventionally case-sensitive.
    pub fn case_insensitive_names(self) -> bool {
        matches!(self, Platform::Windows | Platform::Mac)
    }

    pub fn code(self) -> &'static str {
        match self {
            Platform::Windows => "WINDOWS",
            Platform::Mac => "MAC",
            Platform::Linux => "LINUX",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_sensitivity_matches_platform_convention() {
        assert!(Platform::Windows.case_insensitive_names());
        assert!(Platform::Mac.case_insensitive_names());
        assert!(!Platform::Linux.case_insensitive_names());
    }

    #[test]
    fn codes_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in [Platform::Windows, Platform::Mac, Platform::Linux] {
            assert!(seen.insert(p.code()));
        }
    }
}
