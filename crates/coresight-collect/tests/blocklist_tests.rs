//! Block-list loader tests.
//!
//! The loader is checked for category handling and rejection discipline; the
//! domain rules themselves are the webtools planner's responsibility and are
//! asserted only through its plan.

use coresight_collect::parse_block_rules;
use coresight_webtools::{DomainCategory, RuleSource};

fn user_source() -> RuleSource {
    RuleSource::UserDefined
}

#[test]
fn block_rules_parse_domain_and_category() {
    let text = "\
# CoreSight list
ads.example.com,advertising
tracker.example.net,tracking
malware.example.org,malware
custom.example,social-media
";
    let result = parse_block_rules(text, user_source());

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.rules.len(), 4);
    assert_eq!(result.defaulted_categories, 0);
    assert_eq!(result.plan.total, 4);

    let categories: Vec<DomainCategory> = result.rules.iter().map(|rule| rule.category).collect();
    assert!(categories.contains(&DomainCategory::Advertising));
    assert!(categories.contains(&DomainCategory::Tracking));
    assert!(categories.contains(&DomainCategory::Malware));
    assert!(categories.contains(&DomainCategory::SocialMedia));
}

#[test]
fn unknown_category_is_rejected_and_never_coerced_to_custom() {
    let result = parse_block_rules("example.com,super-sketchy\n", user_source());

    assert!(result.rules.is_empty());
    assert!(result.plan.entries.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("unknown category"));
    assert_eq!(result.defaulted_categories, 0);
}

#[test]
fn missing_category_is_custom_and_counted_as_defaulted() {
    let result = parse_block_rules("example.com\n", user_source());

    assert!(result.rejected.is_empty());
    assert_eq!(result.rules.len(), 1);
    assert_eq!(result.rules[0].category, DomainCategory::Custom);
    assert!(result.rules[0].reason.contains("no category"));
    assert_eq!(result.defaulted_categories, 1);
}

#[test]
fn block_comments_blank_lines_and_trailing_comments_are_skipped() {
    let text = "\n# a comment\n   \nads.example.com,advertising # inline note\n";
    let result = parse_block_rules(text, user_source());

    assert!(result.rejected.is_empty());
    assert_eq!(result.rules.len(), 1);
    assert_eq!(result.rules[0].domain, "ads.example.com");
}

#[test]
fn domain_validation_is_delegated_to_the_webtools_plan() {
    let result = parse_block_rules(
        "bad domain.example,advertising\nnodots,advertising\n",
        user_source(),
    );

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.rules.len(), 2);
    assert!(result.plan.entries.is_empty());
    assert_eq!(result.plan.invalid_removed.len(), 2);
    assert!(result
        .plan
        .invalid_removed
        .iter()
        .any(|rejected| rejected.reason.contains("whitespace")));
}

#[test]
fn valid_rules_produce_both_loopback_entries() {
    let result = parse_block_rules("Ads.Example.com,advertising\n", user_source());

    assert!(result.rejected.is_empty());
    assert_eq!(result.plan.entries.len(), 1);
    let entry = &result.plan.entries[0];
    assert_eq!(entry.domain, "ads.example.com");
    assert_eq!(entry.ipv4, "127.0.0.1");
    assert_eq!(entry.ipv6, "::1");
    assert_eq!(entry.category, DomainCategory::Advertising);
}

#[test]
fn more_than_two_comma_separated_fields_is_rejected() {
    let result = parse_block_rules("example.com,advertising,extra\n", user_source());

    assert!(result.rules.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("more than one comma"));
}

#[test]
fn rule_source_provenance_is_carried_into_every_rule() {
    let source = RuleSource::VendorList {
        list_name: "test list".to_string(),
        list_version: "1.0".to_string(),
    };
    let result = parse_block_rules(
        "one.example.com,advertising\ntwo.example.com,tracking\n",
        source,
    );

    assert_eq!(result.rules.len(), 2);
    for rule in &result.rules {
        match &rule.source {
            RuleSource::VendorList {
                list_name,
                list_version,
            } => {
                assert_eq!(list_name, "test list");
                assert_eq!(list_version, "1.0");
            }
            other => panic!("expected VendorList provenance, got {other:?}"),
        }
    }
}

#[test]
fn block_rule_order_is_deterministic_for_permuted_lines() {
    let alpha = "alpha.example.com,advertising";
    let beta = "beta.example.com,tracking";
    let gamma = "gamma.example.com,malware";

    let forward = parse_block_rules(&format!("{alpha}\n{beta}\n{gamma}\n"), user_source());
    let backward = parse_block_rules(&format!("{gamma}\n{alpha}\n{beta}\n"), user_source());

    assert_eq!(forward, backward);
    assert_eq!(forward.rules.len(), 3);
    assert_eq!(forward.plan.total, 3);
}

#[test]
fn empty_block_text_is_an_empty_result() {
    let result = parse_block_rules("", user_source());

    assert!(result.rules.is_empty());
    assert!(result.plan.entries.is_empty());
    assert!(result.rejected.is_empty());
    assert_eq!(result.defaulted_categories, 0);
}
