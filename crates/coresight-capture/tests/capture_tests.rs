//! Integration tests for `coresight-capture`.
//!
//! All fixtures are synthetic: no filesystem is touched, no screen is
//! captured, nothing is uploaded, and nothing is deleted. The tests pin the
//! contracts the desktop shell relies on — a local-only policy, destination
//! containment with honest notes, deterministic collision-free naming, and
//! retention that never deletes and never loses a capture it promised to keep.

use coresight_capture::{
    default_policy, plan_destination, plan_name, plan_retention, policy_violations,
    upload_supported, CaptureDestination, CaptureEntry, CaptureNaming, CapturePolicy, NameError,
    RetentionPolicy, SensitiveRegion, MAX_COLLISION_INDEX,
};
use std::path::PathBuf;

const STAMP: &str = "2026-11-13T12:19:21+05:30";
const APP: &str = "Adobe Photoshop";
const BASE_NAME: &str = "CoreSight 2026-11-13T12-19-21-05-30 Adobe Photoshop.png";

fn test_path(relative: &str) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    for part in relative.split('/') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    path
}

fn naming() -> CaptureNaming {
    CaptureNaming {
        prefix: "CoreSight".to_string(),
        include_timestamp: true,
        include_app: true,
    }
}

fn capture(path: &str, bytes: Option<u64>, created_order: u64) -> CaptureEntry {
    CaptureEntry {
        path: test_path(path),
        bytes,
        created_order,
    }
}

// --- privacy ---------------------------------------------------------------

#[test]
fn upload_is_impossible_in_this_build() {
    assert!(
        !upload_supported(),
        "the build must report no upload support"
    );

    let policy = default_policy();
    assert!(!policy.upload_enabled, "captures stay on the machine");
    assert!(policy.require_destination_confirmation);
    assert!(policy.is_local_only());
    assert!(
        policy_violations(&policy).is_empty(),
        "the default policy must satisfy every guarantee: {:?}",
        policy_violations(&policy)
    );
    assert!(
        !policy.sensitive_regions.is_empty(),
        "default policy must carry redaction obligations"
    );
    assert!(policy
        .sensitive_regions
        .iter()
        .any(SensitiveRegion::must_omit));
    assert!(policy
        .sensitive_regions
        .iter()
        .all(|region| !region.reason().is_empty()));
}

#[test]
fn a_hand_built_policy_that_claims_upload_is_reported_not_obeyed() {
    // `upload_enabled` is a report, not a switch: this build still has no
    // upload path. The audit is what lets the shell refuse such a policy.
    let unsafe_policy = CapturePolicy {
        upload_enabled: true,
        ..default_policy()
    };

    assert!(!unsafe_policy.is_local_only());
    let violations = policy_violations(&unsafe_policy);
    assert!(
        violations
            .iter()
            .any(|message| message.contains("uploadEnabled")),
        "violations were: {violations:?}"
    );

    let no_confirmation = CapturePolicy {
        require_destination_confirmation: false,
        ..default_policy()
    };
    assert!(policy_violations(&no_confirmation)
        .iter()
        .any(|message| message.contains("requireDestinationConfirmation")));

    let no_regions = CapturePolicy {
        sensitive_regions: Vec::new(),
        ..default_policy()
    };
    assert!(policy_violations(&no_regions)
        .iter()
        .any(|message| message.contains("sensitiveRegions")));
}

// --- destination -----------------------------------------------------------

