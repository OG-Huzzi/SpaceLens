//! Behaviour and contract tests for the ten aggregate view commands
//! (`getGlobalStatus` … `getPrivacySummary`).
//!
//! All fixtures are synthetic and in memory. They are built with
//! `coresight_history::SnapshotBuilder` plus `coresight_engine` entry structs,
//! exactly like `crates/coresight-cleanup/tests/cleanup_tests.rs`. No file is
//! read, created, moved, or deleted.

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use common::{
    at, classified, classified_sub, dir, drive, file, permission_denied, DRIVE_ID, SCAN_ID,
};
use coresight_classifier::{Category, Subcategory};
use coresight_engine::FsEntry;
use coresight_history::{ClassificationRef, RunId, Snapshot, SnapshotBuilder};
use coresight_ipc::{
    codes, dispatch, Command, Envelope, GetBigFileReportRequest, GetBlockListPlanRequest,
    GetCleanupRequest, GetDnsRankingRequest, GetDuplicateReportRequest, GetGlobalStatusRequest,
    GetHomeSummaryRequest, GetPrivacySummaryRequest, GetStartupReportRequest, GetStorageRequest,
    Response, ScanRecord, ScanStatus, Session,
};
use coresight_systools::startup::{StartupItem, StartupSource};
use coresight_webtools::{
    plan_block_list, rank_dns, summarize_privacy, BlockListOptions, BlockRule, DnsCandidate,
    DnsMeasurement, DnsPrivacyPolicy, DnsRankOptions, DomainCategory, PrivacyFinding,
    PrivacyFindingKind, RuleSource,
};
use serde_json::Value;

const T0: u64 = 1_700_000_000;
const T1: u64 = T0 + 3_600;
const MIB: u64 = 1024 * 1024;
const BIG_FILE_SIZE: u64 = 200 * MIB;
const BIG_FILE_AGE_DAYS: u64 = 10;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn report_file(path: &str, size: u64, category: Category) -> (FsEntry, Option<ClassificationRef>) {
    (file(path, size), classified(category))
}

/// The synthetic snapshot used by the value tests.
///
/// Nine files, 209_971_200 known bytes. Two byte-identical files share a
/// verified SHA-256 hash, and one 200 MiB file is old enough to be a big-file
/// candidate. `cache_bytes` lets the delta tests change one bucket without
/// changing anything else.
fn report_snapshot(cache_bytes: u64) -> Snapshot {
    let mut builder = SnapshotBuilder::new();

    let big_modified = UNIX_EPOCH + Duration::from_secs(T0 - BIG_FILE_AGE_DAYS * 86_400);
    let mut big = file("/scan/media/big.mov", BIG_FILE_SIZE);
    big.modified = Some(big_modified);

    // Distinct proven object identities: the two copies are separate objects,
    // so the duplicate finder may count one recoverable copy.
    let mut dup_a = file("/scan/docs/dup-a.bin", 10_000);
    dup_a.inode = Some(101);
    let mut dup_b = file("/scan/docs/dup-b.bin", 10_000);
    dup_b.inode = Some(102);

    let entries: Vec<(FsEntry, Option<ClassificationRef>)> = vec![
        report_file("/scan/cache/a.bin", cache_bytes, Category::Cache),
        report_file("/scan/temp/b.tmp", 20_000, Category::TemporaryData),
        (
            file("/scan/logs/app.log", 5_000),
            classified_sub(Category::Logs, Subcategory::LogFile),
        ),
        report_file("/scan/docs/report.pdf", 100_000, Category::Documents),
        (
            file("/scan/downloads/pack.zip", 80_000),
            classified_sub(Category::Downloads, Subcategory::Archive),
        ),
        (file("/scan/misc/note.dat", 1_000), None),
        (big, classified(Category::Video)),
        (dup_a, classified(Category::Documents)),
        (dup_b, classified(Category::Documents)),
    ];

    for (entry, classification) in entries {
        builder.push_entry(&entry, classification);
    }
    builder.set_content(
        Path::new("/scan/docs/dup-a.bin"),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
    );
    builder.set_content(
        Path::new("/scan/docs/dup-b.bin"),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
    );
    builder
        .build(RunId("run".to_string()))
        .expect("synthetic snapshot builds")
}

/// A snapshot with one unknown-size directory and one unreadable file, so the
/// honesty rules can be exercised.
fn partial_snapshot() -> Snapshot {
    let mut builder = SnapshotBuilder::new();
    let entries = vec![
        report_file("/scan/cache/a.bin", 30_000, Category::Cache),
        (dir("/scan/cache/sub"), classified(Category::Cache)),
        (permission_denied("/scan/misc/locked"), None),
    ];
    for (entry, classification) in entries {
        builder.push_entry(&entry, classification);
    }
    builder
        .build(RunId("run".to_string()))
        .expect("partial snapshot builds")
}

