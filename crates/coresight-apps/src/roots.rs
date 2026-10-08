//! Install-root detection and executable association (Phase 6.2).
//!
//! ```text
//! ApplicationRecord ──▶ install roots ──▶ footprint scope
//!         │
//!         └─────────────▶ executable association
//! ```
//!
//! ## Rules honored here
//!
//! * **Original paths are preserved byte-for-byte.** Candidates are compared
//!   through [`crate::observe::PathKey`] (platform-encoded bytes); only the
//!   *comparison* layer normalizes names ([`crate::footprint::normalize_name`]).
//! * **The application name is never assumed to equal its directory name.**
//!   Roots come from several independent signals, each recorded explicitly.
//! * **A root is a scope, not a claim.** Containing a path asserts nothing
//!   about ownership; the evidence model decides that.
//! * **Bounded**: at most `limits.max_roots_per_app` roots and at most
//!   `limits.max_children_per_root` children are examined per application;
//!   every skipped item is counted, never silently dropped.

use std::path::{Path, PathBuf};

use crate::domain::{ApplicationRecord, DiscoveryLimits};
use crate::footprint::{normalize_name, BoundedListing, KnownRoots, PathProber};
use crate::observe::{PathKey, ProbedKind};
use crate::ownership::{EvidenceSource, EvidenceStrength};
use crate::pathmatch::{extension_is_ascii, file_name_is_ascii, file_name_str, file_stem_str};

/// Which independent signal produced an install-root candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RootSignal {
    /// The installer recorded this exact location.
    InstallerRecorded,
    /// The parent directory of an executable recorded by application
    /// metadata.
    ExecutableParent,
    /// A platform bundle root (`*.app`) containing the recorded executable.
    BundleRoot,
    /// A `.desktop` entry's `Exec`/`TryExec` absolute path.
    DesktopEntryExec,
    /// `<program root>/<publisher>/<name>` structure.
    PublisherThenName,
    /// `<program root>/<name>` structure (normalized-name comparison only).
    NameUnderProgramRoot,
}

impl RootSignal {
    /// The strongest evidence this signal may support.
    pub fn strength(self) -> EvidenceStrength {
        match self {
            RootSignal::InstallerRecorded | RootSignal::DesktopEntryExec => {
                EvidenceStrength::Direct
            }
            RootSignal::ExecutableParent | RootSignal::BundleRoot => EvidenceStrength::Strong,
            RootSignal::PublisherThenName | RootSignal::NameUnderProgramRoot => {
                EvidenceStrength::Weak
            }
        }
    }

    pub fn source(self) -> EvidenceSource {
        match self {
            RootSignal::InstallerRecorded => EvidenceSource::InventoryRecord,
            RootSignal::ExecutableParent => EvidenceSource::ExecutableMetadata,
            RootSignal::BundleRoot => EvidenceSource::BundleMetadata,
            RootSignal::DesktopEntryExec => EvidenceSource::DesktopEntry,
            RootSignal::PublisherThenName | RootSignal::NameUnderProgramRoot => {
                EvidenceSource::FilesystemPathHeuristic
            }
        }
    }

    fn rank(self) -> u8 {
        match self {
            RootSignal::InstallerRecorded => 0,
            RootSignal::DesktopEntryExec => 1,
            RootSignal::ExecutableParent => 2,
            RootSignal::BundleRoot => 3,
            RootSignal::PublisherThenName => 4,
            RootSignal::NameUnderProgramRoot => 5,
        }
    }
}

/// One candidate installation root for an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRoot {
    /// The path exactly as the signal produced it.
    pub path: PathBuf,
    pub signal: RootSignal,
    pub strength: EvidenceStrength,
}

/// Canonical root ordering: path bytes, then signal rank. Arrival order is
/// never a tie-breaker.
fn root_key(r: &InstallRoot) -> (PathKey, u8) {
    (PathKey(r.path.clone()), r.signal.rank())
}

