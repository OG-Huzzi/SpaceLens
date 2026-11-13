//! Behaviour tests for the dispatcher: the honesty rules, category maths,
//! opportunity ordering, plan safety rejection, and determinism.

mod common;

use common::*;
use coresight_classifier::{Category as ClassifierCategory, Subcategory};
use coresight_ipc::{
    codes, dispatch, ApiError, Category, Command, ConfirmPlanRequest, Drive, GetCategoriesRequest,
    GetDriveSummaryRequest, GetHistoryRequest, GetOpportunitiesRequest, ListDrivesRequest,
    PlanPreview, PlanPreviewItem, PreviewPlanRequest, Response, ScanRecord, ScanStatus, Session,
};

fn categories(envelope: coresight_ipc::Envelope<Response>) -> Vec<Category> {
    match envelope.into_result() {
        Ok(Response::Categories(rows)) => rows,
        other => panic!("expected categories response, got {other:?}"),
    }
}

fn opportunities(envelope: coresight_ipc::Envelope<Response>) -> Vec<coresight_ipc::Opportunity> {
    match envelope.into_result() {
        Ok(Response::Opportunities(rows)) => rows,
        other => panic!("expected opportunities response, got {other:?}"),
    }
}

fn preview(envelope: coresight_ipc::Envelope<Response>) -> PlanPreview {
    match envelope.into_result() {
        Ok(Response::PlanPreview(preview)) => preview,
        other => panic!("expected plan preview, got {other:?}"),
    }
}

fn error(envelope: coresight_ipc::Envelope<Response>) -> ApiError {
    match envelope.into_result() {
        Err(error) => error,
        Ok(other) => panic!("expected typed error, got {other:?}"),
    }
}

fn category_row<'a>(rows: &'a [Category], id: &str) -> &'a Category {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("category {id} missing from {rows:?}"))
}

fn preview_cache(session: &mut Session) -> PlanPreview {
    preview(dispatch(
        session,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec!["scan-1:CACHE:30000".to_string()],
        }),
    ))
}

#[test]
fn start_scan_and_cancel_scan_report_no_backend() {
    let mut session = sample_session();
    let start = error(dispatch(
        &mut session,
        Command::StartScan(coresight_ipc::StartScanRequest {
            drive_id: DRIVE_ID.to_string(),
            full_rescan: None,
            root: None,
        }),
    ));
    assert_eq!(start.code, codes::SCAN_UNAVAILABLE);
    assert!(start.message.to_lowercase().contains("scan backend"));
    assert!(start.detail.unwrap_or_default().contains("no scanner"));

    let cancel = error(dispatch(
        &mut session,
        Command::CancelScan(coresight_ipc::CancelScanRequest {
            scan_id: SCAN_ID.to_string(),
        }),
    ));
    assert_eq!(cancel.code, codes::SCAN_UNAVAILABLE);
    assert!(cancel.message.to_lowercase().contains("scan backend"));
}

#[test]
fn get_history_reports_no_store() {
    let mut session = sample_session();
    let history = error(dispatch(
        &mut session,
        Command::GetHistory(GetHistoryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
    ));
    assert_eq!(history.code, codes::HISTORY_UNAVAILABLE);
    assert!(history.message.to_lowercase().contains("history store"));
}

#[test]
fn list_drives_returns_connected_then_remembered_and_empty_is_honest() {
    let mut session = Session::new();
    let empty = dispatch(&mut session, Command::ListDrives(ListDrivesRequest {}))
        .into_result()
        .expect("empty list is a fact");
    match empty {
        Response::Drives(drives) => assert!(drives.is_empty(), "no fabricated drives"),
        other => panic!("expected drives, got {other:?}"),
    }

    session.register_drive(offline_drive());
    session.register_drive(drive());
    let response = dispatch(&mut session, Command::ListDrives(ListDrivesRequest {}))
        .into_result()
        .expect("drives");
    let drives = match response {
        Response::Drives(drives) => drives,
        other => panic!("expected drives, got {other:?}"),
    };
    assert_eq!(drives.len(), 2);
    assert!(drives[0].connected, "connected drives sort first");
    assert_eq!(drives[0].id, DRIVE_ID);
    assert!(!drives[1].connected);
    assert_eq!(drives[1].kind, coresight_ipc::DriveKind::Removable);
}

#[test]
fn drive_summary_reports_totals_and_rfc3339_timestamp() {
    let mut session = sample_session();
    let response = dispatch(
        &mut session,
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
    )
    .into_result()
    .expect("drive summary");
    let summary = match response {
        Response::DriveSummary(summary) => summary,
        other => panic!("expected drive summary, got {other:?}"),
    };
    assert!(summary.connected);
    let totals = summary.totals.expect("a completed scan has totals");
    assert_eq!(totals.scan_id, SCAN_ID);
    assert_eq!(totals.bytes, 236_000);
    assert!(totals.complete);
    assert_eq!(totals.scanned_entries, 6);
    assert_eq!(totals.entries_without_size, 0);
    assert_eq!(totals.errored_entries, 0);
    assert_eq!(totals.unclassified_entries, 1);

    let last = summary.last_scan.expect("last scan");
    assert_eq!(last.scan_id, SCAN_ID);
    assert_eq!(last.status, ScanStatus::Completed);
    assert_eq!(last.completed_at.as_deref(), Some("2023-11-14T22:13:20Z"));
    assert!(last.complete_scope);
    assert_eq!(summary.snapshot_count, 1);
    assert_eq!(
        summary.exclusions_note, None,
        "not recorded is not `nothing excluded`"
    );
}

#[test]
fn drive_summary_refuses_unknown_and_empty_offline_drives() {
    let mut session = Session::new();
    let unknown = error(dispatch(
        &mut session,
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: "nope".to_string(),
        }),
    ));
    assert_eq!(unknown.code, codes::DRIVE_NOT_FOUND);

    session.register_drive(offline_drive());
    let offline = error(dispatch(
        &mut session,
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: "drive-usb".to_string(),
        }),
    ));
    assert_eq!(offline.code, codes::DRIVE_OFFLINE);
}

