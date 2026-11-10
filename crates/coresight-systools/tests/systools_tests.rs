//! Integration tests for `coresight-systools`.
//!
//! Every fixture is synthetic. The tests prove the crate's contracts: source
//! kinds are never collapsed, unknown is never safe or zero, coverage is
//! honest, process attribution is exact, ordering is deterministic,
//! counters saturate, and the production source contains no I/O, execution,
//! network, persistence, or panic path.

use coresight_classifier::Platform;
use coresight_systools::{
    analyze_startup, known_vendor_updater_pattern, summarize, ApplicationId, DiagnosticFinding,
    FindingSeverity, MemoryInfo, PlatformInfo, ProcessAttribution, ReportCoverage, SourceKind,
    StartupFolderScope, StartupItem, StartupOptions, StartupSource, StartupTier, StorageVolume,
    SystemReport, TaskEntry, TaskInventory, KNOWN_SAFE_VENDOR_UPDATER_PATTERNS,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn item(id: &str, name: &str, source: StartupSource) -> StartupItem {
    StartupItem {
        id: id.to_string(),
        name: name.to_string(),
        command: Some(format!(
            "C:\\Program Files\\{name}\\{name}.exe --background"
        )),
        source,
        enabled: true,
        owner: None,
    }
}

fn run_key(hive: &str, key_path: &str) -> StartupSource {
    StartupSource::RegistryRun {
        hive: hive.to_string(),
        key_path: key_path.to_string(),
    }
}

fn tier_for(name: &str, source: StartupSource) -> StartupTier {
    let report = analyze_startup(&[item("probe", name, source)], &StartupOptions::default());
    report
        .items
        .first()
        .expect("one probe item yields one finding")
        .tier
}

fn windows_platform() -> PlatformInfo {
    PlatformInfo::new(Platform::Windows)
}

fn empty_report() -> SystemReport {
    SystemReport {
        platform: windows_platform(),
        memory: None,
        storage: Vec::new(),
        findings: Vec::new(),
        coverage: ReportCoverage::complete(1),
    }
}

fn volume(id: &str, total: Option<u64>, available: Option<u64>) -> StorageVolume {
    StorageVolume {
        id: id.to_string(),
        label: None,
        mount_point: None,
        file_system: None,
        total_bytes: total,
        available_bytes: available,
        removable: None,
    }
}

fn app(id: &str) -> ApplicationId {
    ApplicationId(id.to_string())
}

fn row(
    app_id: Option<&str>,
    display_name: Option<&str>,
    owned_by_user: bool,
) -> ProcessAttribution {
    ProcessAttribution {
        app_id: app_id.map(app),
        display_name: display_name.map(str::to_string),
        owned_by_user,
    }
}

// --- startup inventory ------------------------------------------------------

#[test]
fn unknown_source_is_off_limits_never_safe_to_disable() {
    let source = StartupSource::Unknown {
        raw: "synthetic://opaque".to_string(),
    };
    let report = analyze_startup(
        &[item("unknown-1", "Mystery", source)],
        &StartupOptions::default(),
    );
    let finding = &report.items[0];
    assert_eq!(finding.tier, StartupTier::OffLimits);
    assert_ne!(finding.tier, StartupTier::SafeToDisable);
    assert!(
        finding.why.contains("could not classify"),
        "why must name the evidence: {}",
        finding.why
    );
}

#[test]
fn privileged_sources_are_off_limits_even_with_allow_listed_names() {
    let daemon = StartupSource::LaunchDaemon {
        label: "com.example.googleupdate".to_string(),
    };
    assert_eq!(tier_for("GoogleUpdate", daemon), StartupTier::OffLimits);

    let machine_key = run_key("HKEY_LOCAL_MACHINE", RUN_KEY);
    assert_eq!(
        tier_for("MicrosoftEdgeUpdate", machine_key),
        StartupTier::OffLimits
    );

    let system_hive = run_key("HKU\\S-1-5-18", RUN_KEY);
    assert_eq!(tier_for("Anything", system_hive), StartupTier::OffLimits);

    let path_suffixed_hive = run_key("HKLM\\SOFTWARE", RUN_KEY);
    assert_eq!(
        tier_for("Anything", path_suffixed_hive),
        StartupTier::OffLimits
    );

    let policy_path = run_key("HKCU", "Software\\Policies\\Vendor\\Run");
    assert_eq!(tier_for("Anything", policy_path), StartupTier::OffLimits);
}

#[test]
fn startup_tier_rules_are_exhaustive() {
    assert_eq!(
        tier_for("App", run_key("HKCU", RUN_KEY)),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::RegistryRunOnce {
                hive: "HKCU".to_string(),
                key_path: RUN_KEY.to_string(),
            }
        ),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::StartupFolder {
                scope: StartupFolderScope::User,
            }
        ),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::StartupFolder {
                scope: StartupFolderScope::AllUsers,
            }
        ),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::LaunchAgent {
                label: "com.example.helper".to_string(),
            }
        ),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::SystemdUnit {
                unit: "cups.service".to_string(),
            }
        ),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::CronJob {
                schedule: "0 3 * * *".to_string(),
            }
        ),
        StartupTier::ReviewFirst
    );
    assert_eq!(
        tier_for(
            "App",
            StartupSource::Unknown {
                raw: "opaque".to_string(),
            }
        ),
        StartupTier::OffLimits
    );
}