fn register_drive_and_scan(
    session: &mut Session,
    scan_id: &str,
    snapshot: Snapshot,
    completed_at: u64,
) {
    session.register_drive(drive());
    session
        .register_scan(ScanRecord {
            scan_id: scan_id.to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot,
            completed_at: Some(at(completed_at)),
            status: ScanStatus::Completed,
        })
        .expect("fixture scan registers");
}

fn register_supplementary_reports(session: &mut Session) {
    session.register_startup_inventory(vec![
        StartupItem {
            id: "startup-updater".to_string(),
            name: "GoogleUpdate".to_string(),
            command: Some("GoogleUpdate.exe /ua".to_string()),
            source: StartupSource::RegistryRun {
                hive: "HKCU".to_string(),
                key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".to_string(),
            },
            enabled: true,
            owner: Some("Local user".to_string()),
        },
        StartupItem {
            id: "startup-daemon".to_string(),
            name: "Security agent".to_string(),
            command: None,
            source: StartupSource::LaunchDaemon {
                label: "com.example.security".to_string(),
            },
            enabled: true,
            owner: Some("System".to_string()),
        },
    ]);

    let candidates = vec![
        DnsCandidate {
            name: "Resolver A".to_string(),
            ipv4: vec!["192.0.2.1".to_string()],
            ipv6: Vec::new(),
            operator: Some("Example privacy cooperative".to_string()),
            privacy_policy: DnsPrivacyPolicy::DocumentedNoLogging,
            supports_dnssec: true,
            supports_filtering: false,
        },
        DnsCandidate {
            name: "Unmeasured resolver".to_string(),
            ipv4: Vec::new(),
            ipv6: Vec::new(),
            operator: None,
            privacy_policy: DnsPrivacyPolicy::Unknown,
            supports_dnssec: false,
            supports_filtering: false,
        },
    ];
    let measurements = vec![DnsMeasurement {
        candidate_name: "Resolver A".to_string(),
        median_latency_ms: 20.0,
        p95_latency_ms: 40.0,
        failure_rate: 0.0,
        samples: 20,
    }];
    session.register_dns_ranking(rank_dns(
        &candidates,
        &measurements,
        &DnsRankOptions::default(),
    ));

    let rules = vec![
        BlockRule {
            domain: "Ads.Example.Test".to_string(),
            category: DomainCategory::Advertising,
            source: RuleSource::UserDefined,
            reason: "advertising".to_string(),
        },
        BlockRule {
            domain: "ads.example.test".to_string(),
            category: DomainCategory::Tracking,
            source: RuleSource::UserDefined,
            reason: "duplicate rule".to_string(),
        },
        BlockRule {
            domain: "not a domain".to_string(),
            category: DomainCategory::Advertising,
            source: RuleSource::UserDefined,
            reason: "invalid".to_string(),
        },
        BlockRule {
            domain: "bank.example.test".to_string(),
            category: DomainCategory::Tracking,
            source: RuleSource::UserDefined,
            reason: "whitelisted".to_string(),
        },
    ];
    let options = BlockListOptions {
        whitelist: vec!["bank.example.test".to_string()],
        include_categories: Vec::new(),
        max_entries: 0,
    };
    session.register_block_list_plan(plan_block_list(&rules, &options));

    session.register_privacy_findings(vec![
        PrivacyFinding {
            kind: PrivacyFindingKind::TrackingCookie,
            count: 24,
            estimated_bytes: Some(1_240_000),
            detail: "Persistent cookies from analytics domains.".to_string(),
        },
        PrivacyFinding {
            kind: PrivacyFindingKind::SessionCookie,
            count: 8,
            estimated_bytes: None,
            detail: "Removing these would sign you out of sites.".to_string(),
        },
        PrivacyFinding {
            kind: PrivacyFindingKind::CacheEntry,
            count: 1_842,
            estimated_bytes: Some(84_200_000),
            detail: "Cached resources; evidence only.".to_string(),
        },
    ]);
}

fn report_session() -> Session {
    let mut session = Session::new();
    register_drive_and_scan(&mut session, SCAN_ID, report_snapshot(30_000), T0);
    register_supplementary_reports(&mut session);
    session
}

fn two_scan_session() -> Session {
    let mut session = Session::new();
    session.register_drive(drive());
    session
        .register_scan(ScanRecord {
            scan_id: "scan-1".to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot: report_snapshot(30_000),
            completed_at: Some(at(T0)),
            status: ScanStatus::Completed,
        })
        .expect("first scan");
    session
        .register_scan(ScanRecord {
            scan_id: "scan-2".to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot: report_snapshot(40_000),
            completed_at: Some(at(T1)),
            status: ScanStatus::Completed,
        })
        .expect("second scan");
    register_supplementary_reports(&mut session);
    session
}

