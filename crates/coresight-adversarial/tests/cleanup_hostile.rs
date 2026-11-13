//! Hostile inputs aimed at the cleanup planning, validation and resolution
//! layer.
//!
//! Every fixture is synthetic: no path here exists, nothing is read, moved or
//! deleted. Each test names the honesty property it protects and states what
//! breaks if it regresses.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use coresight_classifier::Category;
use coresight_cleanup::{
    plan_cleanup, resolve_plan, validate_plan, ActionKind, CleanupItem, CleanupPlan,
    CleanupPreview, PlannerOptions, ResolveError, ResolveOptions, ResolvedPlan, SafetyTier,
    TargetKind, ValidationReport,
};
use coresight_history::{ClassificationRef, ObservedEntry, ObservedKind, RunId, Snapshot};
use coresight_insights::{usage_by_category, CategoryTotals, UsageReport};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Chainable builder for one synthetic observed entry. Every fact defaults to
/// "unknown", which is exactly the state hostile input tends to attack.
struct Entry(ObservedEntry);

impl Entry {
    fn new(path: impl Into<PathBuf>, kind: ObservedKind) -> Self {
        Entry(ObservedEntry {
            path: path.into(),
            kind,
            size: None,
            object: None,
            modified: None,
            classification: None,
            content_sha256: None,
            observation_error: None,
        })
    }

    fn file(path: impl Into<PathBuf>) -> Self {
        Entry::new(path, ObservedKind::File)
    }

    fn dir(path: impl Into<PathBuf>) -> Self {
        Entry::new(path, ObservedKind::Dir)
    }

    fn link(path: impl Into<PathBuf>) -> Self {
        Entry::new(path, ObservedKind::Link)
    }

    fn size(mut self, size: u64) -> Self {
        self.0.size = Some(size);
        self
    }

    fn category(mut self, category: Category) -> Self {
        self.0.classification = Some(ClassificationRef {
            category: category.code().to_string(),
            subcategory: None,
        });
        self
    }

    fn raw_category(mut self, code: &str) -> Self {
        self.0.classification = Some(ClassificationRef {
            category: code.to_string(),
            subcategory: None,
        });
        self
    }

    fn observation_error(mut self, code: &str) -> Self {
        self.0.observation_error = Some(code.to_string());
        self
    }

    fn modified(mut self, when: SystemTime) -> Self {
        self.0.modified = Some(when);
        self
    }

    fn build(self) -> ObservedEntry {
        self.0
    }
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("run-hostile".to_string()),
        entries,
    }
}

fn report_with(pairs: &[(Category, u64)]) -> UsageReport {
    let mut category = vec![CategoryTotals::default(); Category::COUNT];
    for (target, bytes) in pairs {
        let index = Category::ALL
            .iter()
            .position(|candidate| candidate == target)
            .expect("category is part of Category::ALL");
        category[index] = CategoryTotals {
            entries: 1,
            files: 1,
            directories: 0,
            logical_size: *bytes,
            entries_without_size: 0,
        };
    }
    UsageReport {
        category,
        scanned_entries: pairs.len() as u64,
        errored_entries: 0,
        entries_without_size: 0,
        unclassified_entries: 0,
    }
}

fn plan_item(id: &str, category: Category, bytes: u64, tier: SafetyTier) -> CleanupItem {
    CleanupItem {
        id: id.to_string(),
        category,
        title: format!("{category:?}"),
        why_recommended: "because the fixture says so".to_string(),
        recoverable_bytes: bytes,
        tier,
        what_stays_untouched: "everything else".to_string(),
        consequence: "nothing permanent".to_string(),
        destination: Some("OS Trash / app quarantine".to_string()),
        undo: Some("Restore from Trash/quarantine".to_string()),
        action: ActionKind::MoveToTrash,
    }
}

fn plan_with(items: Vec<CleanupItem>) -> CleanupPlan {
    let total = items
        .iter()
        .fold(0u64, |sum, item| sum.saturating_add(item.recoverable_bytes));
    CleanupPlan {
        id: "plan:fixture".to_string(),
        items,
        total_recoverable_bytes: total,
        partial: false,
        off_limits_excluded: Vec::new(),
    }
}

