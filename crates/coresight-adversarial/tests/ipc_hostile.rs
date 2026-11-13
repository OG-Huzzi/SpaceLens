//! Hostile inputs aimed at the IPC envelope: command parsing, error typing,
//! envelope exclusivity, and response-field honesty.
//!
//! Every fixture is synthetic; no IPC transport, filesystem, or session
//! persistence is involved. Each test names the honesty property it protects.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use coresight_classifier::Category;
use coresight_history::{ClassificationRef, ObservedEntry, ObservedKind, RunId, Snapshot};
use coresight_ipc::{
    dispatch_json, ApiError, Command, Drive, DriveKind, Envelope, GetDriveSummaryRequest, Response,
    ScanRecord, ScanStatus, ScanTotals, Session, SessionError,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn entry(path: &str, size: Option<u64>, category: Option<Category>) -> ObservedEntry {
    ObservedEntry {
        path: PathBuf::from(path),
        kind: ObservedKind::File,
        size,
        object: None,
        modified: None,
        classification: category.map(|category| ClassificationRef {
            category: category.code().to_string(),
            subcategory: None,
        }),
        content_sha256: None,
        observation_error: None,
    }
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("run-ipc".to_string()),
        entries,
    }
}

fn drive(id: &str) -> Drive {
    Drive {
        id: id.to_string(),
        root: "C:\\".to_string(),
        label: Some("Fixture".to_string()),
        kind: DriveKind::Internal,
        capacity_bytes: Some(1_000_000),
        available_bytes: Some(500_000),
        connected: true,
    }
}

fn scan(id: &str, drive_id: &str, status: ScanStatus, entries: Vec<ObservedEntry>) -> ScanRecord {
    ScanRecord {
        scan_id: id.to_string(),
        drive_id: drive_id.to_string(),
        snapshot: snapshot(entries),
        completed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
        status,
    }
}

fn session_with_scan(entries: Vec<ObservedEntry>) -> Session {
    let mut session = Session::new();
    session.register_drive(drive("drive-c"));
    session
        .register_scan(scan("scan-1", "drive-c", ScanStatus::Completed, entries))
        .expect("the fixture scan registers");
    session
}

fn error_of(envelope: &Envelope<Response>) -> &ApiError {
    envelope
        .error()
        .expect("the frame must be answered with a typed error")
}

fn error_code(envelope: &Envelope<Response>) -> &'static str {
    error_of(envelope).code
}

fn envelope_json(envelope: &Envelope<Response>) -> serde_json::Value {
    serde_json::to_value(envelope).expect("an envelope with a typed payload serializes")
}

// ---------------------------------------------------------------------------
// Command parsing: malformed, missing, unknown, extra
// ---------------------------------------------------------------------------

// If this regresses, a malformed frame panics the host process instead of
// producing a typed refusal.
#[test]
fn malformed_json_is_a_typed_error_never_a_panic() {
    let hostile = [
        String::new(),
        "{".to_string(),
        "not json at all".to_string(),
        "null".to_string(),
        "[]".to_string(),
        "\"coresight.v1.listDrives\"".to_string(),
        "42".to_string(),
        "[".repeat(10_000),
        format!("{{\"command\":{}}}", "\"x\"".repeat(1_000)),
    ];

    for frame in &hostile {
        let mut session = Session::new();
        let envelope = dispatch_json(&mut session, frame);
        assert!(envelope.is_err(), "frame {:.40?} must be refused", frame);
        assert_eq!(error_code(&envelope), "INVALID_REQUEST");
        assert!(!error_of(&envelope).message.trim().is_empty());
    }
}

// If this regresses, a frame without a command is defaulted into some command
// and a request nobody wrote is executed.
#[test]
fn a_missing_or_non_string_command_is_refused_not_defaulted() {
    for frame in [
        "{}",
        "{\"payload\":{}}",
        "{\"command\":42}",
        "{\"command\":null}",
        "{\"command\":[\"coresight.v1.listDrives\"]}",
        "{\"command\":{}}",
    ] {
        let mut session = Session::new();
        let envelope = dispatch_json(&mut session, frame);
        assert_eq!(
            error_code(&envelope),
            "INVALID_REQUEST",
            "frame {frame} must be a typed invalid request"
        );
    }
}

