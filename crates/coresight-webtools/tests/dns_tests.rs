//! Integration tests for DNS candidate ranking.
//!
//! Synthetic candidates and measurements only. Nothing is probed,
//! resolved, or switched: the tests prove exclusion accounting, the
//! published non-latency-only weighting, exact tie-breaks, and that the
//! note always tells the user the choice (and the traffic exposure) is
//! theirs.

use coresight_webtools::{
    rank_dns, DnsCandidate, DnsExclusionReason, DnsMeasurement, DnsPrivacyPolicy, DnsRankOptions,
    DNS_DNSSEC_WEIGHT, DNS_LATENCY_WEIGHT, DNS_PRIVACY_WEIGHT, DNS_RELIABILITY_WEIGHT,
};

fn candidate(name: &str, policy: DnsPrivacyPolicy, dnssec: bool) -> DnsCandidate {
    DnsCandidate {
        name: name.to_string(),
        ipv4: vec!["192.0.2.53".to_string()],
        ipv6: vec!["2001:db8::53".to_string()],
        operator: Some("Test Operator".to_string()),
        privacy_policy: policy,
        supports_dnssec: dnssec,
        supports_filtering: false,
    }
}

fn measurement(name: &str, median: f64, failure: f64, samples: usize) -> DnsMeasurement {
    DnsMeasurement {
        candidate_name: name.to_string(),
        median_latency_ms: median,
        p95_latency_ms: median * 1.2,
        failure_rate: failure,
        samples,
    }
}

fn permissive_options() -> DnsRankOptions {
    DnsRankOptions {
        min_samples: 3,
        max_failure_rate: 0.10,
        require_documented_privacy: false,
    }
}

fn reason_for(ranking: &coresight_webtools::DnsRanking, name: &str) -> DnsExclusionReason {
    ranking
        .excluded
        .iter()
        .find(|excluded| excluded.name == name)
        .map(|excluded| excluded.reason)
        .expect("candidate must be excluded")
}

#[test]
fn candidate_without_measurement_is_excluded_as_not_measured() {
    let candidates = vec![
        candidate("unmeasured", DnsPrivacyPolicy::DocumentedNoLogging, true),
        candidate("measured", DnsPrivacyPolicy::DocumentedNoLogging, true),
    ];
    let measurements = vec![measurement("measured", 30.0, 0.0, 20)];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    assert_eq!(ranking.ranked.len(), 1);
    assert_eq!(ranking.ranked[0].name, "measured");
    assert_eq!(ranking.excluded.len(), 1);
    assert_eq!(
        reason_for(&ranking, "unmeasured"),
        DnsExclusionReason::NotMeasured
    );
    assert!(ranking.excluded[0].detail.contains("no measurement"));
}

#[test]
fn too_few_samples_is_insufficient_evidence() {
    let candidates = vec![candidate(
        "thin-evidence",
        DnsPrivacyPolicy::DocumentedNoLogging,
        true,
    )];
    let measurements = vec![measurement("thin-evidence", 20.0, 0.0, 2)];
    let options = DnsRankOptions {
        min_samples: 5,
        max_failure_rate: 0.10,
        require_documented_privacy: true,
    };
    let ranking = rank_dns(&candidates, &measurements, &options);

    assert!(ranking.ranked.is_empty());
    assert_eq!(
        reason_for(&ranking, "thin-evidence"),
        DnsExclusionReason::InsufficientEvidence
    );
    assert!(ranking.excluded[0].detail.contains('2'));
    assert!(ranking.excluded[0].detail.contains('5'));
}

#[test]
fn high_failure_rate_is_too_unreliable() {
    let candidates = vec![candidate(
        "flaky",
        DnsPrivacyPolicy::DocumentedNoLogging,
        true,
    )];
    let measurements = vec![measurement("flaky", 20.0, 0.50, 100)];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    assert!(ranking.ranked.is_empty());
    assert_eq!(
        reason_for(&ranking, "flaky"),
        DnsExclusionReason::TooUnreliable
    );
    assert!(ranking.excluded[0].detail.contains("50.00%"));
}

#[test]
fn non_finite_failure_rate_is_too_unreliable() {
    let candidates = vec![candidate(
        "broken-measurement",
        DnsPrivacyPolicy::DocumentedNoLogging,
        true,
    )];
    let measurements = vec![measurement("broken-measurement", 20.0, f64::NAN, 100)];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    assert!(ranking.ranked.is_empty());
    assert_eq!(
        reason_for(&ranking, "broken-measurement"),
        DnsExclusionReason::TooUnreliable
    );
}

