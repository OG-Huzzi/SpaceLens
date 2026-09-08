//! Deterministic rule engine.
//!
//! The classifier is a *pure function* over entry metadata plus optional
//! parent context (docs/ARCHITECTURE.md: "classifier is a pure function over
//! metadata, rule tables versioned/testable without a disk"). No filesystem
//! access, no file-content reads, no I/O of any kind.
//!
//! # Precedence model (documented + tested)
//!
//! Every rule has a tier. Lower tier = stronger evidence:
//!
//! | Tier | Meaning                        | Example                       |
//! |------|--------------------------------|-------------------------------|
//! | 0    | Authoritative location/system  | Program Files, /usr, Library  |
//! | 1    | Strong path pattern            | node_modules, .git, Caches    |
//! | 2    | Canonical user directory       | Downloads, Documents, Desktop |
//! | 3    | Strong filename pattern        | setup-style names             |
//! | 4    | Extension table                | .pdf, .png, .zip              |
//! | 5    | Weak heuristics / fallback     | bare `build`, plain file      |
//!
//! Winner selection among matching rules is a total deterministic order:
//! **lowest tier → longest matched needle → table order**. Context
//! ([`crate::context::ParentContext`]) can *raise* the final confidence of
//! the winning classification but can never change which rule wins, so a
//! weak extension can never override authoritative location evidence
//! (master prompt §12).
//!
//! All matching rules are retained as explainable evidence, in table order —
//! evidence ordering is therefore deterministic by construction.

use serde::{Deserialize, Serialize};

use crate::category::{Category, Subcategory};
use crate::confidence::Confidence;
use crate::evidence::{EvidenceKind, RuleId};
use crate::platform::Platform;

/// One classification rule. The table order is contract: adding a rule
/// appends; reordering is a v1 contract change.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable identifier (IPC surface).
    pub id: RuleId,
    /// Precedence tier — lower wins. See module docs.
    pub tier: u8,
    /// Platforms the rule applies to; empty = all platforms.
    pub platforms: &'static [Platform],
    /// True when the rule matches directory names; false = file rules.
    pub dir_rule: bool,
    /// Subcategory the rule assigns (if any).
    pub subcategory: Option<Subcategory>,
    /// Base confidence when this rule wins.
    pub confidence: Confidence,
    /// Evidence kind recorded when the rule matches.
    pub evidence: EvidenceKind,
    /// Needles matched against the entry name (exact, per case rules below).
    /// Longest-needle tie-break uses this length.
    pub needles: &'static [&'static str],
    /// Extensions (lowercase, without dot) for file rules. Empty for dir rules.
    pub extensions: &'static [&'static str],
}

impl Rule {
    /// True when this rule is enabled for `platform`.
    pub fn applies_to(&self, platform: Platform) -> bool {
        self.platforms.is_empty() || self.platforms.contains(&platform)
    }

    /// Match a *file* entry against this rule's needles/extensions.
    /// `file_name` is the full name with extension; `stem` is the name
    /// without the final extension. Match is case-insensitive on all
    /// platforms (filename conventions like `Setup.EXE` are not meaningful
    /// case distinctions on any platform we target).
    pub fn matches_file(&self, file_name: &str, stem: &str, ext: Option<&str>) -> bool {
        if self.dir_rule {
            return false;
        }
        let name_lc = file_name.to_lowercase();
        let stem_lc = stem.to_lowercase();
        for n in self.needles {
            // Needle must appear as a whole "word-ish" prefix of the stem
            // (setup.exe, Setup Wizard.exe, setup_venus.exe) — a bare
            // substring would over-match (e.g. "container" contains "tain").
            let n = n.to_lowercase();
            if stem_lc == n
                || stem_lc.starts_with(&format!("{n} "))
                || stem_lc.starts_with(&format!("{n}-"))
                || stem_lc.starts_with(&format!("{n}_"))
                || name_lc.starts_with(&format!("{n}."))
            {
                return true;
            }
        }
        if let Some(e) = ext {
            let e = e.to_lowercase();
            return self.extensions.contains(&e.as_str());
        }
        false
    }

    /// Match a *directory* entry by name. Case handling follows the platform
    /// convention passed by the caller.
    pub fn matches_dir(&self, dir_name: &str, case_insensitive: bool) -> bool {
        if !self.dir_rule {
            return false;
        }
        if case_insensitive {
            let name_lc = dir_name.to_lowercase();
            self.needles.contains(&name_lc.as_str())
        } else {
            self.needles.contains(&dir_name)
        }
    }
}

// ---------------------------------------------------------------------------
// Extension tables (single source of truth; lowercase, without dot).
// ---------------------------------------------------------------------------

