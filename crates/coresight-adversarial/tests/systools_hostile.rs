//! Hostile inputs aimed at the system tools: startup inventory, the system
//! report, the task inventory, and the diagnostic/repair fixer.
//!
//! Every fixture is synthetic. Nothing is read from this machine, and nothing
//! is enabled, disabled, repaired, or changed. Each test names the honesty
//! property it protects.

use coresight_classifier::Platform;
use coresight_systools::fixer::{
    default_rules, plan_repairs, run, update_findings, CheckObservation, DiagnosticRule,
    RepairAction, SystemFacts, UpdateCheck, UpdateState,
};
use coresight_systools::{
    analyze_startup, known_vendor_updater_pattern, summarize, DiagnosticFinding, FindingSeverity,
    MemoryInfo, PlatformInfo, ReportCoverage, SourceKind, StartupFolderScope, StartupItem,
    StartupOptions, StartupSource, StartupTier, StorageVolume, SystemReport, TaskEntry,
    TaskInventory,
};

fn item(id: &str, name: &str, source: StartupSource, enabled: bool) -> StartupItem {
    StartupItem {
        id: id.to_string(),
        name: name.to_string(),
        command: Some("C:\\Program Files\\Vendor\\update.exe".to_string()),
        source,
        enabled,
        owner: Some("fixture-user".to_string()),
    }
}

fn user_run() -> StartupSource {
    StartupSource::RegistryRun {
        hive: "HKCU".to_string(),
        key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".to_string(),
    }
}

fn vendor_item(id: &str, source: StartupSource) -> StartupItem {
    item(id, "GoogleUpdate", source, true)
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

fn report_with(memory: Option<MemoryInfo>, storage: Vec<StorageVolume>) -> SystemReport {
    SystemReport {
        platform: PlatformInfo::new(Platform::Windows),
        memory,
        storage,
        findings: Vec::new(),
        coverage: ReportCoverage::complete(2),
    }
}

// ---------------------------------------------------------------------------
// Startup inventory: privilege beats a friendly name
// ---------------------------------------------------------------------------

// If this regresses, a root-owned launch daemon called "GoogleUpdate" is
// labelled safe to disable because its name matches the allow-list.
#[test]
fn a_launch_daemon_with_a_safe_vendor_name_stays_off_limits() {
    let report = analyze_startup(
        &[vendor_item(
            "daemon-1",
            StartupSource::LaunchDaemon {
                label: "com.google.keystone.daemon".to_string(),
            },
        )],
        &StartupOptions::all(),
    );

    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].tier, StartupTier::OffLimits);
    assert!(report.items[0].why.contains("root"));
    assert_eq!(report.count(StartupTier::SafeToDisable), 0);
}

// If this regresses, a machine-wide registry entry is treated as a per-user
// preference just because the vendor name is on the allow-list.
#[test]
fn a_privileged_registry_location_beats_the_vendor_allow_list() {
    let report = analyze_startup(
        &[
            vendor_item(
                "hklm-1",
                StartupSource::RegistryRun {
                    hive: "HKLM".to_string(),
                    key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".to_string(),
                },
            ),
            vendor_item(
                "services-1",
                StartupSource::RegistryRun {
                    hive: "HKCU".to_string(),
                    key_path: "Software\\Policies\\Vendor\\Run".to_string(),
                },
            ),
        ],
        &StartupOptions::all(),
    );

    assert_eq!(report.items.len(), 2);
    for finding in &report.items {
        assert_eq!(
            finding.tier,
            StartupTier::OffLimits,
            "{} must not be safe to disable",
            finding.item.id
        );
    }
}

// If this regresses, a genuinely per-user vendor updater is reported as
// off-limits and the allow-list stops doing its job.
#[test]
fn a_per_user_vendor_updater_is_the_only_safe_to_disable_case() {
    let report = analyze_startup(&[vendor_item("user-1", user_run())], &StartupOptions::all());
    assert_eq!(report.items[0].tier, StartupTier::SafeToDisable);
    assert!(report.items[0].why.contains("did not verify the publisher"));
}

