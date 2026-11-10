//! Authorization and confirmation: the two gates in front of execution.
//!
//! Contract 4 (a destructive plan can never be authorized under a policy that
//! forbids destruction) and contract 5 (execution needs a confirmation that
//! matches the plan's id and byte total) are proved here.

mod common;

use std::fs;

use common::{assert_inside, cleanup_item, cleanup_plan, destructive_policy, reversible_policy};
use coresight_actions::{
    authorize, execute, AuthorizeError, ConfirmedPreview, DirectoryTrash, ExecuteError,
};
use coresight_cleanup::ActionKind;

#[test]
fn a_delete_plan_cannot_be_authorized_under_a_policy_that_forbids_destructive_actions() {
    // The authorization gate touches no filesystem path; the tempdir keeps the
    // "every test owns a tempdir" rule uniform across the suite.
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    let plan = cleanup_plan(
        "plan:delete",
        vec![cleanup_item("CACHE:delete", 10, ActionKind::Delete)],
    );

    let error = authorize(plan, reversible_policy())
        .expect_err("a plan containing a Delete item must never be authorized");

    match error {
        AuthorizeError::Blocked { reasons } => {
            assert!(!reasons.is_empty(), "a block must carry its reasons");
            assert!(
                reasons.iter().any(|reason| reason.code == "GATE_BLOCKED"),
                "the safety gate must be one of the reasons, got {reasons:?}"
            );
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.detail.contains("DESTRUCTIVE_NOT_AUTHORIZED")),
                "the reason must name the destructive classification, got {reasons:?}"
            );
        }
        other => panic!("expected AuthorizeError::Blocked, got {other:?}"),
    }
}

#[test]
fn a_delete_plan_cannot_be_authorized_even_when_the_policy_allows_destruction() {
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    let plan = cleanup_plan(
        "plan:delete",
        vec![cleanup_item("CACHE:delete", 10, ActionKind::Delete)],
    );

    // The validator now returns Allowed â€” the policy authorizes destructive
    // effects â€” so this refusal can only come from this crate's own rule that
    // its token may never claim an action it cannot perform reversibly.
    let error = authorize(plan, destructive_policy())
        .expect_err("this crate has no deletion primitive and must not mint a token for one");

    match error {
        AuthorizeError::UnsupportedAction { item_id, action } => {
            assert_eq!(item_id, "CACHE:delete");
            assert_eq!(action, ActionKind::Delete);
        }
        other => panic!("expected AuthorizeError::UnsupportedAction, got {other:?}"),
    }
}

#[test]
fn an_allowed_plan_yields_a_token_that_names_its_identity_and_policy() {
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    let plan = cleanup_plan(
        "plan:allowed",
        vec![
            cleanup_item("CACHE:1", 40, ActionKind::MoveToTrash),
            cleanup_item("LOGS:2", 60, ActionKind::MoveToTrash),
        ],
    );

    let authorized = authorize(plan, reversible_policy()).expect("a reversible plan is allowed");

    assert_eq!(authorized.plan_id(), "plan:allowed");
    assert_eq!(authorized.total_recoverable_bytes(), 100);
    assert_eq!(authorized.plan().items.len(), 2);
    assert_eq!(authorized.authorization().item_count(), 2);
    assert_eq!(authorized.authorization().total_bytes(), 100);
    assert_eq!(authorized.authorized_action(), ActionKind::MoveToTrash);
    assert!(
        authorized.authorization().policy().reversible,
        "the token records the policy it was minted under"
    );
}

#[test]
fn an_empty_plan_yields_no_token() {
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    let error = authorize(cleanup_plan("plan:empty", Vec::new()), reversible_policy())
        .expect_err("a plan authorizing nothing must not produce a token");

    assert!(matches!(error, AuthorizeError::EmptyPlan), "got {error:?}");
}

#[test]
fn execute_requires_a_confirmation_for_the_same_plan_id() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source = root.join("cache.bin");
    fs::write(&source, b"cache bytes").expect("the fixture file is writable");
    assert_inside(root, &source);

    let plan = cleanup_plan(
        "plan:preview",
        vec![cleanup_item("CACHE:1", 11, ActionKind::MoveToTrash)],
    );
    let authorized = authorize(plan, reversible_policy()).expect("a reversible plan is allowed");
    let trash = DirectoryTrash::new(root.join("trash"));

    let wrong_id =
        ConfirmedPreview::new("plan:somebody-elses", authorized.total_recoverable_bytes());
    let error = execute(
        &authorized,
        &wrong_id,
        &trash,
        std::slice::from_ref(&source),
    )
    .expect_err("a confirmation for another plan must be refused");
    assert!(
        matches!(error, ExecuteError::NotConfirmed { .. }),
        "got {error:?}"
    );
    assert!(source.exists(), "a refused confirmation must touch nothing");

    let right_id =
        ConfirmedPreview::new(authorized.plan_id(), authorized.total_recoverable_bytes());
    let report = execute(
        &authorized,
        &right_id,
        &trash,
        std::slice::from_ref(&source),
    )
    .expect("the matching confirmation admits the plan");
    assert_eq!(report.moved.len(), 1);
    assert!(!source.exists());
    assert_inside(root, &report.moved[0].destination);
}

#[test]
fn execute_requires_a_confirmation_for_the_same_byte_total() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let source = root.join("cache.bin");
    fs::write(&source, b"cache bytes").expect("the fixture file is writable");
    assert_inside(root, &source);

    let plan = cleanup_plan(
        "plan:preview",
        vec![cleanup_item("CACHE:1", 11, ActionKind::MoveToTrash)],
    );
    let authorized = authorize(plan, reversible_policy()).expect("a reversible plan is allowed");
    let trash = DirectoryTrash::new(root.join("trash"));

    let wrong_bytes = ConfirmedPreview::new(
        authorized.plan_id(),
        authorized.total_recoverable_bytes() + 1,
    );
    let error = execute(
        &authorized,
        &wrong_bytes,
        &trash,
        std::slice::from_ref(&source),
    )
    .expect_err("a confirmation showing more bytes than the plan must be refused");
    assert!(
        matches!(error, ExecuteError::NotConfirmed { .. }),
        "got {error:?}"
    );
    assert!(source.exists(), "a refused confirmation must touch nothing");

    let understated = ConfirmedPreview::new(authorized.plan_id(), 0);
    let error = execute(
        &authorized,
        &understated,
        &trash,
        std::slice::from_ref(&source),
    )
    .expect_err("a confirmation showing fewer bytes than the plan must be refused");
    assert!(
        matches!(error, ExecuteError::NotConfirmed { .. }),
        "got {error:?}"
    );
    assert!(source.exists());
}
