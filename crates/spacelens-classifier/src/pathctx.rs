//! Host-independent path analysis and authoritative location knowledge.
//!
//! # Why this module exists
//!
//! Classifying an entry by its **basename alone** is deterministic but not
//! semantically accurate: `/home/user/project/cache` and
//! `/Users/user/Library/Caches` are not the same claim, even though both end
//! in a cache-ish name. SpaceLens therefore distinguishes two *different
//! strengths of knowledge*:
//!
//! * **Authoritative location knowledge** — a *rooted* path prefix that the
//!   platform defines: `C:/Program Files`, `C:/Users/<u>/AppData`,
//!   `/Users/<u>/Library/Caches`, `/usr`, `/var/log`, `/Applications`, …
//! * **Weak basename heuristics** — a bare name that merely *looks* like
//!   something: `build`, `out`, `cache`, `backup`, `tmp`, `logs`.
//!
//! Only the first may reach [`crate::Confidence::High`]; see
//! [`crate::confidence`] for the mechanical policy.
//!
//! # Host independence
//!
//! Paths are split on **both** `/` and `\`, with Windows drive prefixes
//! (`C:`) discarded, and matching is anchored at the first component. No
//! `std::path` separator semantics are involved, so a synthetic Windows path
//! analysed with [`Platform::Windows`](crate::Platform::Windows) behaves
//! identically on a Linux or macOS test host.
//!
//! # Bounded and pure
//!
//! Analysis is allocation-free (fixed-size component buffer, capped at
//! [`MAX_LOCATION_DEPTH`]) and performs no I/O of any kind.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::category::Category;
use crate::evidence::{EvidenceKind, RuleId};
use crate::platform::Platform;

/// How many leading path components are considered. Location patterns are
/// shallow (at most 4 components); deeper components are irrelevant to
/// location matching, and capping here keeps the analysis allocation-free and
/// independent of pathological path depth.
pub const MAX_LOCATION_DEPTH: usize = 8;

/// Semantic class of an authoritative filesystem location.
///
/// Deliberately larger than the [`Category`] set in one respect and smaller in
/// another: it separates *where something lives* (`ApplicationInstall` vs
/// `ApplicationData`) which the primary taxonomy also needs, and it keeps
/// `UserHome` as a pure container that carries no subject-matter signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LocationClass {
    /// OS-managed system tree (`C:/Windows`, `/usr`, `/etc`, `/var`, `/System`).
    System,
    /// Where application *code* is installed (`Program Files`, `/Applications`,
    /// `/opt`).
    ApplicationInstall,
    /// Where applications keep their own data (`AppData`,
    /// `~/Library/Application Support`, `~/.config`, `ProgramData`).
    ApplicationData,
    /// Platform-defined cache tree (`~/Library/Caches`, `/var/cache`, `~/.cache`).
    Cache,
    /// Platform-defined log tree (`~/Library/Logs`, `/var/log`).
    Logs,
    /// Platform-defined transient tree (`/tmp`, `AppData/Local/Temp`).
    Temporary,
    /// The canonical download location of a user profile.
    Downloads,
    /// The canonical documents root of a user profile (the *container* is user
    /// data; the documents inside it are `Documents`).
    UserDocuments,
    /// The canonical desktop root of a user profile.
    UserDesktop,
    /// A user home/profile root. A pure container: it tells us *whose* tree we
    /// are in, never *what* something is.
    UserHome,
}

impl LocationClass {
    /// Stable IPC identifier.
    pub fn code(self) -> &'static str {
        match self {
            LocationClass::System => "SYSTEM",
            LocationClass::ApplicationInstall => "APPLICATION_INSTALL",
            LocationClass::ApplicationData => "APPLICATION_DATA",
            LocationClass::Cache => "CACHE",
            LocationClass::Logs => "LOGS",
            LocationClass::Temporary => "TEMPORARY",
            LocationClass::Downloads => "DOWNLOADS",
            LocationClass::UserDocuments => "USER_DOCUMENTS",
            LocationClass::UserDesktop => "USER_DESKTOP",
            LocationClass::UserHome => "USER_HOME",
        }
    }

    /// Whether this class carries subject-matter information capable of
    /// corroborating a weak name heuristic.
    ///
    /// `UserHome` deliberately does **not**: knowing that an entry lives
    /// somewhere under `/home` says nothing about what it is, so it must not
    /// be allowed to promote `/home/user/project/build` out of the heuristic
    /// confidence band.
    pub const fn corroborates(self) -> bool {
        self.carries_subject_matter()
    }

    /// Whether an entry with no signal of its own may inherit this location's
    /// category.
    ///
    /// `UserHome` may not: a directory that merely sits under `/home` is not
    /// thereby user data, and pretending otherwise would make `Other`
    /// unreachable across most of a user's tree.
    pub const fn inherits_to_contents(self) -> bool {
        self.carries_subject_matter()
    }

    /// True for every class except the pure containers. `UserHome` tells us
    /// *whose* tree we are in — never *what* something is.
    const fn carries_subject_matter(self) -> bool {
        !matches!(self, LocationClass::UserHome)
    }
}

