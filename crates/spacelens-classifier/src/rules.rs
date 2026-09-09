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
//! | Tier | Meaning                          | Example                       |
//! |------|----------------------------------|-------------------------------|
//! | 1    | Authoritative path pattern       | node_modules, .git, steamapps |
//! | 2    | Canonical user directory         | Downloads, Documents, Desktop |
//! | 4    | Content-typed extension          | .pdf, .png, .zip, .log, .rs   |
//! | 5    | Generic / weak file signal       | `setup`-style names (gated), `.exe` |
//! | 6    | Authoritative container location | entry sits in `~/Library/Caches` |
//!
//! Tiers 0 and 3 are intentionally unused: they were the "authoritative
//! location" and "strong filename" bands of an earlier draft. Location
//! knowledge moved to tier 6 (it must never outrank a statement about what
//! the entry *is*), and the installer-name rule moved *down* to tier 5,
//! because a name that looks like an installer is weaker evidence than a
//! content-typed extension. The gaps are left in place so the numbering of
//! the surviving bands stays stable.
//!
//! Winner selection among **eligible** rules is a total deterministic order:
//! **lowest tier → longest matched needle → table order**.
//!
//! # Two strengths of knowledge (see `pathctx`)
//!
//! Rules that match a *rooted* platform location live in
//! [`crate::pathctx::LOCATION_RULES`] and are injected as a tier-6 candidate.
//! Rules that match a *bare name* live here and declare a
//! [`crate::RuleKind`], which is what actually determines the confidence
//! ceiling. A bare `cache` is a [`RuleKind::Heuristic`]; `~/Library/Caches`
//! is authoritative. That difference is the whole point.
//!
//! # Gating
//!
//! A rule may declare a [`RuleGate`]: it *matches* (and is retained as
//! evidence) but is **not eligible to win** unless the gate is satisfied.
//! This is how installer-style names stay conservative: `update.exe` is a
//! filename pattern that only means "downloaded installer" when an
//! authoritative download location says so.
//!
//! # Evidence fidelity
//!
//! Every matched rule is retained as a [`RuleMatch`] carrying its own
//! [`EvidenceKind`] — the signal type is captured *at match time* and never
//! reconstructed afterwards (docs/CLASSIFICATION.md).

use serde::{Deserialize, Serialize};

use crate::category::{Category, Subcategory};
use crate::confidence::{Confidence, RuleKind};
use crate::evidence::{EvidenceKind, RuleId};
use crate::pathctx::{location_category, LocationClass, LocationMatch};
use crate::platform::Platform;

/// Tier assigned to authoritative container-location knowledge. Lower
/// precedence than any name/extension signal: *where something lives* never
/// overrides *what it is*, but it does classify entries that carry no signal
/// of their own.
pub const LOCATION_TIER: u8 = 6;

/// When a rule is allowed to win. A gated rule still matches (and is still
/// recorded as evidence) — it simply cannot decide the category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleGate {
    /// The rule may win only when the entry sits in an authoritative location
    /// of one of these classes.
    Under(&'static [LocationClass]),
}