fn partial_session() -> Session {
    let mut session = Session::new();
    register_drive_and_scan(&mut session, SCAN_ID, partial_snapshot(), T0);
    session
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn every_new_command() -> Vec<Command> {
    vec![
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
        Command::GetHomeSummary(GetHomeSummaryRequest {}),
        Command::GetStorage(GetStorageRequest {}),
        Command::GetCleanup(GetCleanupRequest {}),
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
        Command::GetBigFileReport(GetBigFileReportRequest {}),
        Command::GetStartupReport(GetStartupReportRequest {}),
        Command::GetDnsRanking(GetDnsRankingRequest {}),
        Command::GetBlockListPlan(GetBlockListPlanRequest {}),
        Command::GetPrivacySummary(GetPrivacySummaryRequest {}),
    ]
}

fn ok(session: &mut Session, command: Command) -> Response {
    dispatch(session, command)
        .into_result()
        .expect("command succeeds on the populated fixture")
}

fn payload(response: &Response) -> Value {
    let value = serde_json::to_value(response).expect("response serializes");
    value
        .get("payload")
        .cloned()
        .expect("every success payload has a payload")
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("expected a JSON object")
        .keys()
        .cloned()
        .collect()
}

fn assert_keys(value: &Value, expected: &[&str], what: &str) {
    let actual = keys(value);
    let expected: BTreeSet<String> = expected.iter().map(|key| (*key).to_string()).collect();
    assert_eq!(actual, expected, "{what} keys differ from src/api.ts");
}

fn assert_camel_case(value: &Value, what: &str) {
    let text = serde_json::to_string(value).expect("serializes");
    for snake in [
        "\"contract_version\"",
        "\"simulated_data\"",
        "\"scan_id\"",
        "\"files_seen\"",
        "\"bytes_seen\"",
        "\"total_observed_bytes\"",
        "\"duplicate_report\"",
        "\"big_file_report\"",
        "\"safe_reclaimable_bytes\"",
        "\"dns_ranking\"",
        "\"block_list\"",
        "\"total_count\"",
        "\"estimated_bytes\"",
    ] {
        assert!(
            !text.contains(snake),
            "{what} leaked a snake_case key {snake}: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. Every command dispatches without panicking
// ---------------------------------------------------------------------------

#[test]
fn every_new_command_dispatches_without_panicking() {
    let mut session = report_session();
    for command in every_new_command() {
        let name = command.name().to_string();
        let envelope = dispatch(&mut session, command);
        assert!(
            envelope.is_ok() ^ envelope.is_err(),
            "{name} must return exactly one envelope side"
        );
        assert!(envelope.is_ok(), "{name} must be served by the fixture");
    }
    assert_eq!(Command::ALL_NAMES_V1.len(), 20);
}

// ---------------------------------------------------------------------------
// 2. Every command returns a typed error when the session has no data
// ---------------------------------------------------------------------------

#[test]
fn every_new_command_returns_a_typed_error_without_session_data() {
    let mut session = Session::new();
    for command in every_new_command() {
        let name = command.name().to_string();
        let envelope = dispatch(&mut session, command);
        assert!(
            envelope.ok_value().is_none(),
            "{name} must not fake success"
        );
        let error = envelope
            .error()
            .unwrap_or_else(|| panic!("{name} must return a typed error"));
        assert_eq!(error.code, codes::SESSION_UNAVAILABLE, "{name}");
        assert!(!error.message.is_empty());
        assert!(
            !error.message.contains("0 bytes"),
            "{name} must never present absent data as zero"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Values from the synthetic snapshot
// ---------------------------------------------------------------------------

#[test]
fn global_status_reports_the_latest_scan() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    );
    let Response::GlobalStatus(status) = response else {
        panic!("expected global status");
    };
    assert_eq!(status.contract_version, "v1");
    assert!(!status.simulated_data);
    assert_eq!(status.scan.scan_id.as_deref(), Some(SCAN_ID));
    assert_eq!(status.scan.state, coresight_ipc::ScanStateView::Complete);
    assert_eq!(
        status.scan.completed_at.as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
    assert_eq!(status.scan.files_seen, Some(9));
    assert_eq!(status.scan.bytes_seen, Some(209_971_200));
    assert_eq!(
        status.scan.coverage.completeness,
        coresight_ipc::DataCompleteness::Complete
    );
    // Not recorded by ScanRecord: never fabricated.
    assert!(status.scan.started_at.is_none());
    assert!(status.scan.current_area.is_none());
    assert!(status.scan.eta_seconds.is_none());
}

#[test]
fn global_status_reports_a_running_scan_as_scanning_without_totals() {
    let mut session = common::session_with_snapshot(common::sample_snapshot(), ScanStatus::Running);
    let response = ok(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    );
    let Response::GlobalStatus(status) = response else {
        panic!("expected global status");
    };
    assert_eq!(status.scan.state, coresight_ipc::ScanStateView::Scanning);
    assert_eq!(status.scan.files_seen, Some(6));
    assert_eq!(
        status.scan.bytes_seen, None,
        "a running scan's byte count is progress, not a total"
    );
    assert_eq!(
        status.scan.coverage.completeness,
        coresight_ipc::DataCompleteness::Unknown
    );
}

#[test]
fn home_summary_reports_reclaimable_bytes_drives_and_top_categories() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetHomeSummary(GetHomeSummaryRequest {}),
    );
    let Response::HomeSummary(summary) = response else {
        panic!("expected home summary");
    };
    assert_eq!(summary.generated_at, "2023-11-14T22:13:20Z");
    assert_eq!(
        summary.last_scan_at.as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
    assert_eq!(summary.safe_reclaimable_bytes, Some(55_000));
    assert_eq!(summary.review_reclaimable_bytes, Some(80_000));
    assert_eq!(summary.actionable_opportunity_count, Some(4));
    assert_eq!(summary.drives.len(), 1);
    assert_eq!(summary.drives[0].name, "System");
    assert_eq!(summary.drives[0].total_bytes, Some(1_000_000_000));
    assert_eq!(summary.drives[0].used_bytes, Some(500_000_000));
    assert_eq!(summary.drives[0].used_share, Some(0.5));
    assert_eq!(
        summary.drives[0].health,
        coresight_ipc::DriveHealthView::Unknown
    );
    let ids: Vec<&str> = summary
        .top_categories
        .iter()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(ids, ["DOCUMENTS", "VIDEO", "DOWNLOADS"]);
    assert!(summary.change_since_last_scan.is_none());
}

#[test]
fn home_summary_delta_uses_the_previous_full_scope_scan() {
    let mut session = two_scan_session();
    let response = ok(
        &mut session,
        Command::GetHomeSummary(GetHomeSummaryRequest {}),
    );
    let Response::HomeSummary(summary) = response else {
        panic!("expected home summary");
    };
    let change = summary.change_since_last_scan.expect("two scans compare");
    assert_eq!(change.from_snapshot_id.as_deref(), Some("scan-1"));
    assert_eq!(change.to_snapshot_id.as_deref(), Some("scan-2"));
    assert_eq!(change.total_bytes_delta, Some(10_000));
    let cache = change
        .category_deltas
        .iter()
        .find(|row| row.category_id == "CACHE")
        .expect("cache delta");
    assert_eq!(cache.bytes_delta, Some(10_000));
    assert_eq!(cache.old_bytes, Some(30_000));
    assert_eq!(cache.new_bytes, Some(40_000));
    assert!(!cache.partial);
}

#[test]
fn storage_reports_category_totals_and_nested_reports() {
    let mut session = report_session();
    let response = ok(&mut session, Command::GetStorage(GetStorageRequest {}));
    let Response::Storage(storage) = response else {
        panic!("expected storage");
    };
    assert_eq!(storage.categories.len(), Category::ALL.len());
    assert_eq!(storage.total_observed_bytes, Some(209_971_200));
    assert_eq!(storage.total_observed_items, Some(9));
    assert_eq!(
        storage.coverage.completeness,
        coresight_ipc::DataCompleteness::Complete
    );

    let cache = storage
        .categories
        .iter()
        .find(|row| row.id == "CACHE")
        .expect("cache row");
    assert_eq!(cache.bytes, Some(30_000));
    assert_eq!(cache.entries, Some(1));
    assert_eq!(cache.entries_without_size, Some(0));
    assert_eq!(cache.observation_errors, Some(0));
    assert_eq!(cache.sample_locations.len(), 1);

    let logs = storage
        .categories
        .iter()
        .find(|row| row.id == "LOGS")
        .expect("logs row");
    assert_eq!(logs.children.len(), 1, "the stored LOG_FILE subcategory");
    assert_eq!(logs.children[0].id, "LOG_FILE");

    assert_eq!(storage.duplicate_report.groups.len(), 1);
    assert_eq!(
        storage.duplicate_report.total_recoverable_bytes,
        Some(10_000)
    );
    assert_eq!(storage.big_file_report.files.len(), 1);
    assert_eq!(
        storage.big_file_report.files[0].size_bytes,
        Some(BIG_FILE_SIZE)
    );
}

#[test]
fn cleanup_reports_actionable_items_and_off_limits_rows() {
    let mut session = report_session();
    let response = ok(&mut session, Command::GetCleanup(GetCleanupRequest {}));
    let Response::Cleanup(cleanup) = response else {
        panic!("expected cleanup");
    };
    assert_eq!(cleanup.scan_id.as_deref(), Some(SCAN_ID));
    assert_eq!(cleanup.items.len(), 4);
    assert_eq!(cleanup.safe_recoverable_bytes, Some(55_000));
    assert_eq!(cleanup.review_recoverable_bytes, Some(80_000));
    assert!(cleanup.items.iter().all(|item| item.previewable));
    assert!(cleanup
        .items
        .iter()
        .any(|item| item.category_id == "DOWNLOADS"
            && item.tier == coresight_ipc::CleanupTier::Review));
    let ids: Vec<&str> = cleanup
        .off_limits
        .iter()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(ids, ["off-documents", "off-video", "off-unknown"]);
    assert!(cleanup.off_limits.iter().all(|row| !row.previewable));
    assert!(cleanup
        .off_limits
        .iter()
        .all(|row| row.recoverable_bytes.is_none()));
}

#[test]
fn duplicate_report_uses_verified_content_only() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
    );
    let Response::DuplicateReport(report) = response else {
        panic!("expected duplicate report");
    };
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.total_groups, Some(1));
    assert_eq!(report.total_recoverable_bytes, Some(10_000));
    assert_eq!(report.unverified_entries, Some(7));
    assert_eq!(report.unknown_size_entries, Some(0));
    assert_eq!(report.truncated_groups, Some(0));
    let group = &report.groups[0];
    assert_eq!(group.size_bytes, Some(10_000));
    assert_eq!(group.members.len(), 2);
    assert!(group.hash_verified);
    assert!(group.members.iter().all(|member| !member.is_hardlink_alias));
}

#[test]
fn big_file_report_reports_age_from_the_scan_instant() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetBigFileReport(GetBigFileReportRequest {}),
    );
    let Response::BigFileReport(report) = response else {
        panic!("expected big-file report");
    };
    assert_eq!(report.threshold_bytes, Some(100 * MIB));
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].display_path, "/scan/media/big.mov");
    assert_eq!(report.files[0].size_bytes, Some(BIG_FILE_SIZE));
    assert_eq!(report.files[0].age_days, Some(BIG_FILE_AGE_DAYS));
    assert_eq!(report.files[0].category_name.as_deref(), Some("Video"));
    assert_eq!(report.unknown_age_entries, Some(0));
    assert_eq!(report.truncated, Some(0));
}