/// The primary category a location class implies.
///
/// Note that `UserDocuments`/`UserDesktop`/`UserHome` map to `UserData`: the
/// *container* is user data, while a `.pdf` inside it is `Documents`.
pub const fn location_category(class: LocationClass) -> Category {
    match class {
        LocationClass::System => Category::SystemData,
        LocationClass::ApplicationInstall => Category::Applications,
        LocationClass::ApplicationData => Category::ApplicationData,
        LocationClass::Cache => Category::Cache,
        LocationClass::Logs => Category::Logs,
        LocationClass::Temporary => Category::TemporaryData,
        LocationClass::Downloads => Category::Downloads,
        LocationClass::UserDocuments | LocationClass::UserDesktop | LocationClass::UserHome => {
            Category::UserData
        }
    }
}

/// A rooted location pattern set. Patterns are slash-separated component
/// sequences matched against the **leading** components of the path; `*`
/// matches exactly one component.
///
/// The most specific (longest) matching pattern wins, so `/var/log` beats
/// `/var` and `/users/*/library/caches` beats `/users/*/library`.
#[derive(Debug, Clone, Copy)]
pub struct LocationRule {
    /// Stable identifier (also used as the evidence rule id). Must resolve to
    /// the same category as [`location_category`] of `class`.
    pub id: RuleId,
    /// Platforms this rule applies to; empty = all.
    pub platforms: &'static [Platform],
    pub class: LocationClass,
    /// Evidence kind recorded when this location matches.
    pub evidence: EvidenceKind,
    /// Rooted component patterns, e.g. `"/users/*/appdata"`.
    pub patterns: &'static [&'static str],
}

impl LocationRule {
    pub fn applies_to(&self, platform: Platform) -> bool {
        self.platforms.is_empty() || self.platforms.contains(&platform)
    }
}