fn reason_codes(report: &ValidationReport) -> Vec<&'static str> {
    report.verdict.reasons().iter().map(|r| r.code).collect()
}

/// `CleanupPlan` deliberately has no `Serialize` impl, so the byte-identical
/// assertion is made on an explicit JSON projection of every published field.
fn plan_json(plan: &CleanupPlan) -> String {
    let items: Vec<serde_json::Value> = plan
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "id": item.id,
                "category": item.category.code(),
                "title": item.title,
                "why": item.why_recommended,
                "bytes": item.recoverable_bytes,
                "tier": item.tier,
                "stays": item.what_stays_untouched,
                "consequence": item.consequence,
                "destination": item.destination,
                "undo": item.undo,
                "action": format!("{:?}", item.action),
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "id": plan.id,
        "items": items,
        "total": plan.total_recoverable_bytes,
        "partial": plan.partial,
        "offLimits": plan.off_limits_excluded.iter().map(|c| c.code()).collect::<Vec<_>>(),
    }))
    .expect("the projection contains no non-UTF-8 data")
}

fn assert_well_formed(resolved: &ResolvedPlan) {
    let summed = resolved
        .items
        .iter()
        .flat_map(|item| item.targets.iter())
        .fold(0u64, |sum, target| sum.saturating_add(target.bytes));
    assert_eq!(
        resolved.total_bytes, summed,
        "totalBytes must be the saturating sum of the targets it publishes"
    );
    let counted = resolved
        .items
        .iter()
        .map(|item| item.targets.len())
        .fold(0usize, |sum, len| sum.saturating_add(len));
    assert_eq!(resolved.target_count, counted);
}

// ---------------------------------------------------------------------------
// Degenerate and hostile magnitude
// ---------------------------------------------------------------------------

// If this regresses, two huge category totals wrap to a small number and the
// user is told a multi-exabyte bucket is a few bytes.
#[test]
fn planner_total_saturates_at_u64_max() {
    let report = report_with(&[(Category::Cache, u64::MAX), (Category::Logs, u64::MAX)]);
    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());

    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.total_recoverable_bytes, u64::MAX);
    assert!(
        plan.id.contains("18446744073709551615"),
        "the plan id must carry the saturated total, got {}",
        plan.id
    );
}

// If this regresses, the validator's own byte sum wraps and a plan that
// over-claims by an unrepresentable amount is reported as balanced.
#[test]
fn validator_byte_sum_saturates_at_u64_max() {
    let plan = plan_with(vec![
        plan_item("CACHE:max", Category::Cache, u64::MAX, SafetyTier::Safe),
        plan_item("LOGS:max", Category::Logs, u64::MAX, SafetyTier::Safe),
    ]);
    assert_eq!(plan.total_recoverable_bytes, u64::MAX);

    let codes = reason_codes(&validate_plan(&plan));
    assert!(
        !codes.contains(&"BYTE_ACCOUNTING_MISMATCH"),
        "a saturating sum equal to the declared total must not be a mismatch: {codes:?}"
    );
    assert!(
        codes.iter().all(|code| *code == "GATE_BLOCKED"),
        "only the current-build policy veto may remain: {codes:?}"
    );
}

// If this regresses, the planner's minimum-size gate accepts one byte less
// than the caller asked for (an off-by-one at u64::MAX).
#[test]
fn planner_minimum_size_boundary_is_exact_at_u64_max() {
    let at_limit = report_with(&[(Category::Cache, u64::MAX)]);
    let below_limit = report_with(&[(Category::Cache, u64::MAX - 1)]);
    let options = PlannerOptions {
        include_review: false,
        min_recoverable_bytes: u64::MAX,
    };

    assert_eq!(plan_cleanup(&at_limit, &options).items.len(), 1);
    assert_eq!(plan_cleanup(&below_limit, &options).items.len(), 0);
}