// If this regresses, a command from another contract version is routed to a
// v1 handler, or an unknown name is silently accepted.
#[test]
fn an_unknown_command_discriminant_is_never_guessed() {
    for name in [
        "coresight.v1.nope",
        "coresight.v2.getCategories",
        "CORESIGHT.V1.GETCATEGORIES",
        "getCategories",
        "",
        "coresight.v1.getCategories ",
    ] {
        let frame = serde_json::json!({ "command": name, "payload": {} });
        let mut session = Session::new();
        let envelope = dispatch_json(&mut session, &frame.to_string());
        assert_eq!(
            error_code(&envelope),
            "UNKNOWN_COMMAND",
            "name {name:?} must not route to a handler"
        );
        assert!(error_of(&envelope).detail.is_some());
    }
}

// If this regresses, a required payload field is silently defaulted (for
// example an empty drive id) and the request addresses something the caller
// never named.
#[test]
fn missing_required_payload_fields_are_invalid_not_defaulted() {
    let frames = [
        ("coresight.v1.startScan", "{}"),
        ("coresight.v1.cancelScan", "{}"),
        ("coresight.v1.getDriveSummary", "{}"),
        ("coresight.v1.getCategories", "{}"),
        ("coresight.v1.getOpportunities", "{}"),
        ("coresight.v1.previewPlan", "{}"),
        ("coresight.v1.confirmPlan", "{}"),
        ("coresight.v1.executePlan", "{}"),
        ("coresight.v1.getHistory", "{}"),
    ];

    for (command, payload) in frames {
        let frame = serde_json::json!({ "command": command, "payload": serde_json::from_str::<serde_json::Value>(payload).expect("object") });
        let mut session = Session::new();
        let envelope = dispatch_json(&mut session, &frame.to_string());
        assert_eq!(
            error_code(&envelope),
            "INVALID_REQUEST",
            "{command} must require its documented fields"
        );
        let detail = error_of(&envelope).detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains(command),
            "the detail must name the failing contract, got {detail}"
        );
    }
}

// If this regresses, a wrongly typed field (number for a string, string for a
// list) is coerced and the request proceeds with corrupted intent.
#[test]
fn wrongly_typed_payload_fields_are_invalid() {
    let frames = [
        serde_json::json!({"command":"coresight.v1.getDriveSummary","payload":{"driveId":42}}),
        serde_json::json!({"command":"coresight.v1.getCategories","payload":{"scanId":true}}),
        serde_json::json!({"command":"coresight.v1.previewPlan","payload":{"opportunityIds":"CACHE:1"}}),
        serde_json::json!({"command":"coresight.v1.confirmPlan","payload":{"planId":["x"]}}),
        serde_json::json!({"command":"coresight.v1.startScan","payload":{"driveId":"d","fullRescan":"yes"}}),
    ];

    for frame in frames {
        let mut session = Session::new();
        let envelope = dispatch_json(&mut session, &frame.to_string());
        assert_eq!(error_code(&envelope), "INVALID_REQUEST", "frame {frame}");
    }
}

// If this regresses, forward-compatible extra fields break v1 clients, or a
// payload made only of unknown fields is accepted as an empty request.
#[test]
fn extra_unknown_fields_are_ignored_without_defaulting_required_fields() {
    let frame = serde_json::json!({
        "command": "coresight.v1.getDriveSummary",
        "payload": { "driveId": "drive-c", "aFieldFromV1_1": 7 },
        "aFieldFromTheTransport": true
    });
    let command = Command::from_wire(&frame).expect("extra fields are additive");
    assert_eq!(
        command,
        Command::GetDriveSummary(GetDriveSummaryRequest {
            drive_id: "drive-c".to_string()
        })
    );

    let only_extras = serde_json::json!({
        "command": "coresight.v1.getDriveSummary",
        "payload": { "aFieldFromV1_1": 7 }
    });
    let error = Command::from_wire(&only_extras).expect_err("the required field is still required");
    assert_eq!(error.code, "INVALID_REQUEST");
}

