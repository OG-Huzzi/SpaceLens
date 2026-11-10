//! Integration tests for privacy findings summarization.
//!
//! The central honesty rule is exercised explicitly: a missing byte
//! estimate makes the total `None`, never a partial number. Session
//! cookies stay distinct from tracking cookies, counts saturate, and the
//! order is independent of input order.

use coresight_webtools::{summarize_privacy, PrivacyFinding, PrivacyFindingKind};

fn finding(
    kind: PrivacyFindingKind,
    count: u64,
    estimated_bytes: Option<u64>,
    detail: &str,
) -> PrivacyFinding {
    PrivacyFinding {
        kind,
        count,
        estimated_bytes,
        detail: detail.to_string(),
    }
}

#[test]
fn missing_byte_estimate_makes_total_none_not_partial() {
    let findings = vec![
        finding(
            PrivacyFindingKind::TrackingCookie,
            10,
            Some(4_096),
            "tracking cookies",
        ),
        finding(
            PrivacyFindingKind::BrowsingHistory,
            25,
            None,
            "history database",
        ),
    ];
    let summary = summarize_privacy(&findings);

    assert_eq!(
        summary.total_estimated_bytes, None,
        "a partial byte sum must never be presented as the total"
    );
    assert_eq!(summary.total_count, 35);
    assert!(summary.note.contains("not reported"));
    assert!(summary
        .note
        .contains("partial sum must never be presented as complete"));
}

#[test]
fn all_known_byte_estimates_sum_without_saturation_loss() {
    let findings = vec![
        finding(PrivacyFindingKind::CacheEntry, 5, Some(1_000), "cache a"),
        finding(
            PrivacyFindingKind::SiteStorage,
            2,
            Some(2_000),
            "site storage a",
        ),
    ];
    let summary = summarize_privacy(&findings);

    assert_eq!(summary.total_estimated_bytes, Some(3_000));
    assert_eq!(summary.total_count, 7);
    assert!(!summary.has_unknown_kinds);
}

#[test]
fn counts_and_bytes_saturate_instead_of_wrapping() {
    let findings = vec![
        finding(
            PrivacyFindingKind::CacheEntry,
            u64::MAX,
            Some(u64::MAX),
            "cache max",
        ),
        finding(
            PrivacyFindingKind::SiteStorage,
            1,
            Some(1),
            "site storage one",
        ),
    ];
    let summary = summarize_privacy(&findings);

    assert_eq!(summary.total_count, u64::MAX);
    assert_eq!(summary.total_estimated_bytes, Some(u64::MAX));
}

#[test]
fn deterministic_order_by_kind_then_detail_regardless_of_input_order() {
    let findings = vec![
        finding(PrivacyFindingKind::Unknown, 1, None, "z unknown"),
        finding(PrivacyFindingKind::SiteStorage, 1, Some(5), "site"),
        finding(PrivacyFindingKind::CacheEntry, 1, Some(5), "cache b"),
        finding(PrivacyFindingKind::BrowsingHistory, 1, None, "history"),
        finding(PrivacyFindingKind::SessionCookie, 1, Some(5), "session"),
        finding(PrivacyFindingKind::TrackingCookie, 1, Some(5), "tracking b"),
        finding(PrivacyFindingKind::TrackingCookie, 1, Some(5), "tracking a"),
    ];
    let mut reversed = findings.clone();
    reversed.reverse();

    let forward = summarize_privacy(&findings);
    let backward = summarize_privacy(&reversed);
    assert_eq!(forward, backward);

    let kinds: Vec<PrivacyFindingKind> = forward.findings.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        vec![
            PrivacyFindingKind::TrackingCookie,
            PrivacyFindingKind::TrackingCookie,
            PrivacyFindingKind::SessionCookie,
            PrivacyFindingKind::BrowsingHistory,
            PrivacyFindingKind::CacheEntry,
            PrivacyFindingKind::SiteStorage,
            PrivacyFindingKind::Unknown,
        ]
    );
    let tracking_details: Vec<&str> = forward
        .findings
        .iter()
        .filter(|f| f.kind == PrivacyFindingKind::TrackingCookie)
        .map(|f| f.detail.as_str())
        .collect();
    assert_eq!(tracking_details, vec!["tracking a", "tracking b"]);
}

#[test]
fn session_cookies_stay_distinct_and_note_warns_about_sign_out() {
    let findings = vec![
        finding(
            PrivacyFindingKind::SessionCookie,
            4,
            Some(512),
            "shop.example.com session",
        ),
        finding(
            PrivacyFindingKind::TrackingCookie,
            9,
            Some(2_048),
            "ads.example.com tracking",
        ),
    ];
    let summary = summarize_privacy(&findings);

    let kinds: Vec<PrivacyFindingKind> = summary.findings.iter().map(|f| f.kind).collect();
    assert!(kinds.contains(&PrivacyFindingKind::SessionCookie));
    assert!(kinds.contains(&PrivacyFindingKind::TrackingCookie));
    let note = summary.note.to_lowercase();
    assert!(note.contains("session cookies"));
    assert!(note.contains("persistent"));
    assert!(note.contains("signs you out"));
}

#[test]
fn unknown_kinds_are_flagged_and_never_renamed() {
    let findings = vec![
        finding(PrivacyFindingKind::Unknown, 3, Some(10), "mystery entry"),
        finding(
            PrivacyFindingKind::TrackingCookie,
            2,
            Some(10),
            "known entry",
        ),
    ];
    let summary = summarize_privacy(&findings);

    assert!(summary.has_unknown_kinds);
    assert!(summary
        .findings
        .iter()
        .any(|f| f.kind == PrivacyFindingKind::Unknown && f.detail == "mystery entry"));
    assert!(summary.note.to_lowercase().contains("unknown kind"));
}

#[test]
fn empty_findings_is_a_complete_zero_total() {
    let summary = summarize_privacy(&[]);
    assert!(summary.findings.is_empty());
    assert_eq!(summary.total_count, 0);
    assert_eq!(summary.total_estimated_bytes, Some(0));
    assert!(!summary.has_unknown_kinds);
    assert!(summary.note.contains("No findings were supplied"));
}

#[test]
fn known_zero_bytes_is_not_unknown() {
    let findings = vec![finding(
        PrivacyFindingKind::CacheEntry,
        1,
        Some(0),
        "empty cache bucket",
    )];
    let summary = summarize_privacy(&findings);
    assert_eq!(summary.total_estimated_bytes, Some(0));
}
