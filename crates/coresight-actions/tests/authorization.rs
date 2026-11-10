//! Authorization and confirmation: the two gates in front of execution.
//!
//! Contract 4 (a destructive plan can never be authorized under a policy that
//! forbids destruction), contract 5 (execution needs a confirmation that
//! matches the plan's id, resolved target count, and resolved byte total), and
//! the resolved-plan membership binding are proved here.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{
    assert_inside, cleanup_item, cleanup_plan, destructive_policy, resolved_plan, reversible_policy,
};
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
    let resolved = resolved_plan(&plan, vec![vec![(PathBuf::from("cache.bin"), 10)]]);

    let error = authorize(plan, resolved, reversible_policy())
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
    let resolved = resolved_plan(&plan, vec![vec![(PathBuf::from("cache.bin"), 10)]]);

    // The validator now returns Allowed — the policy authorizes destructive
    // effects — so this refusal can only come from this crate's own rule that
    // its token may never claim an action it cannot perform reversibly.
    let error = authorize(plan, resolved, destructive_policy())
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
    let resolved = resolved_plan(
        &plan,
        vec![
            vec![(PathBuf::from("cache.bin"), 40)],
            vec![(PathBuf::from("app.log"), 60)],
        ],
    );

    let authorized =
        authorize(plan, resolved, reversible_policy()).expect("a reversible plan is allowed");

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
    assert_eq!(authorized.resolved_target_count(), 2);
    assert_eq!(authorized.resolved_total_bytes(), 100);
    assert_eq!(authorized.authorization().resolved_target_count(), 2);
    assert_eq!(authorized.authorization().resolved_total_bytes(), 100);
    assert_eq!(
        authorized.authorized_paths().collect::<Vec<&Path>>(),
        vec![Path::new("app.log"), Path::new("cache.bin")],
        "the token freezes its resolved paths, in canonical set order"
    );
}

#[test]
fn an_empty_plan_yields_no_token() {
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    let plan = cleanup_plan("plan:empty", Vec::new());
    let resolved = resolved_plan(&plan, Vec::new());

    let error = authorize(plan, resolved, reversible_policy())
        .expect_err("a plan authorizing nothing must not produce a token");

    assert!(matches!(error, AuthorizeError::EmptyPlan), "got {error:?}");
}