// If this regresses, a non-object payload or a missing payload is treated as a
// valid empty object for every command.
#[test]
fn a_non_object_payload_is_invalid_for_a_command_that_needs_fields() {
    for payload in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!("x"),
    ] {
        let frame = serde_json::json!({
            "command": "coresight.v1.getDriveSummary",
            "payload": payload
        });
        let mut session = Session::new();
        assert_eq!(
            error_code(&dispatch_json(&mut session, &frame.to_string())),
            "INVALID_REQUEST"
        );
    }
}

// ---------------------------------------------------------------------------
// Envelope integrity
// ---------------------------------------------------------------------------

// If this regresses, a frame carries both a payload and an error (or neither)
// and the UI cannot decide what happened.
#[test]
fn an_envelope_carries_exactly_one_side() {
    // `Envelope`'s fields are private and the only constructors are `ok` and
    // `err`, so "both" and "neither" are structurally unrepresentable. This
    // test asserts the observable consequence over every envelope the
    // dispatcher can actually produce.
    let frames = [
        "{\"command\":\"coresight.v1.listDrives\",\"payload\":{}}".to_string(),
        "{\"command\":\"coresight.v1.getHistory\",\"payload\":{\"driveId\":\"d\"}}".to_string(),
        "{}".to_string(),
        "{\"command\":\"coresight.v1.nope\",\"payload\":{}}".to_string(),
    ];

    for frame in &frames {
        let mut session = Session::new();
        let envelope = dispatch_json(&mut session, frame);
        assert_ne!(
            envelope.is_ok(),
            envelope.is_err(),
            "exactly one side must be set for {frame}"
        );

        let value = envelope_json(&envelope);
        let object = value.as_object().expect("an envelope is a JSON object");
        let sides: Vec<&String> = object
            .keys()
            .filter(|key| key.as_str() == "ok" || key.as_str() == "error")
            .collect();
        assert_eq!(sides.len(), 1, "got keys {object:?}");
        assert_eq!(object.len(), 1, "no third envelope field may appear");

        match envelope.into_result() {
            Ok(_) => assert!(object.contains_key("ok")),
            Err(error) => {
                assert!(object.contains_key("error"));
                assert!(!error.message.is_empty());
            }
        }
    }
}

// If this regresses, an error serializes without its code or message and the
// UI has to invent one.
#[test]
fn an_error_always_carries_a_stable_code_and_a_message() {
    let mut session = Session::new();
    let envelope = dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.getHistory\",\"payload\":{\"driveId\":\"d\"}}",
    );
    let value = envelope_json(&envelope);
    let error = &value["error"];

    assert_eq!(error["code"], "HISTORY_UNAVAILABLE");
    assert!(error["message"]
        .as_str()
        .is_some_and(|text| !text.is_empty()));
    assert!(
        !error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stack"),
        "no error may expose a stack trace"
    );
}

// If this regresses, a plan rejection publishes the validator's reasons in one
// place but not the other.
#[test]
fn a_plan_rejection_publishes_its_reasons_as_a_typed_list() {
    let error = ApiError::plan_rejected(&["PARTIAL_KNOWLEDGE: fixture".to_string()]);
    let value = serde_json::to_value(&error).expect("an error serializes");

    assert_eq!(value["code"], "PLAN_REJECTED_UNSAFE");
    assert_eq!(value["reasons"][0], "PARTIAL_KNOWLEDGE: fixture");
    assert_eq!(
        error.rejection_reasons(),
        vec!["PARTIAL_KNOWLEDGE: fixture".to_string()]
    );

    // A non-rejection code must never grow a reasons array.
    let other = ApiError::new("INVALID_REQUEST", "no");
    assert!(!serde_json::to_value(&other)
        .expect("serializes")
        .as_object()
        .expect("object")
        .contains_key("reasons"));
}

// ---------------------------------------------------------------------------
// Response honesty: numbers vs unknown
// ---------------------------------------------------------------------------

// If this regresses, an unknown count is emitted as `0` or a number is emitted
// as `null`, and the UI cannot tell "none" from "not measured".
#[test]
fn numeric_fields_are_numbers_and_unknowns_are_null_never_zero() {
    let totals = ScanTotals {
        scan_id: "scan-1".to_string(),
        bytes: 0,
        complete: false,
        scanned_entries: 1,
        entries_without_size: 7,
        errored_entries: 0,
        unclassified_entries: 0,
    };
    let value = serde_json::to_value(&totals).expect("totals serialize");

    for field in [
        "bytes",
        "scannedEntries",
        "entriesWithoutSize",
        "erroredEntries",
        "unclassifiedEntries",
    ] {
        assert!(
            value[field].is_number(),
            "{field} must be a number, got {}",
            value[field]
        );
    }
    assert_eq!(value["bytes"], 0);
    assert_eq!(value["entriesWithoutSize"], 7);
    assert_eq!(
        value["complete"], false,
        "seven unmeasured entries means the byte total is not complete"
    );
}

