//! Hostile inputs aimed at the web tools: the hosts-file block planner, DNS
//! ranking, and the privacy summary.
//!
//! Every fixture is synthetic. No network call, DNS query, or hosts-file write
//! happens here. Each test names the honesty property it protects.

use coresight_webtools::{
    plan_block_list, plan_unblock, rank_dns, summarize_privacy, BlockListOptions, BlockRule,
    DnsCandidate, DnsExclusionReason, DnsMeasurement, DnsPrivacyPolicy, DnsRankOptions,
    DomainCategory, PrivacyFinding, PrivacyFindingKind, RuleSource,
};

fn rule(domain: &str, category: DomainCategory) -> BlockRule {
    BlockRule {
        domain: domain.to_string(),
        category,
        source: RuleSource::UserDefined,
        reason: "fixture".to_string(),
    }
}

fn options(whitelist: &[&str], max_entries: usize) -> BlockListOptions {
    BlockListOptions {
        whitelist: whitelist.iter().map(|entry| entry.to_string()).collect(),
        include_categories: Vec::new(),
        max_entries,
    }
}

fn candidate(name: &str) -> DnsCandidate {
    DnsCandidate {
        name: name.to_string(),
        ipv4: vec!["192.0.2.1".to_string()],
        ipv6: Vec::new(),
        operator: Some("Fixture Operator".to_string()),
        privacy_policy: DnsPrivacyPolicy::DocumentedNoLogging,
        supports_dnssec: true,
        supports_filtering: false,
    }
}

fn measurement(
    name: &str,
    median: f64,
    p95: f64,
    failure_rate: f64,
    samples: usize,
) -> DnsMeasurement {
    DnsMeasurement {
        candidate_name: name.to_string(),
        median_latency_ms: median,
        p95_latency_ms: p95,
        failure_rate,
        samples,
    }
}

fn rank_options(min_samples: usize, max_failure_rate: f64) -> DnsRankOptions {
    DnsRankOptions {
        min_samples,
        max_failure_rate,
        require_documented_privacy: false,
    }
}

// ---------------------------------------------------------------------------
// Block list: contradiction, path/string abuse, magnitude
// ---------------------------------------------------------------------------

// If this regresses, whitelisting a parent domain silently whitelists (or
// blocks) every subdomain, and a rule the user wrote stops applying.
#[test]
fn whitelist_matching_is_exact_and_never_covers_a_subdomain() {
    let plan = plan_block_list(
        &[
            rule("sub.example.com", DomainCategory::Tracking),
            rule("example.com", DomainCategory::Tracking),
        ],
        &options(&["example.com"], 0),
    );
    assert_eq!(plan.total, 1);
    assert_eq!(plan.entries[0].domain, "sub.example.com");
    assert_eq!(plan.whitelisted, vec!["example.com".to_string()]);

    let child_only = plan_block_list(
        &[rule("example.com", DomainCategory::Tracking)],
        &options(&["sub.example.com"], 0),
    );
    assert_eq!(
        child_only.total, 1,
        "a child whitelist must not cover its parent"
    );
    assert!(child_only.whitelisted.is_empty());
}

// If this regresses, a user who whitelisted a domain still gets it blocked and
// is never told the exclusion happened.
#[test]
fn a_whitelisted_domain_is_never_blocked_and_the_exclusion_is_published() {
    let plan = plan_block_list(
        &[
            rule("example.com", DomainCategory::Tracking),
            rule("kept.example", DomainCategory::Malware),
        ],
        &options(&["Example.COM"], 0),
    );

    assert_eq!(plan.total, 1);
    assert_eq!(plan.entries[0].domain, "kept.example");
    assert_eq!(plan.whitelisted, vec!["example.com".to_string()]);
    assert!(plan.invalid_removed.is_empty());
    assert_eq!(plan.duplicates_removed, 0);
}

