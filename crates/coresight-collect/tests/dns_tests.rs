//! DNS candidate loader tests.
//!
//! No resolver is contacted and no measurement is invented: these tests only
//! prove the documented line format, the rejection reasons, and deterministic
//! ordering.

use coresight_collect::parse_dns_candidates;
use coresight_webtools::DnsPrivacyPolicy;

#[test]
fn dns_line_parses_every_documented_field() {
    let text = "\
# CoreSight candidate listing
Cloudflare | ipv4=1.1.1.1,1.0.0.1 | ipv6=2606:4700:4700::1111 | dnssec=yes | filter=no | privacy=no-logging | operator=Cloudflare
Quad9 | ipv4=9.9.9.9 | ipv6=2620:fe::fe | dnssec=yes | filter=yes | privacy=retention:90
";
    let result = parse_dns_candidates(text);

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.candidates.len(), 2);

    let cloudflare = result
        .candidates
        .iter()
        .find(|candidate| candidate.name == "Cloudflare");
    match cloudflare {
        Some(candidate) => {
            assert_eq!(candidate.ipv4, vec!["1.1.1.1", "1.0.0.1"]);
            assert_eq!(candidate.ipv6, vec!["2606:4700:4700::1111"]);
            assert_eq!(candidate.operator.as_deref(), Some("Cloudflare"));
            assert!(candidate.supports_dnssec);
            assert!(!candidate.supports_filtering);
            assert_eq!(
                candidate.privacy_policy,
                DnsPrivacyPolicy::DocumentedNoLogging
            );
        }
        None => panic!("Cloudflare candidate missing"),
    }

    let quad9 = result
        .candidates
        .iter()
        .find(|candidate| candidate.name == "Quad9");
    match quad9 {
        Some(candidate) => {
            assert_eq!(
                candidate.privacy_policy,
                DnsPrivacyPolicy::DocumentedRetention { days: 90 }
            );
            assert!(candidate.supports_filtering);
        }
        None => panic!("Quad9 candidate missing"),
    }
}

#[test]
fn dns_ipv6_only_candidate_is_accepted() {
    let result = parse_dns_candidates(
        "IPv6 only | ipv6=2001:4860:4860::8888 | dnssec=no | filter=no | privacy=unknown\n",
    );

    assert!(result.rejected.is_empty());
    assert_eq!(result.candidates.len(), 1);
    assert!(result.candidates[0].ipv4.is_empty());
    assert_eq!(
        result.candidates[0].privacy_policy,
        DnsPrivacyPolicy::Unknown
    );
}

#[test]
fn dns_invalid_flag_is_rejected_with_its_line_number() {
    let text = "Good | ipv4=1.1.1.1 | dnssec=yes | filter=no | privacy=unknown\n\
                Bad | ipv4=1.1.1.1 | dnssec=maybe | filter=no | privacy=unknown\n";
    let result = parse_dns_candidates(text);

    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(result.rejected[0].line, 2);
    assert!(result.rejected[0].reason.contains("dnssec"));
    assert!(result.rejected[0].text.contains("maybe"));
}

#[test]
fn dns_unknown_field_key_is_rejected() {
    let result = parse_dns_candidates(
        "Odd | ipv4=1.1.1.1 | dnssec=yes | filter=no | privacy=unknown | colour=blue\n",
    );

    assert!(result.candidates.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("unknown field key"));
}

#[test]
fn dns_missing_required_field_is_rejected_not_defaulted() {
    let result = parse_dns_candidates("No privacy | ipv4=1.1.1.1 | dnssec=yes | filter=no\n");

    assert!(result.candidates.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("privacy"));
}

#[test]
fn dns_malformed_address_is_rejected() {
    let result = parse_dns_candidates(
        "Bad address | ipv4=999.1.1.1 | dnssec=no | filter=no | privacy=unknown\n",
    );

    assert!(result.candidates.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("IPv4"));
}

#[test]
fn dns_candidate_without_addresses_is_rejected() {
    let result =
        parse_dns_candidates("Empty | ipv4= | ipv6= | dnssec=no | filter=no | privacy=unknown\n");

    assert!(result.candidates.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("no resolver address"));
}

#[test]
fn dns_empty_input_is_an_empty_result() {
    let result = parse_dns_candidates("");
    assert!(result.candidates.is_empty());
    assert!(result.rejected.is_empty());
}

#[test]
fn dns_comments_blank_lines_and_a_bom_are_skipped() {
    let text = "\u{feff}# comment\n\n   \nA | ipv4=192.0.2.53 | dnssec=yes | filter=no | privacy=unknown\n";
    let result = parse_dns_candidates(text);

    assert!(result.rejected.is_empty());
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].name, "A");
}

#[test]
fn dns_duplicate_names_are_preserved_not_silently_dropped() {
    let text = "Same | ipv4=192.0.2.1 | dnssec=yes | filter=no | privacy=unknown\n\
                Same | ipv4=192.0.2.2 | dnssec=yes | filter=no | privacy=unknown\n";
    let result = parse_dns_candidates(text);

    assert!(result.rejected.is_empty());
    assert_eq!(result.candidates.len(), 2);
}

#[test]
fn dns_candidate_order_is_deterministic_for_permuted_lines() {
    let alpha = "Alpha | ipv4=192.0.2.1 | dnssec=yes | filter=no | privacy=unknown";
    let beta = "Beta | ipv4=192.0.2.2 | dnssec=no | filter=yes | privacy=no-logging";
    let gamma = "Gamma | ipv4=192.0.2.3 | dnssec=yes | filter=yes | privacy=undisclosed";

    let forward = parse_dns_candidates(&format!("{alpha}\n{beta}\n{gamma}\n"));
    let backward = parse_dns_candidates(&format!("{gamma}\n{alpha}\n{beta}\n"));

    assert_eq!(forward, backward);
    assert_eq!(forward.candidates.len(), 3);
    assert_eq!(forward.candidates[0].name, "Alpha");
    assert_eq!(forward.candidates[1].name, "Beta");
    assert_eq!(forward.candidates[2].name, "Gamma");
}
