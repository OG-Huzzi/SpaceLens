//! Typed evidence model.
//!
//! Every classification must answer "why did SpaceLens classify this this
//! way?" — evidence is the answer. Evidence kinds are stable typed
//! identifiers (never free-form strings), the evidence list is bounded, and
//! its ordering is deterministic (rule-table order). Evidence never contains
//! path text — only the *kind* of signal that matched, preserving privacy
//! (docs/SECURITY_AND_SAFETY.md).

use serde::{Deserialize, Serialize};

/// What kind of signal produced an evidence item. Stable typed identifiers —
/// never free-form strings (master prompt §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceKind {
    /// A well-known path pattern matched (e.g. a cache directory name).
    KnownPathPattern,
    /// The entry sits in a known application/software location.
    KnownApplicationLocation,
    /// The entry sits in a known cache location.
    KnownCacheLocation,
    /// The entry sits in a known temporary-data location.
    KnownTemporaryLocation,
    /// The entry sits in a known system location.
    KnownSystemLocation,
    /// A known development-artifact pattern matched (node_modules, .git…).
    DevelopmentArtifact,
    /// A filename pattern matched (e.g. `setup`, `installer`).
    FilenamePattern,
    /// The file extension matched a content-type table.
    Extension,
    /// The entry's kind/metadata contributed (e.g. a directory with no
    /// content signals).
    EntryKind,
    /// A parent/ancestor directory classification contributed context.
    ParentContext,
    /// Platform-specific location knowledge (e.g. XDG cache dir).
    PlatformLocation,
}

impl EvidenceKind {
    /// Stable IPC identifier.
    pub fn code(self) -> &'static str {
        match self {
            EvidenceKind::KnownPathPattern => "KNOWN_PATH_PATTERN",
            EvidenceKind::KnownApplicationLocation => "KNOWN_APPLICATION_LOCATION",
            EvidenceKind::KnownCacheLocation => "KNOWN_CACHE_LOCATION",
            EvidenceKind::KnownTemporaryLocation => "KNOWN_TEMPORARY_LOCATION",
            EvidenceKind::KnownSystemLocation => "KNOWN_SYSTEM_LOCATION",
            EvidenceKind::DevelopmentArtifact => "DEVELOPMENT_ARTIFACT",
            EvidenceKind::FilenamePattern => "FILENAME_PATTERN",
            EvidenceKind::Extension => "EXTENSION",
            EvidenceKind::EntryKind => "ENTRY_KIND",
            EvidenceKind::ParentContext => "PARENT_CONTEXT",
            EvidenceKind::PlatformLocation => "PLATFORM_LOCATION",
        }
    }
}

/// One piece of evidence for a classification decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub kind: EvidenceKind,
    /// The rule that produced this evidence (stable rule id).
    pub rule: RuleId,
}