// If this regresses, an unclassifiable source is guessed into a tier.
#[test]
fn an_unrecognized_startup_source_is_off_limits() {
    let report = analyze_startup(
        &[item(
            "unknown-1",
            "Mystery",
            StartupSource::Unknown {
                raw: "some future launch mechanism".to_string(),
            },
            true,
        )],
        &StartupOptions::all(),
    );
    assert_eq!(report.items[0].tier, StartupTier::OffLimits);
    assert!(report.items[0].why.contains("could not classify"));
}

// If this regresses, a substring match lets "EvilGoogleUpdate" or
// "GoogleUpdateHelper" inherit the allow-list's blessing.
#[test]
fn the_vendor_allow_list_never_matches_a_substring() {
    assert_eq!(known_vendor_updater_pattern("EvilGoogleUpdate"), None);
    assert_eq!(known_vendor_updater_pattern("GoogleUpdateHelper"), None);
    assert_eq!(known_vendor_updater_pattern("com.google.update"), None);
    assert_eq!(
        known_vendor_updater_pattern("GoogleUpdate (32-bit)"),
        Some("googleupdate")
    );
    assert_eq!(
        known_vendor_updater_pattern("googleupdate.lnk"),
        Some("googleupdate")
    );
    assert_eq!(
        known_vendor_updater_pattern("  GOOGLEUPDATER  "),
        Some("googleupdater")
    );
}

// If this regresses, truncation shrinks the "how much starts at login" totals
// instead of only the displayed list.
#[test]
fn startup_truncation_keeps_the_inventory_totals_intact() {
    let items = [
        item("a", "Alpha", user_run(), true),
        item("b", "Beta", user_run(), true),
        item("c", "Gamma", user_run(), false),
    ];
    let report = analyze_startup(
        &items,
        &StartupOptions {
            include_disabled: true,
            max_items: Some(1),
        },
    );

    assert_eq!(report.items.len(), 1);
    assert_eq!(report.truncated, 2);
    assert!(report.is_truncated());
    assert_eq!(report.total_enabled, 2);
    assert_eq!(report.supplied_items(), 3);
    assert_eq!(report.by_source[0].source, SourceKind::RegistryRun);
    assert_eq!(report.by_source[0].total, 3);
    assert_eq!(report.by_source[0].enabled, 2);
}

// If this regresses, a `usize::MAX` cap is an allocation request, or a zero
// cap silently reports an empty inventory.
#[test]
fn startup_caps_are_exact_at_both_extremes() {
    let items = [
        item("a", "Alpha", user_run(), true),
        item("b", "Beta", user_run(), true),
    ];

    let none = analyze_startup(
        &items,
        &StartupOptions {
            include_disabled: true,
            max_items: Some(0),
        },
    );
    assert!(none.items.is_empty());
    assert_eq!(none.truncated, 2);
    assert_eq!(none.total_enabled, 2);

    let all = analyze_startup(
        &items,
        &StartupOptions {
            include_disabled: true,
            max_items: Some(usize::MAX),
        },
    );
    assert_eq!(all.items.len(), 2);
    assert_eq!(all.truncated, 0);
}

// If this regresses, disabled items are counted as running or disappear from
// the inventory.
#[test]
fn disabled_items_stay_in_the_inventory_but_out_of_the_enabled_count() {
    let report = analyze_startup(
        &[
            item("a", "Alpha", user_run(), false),
            item("b", "Beta", user_run(), true),
        ],
        &StartupOptions::enabled_only(),
    );

    assert_eq!(report.items.len(), 1);
    assert_eq!(report.total_enabled, 1);
    assert_eq!(report.supplied_items(), 2, "the inventory still holds both");
}