// If this regresses, a `usize::MAX` target budget is treated as an allocation
// request and the process hangs or aborts instead of resolving the plan.
#[test]
fn usize_max_target_budget_does_not_allocate_or_hang() {
    let snapshot = snapshot(vec![
        Entry::file("/cache/a")
            .size(1)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/b")
            .size(2)
            .category(Category::Cache)
            .build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:3",
        Category::Cache,
        3,
        SafetyTier::Safe,
    )]);
    let resolved = resolve_plan(
        &plan,
        &snapshot,
        &ResolveOptions {
            max_targets: usize::MAX,
        },
    )
    .expect("an effectively unbounded budget accepts two targets");

    assert_eq!(resolved.target_count, 2);
    assert_well_formed(&resolved);
}

// If this regresses, an empty plan is resolved into an "empty but successful"
// plan the user could confirm while nothing was ever listed.
#[test]
fn empty_plan_and_empty_snapshot_are_refused_not_silently_empty() {
    let empty_snapshot = snapshot(Vec::new());
    let empty_plan = CleanupPlan {
        id: "plan:empty".to_string(),
        items: Vec::new(),
        total_recoverable_bytes: 0,
        partial: false,
        off_limits_excluded: Vec::new(),
    };
    assert_eq!(
        resolve_plan(&empty_plan, &empty_snapshot, &ResolveOptions::default()),
        Err(ResolveError::EmptyPlan)
    );

    let one_item = plan_with(vec![plan_item(
        "CACHE:1",
        Category::Cache,
        1,
        SafetyTier::Safe,
    )]);
    assert_eq!(
        resolve_plan(&one_item, &empty_snapshot, &ResolveOptions::default()),
        Err(ResolveError::PlanItemMissing {
            item_id: "CACHE:1".to_string()
        })
    );
}

// If this regresses, a plan whose item has no matching entry vanishes from the
// preview while its claimed bytes stay in the review.
#[test]
fn an_item_with_no_concrete_target_is_refused_not_dropped() {
    let snapshot = snapshot(vec![Entry::file("/logs/one")
        .size(10)
        .category(Category::Logs)
        .build()]);
    let plan = plan_with(vec![
        plan_item("CACHE:5", Category::Cache, 5, SafetyTier::Safe),
        plan_item("LOGS:10", Category::Logs, 10, SafetyTier::Safe),
    ]);

    assert_eq!(
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()),
        Err(ResolveError::PlanItemMissing {
            item_id: "CACHE:5".to_string()
        })
    );
}

// If this regresses, an over-budget resolution returns a shortened target
// list, i.e. a plan the user never reviewed.
#[test]
fn target_budget_exceeded_is_an_error_never_a_truncation() {
    let snapshot = snapshot(vec![
        Entry::file("/cache/a")
            .size(1)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/b")
            .size(1)
            .category(Category::Cache)
            .build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:2",
        Category::Cache,
        2,
        SafetyTier::Safe,
    )]);

    assert_eq!(
        resolve_plan(&plan, &snapshot, &ResolveOptions { max_targets: 1 }),
        Err(ResolveError::TargetBudgetExceeded {
            max_targets: 1,
            requested: 2
        })
    );
}

// ---------------------------------------------------------------------------
// Contradictory input
// ---------------------------------------------------------------------------

// If this regresses, a plan whose total is smaller than one of its own lines
// passes validation and the preview understates what would be moved.
#[test]
fn validator_rejects_a_total_smaller_than_one_item() {
    let mut plan = plan_with(vec![plan_item(
        "CACHE:10",
        Category::Cache,
        10,
        SafetyTier::Safe,
    )]);
    plan.total_recoverable_bytes = 1;

    let verdict = validate_plan(&plan);
    assert!(!verdict.is_allowed());
    assert!(
        reason_codes(&verdict).contains(&"BYTE_ACCOUNTING_MISMATCH"),
        "got {:?}",
        verdict.blocking_messages()
    );
    let message = verdict.blocking_messages().join(" | ");
    assert!(
        message.contains("plan total 1") && message.contains("sum of its items 10"),
        "the message must publish both numbers, got {message}"
    );
}

