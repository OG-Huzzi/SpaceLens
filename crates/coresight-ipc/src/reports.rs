//! Projections that implement the ten aggregate view commands.
//!
//! Every handler is a pure function over already-registered [`Session`]
//! facts. There is no filesystem access, no subprocess, no socket, no clock
//! read, and no sample data: a session that lacks the required facts gets a
//! typed [`codes::SESSION_UNAVAILABLE`] error.
//!
//! # Data sources
//!
//! | Command | Source |
//! |---|---|
//! | `getGlobalStatus` | latest registered scan's record + derived report |
//! | `getHomeSummary` | latest full-scope scan's report + plan + drives |
//! | `getStorage` | latest full-scope scan's snapshot + report |
//! | `getCleanup` | latest full-scope scan's plan + report |
//! | `getDuplicateReport` | `coresight_filetools::find_duplicates` over the snapshot |
//! | `getBigFileReport` | `coresight_filetools::find_big_files` over the snapshot |
//! | `getStartupReport` | startup inventory registered by the shell |
//! | `getDnsRanking` | DNS ranking registered by the shell |
//! | `getBlockListPlan` | block-list plan registered by the shell |
//! | `getPrivacySummary` | privacy findings registered by the shell |
//!
//! # Determinism
//!
//! Categories are emitted in `Category::ALL` order, duplicate groups in the
//! filetools crate's documented order, big files by `(size desc, path asc)`,
//! and startup/DNS/privacy rows in their source crates' canonical orders. The
//! same session and command serialize byte-identically.
//!
//! # Unknown is not zero
//!
//! A byte total is `None` whenever any contributing observation is unknown.
//! Ages are measured from the scan's registered completion instant — this
//! build never reads the wall clock.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use coresight_classifier::Category;
use coresight_cleanup::{CleanupItem, SafetyTier};
use coresight_filetools::{find_big_files, find_duplicates, BigFileOptions, DuplicateOptions};
use coresight_history::ObservedEntry;
use coresight_insights::UsageReport;
use coresight_systools::startup::{analyze_startup, StartupOptions, StartupTier};
use coresight_webtools::{DnsPrivacyPolicy, DnsRanking, DomainCategory, PrivacyFindingKind};

use crate::error::{codes, ApiError, Envelope};
use crate::response::Response;
use crate::session::{ScanState, Session};
use crate::time::rfc3339_utc;
use crate::views::{
    BigFileReportView, BigFileView, BlockListEntryView, BlockListPlanView, CategoryDeltaView,
    CategoryTotals, ChangeSinceLastScanView, CleanupDataView, CleanupEvidence, CleanupEvidenceKind,
    CleanupItemView, CleanupTier, DataCompleteness, DataCoverage, DnsExcludedCandidate,
    DnsExclusionReasonView, DnsPrivacyPolicyView, DnsRankedCandidate, DnsRankingView,
    DriveConnectionView, DriveHealthView, DriveKindView, DriveView, DuplicateGroupView,
    DuplicateMemberView, DuplicateReportView, GlobalStatusView, HomeSummaryView,
    PrivacyFindingKindView, PrivacyFindingView, PrivacySummaryView, RejectedDomainView,
    ScanStateView, ScanStatusView, StartupItemView, StartupReportView, StartupTierView,
    StorageDataView, StorageLocation,
};

/// Minimum logical size for a file to appear in the big-file report.
///
/// This is the source crate's documented default (`BigFileOptions`): a file
/// smaller than 100 MiB is not a "big file" for this report. The value is
/// published in every response as `thresholdBytes`.
const BIG_FILE_THRESHOLD_BYTES: u64 = 100 * 1024 * 1024;

/// Maximum big-file candidates published. Omitted candidates are counted in
/// `truncated`, never silently dropped.
const BIG_FILE_MAX_RESULTS: usize = 100;

/// Sample locations published per category row.
const MAX_SAMPLE_LOCATIONS: usize = 3;

const SECONDS_PER_DAY: u64 = 86_400;

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// `coresight.v1.getGlobalStatus`.
pub(crate) fn handle_get_global_status(session: &Session) -> Envelope<Response> {
    let Some(scan) = session.latest_scan() else {
        return Envelope::err(no_scan_registered());
    };
    Envelope::ok(Response::GlobalStatus(GlobalStatusView {
        contract_version: crate::views::CONTRACT_VERSION.to_string(),
        scan: scan_status_view(scan),
        simulated_data: false,
    }))
}

/// `coresight.v1.getHomeSummary`.
pub(crate) fn handle_get_home_summary(session: &Session) -> Envelope<Response> {
    let scan = match latest_full_scope_scan(session) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    let Some(generated_at) = scan.record.completed_at.and_then(rfc3339_utc) else {
        return Envelope::err(no_scan_instant());
    };
    let rows = category_rows(scan, session.previous_full_scope_scan(scan));
    Envelope::ok(Response::HomeSummary(HomeSummaryView {
        generated_at,
        last_scan_at: scan.record.completed_at.and_then(rfc3339_utc),
        safe_reclaimable_bytes: tier_recoverable_bytes(&scan.plan, SafetyTier::Safe),
        review_reclaimable_bytes: tier_recoverable_bytes(&scan.plan, SafetyTier::Review),
        actionable_opportunity_count: Some(
            u64::try_from(scan.plan.items.len()).unwrap_or(u64::MAX),
        ),
        drives: drive_views(session),
        top_categories: top_categories(&rows),
        change_since_last_scan: change_since_last_scan(session, scan),
        coverage: coverage_for_report(&scan.report),
    }))
}

