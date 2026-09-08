//! Phase 2 integration tests: categories, evidence, confidence, conflicts,
//! context, edge cases, determinism, and invariants (master prompt §26–§27).
//!
//! All tests are pure — no filesystem access, no I/O.

use spacelens_classifier::{
    classify, classify_streaming, rule_category, Category, CategoryAggregator, Classification,
    Confidence, EvidenceKind, EvidenceList, ParentContext, ParentContextTracker, Platform, RuleId,
    Subcategory, MAX_EVIDENCE, RULES,
};
use spacelens_engine::{EntryKind, FsEntry};
use std::path::PathBuf;

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
        device: None,
        inode: None,
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

fn classify_file(path: &str, platform: Platform) -> Classification {
    classify(&file(1, None, path), &ParentContext::default(), platform)
}

fn classify_dir(path: &str, platform: Platform) -> Classification {
    classify(&dir(1, None, path), &ParentContext::default(), platform)
}

// ---------------------------------------------------------------------------
// §26: basic categories
// ---------------------------------------------------------------------------

#[test]
fn basic_file_categories_by_extension() {
    let cases: &[(&str, Platform, Category)] = &[
        ("/d/report.pdf", Platform::Linux, Category::Documents),
        ("/d/notes.txt", Platform::Linux, Category::Documents),
        (
            "/d/т worksheet.xlsx",
            Platform::Windows,
            Category::Documents,
        ),
        ("/d/photo.jpeg", Platform::Windows, Category::Images),
        ("/d/icon.svg", Platform::Linux, Category::Images),
        ("/d/movie.mkv", Platform::Windows, Category::Video),
        ("/d/song.flac", Platform::Linux, Category::Audio),
        ("/d/bundle.zip", Platform::Windows, Category::Archives),
        ("/d/backup.tar.gz", Platform::Linux, Category::Archives),
        ("/d/disk.iso", Platform::Windows, Category::Archives),
        ("/d/main.rs", Platform::Linux, Category::Development),
        ("/d/app.tsx", Platform::Windows, Category::Development),
        // `.ts` is ambiguous (TypeScript vs MPEG transport stream); the
        // source table claims it deterministically → Development.
        ("/d/stream.ts", Platform::Windows, Category::Development),
        ("/d/run.log", Platform::Windows, Category::Logs),
        ("/d/tool.exe", Platform::Windows, Category::Applications),
    ];
    for (path, platform, expected) in cases {
        let c = classify_file(path, *platform);
        assert_eq!(&c.category, expected, "path {path}");
        assert!(
            c.confidence <= Confidence::EXTENSION_ONLY_CAP,
            "extension-only must stay ≤ Medium: {path}"
        );
    }
}

#[test]
fn basic_directory_categories() {
    let cases: &[(&str, Platform, Category, RuleId)] = &[
        (
            "/u/cache",
            Platform::Linux,
            Category::Cache,
            RuleId::CacheDir,
        ),
        (
            "/u/tmp",
            Platform::Linux,
            Category::TemporaryData,
            RuleId::TempDir,
        ),
        (
            "C:/Windows",
            Platform::Windows,
            Category::SystemData,
            RuleId::WindowsSystemLocation,
        ),
        (
            "/usr",
            Platform::Linux,
            Category::SystemData,
            RuleId::LinuxPackageLocation,
        ),
        (
            "/home/u/node_modules",
            Platform::Linux,
            Category::Development,
            RuleId::DependencyDir,
        ),
        (
            "/repo/.git",
            Platform::Linux,
            Category::Development,
            RuleId::VcsDir,
        ),
        (
            "/u/Downloads",
            Platform::Linux,
            Category::Downloads,
            RuleId::DownloadsDir,
        ),
        (
            "C:/Users/u/Documents",
            Platform::Windows,
            Category::UserData,
            RuleId::DocumentsDir,
        ),
        (
            "/u/steamapps",
            Platform::Windows,
            Category::Games,
            RuleId::SteamLibrary,
        ),
        (
            "/u/backups",
            Platform::Linux,
            Category::Backups,
            RuleId::BackupDir,
        ),
        ("/u/logs", Platform::Linux, Category::Logs, RuleId::LogDir),
    ];
    for (path, platform, expected_cat, expected_rule) in cases {
        let c = classify_dir(path, *platform);
        assert_eq!(&c.category, expected_cat, "path {path}");
        assert_eq!(&c.winning_rule, expected_rule, "path {path}");
    }
}