#[test]
fn startup_report_is_analysed_from_the_registered_inventory() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetStartupReport(GetStartupReportRequest {}),
    );
    let Response::StartupReport(report) = response else {
        panic!("expected startup report");
    };
    assert_eq!(report.items.len(), 2);
    assert_eq!(report.total_enabled, Some(2));
    assert_eq!(report.truncated, Some(0));
    // Off-limits items are published first (systools canonical order).
    assert_eq!(report.items[0].name, "Security agent");
    assert_eq!(
        report.items[0].tier,
        coresight_ipc::StartupTierView::OffLimits
    );
    assert!(report.items[0].command_label.is_none());
    let updater = report
        .items
        .iter()
        .find(|item| item.name == "GoogleUpdate")
        .expect("updater");
    assert_eq!(updater.tier, coresight_ipc::StartupTierView::SafeToDisable);
    assert_eq!(updater.source_label, "Registry Run key");
}

#[test]
fn dns_ranking_is_projected_from_the_registered_ranking() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetDnsRanking(GetDnsRankingRequest {}),
    );
    let Response::DnsRanking(ranking) = response else {
        panic!("expected DNS ranking");
    };
    assert_eq!(ranking.ranked.len(), 1);
    assert_eq!(ranking.ranked[0].name, "Resolver A");
    // 20 ms median scores 100; 40 ms p95 scores 93; the averaged latency
    // component is 97, giving 98.8 → 98 weighted points.
    assert_eq!(ranking.ranked[0].score, Some(98));
    assert_eq!(ranking.ranked[0].samples, Some(20));
    assert_eq!(
        ranking.ranked[0].privacy_policy,
        coresight_ipc::DnsPrivacyPolicyView::DocumentedNoLogging
    );
    assert_eq!(ranking.excluded.len(), 1);
    assert_eq!(
        ranking.excluded[0].reason,
        coresight_ipc::DnsExclusionReasonView::NotMeasured
    );
    assert!(ranking.note.contains("never switches DNS"));
}