/// `coresight.v1.getStorage`.
pub(crate) fn handle_get_storage(session: &Session) -> Envelope<Response> {
    let scan = match latest_full_scope_scan(session) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    let complete = report_is_complete(&scan.report);
    Envelope::ok(Response::Storage(StorageDataView {
        categories: category_rows(scan, session.previous_full_scope_scan(scan)),
        total_observed_bytes: if complete {
            Some(scan.report.total_logical_size())
        } else {
            None
        },
        total_observed_items: Some(scan.report.scanned_entries),
        duplicate_report: duplicate_report(scan),
        big_file_report: big_file_report(scan),
        coverage: coverage_for_report(&scan.report),
    }))
}

/// `coresight.v1.getCleanup`.
pub(crate) fn handle_get_cleanup(session: &Session) -> Envelope<Response> {
    let scan = match latest_full_scope_scan(session) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    Envelope::ok(Response::Cleanup(cleanup_data(scan)))
}

/// `coresight.v1.getDuplicateReport`.
pub(crate) fn handle_get_duplicate_report(session: &Session) -> Envelope<Response> {
    let scan = match latest_full_scope_scan(session) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    Envelope::ok(Response::DuplicateReport(duplicate_report(scan)))
}

/// `coresight.v1.getBigFileReport`.
pub(crate) fn handle_get_big_file_report(session: &Session) -> Envelope<Response> {
    let scan = match latest_full_scope_scan(session) {
        Ok(scan) => scan,
        Err(error) => return Envelope::err(error),
    };
    Envelope::ok(Response::BigFileReport(big_file_report(scan)))
}

/// `coresight.v1.getStartupReport`.
pub(crate) fn handle_get_startup_report(session: &Session) -> Envelope<Response> {
    let Some(items) = session.startup_items() else {
        return Envelope::err(no_registered_report("startup inventory"));
    };
    let report = analyze_startup(items, &StartupOptions::all());
    let truncated = u64::try_from(report.truncated).unwrap_or(u64::MAX);
    let items: Vec<StartupItemView> = report
        .items
        .iter()
        .map(|finding| StartupItemView {
            id: finding.item.id.clone(),
            name: finding.item.name.clone(),
            enabled: finding.item.enabled,
            source_label: finding.item.source.kind().label().to_string(),
            command_label: finding.item.command.clone(),
            owner: finding.item.owner.clone(),
            tier: startup_tier(finding.tier),
            why: finding.why.clone(),
        })
        .collect();
    Envelope::ok(Response::StartupReport(StartupReportView {
        items,
        total_enabled: Some(u64::try_from(report.total_enabled).unwrap_or(u64::MAX)),
        truncated: Some(truncated),
        coverage: DataCoverage {
            completeness: if truncated == 0 {
                DataCompleteness::Complete
            } else {
                DataCompleteness::Partial
            },
            unreadable_entries: None,
            unknown_size_entries: None,
            excluded_areas: Vec::new(),
            note: Some(
                "Covers the startup inventory registered in this session. This build performs \
                 no startup enumeration of its own."
                    .to_string(),
            ),
        },
    }))
}

/// `coresight.v1.getDnsRanking`.
pub(crate) fn handle_get_dns_ranking(session: &Session) -> Envelope<Response> {
    let Some(ranking) = session.dns_ranking() else {
        return Envelope::err(no_registered_report("DNS ranking"));
    };
    Envelope::ok(Response::DnsRanking(dns_ranking_view(ranking)))
}

/// `coresight.v1.getBlockListPlan`.
pub(crate) fn handle_get_block_list_plan(session: &Session) -> Envelope<Response> {
    let Some(plan) = session.block_list_plan() else {
        return Envelope::err(no_registered_report("block-list plan"));
    };
    let total = u64::try_from(plan.total).unwrap_or(u64::MAX);
    let duplicates = u64::try_from(plan.duplicates_removed).unwrap_or(u64::MAX);
    let truncated = u64::try_from(plan.truncated).unwrap_or(u64::MAX);
    let invalid: Vec<RejectedDomainView> = plan
        .invalid_removed
        .iter()
        .map(|row| RejectedDomainView {
            domain: row.domain.clone(),
            reason: row.reason.clone(),
        })
        .collect();
    let entries: Vec<BlockListEntryView> = plan
        .entries
        .iter()
        .map(|entry| BlockListEntryView {
            domain: entry.domain.clone(),
            category: domain_category_code(entry.category).to_string(),
            ipv4: entry.ipv4.clone(),
            ipv6: entry.ipv6.clone(),
        })
        .collect();
    let completeness = if invalid.is_empty() && truncated == 0 {
        DataCompleteness::Complete
    } else {
        DataCompleteness::Partial
    };
    Envelope::ok(Response::BlockListPlan(BlockListPlanView {
        entries,
        total: Some(total),
        duplicates_removed: Some(duplicates),
        invalid_removed: invalid,
        whitelisted: plan.whitelisted.clone(),
        truncated: Some(truncated),
        reversible: true,
        note: format!(
            "This is a reversible plan of {total} hosts-file entries; it writes nothing. \
             {duplicates} duplicate rule(s) were collapsed and {} rejected entries are listed \
             with their reasons. Applying it is a separate, explicitly authorized step.",
            plan.invalid_removed.len()
        ),
        coverage: DataCoverage {
            completeness,
            unreadable_entries: None,
            unknown_size_entries: None,
            excluded_areas: Vec::new(),
            note: Some(
                "Covers the block rules registered in this session; CoreSight read no hosts file \
                 and wrote nothing."
                    .to_string(),
            ),
        },
    }))
}