#[test]
fn vendor_updater_allow_list_is_small_and_strict() {
    assert!(KNOWN_SAFE_VENDOR_UPDATER_PATTERNS.len() <= 3);
    assert_eq!(
        known_vendor_updater_pattern("GoogleUpdate"),
        Some("googleupdate")
    );
    assert_eq!(
        known_vendor_updater_pattern("MicrosoftEdgeUpdate.lnk"),
        Some("microsoftedgeupdate")
    );
    assert_eq!(
        known_vendor_updater_pattern("GoogleUpdate (32-bit)"),
        Some("googleupdate")
    );
    assert_eq!(known_vendor_updater_pattern("EvilGoogleUpdate"), None);
    assert_eq!(known_vendor_updater_pattern("My App"), None);

    let report = analyze_startup(
        &[item("safe-1", "GoogleUpdate", run_key("HKCU", RUN_KEY))],
        &StartupOptions::default(),
    );
    assert_eq!(report.items[0].tier, StartupTier::SafeToDisable);
    assert!(report.items[0].why.contains("vendor-updater allow-list"));
}

#[test]
fn review_tier_explains_its_evidence_in_a_real_sentence() {
    let report = analyze_startup(
        &[item(
            "folder-1",
            "BackupSync",
            StartupSource::StartupFolder {
                scope: StartupFolderScope::User,
            },
        )],
        &StartupOptions::default(),
    );
    let finding = &report.items[0];
    assert_eq!(finding.tier, StartupTier::ReviewFirst);
    assert!(finding.why.contains("per-user Startup folder"));
    assert!(finding.why.contains("reversible"));
    assert!(finding.why.ends_with('.'), "why must be a sentence");
}

#[test]
fn startup_report_counts_the_whole_inventory_across_truncation() {
    let mut disabled = item("run-3", "DisabledThing", run_key("HKCU", RUN_KEY));
    disabled.enabled = false;

    let items = vec![
        item("run-1", "AppOne", run_key("HKCU", RUN_KEY)),
        item("run-2", "AppTwo", run_key("HKCU", RUN_KEY)),
        item("machine-1", "MachineThing", run_key("HKLM", RUN_KEY)),
        item(
            "unknown-1",
            "Opaque",
            StartupSource::Unknown {
                raw: "opaque".to_string(),
            },
        ),
        disabled,
    ];

    let report = analyze_startup(
        &items,
        &StartupOptions {
            include_disabled: true,
            max_items: Some(2),
        },
    );
    assert_eq!(report.items.len(), 2);
    assert_eq!(report.truncated, 3);
    assert_eq!(report.total_enabled, 4);
    assert_eq!(report.supplied_items(), 5);
    assert!(report
        .items
        .iter()
        .all(|finding| finding.tier == StartupTier::OffLimits));
    assert_eq!(report.count(StartupTier::OffLimits), 2);
}

#[test]
fn startup_options_can_omit_disabled_findings_without_losing_counts() {
    let mut disabled = item("run-2", "DisabledThing", run_key("HKCU", RUN_KEY));
    disabled.enabled = false;
    let items = vec![item("run-1", "AppOne", run_key("HKCU", RUN_KEY)), disabled];

    let report = analyze_startup(&items, &StartupOptions::enabled_only());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.truncated, 0);
    assert_eq!(report.total_enabled, 1);
    assert_eq!(report.supplied_items(), 2);
    assert_eq!(
        report.by_source,
        vec![coresight_systools::SourceCount {
            source: SourceKind::RegistryRun,
            total: 2,
            enabled: 1,
        }]
    );
}

