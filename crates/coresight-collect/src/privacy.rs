//! Browser-profile privacy listing loader.
//!
//! ## Input format
//!
//! One finding per line: `kind,count,bytes_or_unknown[,detail]`.
//!
//! * `kind` is ASCII case-insensitive: `tracking_cookie`, `session_cookie`,
//!   `browsing_history`, `cache_entry`, `site_storage`, or `unknown`.
//!   Hyphens are accepted in place of underscores. Any other token becomes
//!   [`PrivacyFindingKind::Unknown`] and is kept — it is counted, never
//!   dropped and never guessed into a different kind.
//! * `count` is a non-negative decimal integer.
//! * `bytes_or_unknown` is a decimal integer, an empty field, or `unknown`.
//!   An empty or `unknown` field becomes `estimated_bytes: None`; it is
//!   **never** `Some(0)`. A malformed token is a rejected line, not an
//!   unknown size: "12 KB" is a format error, not evidence of zero bytes.
//! * `detail` is optional free text shown to the user. It must not contain a
//!   comma (commas separate fields); when omitted or empty, a deterministic
//!   detail is generated and, for an unrecognised kind, names the original
//!   token so the evidence is not lost.
//!
//! ```text
//! # kind,count,bytes_or_unknown[,detail]
//! tracking_cookie,12,4096,Default profile
//! session_cookie,3,unknown,Default profile
//! cache_entry,120,
//! mystery_kind,7,unknown,Work profile
//! ```
//!
//! Blank lines and lines whose first non-whitespace character is `#` are
//! ignored. Every other malformed line is rejected with its 1-based line
//! number and a reason. Empty input is an empty result, not an error.
//!
//! ## What this loader does not do
//!
//! It does not read a browser profile, estimate a size, delete anything, or
//! infer that an absent size is zero. The counts are claims supplied by the
//! shell. `coresight-webtools`' `summarize_privacy` remains the authority on
//! totals and on the session-cookie consequence. No I/O happens here.

use coresight_webtools::{PrivacyFinding, PrivacyFindingKind};
use serde::{Deserialize, Serialize};

use crate::text::{excerpt, fits, is_blank_or_comment, text_lines, truncate_chars, LineReject};

/// Longest kind token this loader will consider.
pub const MAX_PRIVACY_KIND_CHARS: usize = 64;

/// Longest detail this loader will carry.
pub const MAX_PRIVACY_DETAIL_CHARS: usize = 512;

/// The result of loading a privacy listing.
///
/// `findings` is ordered by kind, detail, byte estimate, and count — the same
/// deterministic order `summarize_privacy` uses — so permuting input lines
/// cannot change it. `rejected` holds every malformed line with its line
/// number. Unknown kinds are present in `findings`, not in `rejected`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyParseResult {
    /// Successfully parsed findings, in canonical order.
    pub findings: Vec<PrivacyFinding>,
    /// Lines that produced no finding, with line number and reason.
    pub rejected: Vec<LineReject>,
}

impl PrivacyParseResult {
    /// How many findings carry [`PrivacyFindingKind::Unknown`].
    ///
    /// Counted explicitly because an unknown kind is a coverage gap, not a
    /// normal category: the UI must be able to say how many rows it could not
    /// classify. Saturating, so the count can never wrap.
    pub fn unknown_kind_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.kind == PrivacyFindingKind::Unknown)
            .fold(0usize, |total, _| total.saturating_add(1))
    }
}

/// Parse a privacy finding listing.
///
/// This function is total: empty text, whitespace, NUL bytes, a megabyte of
/// junk, and deeply nested brackets all return a [`PrivacyParseResult`]. It
/// performs no I/O and removes nothing.
pub fn parse_privacy_listing(text: &str) -> PrivacyParseResult {
    let mut findings: Vec<PrivacyFinding> = Vec::new();
    let mut rejected: Vec<LineReject> = Vec::new();

    for (line_number, raw) in text_lines(text) {
        if is_blank_or_comment(raw) {
            continue;
        }
        if raw.contains('\0') {
            rejected.push(LineReject::new(
                line_number,
                raw,
                "line contains a NUL byte",
            ));
            continue;
        }
        match parse_privacy_line(raw.trim()) {
            Ok(finding) => findings.push(finding),
            Err(reason) => rejected.push(LineReject::new(line_number, raw, reason)),
        }
    }

    sort_findings(&mut findings);
    rejected.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.reason.cmp(&right.reason))
    });

    PrivacyParseResult { findings, rejected }
}