// If this regresses, the report depends on input order and two runs disagree.
#[test]
fn startup_findings_are_byte_identical_across_permutations() {
    let items = [
        item("a", "Alpha", user_run(), true),
        item("b", "Beta", user_run(), false),
        item(
            "c",
            "Gamma",
            StartupSource::LaunchAgent {
                label: "com.example.agent".to_string(),
            },
            true,
        ),
    ];
    let orders = [
        vec![items[0].clone(), items[1].clone(), items[2].clone()],
        vec![items[2].clone(), items[0].clone(), items[1].clone()],
        vec![items[1].clone(), items[2].clone(), items[0].clone()],
    ];

    let reports: Vec<String> = orders
        .iter()
        .map(|order| {
            serde_json::to_string(&analyze_startup(order, &StartupOptions::all()))
                .expect("the report serializes")
        })
        .collect();
    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
}

// If this regresses, two items that differ only in a source payload (the same
// shortcut in the per-user and all-users Startup folders) swap places between
// runs even though all other fields are equal.
// FIXED: StartupReport orders by the source payload as a final tie-break.
#[test]
fn startup_ties_are_broken_by_source_payload_not_by_input_order() {
    let user = vendor_item(
        "googleupdate.lnk",
        StartupSource::StartupFolder {
            scope: StartupFolderScope::User,
        },
    );
    let all_users = vendor_item(
        "googleupdate.lnk",
        StartupSource::StartupFolder {
            scope: StartupFolderScope::AllUsers,
        },
    );

    let forward = analyze_startup(&[user.clone(), all_users.clone()], &StartupOptions::all());
    let reverse = analyze_startup(&[all_users, user], &StartupOptions::all());

    assert_eq!(
        serde_json::to_string(&forward).expect("serializes"),
        serde_json::to_string(&reverse).expect("serializes"),
        "a report whose order depends on input order cannot be reviewed or diffed"
    );
}

// If this regresses, an empty inventory is described as complete or healthy.
#[test]
fn an_empty_startup_inventory_is_empty_not_complete() {
    let report = analyze_startup(&[], &StartupOptions::all());
    assert!(report.items.is_empty());
    assert!(report.by_source.is_empty());
    assert_eq!(report.total_enabled, 0);
    assert_eq!(report.supplied_items(), 0);
    assert!(!report.is_truncated());
    assert_eq!(report.count(StartupTier::SafeToDisable), 0);
}

// ---------------------------------------------------------------------------
// System report: unknown is not zero
// ---------------------------------------------------------------------------

// If this regresses, an uncollected memory total is rendered as "0 B" and the
// user believes the machine has no RAM.
#[test]
fn unknown_memory_is_never_rendered_as_zero() {
    let report = report_with(Some(MemoryInfo::unknown()), Vec::new());
    let text = summarize(&report);

    assert!(text.contains("RAM unknown"), "got {text}");
    assert!(!text.contains("RAM 0 B"), "got {text}");
    assert_eq!(
        report
            .memory
            .as_ref()
            .and_then(MemoryInfo::used_bytes_resolved),
        None
    );
    assert_eq!(
        report
            .memory
            .as_ref()
            .and_then(MemoryInfo::used_ratio_percent),
        None
    );
}

// If this regresses, a memory record whose used share is unknown reports 0 %,
// which reads as an idle machine.
#[test]
fn a_zero_total_yields_an_unknown_ratio_not_zero_percent() {
    let memory = MemoryInfo {
        total_bytes: Some(0),
        available_bytes: Some(0),
        used_bytes: None,
    };
    assert_eq!(memory.used_ratio_percent(), None);
    assert_eq!(memory.used_bytes_resolved(), Some(0));

    let over = MemoryInfo {
        total_bytes: Some(100),
        available_bytes: None,
        used_bytes: Some(1_000),
    };
    assert_eq!(
        over.used_ratio_percent(),
        Some(100),
        "a contradictory used>total is clamped, never reported above 100 %"
    );
}