#[test]
fn undisclosed_privacy_is_excluded_when_required() {
    let candidates = vec![candidate(
        "secret-resolver",
        DnsPrivacyPolicy::Undisclosed,
        true,
    )];
    let measurements = vec![measurement("secret-resolver", 20.0, 0.0, 100)];
    let options = DnsRankOptions {
        min_samples: 3,
        max_failure_rate: 0.10,
        require_documented_privacy: true,
    };
    let ranking = rank_dns(&candidates, &measurements, &options);

    assert!(ranking.ranked.is_empty());
    assert_eq!(
        reason_for(&ranking, "secret-resolver"),
        DnsExclusionReason::UndisclosedPrivacy
    );
    assert!(ranking.excluded[0].detail.contains("UNDISCLOSED"));
}

#[test]
fn undisclosed_privacy_is_ranked_when_not_required_but_scores_zero_privacy() {
    let candidates = vec![candidate(
        "secret-resolver",
        DnsPrivacyPolicy::Undisclosed,
        true,
    )];
    let measurements = vec![measurement("secret-resolver", 20.0, 0.0, 100)];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    assert_eq!(ranking.ranked.len(), 1);
    assert_eq!(ranking.ranked[0].privacy_score, 0);
    assert!(ranking.excluded.is_empty());
}

#[test]
fn ranking_is_not_latency_only() {
    let candidates = vec![
        candidate("fast-open", DnsPrivacyPolicy::Undisclosed, false),
        candidate("privacy-dns", DnsPrivacyPolicy::DocumentedNoLogging, true),
    ];
    let measurements = vec![
        measurement("fast-open", 10.0, 0.0, 50),
        measurement("privacy-dns", 60.0, 0.0, 50),
    ];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    assert_eq!(ranking.ranked.len(), 2);
    assert_eq!(
        ranking.ranked[0].name, "privacy-dns",
        "a slower but private, DNSSEC-validating resolver must be able to outrank a faster undisclosed one"
    );
    assert_eq!(ranking.ranked[1].name, "fast-open");
    assert!(ranking.ranked[0].score > ranking.ranked[1].score);
    assert!(ranking.ranked[0].latency_score < ranking.ranked[1].latency_score);
}

#[test]
fn equal_scores_tie_break_on_name_ascending() {
    let candidates = vec![
        candidate("Zeta", DnsPrivacyPolicy::DocumentedNoLogging, true),
        candidate("Alpha", DnsPrivacyPolicy::DocumentedNoLogging, true),
    ];
    let measurements = vec![
        measurement("Zeta", 40.0, 0.0, 100),
        measurement("Alpha", 40.0, 0.0, 100),
    ];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    assert_eq!(ranking.ranked.len(), 2);
    assert_eq!(ranking.ranked[0].score, ranking.ranked[1].score);
    assert_eq!(ranking.ranked[0].name, "Alpha");
    assert_eq!(ranking.ranked[1].name, "Zeta");
}

#[test]
fn deterministic_across_input_permutations() {
    let candidates = vec![
        candidate("one", DnsPrivacyPolicy::DocumentedNoLogging, true),
        candidate(
            "two",
            DnsPrivacyPolicy::DocumentedRetention { days: 30 },
            false,
        ),
        candidate("three", DnsPrivacyPolicy::Unknown, true),
        candidate("four", DnsPrivacyPolicy::Undisclosed, false),
    ];
    let measurements = vec![
        measurement("one", 25.0, 0.01, 100),
        measurement("two", 45.0, 0.02, 60),
        measurement("three", 80.0, 0.05, 30),
        measurement("four", 15.0, 0.09, 20),
    ];
    let reversed_candidates: Vec<DnsCandidate> = candidates.iter().rev().cloned().collect();
    let reversed_measurements: Vec<DnsMeasurement> = measurements.iter().rev().cloned().collect();

    let first = rank_dns(&candidates, &measurements, &permissive_options());
    let second = rank_dns(
        &reversed_candidates,
        &reversed_measurements,
        &permissive_options(),
    );
    assert_eq!(first, second);
}