#[test]
fn offline_drive_with_a_retained_snapshot_still_answers() {
    let mut session = Session::new();
    session.register_drive(Drive {
        connected: false,
        ..offline_drive()
    });
    session
        .register_scan(ScanRecord {
            scan_id: "scan-usb".to_string(),
            drive_id: "drive-usb".to_string(),
            snapshot: sample_snapshot(),
            completed_at: Some(at(1_700_000_000)),
            status: ScanStatus::Completed,
        })
        .expect("offline drive scan registers");
    let response = dispatch(
        &mut session,
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: "drive-usb".to_string(),
        }),
    )
    .into_result()
    .expect("remembered facts answer offline");
    match response {
        Response::DriveSummary(summary) => {
            assert!(!summary.connected);
            assert_eq!(summary.totals.map(|t| t.bytes), Some(236_000));
        }
        other => panic!("expected drive summary, got {other:?}"),
    }
}

#[test]
fn categories_are_in_canonical_order_with_honest_shares() {
    let mut session = sample_session();
    let rows = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    assert_eq!(rows.len(), ClassifierCategory::ALL.len());
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let expected: Vec<&str> = ClassifierCategory::ALL
        .iter()
        .map(|category| category.code())
        .collect();
    assert_eq!(ids, expected, "Category::ALL is the canonical order");

    let cache = category_row(&rows, "CACHE");
    assert_eq!(cache.bytes, 30_000);
    assert_eq!(cache.item_count, 1);
    assert!(cache.complete);
    let share = cache.share_of_parent.expect("complete parent has a share");
    let expected_share = 30_000.0 / 236_000.0;
    assert!(
        (share - expected_share).abs() < 1e-12,
        "share {share} != {expected_share}"
    );
    assert_eq!(cache.delta_since_last, None, "no previous scan is unknown");

    let downloads = category_row(&rows, "DOWNLOADS");
    assert_eq!(downloads.bytes, 80_000);
    assert_eq!(downloads.name, "Downloads");
}

#[test]
fn share_of_parent_is_none_when_parent_total_is_zero() {
    let snapshot = common::snapshot(vec![(
        common::file("/scan/empty/zero.bin", 0),
        common::classified(ClassifierCategory::Cache),
    )]);
    let mut session = session_with_snapshot(snapshot, ScanStatus::Completed);
    let rows = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    let cache = category_row(&rows, "CACHE");
    assert_eq!(cache.bytes, 0);
    assert_eq!(cache.item_count, 1);
    assert_eq!(
        cache.share_of_parent, None,
        "a zero parent total makes every share unknown, not 0%"
    );
    assert!(cache.complete, "a known zero-byte entry is complete");
}

#[test]
fn complete_scan_marks_zero_byte_category_share_as_true_zero() {
    let mut session = sample_session();
    let rows = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    let applications = category_row(&rows, "APPLICATIONS");
    assert_eq!(applications.bytes, 0);
    assert_eq!(applications.item_count, 0);
    assert_eq!(
        applications.share_of_parent,
        Some(0.0),
        "a real 0% share is representable when the parent total is known"
    );
}