#[test]
fn a_requested_destination_outside_allowed_roots_is_refused_and_explained() {
    let plan = plan_destination(
        Some(&test_path("outside/shots")),
        &test_path("system/Screenshots"),
        &[test_path("pictures"), test_path("pictures/coresight")],
    );

    assert!(!plan.writable, "outside every root means not writable");
    assert!(plan.is_user_chosen(), "the user's choice is not replaced");
    assert_eq!(plan.path(), test_path("outside/shots").as_path());
    assert!(
        plan.note.contains("Writable: no"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note.contains("outside every allowed root"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note.contains("does not silently fall back"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note.contains("pictures"),
        "the note names the roots it checked: {}",
        plan.note
    );
}

#[test]
fn a_requested_destination_inside_an_allowed_root_is_writable() {
    let roots = vec![test_path("pictures"), test_path("pictures/coresight")];
    let plan = plan_destination(
        Some(&test_path("pictures/coresight/shots")),
        &test_path("system/Screenshots"),
        &roots,
    );

    assert!(plan.writable);
    assert!(matches!(
        &plan.destination,
        CaptureDestination::UserChosenDirectory { .. }
    ));
    assert!(
        plan.note.contains("Writable: yes"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note.contains("the allowed root"),
        "the note names the root: {}",
        plan.note
    );
    // The most specific matching root is the one reported.
    assert!(
        plan.note
            .contains(&test_path("pictures/coresight").display().to_string()),
        "note was: {}",
        plan.note
    );
}

#[test]
fn destination_containment_is_component_wise_and_resolves_dot_dot() {
    let roots = vec![test_path("pictures")];

    let exact = plan_destination(
        Some(&test_path("pictures")),
        &test_path("system/Screenshots"),
        &roots,
    );
    assert!(exact.writable, "the root itself is a legal destination");

    let sibling = plan_destination(
        Some(&test_path("pictures2/shots")),
        &test_path("system/Screenshots"),
        &roots,
    );
    assert!(
        !sibling.writable,
        "a sibling that merely shares a string prefix is not inside"
    );

    let escape = plan_destination(
        Some(&test_path("pictures/../outside")),
        &test_path("system/Screenshots"),
        &roots,
    );
    assert!(
        !escape.writable,
        "`..` must be resolved before containment is decided"
    );

    let above_root = plan_destination(
        Some(&test_path("../anywhere")),
        &test_path("system/Screenshots"),
        &roots,
    );
    assert!(
        !above_root.writable,
        "a path that climbs above its own root is refused, not guessed at"
    );
}

#[test]
fn no_allowed_roots_means_no_writable_user_destination() {
    let plan = plan_destination(
        Some(&test_path("pictures/shots")),
        &test_path("system/Screenshots"),
        &[],
    );
    assert!(!plan.writable);
    assert!(
        plan.note.contains("no allowed roots are configured"),
        "note was: {}",
        plan.note
    );
}

#[test]
fn an_empty_requested_path_is_refused_rather_than_replaced() {
    let plan = plan_destination(
        Some(&PathBuf::new()),
        &test_path("system/Screenshots"),
        &[test_path("pictures")],
    );
    assert!(!plan.writable);
    assert!(plan.is_user_chosen(), "no silent substitution");
    assert!(plan.note.contains("empty path"), "note was: {}", plan.note);
}

#[test]
fn a_destination_fallback_is_always_explained() {
    let plan = plan_destination(
        None,
        &test_path("system/Screenshots"),
        &[test_path("pictures")],
    );

    assert!(plan.writable);
    assert!(matches!(
        &plan.destination,
        CaptureDestination::PlatformDefault { .. }
    ));
    assert_eq!(plan.path(), test_path("system/Screenshots").as_path());
    assert!(
        plan.note
            .contains("Destination: the platform default folder"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note.contains("because no directory was requested"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note
            .contains(&test_path("system/Screenshots").display().to_string()),
        "the note names the fallback folder: {}",
        plan.note
    );
    assert!(
        plan.note.contains("confirm"),
        "the fallback still requires confirmation: {}",
        plan.note
    );
}

#[test]
fn every_destination_note_names_the_choice_and_the_reason() {
    let roots = vec![test_path("pictures")];
    let cases = [
        plan_destination(
            Some(&test_path("pictures/shots")),
            &test_path("system/Screenshots"),
            &roots,
        ),
        plan_destination(
            Some(&test_path("elsewhere")),
            &test_path("system/Screenshots"),
            &roots,
        ),
        plan_destination(None, &test_path("system/Screenshots"), &roots),
    ];

    for plan in cases {
        assert!(
            plan.note.starts_with("Destination: "),
            "note was: {}",
            plan.note
        );
        assert!(plan.note.contains("Writable: "), "note was: {}", plan.note);
    }
}

// --- naming ----------------------------------------------------------------

#[test]
fn names_are_deterministic_and_human_readable() {
    let first = plan_name(&naming(), STAMP, Some(APP), &[]).expect("a fresh name");
    assert_eq!(first.file_name, BASE_NAME);
    assert_eq!(first.collision_index, 0);

    let second = plan_name(&naming(), STAMP, Some(APP), &[]).expect("the same name");
    assert_eq!(
        first, second,
        "identical inputs must produce identical names"
    );

    let other_stamp =
        plan_name(&naming(), "2026-01-01T00:00:00Z", Some(APP), &[]).expect("a different stamp");
    assert_ne!(
        first.file_name, other_stamp.file_name,
        "the timestamp is part of the name"
    );
    assert!(first.file_name.ends_with(".png"));
}

#[test]
fn collisions_increment_and_never_overwrite() {
    let second = plan_name(&naming(), STAMP, Some(APP), &[BASE_NAME.to_string()])
        .expect("one collision still has a free name");
    assert_eq!(
        second.file_name,
        "CoreSight 2026-11-13T12-19-21-05-30 Adobe Photoshop (1).png"
    );
    assert_eq!(second.collision_index, 1);

    let third = plan_name(
        &naming(),
        STAMP,
        Some(APP),
        &[BASE_NAME.to_string(), second.file_name.clone()],
    )
    .expect("two collisions still have a free name");
    assert_eq!(
        third.file_name,
        "CoreSight 2026-11-13T12-19-21-05-30 Adobe Photoshop (2).png"
    );
    assert_eq!(third.collision_index, 2);
}

#[test]
fn successive_plans_never_repeat_a_name() {
    let mut existing: Vec<String> = Vec::new();
    for expected_index in 0..5 {
        let plan = plan_name(&naming(), STAMP, Some(APP), &existing).expect("a free name");
        assert_eq!(plan.collision_index, expected_index);
        assert!(
            !existing.contains(&plan.file_name),
            "{} was already used",
            plan.file_name
        );
        existing.push(plan.file_name);
    }
    assert_eq!(existing.len(), 5, "five captures, five distinct names");
}

#[test]
fn collision_comparison_is_case_insensitive() {
    // Windows and macOS treat these as the same file; accepting the
    // differently-cased name would overwrite the existing capture.
    let plan = plan_name(&naming(), STAMP, Some(APP), &[BASE_NAME.to_uppercase()])
        .expect("a case variant is still a collision");
    assert_eq!(plan.collision_index, 1);
}

#[test]
fn collision_exhaustion_returns_no_free_name() {
    let stem = BASE_NAME
        .strip_suffix(".png")
        .expect("the base name ends in .png");
    let mut existing: Vec<String> = Vec::new();
    for index in 0..=MAX_COLLISION_INDEX {
        if index == 0 {
            existing.push(format!("{stem}.png"));
        } else {
            existing.push(format!("{stem} ({index}).png"));
        }
    }

    let error = plan_name(&naming(), STAMP, Some(APP), &existing)
        .expect_err("every index up to the bound is taken");
    assert_eq!(error, NameError::NoFreeName);
}

#[test]
fn invalid_prefixes_are_rejected_with_a_reason() {
    let cases = [
        ("", "empty"),
        ("   ", "whitespace"),
        ("shots/2026", "separator"),
        ("shots\\2026", "separator"),
        ("shot\0name", "NUL"),
        ("CON", "reserved"),
        ("con", "reserved"),
        ("NUL", "reserved"),
        ("COM1", "reserved"),
        ("lpt9", "reserved"),
        ("CON.png", "reserved"),
        ("trailing ", "space or a dot"),
        ("trailing.", "space or a dot"),
        ("bad:name", "drive"),
        ("bad*name", "Windows"),
        (".", "space or a dot"),
        ("..", "space or a dot"),
    ];

    for (prefix, expected) in cases {
        let mut settings = naming();
        settings.prefix = prefix.to_string();
        match plan_name(&settings, STAMP, None, &[]) {
            Err(NameError::InvalidPrefix { reason }) => {
                assert!(!reason.is_empty(), "prefix {prefix:?} needs a reason");
                assert!(
                    reason.contains(expected),
                    "prefix {prefix:?} reason was: {reason}"
                );
            }
            other => panic!("prefix {prefix:?} must be rejected, got {other:?}"),
        }
    }
}

#[test]
fn lookalike_prefixes_that_are_legal_are_accepted() {
    for prefix in ["Console", "COM10", "LPT0", "My.Capture", "Screenshot"] {
        let mut settings = naming();
        settings.prefix = prefix.to_string();
        let name = plan_name(&settings, STAMP, None, &[]).expect("a legal prefix");
        assert!(
            name.file_name.starts_with(prefix),
            "prefix {prefix:?} produced {}",
            name.file_name
        );
        assert!(name.file_name.ends_with(".png"));
    }
}

#[test]
fn timestamp_and_app_tokens_are_optional() {
    let mut no_timestamp = naming();
    no_timestamp.include_timestamp = false;
    let name = plan_name(&no_timestamp, "not a timestamp", Some("Editor"), &[])
        .expect("a disabled timestamp is never read");
    assert_eq!(
        name.file_name, "CoreSight Editor.png",
        "the timestamp is ignored when disabled"
    );

    let mut no_app = naming();
    no_app.include_app = false;
    let name = plan_name(&no_app, STAMP, Some("Editor"), &[]).expect("a disabled app is ignored");
    assert_eq!(name.file_name, "CoreSight 2026-11-13T12-19-21-05-30.png");

    let unknown_app =
        plan_name(&naming(), STAMP, None, &[]).expect("an unknown app is not an error");
    assert_eq!(
        unknown_app.file_name,
        "CoreSight 2026-11-13T12-19-21-05-30.png"
    );
}

#[test]
fn a_requested_timestamp_must_contribute_characters() {
    let error = plan_name(&naming(), "   ", None, &[]).expect_err("a blank timestamp is unusable");
    match error {
        NameError::InvalidTimestamp { reason } => {
            assert!(reason.contains("RFC 3339"), "reason was: {reason}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn a_planned_name_joins_the_destination_directory() {
    let name = plan_name(&naming(), STAMP, Some(APP), &[]).expect("a fresh name");
    let path = name.path_in(&test_path("pictures/shots"));
    assert_eq!(path, test_path("pictures/shots").join(BASE_NAME));
}

// --- retention -------------------------------------------------------------

#[test]
fn keep_last_zero_keeps_everything() {
    let captures = vec![
        capture("shots/a.png", Some(100), 1),
        capture("shots/b.png", Some(200), 2),
        capture("shots/c.png", Some(300), 3),
    ];

    let plan = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 0,
            max_bytes: None,
        },
    );
    assert_eq!(plan.keep.len(), 3);
    assert!(
        plan.delete_candidates.is_empty(),
        "keep 0 must never mean delete all"
    );
    assert_eq!(plan.total_bytes, Some(600));
    assert!(
        plan.note.contains("keep everything"),
        "note was: {}",
        plan.note
    );

    // A byte budget must not turn "keep everything" into deletion either.
    let capped = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 0,
            max_bytes: Some(1),
        },
    );
    assert_eq!(capped.keep.len(), 3);
    assert!(capped.delete_candidates.is_empty());
    assert!(capped.note.contains("deliberately not applied"));
}

#[test]
fn retention_keeps_the_newest_and_proposes_the_rest() {
    let captures = vec![
        capture("shots/old.png", Some(1_000), 1),
        capture("shots/new.png", Some(2_000), 3),
        capture("shots/mid.png", Some(3_000), 2),
    ];
    let plan = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 2,
            max_bytes: None,
        },
    );

    assert_eq!(
        plan.keep,
        vec![test_path("shots/new.png"), test_path("shots/mid.png")]
    );
    assert_eq!(plan.delete_candidates, vec![test_path("shots/old.png")]);
    assert_eq!(plan.total_bytes, Some(5_000));
    assert!(
        plan.note.contains("Keep-last is 2"),
        "note was: {}",
        plan.note
    );
    assert!(
        plan.note.contains("proposal only"),
        "note was: {}",
        plan.note
    );
}

#[test]
fn delete_candidates_never_include_a_kept_capture() {
    let captures = vec![
        capture("shots/a.png", Some(400), 1),
        capture("shots/b.png", Some(300), 2),
        capture("shots/c.png", Some(200), 3),
        capture("shots/d.png", Some(100), 4),
        capture("shots/e.png", Some(900), 5),
    ];
    let plan = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 3,
            max_bytes: Some(2_500),
        },
    );

    assert_eq!(
        plan.keep.len() + plan.delete_candidates.len(),
        captures.len(),
        "every capture is accounted for exactly once"
    );
    for kept in &plan.keep {
        assert!(
            !plan.delete_candidates.contains(kept),
            "{} is both kept and proposed for deletion",
            kept.display()
        );
    }
    // Nothing was deleted: the planner receives a shared slice and has no
    // filesystem access, so the input is still complete.
    assert_eq!(captures.len(), 5);
}

#[test]
fn total_bytes_is_unknown_when_a_retained_size_is_unknown() {
    let captures = vec![
        capture("shots/new.png", None, 2),
        capture("shots/old.png", Some(10), 1),
    ];
    let plan = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 1,
            max_bytes: None,
        },
    );
    assert_eq!(plan.keep, vec![test_path("shots/new.png")]);
    assert_eq!(plan.total_bytes, None);
    assert!(plan.note.contains("unknown"), "note was: {}", plan.note);

    // An unknown size on a capture that is being deleted does not taint the
    // total of the captures that are kept.
    let deleted_unknown = vec![
        capture("shots/new.png", Some(10), 2),
        capture("shots/old.png", None, 1),
    ];
    let plan = plan_retention(
        &deleted_unknown,
        &RetentionPolicy {
            keep_last: 1,
            max_bytes: None,
        },
    );
    assert_eq!(plan.total_bytes, Some(10));
}