#[test]
fn duplicate_measurement_records_choose_most_samples_deterministically() {
    let candidates = vec![candidate(
        "resolver",
        DnsPrivacyPolicy::DocumentedNoLogging,
        true,
    )];
    let lower_evidence = measurement("resolver", 200.0, 0.50, 5);
    let higher_evidence = measurement("Resolver", 40.0, 0.00, 500);

    let first = rank_dns(
        &candidates,
        &[lower_evidence.clone(), higher_evidence.clone()],
        &permissive_options(),
    );
    let second = rank_dns(
        &candidates,
        &[higher_evidence, lower_evidence],
        &permissive_options(),
    );

    assert_eq!(first, second);
    assert_eq!(first.ranked.len(), 1);
    assert_eq!(first.ranked[0].median_latency_ms, 40.0);
    assert_eq!(first.ranked[0].samples, 500);
}

#[test]
fn every_excluded_candidate_is_listed_with_a_reason() {
    let candidates = vec![
        candidate("no-data", DnsPrivacyPolicy::DocumentedNoLogging, true),
        candidate("few", DnsPrivacyPolicy::DocumentedNoLogging, true),
        candidate("flaky", DnsPrivacyPolicy::DocumentedNoLogging, true),
        candidate("secret", DnsPrivacyPolicy::Undisclosed, true),
        candidate("good", DnsPrivacyPolicy::DocumentedNoLogging, true),
    ];
    let measurements = vec![
        measurement("few", 20.0, 0.0, 1),
        measurement("flaky", 20.0, 0.90, 100),
        measurement("secret", 20.0, 0.0, 100),
        measurement("good", 30.0, 0.0, 100),
    ];
    let options = DnsRankOptions {
        min_samples: 5,
        max_failure_rate: 0.10,
        require_documented_privacy: true,
    };
    let ranking = rank_dns(&candidates, &measurements, &options);

    assert_eq!(ranking.ranked.len(), 1);
    assert_eq!(ranking.ranked[0].name, "good");
    assert_eq!(ranking.excluded.len(), 4);
    assert_eq!(
        reason_for(&ranking, "no-data"),
        DnsExclusionReason::NotMeasured
    );
    assert_eq!(
        reason_for(&ranking, "few"),
        DnsExclusionReason::InsufficientEvidence
    );
    assert_eq!(
        reason_for(&ranking, "flaky"),
        DnsExclusionReason::TooUnreliable
    );
    assert_eq!(
        reason_for(&ranking, "secret"),
        DnsExclusionReason::UndisclosedPrivacy
    );
    assert!(ranking
        .excluded
        .iter()
        .all(|excluded| !excluded.detail.is_empty()));
    let names: Vec<&str> = ranking
        .excluded
        .iter()
        .map(|excluded| excluded.name.as_str())
        .collect();
    assert_eq!(names, vec!["few", "flaky", "no-data", "secret"]);
}

#[test]
fn published_component_scores_produce_the_total() {
    let candidates = vec![candidate(
        "scored",
        DnsPrivacyPolicy::DocumentedNoLogging,
        true,
    )];
    let measurements = vec![measurement("scored", 100.0, 0.05, 20)];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());
    let ranked = &ranking.ranked[0];

    assert_eq!(ranked.dnssec_score, 100);
    assert_eq!(ranked.privacy_score, 100);
    assert_eq!(ranked.reliability_score, 95);
    let expected = (ranked.latency_score * DNS_LATENCY_WEIGHT
        + ranked.reliability_score * DNS_RELIABILITY_WEIGHT
        + ranked.dnssec_score * DNS_DNSSEC_WEIGHT
        + ranked.privacy_score * DNS_PRIVACY_WEIGHT)
        / 100;
    assert_eq!(ranked.score, expected);
    assert!(ranked.latency_score > 0 && ranked.latency_score < 100);
}

#[test]
fn note_states_that_core_sight_never_switches_dns() {
    let candidates = vec![candidate(
        "only",
        DnsPrivacyPolicy::DocumentedNoLogging,
        true,
    )];
    let measurements = vec![measurement("only", 20.0, 0.0, 100)];
    let ranking = rank_dns(&candidates, &measurements, &permissive_options());

    let note = ranking.note.to_lowercase();
    assert!(note.contains("switching dns"));
    assert!(note.contains("resolver"));
    assert!(note.contains("choice is the user's"));
    assert!(note.contains("never switches dns"));
    assert!(note.contains("40% latency"));
    assert!(note.contains("30% reliability"));
    assert!(note.contains("15% dnssec"));
    assert!(note.contains("15% documented privacy"));
}

#[test]
fn empty_candidate_list_still_carries_the_never_switch_note() {
    let ranking = rank_dns(&[], &[], &permissive_options());
    assert!(ranking.ranked.is_empty());
    assert!(ranking.excluded.is_empty());
    assert!(ranking.note.to_lowercase().contains("never switches dns"));
}