// If this regresses, one unknown volume capacity is summed as zero and the
// total looks complete.
#[test]
fn storage_totals_are_unknown_if_any_capacity_is_unknown() {
    let report = report_with(
        None,
        vec![
            volume("a", Some(100), Some(50)),
            volume("b", None, Some(10)),
        ],
    );
    assert_eq!(report.known_storage_total_bytes(), None);
    let text = summarize(&report);
    assert!(text.contains("1 with unknown capacity"), "got {text}");
    assert!(!text.contains("125 B"), "got {text}");
}

// If this regresses, two maximum capacities wrap to a small total.
#[test]
fn storage_totals_saturate_at_u64_max() {
    let report = report_with(
        None,
        vec![
            volume("a", Some(u64::MAX), Some(0)),
            volume("b", Some(u64::MAX), Some(0)),
        ],
    );
    assert_eq!(report.known_storage_total_bytes(), Some(u64::MAX));
}

// If this regresses, an empty report claims to be a complete examination.
#[test]
fn empty_coverage_is_not_complete_and_says_why() {
    let coverage = ReportCoverage::complete(0);
    assert!(!coverage.is_complete());
    assert!(coverage.note().contains("no sources were examined"));

    let report = SystemReport {
        platform: PlatformInfo::new(Platform::Linux),
        memory: None,
        storage: Vec::new(),
        findings: Vec::new(),
        coverage,
    };
    assert!(!report.is_complete());
    let text = summarize(&report);
    assert!(text.contains("coverage 0 of 0"), "got {text}");
    assert!(text.contains("incomplete"), "got {text}");
}

// If this regresses, a failed source is hidden because the counters happen to
// sum to the total.
#[test]
fn a_failed_source_makes_coverage_incomplete() {
    let coverage = ReportCoverage::new(3, 2, 1, 0);
    assert!(!coverage.is_complete());
    assert!(coverage.is_consistent());
    assert!(coverage.note().contains("1 failed"));
}

// If this regresses, overflowing counters saturate into apparent agreement and
// the coverage line calls an impossible tally consistent.
#[test]
fn inconsistent_coverage_counters_are_reported_not_saturated_into_agreement() {
    let coverage = ReportCoverage::new(usize::MAX, usize::MAX, 1, 0);
    assert!(!coverage.is_consistent());
    assert_eq!(coverage.accounted_sources(), usize::MAX);
    let note = coverage.note();
    assert!(note.contains("inconsistent"), "got {note}");
    assert!(note.contains("incomplete"), "got {note}");
}

// If this regresses, findings are listed in input order and a critical finding
// can be shown below an informational one.
#[test]
fn findings_are_ordered_by_severity_then_code() {
    let report = SystemReport {
        platform: PlatformInfo::new(Platform::Windows),
        memory: None,
        storage: Vec::new(),
        findings: vec![
            DiagnosticFinding {
                code: "Z_INFO",
                severity: FindingSeverity::Info,
                summary: "info".to_string(),
                evidence: "e".to_string(),
            },
            DiagnosticFinding {
                code: "A_CRITICAL",
                severity: FindingSeverity::Critical,
                summary: "critical".to_string(),
                evidence: "e".to_string(),
            },
        ],
        coverage: ReportCoverage::complete(1),
    };

    let ordered: Vec<&str> = report
        .findings_sorted()
        .iter()
        .map(|finding| finding.code)
        .collect();
    assert_eq!(ordered, vec!["A_CRITICAL", "Z_INFO"]);
    assert_eq!(report.count_by_severity(FindingSeverity::Critical), 1);
    assert!(summarize(&report).contains("1 critical"));
}

// If this regresses, a whitespace-only OS field is rendered as a real value.
#[test]
fn blank_platform_fields_are_treated_as_unknown() {
    let mut platform = PlatformInfo::new(Platform::Windows);
    platform.os_name = Some("   ".to_string());
    platform.os_version = Some(String::new());
    let text = platform.describe();
    assert!(text.contains("OS details unknown"), "got {text}");
}