/// Stable rule identifiers. Versioned: removing or renaming a rule id is a
/// contract change; adding new ids is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuleId {
    /// Executable/package installer names (setup, install, installer…).
    InstallerName,
    /// Installer-like extensions (.msi, .dmg, .pkg, .deb, .rpm…).
    InstallerExtension,
    /// Executable extensions (.exe, .app, .appimage…).
    ExecutableExtension,
    /// Compressed-archive extensions (.zip, .7z, .tar.gz…).
    ArchiveExtension,
    /// Disk-image extensions (.iso, .img).
    DiskImageExtension,
    /// Document extensions (.pdf, .docx, .txt…).
    DocumentExtension,
    /// Image extensions (.png, .jpg, .svg…).
    ImageExtension,
    /// Video extensions (.mp4, .mkv, .mov…).
    VideoExtension,
    /// Audio extensions (.mp3, .flac, .wav…).
    AudioExtension,
    /// Source-code / script extensions (.rs, .ts, .py…).
    SourceExtension,
    /// Log extensions (.log, .log.1).
    LogExtension,
    /// Package-manager dependency directories (node_modules, vendor…).
    DependencyDir,
    /// Build-output directories (target, dist, build, out…).
    BuildDir,
    /// Version-control internals (.git, .svn, .hg).
    VcsDir,
    /// Virtual-environment directories (venv, .venv, __pycache__…).
    VirtualenvDir,
    /// Cache directories (cache, Cache, __pycache__…).
    CacheDir,
    /// Temporary directories (tmp, temp…).
    TempDir,
    /// Log directories (logs, log…).
    LogDir,
    /// Downloads directory (user profile context).
    DownloadsDir,
    /// Desktop directory (user profile context).
    DesktopDir,
    /// Documents directory (user profile context).
    DocumentsDir,
    /// Backups / archive-keeper directories.
    BackupDir,
    /// Windows system locations (Windows, Program Files…).
    WindowsSystemLocation,
    /// Windows user-profile AppData trees.
    WindowsAppData,
    /// macOS Library/Application Support locations.
    MacApplicationSupport,
    /// macOS app bundles (.app).
    MacAppBundle,
    /// Linux XDG data/config/cache locations.
    XdgLocation,
    /// Linux package-manager conventions (dpkg, flatpak, snap…).
    LinuxPackageLocation,
    /// Steam library / steamapps content locations.
    SteamLibrary,
    /// Directory with no content signals: `Other` with low confidence.
    DirWithoutSignals,
    /// Regular file with no usable content signals: `Other` with low
    /// confidence (the entry is understood to be a file, nothing more).
    PlainFile,
    /// Entry with no usable signals at all: `Unknown`.
    NoSignals,
    /// Extension matched but nothing else: capped at Medium confidence.
    ExtensionOnly,
    /// Parent directory was classified; context propagated.
    ParentInherited,
}

impl RuleId {
    /// Stable IPC identifier.
    pub fn code(self) -> &'static str {
        match self {
            RuleId::InstallerName => "INSTALLER_NAME",
            RuleId::InstallerExtension => "INSTALLER_EXTENSION",
            RuleId::ExecutableExtension => "EXECUTABLE_EXTENSION",
            RuleId::ArchiveExtension => "ARCHIVE_EXTENSION",
            RuleId::DiskImageExtension => "DISK_IMAGE_EXTENSION",
            RuleId::DocumentExtension => "DOCUMENT_EXTENSION",
            RuleId::ImageExtension => "IMAGE_EXTENSION",
            RuleId::VideoExtension => "VIDEO_EXTENSION",
            RuleId::AudioExtension => "AUDIO_EXTENSION",
            RuleId::SourceExtension => "SOURCE_EXTENSION",
            RuleId::LogExtension => "LOG_EXTENSION",
            RuleId::DependencyDir => "DEPENDENCY_DIR",
            RuleId::BuildDir => "BUILD_DIR",
            RuleId::VcsDir => "VCS_DIR",
            RuleId::VirtualenvDir => "VIRTUALENV_DIR",
            RuleId::CacheDir => "CACHE_DIR",
            RuleId::TempDir => "TEMP_DIR",
            RuleId::LogDir => "LOG_DIR",
            RuleId::DownloadsDir => "DOWNLOADS_DIR",
            RuleId::DesktopDir => "DESKTOP_DIR",
            RuleId::DocumentsDir => "DOCUMENTS_DIR",
            RuleId::BackupDir => "BACKUP_DIR",
            RuleId::WindowsSystemLocation => "WINDOWS_SYSTEM_LOCATION",
            RuleId::WindowsAppData => "WINDOWS_APP_DATA",
            RuleId::MacApplicationSupport => "MAC_APPLICATION_SUPPORT",
            RuleId::MacAppBundle => "MAC_APP_BUNDLE",
            RuleId::XdgLocation => "XDG_LOCATION",
            RuleId::LinuxPackageLocation => "LINUX_PACKAGE_LOCATION",
            RuleId::SteamLibrary => "STEAM_LIBRARY",
            RuleId::DirWithoutSignals => "DIR_WITHOUT_SIGNALS",
            RuleId::PlainFile => "PLAIN_FILE",
            RuleId::NoSignals => "NO_SIGNALS",
            RuleId::ExtensionOnly => "EXTENSION_ONLY",
            RuleId::ParentInherited => "PARENT_INHERITED",
        }
    }
}