/// Authoritative location table. Order is contract only as a tie-break between
/// equally specific patterns.
pub const LOCATION_RULES: &[LocationRule] = &[
    // -- Windows ----------------------------------------------------------
    LocationRule {
        id: RuleId::WindowsSystemLocation,
        platforms: &[Platform::Windows],
        class: LocationClass::System,
        evidence: EvidenceKind::KnownSystemLocation,
        patterns: &["/windows"],
    },
    LocationRule {
        id: RuleId::ApplicationInstallLocation,
        platforms: &[Platform::Windows],
        class: LocationClass::ApplicationInstall,
        evidence: EvidenceKind::KnownApplicationLocation,
        patterns: &["/program files", "/program files (x86)"],
    },
    LocationRule {
        id: RuleId::WindowsProgramData,
        platforms: &[Platform::Windows],
        class: LocationClass::ApplicationData,
        evidence: EvidenceKind::KnownApplicationLocation,
        patterns: &["/programdata"],
    },
    LocationRule {
        id: RuleId::UserProfile,
        platforms: &[Platform::Windows],
        class: LocationClass::UserHome,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*"],
    },
    LocationRule {
        id: RuleId::WindowsAppData,
        platforms: &[Platform::Windows],
        class: LocationClass::ApplicationData,
        evidence: EvidenceKind::KnownApplicationLocation,
        patterns: &["/users/*/appdata"],
    },
    LocationRule {
        id: RuleId::TempDir,
        platforms: &[Platform::Windows],
        class: LocationClass::Temporary,
        evidence: EvidenceKind::KnownTemporaryLocation,
        patterns: &["/users/*/appdata/local/temp", "/windows/temp"],
    },
    LocationRule {
        id: RuleId::DownloadsDir,
        platforms: &[Platform::Windows],
        class: LocationClass::Downloads,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/downloads"],
    },
    LocationRule {
        id: RuleId::DocumentsDir,
        platforms: &[Platform::Windows],
        class: LocationClass::UserDocuments,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/documents", "/users/*/my documents"],
    },
    LocationRule {
        id: RuleId::DesktopDir,
        platforms: &[Platform::Windows],
        class: LocationClass::UserDesktop,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/desktop"],
    },
    // -- macOS ------------------------------------------------------------
    LocationRule {
        id: RuleId::MacSystemLocation,
        platforms: &[Platform::Mac],
        class: LocationClass::System,
        evidence: EvidenceKind::KnownSystemLocation,
        patterns: &["/system", "/private"],
    },
    LocationRule {
        id: RuleId::ApplicationInstallLocation,
        platforms: &[Platform::Mac],
        class: LocationClass::ApplicationInstall,
        evidence: EvidenceKind::KnownApplicationLocation,
        patterns: &["/applications"],
    },
    LocationRule {
        id: RuleId::MacApplicationSupport,
        platforms: &[Platform::Mac],
        class: LocationClass::ApplicationData,
        evidence: EvidenceKind::KnownApplicationLocation,
        patterns: &[
            "/library",
            "/users/*/library",
            "/users/*/library/application support",
        ],
    },
    LocationRule {
        id: RuleId::UserProfile,
        platforms: &[Platform::Mac],
        class: LocationClass::UserHome,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*"],
    },
    LocationRule {
        id: RuleId::CacheDir,
        platforms: &[Platform::Mac],
        class: LocationClass::Cache,
        evidence: EvidenceKind::KnownCacheLocation,
        patterns: &["/users/*/library/caches"],
    },
    LocationRule {
        id: RuleId::LogDir,
        platforms: &[Platform::Mac],
        class: LocationClass::Logs,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/library/logs"],
    },
    LocationRule {
        id: RuleId::TempDir,
        platforms: &[Platform::Mac],
        class: LocationClass::Temporary,
        evidence: EvidenceKind::KnownTemporaryLocation,
        patterns: &["/tmp", "/var/folders"],
    },
    LocationRule {
        id: RuleId::DownloadsDir,
        platforms: &[Platform::Mac],
        class: LocationClass::Downloads,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/downloads"],
    },
    LocationRule {
        id: RuleId::DocumentsDir,
        platforms: &[Platform::Mac],
        class: LocationClass::UserDocuments,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/documents"],
    },
    LocationRule {
        id: RuleId::DesktopDir,
        platforms: &[Platform::Mac],
        class: LocationClass::UserDesktop,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/users/*/desktop"],
    },
    // -- Linux ------------------------------------------------------------
    // `/usr`, `/etc`, `/var` are OS-managed system trees; `/opt` and `/snap`
    // are third-party application *install* locations. They are not the same
    // semantic entity and are therefore not collapsed into one rule.
    LocationRule {
        id: RuleId::LinuxPackageLocation,
        platforms: &[Platform::Linux],
        class: LocationClass::System,
        evidence: EvidenceKind::KnownSystemLocation,
        patterns: &["/usr", "/etc", "/var", "/bin", "/sbin", "/lib", "/lib64"],
    },
    LocationRule {
        id: RuleId::ApplicationInstallLocation,
        platforms: &[Platform::Linux],
        class: LocationClass::ApplicationInstall,
        evidence: EvidenceKind::KnownApplicationLocation,
        patterns: &["/opt", "/snap"],
    },
    LocationRule {
        id: RuleId::LogDir,
        platforms: &[Platform::Linux],
        class: LocationClass::Logs,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/var/log"],
    },
    LocationRule {
        id: RuleId::CacheDir,
        platforms: &[Platform::Linux],
        class: LocationClass::Cache,
        evidence: EvidenceKind::KnownCacheLocation,
        patterns: &["/var/cache"],
    },
    LocationRule {
        id: RuleId::TempDir,
        platforms: &[Platform::Linux],
        class: LocationClass::Temporary,
        evidence: EvidenceKind::KnownTemporaryLocation,
        patterns: &["/tmp", "/var/tmp"],
    },
    LocationRule {
        id: RuleId::UserProfile,
        platforms: &[Platform::Linux],
        class: LocationClass::UserHome,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/home/*", "/root"],
    },
    LocationRule {
        id: RuleId::CacheDir,
        platforms: &[Platform::Linux],
        class: LocationClass::Cache,
        evidence: EvidenceKind::KnownCacheLocation,
        patterns: &["/home/*/.cache"],
    },
    // XDG config/data dirs are owned by applications, not authored by the
    // user: they answer "how much space does this application use?".
    LocationRule {
        id: RuleId::XdgLocation,
        platforms: &[Platform::Linux],
        class: LocationClass::ApplicationData,
        evidence: EvidenceKind::PlatformLocation,
        patterns: &["/home/*/.config", "/home/*/.local/share"],
    },
    // XDG user dirs are conventionally capitalised on Linux, and Linux name
    // matching is case-sensitive: both spellings are listed explicitly rather
    // than weakening case handling for the whole platform.
    LocationRule {
        id: RuleId::DownloadsDir,
        platforms: &[Platform::Linux],
        class: LocationClass::Downloads,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/home/*/downloads", "/home/*/Downloads"],
    },
    LocationRule {
        id: RuleId::DocumentsDir,
        platforms: &[Platform::Linux],
        class: LocationClass::UserDocuments,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/home/*/documents", "/home/*/Documents"],
    },
    LocationRule {
        id: RuleId::DesktopDir,
        platforms: &[Platform::Linux],
        class: LocationClass::UserDesktop,
        evidence: EvidenceKind::KnownPathPattern,
        patterns: &["/home/*/desktop", "/home/*/Desktop"],
    },
];

