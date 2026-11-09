//! Integration tests for `coresight-cleanup`.
//!
//! All fixtures are synthetic. No filesystem is touched; no file is
//! moved, deleted, or renamed. The tests prove the planner/validator
//! contracts: safe/ review/ off-limits tiers, preview completeness,
//! byte accounting, partial-knowledge blocking, duplicate rejection,
//! and that the capabilities gate blocks execution in the current build
//! while allowing a reversible plan under an explicitly authorized
//! policy.

use coresight_capabilities::safety::ExecutionPolicy;
use coresight_classifier::Category;
use coresight_cleanup::{
    plan_cleanup, validate_plan, validate_with_policy, ActionKind, CleanupItem, CleanupPlan,
    CleanupPreview, PlannerOptions, SafetyTier,
};
use coresight_engine::{EntryKind, FsEntry};
use coresight_history::{ClassificationRef, RunId, SnapshotBuilder};
use coresight_insights::{usage_by_category, UsageReport};
use std::path::PathBuf;

fn file(path: &str, size: u64) -> FsEntry {
    FsEntry {
        id: 0,
        parent_id: None,
        path: PathBuf::from(path),
        kind: EntryKind::File,
        size,
        allocated_size: Some(size),
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: Some(1),
        inode: Some(1),
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

fn classified(category: Category) -> Option<ClassificationRef> {
    Some(ClassificationRef {
        category: category.code().to_string(),
        subcategory: None,
    })
}

fn usage(entries: Vec<(FsEntry, Option<ClassificationRef>)>) -> UsageReport {
    let mut builder = SnapshotBuilder::new();
    for (entry, classification) in entries {
        builder.push_entry(&entry, classification);
    }
    usage_by_category(&builder.build(RunId("run".to_string())).unwrap())
}

fn safe_policy() -> ExecutionPolicy {
    ExecutionPolicy {
        read_only: true,
        reversible: true,
        destructive: false,
        privileged: false,
        permission_sensitive: false,
    }
}

#[test]
fn safe_categories_are_planned_and_reversible() {
    let report = usage(vec![
        (file("/cache1", 30_000), classified(Category::Cache)),
        (file("/tmp1", 20_000), classified(Category::TemporaryData)),
        (file("/log1", 5_000), classified(Category::Logs)),
    ]);
    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    assert_eq!(plan.items.len(), 3);
    assert_eq!(plan.total_recoverable_bytes, 55_000);
    assert!(plan.items.iter().all(|it| it.tier == SafetyTier::Safe));
    assert!(plan
        .items
        .iter()
        .all(|it| it.action == ActionKind::MoveToTrash));
    assert!(!plan.partial);
}

#[test]
fn off_limits_categories_are_excluded_and_reported() {
    let report = usage(vec![
        (file("/sys", 999_999), classified(Category::SystemData)),
        (file("/doc", 50_000), classified(Category::Documents)),
        (file("/app", 10_000), classified(Category::Applications)),
        (file("/cache", 1_000), classified(Category::Cache)),
    ]);
    let plan = plan_cleanup(
        &report,
        &PlannerOptions {
            include_review: true,
            min_recoverable_bytes: 1,
        },
    );
    let categories: Vec<Category> = plan.items.iter().map(|it| it.category).collect();
    assert!(
        !categories.contains(&Category::SystemData),
        "SystemData must never be actionable"
    );
    assert!(
        !categories.contains(&Category::Documents),
        "Documents must never be actionable"
    );
    assert_eq!(plan.off_limits_excluded.len(), 3);
    assert_eq!(plan.items.len(), 1);
}

#[test]
fn review_categories_require_explicit_opt_in() {
    let report = usage(vec![
        (file("/dl", 80_000), classified(Category::Downloads)),
        (file("/cache", 1_000), classified(Category::Cache)),
    ]);
    let silent = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    assert_eq!(silent.items.len(), 1, "review items are silent by default");
    let with_review = plan_cleanup(
        &report,
        &PlannerOptions {
            include_review: true,
            min_recoverable_bytes: 1,
        },
    );
    assert_eq!(with_review.items.len(), 2);
    assert!(with_review
        .items
        .iter()
        .any(|it| it.tier == SafetyTier::Review));
}

#[test]
fn preview_is_complete_for_planned_items() {
    let report = usage(vec![(file("/cache", 10_000), classified(Category::Cache))]);
    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    let preview = CleanupPreview::from_plan(&plan);
    assert!(preview.complete);
    assert_eq!(preview.total_bytes, 10_000);
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.destination, "OS Trash / app quarantine");
    assert!(preview.how_to_undo.contains("Restore"));
    assert!(!preview.what_stays_untouched.is_empty());
    assert!(!preview.consequence.is_empty());
}

#[test]
fn validator_blocks_off_limits_items() {
    let plan = CleanupPlan {
        id: "test".to_string(),
        items: vec![CleanupItem {
            id: "SYSTEM_DATA:1".to_string(),
            category: Category::SystemData,
            title: "System data".to_string(),
            why_recommended: "test".to_string(),
            recoverable_bytes: 1,
            tier: SafetyTier::OffLimits,
            what_stays_untouched: "nothing".to_string(),
            consequence: "bad".to_string(),
            destination: Some("Trash".to_string()),
            undo: Some("Restore".to_string()),
            action: ActionKind::MoveToTrash,
        }],
        total_recoverable_bytes: 1,
        partial: false,
        off_limits_excluded: vec![],
    };
    let report = validate_plan(&plan);
    assert!(!report.is_allowed());
    assert!(report
        .blocking_messages()
        .iter()
        .any(|m| m.contains("OFF_LIMITS_CATEGORY")));
}

#[test]
fn validator_blocks_destructive_actions_in_current_build() {
    let report = usage(vec![(file("/cache", 1_000), classified(Category::Cache))]);
    let mut plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    plan.items[0].action = ActionKind::Delete;
    let verdict = validate_plan(&plan);
    assert!(!verdict.is_allowed());
    assert!(verdict
        .blocking_messages()
        .iter()
        .any(|m| m.contains("GATE_BLOCKED")));
}

#[test]
fn validator_allows_reversible_under_authorized_policy() {
    let report = usage(vec![(file("/cache", 1_000), classified(Category::Cache))]);
    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    let verdict = validate_with_policy(&plan, safe_policy());
    assert!(verdict.is_allowed(), "got {:?}", verdict);
}

#[test]
fn partial_knowledge_blocks_safe_tier_plans() {
    let mut errored = file("/denied", 0);
    errored.error = Some(coresight_engine::ErrorCategoryRef::PermissionDenied);
    let report = usage(vec![
        (errored, classified(Category::SystemData)),
        (file("/cache", 1_000), classified(Category::Cache)),
    ]);
    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    assert!(plan.partial);
    let verdict = validate_plan(&plan);
    assert!(!verdict.is_allowed());
    assert!(verdict
        .blocking_messages()
        .iter()
        .any(|m| m.contains("PARTIAL_KNOWLEDGE")));
}

#[test]
fn validator_rejects_duplicate_ids() {
    let report = usage(vec![(file("/cache", 1_000), classified(Category::Cache))]);
    let mut plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    plan.items.push(plan.items[0].clone());
    plan.total_recoverable_bytes = plan
        .items
        .iter()
        .fold(0u64, |acc, it| acc.saturating_add(it.recoverable_bytes));
    let verdict = validate_plan(&plan);
    assert!(!verdict.is_allowed());
    assert!(verdict
        .blocking_messages()
        .iter()
        .any(|m| m.contains("DUPLICATE_ITEM")));
}

#[test]
fn validator_rejects_byte_accounting_mismatch() {
    let report = usage(vec![(file("/cache", 1_000), classified(Category::Cache))]);
    let mut plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    plan.total_recoverable_bytes = 999_999;
    let verdict = validate_plan(&plan);
    assert!(!verdict.is_allowed());
    assert!(verdict
        .blocking_messages()
        .iter()
        .any(|m| m.contains("BYTE_ACCOUNTING_MISMATCH")));
}

#[test]
fn planner_is_deterministic_across_input_permutations() {
    let forward = usage(vec![
        (file("/a", 300), classified(Category::Cache)),
        (file("/b", 100), classified(Category::TemporaryData)),
        (file("/c", 5_000), classified(Category::Logs)),
    ]);
    let backward = usage(vec![
        (file("/c", 5_000), classified(Category::Logs)),
        (file("/b", 100), classified(Category::TemporaryData)),
        (file("/a", 300), classified(Category::Cache)),
    ]);
    let p1 = plan_cleanup(&forward, &PlannerOptions::safe_defaults());
    let p2 = plan_cleanup(&backward, &PlannerOptions::safe_defaults());
    assert_eq!(p1, p2);
}

#[test]
fn empty_report_yields_an_empty_plan() {
    let plan = plan_cleanup(&UsageReport::default(), &PlannerOptions::safe_defaults());
    assert!(plan.items.is_empty());
    assert_eq!(plan.total_recoverable_bytes, 0);
}

// --- architecture guard ----------------------------------------------------

#[test]
fn crate_performs_no_io_or_execution() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for forbidden in [
            "std::fs::",
            "std::process",
            "std::net",
            "reqwest",
            "tokio",
            "rusqlite",
            "std::os::windows",
            "std::os::unix",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains forbidden API {forbidden}",
                path.display()
            );
        }
        // The only allowed raw string quoting is in tests; production
        // source must not contain process execution.
        assert!(
            !text.contains("Command::new"),
            "{} spawns processes",
            path.display()
        );
    }
}