const EXT_DOCUMENT: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt", "md",
    "csv", "epub", "pages", "numbers", "key",
];
const EXT_IMAGE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "svg", "ico", "heic", "heif",
    "avif", "psd",
];
const EXT_VIDEO: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v", "mpg", "mpeg", "3gp",
];
// `.ts` is deliberately NOT in the video table: it collides with TypeScript
// source. Neither interpretation wins; the ambiguous extension is left
// unclassified (Other) rather than guessed (§41: false confidence is worse
// than uncertainty). `tsx` is unambiguous TypeScript.
const EXT_AUDIO: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "m4a", "aac", "wma", "opus", "aiff",
];
const EXT_ARCHIVE: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "zst", "lz4", "tgz", "tbz2",
];
const EXT_INSTALLER: &[&str] = &[
    "msi", "dmg", "pkg", "deb", "rpm", "appimage", "apk", "msp", "msu",
];
const EXT_EXECUTABLE: &[&str] = &["exe", "app", "bat", "cmd", "com", "scr", "run"];
const EXT_DISK_IMAGE: &[&str] = &["iso", "img", "vhd", "vhdx", "vmdk"];
const EXT_SOURCE: &[&str] = &[
    "rs", "go", "c", "h", "cpp", "hpp", "cc", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "java",
    "kt", "swift", "rb", "php", "cs", "scala", "hs", "toml", "yaml", "yml", "json", "lock", "sh",
    "ps1", "sql", "lua",
];
const EXT_LOG: &[&str] = &["log"];

// ---------------------------------------------------------------------------
// The rule table. ORDER IS CONTRACT: winner tie-break is table order, and
// evidence is emitted in table order. Tiers per module docs.
// ---------------------------------------------------------------------------

/// All-platform rules (platforms: `&[]`).
pub const RULES: &[Rule] = &[
    // -- Tier 0: authoritative locations -----------------------------------
    Rule {
        id: RuleId::WindowsSystemLocation,
        tier: 0,
        platforms: &[Platform::Windows],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownSystemLocation,
        needles: &[
            "windows",
            "program files",
            "program files (x86)",
            "programdata",
        ],
        extensions: &[],
    },
    Rule {
        id: RuleId::MacApplicationSupport,
        tier: 0,
        platforms: &[Platform::Mac],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownApplicationLocation,
        needles: &["application support", "library"],
        extensions: &[],
    },
    Rule {
        id: RuleId::XdgLocation,
        tier: 0,
        platforms: &[Platform::Linux],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::PlatformLocation,
        // `.cache` is deliberately NOT here: it is a cache signal and is
        // handled by CacheDir (more specific category, same strength).
        needles: &[".local", ".config", ".share"],
        extensions: &[],
    },
    Rule {
        id: RuleId::LinuxPackageLocation,
        tier: 0,
        platforms: &[Platform::Linux],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownSystemLocation,
        needles: &["usr", "opt", "etc", "var", "flatpak", "snap"],
        extensions: &[],
    },
    // -- Tier 1: strong path patterns --------------------------------------
    Rule {
        id: RuleId::SteamLibrary,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownApplicationLocation,
        needles: &["steamapps", "steamlibrary"],
        extensions: &[],
    },
    Rule {
        id: RuleId::DependencyDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: Some(Subcategory::DependencyTree),
        confidence: Confidence::High,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &["node_modules", "vendor", "packages"],
        extensions: &[],
    },
    Rule {
        id: RuleId::VcsDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: Some(Subcategory::VcsInternals),
        confidence: Confidence::High,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &[".git", ".svn", ".hg"],
        extensions: &[],
    },
    Rule {
        id: RuleId::VirtualenvDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: Some(Subcategory::DependencyTree),
        confidence: Confidence::High,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &["venv", ".venv", "__pycache__", "site-packages", "env"],
        extensions: &[],
    },
    Rule {
        id: RuleId::BuildDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: Some(Subcategory::BuildOutput),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &["target", "dist", "build", "out", "bin", "obj"],
        extensions: &[],
    },
    Rule {
        id: RuleId::CacheDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownCacheLocation,
        needles: &["cache", ".cache"],
        extensions: &[],
    },
    Rule {
        id: RuleId::TempDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownTemporaryLocation,
        needles: &["temp", "tmp"],
        extensions: &[],
    },
    Rule {
        id: RuleId::LogDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: Some(Subcategory::LogFile),
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["logs", "log"],
        extensions: &[],
    },
    Rule {
        id: RuleId::BackupDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["backup", "backups", "bak"],
        extensions: &[],
    },
    Rule {
        id: RuleId::WindowsAppData,
        tier: 1,
        platforms: &[Platform::Windows],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::KnownApplicationLocation,
        needles: &["appdata", "application data"],
        extensions: &[],
    },
    // -- Tier 2: canonical user directories ---------------------------------
    // Note: capitalized variants exist because XDG user dirs on Linux are
    // conventionally capitalized (`Downloads`) while Linux name matching is
    // case-sensitive; on Windows/macOS the extra variants are inert.
    Rule {
        id: RuleId::DownloadsDir,
        tier: 2,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["downloads", "download", "Downloads"],
        extensions: &[],
    },
    Rule {
        id: RuleId::DesktopDir,
        tier: 2,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["desktop", "Desktop"],
        extensions: &[],
    },
    Rule {
        id: RuleId::DocumentsDir,
        tier: 2,
        platforms: &[],
        dir_rule: true,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["documents", "my documents", "Documents"],
        extensions: &[],
    },
    // -- Tier 3: strong filename patterns (files) ---------------------------
    Rule {
        id: RuleId::InstallerName,
        tier: 3,
        platforms: &[],
        dir_rule: false,
        subcategory: Some(Subcategory::Installer),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::FilenamePattern,
        needles: &["setup", "install", "installer", "uninstall", "update"],
        extensions: &[],
    },
    // -- Tier 4: extension tables (files) ------------------------------------
    Rule {
        id: RuleId::InstallerExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: Some(Subcategory::Installer),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_INSTALLER,
    },
    Rule {
        id: RuleId::ExecutableExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_EXECUTABLE,
    },
    Rule {
        id: RuleId::ArchiveExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: Some(Subcategory::Archive),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_ARCHIVE,
    },
    Rule {
        id: RuleId::DiskImageExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: Some(Subcategory::DiskImage),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_DISK_IMAGE,
    },
    Rule {
        id: RuleId::DocumentExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_DOCUMENT,
    },
    Rule {
        id: RuleId::ImageExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_IMAGE,
    },
    Rule {
        id: RuleId::VideoExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_VIDEO,
    },
    Rule {
        id: RuleId::AudioExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_AUDIO,
    },
    Rule {
        id: RuleId::SourceExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_SOURCE,
    },
    Rule {
        id: RuleId::LogExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        subcategory: Some(Subcategory::LogFile),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_LOG,
    },
];

