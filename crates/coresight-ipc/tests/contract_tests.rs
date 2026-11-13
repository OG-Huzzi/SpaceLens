//! Contract tests: wire names, camelCase payloads, JSON round-trips for every
//! command and every response, and envelope exclusivity.

mod common;

use common::*;
use coresight_classifier::Category as ClassifierCategory;
use coresight_ipc::{
    codes, dispatch, dispatch_json, ApiError, CancelScanRequest, CancelScanResponse, Command,
    ConfirmPlanRequest, ConfirmPlanResponse, DriveKind, ExecutePlanRequest, ExecutePlanResponse,
    GetCategoriesRequest, GetDriveSummaryRequest, GetHistoryRequest, GetOpportunitiesRequest,
    ListDrivesRequest, PlanRejection, PlanResult, PreviewPlanRequest, Response,
    SnapshotCategoryDelta, SnapshotDelta, StartScanRequest, StartScanResponse,
};

fn every_command() -> Vec<Command> {
    vec![
        Command::StartScan(StartScanRequest {
            drive_id: DRIVE_ID.to_string(),
            full_rescan: Some(true),
        }),
        Command::CancelScan(CancelScanRequest {
            scan_id: SCAN_ID.to_string(),
        }),
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: Some("CACHE".to_string()),
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
        Command::ExecutePlan(ExecutePlanRequest {
            plan_id: "plan:scan-1:CACHE:30000".to_string(),
        }),
        Command::GetHistory(GetHistoryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
        Command::ListDrives(ListDrivesRequest {}),
    ]
}

#[test]
fn commands_serialize_with_namespaced_discriminants_and_camel_case_payloads() {
    let commands = every_command();
    assert_eq!(commands.len(), Command::ALL_NAMES.len());

    for command in &commands {
        let value = serde_json::to_value(command).expect("command serializes");
        assert_eq!(
            value.get("command").and_then(|v| v.as_str()),
            Some(command.name()),
            "discriminant must be the full coresight.v1 name"
        );
        assert!(
            command.name().starts_with("coresight.v1."),
            "every command is namespaced"
        );
        let text = value.to_string();
        assert!(
            !text.contains("scan_id") && !text.contains("drive_id"),
            "command payloads are camelCase, got {text}"
        );
    }

    let start = serde_json::to_value(&commands[0]).expect("startScan serializes");
    assert_eq!(start["payload"]["driveId"], "drive-c");
    assert_eq!(start["payload"]["fullRescan"], true);

    let categories = serde_json::to_value(&commands[3]).expect("getCategories serializes");
    assert_eq!(categories["payload"]["scanId"], "scan-1");
    assert_eq!(categories["payload"]["parentCategory"], "CACHE");

    let confirm = serde_json::to_value(&commands[6]).expect("confirmPlan serializes");
    assert_eq!(confirm["payload"]["planId"], "plan:scan-1:CACHE:30000");
    assert_eq!(confirm["payload"]["ackedWarnings"], serde_json::json!([]));

    let mut names = std::collections::BTreeSet::new();
    for command in &commands {
        assert!(names.insert(command.name()), "command names are unique");
    }
    assert_eq!(names.len(), 10);
}

#[test]
fn every_command_survives_json_round_trip() {
    for command in every_command() {
        let text = serde_json::to_string(&command).expect("command serializes");
        let parsed: Command = serde_json::from_str(&text).expect("command parses back");
        assert_eq!(parsed, command, "round-trip changed {text}");
    }
}

#[test]
fn unknown_command_is_a_typed_error_not_a_guess() {
    let mut session = sample_session();
    let envelope = dispatch_json(
        &mut session,
        r#"{"command":"coresight.v1.teleport","payload":{}}"#,
    );
    let error = envelope.error().expect("unknown command is an error");
    assert_eq!(error.code, codes::UNKNOWN_COMMAND);
    assert!(envelope.ok_value().is_none());
    assert!(!error.message.contains("panicked"));
}

#[test]
fn malformed_requests_are_typed_invalid_requests() {
    let mut session = sample_session();
    for request in [
        "not json at all",
        "[]",
        r#"{"payload":{}}"#,
        r#"{"command":"coresight.v1.getCategories","payload":{}}"#,
        r#"{"command":"coresight.v1.getCategories","payload":{"scanId":7}}"#,
        r#"{"command":"coresight.v1.listDrives","payload":null}"#,
    ] {
        let envelope = dispatch_json(&mut session, request);
        let error = envelope
            .error()
            .unwrap_or_else(|| panic!("request {request} must be refused"));
        assert_eq!(error.code, codes::INVALID_REQUEST, "request: {request}");
        assert!(envelope.ok_value().is_none());
    }
}

#[test]
fn envelope_serializes_exactly_one_side() {
    let ok = serde_json::to_value(coresight_ipc::Envelope::ok(7u8)).expect("ok serializes");
    let object = ok.as_object().expect("envelope is an object");
    assert_eq!(object.len(), 1, "only `ok` is present: {ok}");
    assert_eq!(object.get("ok"), Some(&serde_json::json!(7)));

    let error = serde_json::to_value(coresight_ipc::Envelope::<u8>::err(ApiError::new(
        codes::INVALID_REQUEST,
        "bad request",
    )))
    .expect("error serializes");
    let object = error.as_object().expect("envelope is an object");
    assert_eq!(object.len(), 1, "only `error` is present: {error}");
    assert!(object.get("error").is_some());
}

#[test]
fn every_response_survives_json_round_trip() {
    let mut session = sample_session();
    let mut responses = vec![
        Response::StartScan(StartScanResponse {
            scan_id: "scan-1".to_string(),
        }),
        Response::CancelScan(CancelScanResponse { cancelled: true }),
        Response::ConfirmPlan(ConfirmPlanResponse {
            accepted: false,
            rejected: vec![PlanRejection {
                code: "REVERSIBLE_NOT_AUTHORIZED".to_string(),
                message: "reversible actions are not authorized".to_string(),
            }],
        }),
        Response::ExecutePlan(ExecutePlanResponse {
            results: vec![PlanResult {
                item_id: "scan-1:CACHE:30000".to_string(),
                outcome: "moved".to_string(),
                bytes: 30_000,
            }],
            freed_bytes: 30_000,
            freed_complete: true,
            leftovers: Vec::new(),
        }),
        Response::History(vec![SnapshotDelta {
            from_scan_id: "scan-1".to_string(),
            to_scan_id: "scan-2".to_string(),
            from_at: "2023-11-14T22:13:20Z".to_string(),
            to_at: "2023-11-14T23:13:20Z".to_string(),
            total_bytes_delta: 10_000,
            complete: true,
            categories: vec![SnapshotCategoryDelta {
                category_id: "CACHE".to_string(),
                bytes_delta: 10_000,
                complete: true,
            }],
        }]),
    ];

    let summary = dispatch(
        &mut session,
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: DRIVE_ID.to_string(),
        }),
    )
    .into_result()
    .expect("drive summary");
    responses.push(summary);

    let categories = dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    )
    .into_result()
    .expect("categories");
    responses.push(categories);

    let opportunities = dispatch(
        &mut session,
        Command::GetOpportunities(GetOpportunitiesRequest {
            scan_id: SCAN_ID.to_string(),
        }),
    )
    .into_result()
    .expect("opportunities");
    responses.push(opportunities);

    let preview = dispatch(
        &mut session,
        Command::PreviewPlan(PreviewPlanRequest {
            opportunity_ids: vec!["scan-1:CACHE:30000".to_string()],
        }),
    )
    .into_result()
    .expect("preview");
    responses.push(preview);

    let drives = dispatch(&mut session, Command::ListDrives(ListDrivesRequest {}))
        .into_result()
        .expect("drives");
    responses.push(drives);

    let mut seen_kinds = std::collections::BTreeSet::new();
    for response in &responses {
        let kind = response.command_name();
        assert!(Command::ALL_NAMES.contains(&kind));
        assert!(seen_kinds.insert(kind), "one response per command kind");

        let text = serde_json::to_string(response).expect("response serializes");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["kind"], kind);
        let parsed: Response = serde_json::from_str(&text).expect("response parses back");
        assert_eq!(&parsed, response, "round-trip changed {text}");
    }
    assert_eq!(seen_kinds.len(), 10, "all ten command responses covered");
}