#[test]
fn a_resolved_plan_that_does_not_describe_the_plan_cannot_be_authorized() {
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    // Wrong plan id: the resolved path set belongs to another plan.
    let plan = cleanup_plan(
        "plan:small",
        vec![cleanup_item("CACHE:1", 10, ActionKind::MoveToTrash)],
    );
    let mut wrong_id = resolved_plan(&plan, vec![vec![(PathBuf::from("cache.bin"), 10)]]);
    wrong_id.plan_id = "plan:somebody-elses".to_string();
    let error = authorize(plan, wrong_id, reversible_policy())
        .expect_err("a token must not be minted from another plan's resolution");
    assert!(
        matches!(error, AuthorizeError::ResolvedPlanMismatch { .. }),
        "got {error:?}"
    );

    // Declared totals that do not match the target list: without this check a
    // caller could declare a three-target preview while carrying nine paths.
    let plan = cleanup_plan(
        "plan:small",
        vec![cleanup_item("CACHE:1", 10, ActionKind::MoveToTrash)],
    );
    let mut lying = resolved_plan(&plan, vec![vec![(PathBuf::from("cache.bin"), 10)]]);
    lying.target_count = 3;
    let error = authorize(plan, lying, reversible_policy())
        .expect_err("the declared target count must equal the target lists");
    assert!(
        matches!(error, AuthorizeError::ResolvedPlanMismatch { .. }),
        "got {error:?}"
    );

    // Declared unmapped bytes that contradict the plan claim and the resolved
    // total are refused too: the audit number must be derived, not asserted.
    let plan = cleanup_plan(
        "plan:small",
        vec![cleanup_item("CACHE:1", 10, ActionKind::MoveToTrash)],
    );
    let mut lying = resolved_plan(&plan, vec![vec![(PathBuf::from("cache.bin"), 4)]]);
    lying.unmapped_bytes = 0;
    let error = authorize(plan, lying, reversible_policy())
        .expect_err("unmapped bytes must equal the claim minus the resolved total");
    assert!(
        matches!(error, AuthorizeError::ResolvedPlanMismatch { .. }),
        "got {error:?}"
    );

    // A resolved plan with more items than the plan's item list is refused.
    let two_item_plan = cleanup_plan(
        "plan:two",
        vec![
            cleanup_item("CACHE:1", 5, ActionKind::MoveToTrash),
            cleanup_item("LOGS:2", 5, ActionKind::MoveToTrash),
        ],
    );
    let oversized = resolved_plan(
        &two_item_plan,
        vec![
            vec![(PathBuf::from("cache.bin"), 5)],
            vec![(PathBuf::from("app.log"), 5)],
        ],
    );
    let one_item_plan = cleanup_plan(
        "plan:two",
        vec![cleanup_item("CACHE:1", 5, ActionKind::MoveToTrash)],
    );
    let error = authorize(one_item_plan, oversized, reversible_policy())
        .expect_err("the resolved plan must describe exactly this plan's items");
    assert!(
        matches!(error, AuthorizeError::ResolvedPlanMismatch { .. }),
        "got {error:?}"
    );
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
    let resolved = resolved_plan(&plan, vec![vec![(source.clone(), 11)]]);
    let authorized =
        authorize(plan, resolved, reversible_policy()).expect("a reversible plan is allowed");
    let trash = DirectoryTrash::new(root.join("trash"));

    let wrong_id = ConfirmedPreview::new(
        "plan:somebody-elses",
        authorized.resolved_total_bytes(),
        authorized.resolved_target_count(),
    );
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

    let right_id = ConfirmedPreview::new(
        authorized.plan_id(),
        authorized.resolved_total_bytes(),
        authorized.resolved_target_count(),
    );
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
    let resolved = resolved_plan(&plan, vec![vec![(source.clone(), 11)]]);
    let authorized =
        authorize(plan, resolved, reversible_policy()).expect("a reversible plan is allowed");
    let trash = DirectoryTrash::new(root.join("trash"));

    let wrong_bytes = ConfirmedPreview::new(
        authorized.plan_id(),
        authorized.resolved_total_bytes() + 1,
        authorized.resolved_target_count(),
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

    let understated =
        ConfirmedPreview::new(authorized.plan_id(), 0, authorized.resolved_target_count());
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

#[test]
fn a_confirmation_for_a_three_target_preview_cannot_authorise_a_nine_target_execution() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let trash = DirectoryTrash::new(root.join("trash"));

    let mut targets: Vec<(PathBuf, u64)> = Vec::new();
    for index in 0..9 {
        let path = root.join(format!("cache-{index}.bin"));
        fs::write(&path, b"x").expect("writable");
        assert_inside(root, &path);
        targets.push((path, 1));
    }
    let plan = cleanup_plan(
        "plan:nine",
        vec![cleanup_item("CACHE:9", 9, ActionKind::MoveToTrash)],
    );
    let resolved = resolved_plan(&plan, vec![targets.clone()]);
    let authorized =
        authorize(plan, resolved, reversible_policy()).expect("nine targets authorize");

    // Exactly what a three-file preview would have confirmed: three targets,
    // three bytes. It must not authorize the nine-target execution.
    let three_file_preview = ConfirmedPreview::new(authorized.plan_id(), 3, 3);
    let paths: Vec<PathBuf> = targets.iter().map(|(path, _)| path.clone()).collect();
    let error = execute(&authorized, &three_file_preview, &trash, &paths)
        .expect_err("a three-target confirmation must not authorize nine targets");
    assert!(
        matches!(error, ExecuteError::NotConfirmed { .. }),
        "got {error:?}"
    );
    for path in &paths {
        assert!(
            path.exists(),
            "a refused confirmation must touch nothing: {}",
            path.display()
        );
    }

    // The same nine paths with the resolved confirmation do execute, so the
    // refusal above came from the numbers, not from a broken fixture.
    let nine_targets = ConfirmedPreview::from_resolved(authorized.resolved());
    let report = execute(&authorized, &nine_targets, &trash, &paths)
        .expect("the confirmation for the nine-target resolved plan admits it");
    assert_eq!(report.moved.len(), 9);
}