// If this regresses, only the first problem is reported and the user fixes one
// blocker at a time while the rest stay hidden.
#[test]
fn validator_reports_every_independent_problem_not_just_the_first() {
    let mut broken = plan_item(
        "SYSTEM_DATA:5",
        Category::SystemData,
        5,
        SafetyTier::OffLimits,
    );
    broken.why_recommended.clear();
    broken.destination = None;
    broken.undo = None;

    let mut plan = plan_with(vec![broken.clone(), broken]);
    plan.total_recoverable_bytes = 10;

    let verdict = validate_plan(&plan);
    let codes = reason_codes(&verdict);
    for expected in [
        "OFF_LIMITS_CATEGORY",
        "MISSING_REASONING",
        "INCOMPLETE_PREVIEW",
        "DUPLICATE_ITEM",
    ] {
        assert!(
            codes.contains(&expected),
            "missing {expected} in {codes:?} ({:?})",
            verdict.blocking_messages()
        );
    }
}

// If this regresses, a report with unmeasured entries is planned as if its
// byte totals were complete.
#[test]
fn planner_marks_a_report_with_unknown_sizes_partial_and_blocks_it() {
    let mut report = report_with(&[(Category::Cache, 1_000)]);
    report.entries_without_size = 1;

    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    assert!(plan.partial, "unknown sizes must mark the plan partial");
    let verdict = validate_plan(&plan);
    assert!(!verdict.is_allowed());
    assert!(reason_codes(&verdict).contains(&"PARTIAL_KNOWLEDGE"));
}

// If this regresses, observation errors are dropped and a partial scan is
// presented as a complete cleanup plan.
#[test]
fn planner_marks_a_report_with_observation_errors_partial() {
    let mut report = report_with(&[(Category::TemporaryData, 42)]);
    report.errored_entries = 3;

    let plan = plan_cleanup(&report, &PlannerOptions::safe_defaults());
    assert!(plan.partial);
    assert!(!validate_plan(&plan).is_allowed());
}

// If this regresses, a plan claiming 999 bytes while its only item says 10 is
// resolved with the gap silently zeroed.
#[test]
fn resolved_unmapped_bytes_publish_the_gap_and_never_go_negative() {
    let snapshot = snapshot(vec![Entry::file("/cache/a")
        .size(10)
        .category(Category::Cache)
        .build()]);
    let mut over_claiming = plan_with(vec![plan_item(
        "CACHE:10",
        Category::Cache,
        10,
        SafetyTier::Safe,
    )]);
    over_claiming.total_recoverable_bytes = 1_000;

    let resolved = resolve_plan(&over_claiming, &snapshot, &ResolveOptions::default())
        .expect("one concrete file resolves");
    assert_eq!(resolved.total_bytes, 10);
    assert_eq!(resolved.unmapped_bytes, 990);
    assert_well_formed(&resolved);

    let mut under_claiming = over_claiming.clone();
    under_claiming.total_recoverable_bytes = 5;
    let resolved = resolve_plan(&under_claiming, &snapshot, &ResolveOptions::default())
        .expect("one concrete file resolves");
    assert_eq!(resolved.total_bytes, 10);
    assert_eq!(
        resolved.unmapped_bytes, 0,
        "a claim smaller than the pinning must saturate at zero, never wrap"
    );
}

// If this regresses, a resolved plan publishes a total that disagrees with the
// targets a preview renders line by line.
#[test]
fn resolved_total_equals_the_saturating_sum_of_its_targets() {
    let snapshot = snapshot(vec![
        Entry::file("/cache/a")
            .size(u64::MAX)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/b")
            .size(u64::MAX)
            .category(Category::Cache)
            .build(),
    ]);
    let mut plan = plan_with(vec![plan_item(
        "CACHE:max",
        Category::Cache,
        u64::MAX,
        SafetyTier::Safe,
    )]);
    plan.total_recoverable_bytes = u64::MAX;

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("two files resolve");
    assert_eq!(resolved.total_bytes, u64::MAX);
    assert_eq!(resolved.unmapped_bytes, 0);
    assert_well_formed(&resolved);
    assert_eq!(
        resolved
            .items
            .iter()
            .flat_map(|item| item.targets.iter())
            .map(|target| target.bytes)
            .collect::<Vec<_>>(),
        vec![u64::MAX, u64::MAX]
    );
}