// ---------------------------------------------------------------------------
// Task inventory
// ---------------------------------------------------------------------------

// If this regresses, unattributed processes are folded into an application and
// the total stops describing the machine.
#[test]
fn unattributed_processes_are_counted_and_never_folded_into_an_app() {
    let inventory = TaskInventory::new(
        vec![
            TaskEntry {
                app_id: "app".to_string(),
                display_name: "App".to_string(),
                running: true,
                process_count: 2,
                owned_by_user: true,
            },
            TaskEntry {
                app_id: "   ".to_string(),
                display_name: "Blank id".to_string(),
                running: true,
                process_count: 3,
                owned_by_user: true,
            },
        ],
        5,
    );

    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.unattributed_processes, 8);
    assert_eq!(inventory.attributed_processes(), 2);
    assert_eq!(inventory.total_processes(), 10);
    assert!(!inventory.is_fully_attributed());
}

// If this regresses, a maximum unattributed count plus one process wraps.
#[test]
fn task_totals_saturate_at_usize_max() {
    let inventory = TaskInventory::new(
        vec![TaskEntry {
            app_id: "app".to_string(),
            display_name: "App".to_string(),
            running: true,
            process_count: 1,
            owned_by_user: true,
        }],
        usize::MAX,
    );
    assert_eq!(inventory.total_processes(), usize::MAX);
    assert_eq!(inventory.unattributed_processes, usize::MAX);

    let merged = TaskInventory::new(
        vec![
            TaskEntry {
                app_id: "app".to_string(),
                display_name: "App".to_string(),
                running: true,
                process_count: usize::MAX,
                owned_by_user: true,
            },
            TaskEntry {
                app_id: "app".to_string(),
                display_name: "App".to_string(),
                running: true,
                process_count: usize::MAX,
                owned_by_user: true,
            },
        ],
        0,
    );
    assert_eq!(merged.entries.len(), 1);
    assert_eq!(merged.entries[0].process_count, usize::MAX);
    assert_eq!(merged.total_processes(), usize::MAX);
}

// If this regresses, two applications sharing a display name are merged and
// one of them becomes invisible.
#[test]
fn distinct_app_ids_with_one_display_name_stay_separate() {
    let inventory = TaskInventory::new(
        vec![
            TaskEntry {
                app_id: "app.one".to_string(),
                display_name: "Same Name".to_string(),
                running: true,
                process_count: 1,
                owned_by_user: true,
            },
            TaskEntry {
                app_id: "app.two".to_string(),
                display_name: "Same Name".to_string(),
                running: false,
                process_count: 1,
                owned_by_user: false,
            },
        ],
        0,
    );

    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.running_apps(), 1);
    assert_eq!(
        inventory.entry("app.one").map(|entry| entry.running),
        Some(true)
    );
    assert_eq!(
        inventory.entry("app.two").map(|entry| entry.running),
        Some(false)
    );
    assert_eq!(inventory.entry("missing"), None);
}

// If this regresses, merging two rows for one app labels a partially-owned
// application as "yours".
#[test]
fn merging_owned_and_unowned_rows_keeps_ownership_conservative() {
    let inventory = TaskInventory::from_attributions(&[
        coresight_systools::ProcessAttribution {
            app_id: Some(coresight_systools::ApplicationId("app".to_string())),
            display_name: Some("App".to_string()),
            owned_by_user: true,
        },
        coresight_systools::ProcessAttribution {
            app_id: Some(coresight_systools::ApplicationId("app".to_string())),
            display_name: Some("App".to_string()),
            owned_by_user: false,
        },
    ]);

    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].process_count, 2);
    assert!(!inventory.entries[0].owned_by_user);
}

// ---------------------------------------------------------------------------
// Fixer: a skipped check is not a passing check
// ---------------------------------------------------------------------------