// If this regresses, a degenerate spelling is normalized into a domain and
// written into a hosts file.
#[test]
fn degenerate_domain_spellings_are_rejected_with_a_reason() {
    let hostile = [
        "",
        "   ",
        ".",
        "..",
        "a",
        "a..b",
        ".example.com",
        "example.com.",
        "example.com/",
        "example.com:443",
        "*.example.com",
        "exa mple.com",
        "CON",
        "NUL",
        "PRN",
        "AUX",
        "COM1",
        "localhost",
        "-lead.example",
        "trail-.example",
        "tab\texample.com",
    ];

    let rules: Vec<BlockRule> = hostile
        .iter()
        .map(|domain| rule(domain, DomainCategory::Advertising))
        .collect();
    let plan = plan_block_list(&rules, &options(&[], 0));

    assert_eq!(plan.total, 0, "no hostile spelling may become an entry");
    assert_eq!(
        plan.invalid_removed.len(),
        hostile.len(),
        "every rejection must be reported, got {:?}",
        plan.invalid_removed
    );
    for rejected in &plan.invalid_removed {
        assert!(!rejected.reason.trim().is_empty());
        assert!(
            hostile.contains(&rejected.domain.as_str()),
            "the original text must be quoted back, got {:?}",
            rejected.domain
        );
    }
}

// If this regresses, the length limit is off by one and a domain the DNS wire
// format cannot carry is accepted.
#[test]
fn a_4097_byte_domain_is_rejected_but_a_253_byte_one_is_accepted() {
    let long_label = "a".repeat(63);
    let exact = format!("{long_label}.example.com");
    assert!(exact.len() <= 253);

    let plan = plan_block_list(
        &[
            rule(&exact, DomainCategory::Advertising),
            rule(
                &format!("{exact}.{}", "b".repeat(200)),
                DomainCategory::Advertising,
            ),
        ],
        &options(&[], 0),
    );

    assert_eq!(plan.total, 1);
    assert_eq!(plan.entries[0].domain, exact.to_ascii_lowercase());
    assert_eq!(plan.invalid_removed.len(), 1);
    assert!(plan.invalid_removed[0].reason.contains("maximum"));
}

// If this regresses, a malware rule is downgraded to advertising because the
// weaker rule arrived later.
#[test]
fn duplicate_rules_keep_the_most_protective_category() {
    let plan = plan_block_list(
        &[
            rule("evil.example", DomainCategory::Advertising),
            rule("evil.example", DomainCategory::Malware),
            rule("evil.example", DomainCategory::Tracking),
        ],
        &options(&[], 0),
    );

    assert_eq!(plan.total, 1);
    assert_eq!(plan.entries[0].category, DomainCategory::Malware);
    assert_eq!(plan.duplicates_removed, 2);
}

// If this regresses, a capped plan hides the entries it dropped.
#[test]
fn block_list_truncation_is_counted_exactly() {
    let plan = plan_block_list(
        &[
            rule("c.example", DomainCategory::Advertising),
            rule("a.example", DomainCategory::Advertising),
            rule("b.example", DomainCategory::Advertising),
        ],
        &options(&[], 1),
    );

    assert_eq!(plan.total, 1);
    assert_eq!(plan.entries[0].domain, "a.example", "kept in sorted order");
    assert_eq!(plan.truncated, 2);
}

// If this regresses, a `usize::MAX` cap is treated as an allocation request.
#[test]
fn block_list_usize_max_cap_does_not_allocate_or_hang() {
    let plan = plan_block_list(
        &[
            rule("a.example", DomainCategory::Advertising),
            rule("b.example", DomainCategory::Advertising),
        ],
        &options(&[], usize::MAX),
    );
    assert_eq!(plan.total, 2);
    assert_eq!(plan.truncated, 0);
}

// If this regresses, the plan depends on rule or whitelist order and a retry
// produces a different hosts file.
#[test]
fn block_list_is_byte_identical_across_rule_and_whitelist_permutations() {
    let rules = [
        rule("b.example", DomainCategory::Tracking),
        rule("a.example", DomainCategory::Malware),
        rule("a.example", DomainCategory::Advertising),
        rule("bad domain", DomainCategory::Advertising),
    ];
    let orders = [
        vec![
            rules[0].clone(),
            rules[1].clone(),
            rules[2].clone(),
            rules[3].clone(),
        ],
        vec![
            rules[3].clone(),
            rules[2].clone(),
            rules[0].clone(),
            rules[1].clone(),
        ],
        vec![
            rules[1].clone(),
            rules[3].clone(),
            rules[2].clone(),
            rules[0].clone(),
        ],
    ];
    let whitelists = [vec!["z.example"], vec!["q.example"], vec!["m.example"]];

    let plans: Vec<String> = orders
        .iter()
        .zip(whitelists.iter())
        .map(|(order, whitelist)| {
            serde_json::to_string(&plan_block_list(order, &options(whitelist, 0)))
                .expect("the plan serializes")
        })
        .collect();

    assert_eq!(plans[0], plans[1]);
    assert_eq!(plans[1], plans[2]);
}

