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
use crate::observe::{DirectoryObservation, FileObservation, PathObservation};

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
    // ---- Phase 6.2 additions (appended: existing canonical order is
    // unchanged). Footprint roles are an OBSERVATION taxonomy — none of them
    // implies removability.
    /// An executable file belonging to (or recorded for) the application.
    Executable,
    /// A shared library / runtime component.
    SharedLibrary,
    /// Application-owned data that is not user content.
    ApplicationData,
    /// Crash dumps / diagnostic reports.
    CrashData,
    /// Uninstall metadata (registry-recorded uninstaller, receipts).
    UninstallMetadata,
    /// Any other associated artifact.
    Other,
    /// A regular file inside an install tree with no more specific role.
    InstallFile,
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

/// One bounded directory listing: at most `max` canonically-smallest
/// names plus the EXACT count of entries visited beyond them. Memory is
/// O(max) regardless of directory size — the bound is real, not a
/// post-hoc truncate of a fully materialized listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoundedListing {
    /// Canonically ascending, deduplicated names (≤ the requested max).
    pub names: Vec<PathBuf>,
    /// Entries visited beyond the kept set (exact).
    pub overflow: u64,
}

/// Streaming bounded-name insertion over encoded path bytes: keeps the
/// `max` canonically-smallest names; everything else increments
/// `overflow`. O(max) memory, call-order independent (the kept set is
/// always the canonically-smallest names visited). Implementors of
/// [`PathProber`] use this to enumerate without materializing whole
/// directories.
pub fn offer_path(
    set: &mut std::collections::BTreeSet<PathBuf>,
    max: usize,
    name: PathBuf,
    overflow: &mut u64,
) {
    if max == 0 {
        *overflow += 1;
        return;
    }
    if set.contains(&name) {
        return;
    }
    if set.len() < max {
        set.insert(name);
        return;
    }
    let largest = match set.iter().next_back() {
        Some(l) => l.clone(),
        None => {
            set.insert(name);
            return;
        }
    };
    if name < largest {
        set.remove(&largest);
        *overflow += 1;
        set.insert(name);
    } else {
        *overflow += 1;
    }
}

/// Probes a filesystem (real or fake), non-recursively and BOUNDED:
/// implementations must not materialize a directory beyond the requested
/// `max` names (memory stays O(max) however large the directory is).
pub trait PathProber {
    /// Immediate child directories of `dir`, bounded.
    fn children_bounded(&self, dir: &Path, max: usize) -> BoundedListing;
    /// Immediate entries (any kind) of `dir`, bounded.
    fn entries_bounded(&self, dir: &Path, max: usize) -> BoundedListing;

    /// Typed bounded listing with an explicit [`AccessState`] (Phase 6.2).
    /// The default is honest: a prober that does not implement it reports
    /// `Unsupported` — never an empty success.
    ///
    /// [`AccessState`]: coresight_capabilities::AccessState
    fn list_dir(&self, dir: &Path, max: usize) -> DirectoryObservation {
        let _ = (dir, max);
        DirectoryObservation::unsupported("this prober does not expose typed directory listings")
    }

    /// Link-aware metadata including the canonical object identity.
    fn stat(&self, path: &Path) -> PathObservation {
        let _ = path;
        PathObservation::unsupported("this prober does not expose typed metadata")
    }

    /// Bounded read of a metadata file (at most `max_bytes`).
    fn read_file_bounded(&self, path: &Path, max_bytes: u64) -> FileObservation {
        let _ = (path, max_bytes);
        FileObservation::unsupported("this prober does not expose file content")
    }
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
    // Matching key only: a non-UTF-8 name stays a NAME (lossy), it never
    // vanishes into an empty string — it simply will not match an app's
    // name unless it really does.
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
    let name = entry
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    names_match(&normalize_name(&name), norm)
}

/// Bounded listing straight from the prober: the examined subset is the
/// canonically-smallest `cap` names (deterministic under any enumeration
/// order), and the prober never materialized more than `cap` names.
fn bounded_children(
    prober: &dyn PathProber,
    dir: &Path,
    cap: usize,
    truncated: &mut u64,
) -> Vec<PathBuf> {
    let listing = prober.children_bounded(dir, cap);
    *truncated += listing.overflow;
    listing.names
}

/// Same as [`bounded_children`] for arbitrary entries.
fn bounded_entries(
    prober: &dyn PathProber,
    dir: &Path,
    cap: usize,
    truncated: &mut u64,
) -> Vec<PathBuf> {
    let listing = prober.entries_bounded(dir, cap);
    *truncated += listing.overflow;
    listing.names
}

/// The admission key of a footprint candidate: (path bytes, app id, kind).
/// Byte-ordered so canonical order is platform-stable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateKey {
    path_bytes: Vec<u8>,
    app: String,
    kind: FootprintKind,
}

impl CandidateKey {
    fn of(candidate: &FootprintCandidate) -> Self {
        CandidateKey {
            path_bytes: candidate.path.as_os_str().as_encoded_bytes().to_vec(),
            app: candidate.app.0.clone(),
            kind: candidate.kind,
        }
    }
}

