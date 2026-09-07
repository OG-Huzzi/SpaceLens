//! Versioned IPC contract types (`spacelens.v1.*`).
//!
//! Mirrors `docs/API_CONTRACTS.md`. The frontend must never need filesystem
//! knowledge: sizes are bytes, categories carry no paths.

use serde::{Deserialize, Serialize};

/// Contract version. Bumped only with a documented migration.
pub const CONTRACT_VERSION: &str = "v1";

/// Human-readable storage category. No filesystem paths at this layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub bytes: u64,
    pub share_of_parent: f64,
    pub delta_since_last: Option<i64>,
    pub item_count: u64,
}

/// Safety tier for a cleanup opportunity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafetyTier {
    Safe,
    Review,
}

/// A cleanup recommendation with its reasoning attached.
/// Advisory only — cannot cause deletion by itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opportunity {
    pub id: String,
    pub tier: SafetyTier,
    pub title: String,
    pub what_it_is: String,
    pub why_recommended: String,
    pub bytes: u64,
    pub recoverable_bytes: u64,
    pub stays_untouched: String,
    pub consequence: String,
}

/// Typed error envelope returned by every command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_version_is_v1() {
        assert_eq!(CONTRACT_VERSION, "v1");
    }

    #[test]
    fn category_serializes_camel_case_without_paths() {
        let cat = Category {
            id: "games".to_string(),
            name: "Games".to_string(),
            bytes: 312_000_000_000,
            share_of_parent: 0.42,
            delta_since_last: Some(14_000_000_000),
            item_count: 128,
        };
        let json = serde_json::to_string(&cat).unwrap();
        assert!(json.contains("\"shareOfParent\""));
        assert!(json.contains("\"deltaSinceLast\""));
        assert!(!json.contains("path"));
    }

    #[test]
    fn opportunity_requires_reasoning_fields() {
        let json = r#"{
            "id": "opp-1", "tier": "review", "title": "Old downloads",
            "whatItIs": "Files in Downloads older than a year",
            "whyRecommended": "Untouched for 400+ days",
            "bytes": 18000000000, "recoverableBytes": 18000000000,
            "staysUntouched": "Everything newer than a year",
            "consequence": "Items move to the Recycle Bin; restore within 30 days"
        }"#;
        let opp: Opportunity = serde_json::from_str(json).unwrap();
        assert_eq!(opp.tier, SafetyTier::Review);
        assert!(!opp.consequence.is_empty());
    }
}
