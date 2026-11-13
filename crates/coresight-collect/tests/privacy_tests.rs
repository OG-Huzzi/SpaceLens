//! Privacy listing loader tests.
//!
//! These prove the `None`-not-zero contract, unknown-kind accounting, and
//! deterministic ordering. Nothing is deleted, read, or estimated.

use coresight_collect::parse_privacy_listing;
use coresight_webtools::{summarize_privacy, PrivacyFindingKind};

#[test]
fn bytes_unknown_and_empty_become_none_never_zero() {
    let text = "\
tracking_cookie,12,4096
session_cookie,3,unknown
cache_entry,120,
";
    let result = parse_privacy_listing(text);

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.findings.len(), 3);

    let tracking = result
        .findings
        .iter()
        .find(|finding| finding.kind == PrivacyFindingKind::TrackingCookie);
    match tracking {
        Some(finding) => assert_eq!(finding.estimated_bytes, Some(4096)),
        None => panic!("tracking cookie finding missing"),
    }

    let session = result
        .findings
        .iter()
        .find(|finding| finding.kind == PrivacyFindingKind::SessionCookie);
    match session {
        Some(finding) => assert_eq!(finding.estimated_bytes, None),
        None => panic!("session cookie finding missing"),
    }

    let cache = result
        .findings
        .iter()
        .find(|finding| finding.kind == PrivacyFindingKind::CacheEntry);
    match cache {
        Some(finding) => assert_eq!(finding.estimated_bytes, None),
        None => panic!("cache finding missing"),
    }

    assert!(result
        .findings
        .iter()
        .all(|finding| finding.estimated_bytes != Some(0)));
    assert_eq!(
        summarize_privacy(&result.findings).total_estimated_bytes,
        None
    );
}

#[test]
fn a_literal_zero_is_some_zero_not_unknown() {
    let result = parse_privacy_listing("cache_entry,1,0\n");

    assert!(result.rejected.is_empty());
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].estimated_bytes, Some(0));
    assert_eq!(
        summarize_privacy(&result.findings).total_estimated_bytes,
        Some(0)
    );
}

#[test]
fn unknown_kind_is_kept_and_counted() {
    let text = "\
mystery_kind,7,unknown,Work profile
cache_entry,2,1024,Default profile
another_mystery,1,unknown
";
    let result = parse_privacy_listing(text);

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.findings.len(), 3);
    assert_eq!(result.unknown_kind_count(), 2);

    let summary = summarize_privacy(&result.findings);
    assert!(summary.has_unknown_kinds);
    assert_eq!(summary.total_count, 10);
    assert_eq!(summary.total_estimated_bytes, None);

    let mystery = result
        .findings
        .iter()
        .find(|finding| finding.detail.contains("mystery_kind"));
    match mystery {
        Some(finding) => {
            assert_eq!(finding.kind, PrivacyFindingKind::Unknown);
            assert!(finding.detail.contains("Work profile"));
        }
        None => panic!("unknown kind token was not preserved in the detail"),
    }
}

#[test]
fn malformed_count_is_rejected_with_its_line_number() {
    let text = "cache_entry,2,1024\ncache_entry,not-a-number,1024\n";
    let result = parse_privacy_listing(text);

    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(result.rejected[0].line, 2);
    assert!(result.rejected[0].reason.contains("count"));
}

#[test]
fn malformed_bytes_is_rejected_not_treated_as_unknown() {
    let result = parse_privacy_listing("cache_entry,1,12 KB\n");

    assert!(result.findings.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("bytes"));
}

#[test]
fn optional_detail_is_preserved_or_generated_deterministically() {
    let text = "\
cache_entry,1,1024,Default profile
tracking_cookie,4,unknown
";
    let result = parse_privacy_listing(text);

    assert!(result.rejected.is_empty());
    let cache = result
        .findings
        .iter()
        .find(|finding| finding.kind == PrivacyFindingKind::CacheEntry);
    match cache {
        Some(finding) => assert_eq!(finding.detail, "Default profile"),
        None => panic!("cache finding missing"),
    }

    let tracking = result
        .findings
        .iter()
        .find(|finding| finding.kind == PrivacyFindingKind::TrackingCookie);
    match tracking {
        Some(finding) => assert!(finding.detail.contains("tracking_cookie")),
        None => panic!("tracking finding missing"),
    }
}

#[test]
fn too_many_comma_fields_is_rejected() {
    let result = parse_privacy_listing("cache_entry,1,1024,Default profile,extra\n");

    assert!(result.findings.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("more than four"));
}

#[test]
fn privacy_order_is_deterministic_for_permuted_lines() {
    let tracking = "tracking_cookie,12,4096,Default profile";
    let session = "session_cookie,3,unknown,Default profile";
    let cache = "cache_entry,120,2048,Default profile";

    let forward = parse_privacy_listing(&format!("{tracking}\n{session}\n{cache}\n"));
    let backward = parse_privacy_listing(&format!("{cache}\n{tracking}\n{session}\n"));

    assert_eq!(forward, backward);
    assert_eq!(forward.findings.len(), 3);
    assert_eq!(forward.findings[0].kind, PrivacyFindingKind::TrackingCookie);
}

#[test]
fn empty_privacy_listing_is_an_empty_result() {
    let result = parse_privacy_listing("");

    assert!(result.findings.is_empty());
    assert!(result.rejected.is_empty());
    assert_eq!(result.unknown_kind_count(), 0);
}