#[test]
fn the_byte_budget_only_trims_older_captures() {
    let captures = vec![
        capture("shots/a.png", Some(100), 1),
        capture("shots/b.png", Some(100), 2),
        capture("shots/c.png", Some(100), 3),
    ];

    // The keep window is a promise: even a budget smaller than one capture
    // never proposes the newest capture for deletion.
    let tight = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 1,
            max_bytes: Some(50),
        },
    );
    assert_eq!(tight.keep, vec![test_path("shots/c.png")]);
    assert_eq!(tight.delete_candidates.len(), 2);

    // Within the budget, older captures are retained while they fit.
    let roomy = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 1,
            max_bytes: Some(250),
        },
    );
    assert_eq!(
        roomy.keep,
        vec![test_path("shots/c.png"), test_path("shots/b.png")]
    );
    assert_eq!(roomy.delete_candidates, vec![test_path("shots/a.png")]);
    assert_eq!(roomy.total_bytes, Some(200));
}

#[test]
fn the_byte_budget_is_not_applied_when_a_size_is_unknown() {
    let captures = vec![
        capture("shots/new.png", None, 3),
        capture("shots/mid.png", Some(10), 2),
        capture("shots/old.png", Some(10), 1),
    ];
    let plan = plan_retention(
        &captures,
        &RetentionPolicy {
            keep_last: 1,
            max_bytes: Some(1_000),
        },
    );

    assert_eq!(
        plan.keep.len(),
        1,
        "a size the planner cannot see must not license a decision"
    );
    assert_eq!(plan.total_bytes, None);
    assert!(
        plan.note.contains("no recorded size"),
        "note was: {}",
        plan.note
    );
}