// If this regresses, an invalid whitelist entry is dropped without a trace and
// the user believes it participated.
#[test]
fn an_invalid_whitelist_entry_is_reported_not_ignored() {
    let plan = plan_block_list(
        &[rule("example.com", DomainCategory::Advertising)],
        &options(&["not a domain", ""], 0),
    );

    assert_eq!(plan.total, 1, "an unusable whitelist excludes nothing");
    assert_eq!(plan.invalid_removed.len(), 2);
    assert!(plan.whitelisted.is_empty());
    for rejected in &plan.invalid_removed {
        assert!(rejected.reason.starts_with("invalid whitelist entry:"));
    }
}

// If this regresses, the undo plan implies domains that were never blocked.
#[test]
fn the_unblock_plan_names_exactly_the_blocked_entries() {
    let plan = plan_block_list(
        &[
            rule("a.example", DomainCategory::Advertising),
            rule("b.example", DomainCategory::Advertising),
        ],
        &options(&["b.example"], 0),
    );
    let unblock = plan_unblock(&plan);

    assert_eq!(unblock.total, 1);
    assert_eq!(unblock.entries[0].domain, "a.example");
    assert!(unblock.note.contains("separately authorized"));
}

// ---------------------------------------------------------------------------
// DNS ranking: contradictory measurements
// ---------------------------------------------------------------------------

// If this regresses, an unmeasurable failure rate is ranked, and the ranking
// presents a fabricated reliability number as evidence.
#[test]
fn non_finite_failure_rates_are_excluded_not_ranked() {
    let candidates = [
        candidate("nan"),
        candidate("plus-infinity"),
        candidate("minus-infinity"),
    ];
    let measurements = [
        measurement("nan", 10.0, 10.0, f64::NAN, 100),
        measurement("plus-infinity", 10.0, 10.0, f64::INFINITY, 100),
        measurement("minus-infinity", 10.0, 10.0, f64::NEG_INFINITY, 100),
    ];
    let ranking = rank_dns(&candidates, &measurements, &rank_options(1, 1.0));

    assert!(ranking.ranked.is_empty());
    assert_eq!(ranking.excluded.len(), 3);
    for excluded in &ranking.excluded {
        assert_eq!(excluded.reason, DnsExclusionReason::TooUnreliable);
        assert!(excluded.detail.contains("not a finite number"));
    }
}

// If this regresses, an impossible negative failure rate is scored as a
// perfect 100% reliability and outranks every real resolver.
// FIXED: rank_dns now rejects a rate outside 0..=1 instead of clamping it to
// "no failures".
#[test]
fn a_negative_failure_rate_is_never_scored_as_perfect_reliability() {
    let candidates = [candidate("minus-fifty-percent")];
    let measurements = [measurement("minus-fifty-percent", 10.0, 10.0, -0.5, 100)];
    let ranking = rank_dns(&candidates, &measurements, &rank_options(1, 0.05));

    assert!(
        ranking.ranked.is_empty(),
        "a failure rate outside 0..=1 is not evidence: {:?}",
        ranking
            .ranked
            .iter()
            .map(|row| (row.name.as_str(), row.reliability_score))
            .collect::<Vec<_>>()
    );
}

// If this regresses, a NaN failure-rate ceiling silently disables the filter
// and a resolver that fails every query is ranked as healthy.
// FIXED: an unusable ceiling now fails closed and excludes the candidate.
#[test]
fn a_nan_failure_rate_ceiling_does_not_silently_accept_everything() {
    let candidates = [candidate("always-fails")];
    let measurements = [measurement("always-fails", 10.0, 10.0, 0.9, 100)];
    let ranking = rank_dns(&candidates, &measurements, &rank_options(1, f64::NAN));

    assert!(
        ranking.ranked.is_empty(),
        "an unusable ceiling must fail closed, got {:?}",
        ranking
            .ranked
            .iter()
            .map(|row| (row.name.as_str(), row.reliability_score))
            .collect::<Vec<_>>()
    );
}