/// `coresight.v1.getPrivacySummary`.
pub(crate) fn handle_get_privacy_summary(session: &Session) -> Envelope<Response> {
    let Some(findings) = session.privacy_findings() else {
        return Envelope::err(no_registered_report("privacy findings"));
    };
    let summary = coresight_webtools::summarize_privacy(findings);
    let unknown_size_entries = u64::try_from(
        summary
            .findings
            .iter()
            .filter(|finding| finding.estimated_bytes.is_none())
            .count(),
    )
    .unwrap_or(u64::MAX);
    let completeness = if summary.total_estimated_bytes.is_none() || summary.has_unknown_kinds {
        DataCompleteness::Partial
    } else {
        DataCompleteness::Complete
    };
    let rows: Vec<PrivacyFindingView> = summary
        .findings
        .iter()
        .map(|finding| PrivacyFindingView {
            kind: privacy_kind(finding.kind),
            count: Some(finding.count),
            estimated_bytes: finding.estimated_bytes,
            detail: finding.detail.clone(),
        })
        .collect();
    Envelope::ok(Response::PrivacySummary(PrivacySummaryView {
        findings: rows,
        total_count: Some(summary.total_count),
        total_estimated_bytes: summary.total_estimated_bytes,
        has_unknown_kinds: summary.has_unknown_kinds,
        note: summary.note,
        coverage: DataCoverage {
            completeness,
            unreadable_entries: None,
            unknown_size_entries: Some(unknown_size_entries),
            excluded_areas: Vec::new(),
            note: Some(
                "Covers the privacy findings registered in this session. These are evidence \
                 counts, not a delete list; nothing was removed."
                    .to_string(),
            ),
        },
    }))
}

// ---------------------------------------------------------------------------
// Session access + errors
// ---------------------------------------------------------------------------

fn latest_full_scope_scan(session: &Session) -> Result<&ScanState, ApiError> {
    session
        .latest_full_scope_scan()
        .ok_or_else(no_scan_registered)
}

fn no_scan_registered() -> ApiError {
    ApiError::new(
        codes::SESSION_UNAVAILABLE,
        "No completed full-scope scan is registered in this CoreSight session, so this \
         report is unavailable. Nothing was changed.",
    )
    .with_detail(
        "register a completed scan with Session::register_scan before requesting an aggregate \
         report",
    )
}

fn no_scan_instant() -> ApiError {
    ApiError::new(
        codes::SESSION_UNAVAILABLE,
        "The newest scan did not record a completion time, so a report that needs a deterministic \
         reference instant cannot be built. Nothing was changed.",
    )
    .with_detail("ScanRecord::completed_at is None; register the scan with its completion instant")
}

