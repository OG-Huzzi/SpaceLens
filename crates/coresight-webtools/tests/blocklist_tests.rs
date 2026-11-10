//! Integration tests for the web-blocker planning half of
//! `coresight-webtools`.
//!
//! All rules are synthetic. Nothing is written to a hosts file: these
//! tests prove validation, normalization, whitelist precedence, exact
//! duplicate accounting, deterministic ordering, truncation reporting,
//! and that the unblock plan names exactly the planned entries.

use coresight_webtools::{
    plan_block_list, plan_unblock, BlockListOptions, BlockListPlan, BlockRule, DomainCategory,
    RuleSource,
};

fn rule(domain: &str, category: DomainCategory) -> BlockRule {
    BlockRule {
        domain: domain.to_string(),
        category,
        source: RuleSource::UserDefined,
        reason: "test rule".to_string(),
    }
}

fn reason_for<'a>(plan: &'a BlockListPlan, domain: &str) -> &'a str {
    plan.invalid_removed
        .iter()
        .find(|rejected| rejected.domain == domain)
        .map(|rejected| rejected.reason.as_str())
        .expect("domain must be reported as rejected")
}

fn domains(plan: &BlockListPlan) -> Vec<&str> {
    plan.entries
        .iter()
        .map(|entry| entry.domain.as_str())
        .collect()
}

#[test]
fn block_plan_uses_both_loopback_addresses() {
    let plan = plan_block_list(
        &[rule("ads.example.com", DomainCategory::Advertising)],
        &BlockListOptions::default(),
    );
    assert_eq!(plan.total, 1);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].domain, "ads.example.com");
    assert_eq!(plan.entries[0].ipv4, "127.0.0.1");
    assert_eq!(plan.entries[0].ipv6, "::1");
    assert_eq!(plan.entries[0].category, DomainCategory::Advertising);
}

#[test]
fn uppercase_domains_are_normalized_to_lowercase() {
    let plan = plan_block_list(
        &[rule("ADS.Example.COM", DomainCategory::Advertising)],
        &BlockListOptions::default(),
    );
    assert_eq!(plan.total, 1);
    assert_eq!(domains(&plan), vec!["ads.example.com"]);
    assert!(plan.invalid_removed.is_empty());
}

#[test]
fn invalid_domains_are_rejected_with_reasons_not_dropped() {
    let rules = vec![
        rule("", DomainCategory::Custom),
        rule("   ", DomainCategory::Custom),
        rule("ads.example.com/path", DomainCategory::Custom),
        rule("ads.example.com:8080", DomainCategory::Custom),
        rule("*.ads.example.com", DomainCategory::Custom),
        rule("nodot", DomainCategory::Custom),
        rule("double..dot.com", DomainCategory::Custom),
        rule("-leading.example.com", DomainCategory::Custom),
        rule("trailing-.example.com", DomainCategory::Custom),
        rule("bad_label.example.com", DomainCategory::Custom),
    ];
    let plan = plan_block_list(&rules, &BlockListOptions::default());

    assert_eq!(plan.total, 0);
    assert_eq!(plan.entries.len(), 0);
    assert_eq!(
        plan.invalid_removed.len(),
        rules.len(),
        "every invalid rule must be reported"
    );
    assert!(reason_for(&plan, "").contains("empty"));
    assert!(reason_for(&plan, "   ").contains("whitespace"));
    assert!(reason_for(&plan, "ads.example.com/path").contains('/'));
    assert!(reason_for(&plan, "ads.example.com:8080").contains(':'));
    assert!(reason_for(&plan, "*.ads.example.com").contains('*'));
    assert!(reason_for(&plan, "nodot").contains("dot"));
    assert!(reason_for(&plan, "double..dot.com").contains("empty label"));
    assert!(reason_for(&plan, "-leading.example.com").contains("hyphen"));
    assert!(reason_for(&plan, "trailing-.example.com").contains("hyphen"));
    assert!(reason_for(&plan, "bad_label.example.com").contains("invalid character"));
    assert!(plan
        .invalid_removed
        .iter()
        .all(|rejected| !rejected.reason.is_empty()));
}

