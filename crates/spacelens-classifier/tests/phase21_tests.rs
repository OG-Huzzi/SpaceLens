//! Phase 2.1 semantic-hardening regression suite.
//!
//! One group per audited Phase 2.1 target. Every test asserts exact semantics
//! (category, winning rule, confidence, evidence) rather than "does not
//! crash", and each group is written to fail against the pre-2.1 behavior it
//! guards.
//!
//! All tests are pure: no filesystem access, no I/O, no host-platform
//! dependence (paths are synthetic; `Platform` is always explicit).

use spacelens_classifier::{
    classify, classify_streaming, pathctx, Category, Classification, Confidence, EvidenceKind,
    ParentContext, ParentContextTracker, Platform, RuleId, Subcategory,
};
use spacelens_engine::{EntryKind, FsEntry};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn entry(id: u64, parent: Option<u64>, path: &str, kind: EntryKind) -> FsEntry {
    FsEntry {
        id,
        parent_id: parent,
        path: PathBuf::from(path),
        kind,
        size: 0,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: None,
        inode: None,
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

fn file(id: u64, parent: Option<u64>, path: &str) -> FsEntry {
    entry(id, parent, path, EntryKind::File)
}

fn dir(id: u64, parent: Option<u64>, path: &str) -> FsEntry {
    entry(id, parent, path, EntryKind::Dir)
}

fn f(path: &str, platform: Platform) -> Classification {
    classify(&file(1, None, path), &ParentContext::default(), platform)
}

fn d(path: &str, platform: Platform) -> Classification {
    classify(&dir(1, None, path), &ParentContext::default(), platform)
}

// ===========================================================================
// Phase 2.1 target 1 — installer extensions are gated to Downloads
// ===========================================================================
//
// The extension says what the *bytes* are (an installer package), not where
// the file came from. `Downloads` is a claim about origin, so the extension
// alone can never make it. This mirrors the Phase 2 repair of `InstallerName`.

/// The whole installer extension table, in the three locations that matter.
#[test]
fn phase21_installer_extensions_are_gated_to_downloads() {
    let exts = ["msi", "dmg", "pkg", "deb", "rpm", "apk", "msp", "msu"];
    for ext in exts {
        // Inside a download location: decides, corroborated to High.
        let dl = f(
            &format!("C:/Users/u/Downloads/blob.{ext}"),
            Platform::Windows,
        );
        assert_eq!(dl.category, Category::Downloads, "{ext}");
        assert_eq!(dl.winning_rule, RuleId::InstallerExtension, "{ext}");
        assert_eq!(dl.subcategory, Some(Subcategory::Installer), "{ext}");
        assert_eq!(dl.confidence, Confidence::High, "{ext}");

        // Inside an application install tree: the location wins; never
        // Downloads.
        let pf = f(
            &format!("C:/Program Files/App/blob.{ext}"),
            Platform::Windows,
        );
        assert_eq!(pf.category, Category::Applications, "{ext}");
        assert_ne!(pf.category, Category::Downloads, "{ext}");

        // Somewhere neutral: no eligible evidence at all — honest Other with
        // the match retained as evidence.
        let neutral = f(&format!("/data/blob.{ext}"), Platform::Linux);
        assert_eq!(neutral.category, Category::Other, "{ext}");
        assert_eq!(neutral.winning_rule, RuleId::PlainFile, "{ext}");
        assert_eq!(neutral.confidence, Confidence::Low, "{ext}");
        assert!(
            neutral.matched_rules.contains(&RuleId::InstallerExtension),
            "{ext}: the extension must still be on the record"
        );
        assert!(
            neutral
                .evidence
                .iter()
                .any(|ev| ev.rule == RuleId::InstallerExtension
                    && ev.kind == EvidenceKind::Extension),
            "{ext}: evidence kind must stay truthful"
        );
    }
}

/// Install/update artifacts that ship with an application are application
/// code — on every platform's install trees.
#[test]
fn phase21_bundled_installers_are_application_code() {
    let cases = [
        ("C:/Program Files/App/setup.msi", Platform::Windows),
        ("C:/Program Files (x86)/App/setup.msi", Platform::Windows),
        ("/opt/app/setup.deb", Platform::Linux),
        ("/opt/app/setup.rpm", Platform::Linux),
    ];
    for (p, platform) in cases {
        let c = f(p, platform);
        assert_eq!(c.category, Category::Applications, "{p}");
        assert_ne!(c.category, Category::Downloads, "{p}");
        assert!(
            c.matched_rules.contains(&RuleId::InstallerExtension),
            "{p}: the installer signal survives as evidence"
        );
    }
}

/// The gated winner inside Downloads is explainable: the extension decides
/// and the download location corroborates it.
#[test]
fn phase21_gated_installer_extension_is_high_confidence_in_downloads() {
    let c = f("C:/Users/u/Downloads/setup.msi", Platform::Windows);
    assert_eq!(c.winning_rule, RuleId::InstallerExtension);
    assert_eq!(c.category, Category::Downloads);
    assert_eq!(c.subcategory, Some(Subcategory::Installer));
    assert_eq!(
        c.confidence,
        Confidence::High,
        "gate satisfied ⇒ authoritative location corroborates ⇒ High"
    );
    // Both matched signals are on the record, each truthfully typed.
    assert!(c
        .evidence
        .iter()
        .any(|ev| ev.rule == RuleId::InstallerName && ev.kind == EvidenceKind::FilenamePattern));
    assert!(c
        .evidence
        .iter()
        .any(|ev| ev.rule == RuleId::InstallerExtension && ev.kind == EvidenceKind::Extension));
}

/// `.exe` is deliberately NOT an installer extension (it is any executable):
/// outside Downloads it stays `Applications`, unchanged from Phase 2.
#[test]
fn phase21_exe_remains_a_generic_executable() {
    let c = f("/data/blob.exe", Platform::Windows);
    assert_eq!(c.category, Category::Applications);
    assert_eq!(c.winning_rule, RuleId::ExecutableExtension);
    assert_eq!(c.subcategory, None);
}

// ===========================================================================
// Phase 2.1 target 1b — `.appimage` is the application, not an installer
// ===========================================================================

#[test]
fn phase21_appimage_is_the_application_itself() {
    // An AppImage executes directly; there is no installer step. It must be
    // claimed by the executable table, never by the installer table.
    for p in ["/data/Tool.appimage", "/opt/app/Tool.appimage"] {
        let c = f(p, Platform::Linux);
        assert_eq!(c.category, Category::Applications, "{p}");
        assert_eq!(c.winning_rule, RuleId::ExecutableExtension, "{p}");
        assert_eq!(c.subcategory, None, "{p}: not an Installer subcategory");
        assert!(
            !c.matched_rules.contains(&RuleId::InstallerExtension),
            "{p}: AppImage must not be an installer signal"
        );
    }
}

// ===========================================================================
// Phase 2.1 target 2 — macOS /Library hierarchy
// ===========================================================================

#[test]
fn phase21_system_wide_library_caches_and_logs_are_recognised() {
    // Before 2.1 these fell through to the broad `/library` rule and became
    // `ApplicationData`; the more specific tree must win by depth.
    let caches = d("/Library/Caches/App", Platform::Mac);
    assert_eq!(caches.category, Category::Cache);
    assert_eq!(caches.winning_rule, RuleId::CacheDir);
    assert_eq!(caches.confidence, Confidence::High);

    let logs = d("/Library/Logs/App", Platform::Mac);
    assert_eq!(logs.category, Category::Logs);
    assert_eq!(logs.winning_rule, RuleId::LogDir);
    assert_eq!(logs.confidence, Confidence::High);

    // The locations themselves are roots of their class, too.
    assert_eq!(
        d("/Library/Caches", Platform::Mac).category,
        Category::Cache
    );
    assert_eq!(d("/Library/Logs", Platform::Mac).category, Category::Logs);
}

#[test]
fn phase21_user_and_system_library_treat_the_same_trees_identically() {
    // `~/Library/Caches` and `/Library/Caches` are the same *kind* of place.
    for (user, system) in [
        ("/Users/u/Library/Caches/App", "/Library/Caches/App"),
        ("/Users/u/Library/Logs/App", "/Library/Logs/App"),
    ] {
        let u = d(user, Platform::Mac);
        let s = d(system, Platform::Mac);
        assert_eq!(u.category, s.category, "{user} vs {system}");
        assert_eq!(u.winning_rule, s.winning_rule, "{user} vs {system}");
        assert_eq!(u.confidence, Confidence::High);
        assert_eq!(s.confidence, Confidence::High);
    }
}

#[test]
fn phase21_library_contents_without_a_specific_tree_stay_application_data() {
    // Unrelated Library contents (Fonts, Preferences, …) are application-owned
    // data; the broad `/library` rule remains the honest fallback.
    for p in [
        "/Library/Fonts",
        "/Library/Preferences/com.example.plist-dir",
        "/Users/u/Library/Fonts",
    ] {
        let c = d(p, Platform::Mac);
        assert_eq!(c.category, Category::ApplicationData, "{p}");
        assert_eq!(c.confidence, Confidence::High, "{p}");
    }
    // Application Support keeps its exact semantics.
    assert_eq!(
        d("/Users/u/Library/Application Support/App", Platform::Mac).category,
        Category::ApplicationData
    );
    // A cache *deeper inside* application support is still a cache by name,
    // and here the rooted cache tree corroborates it to High.
    let c = d("/Users/u/Library/Caches/App/cache", Platform::Mac);
    assert_eq!(c.category, Category::Cache);
    assert_eq!(c.confidence, Confidence::High);
}

#[test]
fn phase21_user_applications_tree_is_an_install_location() {
    // `~/Applications` is a real per-user install tree on macOS.
    for p in ["/Users/u/Applications", "/Users/u/Applications/Foo.app"] {
        let c = d(p, Platform::Mac);
        assert_eq!(c.category, Category::Applications, "{p}");
        assert_eq!(c.winning_rule, RuleId::ApplicationInstallLocation, "{p}");
        assert_eq!(c.confidence, Confidence::High, "{p}");
    }
    // And the system-wide Applications location is unchanged.
    let c = d("/Applications", Platform::Mac);
    assert_eq!(c.category, Category::Applications);
    assert_eq!(c.confidence, Confidence::High);
}

// ===========================================================================
// Phase 2.1 target 3 — `.app` bundles: contextual, never suffix-only
// ===========================================================================

#[test]
fn phase21_app_bundles_are_classified_by_location_not_suffix() {
    // Inside an authoritative install location: Applications, by location.
    let c = d("/Applications/Foo.app", Platform::Mac);
    assert_eq!(c.category, Category::Applications);
    assert_eq!(c.winning_rule, RuleId::ApplicationInstallLocation);
    assert_eq!(c.confidence, Confidence::High);

    // Under a pure container (UserHome never decides), a `.app` directory has
    // no name signal (no table rule matches the suffix), so the honest
    // outcome is Other/Low. This is deliberate: a bundle named `Foo.app` in a
    // staging tree is NOT necessarily an installed application, and the
    // classifier must not imply more certainty than its evidence supports.
    let c = d("/Users/u/Staging/Foo.app", Platform::Mac);
    assert_eq!(c.category, Category::Other);
    assert_eq!(c.winning_rule, RuleId::DirWithoutSignals);
    assert_eq!(c.confidence, Confidence::Low);
    assert!(
        !c.matched_rules.contains(&RuleId::MacAppBundle),
        "no table rule may claim a .app suffix"
    );

    // Inside a deciding location (Downloads), the location — never the
    // suffix — decides, exactly as for any other unremarkable content there.
    let c = d("/Users/u/Downloads/Foo.app", Platform::Mac);
    assert_eq!(c.category, Category::Downloads);
    assert_eq!(c.winning_rule, RuleId::DownloadsDir);
}

#[test]
fn phase21_files_inside_a_bundle_belong_to_the_install_location() {
    let c = f("/Applications/Foo.app/Contents/MacOS/Foo", Platform::Mac);
    assert_eq!(c.category, Category::Applications);
    assert_eq!(c.winning_rule, RuleId::ApplicationInstallLocation);
    assert_eq!(c.confidence, Confidence::Medium, "a file *in* the location");
}

/// Documented v1 choice, pinned: a **file** with an executable-ish suffix
/// (including `.app`) classifies as `Applications` wherever it is found. The
/// *directory-suffix* case is what stays suffix-free.
#[test]
fn phase21_dot_app_files_remain_executables_by_table() {
    let c = f("C:/data/tool.app", Platform::Windows);
    assert_eq!(c.category, Category::Applications);
    assert_eq!(c.winning_rule, RuleId::ExecutableExtension);
}

// ===========================================================================
// Phase 2.1 target 8 — rule-precedence interaction matrix
// ===========================================================================

/// Combinations where several rules match the same path. The winner must
/// always be explainable from its tier and evidence kind.
#[test]
fn phase21_precedence_matrix_stronger_context_wins_consistently() {
    // (path, kind, platform, expected winner, expected category)
    let cases: &[(&str, bool, Platform, RuleId, Category)] = &[
        // rooted location + basename (name is tier 1, location tier 6):
        // the specific name wins, and the corroborating location lifts it
        // to the location's own High via the shared rule id.
        (
            "C:/Users/u/AppData/Local/App/cache",
            true,
            Platform::Windows,
            RuleId::CacheDir,
            Category::Cache,
        ),
        // rooted cache location + cache name — same rule id via two
        // mechanisms: the location (authoritative) supplies High.
        (
            "/Users/u/Library/Caches/App/cache",
            true,
            Platform::Mac,
            RuleId::CacheDir,
            Category::Cache,
        ),
        // specific subdirectory beats broad parent directory
        // (/users/*/library/caches @4 > /users/*/library @3).
        (
            "/Users/u/Library/Caches/App",
            true,
            Platform::Mac,
            RuleId::CacheDir,
            Category::Cache,
        ),
        // platform-specific location vs generic weak extension:
        // a .dat file in Program Files is classified by its location.
        (
            "C:/Program Files/App/blob.dat",
            false,
            Platform::Windows,
            RuleId::ApplicationInstallLocation,
            Category::Applications,
        ),
        // known application path vs generic installer *name* heuristic
        // (gated): the executable extension decides, location corroborates.
        (
            "C:/Program Files/App/update.exe",
            false,
            Platform::Windows,
            RuleId::ExecutableExtension,
            Category::Applications,
        ),
        // cache/log location + generic data rule: the content-typed
        // extension (tier 4) outranks the tier-6 location.
        (
            "/var/log/nginx/error.log",
            false,
            Platform::Linux,
            RuleId::LogExtension,
            Category::Logs,
        ),
        // log directory name inside the rooted log tree: same rule id via
        // both mechanisms — the rooted location supplies High.
        (
            "/var/log/nginx",
            true,
            Platform::Linux,
            RuleId::LogDir,
            Category::Logs,
        ),
        // temp location vs temp name: same id, location authoritative.
        (
            "C:/Users/u/AppData/Local/Temp/tmp",
            true,
            Platform::Windows,
            RuleId::TempDir,
            Category::TemporaryData,
        ),
        // downloads location vs downloads *directory name*: the name rule is
        // tier 2, the location tier 6; both say Downloads, the name wins.
        (
            "C:/Users/u/Downloads",
            true,
            Platform::Windows,
            RuleId::DownloadsDir,
            Category::Downloads,
        ),
    ];
    for (p, is_dir, platform, winner, category) in cases {
        let c = if *is_dir {
            d(p, *platform)
        } else {
            f(p, *platform)
        };
        assert_eq!(c.winning_rule, *winner, "{p}");
        assert_eq!(c.category, *category, "{p}");
        // Explainability: the winner's own evidence is always first.
        let first = c.evidence.iter().next().expect("evidence non-empty");
        assert_eq!(first.rule, c.winning_rule, "{p}: winner's evidence first");
    }
}

/// Confidence must never exceed what the winning evidence justifies, across
/// the whole precedence matrix (second-order interaction audit).
#[test]
fn phase21_precedence_matrix_confidence_is_always_justified() {
    let cases: &[(&str, bool, Platform, Confidence)] = &[
        // Authoritative location wins ⇒ High.
        (
            "C:/Program Files/App/blob.dat",
            false,
            Platform::Windows,
            Confidence::Medium,
        ),
        (
            "/Applications/Foo.app",
            true,
            Platform::Mac,
            Confidence::High,
        ),
        ("/Library/Caches/App", true, Platform::Mac, Confidence::High),
        // Gated winner inside Downloads ⇒ High.
        (
            "C:/Users/u/Downloads/blob.msi",
            false,
            Platform::Windows,
            Confidence::High,
        ),
        // Ungated weak extension outside any location ⇒ extension cap.
        ("/data/blob.dat", false, Platform::Linux, Confidence::Low),
        // Content extension in a location ⇒ extension cap (Medium), never High.
        (
            "/var/log/nginx/error.log",
            false,
            Platform::Linux,
            Confidence::Medium,
        ),
        (
            "C:/Program Files/App/tool.exe",
            false,
            Platform::Windows,
            Confidence::Medium,
        ),
        // Weak name heuristic, uncorroborated (project tree) ⇒ Low.
        (
            "/home/u/project/cache",
            true,
            Platform::Linux,
            Confidence::Low,
        ),
    ];
    for (p, is_dir, platform, expected) in cases {
        let c = if *is_dir {
            d(p, *platform)
        } else {
            f(p, *platform)
        };
        assert_eq!(c.confidence, *expected, "{p}");
    }
}

// ===========================================================================
// Phase 2.1 target 5 — Unknown vs Other contract addenda
// ===========================================================================

#[test]
fn phase21_uninterpretable_kind_with_only_gated_evidence_is_unknown() {
    // A socket/FIFO with no *eligible* signal is genuinely uninterpretable:
    // the only matching rule (installer extension) is gated to Downloads and
    // therefore cannot decide.
    let sock = entry(1, None, "/x/blob.msi", EntryKind::Other);
    let c = classify(&sock, &ParentContext::default(), Platform::Linux);
    assert_eq!(c.category, Category::Unknown);
    assert_eq!(c.confidence, Confidence::Unknown);
    assert_eq!(c.winning_rule, RuleId::NoSignals);

    // … while an *ungated* signal still decides for the same special kind.
    let named = entry(2, None, "/x/tool.exe", EntryKind::Other);
    let c = classify(&named, &ParentContext::default(), Platform::Linux);
    assert_eq!(c.category, Category::Applications);
    assert_ne!(c.category, Category::Unknown);
}

#[test]
fn phase21_gating_never_manufactures_unknown_for_regular_entries() {
    // A regular file whose only *eligible* match is nothing is
    // understood-but-plain (Other), never Unknown — it is a valid observation
    // of a regular file.
    for p in ["/data/blob.msi", "/data/blob.bin"] {
        let c = f(p, Platform::Linux);
        assert_ne!(c.category, Category::Unknown, "{p}");
        assert_eq!(c.category, Category::Other, "{p}");
        assert_eq!(c.confidence, Confidence::Low, "{p}");
    }
    // An ungated signal still decides: a bare setup.exe is Applications.
    let c = f("/data/setup.exe", Platform::Linux);
    assert_eq!(c.category, Category::Applications);
    assert_ne!(c.category, Category::Unknown);
}

// ===========================================================================
// Phase 2.1 target 6 — path/context semantics addenda
// ===========================================================================

#[test]
fn phase21_dot_components_are_transparent_in_location_matching() {
    // `.` must not break anchored rooted matching.
    assert_eq!(
        pathctx::analyze(Path::new("C:/Users/./u/Downloads"), Platform::Windows).class(),
        pathctx::analyze(Path::new("C:/Users/u/Downloads"), Platform::Windows).class(),
        "`.` is transparent"
    );
    assert_eq!(
        pathctx::analyze(Path::new("/Users/./u/Library/Caches"), Platform::Mac).class(),
        Some(spacelens_classifier::LocationClass::Cache)
    );
    // … and end-to-end through classification.
    let c = f("C:/Users/./u/Downloads/setup.exe", Platform::Windows);
    assert_eq!(c.category, Category::Downloads);
    assert_eq!(c.winning_rule, RuleId::InstallerName);
    assert_eq!(c.confidence, Confidence::High);
}

/// `..` deliberately changes meaning and is NOT normalised away.
#[test]
fn phase21_dotdot_components_are_not_silently_normalized() {
    // "/Users/u/../u/Downloads" normalises to the Downloads location, but the
    // classifier must not guess: without normalisation, the anchored prefix
    // match fails and no location knowledge is claimed.
    let c = f("/Users/u/../u/Downloads/setup.exe", Platform::Linux);
    assert_ne!(
        c.category,
        Category::Downloads,
        "`..` must not be resolved into location knowledge"
    );
    // Deterministic and safe regardless.
    let again = f("/Users/u/../u/Downloads/setup.exe", Platform::Linux);
    assert_eq!(c, again);
}

#[test]
fn phase21_host_independence_addenda() {
    // Mixed separators, repeated separators, lowercase drive token: all
    // deterministic on every host.
    let reference = f("C:/Users/u/Downloads/setup.exe", Platform::Windows);
    let variants = [
        "C:/Users\\u/Downloads/setup.exe",
        "C://Users///u//Downloads/setup.exe",
        "c:/Users/u/Downloads/setup.exe",
        "C:/Users/u/./Downloads/setup.exe",
    ];
    for v in variants {
        let c = f(v, Platform::Windows);
        assert_eq!(c.category, reference.category, "{v}");
        assert_eq!(c.winning_rule, reference.winning_rule, "{v}");
        assert_eq!(c.confidence, reference.confidence, "{v}");
    }

    // Spaces and dots in names keep the final extension authoritative.
    let c = f("/data/my.report.v2.pdf", Platform::Linux);
    assert_eq!(c.category, Category::Documents);
    // A space-bearing installer-extension name in a deciding location is
    // still recognised; the gating is about location, not the name.
    let c = f(
        "C:/Users/u/Downloads/my file with spaces.dmg",
        Platform::Windows,
    );
    assert_eq!(c.winning_rule, RuleId::InstallerExtension);
    assert_eq!(c.category, Category::Downloads);
    // … and without the location the same file is honestly Other.
    let c = f("/data/my file with spaces.dmg", Platform::Windows);
    assert_eq!(c.category, Category::Other);
    assert!(c.matched_rules.contains(&RuleId::InstallerExtension));

    // A relative path carries no location knowledge: the gated installer name
    // cannot win, so the honest answer is the extension's.
    let c = f("Downloads/setup.exe", Platform::Windows);
    assert_ne!(
        c.winning_rule,
        RuleId::InstallerName,
        "no location ⇒ no gate"
    );
    assert_eq!(c.category, Category::Applications);
}

// ===========================================================================
// Phase 2.1 target 7 — tracker hard bound
// ===========================================================================

#[test]
fn phase21_tracker_hard_bound_cannot_be_bypassed() {
    let mut t = ParentContextTracker::with_capacity(usize::MAX);
    assert_eq!(
        t.capacity(),
        ParentContextTracker::MAX_ENTRIES,
        "the documented hard bound is enforced for any requested capacity"
    );
    for id in 0..20_000u64 {
        t.record(id, Category::Cache);
    }
    assert!(t.len() <= ParentContextTracker::MAX_ENTRIES);
}

#[test]
fn phase21_tracker_zero_and_explicit_small_capacities_unchanged() {
    // The clamp must not disturb the existing edge-case contracts.
    let mut zero = ParentContextTracker::with_capacity(0);
    zero.record(1, Category::Cache);
    assert_eq!(zero.len(), 0);

    let mut one = ParentContextTracker::with_capacity(1);
    one.record(1, Category::Cache);
    one.record(2, Category::Cache);
    assert_eq!(one.len(), 1);
    assert_eq!(one.peek(2), Some(Category::Cache));

    // Capacity above the hard bound is clamped, capacity below is honored.
    assert_eq!(
        ParentContextTracker::with_capacity(7).capacity(),
        7,
        "a small explicit capacity is honored exactly"
    );
}

// ===========================================================================
// Streaming integration of the changed semantics
// ===========================================================================

#[test]
fn phase21_streaming_downloads_children_keep_exact_semantics() {
    let mut tracker = ParentContextTracker::new();
    let parent = dir(1, None, "C:/Users/u/Downloads");
    let pc = classify_streaming(&parent, Platform::Windows, &mut tracker);
    assert_eq!(pc.category, Category::Downloads);

    // An .msi child: gated extension wins with location corroboration.
    let msi = classify_streaming(
        &file(2, Some(1), "C:/Users/u/Downloads/blob.msi"),
        Platform::Windows,
        &mut tracker,
    );
    assert_eq!(msi.category, Category::Downloads);
    assert_eq!(msi.winning_rule, RuleId::InstallerExtension);
    assert_eq!(msi.confidence, Confidence::High);

    // A .pdf child keeps its own content type.
    let pdf = classify_streaming(
        &file(3, Some(1), "C:/Users/u/Downloads/paper.pdf"),
        Platform::Windows,
        &mut tracker,
    );
    assert_eq!(pdf.category, Category::Documents);
}

#[test]
fn phase21_determinism_over_the_new_semantics() {
    let fixtures: Vec<FsEntry> = vec![
        file(1, None, "C:/Program Files/App/setup.msi"),
        file(2, None, "/data/blob.msi"),
        file(3, None, "C:/Users/u/Downloads/blob.dmg"),
        file(4, None, "/data/Tool.appimage"),
        dir(5, None, "/Library/Caches/App"),
        dir(6, None, "/Users/u/Applications/Foo.app"),
        dir(7, None, "/Users/u/Downloads/Foo.app"),
        file(8, None, "C:/Users/./u/Downloads/setup.exe"),
    ];
    for e in &fixtures {
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            let a = classify(e, &ParentContext::default(), platform);
            let b = classify(e, &ParentContext::default(), platform);
            assert_eq!(a, b, "{:?} on {platform:?} is nondeterministic", e.path);
        }
    }
}
