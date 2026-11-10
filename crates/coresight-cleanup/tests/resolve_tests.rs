//! Resolution tests: a category-level plan becomes concrete, reviewed paths.
//!
//! Every fixture is synthetic and in memory: `resolve_plan` reads only a
//! `Snapshot`, so these tests touch no filesystem. Each module rule has its
//! own test, and the determinism rule is proved by permuting the input.

use coresight_classifier::Category;
use coresight_cleanup::{
    resolve_plan, ActionKind, CleanupItem, CleanupPlan, PlanTarget, ResolveError, ResolveOptions,
    ResolvedPlan, SafetyTier, TargetKind,
};
use coresight_history::{ClassificationRef, ObservedEntry, ObservedKind, RunId, Snapshot};
use std::path::PathBuf;

// --- fixtures --------------------------------------------------------------

fn item(id: &str, category: Category, bytes: u64) -> CleanupItem {
    CleanupItem {
        id: id.to_string(),
        category,
        title: "fixture".to_string(),
        why_recommended: "fixture reasoning".to_string(),
        recoverable_bytes: bytes,
        tier: SafetyTier::Safe,
        what_stays_untouched: "everything else".to_string(),
        consequence: "fixture consequence".to_string(),
        destination: Some("OS Trash / app quarantine".to_string()),
        undo: Some("restore from Trash/quarantine".to_string()),
        action: ActionKind::MoveToTrash,
    }
}

fn plan(id: &str, items: Vec<CleanupItem>) -> CleanupPlan {
    let total = items
        .iter()
        .fold(0u64, |sum, item| sum.saturating_add(item.recoverable_bytes));
    CleanupPlan {
        id: id.to_string(),
        items,
        total_recoverable_bytes: total,
        partial: false,
        off_limits_excluded: Vec::new(),
    }
}

fn entry(
    path: &str,
    kind: ObservedKind,
    size: Option<u64>,
    category: Option<&str>,
    observation_error: Option<&str>,
) -> ObservedEntry {
    ObservedEntry {
        path: PathBuf::from(path),
        kind,
        size,
        object: None,
        modified: None,
        classification: category.map(|code| ClassificationRef {
            category: code.to_string(),
            subcategory: None,
        }),
        content_sha256: None,
        observation_error: observation_error.map(str::to_string),
    }
}

/// A fully observed regular file in `category`.
fn file(path: &str, size: u64, category: Category) -> ObservedEntry {
    entry(
        path,
        ObservedKind::File,
        Some(size),
        Some(category.code()),
        None,
    )
}

/// A fully observed directory (snapshot directories carry no own size).
fn dir(path: &str, category: Category) -> ObservedEntry {
    entry(path, ObservedKind::Dir, None, Some(category.code()), None)
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("run".to_string()),
        entries,
    }
}

fn defaults() -> ResolveOptions {
    ResolveOptions::default()
}

fn paths(targets: &[PlanTarget]) -> Vec<&std::path::Path> {
    targets.iter().map(|target| target.path.as_path()).collect()
}

// --- rule 1: exact category match ------------------------------------------

#[test]
fn only_entries_with_the_item_category_are_resolved() {
    let snapshot = snapshot(vec![
        file("/cache/a", 10, Category::Cache),
        file("/logs/b", 20, Category::Logs),
        file("/docs/c", 30, Category::Documents),
    ]);
    let plan = plan(
        "plan:categories",
        vec![
            item("CACHE:10", Category::Cache, 10),
            item("LOGS:20", Category::Logs, 20),
        ],
    );

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("both items resolve");

    assert_eq!(resolved.items.len(), 2);
    assert_eq!(
        paths(&resolved.items[0].targets),
        vec![std::path::Path::new("/cache/a")]
    );
    assert_eq!(
        paths(&resolved.items[1].targets),
        vec![std::path::Path::new("/logs/b")]
    );
    assert_eq!(
        resolved.unresolved_entries, 0,
        "entries that match no plan item are not 'skipped targets'"
    );
}