// If this regresses, a share of an unknown parent is rendered as 0.0 and an
// unavailable summary is rendered as a zeroed object.
#[test]
fn an_unknown_share_and_an_unavailable_summary_serialize_as_null() {
    let mut session = session_with_scan(vec![
        entry("/cache/a", Some(10), Some(Category::Cache)),
        entry("/cache/b", None, Some(Category::Cache)),
    ]);
    let envelope = dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.getCategories\",\"payload\":{\"scanId\":\"scan-1\"}}",
    );
    let value = envelope_json(&envelope);
    let rows = value["ok"]["payload"]
        .as_array()
        .expect("getCategories returns an array");
    let cache = rows
        .iter()
        .find(|row| row["id"] == "CACHE")
        .expect("the fixture has a cache row");

    assert_eq!(cache["bytes"], 10);
    assert!(
        cache["shareOfParent"].is_null(),
        "an incomplete parent total makes the share unknown, never 0.0: {cache}"
    );
    assert_eq!(cache["complete"], false);
    assert_eq!(cache["entriesWithoutSize"], 1);

    let summary = dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.getDriveSummary\",\"payload\":{\"driveId\":\"drive-c\"}}",
    );
    let summary = envelope_json(&summary);
    assert_eq!(summary["ok"]["payload"]["totals"]["complete"], false);
    assert_eq!(summary["ok"]["payload"]["totals"]["entriesWithoutSize"], 1);
    assert!(
        summary["ok"]["payload"]["exclusionsNote"].is_null(),
        "no exclusion report must be null, not an empty claim"
    );
}

// If this regresses, an incomplete scan publishes a storage total that sums
// only the entries it managed to measure.
#[test]
fn an_incomplete_scan_publishes_no_storage_total() {
    let mut session = session_with_scan(vec![
        entry("/cache/a", Some(10), Some(Category::Cache)),
        entry("/cache/b", None, Some(Category::Cache)),
    ]);
    let envelope = dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.getStorage\",\"payload\":{}}",
    );
    let value = envelope_json(&envelope);

    assert!(
        value["ok"]["payload"]["totalObservedBytes"].is_null(),
        "a partial byte total must be unknown, got {}",
        value["ok"]["payload"]["totalObservedBytes"]
    );
    assert_eq!(value["ok"]["payload"]["totalObservedItems"], 2);
}