fn rule(code: &'static str, check_id: &'static str) -> DiagnosticRule {
    DiagnosticRule {
        code,
        check_id,
        severity: FindingSeverity::Attention,
        description: "fixture rule",
        recommended_interval_days: None,
    }
}

fn observation(check_id: &'static str, observed: Option<&str>, failed: bool) -> CheckObservation {
    CheckObservation {
        check_id,
        observed: observed.map(str::to_string),
        source: "fixture collector".to_string(),
        failed,
    }
}

// If this regresses, a rule whose check failed is counted as evaluated and the
// user reads a clean bill of health.
#[test]
fn a_failed_or_absent_check_is_skipped_not_passed() {
    let facts = SystemFacts {
        checks: vec![
            observation("disk_free", Some("1 GiB free"), true),
            observation("backup_last_success", None, false),
            observation("os_build_age", Some("   "), false),
        ],
    };
    let rules = [
        rule("CORE_DISK_FREE_LOW", "disk_free"),
        rule("CORE_BACKUP_NOT_OBSERVED", "backup_last_success"),
        rule("CORE_OS_BUILD_AGED", "os_build_age"),
        rule("CORE_RESTART_PENDING", "reboot_pending"),
    ];
    let diagnostic = run(&facts, &rules);

    assert!(diagnostic.findings.is_empty());
    assert_eq!(diagnostic.skipped, 4);
    assert_eq!(diagnostic.evaluated, 0);
    assert_eq!(diagnostic.accounted_rules(), 4);
    assert!(!diagnostic.coverage_note().contains("none skipped"));
    assert!(diagnostic.coverage_note().contains("skipped"));
}

// If this regresses, a failed observation for one check masks a usable
// observation of the same check.
#[test]
fn a_failed_observation_never_masks_a_usable_one() {
    let facts = SystemFacts {
        checks: vec![
            observation("disk_free", Some("0 bytes free"), true),
            observation("disk_free", Some("4.2 GiB free of 500 GiB"), false),
        ],
    };
    let diagnostic = run(&facts, &[rule("CORE_DISK_FREE_LOW", "disk_free")]);

    assert_eq!(diagnostic.findings.len(), 1);
    assert_eq!(diagnostic.findings[0].observed, "4.2 GiB free of 500 GiB");
    assert_eq!(diagnostic.skipped, 0);
    assert_eq!(diagnostic.evaluated, 1);
}

// If this regresses, evidence text is rewritten or a blank observation is
// shown as proof.
#[test]
fn fixer_quotes_evidence_verbatim_and_never_invents_it() {
    let facts = SystemFacts {
        checks: vec![observation("disk_free", Some("  3 GiB free  "), false)],
    };
    let diagnostic = run(&facts, &[rule("CORE_DISK_FREE_LOW", "disk_free")]);
    assert_eq!(diagnostic.findings[0].observed, "3 GiB free");

    let blank = SystemFacts {
        checks: vec![observation("disk_free", Some("   "), false)],
    };
    assert_eq!(
        run(&blank, &[rule("CORE_DISK_FREE_LOW", "disk_free")]).skipped,
        1
    );
}

// If this regresses, the diagnostic run depends on rule or check order.
#[test]
fn fixer_run_is_byte_identical_across_permutations() {
    let rules = [
        rule("B_RULE", "b_check"),
        rule("A_RULE", "a_check"),
        rule("C_RULE", "c_check"),
    ];
    let checks = [
        observation("a_check", Some("a evidence"), false),
        observation("b_check", Some("b evidence"), false),
    ];

    let forward = run(
        &SystemFacts {
            checks: checks.to_vec(),
        },
        &rules,
    );
    let reverse = run(
        &SystemFacts {
            checks: vec![checks[1].clone(), checks[0].clone()],
        },
        &[rules[2].clone(), rules[0].clone(), rules[1].clone()],
    );

    assert_eq!(
        serde_json::to_string(&forward).expect("serializes"),
        serde_json::to_string(&reverse).expect("serializes")
    );
    assert_eq!(forward.skipped, 1);
}