/// Canonical precedence between two same-key candidates: stronger
/// confidence wins, then the fuller evidence list. Arrival order is never
/// a tie-breaker.
fn candidate_rank(c: &FootprintCandidate) -> (Confidence, &[FootprintEvidence]) {
    (c.confidence, c.evidence.as_slice())
}

/// Bounded candidate admission. Working memory is O(max_records)
/// candidates — never the whole probe fan-out: a candidate joins only
/// when capacity exists (evicting the canonically-largest key, counted)
/// or its key is canonically smaller than the largest held key. Keys are
/// immutable, so the published set is always the canonically-first
/// `max_records` candidates regardless of probe order. Same-key
/// duplicates resolve by [`candidate_rank`]. The per-candidate evidence
/// bound applies at admission (bounded payloads).
fn admit_candidate(
    admitted: &mut std::collections::BTreeMap<CandidateKey, FootprintCandidate>,
    limits: &DiscoveryLimits,
    truncated: &mut u64,
    evidence_truncated: &mut u64,
    mut candidate: FootprintCandidate,
) {
    // Evidence is published in CANONICAL order, so the same facts always
    // render identically regardless of the order the probes produced them.
    candidate.evidence.sort();
    if candidate.evidence.len() > limits.max_evidence_per_candidate {
        *evidence_truncated +=
            (candidate.evidence.len() - limits.max_evidence_per_candidate) as u64;
        candidate
            .evidence
            .truncate(limits.max_evidence_per_candidate);
    }
    let key = CandidateKey::of(&candidate);
    if let Some(existing) = admitted.get(&key) {
        if candidate_rank(&candidate) > candidate_rank(existing) {
            admitted.insert(key, candidate);
        }
        return;
    }
    if admitted.len() < limits.max_records {
        admitted.insert(key, candidate);
        return;
    }
    let largest = admitted.keys().next_back().cloned();
    match largest {
        Some(largest) if key < largest => {
            admitted.remove(&largest);
            *truncated += 1;
            admitted.insert(key, candidate);
        }
        _ => *truncated += 1,
    }
}

/// Discover footprint candidates for `apps` using bounded probes. See
/// [`FootprintReport`] for how each applied limit is reported.
pub fn discover_footprints(
    apps: &[ApplicationRecord],
    roots: &KnownRoots,
    prober: &dyn PathProber,
    limits: &DiscoveryLimits,
) -> FootprintReport {
    let mut admitted: std::collections::BTreeMap<CandidateKey, FootprintCandidate> =
        std::collections::BTreeMap::new();
    let mut candidates_truncated = 0u64;
    let mut evidence_truncated = 0u64;
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
            admit_candidate(
                &mut admitted,
                limits,
                &mut candidates_truncated,
                &mut evidence_truncated,
                FootprintCandidate {
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
                },
            );
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
                            admit_candidate(
                    &mut admitted,
                    limits,
                    &mut candidates_truncated,
                    &mut evidence_truncated,
                    FootprintCandidate {
                                path: grand,
                                app: app.id.clone(),
                                kind: FootprintKind::UserData,
                                // Both evidence items below derive from the SAME
                                // normalized-name signal (correlated), so they
                                // cannot be summed into a stronger claim: the
                                // candidate is capped at `Probable`
                                // (docs/APPLICATIONS.md, correlation ceiling).
                                confidence: Confidence::Probable,
                                evidence: vec![
                                    FootprintEvidence::new(
                                        EvidenceKind::PublisherDirectory,
                                        Confidence::Probable,
                                        "footprint-scan",
                                        scope,
                                        "parent directory matches the application's publisher",
                                    ),
                                    FootprintEvidence::new(
                                        EvidenceKind::KnownApplicationDirectory,
                                        Confidence::Probable,
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
                    admit_candidate(
                    &mut admitted,
                    limits,
                    &mut candidates_truncated,
                    &mut evidence_truncated,
                    FootprintCandidate {
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
                    admit_candidate(
                        &mut admitted,
                        limits,
                        &mut candidates_truncated,
                        &mut evidence_truncated,
                        FootprintCandidate {
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
                        },
                    );
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
                    admit_candidate(
                        &mut admitted,
                        limits,
                        &mut candidates_truncated,
                        &mut evidence_truncated,
                        FootprintCandidate {
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
                        },
                    );
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
                    admit_candidate(
                        &mut admitted,
                        limits,
                        &mut candidates_truncated,
                        &mut evidence_truncated,
                        FootprintCandidate {
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
                        },
                    );
                }
            }
        }
    }

    // The published set was admitted under the `max_records` bound during
    // probing (see [`admit_candidate`]); publishing re-orders it into the
    // canonical order. Memory stayed O(max_records) throughout.
    let mut candidates: Vec<FootprintCandidate> = admitted.into_values().collect();
    candidates.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_encoded_bytes()
            .cmp(b.path.as_os_str().as_encoded_bytes())
            .then(a.app.0.cmp(&b.app.0))
            .then(a.kind.cmp(&b.kind))
    });

    FootprintReport {
        candidates,
        candidates_truncated,
        children_truncated,
        apps_truncated,
        evidence_truncated,
    }
}