#[test]
fn category_payload_has_no_paths_and_uses_camel_case() {
    let mut session = sample_session();
    let response = dispatch(
        &mut session,
        Command::GetCategories(GetCategoriesRequest {
            scan_id: SCAN_ID.to_string(),
            parent_category: None,
        }),
    )
    .into_result()
    .expect("categories");
    let text = serde_json::to_string(&response).expect("serializes");
    assert!(
        !text.contains("\"path\""),
        "categories must never expose paths: {text}"
    );
    assert!(text.contains("\"shareOfParent\""));
    assert!(text.contains("\"deltaSinceLast\"") || text.contains("\"itemCount\""));
    assert!(text.contains("\"entriesWithoutSize\""));
    // Byte counts stay integers, not strings.
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    let rows = value["payload"].as_array().expect("category array");
    assert_eq!(rows.len(), ClassifierCategory::ALL.len());
    assert!(rows[0]["bytes"].is_u64());
}

#[test]
fn error_codes_are_stable_and_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for code in codes::ALL {
        assert!(seen.insert(code), "duplicate error code {code}");
        assert!(
            code.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
            "codes are SCREAMING_SNAKE_CASE: {code}"
        );
    }
    for required in [
        codes::SCAN_IN_PROGRESS,
        codes::DRIVE_OFFLINE,
        codes::PLAN_REJECTED_UNSAFE,
        codes::NOTHING_TO_DO,
        codes::PERMISSION_GAP,
        codes::DB_CORRUPT_RESCANNED,
        codes::UNKNOWN_COMMAND,
        codes::SCAN_NOT_FOUND,
        codes::SCAN_UNAVAILABLE,
        codes::HISTORY_UNAVAILABLE,
        codes::NOT_IMPLEMENTED_IN_THIS_BUILD,
        codes::INVALID_REQUEST,
    ] {
        assert!(seen.contains(required), "missing required code {required}");
    }
}

#[test]
fn plan_rejection_reasons_round_trip_through_detail() {
    let reasons = vec![
        "GATE_BLOCKED: CACHE:30000: REVERSIBLE_NOT_AUTHORIZED".to_string(),
        "PARTIAL_KNOWLEDGE: test".to_string(),
    ];
    let error = ApiError::plan_rejected(&reasons);
    assert_eq!(error.code, codes::PLAN_REJECTED_UNSAFE);
    assert_eq!(error.rejection_reasons(), reasons);
    assert!(error.message.contains("read-only"));
    assert!(!error.message.contains("stack"));
    let value = serde_json::to_value(&error).expect("error serializes");
    assert_eq!(value["reasons"], serde_json::json!(reasons));
    assert!(value.get("detail").is_some());

    let plain = ApiError::new(codes::INVALID_REQUEST, "x");
    assert!(plain.rejection_reasons().is_empty());
    let plain = serde_json::to_value(&plain).expect("error serializes");
    assert!(
        plain.get("reasons").is_none(),
        "only plan rejections carry reasons[]"
    );
}

#[test]
fn drive_kind_is_lowercase_on_the_wire() {
    let text = serde_json::to_string(&DriveKind::Removable).expect("serializes");
    assert_eq!(text, "\"removable\"");
}