#[test]
fn domain_length_boundaries_are_enforced() {
    let label61 = "a".repeat(61);
    let label62 = "a".repeat(62);
    let label63 = "a".repeat(63);
    let label64 = "a".repeat(64);

    let valid_label = format!("{label63}.com");
    let invalid_label = format!("{label64}.com");
    let valid_max = format!("{label63}.{label63}.{label63}.{label61}");
    let invalid_max = format!("{label63}.{label63}.{label63}.{label62}");
    assert_eq!(valid_max.len(), 253);
    assert_eq!(invalid_max.len(), 254);

    let rules = vec![
        rule(&valid_label, DomainCategory::Custom),
        rule(&invalid_label, DomainCategory::Custom),
        rule(&valid_max, DomainCategory::Custom),
        rule(&invalid_max, DomainCategory::Custom),
    ];
    let plan = plan_block_list(&rules, &BlockListOptions::default());

    assert_eq!(plan.total, 2);
    assert_eq!(plan.invalid_removed.len(), 2);
    assert!(reason_for(&plan, &invalid_label).contains("63"));
    assert!(reason_for(&plan, &invalid_max).contains("253"));
}

#[test]
fn whitelist_wins_over_explicit_block_and_is_reported() {
    let rules = vec![
        rule("tracker.example.com", DomainCategory::Tracking),
        rule("ads.example.com", DomainCategory::Advertising),
    ];
    let options = BlockListOptions {
        whitelist: vec!["TRACKER.Example.com".to_string()],
        include_categories: Vec::new(),
        max_entries: 0,
    };
    let plan = plan_block_list(&rules, &options);

    assert_eq!(domains(&plan), vec!["ads.example.com"]);
    assert_eq!(plan.whitelisted, vec!["tracker.example.com"]);
    assert_eq!(plan.total, 1);
    assert!(plan.invalid_removed.is_empty());
}

#[test]
fn duplicate_domains_collapse_with_exact_count() {
    let rules = vec![
        rule("ads.example.com", DomainCategory::Advertising),
        rule("Ads.Example.com", DomainCategory::Advertising),
        rule("ads.example.com", DomainCategory::Advertising),
        rule("other.example.com", DomainCategory::Tracking),
    ];
    let plan = plan_block_list(&rules, &BlockListOptions::default());

    assert_eq!(plan.total, 2);
    assert_eq!(plan.duplicates_removed, 2);
    assert_eq!(domains(&plan), vec!["ads.example.com", "other.example.com"]);
}

#[test]
fn duplicate_categories_keep_most_protective_label_regardless_of_order() {
    let forward = vec![
        rule("evil.example.com", DomainCategory::Custom),
        rule("evil.example.com", DomainCategory::Malware),
    ];
    let backward = vec![
        rule("evil.example.com", DomainCategory::Malware),
        rule("evil.example.com", DomainCategory::Custom),
    ];
    let plan_forward = plan_block_list(&forward, &BlockListOptions::default());
    let plan_backward = plan_block_list(&backward, &BlockListOptions::default());

    assert_eq!(plan_forward, plan_backward);
    assert_eq!(plan_forward.entries[0].category, DomainCategory::Malware);
    assert_eq!(plan_forward.duplicates_removed, 1);
}

#[test]
fn deterministic_across_input_permutations() {
    let rules = vec![
        rule("zeta.example.com", DomainCategory::Tracking),
        rule("bad_domain", DomainCategory::Custom),
        rule("alpha.example.com", DomainCategory::Advertising),
        rule("alpha.example.com", DomainCategory::Custom),
        rule("safe.example.com", DomainCategory::SocialMedia),
    ];
    let reversed: Vec<BlockRule> = rules.iter().rev().cloned().collect();
    let options_forward = BlockListOptions {
        whitelist: vec![
            "safe.example.com".to_string(),
            "never.example.com".to_string(),
        ],
        include_categories: Vec::new(),
        max_entries: 0,
    };
    let options_reversed = BlockListOptions {
        whitelist: vec![
            "never.example.com".to_string(),
            "safe.example.com".to_string(),
        ],
        include_categories: Vec::new(),
        max_entries: 0,
    };

    let first = plan_block_list(&rules, &options_forward);
    let second = plan_block_list(&reversed, &options_reversed);
    assert_eq!(first, second);
}