fn no_registered_report(what: &str) -> ApiError {
    ApiError::new(
        codes::SESSION_UNAVAILABLE,
        format!(
            "This CoreSight session has no {what} registered, so the report is unavailable. \
             Nothing was changed."
        ),
    )
    .with_detail(format!(
        "no {what} is present in the session; the shell must register the real data first"
    ))
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

fn report_is_complete(report: &UsageReport) -> bool {
    report.entries_without_size == 0 && report.errored_entries == 0
}

fn coverage_for_report(report: &UsageReport) -> DataCoverage {
    let complete = report_is_complete(report);
    DataCoverage {
        completeness: if complete {
            DataCompleteness::Complete
        } else {
            DataCompleteness::Partial
        },
        unreadable_entries: Some(report.errored_entries),
        unknown_size_entries: Some(report.entries_without_size),
        excluded_areas: Vec::new(),
        note: if complete {
            None
        } else {
            Some(format!(
                "{} entries could not be fully read and {} entries had no known size; byte \
                 totals are lower bounds, not totals.",
                report.errored_entries, report.entries_without_size
            ))
        },
    }
}

fn combined_coverage(previous: &UsageReport, current: &UsageReport) -> DataCoverage {
    let complete = report_is_complete(previous) && report_is_complete(current);
    DataCoverage {
        completeness: if complete {
            DataCompleteness::Complete
        } else {
            DataCompleteness::Partial
        },
        unreadable_entries: Some(
            previous
                .errored_entries
                .saturating_add(current.errored_entries),
        ),
        unknown_size_entries: Some(
            previous
                .entries_without_size
                .saturating_add(current.entries_without_size),
        ),
        excluded_areas: Vec::new(),
        note: if complete {
            Some("Both scans covered every observed entry.".to_string())
        } else {
            Some(
                "At least one of the compared scans had unreadable or unsized entries; the \
                 deltas are lower-bound statements."
                    .to_string(),
            )
        },
    }
}

// ---------------------------------------------------------------------------
// Global status
// ---------------------------------------------------------------------------

fn scan_status_view(scan: &ScanState) -> ScanStatusView {
    let complete = report_is_complete(&scan.report);
    let state = match scan.record.status {
        crate::response::ScanStatus::Running => ScanStateView::Scanning,
        crate::response::ScanStatus::Failed => ScanStateView::Failed,
        crate::response::ScanStatus::Cancelled => ScanStateView::Partial,
        crate::response::ScanStatus::Completed
        | crate::response::ScanStatus::CompletedWithLimits => {
            if complete {
                ScanStateView::Complete
            } else {
                ScanStateView::Partial
            }
        }
    };
    let coverage =
        if scan.record.status.observes_full_scope() {
            coverage_for_report(&scan.report)
        } else {
            DataCoverage {
            completeness: match scan.record.status {
                crate::response::ScanStatus::Running | crate::response::ScanStatus::Failed => {
                    DataCompleteness::Unknown
                }
                _ => DataCompleteness::Partial,
            },
            unreadable_entries: Some(scan.report.errored_entries),
            unknown_size_entries: Some(scan.report.entries_without_size),
            excluded_areas: Vec::new(),
            note: Some(match scan.record.status {
                crate::response::ScanStatus::Running => {
                    "The scan is still running; counts describe progress, not totals."
                }
                crate::response::ScanStatus::Failed => {
                    "The scan failed; its observations are not a complete report."
                }
                _ => "The scan did not finish its full declared scope; the observed subset is \
                      incomplete.",
            }
            .to_string()),
        }
        };
    ScanStatusView {
        scan_id: Some(scan.record.scan_id.clone()),
        state,
        // This build records only completion instants; a start instant and
        // progress estimate are not invented.
        started_at: None,
        completed_at: scan.record.completed_at.and_then(rfc3339_utc),
        files_seen: Some(scan.report.scanned_entries),
        bytes_seen: if scan.record.status.observes_full_scope() && complete {
            Some(scan.report.total_logical_size())
        } else {
            None
        },
        current_area: None,
        eta_seconds: None,
        coverage,
    }
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct SubcategoryEvidence {
    entries: u64,
    files: u64,
    directories: u64,
    bytes: u64,
    entries_without_size: u64,
    observation_errors: u64,
    samples: Vec<StorageLocation>,
}

#[derive(Default, Clone)]
struct CategoryEvidence {
    observation_errors: u64,
    samples: Vec<StorageLocation>,
    subcategories: BTreeMap<String, SubcategoryEvidence>,
    uncategorized: SubcategoryEvidence,
}

fn collect_evidence(entries: &[ObservedEntry]) -> Vec<CategoryEvidence> {
    let mut evidence: Vec<CategoryEvidence> = vec![CategoryEvidence::default(); Category::COUNT];
    for entry in entries {
        let index = entry
            .classification
            .as_ref()
            .and_then(|classification| decode_category_code(&classification.category))
            .and_then(|category| Category::ALL.iter().position(|known| *known == category))
            .unwrap_or(Category::Unknown as usize);
        let sample = storage_location(entry);
        let Some(row) = evidence.get_mut(index) else {
            continue;
        };
        if entry.observation_error.is_some() {
            row.observation_errors = row.observation_errors.saturating_add(1);
        }
        if row.samples.len() < MAX_SAMPLE_LOCATIONS {
            row.samples.push(sample.clone());
        }
        let bucket = match entry
            .classification
            .as_ref()
            .and_then(|classification| classification.subcategory.clone())
        {
            Some(code) => row.subcategories.entry(code).or_default(),
            None => &mut row.uncategorized,
        };
        bucket.entries = bucket.entries.saturating_add(1);
        match entry.kind {
            coresight_history::ObservedKind::File => {
                bucket.files = bucket.files.saturating_add(1);
            }
            coresight_history::ObservedKind::Dir => {
                bucket.directories = bucket.directories.saturating_add(1);
            }
            _ => {}
        }
        match entry.size {
            Some(size) => bucket.bytes = bucket.bytes.saturating_add(size),
            None => bucket.entries_without_size = bucket.entries_without_size.saturating_add(1),
        }
        if entry.observation_error.is_some() {
            bucket.observation_errors = bucket.observation_errors.saturating_add(1);
        }
        if bucket.samples.len() < MAX_SAMPLE_LOCATIONS {
            bucket.samples.push(sample);
        }
    }
    evidence
}

fn storage_location(entry: &ObservedEntry) -> StorageLocation {
    let label = match entry.kind {
        coresight_history::ObservedKind::File => "Observed file",
        coresight_history::ObservedKind::Dir => "Observed directory",
        coresight_history::ObservedKind::Link => "Observed link",
        coresight_history::ObservedKind::Other => "Observed entry",
    };
    StorageLocation {
        label: label.to_string(),
        display_path: entry.path.to_string_lossy().into_owned(),
        bytes: entry.size,
    }
}

fn decode_category_code(code: &str) -> Option<Category> {
    Category::ALL
        .iter()
        .copied()
        .find(|category| category.code() == code)
}

fn category_rows(scan: &ScanState, previous: Option<&ScanState>) -> Vec<CategoryTotals> {
    let report = &scan.report;
    let evidence = collect_evidence(&scan.record.snapshot.entries);
    let complete = report_is_complete(report);
    let total_bytes = report.total_logical_size();
    let parent_total = if complete && total_bytes > 0 {
        Some(total_bytes)
    } else {
        None
    };
    let previous_report = previous
        .filter(|scan| report_is_complete(&scan.report))
        .map(|scan| &scan.report);

    Category::ALL
        .iter()
        .enumerate()
        .map(|(index, category)| {
            let totals = report.category.get(index).copied().unwrap_or_default();
            let row_evidence = evidence.get(index);
            let observation_errors = row_evidence.map_or(0, |row| row.observation_errors);
            let bytes = if totals.entries_without_size == 0 && observation_errors == 0 {
                Some(totals.logical_size)
            } else {
                None
            };
            let share_of_parent = bytes.and_then(|value| {
                parent_total
                    .filter(|total| *total > 0)
                    .map(|total| share_ratio(value, total))
            });
            let delta_since_last = if complete {
                previous_report.and_then(|previous_report| {
                    let old = previous_report
                        .category
                        .get(index)
                        .copied()
                        .unwrap_or_default();
                    if old.entries_without_size == 0 {
                        Some(saturating_delta(totals.logical_size, old.logical_size))
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            let children = row_evidence.map_or_else(Vec::new, |row| child_rows(row, bytes));
            CategoryTotals {
                id: category.code().to_string(),
                name: category_title(*category).to_string(),
                bytes,
                share_of_parent,
                delta_since_last,
                item_count: Some(totals.entries),
                entries: Some(totals.entries),
                files: Some(totals.files),
                directories: Some(totals.directories),
                logical_size: bytes,
                children,
                entries_without_size: Some(totals.entries_without_size),
                observation_errors: Some(observation_errors),
                sample_locations: row_evidence.map_or_else(Vec::new, |row| row.samples.clone()),
            }
        })
        .collect()
}

fn child_rows(row: &CategoryEvidence, parent_bytes: Option<u64>) -> Vec<CategoryTotals> {
    let mut rows: Vec<CategoryTotals> = Vec::with_capacity(row.subcategories.len() + 1);
    for (code, bucket) in &row.subcategories {
        rows.push(child_row(
            code,
            subcategory_name(Some(code)),
            bucket,
            parent_bytes,
        ));
    }
    if row.uncategorized.entries > 0 {
        rows.push(child_row(
            "UNSUBCATEGORIZED",
            "Uncategorized",
            &row.uncategorized,
            parent_bytes,
        ));
    }
    rows
}

fn child_row(
    id: &str,
    name: &str,
    bucket: &SubcategoryEvidence,
    parent_bytes: Option<u64>,
) -> CategoryTotals {
    let bytes = if bucket.entries_without_size == 0 && bucket.observation_errors == 0 {
        Some(bucket.bytes)
    } else {
        None
    };
    let share_of_parent = bytes.and_then(|value| {
        parent_bytes
            .filter(|total| *total > 0)
            .map(|total| share_ratio(value, total))
    });
    CategoryTotals {
        id: id.to_string(),
        name: name.to_string(),
        bytes,
        share_of_parent,
        // No per-subcategory baseline is retained in this build.
        delta_since_last: None,
        item_count: Some(bucket.entries),
        entries: Some(bucket.entries),
        files: Some(bucket.files),
        directories: Some(bucket.directories),
        logical_size: bytes,
        children: Vec::new(),
        entries_without_size: Some(bucket.entries_without_size),
        observation_errors: Some(bucket.observation_errors),
        sample_locations: bucket.samples.clone(),
    }
}

fn top_categories(rows: &[CategoryTotals]) -> Vec<CategoryTotals> {
    let mut indexes: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.bytes.is_some_and(|bytes| bytes > 0))
        .map(|(index, _)| index)
        .collect();
    indexes.sort_by(|left, right| {
        rows.get(*right)
            .and_then(|row| row.bytes)
            .cmp(&rows.get(*left).and_then(|row| row.bytes))
            .then_with(|| left.cmp(right))
    });
    indexes.truncate(3);
    // Selection is by size; presentation stays in `Category::ALL` order.
    indexes.sort_unstable();
    indexes
        .into_iter()
        .filter_map(|index| rows.get(index).cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// Change since last scan
// ---------------------------------------------------------------------------

fn change_since_last_scan(
    session: &Session,
    current: &ScanState,
) -> Option<ChangeSinceLastScanView> {
    let previous = session.previous_full_scope_scan(current)?;
    let complete = report_is_complete(&previous.report) && report_is_complete(&current.report);
    let category_deltas = Category::ALL
        .iter()
        .enumerate()
        .map(|(index, category)| {
            let new = current
                .report
                .category
                .get(index)
                .copied()
                .unwrap_or_default();
            let old = previous
                .report
                .category
                .get(index)
                .copied()
                .unwrap_or_default();
            let new_known = complete && new.entries_without_size == 0;
            let old_known = complete && old.entries_without_size == 0;
            CategoryDeltaView {
                category_id: category.code().to_string(),
                name: category_title(*category).to_string(),
                bytes_delta: if new_known && old_known {
                    Some(saturating_delta(new.logical_size, old.logical_size))
                } else {
                    None
                },
                entries_delta: if complete {
                    Some(saturating_delta(new.entries, old.entries))
                } else {
                    None
                },
                new_bytes: if new_known {
                    Some(new.logical_size)
                } else {
                    None
                },
                old_bytes: if old_known {
                    Some(old.logical_size)
                } else {
                    None
                },
                partial: !complete,
            }
        })
        .collect();
    Some(ChangeSinceLastScanView {
        from_snapshot_id: Some(previous.record.scan_id.clone()),
        to_snapshot_id: Some(current.record.scan_id.clone()),
        from_at: previous.record.completed_at.and_then(rfc3339_utc),
        to_at: current.record.completed_at.and_then(rfc3339_utc),
        total_bytes_delta: if complete {
            Some(saturating_delta(
                current.report.total_logical_size(),
                previous.report.total_logical_size(),
            ))
        } else {
            None
        },
        category_deltas,
        coverage: combined_coverage(&previous.report, &current.report),
        note: Some(if complete {
            "Compared with the previous full-scope scan of this drive.".to_string()
        } else {
            "Compared with the previous full-scope scan of this drive; at least one side is \
             incomplete, so the deltas are lower-bound statements."
                .to_string()
        }),
    })
}

/// A share in `0..1`, rounded to six decimal places.
///
/// The UI renders shares as percentages; keeping six significant decimals
/// avoids publishing a binary-float artefact and keeps the value stable
/// through JSON transports that shorten decimal representations.
fn share_ratio(part: u64, whole: u64) -> f64 {
    (part as f64 / whole as f64 * 1_000_000.0).round() / 1_000_000.0
}

fn saturating_delta(new: u64, old: u64) -> i64 {
    let difference = i128::from(new) - i128::from(old);
    if difference > i128::from(i64::MAX) {
        i64::MAX
    } else if difference < i128::from(i64::MIN) {
        i64::MIN
    } else {
        difference as i64
    }
}

// ---------------------------------------------------------------------------
// Drives
// ---------------------------------------------------------------------------

fn drive_views(session: &Session) -> Vec<DriveView> {
    session
        .drives_in_list_order()
        .into_iter()
        .map(|drive| {
            let scans = session.scans_for_drive(&drive.id);
            let last_scan_at = scans
                .iter()
                .rev()
                .find_map(|scan| scan.record.completed_at.and_then(rfc3339_utc));
            let used_bytes = match (drive.capacity_bytes, drive.available_bytes) {
                // Free space larger than capacity is inconsistent evidence;
                // the used amount is unknown rather than silently clamped.
                (Some(total), Some(available)) if available <= total => Some(total - available),
                _ => None,
            };
            let used_share = match (used_bytes, drive.capacity_bytes) {
                (Some(used), Some(total)) if total > 0 => Some(share_ratio(used, total)),
                _ => None,
            };
            let name = drive
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| drive.root.trim())
                .to_string();
            DriveView {
                id: drive.id.clone(),
                name,
                kind: match drive.kind {
                    crate::response::DriveKind::Internal => DriveKindView::Internal,
                    // The UI contract has only two kinds; removable, network,
                    // and unclassified volumes are all "external".
                    _ => DriveKindView::External,
                },
                connection: if drive.connected {
                    DriveConnectionView::Connected
                } else {
                    DriveConnectionView::Remembered
                },
                // No health policy is recorded by this build, so health is an
                // honest unknown rather than a fabricated "healthy".
                health: DriveHealthView::Unknown,
                total_bytes: drive.capacity_bytes,
                used_bytes,
                free_bytes: drive.available_bytes,
                used_share,
                last_scan_at,
                snapshot_count: Some(u64::try_from(scans.len()).unwrap_or(u64::MAX)),
                note: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

fn tier_recoverable_bytes(plan: &coresight_cleanup::CleanupPlan, tier: SafetyTier) -> Option<u64> {
    let mut total = 0u64;
    let mut seen = false;
    for item in &plan.items {
        if item.tier == tier {
            seen = true;
            total = total.saturating_add(item.recoverable_bytes);
        }
    }
    if !seen {
        // Nothing planned in this tier is a true zero, not an unknown.
        return Some(0);
    }
    if plan.partial {
        None
    } else {
        Some(total)
    }
}

fn cleanup_data(scan: &ScanState) -> CleanupDataView {
    let items: Vec<CleanupItemView> = scan
        .plan
        .items
        .iter()
        .map(|item| cleanup_item(scan, item))
        .collect();
    let evidence = collect_evidence(&scan.record.snapshot.entries);
    let off_limits: Vec<CleanupItemView> = scan
        .plan
        .off_limits_excluded
        .iter()
        .map(|category| {
            let observation_errors = Category::ALL
                .iter()
                .position(|known| known == category)
                .and_then(|index| evidence.get(index))
                .map_or(0, |row| row.observation_errors);
            off_limits_item(&scan.report, *category, observation_errors)
        })
        .collect();
    CleanupDataView {
        scan_id: Some(scan.record.scan_id.clone()),
        last_scan_at: scan.record.completed_at.and_then(rfc3339_utc),
        items,
        off_limits,
        safe_recoverable_bytes: tier_recoverable_bytes(&scan.plan, SafetyTier::Safe),
        review_recoverable_bytes: tier_recoverable_bytes(&scan.plan, SafetyTier::Review),
        coverage: coverage_for_report(&scan.report),
    }
}

fn cleanup_item(scan: &ScanState, item: &CleanupItem) -> CleanupItemView {
    let totals = scan.report.totals_for(item.category);
    let known = !scan.plan.partial && totals.entries_without_size == 0;
    let destination = item
        .destination
        .as_deref()
        .unwrap_or("an unspecified destination");
    let undo = item.undo.as_deref().unwrap_or("an unspecified undo path");
    let tier = match item.tier {
        SafetyTier::Safe => CleanupTier::Safe,
        SafetyTier::Review => CleanupTier::Review,
        SafetyTier::OffLimits => CleanupTier::OffLimits,
    };
    let mut evidence = vec![
        CleanupEvidence {
            kind: CleanupEvidenceKind::Observation,
            label: "Scan observation".to_string(),
            detail: format!(
                "This scan observed {} entries in the {} category; {} of them had no known \
                 size.",
                totals.entries,
                category_title(item.category),
                totals.entries_without_size
            ),
        },
        CleanupEvidence {
            kind: CleanupEvidenceKind::Verification,
            label: "Category-level classification".to_string(),
            detail: format!(
                "The cleanup planner classifies this item as {tier:?} from category totals \
                 only; no per-file content verification was recorded for it."
            ),
        },
        CleanupEvidence {
            kind: CleanupEvidenceKind::Safety,
            label: "Reversible move".to_string(),
            detail: format!(
                "The planned action moves items to {destination}; the recorded undo path is \
                 {undo}."
            ),
        },
    ];
    if scan.report.errored_entries > 0 {
        evidence.push(CleanupEvidence {
            kind: CleanupEvidenceKind::Observation,
            label: "Incomplete observation".to_string(),
            detail: format!(
                "{} entries in the underlying scan could not be fully read.",
                scan.report.errored_entries
            ),
        });
    }
    CleanupItemView {
        id: crate::session::opportunity_id_for(&scan.record.scan_id, item),
        category_id: item.category.code().to_string(),
        tier,
        title: item.title.clone(),
        what_it_is: what_it_is(item, totals.entries, totals.entries_without_size),
        why: item.why_recommended.clone(),
        observed_bytes: if known {
            Some(totals.logical_size)
        } else {
            None
        },
        recoverable_bytes: if known {
            Some(item.recoverable_bytes)
        } else {
            None
        },
        item_count: Some(totals.entries),
        // The aggregate retains no age evidence; `None` is honest and must not
        // be rendered as "0 days old".
        estimated_age_days: None,
        what_stays_untouched: item.what_stays_untouched.clone(),
        consequence: item.consequence.clone(),
        evidence,
        previewable: item.destination.is_some()
            && item.undo.is_some()
            && item.action.is_reversible()
            && item.tier != SafetyTier::OffLimits,
        off_limits_reason: None,
    }
}

fn off_limits_item(
    report: &UsageReport,
    category: Category,
    observation_errors: u64,
) -> CleanupItemView {
    let totals = report.totals_for(category);
    let title = category_title(category);
    CleanupItemView {
        id: format!("off-{}", category.code().to_ascii_lowercase()),
        category_id: category.code().to_string(),
        tier: CleanupTier::OffLimits,
        title: title.to_string(),
        what_it_is: format!("{title} data observed during this scan."),
        why: "CoreSight's cleanup policy keeps this category off limits: it can contain user or \
              system data that cannot be regenerated."
            .to_string(),
        observed_bytes: if totals.entries_without_size == 0 && observation_errors == 0 {
            Some(totals.logical_size)
        } else {
            None
        },
        recoverable_bytes: None,
        item_count: Some(totals.entries),
        estimated_age_days: None,
        what_stays_untouched: format!(
            "Everything in {title} stays untouched; this row is shown for understanding only."
        ),
        consequence: "No action is available for this category.".to_string(),
        evidence: Vec::new(),
        previewable: false,
        off_limits_reason: Some(
            "Protected category — exposure only, never actionable.".to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Duplicates
// ---------------------------------------------------------------------------

fn duplicate_report(scan: &ScanState) -> DuplicateReportView {
    let report = find_duplicates(&scan.record.snapshot, &DuplicateOptions::default());
    let groups: Vec<DuplicateGroupView> = report
        .groups
        .iter()
        .map(|group| DuplicateGroupView {
            id: format!("dup-{}", group.content_sha256),
            content_sha256: group.content_sha256.clone(),
            hash_verified: true,
            size_bytes: Some(group.size),
            members: group
                .members
                .iter()
                .map(|member| DuplicateMemberView {
                    display_path: member.path.to_string_lossy().into_owned(),
                    size_bytes: Some(group.size),
                    is_hardlink_alias: member.is_hardlink_alias,
                })
                .collect(),
            recoverable_bytes: Some(group.recoverable_bytes),
        })
        .collect();
    let partial = report.unverified_entries > 0 || report.unknown_size_entries > 0;
    DuplicateReportView {
        groups,
        total_groups: Some(report.total_groups),
        total_recoverable_bytes: Some(report.total_recoverable_bytes),
        unverified_entries: Some(report.unverified_entries),
        unknown_size_entries: Some(report.unknown_size_entries),
        truncated_groups: Some(report.truncated_groups),
        coverage: DataCoverage {
            completeness: if partial {
                DataCompleteness::Partial
            } else {
                DataCompleteness::Complete
            },
            unreadable_entries: None,
            unknown_size_entries: Some(report.unknown_size_entries),
            excluded_areas: Vec::new(),
            note: Some(
                "Only byte-identical files with a verified SHA-256 content hash are grouped; \
                 files without a verified hash or without an observed size are counted but \
                 never claimed as duplicates."
                    .to_string(),
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Big files
// ---------------------------------------------------------------------------

fn big_file_report(scan: &ScanState) -> BigFileReportView {
    let reference = scan.record.completed_at;
    let options = BigFileOptions {
        min_size_bytes: BIG_FILE_THRESHOLD_BYTES,
        // No age filter: age is published per row instead of silently
        // excluding rows whose age is unknown.
        min_age_days: None,
        max_results: BIG_FILE_MAX_RESULTS,
    };
    let report = find_big_files(
        &scan.record.snapshot,
        reference.unwrap_or(UNIX_EPOCH),
        &options,
    );
    let mut unknown_age_entries = 0u64;
    let files: Vec<BigFileView> = report
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let age_days = scan_age_days(reference, candidate.modified);
            if age_days.is_none() {
                unknown_age_entries = unknown_age_entries.saturating_add(1);
            }
            BigFileView {
                id: format!("big-{index}"),
                display_path: candidate.path.to_string_lossy().into_owned(),
                size_bytes: Some(candidate.size),
                modified_at: candidate.modified.and_then(rfc3339_utc),
                age_days,
                category_name: candidate
                    .category
                    .map(|category| category_title(category).to_string()),
            }
        })
        .collect();
    let mut coverage = coverage_for_report(&scan.report);
    let age_note = "Ages are measured from the scan's registered completion instant; this build \
                    reads no clock.";
    coverage.note = Some(match coverage.note {
        Some(note) => format!("{note} {age_note}"),
        None => age_note.to_string(),
    });
    BigFileReportView {
        files,
        threshold_bytes: Some(BIG_FILE_THRESHOLD_BYTES),
        unknown_age_entries: Some(unknown_age_entries),
        truncated: Some(report.truncated),
        coverage,
    }
}

fn scan_age_days(reference: Option<SystemTime>, modified: Option<SystemTime>) -> Option<u64> {
    let reference = reference?;
    let modified = modified?;
    reference
        .duration_since(modified)
        .ok()
        .map(|age| age.as_secs() / SECONDS_PER_DAY)
}

// ---------------------------------------------------------------------------
// Startup / DNS / block list / privacy vocabulary
// ---------------------------------------------------------------------------

fn startup_tier(tier: StartupTier) -> StartupTierView {
    match tier {
        StartupTier::SafeToDisable => StartupTierView::SafeToDisable,
        StartupTier::ReviewFirst => StartupTierView::ReviewFirst,
        StartupTier::OffLimits => StartupTierView::OffLimits,
    }
}

fn dns_privacy_policy(policy: DnsPrivacyPolicy) -> DnsPrivacyPolicyView {
    match policy {
        DnsPrivacyPolicy::DocumentedNoLogging => DnsPrivacyPolicyView::DocumentedNoLogging,
        DnsPrivacyPolicy::DocumentedRetention { .. } => DnsPrivacyPolicyView::DocumentedRetention,
        DnsPrivacyPolicy::Undisclosed => DnsPrivacyPolicyView::Undisclosed,
        DnsPrivacyPolicy::Unknown => DnsPrivacyPolicyView::Unknown,
    }
}

fn dns_exclusion_reason(reason: coresight_webtools::DnsExclusionReason) -> DnsExclusionReasonView {
    match reason {
        coresight_webtools::DnsExclusionReason::InsufficientEvidence => {
            DnsExclusionReasonView::InsufficientEvidence
        }
        coresight_webtools::DnsExclusionReason::TooUnreliable => {
            DnsExclusionReasonView::TooUnreliable
        }
        coresight_webtools::DnsExclusionReason::UndisclosedPrivacy => {
            DnsExclusionReasonView::UndisclosedPrivacy
        }
        coresight_webtools::DnsExclusionReason::NotMeasured => DnsExclusionReasonView::NotMeasured,
    }
}

fn dns_ranking_view(ranking: &DnsRanking) -> DnsRankingView {
    let candidate_count = ranking.ranked.len().saturating_add(ranking.excluded.len());
    DnsRankingView {
        ranked: ranking
            .ranked
            .iter()
            .map(|candidate| DnsRankedCandidate {
                name: candidate.name.clone(),
                operator: candidate.operator.clone(),
                score: Some(candidate.score),
                latency_score: Some(candidate.latency_score),
                reliability_score: Some(candidate.reliability_score),
                dnssec_score: Some(candidate.dnssec_score),
                privacy_score: Some(candidate.privacy_score),
                median_latency_ms: Some(candidate.median_latency_ms),
                p95_latency_ms: Some(candidate.p95_latency_ms),
                failure_rate: Some(candidate.failure_rate),
                samples: Some(u64::try_from(candidate.samples).unwrap_or(u64::MAX)),
                privacy_policy: dns_privacy_policy(candidate.privacy_policy),
                supports_dnssec: Some(candidate.supports_dnssec),
                supports_filtering: Some(candidate.supports_filtering),
            })
            .collect(),
        excluded: ranking
            .excluded
            .iter()
            .map(|excluded| DnsExcludedCandidate {
                name: excluded.name.clone(),
                reason: dns_exclusion_reason(excluded.reason),
                detail: excluded.detail.clone(),
            })
            .collect(),
        note: ranking.note.clone(),
        coverage: DataCoverage {
            completeness: if candidate_count == 0 {
                DataCompleteness::Unknown
            } else {
                DataCompleteness::Complete
            },
            unreadable_entries: None,
            unknown_size_entries: None,
            excluded_areas: Vec::new(),
            note: Some(
                "Covers every candidate and measurement supplied to this session; excluded \
                 candidates are listed with a reason. CoreSight contacted no resolver and \
                 changed no DNS setting."
                    .to_string(),
            ),
        },
    }
}

fn domain_category_code(category: DomainCategory) -> &'static str {
    match category {
        DomainCategory::Advertising => "ADVERTISING",
        DomainCategory::Tracking => "TRACKING",
        DomainCategory::Malware => "MALWARE",
        DomainCategory::Adult => "ADULT",
        DomainCategory::Gambling => "GAMBLING",
        DomainCategory::SocialMedia => "SOCIAL_MEDIA",
        DomainCategory::Custom => "CUSTOM",
    }
}

fn privacy_kind(kind: PrivacyFindingKind) -> PrivacyFindingKindView {
    match kind {
        PrivacyFindingKind::TrackingCookie => PrivacyFindingKindView::TrackingCookie,
        PrivacyFindingKind::SessionCookie => PrivacyFindingKindView::SessionCookie,
        PrivacyFindingKind::BrowsingHistory => PrivacyFindingKindView::BrowsingHistory,
        PrivacyFindingKind::CacheEntry => PrivacyFindingKindView::CacheEntry,
        PrivacyFindingKind::SiteStorage => PrivacyFindingKindView::SiteStorage,
        PrivacyFindingKind::Unknown => PrivacyFindingKindView::Unknown,
    }
}

// The wording helpers are defined next to the original opportunity
// projection in `dispatch.rs`, so both surfaces present one wording.
use crate::dispatch::{category_title, subcategory_name, what_it_is};