// If this regresses, a directory target's byte count wraps and a folder of two
// exabyte files is reported as empty.
#[test]
fn directory_target_bytes_are_a_saturating_lower_bound() {
    let snapshot = snapshot(vec![
        Entry::dir("/cache").category(Category::Cache).build(),
        Entry::file("/cache/a")
            .size(u64::MAX)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/b")
            .size(u64::MAX)
            .category(Category::Cache)
            .build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:max",
        Category::Cache,
        u64::MAX,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the directory resolves");
    assert_eq!(resolved.target_count, 1, "the parent absorbs its children");
    assert_eq!(resolved.items[0].targets[0].kind, TargetKind::Directory);
    assert_eq!(resolved.items[0].targets[0].bytes, u64::MAX);
    assert_well_formed(&resolved);
}

// If this regresses, a directory and its own child are both listed as targets
// and the executor attempts the same move twice.
#[test]
fn nested_paths_are_absorbed_so_no_path_is_moved_twice() {
    let snapshot = snapshot(vec![
        Entry::dir("/cache").category(Category::Cache).build(),
        Entry::dir("/cache/old").category(Category::Cache).build(),
        Entry::file("/cache/old/a")
            .size(7)
            .category(Category::Cache)
            .build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:7",
        Category::Cache,
        7,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the root resolves");
    assert_eq!(resolved.target_count, 1);
    assert_eq!(
        resolved.items[0].targets[0].path,
        PathBuf::from("/cache"),
        "the shallowest directory must win"
    );
    assert_eq!(resolved.items[0].targets[0].bytes, 7);
}

// ---------------------------------------------------------------------------
// Unknown masquerading as known
// ---------------------------------------------------------------------------

// If this regresses, an entry we could not read, could not size, or could not
// classify is folded into a concrete target and its bytes are invented.
#[test]
fn unreadable_unsized_and_non_file_entries_are_counted_not_targeted() {
    let snapshot = snapshot(vec![
        Entry::dir("/cache").category(Category::Cache).build(),
        Entry::file("/cache/good")
            .size(100)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/unsized")
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/denied")
            .size(1)
            .category(Category::Cache)
            .observation_error("PERMISSION_DENIED")
            .build(),
        Entry::link("/cache/link").category(Category::Cache).build(),
        Entry::new("/cache/socket", ObservedKind::Other)
            .category(Category::Cache)
            .build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:100",
        Category::Cache,
        100,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("one good file resolves");
    assert_eq!(resolved.target_count, 1, "only the good file is a target");
    assert_eq!(
        resolved.unresolved_entries, 4,
        "unsized, unreadable, link and other must each be counted exactly once"
    );
    assert_well_formed(&resolved);

    // The unreadable file's own size must not leak into the directory total.
    assert_eq!(resolved.total_bytes, 100);
}

// If this regresses, an unrecognized stored code is guessed into `Unknown`
// (or into any other bucket) and becomes actionable.
#[test]
fn an_unrecognized_category_code_is_never_guessed_into_unknown() {
    let snapshot = snapshot(vec![Entry::file("/cache/future")
        .size(500)
        .raw_category("FUTURE_CATEGORY_FROM_A_NEWER_RULES_VERSION")
        .build()]);

    let report = usage_by_category(&snapshot);
    assert_eq!(report.unclassified_entries, 1);
    assert_eq!(report.totals_for(Category::Unknown).logical_size, 500);
    assert_eq!(report.totals_for(Category::Other).logical_size, 0);
    assert_eq!(report.totals_for(Category::Cache).logical_size, 0);

    let plan = plan_with(vec![plan_item(
        "UNKNOWN:500",
        Category::Unknown,
        500,
        SafetyTier::OffLimits,
    )]);
    assert_eq!(
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()),
        Err(ResolveError::PlanItemMissing {
            item_id: "UNKNOWN:500".to_string()
        }),
        "an undecodable code must match nothing, not even the Unknown bucket"
    );
}

// If this regresses, a directory with unknown-size children is reported with a
// complete byte total it cannot prove.
#[test]
fn a_directory_with_unknown_children_reports_only_what_it_could_weigh() {
    let snapshot = snapshot(vec![
        Entry::dir("/logs").category(Category::Logs).build(),
        Entry::file("/logs/a")
            .size(5)
            .category(Category::Logs)
            .build(),
        Entry::file("/logs/b").category(Category::Logs).build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "LOGS:5",
        Category::Logs,
        5,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the directory resolves");
    assert_eq!(resolved.total_bytes, 5, "the unknown child adds nothing");
    assert_eq!(
        resolved.unresolved_entries, 1,
        "the unsized child must still be counted as unresolved"
    );
}

// ---------------------------------------------------------------------------
// Ordering and determinism
// ---------------------------------------------------------------------------

// If this regresses, two snapshots with the same facts in a different order
// produce different plans, so a cached preview no longer matches the plan.
#[test]
fn planner_output_is_byte_identical_across_entry_permutations() {
    let entries = [
        Entry::file("/cache/a")
            .size(30)
            .category(Category::Cache)
            .build(),
        Entry::file("/logs/b")
            .size(10)
            .category(Category::Logs)
            .build(),
        Entry::file("/tmp/c")
            .size(20)
            .category(Category::TemporaryData)
            .build(),
    ];
    let orders = [
        vec![entries[0].clone(), entries[1].clone(), entries[2].clone()],
        vec![entries[2].clone(), entries[0].clone(), entries[1].clone()],
        vec![entries[2].clone(), entries[1].clone(), entries[0].clone()],
    ];

    let plans: Vec<String> = orders
        .iter()
        .map(|order| {
            let report = usage_by_category(&snapshot(order.clone()));
            plan_json(&plan_cleanup(&report, &PlannerOptions::safe_defaults()))
        })
        .collect();

    assert_eq!(plans[0], plans[1]);
    assert_eq!(plans[1], plans[2]);
}

// If this regresses, the concrete target list depends on snapshot arrival
// order and a preview can be reviewed against a different order than it runs.
#[test]
fn resolved_plan_is_byte_identical_across_snapshot_permutations() {
    let entries = [
        Entry::file("/cache/a")
            .size(1)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/b")
            .size(2)
            .category(Category::Cache)
            .build(),
        Entry::file("/cache/c")
            .size(3)
            .category(Category::Cache)
            .build(),
    ];
    let orders = [
        vec![entries[0].clone(), entries[1].clone(), entries[2].clone()],
        vec![entries[1].clone(), entries[2].clone(), entries[0].clone()],
        vec![entries[2].clone(), entries[1].clone(), entries[0].clone()],
    ];
    let plan = plan_with(vec![plan_item(
        "CACHE:6",
        Category::Cache,
        6,
        SafetyTier::Safe,
    )]);

    let resolved: Vec<String> = orders
        .iter()
        .map(|order| {
            let resolved =
                resolve_plan(&plan, &snapshot(order.clone()), &ResolveOptions::default())
                    .expect("three files resolve");
            serde_json::to_string(&resolved).expect("utf-8 fixtures serialize")
        })
        .collect();

    assert_eq!(resolved[0], resolved[1]);
    assert_eq!(resolved[1], resolved[2]);
    assert!(
        resolved[0].contains("/cache/a") && resolved[0].contains("/cache/c"),
        "the fixture must actually exercise ordering"
    );
}

// ---------------------------------------------------------------------------
// Path and string abuse
// ---------------------------------------------------------------------------

// If this regresses, an empty or dot path from a corrupt snapshot becomes an
// actionable target; executing it would move the working directory or a bare
// relative name the user never saw.
#[test]
fn degenerate_paths_never_become_actionable_cleanup_targets() {
    for hostile in ["", ".", "..", "cache/", "cache/.", "report ", "report."] {
        let snapshot = snapshot(vec![Entry::file(hostile)
            .size(10)
            .category(Category::Cache)
            .build()]);
        let plan = plan_with(vec![plan_item(
            "CACHE:10",
            Category::Cache,
            10,
            SafetyTier::Safe,
        )]);
        assert_eq!(
            resolve_plan(&plan, &snapshot, &ResolveOptions::default()),
            Err(ResolveError::PlanItemMissing {
                item_id: "CACHE:10".to_string()
            }),
            "path {hostile:?} must be refused, not published as a move target"
        );
    }
}

// If this regresses, embedded NULs or reserved device names survive into a
// move list instead of being rejected at the planning boundary.
#[test]
fn impossible_windows_paths_never_become_actionable_targets() {
    for hostile in ["cache\0name", "CON", "NUL", "PRN", "AUX", "COM1", "CON.txt"] {
        let snapshot = snapshot(vec![Entry::file(hostile)
            .size(10)
            .category(Category::Cache)
            .build()]);
        let plan = plan_with(vec![plan_item(
            "CACHE:10",
            Category::Cache,
            10,
            SafetyTier::Safe,
        )]);
        assert_eq!(
            resolve_plan(&plan, &snapshot, &ResolveOptions::default()),
            Err(ResolveError::PlanItemMissing {
                item_id: "CACHE:10".to_string()
            }),
            "path {hostile:?} must not be offered for a move"
        );
    }
}

// If this regresses, a rejected path is dropped without being counted, so the
// report claims a complete resolution while something was skipped.
#[test]
fn a_rejected_path_is_counted_as_unresolved() {
    let snapshot = snapshot(vec![
        Entry::file("/cache/good")
            .size(10)
            .category(Category::Cache)
            .build(),
        Entry::file("CON").size(1).category(Category::Cache).build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:11",
        Category::Cache,
        11,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the good file resolves");
    assert_eq!(resolved.target_count, 1);
    assert_eq!(resolved.unresolved_entries, 1);
}
// If this regresses, a non-UTF-8 path is lossily rewritten (mojibake) and the
// executor is pointed at a different file than the one the user reviewed.
#[test]
fn a_non_utf8_path_is_preserved_byte_exactly_and_never_laundered() {
    let hostile = non_utf8_path();
    let snapshot = snapshot(vec![Entry::file(hostile.clone())
        .size(10)
        .category(Category::Cache)
        .build()]);
    let plan = plan_with(vec![plan_item(
        "CACHE:10",
        Category::Cache,
        10,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the entry resolves");
    assert_eq!(
        resolved.items[0].targets[0]
            .path
            .as_os_str()
            .as_encoded_bytes(),
        hostile.as_os_str().as_encoded_bytes(),
        "path identity must be byte-exact"
    );
    assert!(
        serde_json::to_string(&resolved).is_err(),
        "a non-UTF-8 path must fail JSON serialization loudly instead of becoming lossy text"
    );
}

// If this regresses, a very long path panics the resolver or is truncated into
// a different path.
#[test]
fn a_4096_character_path_is_kept_verbatim_without_truncation() {
    let long = format!("/cache/{}", "a".repeat(4_000));
    let snapshot = snapshot(vec![Entry::file(long.clone())
        .size(3)
        .category(Category::Cache)
        .build()]);
    let plan = plan_with(vec![plan_item(
        "CACHE:3",
        Category::Cache,
        3,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the entry resolves");
    assert_eq!(
        resolved.items[0].targets[0]
            .path
            .as_os_str()
            .as_encoded_bytes()
            .len(),
        long.len()
    );
}

// If this regresses, a plan that only differs by a trailing separator spelling
// is treated as two distinct locations and both are moved.
#[test]
fn trailing_separators_do_not_create_a_second_target_for_one_directory() {
    let snapshot = snapshot(vec![
        Entry::dir("/cache/").category(Category::Cache).build(),
        Entry::file("/cache/a")
            .size(4)
            .category(Category::Cache)
            .build(),
    ]);
    let plan = plan_with(vec![plan_item(
        "CACHE:4",
        Category::Cache,
        4,
        SafetyTier::Safe,
    )]);

    let resolved =
        resolve_plan(&plan, &snapshot, &ResolveOptions::default()).expect("the directory resolves");
    assert_eq!(resolved.target_count, 1);
    assert_eq!(resolved.total_bytes, 4);
}

// ---------------------------------------------------------------------------
// Preview honesty
// ---------------------------------------------------------------------------

// If this regresses, a preview says it is complete while an item has no
// destination or no undo path, and the UI offers a confirm button for a move
// that cannot be reversed or located.
#[test]
fn preview_is_incomplete_when_an_item_has_no_destination_or_undo() {
    let mut item = plan_item("CACHE:1", Category::Cache, 1, SafetyTier::Safe);
    item.destination = None;
    let preview = CleanupPreview::from_plan(&plan_with(vec![item]));
    assert!(!preview.complete);
    assert_eq!(preview.items[0].destination, "UNSET");

    let mut item = plan_item("CACHE:1", Category::Cache, 1, SafetyTier::Safe);
    item.undo = None;
    let preview = CleanupPreview::from_plan(&plan_with(vec![item]));
    assert!(!preview.complete);
    assert_eq!(preview.items[0].undo, "UNSET");
}

// If this regresses, the preview's total silently disagrees with the plan it
// projects, so the number the user confirms is not the number that was planned.
#[test]
fn preview_total_is_a_faithful_projection_of_the_plan() {
    let mut plan = plan_with(vec![plan_item(
        "CACHE:10",
        Category::Cache,
        10,
        SafetyTier::Safe,
    )]);
    plan.total_recoverable_bytes = 999;
    let preview = CleanupPreview::from_plan(&plan);
    assert_eq!(preview.total_bytes, plan.total_recoverable_bytes);
    assert_eq!(preview.items[0].bytes, 10);
    assert!(
        !validate_plan(&plan).is_allowed(),
        "the validator is the gate that must catch this contradiction"
    );
}

// If this regresses, an empty plan produces a preview that claims a move
// happened when nothing was planned.
#[test]
fn an_empty_plan_previews_as_nothing_moved() {
    let preview = CleanupPreview::from_plan(&CleanupPlan {
        id: "plan:empty".to_string(),
        items: Vec::new(),
        total_recoverable_bytes: 0,
        partial: false,
        off_limits_excluded: Vec::new(),
    });
    assert_eq!(preview.total_bytes, 0);
    assert!(preview.items.is_empty());
    assert_eq!(preview.consequence, "Nothing will be moved.");
}

// ---------------------------------------------------------------------------
// Timing facts must not become evidence
// ---------------------------------------------------------------------------

// If this regresses, a future timestamp is read as "old enough" and an
// untouched file is offered for cleanup.
#[test]
fn a_future_modification_time_is_not_evidence_of_age() {
    let future = SystemTime::now() + Duration::from_secs(86_400);
    let snapshot = snapshot(vec![Entry::file("/cache/future")
        .size(10)
        .category(Category::Cache)
        .modified(future)
        .build()]);
    let report = usage_by_category(&snapshot);
    assert_eq!(report.totals_for(Category::Cache).logical_size, 10);
}

// If this regresses, a report with fewer buckets than the taxonomy invents
// bytes for the missing ones.
// OBSERVATION: `plan_cleanup` treats a missing bucket as zero and derives
// `partial` only from the error counters, so a truncated `UsageReport` yields
// an empty, non-partial plan. Reported, not asserted as a defect.
#[test]
fn truncated_report_buckets_are_not_invented() {
    let mut truncated = report_with(&[]);
    truncated.category.truncate(3);
    truncated.scanned_entries = 50;
    let plan = plan_cleanup(&truncated, &PlannerOptions::safe_defaults());

    assert!(plan.items.is_empty());
    assert_eq!(plan.total_recoverable_bytes, 0);
    assert!(!plan.partial);
}

#[cfg(windows)]
fn non_utf8_path() -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    PathBuf::from(OsString::from_wide(&[
        0x0063, 0x0061, 0x0063, 0x0068, 0x0065, 0xD800, 0x002E, 0x0074, 0x0078, 0x0074,
    ]))
}

#[cfg(unix)]
fn non_utf8_path() -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(vec![
        b'c', b'a', b'c', b'h', b'e', 0xFF, b'.', b't', b'x', b't',
    ]))
}