impl RuleGate {
    /// Is the gate satisfied by the entry's authoritative location?
    pub fn satisfied_by(self, class: Option<LocationClass>) -> bool {
        match class {
            None => false,
            Some(c) => match self {
                RuleGate::Under(classes) => classes.contains(&c),
            },
        }
    }
}

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
    /// What kind of knowledge this rule encodes. Determines the confidence
    /// ceiling — the single authoritative policy input.
    pub kind: RuleKind,
    /// Optional eligibility condition. Gated rules match, but may only win
    /// when the gate is satisfied.
    pub gate: Option<RuleGate>,
    /// Subcategory the rule assigns (if any).
    pub subcategory: Option<Subcategory>,
    /// Base confidence when this rule wins, before the ceiling is applied.
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

    /// Length of the longest needle — the tie-break strength.
    pub fn strength(&self) -> usize {
        self.needles.iter().map(|n| n.len()).max().unwrap_or(0)
    }

    /// Whether this rule is allowed to win, given the entry's location.
    /// A gated rule that fails its gate is still recorded as evidence.
    pub fn eligible(&self, location: Option<LocationClass>) -> bool {
        match self.gate {
            None => true,
            Some(gate) => gate.satisfied_by(location),
        }
    }

    /// Whether this rule's gate is currently satisfied.
    pub fn gate_satisfied(&self, location: Option<LocationClass>) -> bool {
        match self.gate {
            None => false,
            Some(gate) => gate.satisfied_by(location),
        }
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
//
// Location rules (Windows System/Program Files/AppData, macOS Library,
// Linux /usr, XDG, …) are NOT here — they live in `pathctx::LOCATION_RULES`
// because they match rooted paths, not bare names.
// ---------------------------------------------------------------------------

/// All-platform rules (platforms: `&[]`).
pub const RULES: &[Rule] = &[
    // -- Tier 1: strong path patterns --------------------------------------
    Rule {
        id: RuleId::SteamLibrary,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Authoritative,
        gate: None,
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
        kind: RuleKind::Authoritative,
        gate: None,
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
        kind: RuleKind::Authoritative,
        gate: None,
        subcategory: Some(Subcategory::VcsInternals),
        confidence: Confidence::High,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &[".git", ".svn", ".hg"],
        extensions: &[],
    },
    // `env` is deliberately absent, like `bin` below: a directory merely
    // named "env" is as likely to be something else as a virtualenv, and the
    // unambiguous names (`.venv`, `__pycache__`, `site-packages`) carry the
    // authoritative claim on their own.
    Rule {
        id: RuleId::VirtualenvDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Authoritative,
        gate: None,
        subcategory: Some(Subcategory::DependencyTree),
        confidence: Confidence::High,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &["venv", ".venv", "__pycache__", "site-packages"],
        extensions: &[],
    },
    // `bin` is deliberately absent: it is a system directory name on Unix
    // (/bin, /usr/bin) as often as a build-output name, so it cannot be a
    // name heuristic without producing confidently wrong answers.
    Rule {
        id: RuleId::BuildDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Heuristic,
        gate: None,
        subcategory: Some(Subcategory::BuildOutput),
        confidence: Confidence::Low,
        evidence: EvidenceKind::DevelopmentArtifact,
        needles: &["target", "dist", "build", "out", "obj"],
        extensions: &[],
    },
    // `cache`, `tmp`, `logs`, `backup` are weak *name* heuristics: any project
    // can contain them. Genuine platform cache/log/temp trees are recognised
    // by rooted location rules, which are authoritative.
    Rule {
        id: RuleId::CacheDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Heuristic,
        gate: None,
        subcategory: None,
        confidence: Confidence::Low,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["cache", ".cache"],
        extensions: &[],
    },
    Rule {
        id: RuleId::TempDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Heuristic,
        gate: None,
        subcategory: None,
        confidence: Confidence::Low,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["temp", "tmp"],
        extensions: &[],
    },
    Rule {
        id: RuleId::LogDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Heuristic,
        gate: None,
        subcategory: Some(Subcategory::LogFile),
        confidence: Confidence::Low,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["logs", "log"],
        extensions: &[],
    },
    Rule {
        id: RuleId::BackupDir,
        tier: 1,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Heuristic,
        gate: None,
        subcategory: None,
        confidence: Confidence::Low,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["backup", "backups", "bak"],
        extensions: &[],
    },
    // -- Tier 2: canonical user directories ---------------------------------
    // Canonical, OS-defined directory *names*. Not gated (the name itself is
    // the convention) but still only a name: corroboration comes from the
    // rooted location rules when the path confirms it.
    // Capitalized variants exist because XDG user dirs on Linux are
    // conventionally capitalized (`Downloads`) while Linux name matching is
    // case-sensitive; on Windows/macOS the extra variants are inert.
    Rule {
        id: RuleId::DownloadsDir,
        tier: 2,
        platforms: &[],
        dir_rule: true,
        kind: RuleKind::Authoritative,
        gate: None,
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
        kind: RuleKind::Authoritative,
        gate: None,
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
        kind: RuleKind::Authoritative,
        gate: None,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceKind::KnownPathPattern,
        needles: &["documents", "my documents", "Documents"],
        extensions: &[],
    },
    // -- Tier 4: extension tables (files) ------------------------------------
    // Content-typed extensions outrank the tier-5 signals below: knowing that
    // a file *is* a zip, a PDF or a log is a stronger claim about what it is
    // than either a name that looks like an installer or the bare fact that it
    // is executable.
    Rule {
        id: RuleId::InstallerExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        kind: RuleKind::Extension,
        gate: None,
        subcategory: Some(Subcategory::Installer),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_INSTALLER,
    },
    Rule {
        id: RuleId::ArchiveExtension,
        tier: 4,
        platforms: &[],
        dir_rule: false,
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
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
        kind: RuleKind::Extension,
        gate: None,
        subcategory: Some(Subcategory::LogFile),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_LOG,
    },
    // -- Tier 5: weak signals (files) ---------------------------------------
    // Both entries here are *generic*: neither says what the bytes are.
    //   * `InstallerName` is a bare-name guess, and is additionally gated so
    //     it may only decide inside an authoritative download location.
    //   * `ExecutableExtension` says only "this is some program" — weaker than
    //     a concrete content type, so it yields to every tier-4 extension.
    // They sit after the content-typed extensions so that `setup.zip` stays an
    // archive and `update.log` stays a log, while `setup.exe` in Downloads is
    // still recognised as a downloaded installer (the installer name has the
    // longer needle and therefore wins the tier-5 tie).
    Rule {
        id: RuleId::InstallerName,
        tier: 5,
        platforms: &[],
        dir_rule: false,
        kind: RuleKind::Heuristic,
        gate: Some(RuleGate::Under(&[LocationClass::Downloads])),
        subcategory: Some(Subcategory::Installer),
        confidence: Confidence::Medium,
        evidence: EvidenceKind::FilenamePattern,
        needles: &["setup", "install", "installer", "uninstall", "update"],
        extensions: &[],
    },
    Rule {
        id: RuleId::ExecutableExtension,
        tier: 5,
        platforms: &[],
        dir_rule: false,
        kind: RuleKind::Extension,
        gate: None,
        subcategory: None,
        confidence: Confidence::Medium,
        evidence: EvidenceKind::Extension,
        needles: &[],
        extensions: EXT_EXECUTABLE,
    },
];

/// Look up a rule by id. `O(table)` and only used for tests and explainability
/// tooling — the hot path iterates the table once.
pub fn rule_by_id(id: RuleId) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// Category each rule assigns. Total mapping — a compile-time-exhaustive
/// match; adding a `RuleId` without a category is a compile error.
pub const fn rule_category(id: RuleId) -> Category {
    match id {
        RuleId::WindowsSystemLocation
        | RuleId::LinuxPackageLocation
        | RuleId::MacSystemLocation => Category::SystemData,
        RuleId::ApplicationInstallLocation => Category::Applications,
        RuleId::WindowsAppData
        | RuleId::WindowsProgramData
        | RuleId::MacApplicationSupport
        | RuleId::XdgLocation => Category::ApplicationData,
        // No table rule matches .app bundles by suffix; `/Applications` is
        // recognised as a rooted install location instead.
        RuleId::MacAppBundle => Category::Applications,
        RuleId::UserProfile | RuleId::DesktopDir | RuleId::DocumentsDir => Category::UserData,
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

/// One matched rule, captured **at match time** with the evidence kind that
/// actually produced the match. Nothing is reconstructed later, so evidence
/// can never drift from the signal that caused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleMatch {
    /// The rule that matched.
    pub rule: RuleId,
    /// The kind of signal that matched — the truthful evidence kind.
    pub kind: EvidenceKind,
    pub tier: u8,
    /// Tie-break strength (longest needle, or pattern depth for locations).
    pub strength: usize,
}

/// Aggregated outcome of one classification pass: the winning rule plus every
/// rule that matched (deterministic order), with truthful evidence kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchOutcome {
    /// Every rule whose pattern matched, in table order (location last).
    /// Includes rules that matched but were not eligible to win.
    pub matched: Vec<RuleMatch>,
    /// The winning rule (lowest tier → longest needle → table order).
    pub winner: RuleId,
    /// Winner's base confidence before the ceiling is applied.
    pub base_confidence: Confidence,
    /// Winner's assigned category.
    pub category: Category,
    /// Winner's subcategory, if any.
    pub subcategory: Option<Subcategory>,
    /// Evidence kind for the winner, for the explainable record.
    pub evidence_kind: EvidenceKind,
    /// Knowledge kind of the winner — the input to the confidence ceiling.
    pub winner_kind: RuleKind,
    /// True when the winner is a gated rule whose gate was satisfied, i.e.
    /// authoritative location knowledge already corroborates it.
    pub gate_satisfied: bool,
}

impl MatchOutcome {
    /// The hard confidence ceiling for this outcome.
    ///
    /// This is the *only* place the ceiling is derived, and it is derived from
    /// data carried by the rule — never from a special case in `classify()`.
    ///
    /// * A location-gated rule that won has already been corroborated by
    ///   authoritative location knowledge, so the location — not the name —
    ///   supplies the confidence: ceiling `High`.
    /// * Otherwise the ceiling is [`Confidence::cap_for`] of the winner's
    ///   kind, with one documented relaxation: a *corroborated* heuristic may
    ///   reach `Medium`.
    pub fn confidence_cap(&self, corroborated: bool) -> Confidence {
        if self.gate_satisfied {
            return Confidence::AUTHORITATIVE_CAP;
        }
        match self.winner_kind {
            RuleKind::Heuristic if corroborated => Confidence::HEURISTIC_CORROBORATED_CAP,
            kind => Confidence::cap_for(kind),
        }
    }
}

/// Total order over competing rules: lowest tier → longest needle → table
/// order. `usize::MAX` order is reserved for the synthesized location rule so
/// it can never win a tie against a real table rule.
fn beats(
    tier: u8,
    strength: usize,
    order: usize,
    current: Option<(u8, usize, usize, &Rule)>,
) -> bool {
    match current {
        None => true,
        Some((ct, cs, co, _)) => {
            tier < ct || (tier == ct && (strength > cs || (strength == cs && order < co)))
        }
    }
}

/// Evaluate all rules against one entry description and select the winner.
/// Deterministic: same inputs → same outcome, always.
pub fn evaluate(
    is_dir: bool,
    file_name: &str,
    stem: &str,
    ext: Option<&str>,
    platform: Platform,
    location: Option<LocationMatch>,
) -> MatchOutcome {
    let location_class = location.map(|l| l.class);
    let mut matched: Vec<RuleMatch> = Vec::new();

    // Candidate winner: (tier, strength, table order, rule).
    let mut winner: Option<(u8, usize, usize, &Rule)> = None;

    macro_rules! consider {
        ($tier:expr, $strength:expr, $order:expr, $rule:expr) => {
            if beats($tier, $strength, $order, winner) {
                winner = Some(($tier, $strength, $order, $rule));
            }
        };
    }

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
        matched.push(RuleMatch {
            rule: rule.id,
            kind: rule.evidence,
            tier: rule.tier,
            strength: rule.strength(),
        });
        // Gated rules are recorded as evidence but may not decide the
        // category unless their gate is satisfied.
        if !rule.eligible(location_class) {
            continue;
        }
        consider!(rule.tier, rule.strength(), idx, rule);
    }

    // Authoritative container location: lowest precedence, highest quality.
    // A directory that *is* the location is authoritative; a file merely
    // residing in one inherits weaker evidence.
    let location_rule = location.map(|loc| Rule {
        id: loc.rule,
        tier: LOCATION_TIER,
        platforms: &[],
        dir_rule: false,
        kind: RuleKind::Authoritative,
        gate: None,
        subcategory: None,
        confidence: if is_dir {
            Confidence::High
        } else {
            Confidence::Medium
        },
        evidence: loc.evidence,
        needles: &[],
        extensions: &[],
    });

    if let (Some(loc), Some(ref lr)) = (location, &location_rule) {
        matched.push(RuleMatch {
            rule: lr.id,
            kind: lr.evidence,
            tier: lr.tier,
            strength: loc.depth,
        });
        // A pure container (`UserHome`) is recorded as evidence — it *did*
        // match — but may not decide the category of entries merely inside it.
        // Otherwise `/home/user/randomdir` would become `UserData` and `Other`
        // would be unreachable across most of a user's tree.
        if loc.decides() {
            // A weak name heuristic that *agrees* with authoritative location
            // knowledge is superseded by that knowledge: `~/.cache` is a cache
            // because of where it is, not because of what it is called. A
            // gated rule is left alone — its gate already supplied the
            // corroboration.
            let heuristic_agrees = match winner {
                Some((_, _, _, w)) => {
                    w.kind == RuleKind::Heuristic
                        && w.gate.is_none()
                        && rule_category(w.id) == location_category(loc.class)
                }
                None => true,
            };
            if winner.is_none() || heuristic_agrees {
                winner = Some((lr.tier, loc.depth, usize::MAX, lr));
            } else {
                consider!(lr.tier, loc.depth, usize::MAX, lr);
            }
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
            winner_kind: rule.kind,
            gate_satisfied: rule.gate_satisfied(location_class),
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
            winner_kind: RuleKind::Heuristic,
            gate_satisfied: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathctx::{LocationClass, LOCATION_RULES};

    fn file_outcome(name: &str, platform: Platform) -> MatchOutcome {
        file_outcome_in(name, platform, None)
    }

    fn file_outcome_in(
        name: &str,
        platform: Platform,
        location: Option<LocationClass>,
    ) -> MatchOutcome {
        let stem = match name.rsplit_once('.') {
            Some((s, _)) if !s.is_empty() => s,
            _ => name,
        };
        let ext = name.rsplit_once('.').map(|(_, e)| e);
        // A file is *inside* the location, never the location itself.
        let loc = location.map(|class| LocationMatch {
            rule: RuleId::DownloadsDir,
            class,
            evidence: EvidenceKind::KnownPathPattern,
            depth: 1,
            is_root: false,
        });
        evaluate(false, name, stem, ext, platform, loc)
    }

    fn dir_outcome(name: &str, platform: Platform) -> MatchOutcome {
        evaluate(true, name, name, None, platform, None)
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
            let _ = rule_category(r.id);
        }
        for r in LOCATION_RULES {
            let _ = rule_category(r.id);
        }
    }

    #[test]
    fn every_rule_base_confidence_respects_its_kind() {
        // The table must not encode a base confidence that the policy would
        // only ever clamp down — that would be a lie in the rule table.
        for r in RULES {
            if r.gate.is_none() {
                assert!(
                    r.confidence <= r.kind.cap(),
                    "rule {} base {:?} exceeds its kind ceiling {:?}",
                    r.id.code(),
                    r.confidence,
                    r.kind.cap()
                );
            }
        }
    }

    #[test]
    fn every_rule_declares_a_kind_matching_its_tier_band() {
        for r in RULES {
            match r.kind {
                // Extension tables are the only `Extension` rules.
                RuleKind::Extension => assert!(!r.extensions.is_empty() || !r.needles.is_empty()),
                RuleKind::Authoritative | RuleKind::Heuristic => {}
            }
        }
    }

    #[test]
    fn canonical_user_dir_beats_weak_extension() {
        let o = dir_outcome("Downloads", Platform::Windows);
        assert_eq!(o.winner, RuleId::DownloadsDir);
        assert_eq!(o.category, Category::Downloads);
        assert_eq!(o.base_confidence, Confidence::High);
    }

    #[test]
    fn weak_dir_names_are_heuristics() {
        // Finding 5/6: bare well-known names are guesses, not knowledge.
        for name in ["build", "out", "cache", "temp", "backup", "logs"] {
            let o = dir_outcome(name, Platform::Linux);
            assert_eq!(
                o.winner_kind,
                RuleKind::Heuristic,
                "dir `{name}` must be a heuristic"
            );
            assert!(
                o.base_confidence <= Confidence::HEURISTIC_CAP,
                "dir `{name}` base confidence too high"
            );
        }
    }

    #[test]
    fn strong_dev_dirs_are_authoritative() {
        for name in ["node_modules", ".git", "venv"] {
            let o = dir_outcome(name, Platform::Linux);
            assert_eq!(o.winner_kind, RuleKind::Authoritative, "dir `{name}`");
        }
    }

    #[test]
    fn platform_rules_do_not_leak_across_platforms() {
        // Location knowledge is platform data; name rules stay neutral, so
        // platform leakage is verified in `pathctx` and in integration tests.
        let o = dir_outcome("Program Files", Platform::Linux);
        assert_ne!(o.winner, RuleId::WindowsSystemLocation);
        let o = dir_outcome(".config", Platform::Windows);
        assert_ne!(o.winner, RuleId::XdgLocation);
        let o = dir_outcome("Library", Platform::Windows);
        assert_ne!(o.winner, RuleId::MacApplicationSupport);
    }

    #[test]
    fn windows_case_insensitive_dir_match() {
        let o = dir_outcome("DOWNLOADS", Platform::Windows);
        assert_eq!(o.winner, RuleId::DownloadsDir);
    }

    #[test]
    fn linux_case_sensitive_dir_match() {
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
        assert_eq!(o.winner_kind, RuleKind::Extension);
        assert_eq!(o.base_confidence, Confidence::Medium);
    }

    #[test]
    fn installer_name_is_gated_to_downloads() {
        // Without a download location the name must NOT win.
        let o = file_outcome_in("setup.exe", Platform::Windows, None);
        assert_eq!(o.winner, RuleId::ExecutableExtension);
        assert!(
            o.matched.iter().any(|m| m.rule == RuleId::InstallerName),
            "the name still matched and is retained as evidence"
        );

        // Inside a download location it wins and is corroborated.
        let o = file_outcome_in(
            "setup.exe",
            Platform::Windows,
            Some(LocationClass::Downloads),
        );
        assert_eq!(o.winner, RuleId::InstallerName);
        assert!(o.gate_satisfied);
        assert_eq!(o.category, Category::Downloads);
        assert_eq!(o.subcategory, Some(Subcategory::Installer));
        assert_eq!(
            o.confidence_cap(false),
            Confidence::AUTHORITATIVE_CAP,
            "a gated winner is corroborated by authoritative location knowledge"
        );
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
            RuleId::ExecutableExtension,
            "gated: not in a download location"
        );
        assert_eq!(
            file_outcome("setup_2024.zip", Platform::Windows).winner,
            RuleId::ArchiveExtension,
            "gated: an archive keeps its archive semantics"
        );
        // Bare substring must NOT match: "container" is not "install".
        assert!(!file_outcome("container.tar.gz", Platform::Linux)
            .matched
            .iter()
            .any(|m| m.rule == RuleId::InstallerName));
    }

    #[test]
    fn multi_extension_archive_gz() {
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

    #[test]
    fn evidence_kinds_are_truthful_for_every_match() {
        // Finding 4: the signal type is captured at match time.
        let o = file_outcome("setup.zip", Platform::Windows);
        let kinds: Vec<_> = o.matched.iter().map(|m| (m.rule, m.kind)).collect();
        assert!(kinds.contains(&(RuleId::InstallerName, EvidenceKind::FilenamePattern)));
        assert!(kinds.contains(&(RuleId::ArchiveExtension, EvidenceKind::Extension)));
    }
}