#[test]
fn an_empty_capture_list_plans_nothing() {
    let plan = plan_retention(
        &[],
        &RetentionPolicy {
            keep_last: 5,
            max_bytes: Some(1),
        },
    );
    assert!(plan.keep.is_empty());
    assert!(plan.delete_candidates.is_empty());
    assert_eq!(plan.total_bytes, Some(0));
    assert!(!plan.note.is_empty());
}

#[test]
fn equal_creation_order_is_broken_by_path() {
    let captures = vec![
        capture("shots/b.png", Some(1), 7),
        capture("shots/a.png", Some(1), 7),
    ];
    let policy = RetentionPolicy {
        keep_last: 1,
        max_bytes: None,
    };
    let plan = plan_retention(&captures, &policy);
    assert_eq!(plan.keep, vec![test_path("shots/a.png")]);

    let mut reversed = captures.clone();
    reversed.reverse();
    assert_eq!(plan_retention(&reversed, &policy), plan);
}

#[test]
fn plans_are_deterministic_under_input_permutations() {
    let forward = vec![
        capture("shots/a.png", Some(10), 1),
        capture("shots/b.png", Some(20), 2),
        capture("shots/c.png", Some(30), 3),
    ];
    let mut backward = forward.clone();
    backward.reverse();
    let policy = RetentionPolicy {
        keep_last: 2,
        max_bytes: Some(40),
    };
    assert_eq!(
        plan_retention(&forward, &policy),
        plan_retention(&backward, &policy)
    );

    let existing = vec![BASE_NAME.to_string(), format!("{} (1)", BASE_NAME)];
    let mut reversed_existing = existing.clone();
    reversed_existing.reverse();
    assert_eq!(
        plan_name(&naming(), STAMP, Some(APP), &existing),
        plan_name(&naming(), STAMP, Some(APP), &reversed_existing)
    );

    let roots = vec![test_path("pictures"), test_path("pictures/coresight")];
    let mut reversed_roots = roots.clone();
    reversed_roots.reverse();
    assert_eq!(
        plan_destination(
            Some(&test_path("pictures/coresight/shots")),
            &test_path("system/Screenshots"),
            &roots
        ),
        plan_destination(
            Some(&test_path("pictures/coresight/shots")),
            &test_path("system/Screenshots"),
            &reversed_roots
        )
    );
}