#[test]
fn startup_analysis_is_deterministic_across_permutations() {
    let forward = vec![
        item("b", "Beta", run_key("HKCU", RUN_KEY)),
        item("a", "Alpha", run_key("HKCU", RUN_KEY)),
        item(
            "c",
            "Gamma",
            StartupSource::CronJob {
                schedule: "0 1 * * *".to_string(),
            },
        ),
        item(
            "d",
            "Delta",
            StartupSource::Unknown {
                raw: "opaque".to_string(),
            },
        ),
    ];
    let mut backward = forward.clone();
    backward.reverse();

    let first = analyze_startup(&forward, &StartupOptions::default());
    let second = analyze_startup(&backward, &StartupOptions::default());
    assert_eq!(first, second);
}

#[test]
fn empty_startup_inventory_yields_an_empty_report() {
    let report = analyze_startup(&[], &StartupOptions::default());
    assert!(report.items.is_empty());
    assert!(report.by_source.is_empty());
    assert_eq!(report.total_enabled, 0);
    assert_eq!(report.truncated, 0);
    assert!(!report.is_truncated());
}

// --- system report ----------------------------------------------------------

#[test]
fn memory_unknown_is_not_zero() {
    let memory = MemoryInfo::unknown();
    assert_eq!(memory.total_bytes, None);
    assert_eq!(memory.used_bytes_resolved(), None);
    assert_eq!(memory.used_ratio_percent(), None);

    let report = SystemReport {
        memory: None,
        ..empty_report()
    };
    let summary = summarize(&report);
    assert!(summary.contains("RAM unknown"), "{summary}");
    assert!(!summary.contains("RAM 0"), "{summary}");
    assert!(!summary.contains("0 B"), "{summary}");

    let zero_total = MemoryInfo {
        total_bytes: Some(0),
        available_bytes: Some(0),
        used_bytes: None,
    };
    assert_eq!(zero_total.used_ratio_percent(), None);
}

#[test]
fn storage_unknown_capacity_is_not_zero() {
    let unknown = volume("v1", None, None);
    assert_eq!(unknown.used_bytes(), None);
    assert_eq!(unknown.used_ratio_percent(), None);

    let known_empty = volume("v2", Some(0), Some(0));
    assert_eq!(known_empty.used_bytes(), Some(0));
    assert_eq!(known_empty.used_ratio_percent(), None);

    let half = volume("v3", Some(1_000), Some(500));
    assert_eq!(half.used_bytes(), Some(500));
    assert_eq!(half.used_ratio_percent(), Some(50));
}

#[test]
fn coverage_requires_every_attempted_source_to_succeed() {
    assert!(!ReportCoverage::default().is_complete());
    assert!(!ReportCoverage::complete(0).is_complete());
    assert!(ReportCoverage::complete(3).is_complete());

    let partial = ReportCoverage::new(3, 1, 1, 1);
    assert!(!partial.is_complete());
    assert!(partial.is_consistent());
    let note = partial.note();
    assert!(note.contains("incomplete"), "{note}");
    assert!(note.contains("1 failed"), "{note}");
    assert!(note.contains("1 unsupported"), "{note}");

    let unaccounted = ReportCoverage::new(4, 2, 0, 0);
    assert!(unaccounted.note().contains("2 not reported"));

    let inconsistent = ReportCoverage::new(2, 1, 3, 0);
    assert!(!inconsistent.is_consistent());
    assert!(inconsistent.note().contains("accounting inconsistent"));
}

#[test]
fn summarize_flags_incomplete_coverage() {
    let report = SystemReport {
        coverage: ReportCoverage::new(3, 1, 1, 1),
        ..empty_report()
    };
    let summary = summarize(&report);
    assert!(summary.contains("incomplete"), "{summary}");
    assert!(summary.contains("coverage 1 of 3 sources"), "{summary}");
    assert!(summary.contains("RAM unknown"), "{summary}");
    assert!(
        summary.contains("no diagnostic findings reported"),
        "{summary}"
    );
    assert!(!summary.contains("coverage 3 of 3 sources: complete"));
}