// If this regresses, a plan preview claims completeness while entries were
// skipped, and the user confirms a list that is not the whole plan.
#[test]
fn a_plan_preview_is_incomplete_whenever_anything_was_skipped() {
    let mut session = session_with_scan(vec![
        entry("/cache/a", Some(10), Some(Category::Cache)),
        entry("/cache/b", None, Some(Category::Cache)),
    ]);
    let envelope = dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.previewPlan\",\"payload\":{\"opportunityIds\":[\"scan-1:CACHE:10\"]}}",
    );
    if envelope.is_err() {
        // Resolution may refuse an item it cannot pin; that refusal is equally
        // honest and must be typed.
        assert_eq!(error_code(&envelope), "PLAN_UNRESOLVABLE");
        return;
    }

    let value = envelope_json(&envelope);
    let preview = &value["ok"]["payload"];
    assert_eq!(preview["complete"], false);
    assert_eq!(preview["bytes"], 10);
    assert_eq!(preview["claimedBytes"], 10);
    let warnings = preview["warnings"]
        .as_array()
        .expect("warnings are a typed list");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("could not be included"))),
        "the skipped entry must be named in a warning: {preview}"
    );
    for item in preview["items"].as_array().expect("items are a list") {
        let path = item["path"].as_str().expect("the path is a string");
        assert!(
            path.starts_with("u:") || path.starts_with("e:"),
            "paths must use the workspace's lossless encoding, got {path:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Unavailable capabilities, hostile identifiers
// ---------------------------------------------------------------------------

// If this regresses, an unmeasurable latency is published as a `0.0 ms`
// measurement, which reads as the fastest possible resolver.
#[test]
fn a_nan_measurement_is_never_rendered_as_a_zero_latency() {
    use coresight_ipc::Session as IpcSession;
    use coresight_webtools::{
        rank_dns, DnsCandidate, DnsMeasurement, DnsPrivacyPolicy, DnsRankOptions,
    };

    let candidate = DnsCandidate {
        name: "nan-latency".to_string(),
        ipv4: vec!["192.0.2.1".to_string()],
        ipv6: Vec::new(),
        operator: None,
        privacy_policy: DnsPrivacyPolicy::DocumentedNoLogging,
        supports_dnssec: false,
        supports_filtering: false,
    };
    let measurement = DnsMeasurement {
        candidate_name: "nan-latency".to_string(),
        median_latency_ms: f64::NAN,
        p95_latency_ms: f64::NAN,
        failure_rate: 0.0,
        samples: 10,
    };
    let ranking = rank_dns(
        &[candidate],
        &[measurement],
        &DnsRankOptions {
            min_samples: 1,
            max_failure_rate: 1.0,
            require_documented_privacy: false,
        },
    );
    assert_eq!(ranking.ranked.len(), 1, "latency is scored, not screened");

    let mut session = IpcSession::new();
    session.register_dns_ranking(ranking);
    let value = envelope_json(&dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.getDnsRanking\",\"payload\":{}}",
    ));
    let row = &value["ok"]["payload"]["ranked"][0];

    assert!(
        row["medianLatencyMs"].is_null(),
        "an unrepresentable measurement must be null, never 0.0: {row}"
    );
    assert!(
        row["medianLatencyMs"] != 0.0,
        "null must not be coerced into a zero measurement"
    );
    assert_eq!(
        row["latencyScore"], 0,
        "the component score is 'worst', not unknown"
    );
    assert_eq!(
        row["samples"], 10,
        "the sample count must still show a measurement existed"
    );
}

// If this regresses, a capability this build does not have answers with
// placeholder data instead of a typed refusal.
#[test]
fn unavailable_capabilities_are_typed_refusals_not_placeholder_data() {
    let mut session = Session::new();
    let cases = [
        (
            "{\"command\":\"coresight.v1.startScan\",\"payload\":{\"driveId\":\"d\"}}",
            "SCAN_UNAVAILABLE",
        ),
        (
            "{\"command\":\"coresight.v1.cancelScan\",\"payload\":{\"scanId\":\"s\"}}",
            "SCAN_UNAVAILABLE",
        ),
        (
            "{\"command\":\"coresight.v1.getHistory\",\"payload\":{\"driveId\":\"d\"}}",
            "HISTORY_UNAVAILABLE",
        ),
        (
            "{\"command\":\"coresight.v1.getGlobalStatus\",\"payload\":{}}",
            "SESSION_UNAVAILABLE",
        ),
        (
            "{\"command\":\"coresight.v1.getCategories\",\"payload\":{\"scanId\":\"missing\"}}",
            "SCAN_NOT_FOUND",
        ),
        (
            "{\"command\":\"coresight.v1.getDriveSummary\",\"payload\":{\"driveId\":\"missing\"}}",
            "DRIVE_NOT_FOUND",
        ),
        (
            "{\"command\":\"coresight.v1.previewPlan\",\"payload\":{\"opportunityIds\":[]}}",
            "NOTHING_TO_DO",
        ),
    ];

    for (frame, expected) in cases {
        let envelope = dispatch_json(&mut session, frame);
        assert!(!envelope.is_ok(), "{frame} must be refused");
        assert_eq!(error_code(&envelope), expected, "frame {frame}");
        assert!(envelope.ok_value().is_none());
    }
}

// If this regresses, a scan that is still running (or failed) is served as if
// its facts were complete.
#[test]
fn a_running_or_failed_scan_is_never_served() {
    let mut session = Session::new();
    session.register_drive(drive("drive-c"));
    for (id, status, expected) in [
        ("running", ScanStatus::Running, "SCAN_IN_PROGRESS"),
        ("failed", ScanStatus::Failed, "SCAN_UNAVAILABLE"),
        ("cancelled", ScanStatus::Cancelled, "SCAN_UNAVAILABLE"),
    ] {
        session
            .register_scan(scan(
                id,
                "drive-c",
                status,
                vec![entry("/cache/a", Some(1), Some(Category::Cache))],
            ))
            .expect("the fixture scan registers");
        let frame = serde_json::json!({
            "command": "coresight.v1.getCategories",
            "payload": { "scanId": id }
        });
        let envelope = dispatch_json(&mut session, &frame.to_string());
        assert_eq!(error_code(&envelope), expected, "scan {id}");
    }
}

// If this regresses, a hostile identifier is echoed whole into the error
// detail, forging log lines or bloating the response.
#[test]
fn a_hostile_identifier_is_sanitized_and_bounded_in_the_error_detail() {
    let mut session = Session::new();
    let hostile = format!("\nsecond line\0{}", "A".repeat(200_000));
    let frame = serde_json::json!({
        "command": "coresight.v1.getDriveSummary",
        "payload": { "driveId": hostile }
    });
    let envelope = dispatch_json(&mut session, &frame.to_string());
    let detail = error_of(&envelope)
        .detail
        .as_deref()
        .expect("the refusal carries a detail");

    assert!(
        detail.len() <= 100,
        "the echoed id must be capped, got {} bytes",
        detail.len()
    );
    assert!(
        !detail.contains('\n'),
        "newlines must be neutralized: {detail:?}"
    );
    assert!(
        !detail.contains('\0'),
        "NUL must be neutralized: {detail:?}"
    );
    assert!(
        detail.contains('?'),
        "control characters become '?': {detail:?}"
    );

    // A 4096-character id is likewise bounded.
    let frame = serde_json::json!({
        "command": "coresight.v1.getDriveSummary",
        "payload": { "driveId": "C".repeat(4096) }
    });
    let envelope = dispatch_json(&mut session, &frame.to_string());
    assert!(
        error_of(&envelope)
            .detail
            .as_deref()
            .unwrap_or_default()
            .len()
            <= 100
    );
}

// If this regresses, dispatch depends on hidden state and the same session plus
// the same command serializes differently.
#[test]
fn dispatch_is_byte_identical_for_a_repeated_command() {
    let mut session = session_with_scan(vec![
        entry("/cache/a", Some(3), Some(Category::Cache)),
        entry("/logs/b", Some(2), Some(Category::Logs)),
    ]);
    let frame = "{\"command\":\"coresight.v1.getCategories\",\"payload\":{\"scanId\":\"scan-1\"}}";

    let first = envelope_json(&dispatch_json(&mut session, frame));
    let second = envelope_json(&dispatch_json(&mut session, frame));
    assert_eq!(
        serde_json::to_string(&first).expect("serializes"),
        serde_json::to_string(&second).expect("serializes")
    );
}

// If this regresses, the session silently replaces facts the UI may already be
// showing, or accepts a scan for a drive it never knew.
#[test]
fn session_registration_refuses_blank_and_duplicate_identities() {
    let mut session = Session::new();
    assert_eq!(
        session.register_scan(scan("  ", "drive-c", ScanStatus::Completed, Vec::new())),
        Err(SessionError::EmptyScanId)
    );
    assert_eq!(
        session.register_scan(scan("scan-1", "", ScanStatus::Completed, Vec::new())),
        Err(SessionError::EmptyDriveId)
    );
    assert_eq!(
        session.register_scan(scan("scan-1", "drive-c", ScanStatus::Completed, Vec::new())),
        Err(SessionError::UnknownDrive("drive-c".to_string()))
    );

    session.register_drive(drive("drive-c"));
    session
        .register_scan(scan("scan-1", "drive-c", ScanStatus::Completed, Vec::new()))
        .expect("the first registration succeeds");
    assert_eq!(
        session.register_scan(scan("scan-1", "drive-c", ScanStatus::Completed, Vec::new())),
        Err(SessionError::DuplicateScan("scan-1".to_string()))
    );
}

// If this regresses, an empty opportunity list or an unknown one is previewed
// as an empty plan the user could confirm.
#[test]
fn an_unknown_opportunity_is_named_and_refused() {
    let mut session = session_with_scan(vec![entry("/cache/a", Some(10), Some(Category::Cache))]);
    let frame = serde_json::json!({
        "command": "coresight.v1.previewPlan",
        "payload": { "opportunityIds": ["scan-1:CACHE:999999"] }
    });
    let envelope = dispatch_json(&mut session, &frame.to_string());
    assert_eq!(error_code(&envelope), "UNKNOWN_OPPORTUNITY");
    assert!(error_of(&envelope)
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("CACHE:999999"));
}

// If this regresses, a duplicated selection is previewed twice and the plan's
// byte accounting doubles.
#[test]
fn a_duplicated_opportunity_selection_is_refused() {
    let mut session = session_with_scan(vec![entry("/cache/a", Some(10), Some(Category::Cache))]);
    let frame = serde_json::json!({
        "command": "coresight.v1.previewPlan",
        "payload": { "opportunityIds": ["scan-1:CACHE:10", "scan-1:CACHE:10"] }
    });
    let envelope = dispatch_json(&mut session, &frame.to_string());
    assert_eq!(error_code(&envelope), "INVALID_REQUEST");
}

// If this regresses, an incomplete preview can be confirmed anyway by
// acknowledging its warnings, and the "no confirm anyway" path disappears.
#[test]
fn an_incomplete_preview_cannot_be_confirmed_even_with_every_warning_acked() {
    let mut session = session_with_scan(vec![
        entry("/cache/a", Some(10), Some(Category::Cache)),
        entry("/cache/b", None, Some(Category::Cache)),
    ]);
    let preview = envelope_json(&dispatch_json(
        &mut session,
        "{\"command\":\"coresight.v1.previewPlan\",\"payload\":{\"opportunityIds\":[\"scan-1:CACHE:10\"]}}",
    ));
    let payload = &preview["ok"]["payload"];
    let plan_id = payload["planId"]
        .as_str()
        .expect("the preview names its plan")
        .to_string();
    let warnings: Vec<String> = payload["warnings"]
        .as_array()
        .expect("warnings are a list")
        .iter()
        .map(|warning| warning.as_str().expect("a warning string").to_string())
        .collect();
    assert!(!warnings.is_empty(), "the fixture must produce a warning");

    // Nothing acknowledged: the missing acknowledgements are named.
    let unacked = dispatch_json(
        &mut session,
        &serde_json::json!({
            "command": "coresight.v1.confirmPlan",
            "payload": { "planId": plan_id, "ackedWarnings": [] }
        })
        .to_string(),
    );
    assert_eq!(error_code(&unacked), "PLAN_REJECTED_UNSAFE");
    let reasons = error_of(&unacked).rejection_reasons();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("WARNINGS_NOT_ACKED")),
        "got {reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("PREVIEW_INCOMPLETE")),
        "got {reasons:?}"
    );

    // Every warning acknowledged: incompleteness still blocks it.
    let acked = dispatch_json(
        &mut session,
        &serde_json::json!({
            "command": "coresight.v1.confirmPlan",
            "payload": { "planId": plan_id, "ackedWarnings": warnings }
        })
        .to_string(),
    );
    assert_eq!(error_code(&acked), "PLAN_REJECTED_UNSAFE");
    let reasons = error_of(&acked).rejection_reasons();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("PREVIEW_INCOMPLETE")),
        "acknowledging warnings must not make an incomplete preview complete: {reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|reason| reason.starts_with("WARNINGS_NOT_ACKED")),
        "all warnings were acknowledged: {reasons:?}"
    );
    assert!(acked.ok_value().is_none());
}

// If this regresses, a drive report invents a snapshot count or a last scan
// that never completed.
#[test]
fn a_drive_summary_never_invents_a_completed_scan() {
    let mut session = Session::new();
    session.register_drive(Drive {
        connected: false,
        ..drive("drive-z")
    });
    let frame =
        "{\"command\":\"coresight.v1.getDriveSummary\",\"payload\":{\"driveId\":\"drive-z\"}}";
    let envelope = dispatch_json(&mut session, frame);

    assert_eq!(
        error_code(&envelope),
        "DRIVE_OFFLINE",
        "an offline drive with no retained snapshot has nothing to answer from"
    );
}