/// What an entry's location tells us, once the most specific matching
/// authoritative pattern has been selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocationMatch {
    /// The location rule that matched.
    pub rule: RuleId,
    pub class: LocationClass,
    /// Evidence kind this location contributes.
    pub evidence: EvidenceKind,
    /// Number of leading components the winning pattern covered — the
    /// specificity used to pick the most precise location.
    pub depth: usize,
    /// True when the entry **is** the location (`C:/Windows`) rather than
    /// merely *inside* it (`C:/Windows/System32/foo.dll`).
    ///
    /// This matters for pure containers: `/home/user` is user data, but
    /// `/home/user/anything` is not user data merely by being there.
    pub is_root: bool,
}

impl LocationMatch {
    /// The category this location implies.
    pub const fn category(self) -> Category {
        location_category(self.class)
    }

    /// Whether this location may decide the category of `entry`.
    ///
    /// A location always decides its own root. It decides its contents only
    /// when [`LocationClass::inherits_to_contents`] holds — a pure container
    /// such as `UserHome` says *whose* tree we are in, never *what* an entry
    /// is, so it must not reclassify unremarkable contents into `UserData`
    /// (which would make `Other` unreachable across most of a user's tree).
    pub const fn decides(self) -> bool {
        self.is_root || self.class.inherits_to_contents()
    }
}

/// Result of analysing an entry's path. Pure data; nothing is retained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathContext {
    /// The most specific authoritative location the entry sits in — including
    /// the case where the entry *is* the location (e.g. `C:/Windows`).
    pub location: Option<LocationMatch>,
}

impl PathContext {
    /// Convenience: the matched location class, if any.
    pub const fn class(self) -> Option<LocationClass> {
        match self.location {
            Some(l) => Some(l.class),
            None => None,
        }
    }
}

/// Analyse a path in a host-independent way. Never touches the filesystem.
pub fn analyze(path: &Path, platform: Platform) -> PathContext {
    let Some(raw) = path.to_str() else {
        return PathContext { location: None };
    };

    // Fixed-size buffer: no allocation, no dependence on path depth.
    let mut parts: [&str; MAX_LOCATION_DEPTH] = [""; MAX_LOCATION_DEPTH];
    let mut len = 0usize;
    for part in raw.split(['/', '\\']) {
        if part.is_empty() || is_drive_token(part) {
            continue;
        }
        if len == MAX_LOCATION_DEPTH {
            break;
        }
        parts[len] = part;
        len += 1;
    }
    let parts = &parts[..len];

    let case_insensitive = platform.case_insensitive_names();
    let mut best: Option<LocationMatch> = None;
    for rule in LOCATION_RULES {
        if !rule.applies_to(platform) {
            continue;
        }
        let mut depth = 0usize;
        for pattern in rule.patterns {
            if let Some(n) = pattern_prefix_len(parts, pattern, case_insensitive) {
                if n > depth {
                    depth = n;
                }
            }
        }
        if depth == 0 {
            continue;
        }
        let candidate = LocationMatch {
            rule: rule.id,
            class: rule.class,
            evidence: rule.evidence,
            depth,
            // `len` is the component count of the whole path (capped at
            // MAX_LOCATION_DEPTH). Equal to `depth` ⇒ the path ends exactly at
            // the location, i.e. the entry is the location itself.
            is_root: depth == len,
        };
        // Strictly greater depth wins: equal depth keeps the earlier table
        // entry, so the outcome is deterministic.
        if best.is_none_or(|b| candidate.depth > b.depth) {
            best = Some(candidate);
        }
    }

    PathContext { location: best }
}