// --- serialized contract ---------------------------------------------------

#[test]
fn public_plans_serialize_with_camel_case_fields() {
    let json = serde_json::to_value(naming()).expect("serialize naming");
    assert_eq!(json["prefix"], "CoreSight");
    assert_eq!(json["includeTimestamp"], true);
    assert_eq!(json["includeApp"], true);
    assert!(json.get("include_timestamp").is_none());

    let destination = plan_destination(None, &test_path("system/Screenshots"), &[]);
    let json = serde_json::to_value(&destination).expect("serialize destination plan");
    assert_eq!(json["destination"]["kind"], "PLATFORM_DEFAULT");
    assert_eq!(json["writable"], true);
    assert!(json["note"].is_string());

    let json = serde_json::to_value(default_policy()).expect("serialize policy");
    assert_eq!(json["uploadEnabled"], false);
    assert_eq!(json["requireDestinationConfirmation"], true);
    assert_eq!(json["sensitiveRegions"][0]["kind"], "OMIT");
    assert!(json["sensitiveRegions"][1]["reason"].is_string());

    let json =
        serde_json::to_value(RetentionPolicy::default()).expect("serialize retention policy");
    assert_eq!(json["keepLast"], 100);
    assert!(json["maxBytes"].is_null());
    assert!(json.get("keep_last").is_none());
}