#[test]
fn an_unrecognized_category_code_matches_nothing() {
    // `"cache"` is not the stable code (`"CACHE"`), and an unknown code must
    // never be guessed into a category — not even `Unknown`.
    let snapshot = snapshot(vec![
        file("/cache/a", 10, Category::Cache),
        entry(
            "/cache/odd",
            ObservedKind::File,
            Some(99),
            Some("cache"),
            None,
        ),
        entry(
            "/cache/future",
            ObservedKind::File,
            Some(77),
            Some("SOME_FUTURE_CATEGORY"),
            None,
        ),
    ]);
    let plan = plan("plan:decode", vec![item("CACHE:10", Category::Cache, 10)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("the valid entry resolves");

    assert_eq!(
        paths(&resolved.items[0].targets),
        vec![std::path::Path::new("/cache/a")],
        "only the exact stable code matches"
    );
    assert_eq!(resolved.target_count, 1);
    assert_eq!(
        resolved.total_bytes, 10,
        "the 176 unrecognized bytes are not pinned"
    );
}

// --- rules 2-4: what can never be a target ---------------------------------

#[test]
fn entries_with_observation_errors_are_never_targets_and_are_counted() {
    let snapshot = snapshot(vec![
        file("/cache/good", 10, Category::Cache),
        entry(
            "/cache/denied",
            ObservedKind::File,
            Some(90),
            Some(Category::Cache.code()),
            Some("PERMISSION_DENIED"),
        ),
    ]);
    let plan = plan("plan:errors", vec![item("CACHE:100", Category::Cache, 100)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("one good target remains");

    assert_eq!(
        paths(&resolved.items[0].targets),
        vec![std::path::Path::new("/cache/good")]
    );
    assert_eq!(resolved.unresolved_entries, 1);
    assert_eq!(
        resolved.unmapped_bytes, 90,
        "the unobservable bytes stay visible"
    );
}

#[test]
fn entries_without_a_size_are_never_targets_and_are_counted() {
    let snapshot = snapshot(vec![
        file("/cache/good", 10, Category::Cache),
        entry(
            "/cache/unsized",
            ObservedKind::File,
            None,
            Some(Category::Cache.code()),
            None,
        ),
    ]);
    let plan = plan("plan:sizes", vec![item("CACHE:10", Category::Cache, 10)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("one good target remains");

    assert_eq!(resolved.target_count, 1);
    assert_eq!(resolved.unresolved_entries, 1);
    assert_eq!(resolved.total_bytes, 10);
}

#[test]
fn links_and_other_kinds_are_never_targets_and_are_counted() {
    let snapshot = snapshot(vec![
        file("/cache/good", 10, Category::Cache),
        // Even with a size attached, a link is a different operation.
        entry(
            "/cache/link",
            ObservedKind::Link,
            Some(50),
            Some(Category::Cache.code()),
            None,
        ),
        entry(
            "/cache/special",
            ObservedKind::Other,
            None,
            Some(Category::Cache.code()),
            None,
        ),
    ]);
    let plan = plan("plan:kinds", vec![item("CACHE:10", Category::Cache, 10)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("one good target remains");

    assert_eq!(
        paths(&resolved.items[0].targets),
        vec![std::path::Path::new("/cache/good")]
    );
    assert_eq!(resolved.unresolved_entries, 2);
}

// --- rule 5: a directory absorbs its same-category descendants -------------

#[test]
fn a_directory_target_absorbs_its_same_category_descendants() {
    let snapshot = snapshot(vec![
        dir("/cache", Category::Cache),
        dir("/cache/sub", Category::Cache),
        file("/cache/a", 10, Category::Cache),
        file("/cache/sub/b", 20, Category::Cache),
    ]);
    let plan = plan("plan:dedupe", vec![item("CACHE:30", Category::Cache, 30)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("the directory resolves");

    assert_eq!(
        resolved.items[0].targets,
        vec![PlanTarget {
            path: PathBuf::from("/cache"),
            bytes: 30,
            kind: TargetKind::Directory,
        }],
        "the shallowest directory is the only target"
    );
    assert_eq!(resolved.target_count, 1);
    assert_eq!(
        resolved.unresolved_entries, 0,
        "absorbed descendants are covered by their parent, not errors"
    );
}

#[test]
fn directory_deduplication_is_per_category() {
    // A CACHE directory contains a LOGS file. Both plan items keep their own
    // target: neither category may silently absorb the other's bytes. The
    // action layer's membership check still binds both paths, so this is a
    // documented residual (the second move fails and rolls back), not a
    // silent widening.
    let snapshot = snapshot(vec![
        dir("/cache", Category::Cache),
        file("/cache/app.log", 5, Category::Logs),
    ]);
    let plan = plan(
        "plan:cross-category",
        vec![
            item("CACHE:0", Category::Cache, 0),
            item("LOGS:5", Category::Logs, 5),
        ],
    );

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("both items resolve");

    assert_eq!(resolved.items[0].targets.len(), 1);
    assert_eq!(resolved.items[0].targets[0].path, PathBuf::from("/cache"));
    assert_eq!(
        paths(&resolved.items[1].targets),
        vec![std::path::Path::new("/cache/app.log")],
        "the LOGS item keeps its own path"
    );
    assert_eq!(resolved.target_count, 2);
}

// --- rule 6: directory bytes ------------------------------------------------

#[test]
fn directory_bytes_are_the_saturating_sum_of_regular_files_beneath() {
    let snapshot = snapshot(vec![
        dir("/cache", Category::Cache),
        file("/cache/a", 3, Category::Cache),
        dir("/cache/sub", Category::Cache),
        file("/cache/sub/b", 4, Category::Cache),
        file("/cache/sub/deep/c", 5, Category::Cache),
        // Not beneath `/cache` as a path component.
        file("/cache-extra/d", 100, Category::Cache),
    ]);
    let plan = plan("plan:bytes", vec![item("CACHE:112", Category::Cache, 112)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("targets resolve");

    let cache = &resolved.items[0].targets;
    assert_eq!(cache.len(), 2, "the directory and the sibling file");
    assert_eq!(cache[0].path, PathBuf::from("/cache"));
    assert_eq!(cache[0].bytes, 12, "3 + 4 + 5 bytes beneath the directory");
    assert_eq!(cache[1].path, PathBuf::from("/cache-extra/d"));
    assert_eq!(resolved.total_bytes, 112);
}

#[test]
fn directory_bytes_ignore_unobserved_files() {
    // The lower bound never trusts a file the scan could not fully observe,
    // and never invents a size for an unsized one.
    let snapshot = snapshot(vec![
        dir("/cache", Category::Cache),
        file("/cache/a", 3, Category::Cache),
        entry(
            "/cache/denied",
            ObservedKind::File,
            Some(9),
            Some(Category::Cache.code()),
            Some("PERMISSION_DENIED"),
        ),
        entry(
            "/cache/unsized",
            ObservedKind::File,
            None,
            Some(Category::Cache.code()),
            None,
        ),
    ]);
    let plan = plan(
        "plan:lower-bound",
        vec![item("CACHE:12", Category::Cache, 12)],
    );

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("the directory resolves");

    assert_eq!(resolved.items[0].targets[0].bytes, 3);
    assert_eq!(
        resolved.unresolved_entries, 2,
        "the skipped descendants are still entry-level uncertainty"
    );
    assert_eq!(
        resolved.unmapped_bytes, 9,
        "the 12-byte claim minus the 3-byte pinned lower bound"
    );
}

// --- rules 7-8: totals and unmapped bytes -----------------------------------

#[test]
fn total_bytes_saturates_instead_of_wrapping() {
    let snapshot = snapshot(vec![
        file("/cache/huge", u64::MAX, Category::Cache),
        file("/cache/more", 10, Category::Cache),
    ]);
    let plan = plan(
        "plan:saturate",
        vec![item("CACHE:MAX", Category::Cache, u64::MAX)],
    );

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("targets resolve");

    assert_eq!(
        resolved.total_bytes,
        u64::MAX,
        "saturating sum, never wrapping"
    );
    assert_eq!(resolved.unmapped_bytes, 0);
}

#[test]
fn unmapped_bytes_is_plan_claim_minus_resolved_total() {
    let snapshot = snapshot(vec![file("/cache/a", 400, Category::Cache)]);
    let plan = plan(
        "plan:unmapped",
        vec![item("CACHE:1000", Category::Cache, 1_000)],
    );

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("targets resolve");

    assert_eq!(resolved.total_bytes, 400);
    assert_eq!(resolved.unmapped_bytes, 600);
}

#[test]
fn unmapped_bytes_saturates_when_resolution_exceeds_the_claim() {
    let snapshot = snapshot(vec![file("/cache/a", 400, Category::Cache)]);
    let plan = plan("plan:over", vec![item("CACHE:100", Category::Cache, 100)]);

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("targets resolve");

    assert_eq!(resolved.unmapped_bytes, 0, "saturating subtraction");
    assert_eq!(resolved.total_bytes, 400);
}

// --- rule 9: the hard budget ------------------------------------------------

#[test]
fn the_target_budget_is_a_hard_limit() {
    let snapshot = snapshot(vec![
        file("/cache/a", 1, Category::Cache),
        file("/cache/b", 2, Category::Cache),
    ]);
    let plan = plan("plan:budget", vec![item("CACHE:3", Category::Cache, 3)]);

    let error = resolve_plan(&plan, &snapshot, &ResolveOptions { max_targets: 1 })
        .expect_err("two targets exceed a budget of one");

    assert_eq!(
        error,
        ResolveError::TargetBudgetExceeded {
            max_targets: 1,
            requested: 2,
        },
        "the budget fails the whole resolution instead of truncating"
    );
}

#[test]
fn the_target_budget_counts_emitted_targets_not_absorbed_descendants() {
    let snapshot = snapshot(vec![
        dir("/cache", Category::Cache),
        file("/cache/a", 1, Category::Cache),
        file("/cache/b", 2, Category::Cache),
        file("/cache/c", 3, Category::Cache),
        file("/other", 4, Category::Cache),
    ]);
    let plan = plan(
        "plan:budget-dir",
        vec![item("CACHE:10", Category::Cache, 10)],
    );

    let resolved = resolve_plan(&plan, &snapshot, &ResolveOptions { max_targets: 2 })
        .expect("five candidates collapse to two targets");
    assert_eq!(resolved.target_count, 2);

    let error = resolve_plan(&plan, &snapshot, &ResolveOptions { max_targets: 1 })
        .expect_err("two emitted targets exceed a budget of one");
    assert!(matches!(
        error,
        ResolveError::TargetBudgetExceeded {
            max_targets: 1,
            requested: 2
        }
    ));
}

// --- rule 10: empty plan / item that cannot be pinned -----------------------

#[test]
fn an_empty_plan_cannot_be_resolved() {
    let error = resolve_plan(
        &plan("plan:empty", Vec::new()),
        &snapshot(Vec::new()),
        &defaults(),
    )
    .expect_err("nothing to resolve");

    assert_eq!(error, ResolveError::EmptyPlan);
}

#[test]
fn an_item_with_no_concrete_target_is_refused() {
    let snapshot = snapshot(vec![file("/logs/a", 5, Category::Logs)]);
    let plan = plan("plan:missing", vec![item("CACHE:10", Category::Cache, 10)]);

    let error =
        resolve_plan(&plan, &snapshot, &defaults()).expect_err("the Cache item has no cache entry");

    assert_eq!(
        error,
        ResolveError::PlanItemMissing {
            item_id: "CACHE:10".to_string(),
        },
        "an item the plan claimed bytes for must not silently vanish"
    );
}

// --- rule 11: order and determinism ----------------------------------------

#[test]
fn items_stay_in_plan_order_and_targets_are_sorted_by_path_bytes() {
    let snapshot = snapshot(vec![
        file("/z", 1, Category::Cache),
        file("/logs/z", 2, Category::Logs),
        file("/a", 3, Category::Cache),
    ]);
    let plan = plan(
        "plan:order",
        vec![
            item("LOGS:2", Category::Logs, 2),
            item("CACHE:4", Category::Cache, 4),
        ],
    );

    let resolved = resolve_plan(&plan, &snapshot, &defaults()).expect("targets resolve");

    let ids: Vec<&str> = resolved
        .items
        .iter()
        .map(|item| item.item_id.as_str())
        .collect();
    assert_eq!(ids, vec!["LOGS:2", "CACHE:4"], "plan order is preserved");
    assert_eq!(
        paths(&resolved.items[1].targets),
        vec![std::path::Path::new("/a"), std::path::Path::new("/z")],
        "targets are ordered by path bytes ascending"
    );
    assert_eq!(resolved.items[1].category, Category::Cache);
    assert_eq!(resolved.items[1].tier, SafetyTier::Safe);
}

#[test]
fn resolution_is_deterministic_under_input_permutation() {
    let forward = snapshot(vec![
        file("/cache/a", 10, Category::Cache),
        file("/cache/b", 20, Category::Cache),
        file("/logs/x", 30, Category::Logs),
        file("/logs/y", 40, Category::Logs),
    ]);
    // Same facts, reversed: `Snapshot` normally arrives sorted, but the
    // resolver must not rely on that convention.
    let mut reversed_entries = forward.entries.clone();
    reversed_entries.reverse();
    let reversed = snapshot(reversed_entries);

    let plan = plan(
        "plan:determinism",
        vec![
            item("CACHE:30", Category::Cache, 30),
            item("LOGS:70", Category::Logs, 70),
        ],
    );

    let first = resolve_plan(&plan, &forward, &defaults()).expect("forward resolves");
    let second = resolve_plan(&plan, &reversed, &defaults()).expect("reversed resolves");

    assert_eq!(
        first, second,
        "input order must not change the resolved plan"
    );
}

// --- the resolved plan's shape ---------------------------------------------

#[test]
fn the_resolved_plan_keeps_the_source_plan_identity() {
    let snapshot = snapshot(vec![file("/cache/a", 10, Category::Cache)]);
    let plan = plan("plan:identity", vec![item("CACHE:10", Category::Cache, 10)]);

    let resolved: ResolvedPlan =
        resolve_plan(&plan, &snapshot, &defaults()).expect("the item resolves");

    assert_eq!(resolved.plan_id, plan.id);
    assert_eq!(resolved.items[0].item_id, plan.items[0].id);
    assert_eq!(resolved.items[0].category, plan.items[0].category);
    assert_eq!(resolved.items[0].tier, plan.items[0].tier);
    assert_eq!(resolved.target_count, 1);
}