/// Why an install-root probe stopped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RootDetectionCounts {
    /// Root candidates found but not retained because the bound was reached.
    pub roots_truncated: u64,
    /// Directory entries not examined because the per-root child bound was
    /// reached.
    pub children_truncated: u64,
}

/// Program-directory roots (Windows/Linux system application trees) that are
/// searched for `<name>`/`<publisher>/<name>` structure.
#[derive(Debug, Clone, Default)]
pub struct ProgramRoots {
    /// 64-bit program files equivalent.
    pub program_files: Option<PathBuf>,
    /// 32-bit program files equivalent.
    pub program_files_x86: Option<PathBuf>,
    /// `/Applications`.
    pub applications: Option<PathBuf>,
    /// `~/Applications`.
    pub user_applications: Option<PathBuf>,
    /// `/usr/share/applications` (desktop entries, not roots).
    pub system_desktop_entries: Option<PathBuf>,
    /// `~/.local/share/applications`.
    pub user_desktop_entries: Option<PathBuf>,
}

/// Derive a bounded, deterministic set of candidate install roots for `app`.
///
/// Signals, in canonical precedence order (never arrival order):
/// installer-recorded location; an exact executable path's parent; a bundle
/// root inferred from the executable; the executable's grandparent (the
/// `X.app/Contents` → `X.app` shape); `<program root>/<publisher>/<name>`;
/// and `<program root>/<name>`.
///
/// Bounded by `limits.max_roots_per_app`; determinism: the retained set is
/// the canonically-first `max_roots_per_app` roots.
pub fn detect_install_roots(
    app: &ApplicationRecord,
    roots: &ProgramRoots,
    prober: &dyn PathProber,
    limits: &DiscoveryLimits,
) -> (Vec<InstallRoot>, RootDetectionCounts) {
    let mut counts = RootDetectionCounts::default();
    let mut found: Vec<InstallRoot> = Vec::new();
    fn push(found: &mut Vec<InstallRoot>, path: PathBuf, signal: RootSignal) {
        if path.as_os_str().is_empty() {
            return;
        }
        found.push(InstallRoot {
            path,
            signal,
            strength: signal.strength(),
        });
    }

    if let Some(loc) = &app.install_location {
        push(&mut found, loc.clone(), RootSignal::InstallerRecorded);
    }
    if let Some(exe) = &app.executable_path {
        if let Some(parent) = exe.parent() {
            push(
                &mut found,
                parent.to_path_buf(),
                RootSignal::ExecutableParent,
            );
        }
        // `X.app/Contents/MacOS/exe` → `X.app`; only when the shape really
        // matches, so no root is invented from an unrelated path.
        if let Some(root) = bundle_root_of(exe) {
            push(&mut found, root, RootSignal::BundleRoot);
        }
    }

    let norm = normalize_name(&app.name);
    let publisher_norm = app.publisher.as_deref().map(normalize_name);
    for program_root in [
        roots.program_files.as_ref(),
        roots.program_files_x86.as_ref(),
        roots.applications.as_ref(),
        roots.user_applications.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if norm.is_empty() {
            break;
        }
        let mut listing: BoundedListing =
            prober.children_bounded(program_root, limits.max_children_per_root);
        counts.children_truncated += listing.overflow;
        listing.names.sort_by(|a, b| {
            a.as_os_str()
                .as_encoded_bytes()
                .cmp(b.as_os_str().as_encoded_bytes())
        });
        for child in listing.names {
            // Strict decoding: a non-UTF-8 child name is "cannot interpret"
            // and never matches — it must not match through a
            // replacement-character rendering either.
            let Some(child_norm) = child_name_norm(&child) else {
                continue;
            };
            if child_norm.is_empty() {
                continue;
            }
            if let Some(pub_norm) = publisher_norm.as_deref() {
                if child_norm == pub_norm {
                    let mut inner: BoundedListing =
                        prober.children_bounded(&child, limits.max_children_per_root);
                    counts.children_truncated += inner.overflow;
                    inner.names.sort_by(|a, b| {
                        a.as_os_str()
                            .as_encoded_bytes()
                            .cmp(b.as_os_str().as_encoded_bytes())
                    });
                    for grand in inner.names {
                        if let Some(grand_norm) = child_name_norm(&grand) {
                            if names_agree(&grand_norm, &norm) {
                                push(&mut found, grand, RootSignal::PublisherThenName);
                            }
                        }
                    }
                    continue;
                }
            }
            if names_agree(&child_norm, &norm) {
                push(&mut found, child, RootSignal::NameUnderProgramRoot);
            }
        }
    }

    dedup_roots(&mut found);
    found.sort_by_key(root_key);
    if found.len() > limits.max_roots_per_app {
        counts.roots_truncated += (found.len() - limits.max_roots_per_app) as u64;
        found.truncate(limits.max_roots_per_app);
    }
    (found, counts)
}