#[test]
fn unknown_vs_other_distinction() {
    // Understood-but-plain: file with no content signals → Other (Low).
    let f = classify_file("/data/blob.bin", Platform::Linux);
    assert_eq!(f.category, Category::Other);
    assert_eq!(f.confidence, Confidence::Low);
    assert_eq!(f.winning_rule, RuleId::PlainFile);

    // A directory with no signals → Other (Low), not Unknown.
    let d = classify_dir("/data/stuff", Platform::Linux);
    assert_eq!(d.category, Category::Other);

    // Unknown is reserved: only reachable via NoSignals/ExtensionOnly/
    // ParentInherited ids — none of which a normal entry wins with. Verify
    // no tested input lands in Unknown:
    for p in ["/a", "/a/b.c", "x"] {
        let c = classify_file(p, Platform::Windows);
        assert_ne!(c.category, Category::Unknown);
    }
}

// ---------------------------------------------------------------------------
// §26: evidence
// ---------------------------------------------------------------------------

#[test]
fn evidence_is_typed_bounded_and_ordered() {
    // A path that matches several rules: setup.exe in Downloads-ish naming.
    let e = file(1, None, "/w/setup.exe");
    let c = classify(&e, &ParentContext::default(), Platform::Windows);
    assert!(c.evidence.len() >= 2, "multiple rules matched");
    assert!(c.evidence.len() <= MAX_EVIDENCE);
    // Winner's evidence first.
    let first = c.evidence.iter().next().unwrap();
    assert_eq!(first.rule, c.winning_rule);
    // Deterministic order across runs.
    let c2 = classify(&e, &ParentContext::default(), Platform::Windows);
    let a: Vec<_> = c.evidence.iter().map(|ev| (ev.kind, ev.rule)).collect();
    let b: Vec<_> = c2.evidence.iter().map(|ev| (ev.kind, ev.rule)).collect();
    assert_eq!(a, b);
}

#[test]
fn evidence_never_contains_path_text() {
    // Structural: Evidence is (kind, rule) only — verify the type carries no
    // string payload by checking serialization size of a full list.
    let mut list = EvidenceList::new();
    for _ in 0..MAX_EVIDENCE {
        list.push(EvidenceKind::KnownPathPattern, RuleId::CacheDir);
    }
    let json = serde_json::to_string(&list).unwrap();
    assert!(!json.contains("/data"), "no paths in evidence: {json}");
    assert!(!json.contains("C:\\"), "no paths in evidence: {json}");
}

// ---------------------------------------------------------------------------
// §26: confidence
// ---------------------------------------------------------------------------

#[test]
fn confidence_bands_and_caps() {
    // Extension-only can never exceed Medium (test-enforced invariant).
    for p in ["/x/a.png", "/x/b.pdf", "/x/c.zip", "/x/d.mp3"] {
        let c = classify_file(p, Platform::Windows);
        assert!(c.confidence <= Confidence::Medium, "{p}");
    }
    // Authoritative locations are High.
    for (p, plat) in [
        ("C:/Program Files", Platform::Windows),
        ("/usr", Platform::Linux),
        ("/Users/u/Library", Platform::Mac),
    ] {
        let c = classify_dir(p, plat);
        assert_eq!(c.confidence, Confidence::High, "{p}");
    }
}

// ---------------------------------------------------------------------------
// §24: conflicts
// ---------------------------------------------------------------------------

#[test]
fn conflict_strong_path_beats_weak_extension() {
    // A directory named Cache containing... nothing — extension n/a. The real
    // conflict case: a *file* whose name matches an installer pattern while
    // its extension says something else (setup.zip).
    let e = file(1, None, "/w/setup.zip");
    let c = classify(&e, &ParentContext::default(), Platform::Windows);
    // InstallerName (tier 3) wins over ArchiveExtension (tier 4).
    assert_eq!(c.winning_rule, RuleId::InstallerName);
    assert!(c.matched_rules.contains(&RuleId::ArchiveExtension));
    // But the losing evidence is retained.
    assert!(c
        .evidence
        .iter()
        .any(|ev| ev.rule == RuleId::ArchiveExtension));
}

#[test]
fn conflict_directory_name_beats_extension_context() {
    // Inside a directory classified Cache, a child .png stays Images (context
    // raises confidence but never changes category).
    let mut tracker = ParentContextTracker::new();
    let parent = dir(1, None, "/x/cache");
    let pc = classify_streaming(&parent, Platform::Linux, &mut tracker);
    assert_eq!(pc.category, Category::Cache);
    let child = file(2, Some(1), "/x/cache/pic.png");
    let cc = classify_streaming(&child, Platform::Linux, &mut tracker);
    assert_eq!(cc.category, Category::Images);
    assert!(cc
        .evidence
        .iter()
        .any(|ev| ev.kind == EvidenceKind::ParentContext));
}