#[test]
fn incomplete_scan_makes_shares_unknown_and_marks_rows_incomplete() {
    let snapshot = common::snapshot(vec![
        (
            common::file("/scan/cache/a.bin", 30_000),
            common::classified(ClassifierCategory::Cache),
        ),
        (
            common::dir("/scan/cache/sub"),
            common::classified(ClassifierCategory::Cache),
        ),
        (
            common::permission_denied("/scan/cache/locked.bin"),
            common::classified(ClassifierCategory::Cache),
        ),
    ]);
    let mut session = session_with_snapshot(snapshot, ScanStatus::Completed);
    let rows = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    let cache = category_row(&rows, "CACHE");
    assert_eq!(cache.bytes, 30_000);
    assert_eq!(cache.item_count, 3);
    assert_eq!(cache.entries_without_size, 1);
    assert!(
        !cache.complete,
        "an unmeasured entry makes the bucket partial"
    );
    assert_eq!(
        cache.share_of_parent, None,
        "an incomplete parent total is unknown"
    );
}

#[test]
fn category_delta_uses_the_previous_full_scope_scan() {
    let mut session = session_with_two_scans();
    let newest = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID_2.to_string(),
            parent_category: None,
        }),
    ));
    let cache = category_row(&newest, "CACHE");
    assert_eq!(cache.bytes, 40_000);
    assert_eq!(cache.delta_since_last, Some(10_000));

    let oldest = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    assert_eq!(
        category_row(&oldest, "CACHE").delta_since_last,
        None,
        "the first scan has no baseline"
    );
}

#[test]
fn subcategory_drilldown_is_real_and_parent_scoped() {
    let mut session = sample_session();
    let downloads = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: Some("DOWNLOADS".to_string()),
        }),
    ));
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].id, Subcategory::Archive.code());
    assert_eq!(downloads[0].name, "Archive");
    assert_eq!(downloads[0].bytes, 80_000);
    assert_eq!(downloads[0].item_count, 1);
    assert_eq!(downloads[0].share_of_parent, Some(1.0));
    assert!(downloads[0].complete);

    let cache = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: Some("CACHE".to_string()),
        }),
    ));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache[0].id, "UNSUBCATEGORIZED");
    assert_eq!(cache[0].bytes, 30_000);

    let empty = categories(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: Some("APPLICATIONS".to_string()),
        }),
    ));
    assert!(empty.is_empty(), "no invented empty subcategory rows");

    let bad = error(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: Some("NOPE".to_string()),
        }),
    ));
    assert_eq!(bad.code, codes::INVALID_REQUEST);
}

#[test]
fn opportunities_follow_the_planner_order_and_explain_themselves() {
    let mut session = sample_session();
    let rows = opportunities(dispatch(
        &mut session,
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: SCAN_ID.to_string(),
        }),
    ));
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "scan-1:CACHE:30000",
            "scan-1:TEMPORARY_DATA:20000",
            "scan-1:LOGS:5000",
            "scan-1:DOWNLOADS:80000"
        ],
        "planner order: Safe first by bytes, then Review"
    );
    assert_eq!(rows[0].tier, coresight_ipc::OpportunityTier::Safe);
    assert_eq!(rows[3].tier, coresight_ipc::OpportunityTier::Review);
    for row in &rows {
        assert!(!row.what_it_is.is_empty(), "{} has no explanation", row.id);
        assert!(!row.why_recommended.is_empty());
        assert!(!row.stays_untouched.is_empty());
        assert!(!row.consequence.is_empty());
        assert_eq!(row.bytes, row.recoverable_bytes);
        assert!(!row.preview.summary.is_empty());
        assert_eq!(
            row.preview.destination.as_deref(),
            Some("OS Trash / app quarantine")
        );
    }
    assert_eq!(rows.len(), 4, "off-limits categories are never actionable");
}