#[test]
fn category_filter_selects_only_requested_categories() {
    let rules = vec![
        rule("ads.example.com", DomainCategory::Advertising),
        rule("tracker.example.com", DomainCategory::Tracking),
    ];
    let defaults = plan_block_list(&rules, &BlockListOptions::default());
    assert_eq!(defaults.total, 2, "empty include_categories means all");

    let tracking_only = BlockListOptions {
        whitelist: Vec::new(),
        include_categories: vec![DomainCategory::Tracking],
        max_entries: 0,
    };
    let filtered = plan_block_list(&rules, &tracking_only);
    assert_eq!(domains(&filtered), vec!["tracker.example.com"]);
    assert!(filtered.invalid_removed.is_empty());
}

#[test]
fn max_entries_caps_and_reports_truncation() {
    let rules = vec![
        rule("c.example.com", DomainCategory::Custom),
        rule("a.example.com", DomainCategory::Custom),
        rule("b.example.com", DomainCategory::Custom),
    ];
    let options = BlockListOptions {
        whitelist: Vec::new(),
        include_categories: Vec::new(),
        max_entries: 2,
    };
    let plan = plan_block_list(&rules, &options);

    assert_eq!(plan.total, 2);
    assert_eq!(plan.entries.len(), 2);
    assert_eq!(domains(&plan), vec!["a.example.com", "b.example.com"]);
    assert_eq!(plan.truncated, 1, "the omitted entry must be counted");
}

#[test]
fn zero_max_entries_means_unlimited() {
    let rules = vec![
        rule("a.example.com", DomainCategory::Custom),
        rule("b.example.com", DomainCategory::Custom),
        rule("c.example.com", DomainCategory::Custom),
    ];
    let plan = plan_block_list(&rules, &BlockListOptions::default());
    assert_eq!(plan.total, 3);
    assert_eq!(plan.truncated, 0);
}

#[test]
fn invalid_whitelist_entries_are_reported() {
    let options = BlockListOptions {
        whitelist: vec!["bad_domain".to_string(), "ok.example.com".to_string()],
        include_categories: Vec::new(),
        max_entries: 0,
    };
    let plan = plan_block_list(&[], &options);

    assert_eq!(plan.total, 0);
    assert_eq!(plan.invalid_removed.len(), 1);
    assert_eq!(plan.invalid_removed[0].domain, "bad_domain");
    assert!(plan.invalid_removed[0]
        .reason
        .contains("invalid whitelist entry"));
}

#[test]
fn every_input_rule_is_accounted_for() {
    let rules = vec![
        rule("good.example.com", DomainCategory::Advertising),
        rule("GOOD.example.com", DomainCategory::Custom),
        rule("bad_domain", DomainCategory::Custom),
        rule("safe.example.com", DomainCategory::Tracking),
    ];
    let options = BlockListOptions {
        whitelist: vec!["safe.example.com".to_string()],
        include_categories: Vec::new(),
        max_entries: 0,
    };
    let plan = plan_block_list(&rules, &options);

    let accounted = plan
        .total
        .saturating_add(plan.duplicates_removed)
        .saturating_add(plan.invalid_removed.len())
        .saturating_add(plan.whitelisted.len())
        .saturating_add(plan.truncated);
    assert_eq!(accounted, rules.len());
    assert_eq!(plan.total, 1);
    assert_eq!(plan.duplicates_removed, 1);
    assert_eq!(plan.invalid_removed.len(), 1);
    assert_eq!(plan.whitelisted.len(), 1);
}

#[test]
fn plan_unblock_names_every_planned_entry_and_is_only_a_plan() {
    let rules = vec![
        rule("b.example.com", DomainCategory::Tracking),
        rule("a.example.com", DomainCategory::Advertising),
    ];
    let plan = plan_block_list(&rules, &BlockListOptions::default());
    let unblock = plan_unblock(&plan);

    assert_eq!(unblock.total, plan.total);
    assert_eq!(unblock.entries, plan.entries);
    assert_eq!(domains(&plan), vec!["a.example.com", "b.example.com"]);
    let note = unblock.note.to_lowercase();
    assert!(note.contains("separately authorized"));
    assert!(note.contains("does not write"));
    assert!(note.contains("plan only"));
}

#[test]
fn empty_input_yields_empty_plan() {
    let plan = plan_block_list(&[], &BlockListOptions::default());
    assert_eq!(plan.total, 0);
    assert!(plan.entries.is_empty());
    assert_eq!(plan.duplicates_removed, 0);
    assert!(plan.invalid_removed.is_empty());
    assert!(plan.whitelisted.is_empty());
    assert_eq!(plan.truncated, 0);
}