#[test]
fn summarize_complete_report_is_short_and_honest() {
    let mut platform = windows_platform();
    platform.os_name = Some("Windows 11".to_string());
    platform.os_version = Some("24H2".to_string());
    platform.os_build = Some("26100".to_string());
    platform.architecture = Some("x86_64".to_string());

    let report = SystemReport {
        platform,
        memory: Some(MemoryInfo {
            total_bytes: Some(17_179_869_184),
            available_bytes: Some(8_589_934_592),
            used_bytes: None,
        }),
        storage: vec![volume("v1", Some(549_755_813_888), Some(274_877_906_944))],
        findings: vec![
            DiagnosticFinding {
                code: "CORE_MEMORY_PRESSURE",
                severity: FindingSeverity::Attention,
                summary: "Memory use is above the report threshold.".to_string(),
                evidence: "Used 8 GiB of 16 GiB at collection time.".to_string(),
            },
            DiagnosticFinding {
                code: "CORE_VOLUME_DETAIL",
                severity: FindingSeverity::Info,
                summary: "One fixed volume was examined.".to_string(),
                evidence: "The volume reported capacity and free space.".to_string(),
            },
        ],
        coverage: ReportCoverage::complete(3),
    };

    let summary = summarize(&report);
    assert!(summary.contains("Windows (Windows 11, 24H2, build 26100, x86_64)"));
    assert!(summary.contains("RAM 16.0 GiB"), "{summary}");
    assert!(
        summary.contains("1 storage volume (512.0 GiB total)"),
        "{summary}"
    );
    assert!(
        summary.contains("2 findings (0 critical, 1 attention)"),
        "{summary}"
    );
    assert!(
        summary.contains("coverage 3 of 3 sources: complete"),
        "{summary}"
    );
    assert!(!summary.contains("incomplete"), "{summary}");
}

#[test]
fn findings_are_sorted_by_severity_then_code() {
    let report = SystemReport {
        findings: vec![
            DiagnosticFinding {
                code: "CORE_Z",
                severity: FindingSeverity::Info,
                summary: "z".to_string(),
                evidence: "z".to_string(),
            },
            DiagnosticFinding {
                code: "CORE_A",
                severity: FindingSeverity::Critical,
                summary: "a".to_string(),
                evidence: "a".to_string(),
            },
            DiagnosticFinding {
                code: "CORE_M",
                severity: FindingSeverity::Attention,
                summary: "m".to_string(),
                evidence: "m".to_string(),
            },
        ],
        ..empty_report()
    };
    let codes: Vec<&str> = report
        .findings_sorted()
        .iter()
        .map(|finding| finding.code)
        .collect();
    assert_eq!(codes, vec!["CORE_A", "CORE_M", "CORE_Z"]);
    assert_eq!(report.count_by_severity(FindingSeverity::Critical), 1);
}

#[test]
fn partial_storage_total_is_not_summed() {
    let complete = vec![
        volume("a", Some(1_000), Some(100)),
        volume("b", Some(2_000), Some(200)),
    ];
    let partial = vec![
        volume("a", Some(1_000), Some(100)),
        volume("b", None, Some(200)),
    ];
    let empty: Vec<StorageVolume> = Vec::new();
    let report_with = |storage: Vec<StorageVolume>| SystemReport {
        storage,
        ..empty_report()
    };
    assert_eq!(
        report_with(complete).known_storage_total_bytes(),
        Some(3_000)
    );
    assert_eq!(report_with(partial).known_storage_total_bytes(), None);
    assert_eq!(report_with(empty).known_storage_total_bytes(), None);
    assert!(
        summarize(&report_with(vec![volume("a", None, None)])).contains("1 with unknown capacity")
    );
}

#[test]
fn reports_and_items_serialize_with_camel_case_fields() {
    let startup = analyze_startup(
        &[item("run-1", "App", run_key("HKCU", RUN_KEY))],
        &StartupOptions::default(),
    );
    let startup_json = serde_json::to_string(&startup).expect("startup report serializes");
    assert!(
        startup_json.contains("\"totalEnabled\":1"),
        "{startup_json}"
    );
    assert!(startup_json.contains("\"bySource\""), "{startup_json}");
    assert!(startup_json.contains("\"REGISTRY_RUN\""), "{startup_json}");
    assert!(startup_json.contains("\"keyPath\""), "{startup_json}");

    let system = SystemReport {
        memory: Some(MemoryInfo::unknown()),
        coverage: ReportCoverage::new(2, 1, 1, 0),
        ..empty_report()
    };
    let system_json = serde_json::to_string(&system).expect("system report serializes");
    assert!(system_json.contains("\"sourcesTotal\":2"), "{system_json}");
    assert!(
        system_json.contains("\"sourcesUnsupported\":0"),
        "{system_json}"
    );
    assert!(system_json.contains("\"osName\":null"), "{system_json}");
}

// --- task inventory ---------------------------------------------------------