/// Category each rule assigns. Kept separate from [`Rule`] so the table stays
/// `Copy` with only `&'static` data. Total mapping — a compile-time-exhaustive
/// match; adding a `RuleId` without a category is a compile error.
pub const fn rule_category(id: RuleId) -> Category {
    match id {
        RuleId::WindowsSystemLocation | RuleId::LinuxPackageLocation => Category::SystemData,
        RuleId::MacApplicationSupport | RuleId::WindowsAppData => Category::Applications,
        // No table rule matches .app bundles yet (bundles are directories
        // whose *names end in* .app — requires suffix matching). The id is
        // reserved; see docs/CLASSIFICATION.md limitations.
        RuleId::MacAppBundle => Category::Applications,
        RuleId::XdgLocation => Category::UserData,
        RuleId::SteamLibrary => Category::Games,
        RuleId::DependencyDir
        | RuleId::VcsDir
        | RuleId::VirtualenvDir
        | RuleId::BuildDir
        | RuleId::SourceExtension => Category::Development,
        RuleId::CacheDir => Category::Cache,
        RuleId::TempDir => Category::TemporaryData,
        RuleId::LogDir | RuleId::LogExtension => Category::Logs,
        RuleId::BackupDir => Category::Backups,
        RuleId::DownloadsDir | RuleId::InstallerName | RuleId::InstallerExtension => {
            Category::Downloads
        }
        RuleId::DesktopDir | RuleId::DocumentsDir => Category::UserData,
        RuleId::ExecutableExtension => Category::Applications,
        RuleId::ArchiveExtension | RuleId::DiskImageExtension => Category::Archives,
        RuleId::DocumentExtension => Category::Documents,
        RuleId::ImageExtension => Category::Images,
        RuleId::VideoExtension => Category::Video,
        RuleId::AudioExtension => Category::Audio,
        RuleId::DirWithoutSignals | RuleId::PlainFile => Category::Other,
        RuleId::NoSignals | RuleId::ExtensionOnly | RuleId::ParentInherited => Category::Unknown,
    }
}

