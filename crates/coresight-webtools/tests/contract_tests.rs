//! Architecture and serialization contract tests for `coresight-webtools`.
//!
//! These prove the negative space around the planners: no I/O, process,
//! network, or database API appears in production source; no panicking
//! construct is used; and every output type serializes with the stable
//! camelCase/SCREAMING_SNAKE_CASE shape the UI expects.

use coresight_webtools::{
    plan_block_list, plan_unblock, rank_dns, summarize_privacy, BlockListOptions, BlockRule,
    DnsCandidate, DnsExclusionReason, DnsMeasurement, DnsPrivacyPolicy, DnsRankOptions,
    DomainCategory, PrivacyFinding, PrivacyFindingKind, RuleSource,
};

#[test]
fn crate_contains_no_io_process_network_or_database_access() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for forbidden in [
            "std::fs::",
            "std::process",
            "std::net",
            "std::os::windows",
            "std::os::unix",
            "rusqlite",
            "Command::new",
            "reqwest",
            "tokio",
            "sqlx",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains forbidden API {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn production_source_has_no_panicking_constructs() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for forbidden in [
            "unwrap()",
            "expect(",
            "panic!(",
            "todo!(",
            "unimplemented!(",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains panicking construct {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn block_plan_serializes_with_camel_case_keys_and_screaming_enums() {
    let rules = vec![BlockRule {
        domain: "Ads.Example.com".to_string(),
        category: DomainCategory::SocialMedia,
        source: RuleSource::VendorList {
            list_name: "test list".to_string(),
            list_version: "1.0".to_string(),
        },
        reason: "test".to_string(),
    }];
    let options = BlockListOptions::default();
    let plan = plan_block_list(&rules, &options);
    let unblock = plan_unblock(&plan);
    let json = serde_json::to_value(&plan).unwrap();

    assert!(json.get("total").is_some());
    assert!(json.get("duplicatesRemoved").is_some());
    assert!(json.get("invalidRemoved").is_some());
    assert!(json.get("whitelisted").is_some());
    assert!(json.get("truncated").is_some());
    assert_eq!(json["entries"][0]["domain"], "ads.example.com");
    assert_eq!(json["entries"][0]["ipv4"], "127.0.0.1");
    assert_eq!(json["entries"][0]["ipv6"], "::1");
    assert_eq!(json["entries"][0]["category"], "SOCIAL_MEDIA");

    let unblock_json = serde_json::to_value(&unblock).unwrap();
    assert!(unblock_json.get("entries").is_some());
    assert!(unblock_json.get("note").is_some());

    let source = serde_json::to_value(RuleSource::VendorList {
        list_name: "test list".to_string(),
        list_version: "1.0".to_string(),
    })
    .unwrap();
    assert_eq!(source["VENDOR_LIST"]["listName"], "test list");
    assert_eq!(source["VENDOR_LIST"]["listVersion"], "1.0");
}

#[test]
fn dns_ranking_serializes_with_camel_case_keys() {
    let candidates = vec![DnsCandidate {
        name: "Example".to_string(),
        ipv4: vec!["192.0.2.1".to_string()],
        ipv6: vec![],
        operator: None,
        privacy_policy: DnsPrivacyPolicy::DocumentedRetention { days: 30 },
        supports_dnssec: true,
        supports_filtering: false,
    }];
    let measurements = vec![DnsMeasurement {
        candidate_name: "example".to_string(),
        median_latency_ms: 25.0,
        p95_latency_ms: 40.0,
        failure_rate: 0.0,
        samples: 30,
    }];
    let options = DnsRankOptions {
        min_samples: 1,
        max_failure_rate: 0.1,
        require_documented_privacy: false,
    };
    let ranking = rank_dns(&candidates, &measurements, &options);
    let json = serde_json::to_value(&ranking).unwrap();

    assert!(json.get("ranked").is_some());
    assert!(json.get("excluded").is_some());
    assert!(json.get("note").is_some());
    assert!(json["ranked"][0].get("latencyScore").is_some());
    assert_eq!(json["ranked"][0]["dnssecScore"], 100);
    assert!(json["ranked"][0].get("medianLatencyMs").is_some());
    let reason = serde_json::to_value(DnsExclusionReason::NotMeasured).unwrap();
    assert_eq!(reason, "NOT_MEASURED");
}

#[test]
fn privacy_summary_serializes_with_camel_case_keys() {
    let findings = vec![PrivacyFinding {
        kind: PrivacyFindingKind::SessionCookie,
        count: 3,
        estimated_bytes: None,
        detail: "shop.example".to_string(),
    }];
    let summary = summarize_privacy(&findings);
    let json = serde_json::to_value(&summary).unwrap();

    assert!(json.get("findings").is_some());
    assert!(json.get("totalCount").is_some());
    assert!(json.get("totalEstimatedBytes").is_some());
    assert!(json.get("hasUnknownKinds").is_some());
    assert_eq!(json["findings"][0]["kind"], "SESSION_COOKIE");
    assert_eq!(json["totalEstimatedBytes"], serde_json::Value::Null);
}