#[test]
fn preview_plan_pins_exact_targets() {
    let mut session = sample_session();
    let plan = preview(dispatch(
        &mut session,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec![
                "scan-1:CACHE:30000".to_string(),
                "scan-1:LOGS:5000".to_string(),
            ],
        }),
    ));
    assert_eq!(plan.plan_id, "plan:scan-1:CACHE:30000+scan-1:LOGS:5000");
    assert_eq!(plan.bytes, 35_000);
    assert_eq!(plan.claimed_bytes, 35_000);
    assert!(plan.complete);
    assert!(plan.warnings.is_empty());
    assert_eq!(
        plan.destination.as_deref(),
        Some("OS Trash / app quarantine")
    );
    assert_eq!(
        plan.undo_path.as_deref(),
        Some("Restore from Trash/quarantine")
    );
    assert_eq!(plan.items.len(), 2);
    let cache_target: &PlanPreviewItem = plan
        .items
        .iter()
        .find(|item| item.item_id == "scan-1:CACHE:30000")
        .expect("cache target");
    assert_eq!(cache_target.path, "u:/scan/cache/a.bin");
    assert_eq!(cache_target.bytes, 30_000);
    assert_eq!(cache_target.kind, coresight_ipc::PreviewTargetKind::File);
    let log_target: &PlanPreviewItem = plan
        .items
        .iter()
        .find(|item| item.item_id == "scan-1:LOGS:5000")
        .expect("log target");
    assert_eq!(log_target.path, "u:/scan/logs/app.log");
}

#[test]
fn preview_plan_rejects_unknown_empty_duplicate_and_cross_scan_selections() {
    let mut session = sample_session();

    let empty = error(dispatch(
        &mut session,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: Vec::new(),
        }),
    ));
    assert_eq!(empty.code, codes::NOTHING_TO_DO);

    let unknown = error(dispatch(
        &mut session,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec!["scan-1:CACHE:1".to_string()],
        }),
    ));
    assert_eq!(unknown.code, codes::UNKNOWN_OPPORTUNITY);

    let duplicate = error(dispatch(
        &mut session,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec![
                "scan-1:CACHE:30000".to_string(),
                "scan-1:CACHE:30000".to_string(),
            ],
        }),
    ));
    assert_eq!(duplicate.code, codes::INVALID_REQUEST);
    assert!(duplicate.detail.unwrap_or_default().contains("duplicate"));

    let mut two = session_with_two_scans();
    let cross_scan = error(dispatch(
        &mut two,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec![
                "scan-1:CACHE:30000".to_string(),
                "scan-2:CACHE:40000".to_string(),
            ],
        }),
    ));
    assert_eq!(cross_scan.code, codes::INVALID_REQUEST);

    let mut ambiguous = session_with_two_scans();
    // Two scans may legitimately contain the same category totals; the
    // scan-scoped ids must keep their cards distinct and previewable.
    ambiguous
        .register_scan(ScanRecord {
            scan_id: "scan-3".to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot: sample_snapshot_with_cache(30_000),
            completed_at: Some(at(1_700_010_000)),
            status: ScanStatus::Completed,
        })
        .expect("third scan registers");
    let rows_one = opportunities(dispatch(
        &mut ambiguous,
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: SCAN_ID.to_string(),
        }),
    ));
    let rows_three = opportunities(dispatch(
        &mut ambiguous,
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: "scan-3".to_string(),
        }),
    ));
    assert_eq!(rows_one[0].bytes, rows_three[0].bytes);
    assert_ne!(
        rows_one[0].id, rows_three[0].id,
        "equal category totals in different scans must not share an id"
    );
    let plan = preview(dispatch(
        &mut ambiguous,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec![rows_three[0].id.clone()],
        }),
    ));
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].item_id, rows_three[0].id);
}

#[test]
fn confirm_plan_rejects_with_plan_rejected_unsafe_and_reasons() {
    let mut session = sample_session();
    let plan = preview_cache(&mut session);
    let response = dispatch(
        &mut session,
        Command::ConfirmPlan(ConfirmPlanRequest {
            plan_id: plan.plan_id.clone(),
            acked_warnings: plan.warnings.clone(),
        }),
    );
    let rejection = error(response);
    assert_eq!(rejection.code, codes::PLAN_REJECTED_UNSAFE);
    assert!(rejection.message.contains("read-only"));
    let reasons = rejection.rejection_reasons();
    assert!(!reasons.is_empty(), "rejection must carry its reasons");
    let joined = reasons.join("\n");
    assert!(joined.contains("GATE_BLOCKED"), "got {joined}");
    assert!(
        joined.contains("REVERSIBLE_NOT_AUTHORIZED"),
        "current policy authorizes read-only only, got {joined}"
    );
    assert!(
        !joined.contains("backtrace") && !joined.contains("stack"),
        "reasons are not a stack trace"
    );
}