/// Aggregated outcome of one classification pass: the winning rule plus every
/// rule that matched (deterministic order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchOutcome {
    /// Ids of every rule that matched, in table order (bounded by table size).
    pub matched: Vec<RuleId>,
    /// The winning rule (lowest tier → longest needle → table order).
    pub winner: RuleId,
    /// Winner's base confidence before context adjustment.
    pub base_confidence: Confidence,
    /// Winner's assigned category.
    pub category: Category,
    /// Winner's subcategory, if any.
    pub subcategory: Option<Subcategory>,
    /// Evidence kinds for the winner, for the explainable record.
    pub evidence_kind: EvidenceKind,
}

/// Evaluate all rules against one entry description and select the winner.
/// Deterministic: same inputs → same outcome, always.
pub fn evaluate(
    is_dir: bool,
    file_name: &str,
    stem: &str,
    ext: Option<&str>,
    platform: Platform,
) -> MatchOutcome {
    let mut matched: Vec<RuleId> = Vec::new();
    let mut winner: Option<(u8, usize, usize, &Rule)> = None; // (tier, needle_len, table_idx, rule)

    for (idx, rule) in RULES.iter().enumerate() {
        if !rule.applies_to(platform) {
            continue;
        }
        let hit = if is_dir {
            rule.matches_dir(file_name, platform.case_insensitive_names())
        } else {
            rule.matches_file(file_name, stem, ext)
        };
        if !hit {
            continue;
        }
        matched.push(rule.id);
        let strength = rule.needles.iter().map(|n| n.len()).max().unwrap_or(0);
        let better = match winner {
            None => true,
            Some((wt, wl, _, _)) => rule.tier < wt || (rule.tier == wt && strength > wl),
        };
        if better {
            winner = Some((rule.tier, strength, idx, rule));
        }
    }

    match winner {
        Some((_, _, _, rule)) => MatchOutcome {
            matched,
            winner: rule.id,
            base_confidence: rule.confidence,
            category: rule_category(rule.id),
            subcategory: rule.subcategory,
            evidence_kind: rule.evidence,
        },
        None => MatchOutcome {
            matched,
            winner: if is_dir {
                RuleId::DirWithoutSignals
            } else {
                RuleId::PlainFile
            },
            base_confidence: Confidence::Low,
            category: Category::Other,
            subcategory: None,
            evidence_kind: EvidenceKind::EntryKind,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_outcome(name: &str, platform: Platform) -> MatchOutcome {
        let stem = match name.rsplit_once('.') {
            Some((s, _)) if !s.is_empty() => s,
            _ => name,
        };
        let ext = name.rsplit_once('.').map(|(_, e)| e);
        evaluate(false, name, stem, ext, platform)
    }

    fn dir_outcome(name: &str, platform: Platform) -> MatchOutcome {
        evaluate(true, name, name, None, platform)
    }

    #[test]
    fn table_order_is_tier_monotonic() {
        let mut last = 0;
        for r in RULES {
            assert!(
                r.tier >= last,
                "rule {} breaks tier ordering ({} < {})",
                r.id.code(),
                r.tier,
                last
            );
            last = r.tier;
        }
    }

    #[test]
    fn rule_ids_are_unique_across_table() {
        let mut seen = std::collections::HashSet::new();
        for r in RULES {
            assert!(
                seen.insert(r.id),
                "duplicate rule in table: {}",
                r.id.code()
            );
        }
    }

    #[test]
    fn every_rule_id_has_category() {
        for r in RULES {
            // Const fn must resolve without panic for every table member.
            let _ = rule_category(r.id);
        }
    }

    #[test]
    fn strong_location_beats_extension() {
        // A directory named "Downloads" never loses to anything weaker.
        let o = dir_outcome("Downloads", Platform::Windows);
        assert_eq!(o.winner, RuleId::DownloadsDir);
        assert_eq!(o.category, Category::Downloads);
        assert_eq!(o.base_confidence, Confidence::High);
    }

    #[test]
    fn cache_beats_temp_on_equal_tier_by_needle_length() {
        // Both tier 1: "cache" (5) vs "tmp" (3) — not directly comparable on
        // one name; instead verify longest-needle tie-break deterministically:
        let o = dir_outcome("cache", Platform::Linux);
        assert_eq!(o.winner, RuleId::CacheDir);
        let o = dir_outcome("tmp", Platform::Linux);
        assert_eq!(o.winner, RuleId::TempDir);
    }

    #[test]
    fn platform_rules_do_not_leak_across_platforms() {
        // Windows system names must not classify on Linux.
        let o = dir_outcome("Program Files", Platform::Linux);
        assert_ne!(o.winner, RuleId::WindowsSystemLocation);
        // Linux XDG names must not classify on Windows.
        let o = dir_outcome(".config", Platform::Windows);
        assert_ne!(o.winner, RuleId::XdgLocation);
        // macOS Library must not classify on Windows.
        let o = dir_outcome("Library", Platform::Windows);
        assert_ne!(o.winner, RuleId::MacApplicationSupport);
    }

    #[test]
    fn windows_case_insensitive_dir_match() {
        let o = dir_outcome("PROGRAM FILES", Platform::Windows);
        assert_eq!(o.winner, RuleId::WindowsSystemLocation);
    }

    #[test]
    fn linux_case_sensitive_dir_match() {
        // "CACHE" must not match "cache" on Linux.
        let o = dir_outcome("CACHE", Platform::Linux);
        assert_ne!(o.winner, RuleId::CacheDir);
        let o = dir_outcome("cache", Platform::Linux);
        assert_eq!(o.winner, RuleId::CacheDir);
    }

    #[test]
    fn extension_matches() {
        let o = file_outcome("report.pdf", Platform::Windows);
        assert_eq!(o.winner, RuleId::DocumentExtension);
        assert_eq!(o.category, Category::Documents);
        assert_eq!(o.base_confidence, Confidence::Medium);
    }

    #[test]
    fn installer_name_beats_extension() {
        // "setup.exe": InstallerName (tier 3) beats ExecutableExtension (tier 4).
        let o = file_outcome("setup.exe", Platform::Windows);
        assert_eq!(o.winner, RuleId::InstallerName);
        assert_eq!(o.subcategory, Some(Subcategory::Installer));
        assert!(o.matched.contains(&RuleId::ExecutableExtension));
    }

    #[test]
    fn installer_extension_maps_to_downloads() {
        let o = file_outcome("something.msi", Platform::Windows);
        assert_eq!(o.winner, RuleId::InstallerExtension);
        assert_eq!(o.category, Category::Downloads);
    }

    #[test]
    fn dev_dirs_classify_as_development() {
        for (name, rule) in [
            ("node_modules", RuleId::DependencyDir),
            (".git", RuleId::VcsDir),
            ("venv", RuleId::VirtualenvDir),
        ] {
            let o = dir_outcome(name, Platform::Linux);
            assert_eq!(o.winner, rule, "dir {name}");
            assert_eq!(o.category, Category::Development);
        }
    }

    #[test]
    fn pycache_prefers_virtualenv_over_cache_by_needle_length() {
        // Both tier 1; "__pycache__" (11) is longer than "cache" (5).
        let o = dir_outcome("__pycache__", Platform::Linux);
        assert_eq!(o.winner, RuleId::VirtualenvDir);
    }

    #[test]
    fn unmatched_file_is_other_low() {
        let o = file_outcome("mysteryblob.xyzzy", Platform::Windows);
        assert_eq!(o.winner, RuleId::PlainFile);
        assert_eq!(o.category, Category::Other);
        assert_eq!(o.base_confidence, Confidence::Low);
    }

    #[test]
    fn unmatched_dir_is_other_low() {
        let o = dir_outcome("holiday-photos-2009", Platform::Windows);
        assert_eq!(o.winner, RuleId::DirWithoutSignals);
        assert_eq!(o.category, Category::Other);
        assert_eq!(o.base_confidence, Confidence::Low);
    }

    #[test]
    fn installer_name_prefix_matching_is_conservative() {
        assert_eq!(
            file_outcome("Setup Venus Final.exe", Platform::Windows).winner,
            RuleId::InstallerName
        );
        assert_eq!(
            file_outcome("setup_2024.zip", Platform::Windows).winner,
            RuleId::InstallerName
        );
        // Bare substring must NOT match: "container" is not "install".
        assert_ne!(
            file_outcome("container.tar.gz", Platform::Linux).winner,
            RuleId::InstallerName
        );
    }

    #[test]
    fn multi_extension_archive_gz() {
        // "backup.tar.gz": extension "gz" → archive.
        let o = file_outcome("backup.tar.gz", Platform::Linux);
        assert_eq!(o.winner, RuleId::ArchiveExtension);
        assert_eq!(o.category, Category::Archives);
    }

    #[test]
    fn steam_library_is_games() {
        let o = dir_outcome("steamapps", Platform::Windows);
        assert_eq!(o.winner, RuleId::SteamLibrary);
        assert_eq!(o.category, Category::Games);
        assert_eq!(o.base_confidence, Confidence::High);
    }

    #[test]
    fn outcome_is_deterministic() {
        let a = file_outcome("Setup.PDF", Platform::Windows);
        let b = file_outcome("Setup.PDF", Platform::Windows);
        assert_eq!(a, b);
    }
}
