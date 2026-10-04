//! Application footprint discovery (Phase 6): candidate locations an
//! installed application may own, each with typed evidence. No
//! recursive claiming of same-named files; every association is an
//! evidence-backed candidate.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::{ApplicationId, ApplicationRecord};
use crate::evidence::{AssociationScope, Confidence, EvidenceKind, FootprintEvidence};

/// What kind of footprint a candidate path represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FootprintKind {
    InstallationDirectory,
    UserData,
    Cache,
    Logs,
    Temporary,
    ShortcutEntry,
    StartupIntegration,
    Configuration,
    Unknown,
}

/// One candidate footprint association.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FootprintCandidate {
    pub path: PathBuf,
    pub app: ApplicationId,
    pub kind: FootprintKind,
    pub confidence: Confidence,
    pub evidence: Vec<FootprintEvidence>,
}

/// Probes a filesystem (real or fake), non-recursively.
pub trait PathProber {
    /// Immediate child directories of `dir`.
    fn children(&self, dir: &Path) -> Vec<PathBuf>;
    /// Immediate entries (any kind) of `dir`.
    fn entries(&self, dir: &Path) -> Vec<PathBuf>;
}

/// Known roots for footprint probing (platform-parameterized).
#[derive(Debug, Clone, Default)]
pub struct KnownRoots {
    pub program_data: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub roaming_app_data: Option<PathBuf>,
    pub start_menu_programs: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    pub startup_folder: Option<PathBuf>,
}

/// Normalize a name for fuzzy directory matching.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_sep = true;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push(' ');
            last_was_sep = true;
        }
    }
    out.trim().to_string()
}

fn child_name(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string()
}

fn names_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.contains(b) || b.contains(a)
}

fn looks_like_cache(n: &str) -> bool {
    n.contains("cache") || n.contains("tmp") || n.contains("temp")
}

fn looks_like_logs(n: &str) -> bool {
    n.contains("log") || n.contains("crash")
}

fn file_stem_match(entry: &Path, norm: &str) -> bool {
    let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
    names_match(&normalize_name(name), norm)
}

/// Discover footprint candidates for `apps` using bounded probes.
pub fn discover_footprints(
    apps: &[ApplicationRecord],
    roots: &KnownRoots,
    prober: &dyn PathProber,
) -> Vec<FootprintCandidate> {
    let mut out = Vec::new();
    for app in apps {
        let norm = normalize_name(&app.name);
        let publisher_norm = app.publisher.as_deref().map(normalize_name);

        if let Some(loc) = &app.install_location {
            out.push(FootprintCandidate {
                path: loc.clone(),
                app: app.id.clone(),
                kind: FootprintKind::InstallationDirectory,
                confidence: Confidence::Confirmed,
                evidence: vec![FootprintEvidence::new(
                    EvidenceKind::InstallLocation,
                    Confidence::Confirmed,
                    "inventory",
                    AssociationScope::ThisMachine,
                    "directory equals the installer-recorded install location",
                )],
            });
        }

        for (root, scope) in [
            (&roots.program_data, AssociationScope::ThisMachine),
            (&roots.local_app_data, AssociationScope::CurrentUser),
            (&roots.roaming_app_data, AssociationScope::CurrentUser),
        ] {
            let Some(root) = root else { continue };
            for child in prober.children(root) {
                let child_norm = normalize_name(&child_name(&child));
                if child_norm.is_empty() {
                    continue;
                }
                // A directory that IS the publisher name is treated as
                // a publisher directory (probe one level deeper for the
                // app), not as a same-named application directory.
                let is_publisher_dir = publisher_norm
                    .as_deref()
                    .map(|p| child_norm == p)
                    .unwrap_or(false);
                if is_publisher_dir {
                    for grand in prober.children(&child) {
                        let grand_norm = normalize_name(&child_name(&grand));
                        if names_match(&grand_norm, &norm) {
                            out.push(FootprintCandidate {
                                path: grand,
                                app: app.id.clone(),
                                kind: FootprintKind::UserData,
                                confidence: Confidence::Strong,
                                evidence: vec![
                                    FootprintEvidence::new(
                                        EvidenceKind::PublisherDirectory,
                                        Confidence::Strong,
                                        "footprint-scan",
                                        scope,
                                        "parent directory matches the application's publisher",
                                    ),
                                    FootprintEvidence::new(
                                        EvidenceKind::KnownApplicationDirectory,
                                        Confidence::Strong,
                                        "footprint-scan",
                                        scope,
                                        "directory name matches the installed application name under the publisher directory",
                                    ),
                                ],
                            });
                        }
                    }
                } else if names_match(&child_norm, &norm) {
                    let kind = if looks_like_cache(&child_norm) {
                        FootprintKind::Cache
                    } else if looks_like_logs(&child_norm) {
                        FootprintKind::Logs
                    } else if looks_like_temp(&child_norm) {
                        FootprintKind::Temporary
                    } else {
                        FootprintKind::UserData
                    };
                    out.push(FootprintCandidate {
                        path: child,
                        app: app.id.clone(),
                        kind,
                        confidence: Confidence::Probable,
                        evidence: vec![FootprintEvidence::new(
                            EvidenceKind::KnownApplicationDirectory,
                            Confidence::Probable,
                            "footprint-scan",
                            scope,
                            "directory name matches the installed application name under a standard application data root",
                        )],
                    });
                }
            }
        }

        if let Some(sm) = &roots.start_menu_programs {
            for entry in prober.entries(sm) {
                if entry.extension().and_then(|e| e.to_str()) == Some("lnk")
                    && file_stem_match(&entry, &norm)
                {
                    out.push(FootprintCandidate {
                        path: entry,
                        app: app.id.clone(),
                        kind: FootprintKind::ShortcutEntry,
                        confidence: Confidence::Probable,
                        evidence: vec![FootprintEvidence::new(
                            EvidenceKind::ShortcutReference,
                            Confidence::Probable,
                            "footprint-scan",
                            AssociationScope::ThisMachine,
                            "Start Menu shortcut file name matches the installed application name",
                        )],
                    });
                }
            }
        }

        if let Some(st) = &roots.startup_folder {
            for entry in prober.entries(st) {
                if file_stem_match(&entry, &norm) {
                    out.push(FootprintCandidate {
                        path: entry,
                        app: app.id.clone(),
                        kind: FootprintKind::StartupIntegration,
                        confidence: Confidence::Probable,
                        evidence: vec![FootprintEvidence::new(
                            EvidenceKind::ShortcutReference,
                            Confidence::Probable,
                            "footprint-scan",
                            AssociationScope::CurrentUser,
                            "startup entry name matches the installed application name",
                        )],
                    });
                }
            }
        }

        if let Some(dt) = &roots.desktop {
            for entry in prober.entries(dt) {
                if entry.extension().and_then(|e| e.to_str()) == Some("lnk")
                    && file_stem_match(&entry, &norm)
                {
                    out.push(FootprintCandidate {
                        path: entry,
                        app: app.id.clone(),
                        kind: FootprintKind::ShortcutEntry,
                        confidence: Confidence::Probable,
                        evidence: vec![FootprintEvidence::new(
                            EvidenceKind::ShortcutReference,
                            Confidence::Probable,
                            "footprint-scan",
                            AssociationScope::CurrentUser,
                            "desktop shortcut file name matches the installed application name",
                        )],
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
    });
    out.dedup_by(|a, b| a.path == b.path && a.app == b.app && a.kind == b.kind);
    out
}

fn looks_like_temp(n: &str) -> bool {
    n.contains("tmp") || n.contains("temp")
}