/// Windows drive prefix (`C:`). Meaningless as a semantic path component, and
/// present only on synthetic Windows paths analysed on any host.
fn is_drive_token(part: &str) -> bool {
    let b = part.as_bytes();
    b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic()
}

/// Number of leading components `pattern` covers, or `None` if it is not a
/// prefix of `parts`. `*` matches exactly one component.
fn pattern_prefix_len(parts: &[&str], pattern: &str, case_insensitive: bool) -> Option<usize> {
    let mut depth = 0usize;
    for want in pattern.split('/').filter(|s| !s.is_empty()) {
        let got = parts.get(depth)?;
        if want != "*" {
            let eq = if case_insensitive {
                got.eq_ignore_ascii_case(want)
            } else {
                *got == want
            };
            if !eq {
                return None;
            }
        }
        depth += 1;
    }
    if depth == 0 {
        None
    } else {
        Some(depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_of(path: &str, platform: Platform) -> Option<LocationClass> {
        analyze(Path::new(path), platform).class()
    }

    #[test]
    fn windows_locations_are_host_independent() {
        // Synthetic Windows paths exercise Windows semantics on ANY host:
        // separators are handled by string splitting, never by PathBuf.
        assert_eq!(
            class_of("C:/Windows", Platform::Windows),
            Some(LocationClass::System)
        );
        assert_eq!(
            class_of("C:/Program Files/App", Platform::Windows),
            Some(LocationClass::ApplicationInstall)
        );
        assert_eq!(
            class_of("C:/Program Files (x86)/App", Platform::Windows),
            Some(LocationClass::ApplicationInstall)
        );
        assert_eq!(
            class_of("C:/ProgramData/App", Platform::Windows),
            Some(LocationClass::ApplicationData)
        );
        assert_eq!(
            class_of("C:/Users/user/AppData/Local/App", Platform::Windows),
            Some(LocationClass::ApplicationData)
        );
        assert_eq!(
            class_of("C:/Users/user/AppData/Roaming/App", Platform::Windows),
            Some(LocationClass::ApplicationData)
        );
        assert_eq!(
            class_of("C:/Users/user/Downloads", Platform::Windows),
            Some(LocationClass::Downloads)
        );
        // Backslashes behave identically on every host.
        assert_eq!(
            class_of(r"C:\Users\user\AppData\Local\App", Platform::Windows),
            Some(LocationClass::ApplicationData)
        );
    }

    #[test]
    fn mac_locations() {
        assert_eq!(
            class_of("/Applications/App.app", Platform::Mac),
            Some(LocationClass::ApplicationInstall)
        );
        assert_eq!(
            class_of("/Users/user/Library/Application Support/App", Platform::Mac),
            Some(LocationClass::ApplicationData)
        );
        assert_eq!(
            class_of("/Users/user/Library/Caches/App", Platform::Mac),
            Some(LocationClass::Cache)
        );
        assert_eq!(
            class_of("/Users/user/Library/Logs/App", Platform::Mac),
            Some(LocationClass::Logs)
        );
        assert_eq!(
            class_of("/Users/user/Library", Platform::Mac),
            Some(LocationClass::ApplicationData)
        );
    }

    #[test]
    fn linux_locations_are_not_collapsed() {
        assert_eq!(
            class_of("/usr", Platform::Linux),
            Some(LocationClass::System)
        );
        assert_eq!(
            class_of("/etc", Platform::Linux),
            Some(LocationClass::System)
        );
        assert_eq!(
            class_of("/var/log", Platform::Linux),
            Some(LocationClass::Logs)
        );
        assert_eq!(
            class_of("/var/cache", Platform::Linux),
            Some(LocationClass::Cache)
        );
        assert_eq!(
            class_of("/opt", Platform::Linux),
            Some(LocationClass::ApplicationInstall)
        );
        assert_eq!(
            class_of("/tmp", Platform::Linux),
            Some(LocationClass::Temporary)
        );
        assert_eq!(
            class_of("/home/user/.cache", Platform::Linux),
            Some(LocationClass::Cache)
        );
        assert_eq!(
            class_of("/home/user/.config", Platform::Linux),
            Some(LocationClass::ApplicationData)
        );
        assert_eq!(
            class_of("/home/user/Downloads", Platform::Linux),
            Some(LocationClass::Downloads)
        );
        assert_eq!(
            class_of("/home/user/Downloads", Platform::Linux),
            class_of("/home/user/downloads", Platform::Linux),
            "both XDG spellings must resolve"
        );
    }

    #[test]
    fn most_specific_pattern_wins() {
        let m = analyze(Path::new("/var/log/app/x.log"), Platform::Linux)
            .location
            .unwrap();
        assert_eq!(m.class, LocationClass::Logs);
        assert_eq!(m.depth, 2, "/var/log (2) beats /var-scoped rules");

        let m = analyze(Path::new("/Users/u/Library/Caches/App"), Platform::Mac)
            .location
            .unwrap();
        assert_eq!(m.class, LocationClass::Cache);
        assert_eq!(m.depth, 4);
    }

    #[test]
    fn project_directories_are_not_authoritative_locations() {
        // Finding 6: ordinary project directories must not be mistaken for
        // system/cache/download locations.
        for p in [
            "/home/user/project/cache",
            "/home/user/project/build",
            "/home/user/project/tmp",
            "/home/user/project/backup",
            "/home/user/project/logs",
            "/home/user/windows",
        ] {
            let c = class_of(p, Platform::Linux);
            assert!(
                !matches!(
                    c,
                    Some(
                        LocationClass::System
                            | LocationClass::Cache
                            | LocationClass::Logs
                            | LocationClass::Temporary
                            | LocationClass::Downloads
                    )
                ),
                "{p} must not look like a system location, got {c:?}"
            );
        }
        // `/home/user/...` is only ever the user-home *container*.
        assert_eq!(
            class_of("/home/user/project/cache", Platform::Linux),
            Some(LocationClass::UserHome)
        );
    }

    #[test]
    fn platform_locations_do_not_leak() {
        assert_eq!(class_of("C:/Windows", Platform::Linux), None);
        assert_eq!(class_of("/usr", Platform::Windows), None);
        assert_eq!(class_of("/home/u/.config", Platform::Mac), None);
        // `~/Library` is only Application Support data on macOS. On Windows the
        // same path is recognised by Windows' own `/Users/<u>` profile
        // convention and as nothing more specific — the macOS-specific
        // ApplicationData knowledge must not leak.
        assert_ne!(
            class_of("/Users/u/Library", Platform::Windows),
            Some(LocationClass::ApplicationData)
        );
        assert_eq!(
            class_of("/Users/u/Library/Application Support", Platform::Mac),
            Some(LocationClass::ApplicationData)
        );
    }

    #[test]
    fn user_home_does_not_corroborate() {
        assert!(!LocationClass::UserHome.corroborates());
        assert!(LocationClass::Cache.corroborates());
        assert!(LocationClass::ApplicationData.corroborates());
        assert!(LocationClass::Downloads.corroborates());
    }

    #[test]
    fn degenerate_paths_are_safe() {
        for p in [
            "",
            "/",
            "//",
            "C:\\",
            ".",
            "..",
            "\\\\?\\C:\\huge",
            "\u{0}\u{1}",
        ] {
            let ctx = analyze(Path::new(p), Platform::Windows);
            let _ = ctx;
        }
        assert_eq!(analyze(Path::new(""), Platform::Linux).location, None);
    }

    #[test]
    fn location_category_matches_rule_category() {
        // Invariant: a location rule's id must resolve to the same category as
        // its class, otherwise evidence and category would disagree.
        for rule in LOCATION_RULES {
            assert_eq!(
                crate::rules::rule_category(rule.id),
                location_category(rule.class),
                "location rule {} disagrees with its class",
                rule.id.code()
            );
        }
    }

    #[test]
    fn location_codes_unique() {
        let all = [
            LocationClass::System,
            LocationClass::ApplicationInstall,
            LocationClass::ApplicationData,
            LocationClass::Cache,
            LocationClass::Logs,
            LocationClass::Temporary,
            LocationClass::Downloads,
            LocationClass::UserDocuments,
            LocationClass::UserDesktop,
            LocationClass::UserHome,
        ];
        let mut seen = std::collections::HashSet::new();
        for c in all {
            assert!(seen.insert(c.code()));
        }
    }
}