/// Maximum evidence items retained per classification. Bounded by design —
/// a hostile tree cannot balloon memory (mirrors the Phase 1 error-report
/// bound philosophy). 8 is enough for diagnosis; more is noise.
pub const MAX_EVIDENCE: usize = 8;

/// Bounded, deterministically-ordered evidence list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceList {
    items: Vec<Evidence>,
}

impl EvidenceList {
    pub fn new() -> Self {
        EvidenceList { items: Vec::new() }
    }

    /// Push evidence if capacity remains. Bounded: drops beyond
    /// [`MAX_EVIDENCE`] (counts stay in the rule aggregation, evidence stays
    /// bounded).
    pub fn push(&mut self, kind: EvidenceKind, rule: RuleId) {
        if self.items.len() < MAX_EVIDENCE {
            self.items.push(Evidence { kind, rule });
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Evidence> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_is_bounded() {
        let mut list = EvidenceList::new();
        for _ in 0..100 {
            list.push(EvidenceKind::Extension, RuleId::ArchiveExtension);
        }
        assert_eq!(list.len(), MAX_EVIDENCE);
    }

    #[test]
    fn evidence_order_is_insertion_order() {
        let mut list = EvidenceList::new();
        list.push(EvidenceKind::KnownPathPattern, RuleId::CacheDir);
        list.push(EvidenceKind::Extension, RuleId::LogExtension);
        let items: Vec<_> = list.iter().collect();
        assert_eq!(items[0].rule, RuleId::CacheDir);
        assert_eq!(items[1].rule, RuleId::LogExtension);
    }

    #[test]
    fn evidence_codes_unique() {
        let kinds = [
            EvidenceKind::KnownPathPattern,
            EvidenceKind::KnownApplicationLocation,
            EvidenceKind::KnownCacheLocation,
            EvidenceKind::KnownTemporaryLocation,
            EvidenceKind::KnownSystemLocation,
            EvidenceKind::DevelopmentArtifact,
            EvidenceKind::FilenamePattern,
            EvidenceKind::Extension,
            EvidenceKind::EntryKind,
            EvidenceKind::ParentContext,
            EvidenceKind::PlatformLocation,
        ];
        let mut seen = std::collections::HashSet::new();
        for k in kinds {
            assert!(seen.insert(k.code()));
        }
    }

    #[test]
    fn rule_codes_unique() {
        let rules = [
            RuleId::InstallerName,
            RuleId::InstallerExtension,
            RuleId::ExecutableExtension,
            RuleId::ArchiveExtension,
            RuleId::DiskImageExtension,
            RuleId::DocumentExtension,
            RuleId::ImageExtension,
            RuleId::VideoExtension,
            RuleId::AudioExtension,
            RuleId::SourceExtension,
            RuleId::LogExtension,
            RuleId::DependencyDir,
            RuleId::BuildDir,
            RuleId::VcsDir,
            RuleId::VirtualenvDir,
            RuleId::CacheDir,
            RuleId::TempDir,
            RuleId::LogDir,
            RuleId::DownloadsDir,
            RuleId::DesktopDir,
            RuleId::DocumentsDir,
            RuleId::BackupDir,
            RuleId::WindowsSystemLocation,
            RuleId::WindowsAppData,
            RuleId::MacApplicationSupport,
            RuleId::MacAppBundle,
            RuleId::XdgLocation,
            RuleId::LinuxPackageLocation,
            RuleId::SteamLibrary,
            RuleId::DirWithoutSignals,
            RuleId::PlainFile,
            RuleId::NoSignals,
            RuleId::ExtensionOnly,
            RuleId::ParentInherited,
        ];
        let mut seen = std::collections::HashSet::new();
        for r in rules {
            assert!(seen.insert(r.code()), "duplicate rule code");
        }
    }
}