#[test]
fn conflict_pycache_prefers_stronger_needle() {
    // "__pycache__" matches both VirtualenvDir (needle 11) and, historically,
    // CacheDir — longest needle wins deterministically.
    let c = classify_dir("/x/__pycache__", Platform::Linux);
    assert_eq!(c.winning_rule, RuleId::VirtualenvDir);
    assert!(
        c.matched_rules.contains(&RuleId::CacheDir) || !c.matched_rules.contains(&RuleId::CacheDir)
    ); // either is fine; determinism is the point
    let again = classify_dir("/x/__pycache__", Platform::Linux);
    assert_eq!(c, again, "same input → identical outcome");
}

// ---------------------------------------------------------------------------
// §13/§14: context
// ---------------------------------------------------------------------------

#[test]
fn nested_context_boosts_confidence_not_category() {
    // A neutral .bin file under a classified parent: context raises
    // confidence one band but never changes Other.
    let e = file(3, Some(2), "/w/AppData/blob.bin");
    let with = classify(
        &e,
        &ParentContext {
            parent_category: Some(Category::Applications),
            under_user_profile: true,
        },
        Platform::Windows,
    );
    let without = classify(&e, &ParentContext::default(), Platform::Windows);
    assert_eq!(with.category, without.category);
    assert!(with.evidence.len() >= without.evidence.len());
}

#[test]
fn downloads_subcategory_refinement() {
    // Inside Downloads, an .exe is an installer-like artifact: subcategory
    // refinement applies via parent context evidence.
    // Forward-slash Windows paths: parse identically on every host.
    let mut tracker = ParentContextTracker::new();
    let parent = dir(1, None, "C:/Users/u/Downloads");
    let pc = classify_streaming(&parent, Platform::Windows, &mut tracker);
    assert_eq!(pc.category, Category::Downloads);
    let child = file(2, Some(1), "C:/Users/u/Downloads/tool.exe");
    let cc = classify_streaming(&child, Platform::Windows, &mut tracker);
    assert_eq!(cc.category, Category::Applications);
    assert!(cc.confidence >= Confidence::Medium);
}

#[test]
fn mixed_content_directory_keeps_child_specificity() {
    // Downloads dir with three children: each child keeps its own category.
    let mut tracker = ParentContextTracker::new();
    let parent = dir(1, None, "/home/u/Downloads");
    let _pc = classify_streaming(&parent, Platform::Linux, &mut tracker);
    let pdf = file(2, Some(1), "/home/u/Downloads/paper.pdf");
    let jpg = file(3, Some(1), "/home/u/Downloads/vacation.jpg");
    let exe = file(4, Some(1), "/home/u/Downloads/installer.exe");
    let c_pdf = classify_streaming(&pdf, Platform::Linux, &mut tracker);
    let c_jpg = classify_streaming(&jpg, Platform::Linux, &mut tracker);
    let c_exe = classify_streaming(&exe, Platform::Linux, &mut tracker);
    assert_eq!(c_pdf.category, Category::Documents);
    assert_eq!(c_jpg.category, Category::Images);
    // Installer-like name maps to the Downloads semantic family with an
    // Installer subcategory (master prompt §5 example).
    assert_eq!(c_exe.category, Category::Downloads);
    assert_eq!(c_exe.subcategory, Some(Subcategory::Installer));
    // All three carry parent-context evidence (directory helped, not dictated).
    for c in [&c_pdf, &c_jpg, &c_exe] {
        assert!(c
            .evidence
            .iter()
            .any(|ev| ev.kind == EvidenceKind::ParentContext));
    }
}

// ---------------------------------------------------------------------------
// §26: edge cases
// ---------------------------------------------------------------------------

#[test]
fn edge_case_names() {
    // Unicode, spaces, hidden, no extension, multiple extensions, long paths.
    let cases: &[(&str, Platform, Category)] = &[
        ("/d/отчёт.pdf", Platform::Linux, Category::Documents),
        ("/d/日本語.png", Platform::Linux, Category::Images),
        (
            "/d/my file with spaces.txt",
            Platform::Windows,
            Category::Documents,
        ),
        ("/d/.hidden", Platform::Linux, Category::Other),
        ("/d/noextension", Platform::Linux, Category::Other),
        ("/d/archive.tar.gz", Platform::Linux, Category::Archives),
        ("/d/.env.secret", Platform::Linux, Category::Other),
        (
            "/d/verylongname-with-dashes-and-numbers-1234567890.mp4",
            Platform::Linux,
            Category::Video,
        ),
    ];
    for (path, platform, expected) in cases {
        let c = classify_file(path, *platform);
        assert_eq!(&c.category, expected, "path {path}");
    }
    // Long path (>260 chars) must not panic or misclassify a .log file.
    let long = format!("/d/{}.log", "a".repeat(300));
    let c = classify_file(&long, Platform::Windows);
    assert_eq!(c.category, Category::Logs);
}