#[test]
fn execute_plan_refuses_with_the_same_policy_rejection() {
    let mut session = sample_session();
    let plan = preview_cache(&mut session);
    let execution = error(dispatch(
        &mut session,
        Command::ExecutePlan(coresight_ipc::ExecutePlanRequest {
            plan_id: plan.plan_id.clone(),
        }),
    ));
    assert_eq!(execution.code, codes::PLAN_REJECTED_UNSAFE);
    let reasons = execution.rejection_reasons();
    assert!(!reasons.is_empty());
    assert!(
        reasons.join("\n").contains("REVERSIBLE_NOT_AUTHORIZED"),
        "executePlan must evaluate the same policy veto"
    );

    let unknown = error(dispatch(
        &mut session,
        Command::ExecutePlan(coresight_ipc::ExecutePlanRequest {
            plan_id: "plan:nope".to_string(),
        }),
    ));
    assert_eq!(unknown.code, codes::PLAN_NOT_FOUND);

    let unknown_confirm = error(dispatch(
        &mut session,
        Command::ConfirmPlan(ConfirmPlanRequest {
            plan_id: "plan:nope".to_string(),
            acked_warnings: Vec::new(),
        }),
    ));
    assert_eq!(unknown_confirm.code, codes::PLAN_NOT_FOUND);
}

#[test]
fn scan_lookup_distinguishes_running_cancelled_and_unknown() {
    let mut running = session_with_snapshot(sample_snapshot(), ScanStatus::Running);
    let in_progress = error(dispatch(
        &mut running,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    assert_eq!(in_progress.code, codes::SCAN_IN_PROGRESS);
    let in_progress = error(dispatch(
        &mut running,
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: SCAN_ID.to_string(),
        }),
    ));
    assert_eq!(in_progress.code, codes::SCAN_IN_PROGRESS);

    let mut cancelled = session_with_snapshot(sample_snapshot(), ScanStatus::Cancelled);
    let unavailable = error(dispatch(
        &mut cancelled,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    ));
    assert_eq!(unavailable.code, codes::SCAN_UNAVAILABLE);

    let mut session = sample_session();
    let missing = error(dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: "scan-missing".to_string(),
            parent_category: None,
        }),
    ));
    assert_eq!(missing.code, codes::SCAN_NOT_FOUND);
}

#[test]
fn dispatch_is_deterministic_for_repeated_commands() {
    let mut session = sample_session();
    let command = || {
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        })
    };
    let first = serde_json::to_string(&dispatch(&mut session, command())).expect("serialize");
    let second = serde_json::to_string(&dispatch(&mut session, command())).expect("serialize");
    assert_eq!(
        first, second,
        "same session + same command is byte-identical"
    );

    let opportunities = || {
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: SCAN_ID.to_string(),
        })
    };
    let first = serde_json::to_string(&dispatch(&mut session, opportunities())).expect("serialize");
    let second =
        serde_json::to_string(&dispatch(&mut session, opportunities())).expect("serialize");
    assert_eq!(first, second);

    let preview_command = || {
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec!["scan-1:CACHE:30000".to_string()],
        })
    };
    let first =
        serde_json::to_string(&dispatch(&mut session, preview_command())).expect("serialize");
    let second =
        serde_json::to_string(&dispatch(&mut session, preview_command())).expect("serialize");
    assert_eq!(first, second, "re-previewing the same selection is stable");

    let plan = preview_cache(&mut session);
    let confirm_command = || {
        Command::ConfirmPlan(ConfirmPlanRequest {
            plan_id: plan.plan_id.clone(),
            acked_warnings: plan.warnings.clone(),
        })
    };
    let first =
        serde_json::to_string(&dispatch(&mut session, confirm_command())).expect("serialize");
    let second =
        serde_json::to_string(&dispatch(&mut session, confirm_command())).expect("serialize");
    assert_eq!(first, second);
}

#[test]
fn every_command_variant_dispatches_without_panicking() {
    let mut session = sample_session();
    let commands = vec![
        Command::StartScan(coresight_ipc::StartScanRequest {
            drive_id: DRIVE_ID.to_string(),
            full_rescan: None,
            root: None,
        }),
        Command::CancelScan(coresight_ipc::CancelScanRequest {
            scan_id: SCAN_ID.to_string(),
        }),
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: SCAN_ID.to_string(),
        }),
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec!["scan-1:CACHE:30000".to_string()],
        }),
        Command::ConfirmPlan(ConfirmPlanRequest {
            plan_id: "plan:scan-1:CACHE:30000".to_string(),
            acked_warnings: Vec::new(),
        }),
        Command::ExecutePlan(coresight_ipc::ExecutePlanRequest {
            plan_id: "plan:scan-1:CACHE:30000".to_string(),
        }),
        Command::GetHistory(GetHistoryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
        Command::ListDrives(ListDrivesRequest {}),
    ];
    for command in commands {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = dispatch(&mut session, command);
        }));
        assert!(result.is_ok(), "a command variant panicked");
    }
}