#[test]
fn block_list_plan_is_reversible_and_reports_every_rejection() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetBlockListPlan(GetBlockListPlanRequest {}),
    );
    let Response::BlockListPlan(plan) = response else {
        panic!("expected block list plan");
    };
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].domain, "ads.example.test");
    assert_eq!(plan.entries[0].category, "TRACKING");
    assert_eq!(plan.entries[0].ipv4, "127.0.0.1");
    assert_eq!(plan.entries[0].ipv6, "::1");
    assert_eq!(plan.total, Some(1));
    assert_eq!(plan.duplicates_removed, Some(1));
    assert_eq!(plan.invalid_removed.len(), 1);
    assert_eq!(plan.whitelisted, ["bank.example.test"]);
    assert_eq!(plan.truncated, Some(0));
    assert!(plan.reversible);
    assert!(plan.note.contains("reversible"));
}

#[test]
fn privacy_summary_keeps_unknown_byte_totals_unknown() {
    let mut session = report_session();
    let response = ok(
        &mut session,
        Command::GetPrivacySummary(GetPrivacySummaryRequest {}),
    );
    let Response::PrivacySummary(summary) = response else {
        panic!("expected privacy summary");
    };
    assert_eq!(summary.findings.len(), 3);
    assert_eq!(summary.total_count, Some(1_874));
    assert_eq!(
        summary.total_estimated_bytes, None,
        "one finding has no size estimate, so the total is unknown"
    );
    assert!(!summary.has_unknown_kinds);
    let session_cookie = summary
        .findings
        .iter()
        .find(|finding| finding.kind == coresight_ipc::PrivacyFindingKindView::SessionCookie)
        .expect("session cookie row");
    assert_eq!(session_cookie.estimated_bytes, None);
}