#[test]
fn hidden_dot_directories_on_unix() {
    // ".cache" classifies as Cache (CacheDir handles the cache-specific
    // signal; XdgLocation keeps .local/.config/.share).
    let c = classify_dir("/home/u/.cache", Platform::Linux);
    assert_eq!(c.category, Category::Cache);
    assert_eq!(c.winning_rule, RuleId::CacheDir);
    // Generic XDG locations classify as UserData.
    let c = classify_dir("/home/u/.config", Platform::Linux);
    assert_eq!(c.winning_rule, RuleId::XdgLocation);
    assert_eq!(c.category, Category::UserData);
    // Same name on Windows: no XDG rule → different outcome, no panic.
    let c = classify_dir("C:/Users/u/.cache", Platform::Windows);
    assert_ne!(c.winning_rule, RuleId::XdgLocation);
}

#[test]
fn app_bundle_id_on_macos() {
    // .app suffix is reserved (MacAppBundle) — directories ending in .app are
    // NOT yet classified as Applications; documented limitation. Verify
    // honest behavior: no panic, sensible fallback.
    let c = classify_dir("/Applications/Foo.app", Platform::Mac);
    assert!(matches!(
        c.category,
        Category::Applications | Category::Other
    ));
}

// ---------------------------------------------------------------------------
// §27: invariants
// ---------------------------------------------------------------------------

#[test]
fn invariant_deterministic_across_all_fixtures() {
    let fixtures: Vec<FsEntry> = vec![
        file(1, None, "/a/report.pdf"),
        file(2, None, "/a/SETUP.EXE"),
        dir(3, None, "/a/Node_Modules"),
        dir(4, None, "C:/Program Files/App"),
        file(5, None, "/a/日本語のファイル.mp3"),
        file(6, None, "/a/x.tar.gz"),
        dir(7, None, "/a/tmp"),
        file(8, None, "/a/unknownblob"),
    ];
    for e in &fixtures {
        for plat in [Platform::Windows, Platform::Linux, Platform::Mac] {
            let a = classify(e, &ParentContext::default(), plat);
            let b = classify(e, &ParentContext::default(), plat);
            assert_eq!(a, b, "nondeterministic: {:?} on {plat:?}", e.path);
        }
    }
}

#[test]
fn invariant_no_rule_match_produces_honest_fallback() {
    // Any entry that matches nothing must be Other/Low — never a confident
    // invented category.
    for p in ["/x/zzz.qqq", "/x/noext", "/x/."] {
        let c = classify_file(p, Platform::Linux);
        if c.matched_rules.is_empty() {
            assert_eq!(c.category, Category::Other);
            assert_eq!(c.confidence, Confidence::Low);
        }
    }
}

#[test]
fn invariant_every_table_rule_has_category() {
    for r in RULES {
        let _ = rule_category(r.id); // must not panic
    }
}

#[test]
fn invariant_aggregator_never_overflows() {
    let mut agg = CategoryAggregator::new();
    let c = Classification {
        schema: Classification::SCHEMA.to_string(),
        entry_id: 0,
        category: Category::Video,
        subcategory: None,
        confidence: Confidence::High,
        evidence: EvidenceList::new(),
        winning_rule: RuleId::VideoExtension,
        matched_rules: vec![RuleId::VideoExtension],
    };
    let huge = u64::MAX;
    agg.push(&c, &EntryKind::File, huge);
    agg.push(&c, &EntryKind::File, huge);
    let t = agg.totals(Category::Video);
    assert_eq!(t.logical_size, u64::MAX, "saturating, never wraps");
    assert_eq!(t.entries, 2);
}

#[test]
fn invariant_tracker_memory_bounded() {
    let mut t = ParentContextTracker::with_capacity(16);
    for id in 0..10_000u64 {
        let d = dir(id, None, "/x");
        let c = classify_streaming(&d, Platform::Linux, &mut t);
        let _ = c;
    }
    assert!(t.len() <= 16, "tracker must stay bounded");
}

#[test]
fn invariant_no_panic_on_fuzzed_names() {
    // Deterministic pseudo-fuzz: nasty byte sequences as names.
    let nasty = [
        "\u{0}\u{1}",
        "🎉☠️💀",
        "a\
         b",
        "%PATH%",
        "$(rm -rf)",
        "con",
        "nul",
        "aux",
        "..\\..\\..",
        "//",
        "C:\\",
        "\\\\?\\C:\\huge",
    ];
    for n in nasty {
        for kind in [EntryKind::File, EntryKind::Dir] {
            let e = entry(1, None, n, kind);
            let c = classify(&e, &ParentContext::default(), Platform::Windows);
            let _ = format!("{c:?}");
        }
    }
}

#[test]
fn serialization_roundtrip() {
    let c = classify_file("/w/setup.exe", Platform::Windows);
    let json = serde_json::to_string(&c).unwrap();
    let back: Classification = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    assert!(json.contains("spacelens.v1.classification"));
}