// If this regresses, a rule with no evidence at all is reported as healthy.
#[test]
fn a_run_with_no_facts_admits_it_evaluated_nothing() {
    let diagnostic = run(&SystemFacts::empty(), &default_rules());
    assert!(diagnostic.findings.is_empty());
    assert_eq!(diagnostic.skipped, default_rules().len());
    assert_eq!(diagnostic.evaluated, 0);
    let note = diagnostic.coverage_note();
    assert!(note.contains("0 evaluated"), "got {note}");
    assert!(note.contains("skipped"), "got {note}");
    assert!(
        !note.contains("none skipped"),
        "a skipped check must never read as a passing one: {note}"
    );

    let nothing_supplied = run(&SystemFacts::empty(), &[]);
    assert!(nothing_supplied
        .coverage_note()
        .contains("nothing was evaluated"));
}

// If this regresses, the repair plan proposes a system change instead of an
// inspection.
#[test]
fn a_repair_plan_never_proposes_a_system_change() {
    let facts = SystemFacts {
        checks: vec![observation("disk_free", Some("1 GiB free"), false)],
    };
    let diagnostic = run(&facts, &[rule("CORE_DISK_FREE_LOW", "disk_free")]);
    let plan = plan_repairs(&diagnostic);

    assert_eq!(plan.steps.len(), 1);
    for step in &plan.steps {
        assert_eq!(step.action, RepairAction::InspectOnly);
        assert!(!step.action.changes_system());
        assert!(step.reversible);
        assert!(!step.requires_privilege);
        assert!(step.description.contains("changes nothing"));
    }
    assert!(plan.note.contains("performs no repairs"));
}

// If this regresses, an unknown update state is rendered as healthy, or a
// missing error code is invented.
#[test]
fn update_findings_never_upgrade_unknown_or_invent_an_error_code() {
    let findings = update_findings(&[
        UpdateCheck {
            component: "Update".to_string(),
            state: UpdateState::Unknown,
            evidence: "query returned nothing".to_string(),
            source: "fixture".to_string(),
        },
        UpdateCheck {
            component: "Defender".to_string(),
            state: UpdateState::Error { code: None },
            evidence: "error 0x0".to_string(),
            source: "fixture".to_string(),
        },
    ]);

    let unknown = findings
        .iter()
        .find(|finding| finding.component == "Update")
        .expect("one row per component");
    assert_eq!(unknown.code, "UPDATE_UNKNOWN");
    assert_eq!(unknown.state, UpdateState::Unknown);

    let error = findings
        .iter()
        .find(|finding| finding.component == "Defender")
        .expect("one row per component");
    assert_eq!(error.code, "UPDATE_ERROR");
    assert!(error.summary.contains("did not report an error code"));
    assert!(!error.summary.contains("0x0"));
}

// If this regresses, an empty or whitespace component name is dropped or
// presented as a named component.
#[test]
fn an_unnamed_update_component_is_described_as_unnamed() {
    let findings = update_findings(&[UpdateCheck {
        component: "   ".to_string(),
        state: UpdateState::Stopped,
        evidence: "service status: stopped".to_string(),
        source: "fixture".to_string(),
    }]);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].component.is_empty());
    assert!(findings[0].summary.contains("Unnamed update component"));
}

// If this regresses, healthy components are omitted and "healthy" becomes
// indistinguishable from "never examined".
#[test]
fn healthy_update_components_are_reported_too() {
    let findings = update_findings(&[UpdateCheck {
        component: "Windows Update".to_string(),
        state: UpdateState::Healthy,
        evidence: "service running".to_string(),
        source: "fixture".to_string(),
    }]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "UPDATE_HEALTHY");
}