// If this regresses, a NaN or negative latency earns the same latency score as
// a measured 20 ms resolver.
#[test]
fn unmeasurable_latencies_score_zero_and_never_rank_first() {
    let candidates = [
        candidate("good"),
        candidate("nan-latency"),
        candidate("negative-latency"),
        candidate("huge-latency"),
    ];
    let measurements = [
        measurement("good", 20.0, 20.0, 0.0, 100),
        measurement("nan-latency", f64::NAN, f64::NAN, 0.0, 100),
        measurement("negative-latency", -5.0, -5.0, 0.0, 100),
        measurement("huge-latency", 10_000.0, 10_000.0, 0.0, 100),
    ];
    let ranking = rank_dns(&candidates, &measurements, &rank_options(1, 0.05));

    assert_eq!(ranking.ranked.len(), 4, "latency is scored, not screened");
    for row in &ranking.ranked {
        if row.name == "good" {
            assert_eq!(row.latency_score, 100);
        } else {
            assert_eq!(
                row.latency_score, 0,
                "{} must score zero, not a fabricated measurement",
                row.name
            );
        }
    }
    assert_eq!(ranking.ranked[0].name, "good");
}

// If this regresses, the record chosen for a name depends on array order and
// the same benchmark data ranks differently on a retry.
#[test]
fn measurement_selection_is_independent_of_input_order() {
    let candidates = [candidate("resolver")];
    let few = measurement("resolver", 10.0, 50.0, 0.10, 5);
    let many = measurement("resolver", 40.0, 200.0, 0.02, 50);

    let forward = rank_dns(
        &candidates,
        &[few.clone(), many.clone()],
        &rank_options(1, 1.0),
    );
    let reverse = rank_dns(&candidates, &[many, few], &rank_options(1, 1.0));

    assert_eq!(
        forward.ranked[0].samples, 50,
        "the record with the most samples is the evidence"
    );
    assert_eq!(
        serde_json::to_string(&forward).expect("serializes"),
        serde_json::to_string(&reverse).expect("serializes")
    );
}

// If this regresses, two candidates with equal scores swap places between runs
// and the ranking cannot be reviewed or diffed.
#[test]
fn dns_ranking_is_byte_identical_across_permutations() {
    let candidates = [
        candidate("c.example"),
        candidate("a.example"),
        candidate("b.example"),
    ];
    let measurements = [
        measurement("a.example", 30.0, 40.0, 0.0, 10),
        measurement("b.example", 30.0, 40.0, 0.0, 10),
        measurement("c.example", 30.0, 40.0, 0.0, 10),
    ];

    let forward = rank_dns(&candidates, &measurements, &rank_options(1, 1.0));
    let reversed = rank_dns(
        &[
            candidates[2].clone(),
            candidates[1].clone(),
            candidates[0].clone(),
        ],
        &[
            measurements[2].clone(),
            measurements[1].clone(),
            measurements[0].clone(),
        ],
        &rank_options(1, 1.0),
    );
    let rotated = rank_dns(
        &[
            candidates[1].clone(),
            candidates[2].clone(),
            candidates[0].clone(),
        ],
        &[
            measurements[1].clone(),
            measurements[0].clone(),
            measurements[2].clone(),
        ],
        &rank_options(1, 1.0),
    );

    assert_eq!(
        serde_json::to_string(&forward).expect("serializes"),
        serde_json::to_string(&reversed).expect("serializes")
    );
    assert_eq!(
        serde_json::to_string(&forward).expect("serializes"),
        serde_json::to_string(&rotated).expect("serializes")
    );
}

// If this regresses, an unmeasured candidate is silently omitted instead of
// being listed with a reason.
#[test]
fn an_unmeasured_candidate_is_excluded_with_a_reason() {
    let ranking = rank_dns(
        &[candidate("unmeasured.example")],
        &[],
        &rank_options(1, 1.0),
    );
    assert!(ranking.ranked.is_empty());
    assert_eq!(ranking.excluded.len(), 1);
    assert_eq!(ranking.excluded[0].reason, DnsExclusionReason::NotMeasured);
}

// If this regresses, `usize::MAX` samples or a zero minimum overflow or panic.
#[test]
fn extreme_sample_counts_are_handled_without_wrapping() {
    let candidates = [candidate("max-samples")];
    let measurements = [measurement("max-samples", 20.0, 20.0, 0.0, usize::MAX)];
    let ranking = rank_dns(&candidates, &measurements, &rank_options(0, 1.0));

    assert_eq!(ranking.ranked.len(), 1);
    assert_eq!(ranking.ranked[0].samples, usize::MAX);
    assert!(ranking.ranked[0].score <= 100);

    let empty = rank_dns(&candidates, &measurements, &rank_options(usize::MAX, 1.0));
    assert_eq!(empty.ranked[0].samples, usize::MAX);
}