// ---------------------------------------------------------------------------
// 4. Partial scans must not be promoted to complete totals
// ---------------------------------------------------------------------------

#[test]
fn partial_scan_never_reports_a_partial_total_as_whole() {
    let mut session = partial_session();

    let global = ok(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    );
    let Response::GlobalStatus(global) = global else {
        panic!("expected global status");
    };
    assert_eq!(global.scan.state, coresight_ipc::ScanStateView::Partial);
    assert_eq!(global.scan.files_seen, Some(3));
    assert_eq!(
        global.scan.bytes_seen, None,
        "a partial byte total is not a total"
    );

    let storage = ok(&mut session, Command::GetStorage(GetStorageRequest {}));
    let Response::Storage(storage) = storage else {
        panic!("expected storage");
    };
    assert_eq!(storage.total_observed_bytes, None);
    assert_eq!(
        storage.coverage.completeness,
        coresight_ipc::DataCompleteness::Partial
    );
    let cache = storage
        .categories
        .iter()
        .find(|row| row.id == "CACHE")
        .expect("cache row");
    assert_eq!(cache.bytes, None, "unknown-size entry in the bucket");
    assert_eq!(cache.entries_without_size, Some(1));
    let unknown = storage
        .categories
        .iter()
        .find(|row| row.id == "UNKNOWN")
        .expect("unknown row");
    assert_eq!(unknown.observation_errors, Some(1));
    assert_eq!(unknown.bytes, None);

    let cleanup = ok(&mut session, Command::GetCleanup(GetCleanupRequest {}));
    let Response::Cleanup(cleanup) = cleanup else {
        panic!("expected cleanup");
    };
    assert_eq!(cleanup.safe_recoverable_bytes, None);
    let cache_item = cleanup
        .items
        .iter()
        .find(|item| item.category_id == "CACHE")
        .expect("cache item");
    assert_eq!(cache_item.observed_bytes, None);
    assert_eq!(cache_item.recoverable_bytes, None);

    let home = ok(
        &mut session,
        Command::GetHomeSummary(GetHomeSummaryRequest {}),
    );
    let Response::HomeSummary(home) = home else {
        panic!("expected home summary");
    };
    assert_eq!(home.safe_reclaimable_bytes, None);
}

// ---------------------------------------------------------------------------
// 5. camelCase + serde round-trip + determinism
// ---------------------------------------------------------------------------

#[test]
fn every_new_response_round_trips_and_uses_camel_case() {
    let mut session = report_session();
    for command in every_new_command() {
        let name = command.name().to_string();
        let response = ok(&mut session, command);
        let text = serde_json::to_string(&response).expect("serializes");
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["kind"].as_str(), Some(name.as_str()));
        assert_camel_case(&value["payload"], &name);
        let parsed: Response = serde_json::from_str(&text).expect("response parses back");
        assert_eq!(parsed, response, "{name} round-trip changed the payload");
    }
}

#[test]
fn new_commands_are_deterministic_for_the_same_session() {
    let commands: Vec<&str> = Command::ALL_NAMES_V1[10..].to_vec();
    let mut first = report_session();
    let mut second = report_session();
    for command_name in commands {
        let first_value = serde_json::to_value(dispatch_by_name(&mut first, command_name))
            .expect("first serializes");
        let second_value = serde_json::to_value(dispatch_by_name(&mut second, command_name))
            .expect("second serializes");
        assert_eq!(
            first_value, second_value,
            "{command_name} must serialize byte-identically for an identical session"
        );
    }
    // And re-dispatching within one session changes nothing either.
    let value_before =
        serde_json::to_value(dispatch_by_name(&mut first, "coresight.v1.getStorage"))
            .expect("serializes");
    let value_after = serde_json::to_value(dispatch_by_name(&mut first, "coresight.v1.getStorage"))
        .expect("serializes");
    assert_eq!(value_before, value_after);
}

