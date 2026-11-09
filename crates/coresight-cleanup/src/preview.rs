//! The preview: the exact statement the user confirms before any
//! reversible action is taken (docs/SECURITY_AND_SAFETY.md preview
//! contract).
//!
//! A preview is **complete** only when every item names its destination
//! and its undo path. The validator blocks any plan whose preview is
//! incomplete — there is no "confirm anyway" path.

use serde::{Deserialize, Serialize};

use crate::{CleanupItem, CleanupPlan};

/// One line of the preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub title: String,
    pub bytes: u64,
    pub destination: String,
    pub undo: String,
}

/// The user-facing preview of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPreview {
    pub plan_id: String,
    pub items: Vec<PreviewItem>,
    pub total_bytes: u64,
    pub what_stays_untouched: Vec<String>,
    pub destination: String,
    pub how_to_undo: String,
    pub consequence: String,
    /// Whether every item named its destination and undo path.
    pub complete: bool,
}

impl CleanupPreview {
    /// Build the preview from a plan. `complete` is false when any item
    /// is missing its destination or undo — the validator refuses it.
    pub fn from_plan(plan: &CleanupPlan) -> Self {
        let mut items = Vec::new();
        let mut complete = true;
        for item in &plan.items {
            let destination = item.destination.clone();
            let undo = item.undo.clone();
            if destination.is_none() || undo.is_none() {
                complete = false;
            }
            items.push(PreviewItem {
                title: item.title.clone(),
                bytes: item.recoverable_bytes,
                destination: destination.unwrap_or_else(|| "UNSET".to_string()),
                undo: undo.unwrap_or_else(|| "UNSET".to_string()),
            });
        }
        let what_stays_untouched = plan
            .items
            .iter()
            .map(|it: &CleanupItem| it.what_stays_untouched.clone())
            .collect::<Vec<_>>();
        let consequence = if plan.items.is_empty() {
            "Nothing will be moved.".to_string()
        } else {
            plan.items
                .iter()
                .map(|it: &CleanupItem| it.consequence.clone())
                .collect::<Vec<_>>()
                .join(" ")
        };
        CleanupPreview {
            plan_id: plan.id.clone(),
            items,
            total_bytes: plan.total_recoverable_bytes,
            what_stays_untouched,
            destination: "OS Trash / app quarantine".to_string(),
            how_to_undo: "Restore from Trash/quarantine".to_string(),
            consequence,
            complete,
        }
    }
}