/// Deduplicate by path across signals: the strongest signal for one path
/// wins canonically (never by arrival).
fn dedup_roots(found: &mut Vec<InstallRoot>) {
    found.sort_by(|a, b| {
        PathKey(a.path.clone())
            .cmp(&PathKey(b.path.clone()))
            .then(a.signal.rank().cmp(&b.signal.rank()))
    });
    found.dedup_by(|a, b| PathKey(a.path.clone()) == PathKey(b.path.clone()));
}

fn names_agree(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(b) || b.contains(a))
}

/// Normalized comparison key for a child path, or `None` when the final
/// component is not UTF-8 ("cannot interpret": never matches, never
/// fabricates agreement through replacement characters).
fn child_name_norm(p: &Path) -> Option<String> {
    file_name_str(p).map(normalize_name)
}

/// `…/X.app/Contents/MacOS/exe` → `…/X.app`. `None` when the shape is not a
/// bundle layout — no root is fabricated.
pub fn bundle_root_of(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if !file_name_is_ascii(macos, b"MacOS") {
        return None;
    }
    let contents = macos.parent()?;
    if !file_name_is_ascii(contents, b"Contents") {
        return None;
    }
    let bundle = contents.parent()?;
    if extension_is_ascii(bundle, b"app") {
        Some(bundle.to_path_buf())
    } else {
        None
    }
}

/// How confidently an application record is tied to an executable artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExecutableStatus {
    /// The source recorded this exact executable path.
    ObservedExact,
    /// Inferred from a known structure (bundle layout) without an exact
    /// recorded path.
    Inferred,
    /// A plausible executable under a known root, name-matched only.
    Candidate,
    /// Nothing usable.
    Unknown,
}

/// An application ↔ executable association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableAssociation {
    pub status: ExecutableStatus,
    pub path: PathBuf,
    pub strength: EvidenceStrength,
}

/// Bound on how many name-matched executable candidates are examined.
pub const MAX_EXECUTABLE_CANDIDATES: usize = 32;