fn dispatch_by_name(session: &mut Session, name: &str) -> Envelope<Response> {
    let command = Command::from_name(name, serde_json::json!({}))
        .unwrap_or_else(|error| panic!("command {name} parses: {error:?}"));
    dispatch(session, command)
}

// ---------------------------------------------------------------------------
// 6. JSON shape parity with src/api.ts
// ---------------------------------------------------------------------------

#[test]
fn new_response_shapes_match_the_src_api_ts_interfaces() {
    let mut session = two_scan_session();

    // GlobalStatus + ScanStatus + DataCoverage
    let global = payload(&ok(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    ));
    assert_keys(
        &global,
        &["contractVersion", "scan", "simulatedData"],
        "GlobalStatus",
    );
    assert_keys(
        &global["scan"],
        &[
            "scanId",
            "state",
            "startedAt",
            "completedAt",
            "filesSeen",
            "bytesSeen",
            "currentArea",
            "etaSeconds",
            "coverage",
        ],
        "ScanStatus",
    );
    assert_keys(
        &global["scan"]["coverage"],
        &[
            "completeness",
            "unreadableEntries",
            "unknownSizeEntries",
            "excludedAreas",
            "note",
        ],
        "DataCoverage",
    );

    // HomeSummary + Drive + ChangeSinceLastScan + CategoryDelta
    let home = payload(&ok(
        &mut session,
        Command::GetHomeSummary(GetHomeSummaryRequest {}),
    ));
    assert_keys(
        &home,
        &[
            "generatedAt",
            "lastScanAt",
            "safeReclaimableBytes",
            "reviewReclaimableBytes",
            "actionableOpportunityCount",
            "drives",
            "topCategories",
            "changeSinceLastScan",
            "coverage",
        ],
        "HomeSummary",
    );
    assert_keys(
        &home["drives"][0],
        &[
            "id",
            "name",
            "kind",
            "connection",
            "health",
            "totalBytes",
            "usedBytes",
            "freeBytes",
            "usedShare",
            "lastScanAt",
            "snapshotCount",
            "note",
        ],
        "Drive",
    );
    let change = &home["changeSinceLastScan"];
    assert_keys(
        change,
        &[
            "fromSnapshotId",
            "toSnapshotId",
            "fromAt",
            "toAt",
            "totalBytesDelta",
            "categoryDeltas",
            "coverage",
            "note",
        ],
        "ChangeSinceLastScan",
    );
    assert_keys(
        &change["categoryDeltas"][0],
        &[
            "categoryId",
            "name",
            "bytesDelta",
            "entriesDelta",
            "newBytes",
            "oldBytes",
            "partial",
        ],
        "CategoryDelta",
    );
    assert_keys(
        &home["topCategories"][0],
        &[
            "id",
            "name",
            "bytes",
            "shareOfParent",
            "deltaSinceLast",
            "itemCount",
            "entries",
            "files",
            "directories",
            "logicalSize",
            "children",
            "entriesWithoutSize",
            "observationErrors",
            "sampleLocations",
        ],
        "CategoryTotals",
    );
    assert_keys(
        &home["topCategories"][0]["sampleLocations"][0],
        &["label", "displayPath", "bytes"],
        "StorageLocation",
    );

    // StorageData + DuplicateReport + DuplicateGroup + DuplicateMember +
    // BigFileReport + BigFile
    let storage = payload(&ok(&mut session, Command::GetStorage(GetStorageRequest {})));
    assert_keys(
        &storage,
        &[
            "categories",
            "totalObservedBytes",
            "totalObservedItems",
            "duplicateReport",
            "bigFileReport",
            "coverage",
        ],
        "StorageData",
    );
    assert_keys(
        &storage["duplicateReport"],
        &[
            "groups",
            "totalGroups",
            "totalRecoverableBytes",
            "unverifiedEntries",
            "unknownSizeEntries",
            "truncatedGroups",
            "coverage",
        ],
        "DuplicateReport",
    );
    assert_keys(
        &storage["duplicateReport"]["groups"][0],
        &[
            "id",
            "contentSha256",
            "hashVerified",
            "sizeBytes",
            "members",
            "recoverableBytes",
        ],
        "DuplicateGroup",
    );
    assert_keys(
        &storage["duplicateReport"]["groups"][0]["members"][0],
        &["displayPath", "sizeBytes", "isHardlinkAlias"],
        "DuplicateMember",
    );
    assert_keys(
        &storage["bigFileReport"],
        &[
            "files",
            "thresholdBytes",
            "unknownAgeEntries",
            "truncated",
            "coverage",
        ],
        "BigFileReport",
    );
    assert_keys(
        &storage["bigFileReport"]["files"][0],
        &[
            "id",
            "displayPath",
            "sizeBytes",
            "modifiedAt",
            "ageDays",
            "categoryName",
        ],
        "BigFile",
    );

    // CleanupData + CleanupItem + CleanupEvidence
    let cleanup = payload(&ok(&mut session, Command::GetCleanup(GetCleanupRequest {})));
    assert_keys(
        &cleanup,
        &[
            "scanId",
            "lastScanAt",
            "items",
            "offLimits",
            "safeRecoverableBytes",
            "reviewRecoverableBytes",
            "coverage",
        ],
        "CleanupData",
    );
    assert_keys(
        &cleanup["items"][0],
        &[
            "id",
            "categoryId",
            "tier",
            "title",
            "whatItIs",
            "why",
            "observedBytes",
            "recoverableBytes",
            "itemCount",
            "estimatedAgeDays",
            "whatStaysUntouched",
            "consequence",
            "evidence",
            "previewable",
            "offLimitsReason",
        ],
        "CleanupItem",
    );
    assert_keys(
        &cleanup["items"][0]["evidence"][0],
        &["kind", "label", "detail"],
        "CleanupEvidence",
    );

    // DuplicateReport standalone
    let duplicates = payload(&ok(
        &mut session,
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
    ));
    assert_keys(
        &duplicates,
        &[
            "groups",
            "totalGroups",
            "totalRecoverableBytes",
            "unverifiedEntries",
            "unknownSizeEntries",
            "truncatedGroups",
            "coverage",
        ],
        "DuplicateReport",
    );

    // BigFileReport standalone
    let big_files = payload(&ok(
        &mut session,
        Command::GetBigFileReport(GetBigFileReportRequest {}),
    ));
    assert_keys(
        &big_files,
        &[
            "files",
            "thresholdBytes",
            "unknownAgeEntries",
            "truncated",
            "coverage",
        ],
        "BigFileReport",
    );

    // StartupReport + StartupItem
    let startup = payload(&ok(
        &mut session,
        Command::GetStartupReport(GetStartupReportRequest {}),
    ));
    assert_keys(
        &startup,
        &["items", "totalEnabled", "truncated", "coverage"],
        "StartupReport",
    );
    assert_keys(
        &startup["items"][0],
        &[
            "id",
            "name",
            "enabled",
            "sourceLabel",
            "commandLabel",
            "owner",
            "tier",
            "why",
        ],
        "StartupItem",
    );

    // DnsRanking + DnsRankedCandidate + DnsExcludedCandidate
    let dns = payload(&ok(
        &mut session,
        Command::GetDnsRanking(GetDnsRankingRequest {}),
    ));
    assert_keys(
        &dns,
        &["ranked", "excluded", "note", "coverage"],
        "DnsRanking",
    );
    assert_keys(
        &dns["ranked"][0],
        &[
            "name",
            "operator",
            "score",
            "latencyScore",
            "reliabilityScore",
            "dnssecScore",
            "privacyScore",
            "medianLatencyMs",
            "p95LatencyMs",
            "failureRate",
            "samples",
            "privacyPolicy",
            "supportsDnssec",
            "supportsFiltering",
        ],
        "DnsRankedCandidate",
    );
    assert_keys(
        &dns["excluded"][0],
        &["name", "reason", "detail"],
        "DnsExcludedCandidate",
    );

    // BlockListPlan + BlockListEntry + RejectedDomain
    let block = payload(&ok(
        &mut session,
        Command::GetBlockListPlan(GetBlockListPlanRequest {}),
    ));
    assert_keys(
        &block,
        &[
            "entries",
            "total",
            "duplicatesRemoved",
            "invalidRemoved",
            "whitelisted",
            "truncated",
            "reversible",
            "note",
            "coverage",
        ],
        "BlockListPlan",
    );
    assert_keys(
        &block["entries"][0],
        &["domain", "category", "ipv4", "ipv6"],
        "BlockListEntry",
    );
    assert_keys(
        &block["invalidRemoved"][0],
        &["domain", "reason"],
        "RejectedDomain",
    );

    // PrivacySummary + PrivacyFinding
    let privacy = payload(&ok(
        &mut session,
        Command::GetPrivacySummary(GetPrivacySummaryRequest {}),
    ));
    assert_keys(
        &privacy,
        &[
            "findings",
            "totalCount",
            "totalEstimatedBytes",
            "hasUnknownKinds",
            "note",
            "coverage",
        ],
        "PrivacySummary",
    );
    assert_keys(
        &privacy["findings"][0],
        &["kind", "count", "estimatedBytes", "detail"],
        "PrivacyFinding",
    );

    // The helper itself is used by every assertion above; keep one direct use
    // so the synthetic fixture's summary logic is covered too.
    let direct = summarize_privacy(&[]);
    assert_eq!(direct.total_count, 0);
}