#[test]
fn task_inventory_merges_processes_per_application() {
    let rows = vec![
        row(Some("app-a"), Some("Alpha"), true),
        row(Some("app-a"), Some("Alpha"), false),
        row(Some("app-b"), Some("Beta"), true),
        row(None, Some("Unknown helper"), false),
    ];
    let inventory = TaskInventory::from_attributions(&rows);

    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.entries[0].app_id, "app-a");
    assert_eq!(inventory.entries[0].process_count, 2);
    assert!(inventory.entries[0].running);
    assert!(
        !inventory.entries[0].owned_by_user,
        "a partially owned app is not the user's"
    );
    assert_eq!(inventory.entries[1].display_name, "Beta");
    assert_eq!(inventory.unattributed_processes, 1);
    assert_eq!(inventory.total_processes(), 4);
    assert_eq!(inventory.attributed_processes(), 3);
    assert_eq!(inventory.running_apps(), 2);
    assert!(!inventory.is_fully_attributed());
    assert_eq!(
        inventory.entry("app-b").map(|entry| entry.process_count),
        Some(1)
    );
    assert_eq!(inventory.entry("app-missing"), None);
}

#[test]
fn task_inventory_never_merges_distinct_app_ids_sharing_a_display_name() {
    let rows = vec![
        row(Some("app-1"), Some("Visual Studio Code"), true),
        row(Some("app-2"), Some("Visual Studio Code"), true),
    ];
    let inventory = TaskInventory::from_attributions(&rows);
    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.entries[0].app_id, "app-1");
    assert_eq!(inventory.entries[1].app_id, "app-2");
    assert!(inventory
        .entries
        .iter()
        .all(|entry| entry.display_name == "Visual Studio Code"));
}

#[test]
fn task_inventory_is_deterministic_under_permutation() {
    let forward = vec![
        row(Some("app-b"), Some("Zebra"), true),
        row(Some("app-a"), Some("Alpha"), true),
        row(Some("app-a"), Some("Zebra"), false),
        row(None, None, true),
    ];
    let mut backward = forward.clone();
    backward.reverse();

    let first = TaskInventory::from_attributions(&forward);
    let second = TaskInventory::from_attributions(&backward);
    assert_eq!(first, second);
    assert_eq!(first.entries[0].display_name, "Alpha");
    assert_eq!(first.entries[1].display_name, "Zebra");
}

#[test]
fn blank_app_ids_are_counted_as_unattributed() {
    let inventory = TaskInventory::new(
        vec![TaskEntry {
            app_id: "   ".to_string(),
            display_name: "Nameless".to_string(),
            running: true,
            process_count: 3,
            owned_by_user: true,
        }],
        1,
    );
    assert!(inventory.entries.is_empty());
    assert_eq!(inventory.unattributed_processes, 4);
    assert_eq!(inventory.total_processes(), 4);
}

#[test]
fn counters_saturate_instead_of_wrapping() {
    let inventory = TaskInventory::new(
        vec![TaskEntry {
            app_id: "app".to_string(),
            display_name: "App".to_string(),
            running: true,
            process_count: usize::MAX,
            owned_by_user: true,
        }],
        5,
    );
    assert_eq!(inventory.attributed_processes(), usize::MAX);
    assert_eq!(inventory.total_processes(), usize::MAX);

    let coverage = ReportCoverage::new(usize::MAX, usize::MAX, 1, 1);
    assert_eq!(coverage.accounted_sources(), usize::MAX);
    assert!(!coverage.is_consistent());
    assert!(!coverage.is_complete());

    let volume = volume("v", Some(10), Some(50));
    assert_eq!(volume.used_bytes(), Some(0));
}

// --- architecture guards ----------------------------------------------------

#[test]
fn crate_performs_no_io_or_execution() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut seen = 0usize;
    for entry in std::fs::read_dir(root).expect("src directory must be readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        seen += 1;
        let text = std::fs::read_to_string(&path).expect("source file must be readable");
        for forbidden in [
            "std::fs::",
            "std::process",
            "std::net",
            "rusqlite",
            "Command::new",
            "winreg",
            "std::os::windows",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains forbidden API {forbidden}",
                path.display()
            );
        }
    }
    assert!(seen > 0, "expected at least one production source file");
}

#[test]
fn production_code_contains_no_panic_paths() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut seen = 0usize;
    for entry in std::fs::read_dir(root).expect("src directory must be readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        seen += 1;
        let text = std::fs::read_to_string(&path).expect("source file must be readable");
        for forbidden in [
            ".unwrap()",
            ".expect(",
            "panic!(",
            "todo!(",
            "unimplemented!(",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains forbidden panic path {forbidden}",
                path.display()
            );
        }
    }
    assert!(seen > 0, "expected at least one production source file");
}