/// Associate `app` with an executable artifact using only local metadata.
///
/// `ObservedExact` requires an exact recorded path; `Inferred` requires the
/// bundle layout; `Candidate` is name-derived inside a detected install root
/// (explicitly weak); otherwise `Unknown`.
pub fn associate_executable(
    app: &ApplicationRecord,
    install_roots: &[InstallRoot],
    prober: &dyn PathProber,
    limits: &DiscoveryLimits,
) -> ExecutableAssociation {
    if let Some(exe) = &app.executable_path {
        let observed = prober.stat(exe);
        if observed.access.is_read() {
            return ExecutableAssociation {
                status: ExecutableStatus::ObservedExact,
                path: exe.clone(),
                strength: EvidenceStrength::Strong,
            };
        }
        return ExecutableAssociation {
            status: ExecutableStatus::Unknown,
            path: exe.clone(),
            strength: EvidenceStrength::Weak,
        };
    }

    let norm = normalize_name(&app.name);
    if norm.is_empty() {
        return ExecutableAssociation {
            status: ExecutableStatus::Unknown,
            path: PathBuf::new(),
            strength: EvidenceStrength::Weak,
        };
    }

    for root in install_roots {
        let mut listing = prober.list_dir(&root.path, limits.max_children_per_root);
        if !listing.access.is_read() {
            continue;
        }
        listing.entries.sort_by(|a, b| {
            PathKey(a.path.clone())
                .cmp(&PathKey(b.path.clone()))
                .then(a.kind.cmp(&b.kind))
        });
        listing.entries.truncate(MAX_EXECUTABLE_CANDIDATES);
        for entry in &listing.entries {
            if entry.kind != ProbedKind::File || !looks_executable(&entry.path) {
                continue;
            }
            let name_hit = child_name_norm(&entry.path).is_some_and(|n| names_agree(&n, &norm));
            let stem_hit = file_stem_norm(&entry.path).is_some_and(|n| names_agree(&n, &norm));
            if name_hit || stem_hit {
                return ExecutableAssociation {
                    status: ExecutableStatus::Candidate,
                    path: entry.path.clone(),
                    strength: EvidenceStrength::Weak,
                };
            }
        }
        // A bundle's recorded structure implies the canonical executable
        // location even when the name does not match the app name.
        if root.signal == RootSignal::BundleRoot {
            if let Some(exe) = app.executable_path.clone() {
                return ExecutableAssociation {
                    status: ExecutableStatus::Inferred,
                    path: exe,
                    strength: EvidenceStrength::Moderate,
                };
            }
        }
    }

    ExecutableAssociation {
        status: ExecutableStatus::Unknown,
        path: PathBuf::new(),
        strength: EvidenceStrength::Weak,
    }
}

fn file_stem_norm(p: &Path) -> Option<String> {
    file_stem_str(p).map(normalize_name)
}

fn looks_executable(p: &Path) -> bool {
    extension_is_ascii(p, b"exe")
        || extension_is_ascii(p, b"com")
        || extension_is_ascii(p, b"bat")
        || extension_is_ascii(p, b"cmd")
        || extension_is_ascii(p, b"app")
        || extension_is_ascii(p, b"bin")
        || extension_is_ascii(p, b"sh")
}

/// Convenience: program roots discovered for the current host's standard
/// locations, when the caller has them. Kept as data so shared logic never
/// branches on the OS.
pub fn program_roots_from(
    program_files: Option<PathBuf>,
    program_files_x86: Option<PathBuf>,
) -> ProgramRoots {
    ProgramRoots {
        program_files,
        program_files_x86,
        ..ProgramRoots::default()
    }
}

/// True when `path` lies inside `root` (component-wise, byte-exact; no
/// canonicalization, no lossy conversion).
pub fn path_within(root: &Path, path: &Path) -> bool {
    if path == root {
        return true;
    }
    let mut r = root.components();
    let mut p = path.components();
    loop {
        match (r.next(), p.next()) {
            (Some(rc), Some(pc)) => {
                if rc != pc {
                    return false;
                }
            }
            (None, _) => return true,
            (Some(_), None) => return false,
        }
    }
}

/// Resolve the install root that contains `path`, when one does. The
/// deepest (most specific) containing root wins canonically.
pub fn containing_root<'a>(roots: &'a [InstallRoot], path: &Path) -> Option<&'a InstallRoot> {
    roots
        .iter()
        .filter(|r| path_within(&r.path, path))
        .max_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .len()
                .cmp(&b.path.as_os_str().as_encoded_bytes().len())
                .then(root_key(a).cmp(&root_key(b)))
        })
}

/// The known roots used by footprint scanning plus the program roots.
#[derive(Debug, Clone, Default)]
pub struct ExtractionRoots {
    pub known: KnownRoots,
    pub program: ProgramRoots,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::{offer_path, BoundedListing};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeProber {
        dirs: BTreeMap<PathBuf, Vec<PathBuf>>,
        files: BTreeMap<PathBuf, Vec<PathBuf>>,
    }

