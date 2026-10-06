//! Application footprint discovery (Phase 6): candidate locations an
//! installed application may own, each with typed evidence. No
//! recursive claiming of same-named files; every association is an
//! evidence-backed candidate.
//!
//! ## Boundedness (Objective 27)
//!
//! Every probe is explicitly bounded by [`DiscoveryLimits`]: apps
//! probed, children examined per root, evidence items per candidate,
//! and total candidates published. Children are canonically ordered
//! BEFORE capping, so the examined subset is deterministic under any
//! input order; every overflow is counted exactly in [`FootprintReport`]
//! — nothing is silently dropped.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::{ApplicationId, ApplicationRecord, DiscoveryLimits};
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

/// The bounded result of a footprint scan: candidates plus exact counts
/// of everything a limit stopped — never a silently truncated list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FootprintReport {
    /// Canonically ordered candidates (path bytes, app id, kind);
    /// duplicates by (path, app, kind) collapsed.
    pub candidates: Vec<FootprintCandidate>,
    /// Candidates not published because `max_records` was reached.
    pub candidates_truncated: u64,
    /// Directory entries not examined because the per-root child bound
    /// was reached (exact, counted across all roots/probes).
    pub children_truncated: u64,
    /// Apps not probed because the app bound was reached.
    pub apps_truncated: u64,
    /// Evidence items dropped from candidates because the per-candidate
    /// evidence bound was reached.
    pub evidence_truncated: u64,
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

fn looks_like_temp(n: &str) -> bool {
    n.contains("tmp") || n.contains("temp")
}

fn file_stem_match(entry: &Path, norm: &str) -> bool {
    let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
    names_match(&normalize_name(name), norm)
}

/// Canonically ordered, capped children; overflow is added to
/// `truncated`. Sorting happens BEFORE the cap, so which children are
/// examined never depends on the prober's enumeration order.
fn bounded_children(
    prober: &dyn PathProber,
    dir: &Path,
    cap: usize,
    truncated: &mut u64,
) -> Vec<PathBuf> {
    let mut children = prober.children(dir);
    children.sort_by(|a, b| {
        a.as_os_str()
            .as_encoded_bytes()
            .cmp(b.as_os_str().as_encoded_bytes())
    });
    children.dedup();
    if children.len() > cap {
        *truncated += (children.len() - cap) as u64;
        children.truncate(cap);
    }
    children
}

/// Same as [`bounded_children`] for arbitrary entries.
fn bounded_entries(
    prober: &dyn PathProber,
    dir: &Path,
    cap: usize,
    truncated: &mut u64,
) -> Vec<PathBuf> {
    let mut entries = prober.entries(dir);
    entries.sort_by(|a, b| {
        a.as_os_str()
            .as_encoded_bytes()
            .cmp(b.as_os_str().as_encoded_bytes())
    });
    entries.dedup();
    if entries.len() > cap {
        *truncated += (entries.len() - cap) as u64;
        entries.truncate(cap);
    }
    entries
}

/// Discover footprint candidates for `apps` using bounded probes. See
/// [`FootprintReport`] for how each applied limit is reported.
pub fn discover_footprints(
    apps: &[ApplicationRecord],
    roots: &KnownRoots,
    prober: &dyn PathProber,
    limits: &DiscoveryLimits,
) -> FootprintReport {
    let mut out: Vec<FootprintCandidate> = Vec::new();
    let mut children_truncated = 0u64;
    let mut apps_truncated = 0u64;
    // Apps are probed in canonical order, then capped — deterministic.
    let mut ordered: Vec<&ApplicationRecord> = apps.iter().collect();
    ordered.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.0.cmp(&b.id.0))
    });
    if ordered.len() > limits.max_apps_probed {
        apps_truncated = (ordered.len() - limits.max_apps_probed) as u64;
        ordered.truncate(limits.max_apps_probed);
    }

    for app in ordered {
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
            for child in bounded_children(
                prober,
                root,
                limits.max_children_per_root,
                &mut children_truncated,
            ) {
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
                    for grand in bounded_children(
                        prober,
                        &child,
                        limits.max_children_per_root,
                        &mut children_truncated,
                    ) {
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
            for entry in bounded_entries(
                prober,
                sm,
                limits.max_children_per_root,
                &mut children_truncated,
            ) {
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
            for entry in bounded_entries(
                prober,
                st,
                limits.max_children_per_root,
                &mut children_truncated,
            ) {
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
            for entry in bounded_entries(
                prober,
                dt,
                limits.max_children_per_root,
                &mut children_truncated,
            ) {
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

    // Per-candidate evidence bound (declared policy; counted exactly).
    let mut evidence_truncated = 0u64;
    for cand in &mut out {
        if cand.evidence.len() > limits.max_evidence_per_candidate {
            evidence_truncated += (cand.evidence.len() - limits.max_evidence_per_candidate) as u64;
            cand.evidence.truncate(limits.max_evidence_per_candidate);
        }
    }

    // Total candidate bound — after canonical ordering, so which
    // candidates are published is deterministic.
    let candidates_truncated = if out.len() > limits.max_records {
        let overflow = (out.len() - limits.max_records) as u64;
        out.truncate(limits.max_records);
        overflow
    } else {
        0
    };

    FootprintReport {
        candidates: out,
        candidates_truncated,
        children_truncated,
        apps_truncated,
        evidence_truncated,
    }
}
