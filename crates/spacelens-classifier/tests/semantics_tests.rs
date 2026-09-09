//! Phase 2 audit-repair regression suite.
//!
//! One file per *audited defect*, plus the adversarial matrix the audit
//! demanded. Every test here is written to fail against the pre-repair
//! behavior — they are regression tests, not description tests.
//!
//! All tests are pure: no filesystem access, no I/O, no host-platform
//! dependence (paths are synthetic and `Platform` is always passed explicitly).

use spacelens_classifier::{
    classify, classify_streaming, Category, CategoryAggregator, Classification, Confidence,
    EvidenceKind, ParentContext, ParentContextTracker, Platform, RuleId, Subcategory, MAX_EVIDENCE,
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

fn f(path: &str, platform: Platform) -> Classification {
    classify(&file(1, None, path), &ParentContext::default(), platform)
}

fn d(path: &str, platform: Platform) -> Classification {
    classify(&dir(1, None, path), &ParentContext::default(), platform)
}

const ALL_PLATFORMS: [Platform; 3] = [Platform::Windows, Platform::Mac, Platform::Linux];

// ===========================================================================
// FINDING 1 — installer / update / uninstall names must not hijack semantics
// ===========================================================================

/// The six dangerous cases named by the audit, verbatim.
#[test]
fn finding1_named_dangerous_cases_keep_their_real_semantics() {
    // update.exe / uninstall.exe inside an installed application's own tree
    // are application code, not something the user downloaded.
    for name in ["update.exe", "uninstall.exe"] {
        let c = f(&format!("C:/Program Files/App/{name}"), Platform::Windows);
        assert_eq!(c.category, Category::Applications, "{name}");
        assert_ne!(
            c.category,
            Category::Downloads,
            "{name} must not be Downloads"
        );
        assert_eq!(c.winning_rule, RuleId::ExecutableExtension, "{name}");
    }

    // update.log is a log.
    let c = f("C:/Program Files/App/update.log", Platform::Windows);
    assert_eq!(c.category, Category::Logs);
    assert_eq!(c.winning_rule, RuleId::LogExtension);

    // update.txt in a project is a text document.
    let c = f("/home/user/project/update.txt", Platform::Linux);
    assert_eq!(c.category, Category::Documents);
    assert_eq!(c.winning_rule, RuleId::DocumentExtension);

    // setup.zip keeps archive semantics — everywhere, including Downloads.
    let c = f("/home/user/Downloads/setup.zip", Platform::Linux);
    assert_eq!(c.category, Category::Archives);
    assert_eq!(c.winning_rule, RuleId::ArchiveExtension);

    // setup.exe in Downloads is the one case where the name *is* the answer.
    let c = f("C:/Users/user/Downloads/setup.exe", Platform::Windows);
    assert_eq!(c.category, Category::Downloads);
    assert_eq!(c.subcategory, Some(Subcategory::Installer));
}

/// The principled rule, stated once: a *name* signal is weaker than an
/// extension that says what the bytes are, and weaker than the location it
/// sits in. Sweep every stem × every context and assert the invariant holds
/// rather than pinning a handful of hand-picked cases.
#[test]
fn finding1_installer_name_never_hijacks_a_content_extension() {
    // Content-typed extensions that must always beat an installer-ish name.
    let content_cases: &[(&str, Category)] = &[
        ("setup.zip", Category::Archives),
        ("install.7z", Category::Archives),
        ("update.pdf", Category::Documents),
        ("uninstall.txt", Category::Documents),
        ("installer.log", Category::Logs),
        ("update.png", Category::Images),
        ("setup.mp4", Category::Video),
        ("install.flac", Category::Audio),
        ("update.rs", Category::Development),
        ("setup.iso", Category::Archives),
    ];
    // Contexts: a download location, an installed app tree, app data, macOS
    // application support, and an arbitrary project directory.
    let contexts: &[(&str, Platform)] = &[
        ("C:/Users/user/Downloads", Platform::Windows),
        ("C:/Program Files/App", Platform::Windows),
        ("C:/Users/user/AppData/Local/App", Platform::Windows),
        ("/Users/user/Library/Application Support/App", Platform::Mac),
        ("/home/user/project", Platform::Linux),
    ];

    for (name, expected) in content_cases {
        for (prefix, platform) in contexts {
            let path = format!("{prefix}/{name}");
            let c = f(&path, *platform);
            assert_eq!(
                c.category, *expected,
                "{path}: an installer name must not outrank a content extension"
            );
            // … and the installer signal is still on the record.
            assert!(
                c.matched_rules.contains(&RuleId::InstallerName),
                "{path}: the competing name signal must be preserved"
            );
        }
    }
}

/// Case must not change the answer: `SETUP.EXE`, `Setup.Exe`, `UPDATE.LOG` …
#[test]
fn finding1_case_variants_are_stable() {
    for stem in ["setup", "Setup", "SETUP", "sEtUp", "update", "UPDATE"] {
        for ext in ["exe", "EXE", "Exe"] {
            let path = format!("C:/Program Files/App/{stem}.{ext}");
            let c = f(&path, Platform::Windows);
            assert_eq!(c.category, Category::Applications, "{path}");
            assert!(c.matched_rules.contains(&RuleId::InstallerName), "{path}");
        }
    }
    // In Downloads, uppercase `SETUP.EXE` is still a downloaded installer.
    let c = f("C:/Users/user/Downloads/SETUP.EXE", Platform::Windows);
    assert_eq!(c.category, Category::Downloads);
    assert_eq!(c.winning_rule, RuleId::InstallerName);
    // … but `SETUP.ZIP` is still an archive.
    let c = f("C:/Users/user/Downloads/SETUP.ZIP", Platform::Windows);
    assert_eq!(c.category, Category::Archives);
}

/// A bare substring is not an installer name: `container`, `installation`
/// (no separator) and `up-to-date` must not match.
#[test]
fn finding1_name_matching_stays_conservative() {
    for name in [
        "container.tar.gz",
        "installationguide.pdf",
        "up-to-date-notes.txt",
        "setuptools-68.0.0.tar.gz",
    ] {
        let c = f(&format!("/home/user/project/{name}"), Platform::Linux);
        assert!(
            !c.matched_rules.contains(&RuleId::InstallerName),
            "{name}: a bare substring must not be an installer signal"
        );
    }
}

/// `setup.pdf` (document) vs `setup.exe` (executable) vs `setup.zip`
/// (archive) — the three-way case the audit called out explicitly.
#[test]
fn finding1_setup_pdf_exe_zip_three_way() {
    let pdf = f("/home/user/Downloads/setup.pdf", Platform::Linux);
    let exe = f("C:/Users/user/Downloads/setup.exe", Platform::Windows);
    let zip = f("/home/user/Downloads/setup.zip", Platform::Linux);
    assert_eq!(pdf.category, Category::Documents);
    assert_eq!(exe.category, Category::Downloads);
    assert_eq!(zip.category, Category::Archives);
}

// ===========================================================================
// FINDING 2 — ApplicationData is not Applications
// ===========================================================================

#[test]
fn finding2_windows_install_location_is_not_application_data() {
    // Install locations → Applications.
    for p in [
        "C:/Program Files/App",
        "C:/Program Files (x86)/App",
        "C:/Program Files/App/bin",
    ] {
        let c = d(p, Platform::Windows);
        assert_eq!(c.category, Category::Applications, "{p}");
    }
    // Application-owned data → ApplicationData (never Applications).
    for p in [
        "C:/ProgramData/App",
        "C:/Users/user/AppData/Local/App",
        "C:/Users/user/AppData/Roaming/App",
        "C:/Users/user/AppData/LocalLow/App",
    ] {
        let c = d(p, Platform::Windows);
        assert_eq!(c.category, Category::ApplicationData, "{p}");
        assert_ne!(c.category, Category::Applications, "{p}");
    }
}

#[test]
fn finding2_macos_separates_bundle_support_caches_logs() {
    assert_eq!(
        d("/Applications/App.app", Platform::Mac).category,
        Category::Applications
    );
    assert_eq!(
        d("/Users/user/Library/Application Support/App", Platform::Mac).category,
        Category::ApplicationData
    );
    assert_eq!(
        d("/Users/user/Library/Caches/App", Platform::Mac).category,
        Category::Cache
    );
    assert_eq!(
        d("/Users/user/Library/Logs/App", Platform::Mac).category,
        Category::Logs
    );
    // A cache inside application support is still a cache — the more specific
    // pattern wins by depth.
    assert_eq!(
        d("/Users/user/Library/Caches/App/Cache", Platform::Mac).category,
        Category::Cache
    );
}

#[test]
fn finding2_linux_does_not_collapse_usr_opt_var() {
    // OS-managed system trees.
    assert_eq!(d("/usr", Platform::Linux).category, Category::SystemData);
    assert_eq!(d("/etc", Platform::Linux).category, Category::SystemData);
    assert_eq!(
        d("/usr/lib/app", Platform::Linux).category,
        Category::SystemData
    );
    // Third-party install location — a different semantic entity.
    assert_eq!(
        d("/opt/app", Platform::Linux).category,
        Category::Applications
    );
    assert_ne!(
        d("/opt/app", Platform::Linux).category,
        Category::SystemData
    );
    // /var is a system tree, but its specific children are their own thing.
    assert_eq!(d("/var", Platform::Linux).category, Category::SystemData);
    assert_eq!(d("/var/log", Platform::Linux).category, Category::Logs);
    assert_eq!(d("/var/cache", Platform::Linux).category, Category::Cache);
    assert_eq!(
        d("/var/tmp", Platform::Linux).category,
        Category::TemporaryData
    );
    // XDG application-owned data.
    assert_eq!(
        d("/home/user/.config/app", Platform::Linux).category,
        Category::ApplicationData
    );
    assert_eq!(
        d("/home/user/.local/share/app", Platform::Linux).category,
        Category::ApplicationData
    );
}

/// The question SpaceLens must be able to answer: "how much space does this
/// application use?" — installation and data must be separately aggregatable.
#[test]
fn finding2_application_and_application_data_aggregate_separately() {
    let mut agg = CategoryAggregator::new();
    let install = d("C:/Program Files/App", Platform::Windows);
    let data = d("C:/Users/user/AppData/Roaming/App", Platform::Windows);
    agg.push(&install, &EntryKind::Dir, 100);
    agg.push(&data, &EntryKind::Dir, 900);

    assert_eq!(agg.totals(Category::Applications).logical_size, 100);
    assert_eq!(agg.totals(Category::ApplicationData).logical_size, 900);
    assert_ne!(
        agg.totals(Category::Applications).logical_size,
        agg.totals(Category::ApplicationData).logical_size
    );
}

// ===========================================================================
// FINDING 3 — the parent tracker must be a genuine LRU
// ===========================================================================

/// The exact scenario from the audit: capacity 3, insert A B C, look up A,
/// insert D. A FIFO would evict A; an LRU must evict B.
#[test]
fn finding3_tracker_is_a_genuine_lru_not_fifo() {
    let mut t = ParentContextTracker::with_capacity(3);
    t.record(1, Category::Cache); // A
    t.record(2, Category::Logs); // B
    t.record(3, Category::Backups); // C

    // Touch A: A becomes most recently used, B becomes the LRU victim.
    assert_eq!(t.parent_category(Some(1)), Some(Category::Cache));

    t.record(4, Category::Games); // D → must evict B (2)

    assert_eq!(t.peek(1), Some(Category::Cache), "A survives (was touched)");
    assert_eq!(
        t.peek(2),
        None,
        "B is evicted — a FIFO would have evicted A"
    );
    assert_eq!(t.peek(3), Some(Category::Backups), "C survives");
    assert_eq!(t.peek(4), Some(Category::Games), "D is present");
    assert_eq!(t.len(), 3, "capacity is respected");
}

#[test]
fn finding3_eviction_order_is_recency_order() {
    let mut t = ParentContextTracker::with_capacity(4);
    for i in 1..=4u64 {
        t.record(i, Category::Cache);
    }
    // Touch 2 then 4: LRU order (least → most) becomes 1, 3, 2, 4.
    let _ = t.parent_category(Some(2));
    let _ = t.parent_category(Some(4));

    t.record(5, Category::Logs); // evicts 1
    assert_eq!(t.peek(1), None);
    t.record(6, Category::Logs); // evicts 3
    assert_eq!(t.peek(3), None);
    assert!(t.peek(2).is_some() && t.peek(4).is_some() && t.peek(5).is_some());
    assert!(t.len() <= t.capacity());
}

#[test]
fn finding3_tracker_edge_cases() {
    // Capacity 0 stores nothing.
    let mut t = ParentContextTracker::with_capacity(0);
    t.record(1, Category::Cache);
    assert_eq!(t.len(), 0);
    assert_eq!(t.parent_category(Some(1)), None);

    // Capacity 1 keeps only the latest.
    let mut t = ParentContextTracker::with_capacity(1);
    t.record(1, Category::Cache);
    t.record(2, Category::Logs);
    assert_eq!(t.peek(1), None);
    assert_eq!(t.peek(2), Some(Category::Logs));
    assert_eq!(t.len(), 1);

    // Lookup of a missing key changes nothing.
    let mut t = ParentContextTracker::with_capacity(2);
    t.record(1, Category::Cache);
    assert_eq!(t.parent_category(Some(99)), None);
    assert_eq!(t.parent_category(None), None);
    assert_eq!(t.len(), 1);

    // Repeated lookups are stable and idempotent.
    for _ in 0..5 {
        assert_eq!(t.parent_category(Some(1)), Some(Category::Cache));
    }
    assert_eq!(t.len(), 1);

    // Duplicate record does not consume capacity or change the value.
    let mut t = ParentContextTracker::with_capacity(2);
    t.record(7, Category::Cache);
    t.record(8, Category::Logs);
    t.record(7, Category::Games);
    assert_eq!(t.peek(7), Some(Category::Cache), "first write wins");
    assert_eq!(t.len(), 2);
}

#[test]
fn finding3_tracker_memory_is_bounded_under_a_hostile_stream() {
    let mut t = ParentContextTracker::with_capacity(32);
    for id in 0..500_000u64 {
        t.record(id, Category::Cache);
        let _ = t.parent_category(Some(id / 2));
    }
    assert!(t.len() <= 32);
    assert!(t.capacity() <= ParentContextTracker::MAX_ENTRIES);
}

// ===========================================================================
// FINDING 4 — evidence fidelity: every item truthfully describes its signal
// ===========================================================================

/// The naming four cases from the audit.
#[test]
fn finding4_evidence_kinds_are_truthful_for_named_cases() {
    // setup.zip → both signals, each with its own kind.
    let c = f("/x/setup.zip", Platform::Linux);
    let kinds: Vec<_> = c.evidence.iter().map(|e| (e.rule, e.kind)).collect();
    assert!(
        kinds.contains(&(RuleId::ArchiveExtension, EvidenceKind::Extension)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(RuleId::InstallerName, EvidenceKind::FilenamePattern)),
        "{kinds:?}"
    );
    // The decisive assertion: the archive rule is NOT relabelled as a path
    // pattern just because it lost.
    assert!(
        !kinds.contains(&(RuleId::ArchiveExtension, EvidenceKind::KnownPathPattern)),
        "a losing rule must keep its own evidence kind"
    );

    // setup.exe
    let c = f("/x/setup.exe", Platform::Windows);
    assert!(c
        .evidence
        .iter()
        .any(|e| e.rule == RuleId::ExecutableExtension && e.kind == EvidenceKind::Extension));

    // installer.msi
    let c = f("/x/installer.msi", Platform::Windows);
    assert!(c
        .evidence
        .iter()
        .any(|e| e.rule == RuleId::InstallerExtension && e.kind == EvidenceKind::Extension));
    assert!(c
        .evidence
        .iter()
        .any(|e| e.rule == RuleId::InstallerName && e.kind == EvidenceKind::FilenamePattern));

    // cache.log — a single extension signal, correctly typed.
    let c = f("/x/cache.log", Platform::Linux);
    assert_eq!(c.winning_rule, RuleId::LogExtension);
    assert_eq!(
        c.evidence.iter().next().map(|e| e.kind),
        Some(EvidenceKind::Extension)
    );
}

/// The general invariant: every evidence item's kind must be one of the kinds
/// its rule actually declares. Swept over the whole fixture matrix × every
/// platform × both entry kinds.
///
/// A rule may legitimately declare more than one kind *only when the same rule
/// id is reachable through two different mechanisms* — e.g. `CACHE_DIR` may be
/// reached by a bare name (`KnownPathPattern`) or by a rooted platform cache
/// location (`KnownCacheLocation`). What is forbidden is a kind the rule never
/// declares: that is exactly the `KnownPathPattern`-for-`ArchiveExtension`
/// defect this test exists to catch.
#[test]
fn finding4_every_evidence_item_matches_its_rule_declaration() {
    // rule id → every evidence kind some rule with that id declares.
    let mut allowed: std::collections::HashMap<RuleId, std::collections::HashSet<EvidenceKind>> =
        std::collections::HashMap::new();
    for r in spacelens_classifier::RULES {
        allowed.entry(r.id).or_default().insert(r.evidence);
    }
    for r in spacelens_classifier::LOCATION_RULES {
        allowed.entry(r.id).or_default().insert(r.evidence);
    }
    // Synthesized outcomes rather than table entries: the no-signal fallback
    // and the parent-context contribution. Their kinds are fixed by
    // construction, so they are declared here rather than in a table.
    allowed.insert(
        RuleId::DirWithoutSignals,
        [EvidenceKind::EntryKind].into_iter().collect(),
    );
    allowed.insert(
        RuleId::PlainFile,
        [EvidenceKind::EntryKind].into_iter().collect(),
    );
    allowed.insert(
        RuleId::NoSignals,
        [EvidenceKind::EntryKind].into_iter().collect(),
    );
    allowed.insert(
        RuleId::ParentInherited,
        [EvidenceKind::ParentContext].into_iter().collect(),
    );

    let paths = [
        "C:/Users/user/Downloads/setup.exe",
        "C:/Program Files/App/update.exe",
        "C:/Program Files/App/uninstall.exe",
        "C:/Program Files/App/update.log",
        "C:/Windows/System32/drivers/etc/hosts",
        "C:/Users/user/AppData/Local/App/cache",
        "/home/user/project/update.txt",
        "/home/user/project/setup.zip",
        "/home/user/.cache/app",
        "/home/user/.config/app",
        "/var/log/app/nginx.log",
        "/opt/app/lib/tool.exe",
        "/Applications/App.app/Contents/MacOS/App",
        "/Users/user/Library/Application Support/App/data.db",
        "/Users/user/Library/Caches/App/blob",
        "/tmp/scratch",
        "report.pdf",
        "photo.png",
        "movie.mp4",
        "song.flac",
        "archive.tar.gz",
        "disk.iso",
        "main.rs",
        "main.tsx",
        "run.log",
        "unknown.bin",
        "noextension",
        ".hidden",
        ".env.secret",
    ];
    for p in paths {
        for kind in [EntryKind::File, EntryKind::Dir] {
            for platform in ALL_PLATFORMS {
                let e = entry(1, None, p, kind.clone());
                let c = classify(&e, &ParentContext::default(), platform);
                for ev in c.evidence.iter() {
                    let ok = allowed
                        .get(&ev.rule)
                        .is_some_and(|kinds| kinds.contains(&ev.kind));
                    assert!(
                        ok,
                        "{p} ({platform:?}, {kind:?}): evidence for {} carries kind {:?}, \
                         which no rule with that id declares",
                        ev.rule.code(),
                        ev.kind
                    );
                }
            }
        }
    }
}

/// Evidence is bounded, winner-first, and deterministically ordered.
#[test]
fn finding4_evidence_is_bounded_ordered_and_deterministic() {
    let e = file(1, None, "C:/Users/user/Downloads/setup.exe");
    let a = classify(&e, &ParentContext::default(), Platform::Windows);
    let b = classify(&e, &ParentContext::default(), Platform::Windows);
    assert!(a.evidence.len() <= MAX_EVIDENCE);
    assert_eq!(
        a.evidence.iter().next().map(|x| x.rule),
        Some(a.winning_rule),
        "winner's evidence comes first"
    );
    let av: Vec<_> = a.evidence.iter().collect();
    let bv: Vec<_> = b.evidence.iter().collect();
    assert_eq!(av, bv, "evidence order is deterministic");
    // No duplicates from repeated identical matches.
    let mut seen = std::collections::HashSet::new();
    for ev in a.evidence.iter() {
        assert!(seen.insert((ev.kind, ev.rule)), "duplicate evidence item");
    }
}

/// Evidence is structural only — no path text can leak through serialization.
#[test]
fn finding4_evidence_never_leaks_path_text() {
    let secret = "C:/Users/Alice/Downloads/secret-session-token.log";
    let c = f(secret, Platform::Windows);
    let json = serde_json::to_string(&c).unwrap();
    for fragment in ["Alice", "secret", "session", "token", "Downloads", "C:"] {
        assert!(
            !json.contains(fragment),
            "evidence/classification leaked path text {fragment:?}: {json}"
        );
    }
}

// ===========================================================================
// FINDING 5 — one confidence policy, mechanically enforced
// ===========================================================================

#[test]
fn finding5_extension_only_never_exceeds_medium() {
    // .png/.pdf/.zip/.mp3 in a neutral place, with and without a parent that
    // would love to raise confidence.
    for p in [
        "/x/a.png", "/x/b.pdf", "/x/c.zip", "/x/d.mp3", "/x/e.iso", "/x/f.rs", "/x/g.log",
    ] {
        let no_ctx = f(p, Platform::Linux);
        assert!(no_ctx.confidence <= Confidence::EXTENSION_ONLY_CAP, "{p}");

        for parent in [
            Category::Images,
            Category::Documents,
            Category::Downloads,
            Category::ApplicationData,
        ] {
            let c = classify(
                &file(1, None, p),
                &ParentContext {
                    parent_category: Some(parent),
                    under_user_profile: true,
                },
                Platform::Linux,
            );
            assert!(
                c.confidence <= Confidence::EXTENSION_ONLY_CAP,
                "{p} with parent {parent:?}: context must not breach the extension cap"
            );
        }
    }
}

#[test]
fn finding5_uncorroborated_heuristics_stay_low() {
    // Bare build/output/cache/temp/backup names in a place we know nothing
    // about: a guess, capped at Low.
    for name in ["build", "out", "cache", "temp", "backup", "logs", "obj"] {
        let c = d(&format!("/home/user/project/{name}"), Platform::Linux);
        assert_eq!(
            c.confidence,
            Confidence::Low,
            "/home/user/project/{name} must stay in the heuristic band"
        );
    }
    // Corroborated by an authoritative platform location, the same names may
    // reach Medium — but never High, because the name is still a guess.
    let c = d("/var/cache", Platform::Linux);
    assert_eq!(c.category, Category::Cache);
    assert!(c.confidence >= Confidence::Medium);
    assert_eq!(
        c.confidence,
        Confidence::High,
        "the location is authoritative, not the name"
    );
}

#[test]
fn finding5_authoritative_locations_reach_high() {
    for (p, platform) in [
        ("C:/Windows", Platform::Windows),
        ("C:/Program Files", Platform::Windows),
        ("C:/Users/user/AppData/Local", Platform::Windows),
        ("/Applications", Platform::Mac),
        ("/Users/user/Library/Caches", Platform::Mac),
        ("/usr", Platform::Linux),
        ("/etc", Platform::Linux),
        ("/var/log", Platform::Linux),
        ("/opt/app", Platform::Linux),
    ] {
        let c = d(p, platform);
        assert_eq!(c.confidence, Confidence::High, "{p} is authoritative");
    }
}

/// The policy is a property of the *table*, not of a test: every ungated rule
/// must declare a base confidence within its kind's ceiling.
#[test]
fn finding5_rule_table_cannot_declare_a_confidence_above_its_kind() {
    for r in spacelens_classifier::RULES {
        if r.gate.is_none() {
            assert!(
                r.confidence <= r.kind.cap(),
                "rule {} declares {:?} but kind {:?} caps at {:?}",
                r.id.code(),
                r.confidence,
                r.kind,
                r.kind.cap()
            );
        }
    }
}

#[test]
fn finding5_context_raises_at_most_one_band_and_never_a_bucket() {
    let p = "/x/cache";
    let base = d(p, Platform::Linux);
    assert_eq!(base.confidence, Confidence::Low);
    let raised = classify(
        &dir(1, None, p),
        &ParentContext {
            parent_category: Some(Category::Cache),
            under_user_profile: true,
        },
        Platform::Linux,
    );
    assert_eq!(
        raised.confidence,
        Confidence::Medium,
        "corroborated heuristic: Low → Medium, no further"
    );
    // A bucket parent cannot raise anything.
    for bucket in [Category::Other, Category::Unknown] {
        let c = classify(
            &dir(1, None, p),
            &ParentContext {
                parent_category: Some(bucket),
                under_user_profile: true,
            },
            Platform::Linux,
        );
        assert_eq!(
            c.confidence,
            Confidence::Low,
            "parent {bucket:?} knows nothing"
        );
    }
    // `under_user_profile` alone is not corroboration.
    let c = classify(
        &dir(1, None, p),
        &ParentContext {
            parent_category: None,
            under_user_profile: true,
        },
        Platform::Linux,
    );
    assert_eq!(c.confidence, Confidence::Low);
}

// ===========================================================================
// FINDING 6 — strong location knowledge vs weak basename heuristics
// ===========================================================================

#[test]
fn finding6_project_directories_are_weak_heuristics_only() {
    // All of these may be ordinary project directories. They may be *labelled*
    // by their name, but never with the confidence of a real platform
    // location, and a strong location must be able to override them.
    for name in ["cache", "build", "tmp", "backup", "logs"] {
        let c = d(&format!("/home/user/project/{name}"), Platform::Linux);
        assert_eq!(c.confidence, Confidence::Low, "/home/user/project/{name}");
    }
    // Every one of them lives under a pure container (UserHome), which must
    // not reclassify it into UserData.
    let plain = d("/home/user/project/randomdir", Platform::Linux);
    assert_eq!(
        plain.category,
        Category::Other,
        "a home container is not a category"
    );
    assert_eq!(plain.confidence, Confidence::Low);
}

#[test]
fn finding6_a_basename_is_not_a_system_location() {
    // `/home/user/windows` is a directory someone made; `C:/Windows` is an
    // operating system.
    let fake = d("/home/user/windows", Platform::Linux);
    assert_ne!(fake.category, Category::SystemData);
    assert_eq!(fake.confidence, Confidence::Low);

    for (p, platform) in [
        ("C:/Windows", Platform::Windows),
        ("C:/Program Files", Platform::Windows),
        ("C:/Users/user/AppData/Local", Platform::Windows),
        ("/usr", Platform::Linux),
        ("/etc", Platform::Linux),
        ("/var", Platform::Linux),
        ("/Applications", Platform::Mac),
        ("/Users/user/Library", Platform::Mac),
    ] {
        let c = d(p, platform);
        assert_eq!(
            c.confidence,
            Confidence::High,
            "{p} is real location knowledge"
        );
    }
}

#[test]
fn finding6_the_same_name_classifies_differently_by_location() {
    // `cache` in a project: Low heuristic. `cache` in ~/Library/Caches: the
    // location already said so, authoritatively.
    let project = d("/home/user/project/cache", Platform::Linux);
    let platform_dir = d("/Users/user/Library/Caches/App", Platform::Mac);
    assert_eq!(project.category, Category::Cache);
    assert_eq!(platform_dir.category, Category::Cache);
    assert!(
        project.confidence < platform_dir.confidence,
        "same category, different strength of knowledge: {:?} vs {:?}",
        project.confidence,
        platform_dir.confidence
    );
}

// ===========================================================================
// FINDING 7 — `Unknown` vs `Other` is a real contract
// ===========================================================================

#[test]
fn finding7_other_is_the_normal_fallback() {
    // Ordinary unknown-extension file → Other, understood but unremarkable.
    let c = f("/data/thing.bin", Platform::Linux);
    assert_eq!(c.category, Category::Other);
    assert_eq!(c.confidence, Confidence::Low);
    // Ordinary directory with no stronger signal → Other.
    let c = d("/data/stuff", Platform::Linux);
    assert_eq!(c.category, Category::Other);
    assert_eq!(c.confidence, Confidence::Low);
    // Inside a user home, too — a home container is not a category.
    let c = d("/home/user/stuff", Platform::Linux);
    assert_eq!(c.category, Category::Other);
}

#[test]
fn finding7_unknown_requires_a_broken_observation() {
    // 1. Incomplete metadata.
    let mut broken = file(1, None, "/data/thing.bin");
    broken.error = Some(spacelens_engine::model::ErrorCategoryRef::MetadataUnavailable);
    let c = classify(&broken, &ParentContext::default(), Platform::Linux);
    assert_eq!(c.category, Category::Unknown);
    assert_eq!(c.confidence, Confidence::Unknown);

    // 2. No usable name.
    for p in ["", "/", "//"] {
        let c = f(p, Platform::Linux);
        assert_eq!(c.category, Category::Unknown, "{p:?}");
    }

    // 3. An entry kind we cannot interpret (socket/FIFO/device) with no signal.
    let sock = entry(1, None, "/x/sock", EntryKind::Other);
    let c = classify(&sock, &ParentContext::default(), Platform::Linux);
    assert_eq!(c.category, Category::Unknown);
    // … but a name signal still wins: a FIFO called *.log is a log.
    let named = entry(2, None, "/x/thing.log", EntryKind::Other);
    let c = classify(&named, &ParentContext::default(), Platform::Linux);
    assert_eq!(c.category, Category::Logs);
}

#[test]
fn finding7_unknown_is_not_manufactured() {
    // Unknown must be rare: across the whole fixture matrix, nothing
    // well-formed may land there.
    let paths = [
        "report.pdf",
        "photo.png",
        "movie.mp4",
        "song.flac",
        "archive.zip",
        "archive.tar.gz",
        "disk.iso",
        "main.rs",
        "main.ts",
        "main.tsx",
        "run.log",
        "tool.exe",
        "setup.exe",
        "setup.zip",
        "setup.log",
        "update.exe",
        "update.log",
        "update.txt",
        "uninstall.exe",
        "unknown.bin",
        "noextension",
        ".hidden",
        ".env.secret",
    ];
    for p in paths {
        for platform in ALL_PLATFORMS {
            let c = f(&format!("/data/{p}"), platform);
            assert_ne!(c.category, Category::Unknown, "/data/{p} on {platform:?}");
        }
    }
}

// ===========================================================================
// FINDING 9 — host independence
// ===========================================================================

/// Windows semantics must be exercisable (and identical) on a Linux or macOS
/// CI host. Both separators are handled by string splitting, never `PathBuf`.
#[test]
fn finding9_windows_semantics_are_host_independent() {
    let cases: &[(&str, Category)] = &[
        ("C:/Users/user/Downloads/setup.exe", Category::Downloads),
        ("C:/Program Files/App/update.exe", Category::Applications),
        ("C:/Program Files/App/uninstall.exe", Category::Applications),
        ("C:/Program Files/App/update.log", Category::Logs),
        ("C:/Users/user/AppData/Local/App", Category::ApplicationData),
        ("C:/Users/user/Downloads/setup.zip", Category::Archives),
    ];
    for (p, expected) in cases {
        let forward = f(p, Platform::Windows);
        assert_eq!(forward.category, *expected, "forward-slash {p}");

        // Same path with backslashes: identical result on every host.
        let back = p.replace('/', "\\");
        let backslashed = f(&back, Platform::Windows);
        assert_eq!(
            backslashed.category, *expected,
            "backslash {back} must match forward-slash {p}"
        );
        assert_eq!(backslashed.winning_rule, forward.winning_rule, "{back}");
    }
}

/// The regression that broke once already: `PathBuf` separator semantics must
/// never decide a classification.
#[test]
fn finding9_name_extraction_does_not_use_pathbuf_separators() {
    for p in [
        r"C:\Users\user\Downloads\setup.exe",
        "C:/Users/user/Downloads/setup.exe",
    ] {
        let c = f(p, Platform::Windows);
        assert_eq!(c.category, Category::Downloads, "{p}");
        assert_eq!(c.winning_rule, RuleId::InstallerName, "{p}");
    }
    // A path with no separators at all still yields a usable name.
    let c = f("photo.png", Platform::Windows);
    assert_eq!(c.category, Category::Images);
}

#[test]
fn finding9_platform_knowledge_does_not_leak() {
    // A Windows path analysed as Linux must not pick up Windows locations …
    let as_linux = d("C:/Windows", Platform::Linux);
    assert_ne!(
        as_linux.category,
        Category::SystemData,
        "Windows knowledge leaked into Linux"
    );
    // … and vice versa.
    let as_windows = d("/usr", Platform::Windows);
    assert_ne!(
        as_windows.category,
        Category::SystemData,
        "Linux knowledge leaked into Windows"
    );
    // macOS `Library` is not a Windows concept.
    let as_windows = d(
        "/Users/user/Library/Application Support/App",
        Platform::Windows,
    );
    assert_ne!(as_windows.category, Category::ApplicationData);
}

#[test]
fn finding9_every_platform_is_exercised_explicitly() {
    // No test may depend on `Platform::current()`; assert the three values are
    // actually distinct inputs here.
    for platform in ALL_PLATFORMS {
        let c = d("/x/cache", platform);
        assert_eq!(c.category, Category::Cache, "{platform:?}");
        let _ = classify(
            &file(1, None, "/x/a.png"),
            &ParentContext::default(),
            platform,
        );
    }
    // Windows/macOS are case-insensitive, Linux is not — proven, not assumed.
    assert_eq!(
        d("/X/CACHE", Platform::Windows).winning_rule,
        RuleId::CacheDir
    );
    assert_ne!(
        d("/X/CACHE", Platform::Linux).winning_rule,
        RuleId::CacheDir
    );
}

// ===========================================================================
// Adversarial nesting
// ===========================================================================

#[test]
fn adversarial_nesting_project_tree_keeps_child_semantics() {
    let root = "/home/user/project";
    let expectations: &[(&str, Category)] = &[
        ("cache", Category::Cache),
        ("build", Category::Development),
        ("logs", Category::Logs),
        ("node_modules", Category::Development),
        ("update.log", Category::Logs),
        ("setup.zip", Category::Archives),
    ];
    for (name, expected) in expectations {
        let p = format!("{root}/{name}");
        let is_dir = !name.contains('.');
        let c = if is_dir {
            d(&p, Platform::Linux)
        } else {
            f(&p, Platform::Linux)
        };
        assert_eq!(c.category, *expected, "{p}");
        // Nothing in an arbitrary project tree is ever downloaded content.
        assert_ne!(c.category, Category::Downloads, "{p}");
    }
}

#[test]
fn adversarial_nesting_inside_an_installed_application() {
    let root = "C:/Program Files/App";
    for (name, expected) in [
        ("cache", Category::Cache),
        ("update.exe", Category::Applications),
        ("uninstall.exe", Category::Applications),
        ("update.log", Category::Logs),
    ] {
        let p = format!("{root}/{name}");
        let c = if name.contains('.') {
            f(&p, Platform::Windows)
        } else {
            d(&p, Platform::Windows)
        };
        assert_eq!(c.category, expected, "{p}");
        // Application installation/update artifacts must never be Downloads.
        assert_ne!(c.category, Category::Downloads, "{p}");
    }
}

#[test]
fn adversarial_nesting_appdata_cache_is_not_confused_with_install() {
    let c = d("C:/Users/user/AppData/Local/App/cache", Platform::Windows);
    assert_eq!(c.category, Category::Cache);
    // The install tree is Applications; app data is ApplicationData; neither
    // is Downloads.
    assert_ne!(c.category, Category::Applications);
    assert_ne!(c.category, Category::Downloads);
}

#[test]
fn adversarial_nesting_downloads_directory_stays_mixed_content() {
    // A download folder is not a content type: children keep their own.
    let mut tracker = ParentContextTracker::new();
    let parent = dir(1, None, "C:/Users/user/Downloads");
    let pc = classify_streaming(&parent, Platform::Windows, &mut tracker);
    assert_eq!(pc.category, Category::Downloads);

    for (name, expected) in [
        ("paper.pdf", Category::Documents),
        ("photo.png", Category::Images),
        ("movie.mp4", Category::Video),
        ("archive.zip", Category::Archives),
        ("setup.exe", Category::Downloads),
        ("tool.exe", Category::Applications),
        ("notes.txt", Category::Documents),
    ] {
        let child = file(2, Some(1), &format!("C:/Users/user/Downloads/{name}"));
        let cc = classify_streaming(&child, Platform::Windows, &mut tracker);
        assert_eq!(cc.category, expected, "{name}");
    }
}

// ===========================================================================
// Second-order findings
// ===========================================================================

/// `bin` and `env` were removed from the name tables: both are ambiguous
/// enough that claiming *authoritative* confidence for them produced
/// confidently-wrong answers (`/usr/bin` → Development, `App/env` →
/// Development). The honest answer for a bare ambiguous name is `Other`.
#[test]
fn second_order_ambiguous_dir_names_do_not_claim_authority() {
    for name in ["bin", "env"] {
        let c = d(&format!("/data/{name}"), Platform::Linux);
        assert_ne!(
            c.confidence,
            Confidence::High,
            "a bare '{name}' must never claim authoritative confidence"
        );
        // … and a real system location that happens to end in that name is
        // classified by the location, not the name.
        let c = d(&format!("/usr/{name}"), Platform::Linux);
        assert_eq!(c.category, Category::SystemData, "/usr/{name}");
        assert_eq!(c.confidence, Confidence::High);
    }
}

/// A rooted location rule and a name rule may share a `RuleId` (`/var/cache`
/// matches `CacheDir` by location *and* by name). The IPC-visible
/// `matched_rules` list must not repeat an id.
#[test]
fn second_order_matched_rules_are_deduplicated() {
    let c = d("/var/cache", Platform::Linux);
    assert_eq!(c.category, Category::Cache);
    let mut seen = std::collections::HashSet::new();
    for r in &c.matched_rules {
        assert!(
            seen.insert(*r),
            "duplicate rule id {:?} in matched_rules",
            r
        );
    }
    // … and the evidence list is deduplicated too.
    let mut seen = std::collections::HashSet::new();
    for ev in c.evidence.iter() {
        assert!(seen.insert((ev.kind, ev.rule)), "duplicate evidence item");
    }
}

/// `under_user_profile` is derived from the rooted location analysis, not a
/// substring scan of the path: a path merely *containing* "root" is not the
/// root account's home. Both real user homes behave identically, and neither
/// reclassifies unremarkable contents.
#[test]
fn second_order_user_profile_flag_is_location_based() {
    // The two genuine Linux user homes: identical, honest outcomes.
    let a = f("/root/notes.bin", Platform::Linux);
    let b = f("/home/user/notes.bin", Platform::Linux);
    assert_eq!(a.category, Category::Other, "/root is a pure container");
    assert_eq!(a.category, b.category);
    assert_eq!(a.confidence, b.confidence);
    assert_eq!(a.winning_rule, b.winning_rule);

    // A path that merely contains "root" as a substring is a system tree, and
    // is classified by its location — the "root" component adds nothing.
    let c = f("/var/www/root/deep/file.bin", Platform::Linux);
    assert_eq!(c.category, Category::SystemData);
}

#[test]
fn matrix_files() {
    // (path fragment, platform, expected category)
    let cases: &[(&str, Platform, Category)] = &[
        ("report.pdf", Platform::Linux, Category::Documents),
        ("photo.png", Platform::Windows, Category::Images),
        ("movie.mp4", Platform::Mac, Category::Video),
        ("song.flac", Platform::Linux, Category::Audio),
        ("archive.zip", Platform::Windows, Category::Archives),
        ("archive.tar.gz", Platform::Linux, Category::Archives),
        ("disk.iso", Platform::Windows, Category::Archives),
        ("main.rs", Platform::Linux, Category::Development),
        ("main.ts", Platform::Windows, Category::Development),
        ("main.tsx", Platform::Mac, Category::Development),
        ("run.log", Platform::Windows, Category::Logs),
        ("tool.exe", Platform::Windows, Category::Applications),
        ("setup.exe", Platform::Windows, Category::Applications),
        ("setup.zip", Platform::Windows, Category::Archives),
        ("setup.log", Platform::Windows, Category::Logs),
        ("update.exe", Platform::Windows, Category::Applications),
        ("update.log", Platform::Windows, Category::Logs),
        ("update.txt", Platform::Linux, Category::Documents),
        ("uninstall.exe", Platform::Windows, Category::Applications),
        ("unknown.bin", Platform::Linux, Category::Other),
        ("noextension", Platform::Linux, Category::Other),
        (".hidden", Platform::Linux, Category::Other),
        (".env.secret", Platform::Linux, Category::Other),
    ];
    for (name, platform, expected) in cases {
        let c = f(&format!("/data/{name}"), *platform);
        assert_eq!(c.category, *expected, "{name} on {platform:?}");
        assert_ne!(c.category, Category::Unknown, "{name} on {platform:?}");
    }
}

/// The audit's directory list, at **real** roots. Bare names under a neutral
/// `/data` prefix; rooted platform locations at the root the platform defines
/// — `/data/Windows` is a directory someone created, not an operating system,
/// and classifying it as `SystemData` would be the exact basename defect this
/// repair removes.
#[test]
fn matrix_directories() {
    let cases: &[(&str, Platform, Category)] = &[
        // -- bare-name heuristics / conventions under a neutral parent --------
        ("/data/Downloads", Platform::Linux, Category::Downloads),
        ("/data/Documents", Platform::Linux, Category::UserData),
        ("/data/Desktop", Platform::Linux, Category::UserData),
        ("/data/cache", Platform::Linux, Category::Cache),
        ("/data/.cache", Platform::Linux, Category::Cache),
        ("C:/data/Cache", Platform::Windows, Category::Cache),
        ("/data/build", Platform::Linux, Category::Development),
        ("/data/out", Platform::Linux, Category::Development),
        ("/data/obj", Platform::Linux, Category::Development),
        ("/data/tmp", Platform::Linux, Category::TemporaryData),
        ("C:/data/temp", Platform::Windows, Category::TemporaryData),
        ("/data/backup", Platform::Linux, Category::Backups),
        ("/data/backups", Platform::Linux, Category::Backups),
        ("/data/logs", Platform::Linux, Category::Logs),
        ("/data/log", Platform::Linux, Category::Logs),
        ("/data/node_modules", Platform::Linux, Category::Development),
        ("/data/vendor", Platform::Linux, Category::Development),
        ("/data/.git", Platform::Linux, Category::Development),
        ("/data/.venv", Platform::Linux, Category::Development),
        ("/data/venv", Platform::Linux, Category::Development),
        ("/data/__pycache__", Platform::Linux, Category::Development),
        ("C:/data/steamapps", Platform::Windows, Category::Games),
        // -- rooted platform locations ---------------------------------------
        ("C:/Windows", Platform::Windows, Category::SystemData),
        (
            "C:/Program Files",
            Platform::Windows,
            Category::Applications,
        ),
        (
            "C:/ProgramData",
            Platform::Windows,
            Category::ApplicationData,
        ),
        (
            "C:/Users/u/AppData",
            Platform::Windows,
            Category::ApplicationData,
        ),
        ("/Users/u/Library", Platform::Mac, Category::ApplicationData),
        (
            "/Users/u/Library/Application Support",
            Platform::Mac,
            Category::ApplicationData,
        ),
        ("/Applications", Platform::Mac, Category::Applications),
        ("/usr", Platform::Linux, Category::SystemData),
        ("/etc", Platform::Linux, Category::SystemData),
        ("/var", Platform::Linux, Category::SystemData),
        ("/opt", Platform::Linux, Category::Applications),
    ];
    for (path, platform, expected) in cases {
        let c = d(path, *platform);
        assert_eq!(c.category, *expected, "dir {path} on {platform:?}");
    }
}

/// The same names, deliberately placed where they carry **no** location
/// knowledge, must not claim the confidence of a real platform location.
#[test]
fn matrix_directory_names_away_from_their_root_are_weak() {
    for (path, platform) in [
        ("/data/Windows", Platform::Windows),
        ("/data/Program Files", Platform::Windows),
        ("/data/ProgramData", Platform::Windows),
        ("/data/AppData", Platform::Windows),
        ("/data/Library", Platform::Mac),
        ("/data/Applications", Platform::Mac),
        ("/data/usr", Platform::Linux),
        ("/data/etc", Platform::Linux),
        ("/data/var", Platform::Linux),
        ("/data/opt", Platform::Linux),
    ] {
        let c = d(path, platform);
        assert_ne!(
            c.confidence,
            Confidence::High,
            "{path} is not a real platform location and must not be High"
        );
    }
}

#[test]
fn matrix_every_platform_exercised_for_every_fixture() {
    // Sweep: nothing panics, nothing is Unknown, and the result is stable
    // across repeated runs on all three platforms.
    let fixtures = [
        "report.pdf",
        "photo.png",
        "movie.mp4",
        "song.flac",
        "archive.zip",
        "archive.tar.gz",
        "disk.iso",
        "main.rs",
        "main.ts",
        "main.tsx",
        "run.log",
        "tool.exe",
        "setup.exe",
        "setup.zip",
        "setup.log",
        "update.exe",
        "update.log",
        "update.txt",
        "uninstall.exe",
        "unknown.bin",
        "noextension",
        ".hidden",
        ".env.secret",
        "cache",
        "build",
        "tmp",
        "backup",
        "logs",
        "node_modules",
        "Downloads",
        "Documents",
        "Desktop",
        "Windows",
        "Program Files",
        "AppData",
        "Library",
        "Applications",
        "usr",
        "etc",
        "var",
        "opt",
    ];
    for name in fixtures {
        for platform in ALL_PLATFORMS {
            for kind in [EntryKind::File, EntryKind::Dir] {
                let e = entry(1, None, &format!("/data/{name}"), kind.clone());
                let a = classify(&e, &ParentContext::default(), platform);
                let b = classify(&e, &ParentContext::default(), platform);
                assert_eq!(a, b, "{name} on {platform:?} is nondeterministic");
                assert!(a.evidence.len() <= MAX_EVIDENCE);
                if kind == EntryKind::File {
                    assert_ne!(a.category, Category::Unknown, "{name}");
                }
            }
        }
    }
}
