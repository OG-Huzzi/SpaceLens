//! Shared fixtures for the `coresight-actions` integration tests.
//!
//! Every filesystem test works inside its own `tempfile::tempdir()` and calls
//! [`assert_inside`] on every path it creates, reads, or moves, so a test that
//! accidentally resolved a path outside its private directory fails loudly
//! instead of touching a real home directory.
//!
//! Test crates are exempt from the production rules this crate enforces on
//! itself (`unwrap`/`expect` are used freely here, on purpose: a fixture that
//! silently continued after a failed setup would produce a green test that
//! proved nothing).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use coresight_capabilities::safety::ExecutionPolicy;
use coresight_cleanup::{
    ActionKind, CleanupItem, CleanupPlan, PlanTarget, ResolvedItem, ResolvedPlan, SafetyTier,
    TargetKind,
};

use coresight_actions::{authorize, AuthorizedPlan, ConfirmedPreview};

/// A policy that authorizes reversible actions and nothing else.
///
/// This is the narrowest floor that can admit a `MoveToTrash` plan: the
/// classifier marks `Cache`, `TemporaryData`, and `Logs` items as
/// permission-insensitive, so `permission_sensitive: false` is enough for the
/// fixtures below.
pub fn reversible_policy() -> ExecutionPolicy {
    ExecutionPolicy {
        read_only: true,
        reversible: true,
        destructive: false,
        privileged: false,
        permission_sensitive: false,
    }
}

/// A deliberately over-broad policy: it even authorizes destructive effects.
///
/// Used to prove that `authorize` refuses a `Delete` item on its own merits,
/// not merely because the policy happened to be narrow.
pub fn destructive_policy() -> ExecutionPolicy {
    ExecutionPolicy {
        destructive: true,
        ..reversible_policy()
    }
}

/// One plan item with complete reasoning and a `MoveToTrash` action by
/// default.
///
/// `coresight_classifier::Category` is not a direct dependency of this
/// package's tests (the `Cargo.toml` is fixed), so the value is obtained from
/// the type's own serde representation: the field type is inferred from
/// `CleanupItem::category` and never named in source. `Category` is
/// `SCREAMING_SNAKE_CASE` on the wire, so `"CACHE"` is `Category::Cache`.
pub fn cleanup_item(id: &str, bytes: u64, action: ActionKind) -> CleanupItem {
    CleanupItem {
        id: id.to_string(),
        category: serde_json::from_str("\"CACHE\"").expect("Category::Cache is deserializable"),
        title: "Cache".to_string(),
        why_recommended: "regenerable cache data".to_string(),
        recoverable_bytes: bytes,
        tier: SafetyTier::Safe,
        what_stays_untouched: "documents and user data".to_string(),
        consequence: "regenerated on next use".to_string(),
        destination: Some("directory-based trash".to_string()),
        undo: Some("move it back from the trash directory".to_string()),
        action,
    }
}

/// A plan whose byte total is exactly the sum of its items, as the validator
/// requires.
pub fn cleanup_plan(id: &str, items: Vec<CleanupItem>) -> CleanupPlan {
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

/// A resolved plan that pins `plan`'s items to `per_item` targets, in order.
///
/// Every target is a regular file with the stated logical size. The declared
/// `target_count` and `total_bytes` are derived from the target lists they
/// describe, exactly as `resolve_plan` would derive them, so the fixture is a
/// legitimate witness for `authorize`.
pub fn resolved_plan(plan: &CleanupPlan, per_item: Vec<Vec<(PathBuf, u64)>>) -> ResolvedPlan {
    assert_eq!(
        per_item.len(),
        plan.items.len(),
        "fixture bug: one target list per plan item"
    );
    let mut items = Vec::new();
    let mut total_bytes = 0u64;
    let mut target_count = 0usize;
    for (item, targets) in plan.items.iter().zip(per_item) {
        let targets: Vec<PlanTarget> = targets
            .into_iter()
            .map(|(path, bytes)| PlanTarget {
                path,
                bytes,
                kind: TargetKind::File,
            })
            .collect();
        for target in &targets {
            total_bytes = total_bytes.saturating_add(target.bytes);
            target_count += 1;
        }
        items.push(ResolvedItem {
            item_id: item.id.clone(),
            category: item.category,
            tier: item.tier,
            targets,
        });
    }
    ResolvedPlan {
        plan_id: plan.id.clone(),
        items,
        total_bytes,
        target_count,
        unmapped_bytes: plan.total_recoverable_bytes.saturating_sub(total_bytes),
        unresolved_entries: 0,
    }
}

/// A token for a one-item reversible plan whose resolved targets are exactly
/// `targets`.
///
/// The plan's claimed bytes are the sum of the target bytes, so the fixture
/// has no unmapped bytes and the preview numbers are the plan's numbers.
pub fn authorized_plan_for(targets: Vec<(PathBuf, u64)>) -> AuthorizedPlan {
    let total = targets
        .iter()
        .fold(0u64, |sum, (_, bytes)| sum.saturating_add(*bytes));
    let plan = cleanup_plan(
        "plan:exec",
        vec![cleanup_item("CACHE:1", total, ActionKind::MoveToTrash)],
    );
    let resolved = resolved_plan(&plan, vec![targets]);
    authorize(plan, resolved, reversible_policy())
        .expect("the fixture plan is reversible and complete")
}

/// The confirmation a UI would build from the resolved preview of
/// `authorized`.
pub fn confirmation_for(authorized: &AuthorizedPlan) -> ConfirmedPreview {
    ConfirmedPreview::new(
        authorized.plan_id(),
        authorized.resolved_total_bytes(),
        authorized.resolved_target_count(),
    )
}

/// Fail if `path` does not resolve inside `root`.
///
/// A path that does not exist yet is resolved through its nearest existing
/// parent, so a destination that was never created can still be checked.
pub fn assert_inside(root: &Path, path: &Path) {
    let canonical_root = root
        .canonicalize()
        .expect("the test's own tempdir must exist");
    let resolved = resolve(path);
    assert!(
        resolved.starts_with(&canonical_root),
        "test path {} escaped its tempdir {}",
        resolved.display(),
        canonical_root.display()
    );
}

/// Absolute, symlink-resolved form of a path that may not exist yet.
///
/// Nonexistent trailing components are kept and appended to the nearest
/// existing ancestor, so a path like `<temp>/absent-dir/missing.bin` can still
/// be proven inside `<temp>` without creating anything.
fn resolve(path: &Path) -> PathBuf {
    let mut unresolved: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut resolved = canonical;
            for component in unresolved.iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                unresolved.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}