    impl FakeProber {
        fn with_dirs(mut self, parent: &str, children: &[&str]) -> Self {
            self.dirs.insert(
                PathBuf::from(parent),
                children.iter().map(PathBuf::from).collect(),
            );
            self
        }
        fn with_files(mut self, parent: &str, children: &[&str]) -> Self {
            self.files.insert(
                PathBuf::from(parent),
                children.iter().map(PathBuf::from).collect(),
            );
            self
        }
    }

    impl PathProber for FakeProber {
        fn children_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
            let mut set = std::collections::BTreeSet::new();
            let mut overflow = 0u64;
            for name in self.dirs.get(dir).cloned().unwrap_or_default() {
                offer_path(&mut set, max, name, &mut overflow);
            }
            BoundedListing {
                names: set.into_iter().collect(),
                overflow,
            }
        }
        fn entries_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
            let mut set = std::collections::BTreeSet::new();
            let mut overflow = 0u64;
            for name in self.files.get(dir).cloned().unwrap_or_default() {
                offer_path(&mut set, max, name, &mut overflow);
            }
            BoundedListing {
                names: set.into_iter().collect(),
                overflow,
            }
        }
        fn list_dir(&self, dir: &Path, max: usize) -> crate::observe::DirectoryObservation {
            let entries: Vec<PathBuf> = self
                .files
                .get(dir)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .chain(self.dirs.get(dir).cloned().unwrap_or_default())
                .collect();
            if entries.is_empty() && !self.files.contains_key(dir) && !self.dirs.contains_key(dir) {
                return crate::observe::DirectoryObservation::does_not_exist();
            }
            let mut kept = entries;
            kept.sort_by(|a, b| {
                a.as_os_str()
                    .as_encoded_bytes()
                    .cmp(b.as_os_str().as_encoded_bytes())
            });
            let overflow = kept.len().saturating_sub(max) as u64;
            kept.truncate(max);
            crate::observe::DirectoryObservation::read(
                kept.into_iter()
                    .map(|path| crate::observe::ListedEntry {
                        kind: if self.dirs.contains_key(&path) {
                            crate::observe::ProbedKind::Dir
                        } else {
                            crate::observe::ProbedKind::File
                        },
                        path,
                    })
                    .collect(),
                overflow,
            )
        }
        fn stat(&self, path: &Path) -> crate::observe::PathObservation {
            // Present when the path is a registered child of any known
            // directory (the fixture is path-set based, not filesystem-real).
            let known = self
                .files
                .values()
                .chain(self.dirs.values())
                .any(|children| children.iter().any(|c| c == path));
            if known {
                crate::observe::PathObservation::present(
                    crate::observe::ProbedKind::File,
                    None,
                    None,
                )
            } else {
                crate::observe::PathObservation::does_not_exist()
            }
        }
    }

    fn app(name: &str, publisher: Option<&str>) -> ApplicationRecord {
        ApplicationRecord {
            id: crate::domain::ApplicationId::derive(name, publisher),
            name: name.to_string(),
            version: None,
            publisher: publisher.map(str::to_string),
            install_location: None,
            install_date: None,
            estimated_size_bytes: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            install_source: None,
            source: crate::domain::ApplicationSource::RegistryUninstall,
            kind: crate::domain::PackageKind::Installed,
            system_component: false,
            observed_in_views: Vec::new(),
            bundle_identifier: None,
            executable_path: None,
            provenance: Vec::new(),
        }
    }

    #[test]
    fn installer_recorded_root_is_strongest_and_never_invented() {
        let mut a = app("Example", Some("Vendor"));
        a.install_location = Some(PathBuf::from("C:/Program Files/Example"));
        let (roots, counts) = detect_install_roots(
            &a,
            &ProgramRoots::default(),
            &FakeProber::default(),
            &DiscoveryLimits::default(),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].signal, RootSignal::InstallerRecorded);
        assert_eq!(roots[0].strength, EvidenceStrength::Direct);
        assert_eq!(counts.roots_truncated, 0);
    }

    #[test]
    fn name_under_program_root_is_weak_and_requires_a_real_match() {
        let a = app("Example", Some("Vendor"));
        let prober = FakeProber::default().with_dirs(
            "C:/Program Files",
            &["C:/Program Files/Example", "C:/Program Files/Other"],
        );
        let program = ProgramRoots {
            program_files: Some(PathBuf::from("C:/Program Files")),
            ..ProgramRoots::default()
        };
        let (roots, _) = detect_install_roots(&a, &program, &prober, &DiscoveryLimits::default());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].signal, RootSignal::NameUnderProgramRoot);
        assert_eq!(roots[0].strength, EvidenceStrength::Weak);
    }

    #[test]
    fn publisher_then_name_structure_is_detected() {
        let a = app("Spotify", Some("Spotify AB"));
        let prober = FakeProber::default()
            .with_dirs("C:/Program Files", &["C:/Program Files/Spotify AB"])
            .with_dirs(
                "C:/Program Files/Spotify AB",
                &["C:/Program Files/Spotify AB/Spotify"],
            );
        let program = ProgramRoots {
            program_files: Some(PathBuf::from("C:/Program Files")),
            ..ProgramRoots::default()
        };
        let (roots, _) = detect_install_roots(&a, &program, &prober, &DiscoveryLimits::default());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].signal, RootSignal::PublisherThenName);
    }

    #[test]
    fn bundle_root_requires_the_real_bundle_shape() {
        assert_eq!(
            bundle_root_of(Path::new("/Applications/Thing.app/Contents/MacOS/Thing")),
            Some(PathBuf::from("/Applications/Thing.app"))
        );
        assert_eq!(bundle_root_of(Path::new("/usr/bin/thing")), None);
        assert_eq!(
            bundle_root_of(Path::new("/Applications/Thing.app/Resources/Thing")),
            None
        );
    }

    #[test]
    fn exact_executable_wins_over_name_matching() {
        let mut a = app("Example", Some("Vendor"));
        a.executable_path = Some(PathBuf::from("C:/Program Files/Example/bin/example.exe"));
        let prober = FakeProber::default().with_files(
            "C:/Program Files/Example/bin",
            &["C:/Program Files/Example/bin/example.exe"],
        );
        let roots = vec![InstallRoot {
            path: PathBuf::from("C:/Program Files/Example"),
            signal: RootSignal::InstallerRecorded,
            strength: EvidenceStrength::Direct,
        }];
        let assoc = associate_executable(&a, &roots, &prober, &DiscoveryLimits::default());
        assert_eq!(assoc.status, ExecutableStatus::ObservedExact);
        assert_eq!(assoc.strength, EvidenceStrength::Strong);
    }

    #[test]
    fn name_matched_executable_is_only_a_candidate() {
        let a = app("Example", Some("Vendor"));
        let prober = FakeProber::default().with_files(
            "C:/Program Files/Example",
            &["C:/Program Files/Example/example.exe"],
        );
        let roots = vec![InstallRoot {
            path: PathBuf::from("C:/Program Files/Example"),
            signal: RootSignal::InstallerRecorded,
            strength: EvidenceStrength::Direct,
        }];
        let assoc = associate_executable(&a, &roots, &prober, &DiscoveryLimits::default());
        assert_eq!(assoc.status, ExecutableStatus::Candidate);
        assert_eq!(assoc.strength, EvidenceStrength::Weak);
    }

    #[test]
    fn roots_are_bounded_and_deterministic() {
        let a = app("Example", Some("Vendor"));
        let children: Vec<String> = (0..64)
            .map(|i| format!("C:/Program Files/Example{i:03}"))
            .collect();
        let prober = FakeProber::default().with_dirs(
            "C:/Program Files",
            &children.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );
        let program = ProgramRoots {
            program_files: Some(PathBuf::from("C:/Program Files")),
            ..ProgramRoots::default()
        };
        let limits = DiscoveryLimits {
            max_roots_per_app: 4,
            ..DiscoveryLimits::default()
        };
        let (roots, counts) = detect_install_roots(&a, &program, &prober, &limits);
        assert_eq!(roots.len(), 4);
        assert_eq!(counts.roots_truncated, 60);
        let (again, _) = detect_install_roots(&a, &program, &prober, &limits);
        assert_eq!(roots, again);
    }

    #[test]
    fn path_within_is_component_wise_and_byte_exact() {
        assert!(path_within(
            Path::new("C:/Program Files/App"),
            Path::new("C:/Program Files/App/bin/a.exe")
        ));
        assert!(path_within(
            Path::new("C:/Program Files/App"),
            Path::new("C:/Program Files/App")
        ));
        // A name-prefix sibling is NOT inside.
        assert!(!path_within(
            Path::new("C:/Program Files/App"),
            Path::new("C:/Program Files/Application/a.exe")
        ));
        assert!(!path_within(
            Path::new("C:/Program Files/App"),
            Path::new("C:/Program Files")
        ));
    }

    #[test]
    fn non_utf8_names_never_match_through_replacement_characters() {
        // Two DISTINCT non-UTF-8 names must not agree with each other (or
        // with anything) just because a lossy rendering would collapse both
        // to replacement characters.
        use std::ffi::OsString;
        // Two valid WTF-8 encodings of different unpaired surrogates are
        // non-UTF-8 and valid as native encoded bytes on Windows; on Unix
        // they remain arbitrary non-UTF-8 path bytes.
        let raw_a = b"C:/Program Files/\xed\xa0\x80".to_vec();
        let raw_b = b"C:/Program Files/\xed\xa0\x81".to_vec();
        let a = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw_a.to_vec()) });
        let b = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw_b.to_vec()) });
        assert!(std::str::from_utf8(&raw_a).is_err());
        assert!(std::str::from_utf8(&raw_b).is_err());
        assert_eq!(child_name_norm(&a), None);
        assert_eq!(child_name_norm(&b), None);
        // Bundle-shape matching is byte-exact on the ASCII constants.
        assert_eq!(
            bundle_root_of(Path::new("/x.app/Contents/MacOS/e")),
            Some(PathBuf::from("/x.app"))
        );
        assert_eq!(bundle_root_of(Path::new("/x/macos/contents/e")), None);
        // ... while ordinary ASCII shapes still resolve byte-exactly.
        assert_eq!(
            bundle_root_of(Path::new("/Applications/X.app/Contents/MacOS/x")),
            Some(PathBuf::from("/Applications/X.app"))
        );
        assert!(extension_is_ascii(Path::new("/a/tool.EXE"), b"exe"));
        let raw_ext = b"/a/tool.e\xed\xa0\x80".to_vec();
        let ext_path = PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(raw_ext) });
        assert!(!extension_is_ascii(&ext_path, b"exe"));
    }

    #[test]
    fn deepest_containing_root_wins() {
        let roots = vec![
            InstallRoot {
                path: PathBuf::from("C:/Program Files"),
                signal: RootSignal::NameUnderProgramRoot,
                strength: EvidenceStrength::Weak,
            },
            InstallRoot {
                path: PathBuf::from("C:/Program Files/App"),
                signal: RootSignal::InstallerRecorded,
                strength: EvidenceStrength::Direct,
            },
        ];
        let got = containing_root(&roots, Path::new("C:/Program Files/App/bin/a.exe")).unwrap();
        assert_eq!(got.path, PathBuf::from("C:/Program Files/App"));
        assert!(containing_root(&roots, Path::new("C:/Elsewhere/a.exe")).is_none());
    }
}