/// Parse one non-blank, non-comment line.
fn parse_privacy_line(line: &str) -> Result<PrivacyFinding, String> {
    let mut fields = line.split(',');
    let kind_token = fields.next().unwrap_or("").trim();
    let count_token = match fields.next() {
        Some(token) => token.trim(),
        None => return Err("expected kind,count,bytes_or_unknown[,detail]".to_string()),
    };
    let bytes_token = match fields.next() {
        Some(token) => token.trim(),
        None => return Err("expected kind,count,bytes_or_unknown[,detail]".to_string()),
    };
    let detail_token = fields.next().map(str::trim);
    if fields.next().is_some() {
        return Err(
            "expected kind,count,bytes_or_unknown[,detail]; the line contains more than four comma-separated fields"
                .to_string(),
        );
    }

    if kind_token.is_empty() {
        return Err("missing kind before the first comma".to_string());
    }
    if !fits(kind_token, MAX_PRIVACY_KIND_CHARS) {
        return Err(format!(
            "kind token is longer than the {MAX_PRIVACY_KIND_CHARS}-character limit"
        ));
    }
    if let Some(detail) = detail_token {
        if !fits(detail, MAX_PRIVACY_DETAIL_CHARS) {
            return Err(format!(
                "detail is longer than the {MAX_PRIVACY_DETAIL_CHARS}-character limit"
            ));
        }
    }

    let count = parse_count(count_token)?;
    let estimated_bytes = parse_bytes(bytes_token)?;
    let kind = parse_kind(kind_token);

    let detail = match detail_token {
        Some(detail) if !detail.is_empty() => {
            if kind == PrivacyFindingKind::Unknown {
                // An explicit detail must not erase the evidence for the
                // unknown classification: keep the original token in front.
                let label = if kind_token.eq_ignore_ascii_case("unknown") {
                    "the listing reports this kind as unknown".to_string()
                } else {
                    format!("unrecognised kind {}", quote(kind_token))
                };
                truncate_chars(&format!("{label}; {detail}"), MAX_PRIVACY_DETAIL_CHARS)
            } else {
                detail.to_string()
            }
        }
        _ => default_detail(kind_token, kind),
    };

    Ok(PrivacyFinding {
        kind,
        count,
        estimated_bytes,
        detail,
    })
}

/// Parse a count. Missing or malformed counts are errors, never zero.
fn parse_count(token: &str) -> Result<u64, String> {
    if token.is_empty() {
        return Err("missing count".to_string());
    }
    token
        .parse::<u64>()
        .map_err(|_| format!("count {} is not a non-negative integer", quote(token)))
}

/// Parse the byte estimate. Empty and `unknown` are `None`; `0` is `Some(0)`.
fn parse_bytes(token: &str) -> Result<Option<u64>, String> {
    if token.is_empty() || token.eq_ignore_ascii_case("unknown") {
        return Ok(None);
    }
    match token.parse::<u64>() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(_) => Err(format!(
            "bytes {} is neither a non-negative integer nor 'unknown'",
            quote(token)
        )),
    }
}

/// Map a kind token; an unrecognised token is `Unknown`, never another kind.
fn parse_kind(token: &str) -> PrivacyFindingKind {
    let normalized = token.to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "tracking_cookie" => PrivacyFindingKind::TrackingCookie,
        "session_cookie" => PrivacyFindingKind::SessionCookie,
        "browsing_history" => PrivacyFindingKind::BrowsingHistory,
        "cache_entry" => PrivacyFindingKind::CacheEntry,
        "site_storage" => PrivacyFindingKind::SiteStorage,
        _ => PrivacyFindingKind::Unknown,
    }
}

/// A deterministic detail when the caller supplied none.
fn default_detail(token: &str, kind: PrivacyFindingKind) -> String {
    if kind == PrivacyFindingKind::Unknown {
        if token.eq_ignore_ascii_case("unknown") {
            "the supplied listing reports this kind as unknown".to_string()
        } else {
            format!(
                "unrecognised kind {} in the supplied profile listing",
                quote(token)
            )
        }
    } else {
        format!("{} in the supplied profile listing", quote(token))
    }
}

/// Sort findings by the same total key used by `summarize_privacy`.
fn sort_findings(findings: &mut [PrivacyFinding]) {
    findings.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.detail.cmp(&right.detail))
            .then_with(|| left.estimated_bytes.cmp(&right.estimated_bytes))
            .then_with(|| left.count.cmp(&right.count))
    });
}

/// Quote a short excerpt of caller text for a reason string.
fn quote(text: &str) -> String {
    format!("\"{}\"", excerpt(text))
}