// If this regresses, an empty candidate list produces a fabricated ranking or
// a note that overstates what was done.
#[test]
fn an_empty_dns_input_ranks_nothing_and_says_so() {
    let ranking = rank_dns(&[], &[], &DnsRankOptions::default());
    assert!(ranking.ranked.is_empty());
    assert!(ranking.excluded.is_empty());
    assert!(ranking.note.contains("Ranked 0 of 0"));
}

// ---------------------------------------------------------------------------
// Privacy summary
// ---------------------------------------------------------------------------

// If this regresses, two huge counts wrap to a small number and the summary
// understates what was found.
#[test]
fn privacy_totals_saturate_at_u64_max() {
    let findings = [
        PrivacyFinding {
            kind: PrivacyFindingKind::TrackingCookie,
            count: u64::MAX,
            estimated_bytes: Some(u64::MAX),
            detail: "a".to_string(),
        },
        PrivacyFinding {
            kind: PrivacyFindingKind::CacheEntry,
            count: u64::MAX,
            estimated_bytes: Some(1),
            detail: "b".to_string(),
        },
    ];
    let summary = summarize_privacy(&findings);

    assert_eq!(summary.total_count, u64::MAX);
    assert_eq!(summary.total_estimated_bytes, Some(u64::MAX));
}

// If this regresses, a partial byte sum is presented as the complete total.
#[test]
fn an_unknown_estimate_is_never_replaced_by_a_partial_sum() {
    let findings = [
        PrivacyFinding {
            kind: PrivacyFindingKind::TrackingCookie,
            count: 1,
            estimated_bytes: Some(1_000),
            detail: "known".to_string(),
        },
        PrivacyFinding {
            kind: PrivacyFindingKind::SiteStorage,
            count: 2,
            estimated_bytes: None,
            detail: "unknown".to_string(),
        },
    ];
    let summary = summarize_privacy(&findings);

    assert_eq!(summary.total_count, 3);
    assert_eq!(summary.total_estimated_bytes, None);
    assert!(summary.note.contains("not reported"));
}

// If this regresses, an unknown kind is guessed into a concrete kind and the
// review screen claims knowledge it does not have.
#[test]
fn an_unknown_kind_stays_unknown_and_is_flagged() {
    let summary = summarize_privacy(&[PrivacyFinding {
        kind: PrivacyFindingKind::Unknown,
        count: 1,
        estimated_bytes: None,
        detail: "could not classify".to_string(),
    }]);

    assert!(summary.has_unknown_kinds);
    assert_eq!(summary.findings[0].kind, PrivacyFindingKind::Unknown);
    assert!(summary.note.contains("UNKNOWN"));
}

// If this regresses, the same findings in a different order produce a
// different summary.
#[test]
fn privacy_summary_is_byte_identical_across_permutations() {
    let findings = [
        PrivacyFinding {
            kind: PrivacyFindingKind::TrackingCookie,
            count: 1,
            estimated_bytes: Some(1),
            detail: "b".to_string(),
        },
        PrivacyFinding {
            kind: PrivacyFindingKind::SessionCookie,
            count: 1,
            estimated_bytes: Some(2),
            detail: "a".to_string(),
        },
        PrivacyFinding {
            kind: PrivacyFindingKind::CacheEntry,
            count: 1,
            estimated_bytes: None,
            detail: "c".to_string(),
        },
    ];
    let orders = [
        vec![
            findings[0].clone(),
            findings[1].clone(),
            findings[2].clone(),
        ],
        vec![
            findings[2].clone(),
            findings[0].clone(),
            findings[1].clone(),
        ],
        vec![
            findings[1].clone(),
            findings[2].clone(),
            findings[0].clone(),
        ],
    ];

    let summaries: Vec<String> = orders
        .iter()
        .map(|order| {
            serde_json::to_string(&summarize_privacy(order)).expect("the summary serializes")
        })
        .collect();
    assert_eq!(summaries[0], summaries[1]);
    assert_eq!(summaries[1], summaries[2]);
}

// If this regresses, an empty input reports an unknown total instead of a
// complete zero, or vice versa.
#[test]
fn an_empty_privacy_input_is_a_complete_zero() {
    let summary = summarize_privacy(&[]);
    assert!(summary.findings.is_empty());
    assert_eq!(summary.total_count, 0);
    assert_eq!(summary.total_estimated_bytes, Some(0));
    assert!(!summary.has_unknown_kinds);
    assert!(summary.note.contains("No findings were supplied"));
}
