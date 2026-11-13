//! DNS candidate listing loader.
//!
//! ## Input format
//!
//! The shell supplies one candidate per line, fields separated by `|`. The
//! first field is the candidate name; every later field is `key=value` and
//! may appear in any order:
//!
//! ```text
//! # comment
//! Cloudflare | ipv4=1.1.1.1,1.0.0.1 | ipv6=2606:4700:4700::1111 | dnssec=yes | filter=no | privacy=no-logging | operator=Cloudflare
//! Quad9 | ipv4=9.9.9.9 | ipv6=2620:fe::fe | dnssec=yes | filter=yes | privacy=retention:90
//! Quad9 IPv6 | ipv6=2620:fe::10 | dnssec=yes | filter=no | privacy=undisclosed
//! ```
//!
//! * Keys are ASCII case-insensitive: `ipv4`, `ipv6`, `dnssec`, `filter`
//!   (alias `filtering`), `privacy` (alias `policy`), and optional
//!   `operator` (an empty value means the operator was not reported).
//! * `ipv4` and `ipv6` values are comma-separated address lists. Either list
//!   may be present but empty; at least one address must be supplied. IPv4 is
//!   checked as four decimal octets. IPv6 is checked structurally — hex
//!   digits, colons, an optional `%zone`, and an optional embedded IPv4 tail —
//!   not semantically, and nothing is resolved or contacted.
//! * `dnssec` and `filter` are required and accept `yes`/`no`, `true`/`false`,
//!   or `1`/`0`. They are required rather than defaulted: a missing claim is
//!   not evidence that DNSSEC is absent.
//! * `privacy` is required and accepts `no-logging` (also `no-log`, `nolog`),
//!   `retention:<days>`, `undisclosed`, or `unknown`.
//! * Blank lines and lines whose first non-whitespace character is `#` are
//!   ignored. Inline `#` is data, not a comment.
//!
//! Every other shape is rejected with its 1-based line number and a reason:
//! a missing name, an unknown or duplicate key, a missing required field, an
//! unreadable flag or privacy value, a malformed address, or a field beyond
//! the documented length limits. Empty input is an empty result, not an
//! error.
//!
//! ## What this loader does not do
//!
//! It does not measure, probe, rank, or switch DNS. It does not verify a
//! privacy claim. Duplicate names are preserved as separate candidates; the
//! ranking layer in `coresight-webtools` matches measurements by name and is
//! the authority on how they are used. No I/O of any kind happens here.

use coresight_webtools::{DnsCandidate, DnsPrivacyPolicy};
use serde::{Deserialize, Serialize};

use crate::text::{excerpt, fits, is_blank_or_comment, text_lines, LineReject};

/// Longest candidate name this loader will carry.
pub const MAX_DNS_NAME_CHARS: usize = 256;

/// Longest operator name this loader will carry.
pub const MAX_DNS_OPERATOR_CHARS: usize = 256;

/// Longest single address string this loader will carry.
pub const MAX_DNS_ADDRESS_CHARS: usize = 128;

/// The result of loading a DNS candidate listing.
///
/// `candidates` is ordered by case-folded name and then by the remaining
/// fields, so permuting the input lines cannot change it. `rejected` holds
/// every line that did not become a candidate, with its line number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsParseResult {
    /// Successfully parsed candidates, in canonical order.
    pub candidates: Vec<DnsCandidate>,
    /// Lines that produced no candidate, with line number and reason.
    pub rejected: Vec<LineReject>,
}

/// Parse a DNS candidate listing.
///
/// This function is total: empty text, whitespace, NUL bytes, a megabyte of
/// junk, and deeply nested brackets all return a [`DnsParseResult`]. It
/// performs no network access and never changes a resolver.
pub fn parse_dns_candidates(text: &str) -> DnsParseResult {
    let mut candidates: Vec<DnsCandidate> = Vec::new();
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
        match parse_dns_line(raw.trim()) {
            Ok(candidate) => candidates.push(candidate),
            Err(reason) => rejected.push(LineReject::new(line_number, raw, reason)),
        }
    }

    sort_candidates(&mut candidates);
    rejected.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.reason.cmp(&right.reason))
    });

    DnsParseResult {
        candidates,
        rejected,
    }
}

/// Parse one non-blank, non-comment line.
fn parse_dns_line(line: &str) -> Result<DnsCandidate, String> {
    let mut fields = line.split('|');
    let name = fields.next().unwrap_or("").trim();
    if name.is_empty() {
        return Err("missing candidate name before the first '|' separator".to_string());
    }
    if !fits(name, MAX_DNS_NAME_CHARS) {
        return Err(format!(
            "candidate name is longer than the {MAX_DNS_NAME_CHARS}-character limit"
        ));
    }

    let mut ipv4: Option<Vec<String>> = None;
    let mut ipv6: Option<Vec<String>> = None;
    let mut dnssec: Option<bool> = None;
    let mut filtering: Option<bool> = None;
    let mut privacy: Option<DnsPrivacyPolicy> = None;
    let mut operator: Option<String> = None;

    for field in fields {
        let field = field.trim();
        if field.is_empty() {
            return Err("empty field between '|' separators".to_string());
        }
        let (key, value) = match field.split_once('=') {
            Some(pair) => pair,
            None => return Err(format!("field {} is not in key=value form", quote(field))),
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "ipv4" => {
                if ipv4.is_some() {
                    return Err("duplicate ipv4 field".to_string());
                }
                ipv4 = Some(parse_addresses(value, true)?);
            }
            "ipv6" => {
                if ipv6.is_some() {
                    return Err("duplicate ipv6 field".to_string());
                }
                ipv6 = Some(parse_addresses(value, false)?);
            }
            "dnssec" => {
                if dnssec.is_some() {
                    return Err("duplicate dnssec field".to_string());
                }
                dnssec = Some(parse_flag("dnssec", value)?);
            }
            "filter" | "filtering" => {
                if filtering.is_some() {
                    return Err("duplicate filter/filtering field".to_string());
                }
                filtering = Some(parse_flag("filter", value)?);
            }
            "privacy" | "policy" => {
                if privacy.is_some() {
                    return Err("duplicate privacy/policy field".to_string());
                }
                privacy = Some(parse_privacy_policy(value)?);
            }
            "operator" => {
                if operator.is_some() {
                    return Err("duplicate operator field".to_string());
                }
                if !fits(value, MAX_DNS_OPERATOR_CHARS) {
                    return Err(format!(
                        "operator is longer than the {MAX_DNS_OPERATOR_CHARS}-character limit"
                    ));
                }
                if !value.is_empty() {
                    operator = Some(value.to_string());
                }
            }
            other => {
                return Err(format!(
                "unknown field key {}; expected ipv4, ipv6, dnssec, filter, privacy, or operator",
                quote(other)
            ))
            }
        }
    }

    let dnssec = match dnssec {
        Some(value) => value,
        None => return Err("missing required dnssec=yes|no field".to_string()),
    };
    let filtering = match filtering {
        Some(value) => value,
        None => return Err("missing required filter=yes|no field".to_string()),
    };
    let privacy_policy = match privacy {
        Some(value) => value,
        None => return Err("missing required privacy field".to_string()),
    };

    let ipv4 = ipv4.unwrap_or_default();
    let ipv6 = ipv6.unwrap_or_default();
    if ipv4.is_empty() && ipv6.is_empty() {
        return Err("candidate has no resolver address in ipv4 or ipv6".to_string());
    }

    Ok(DnsCandidate {
        name: name.to_string(),
        ipv4,
        ipv6,
        operator,
        privacy_policy,
        supports_dnssec: dnssec,
        supports_filtering: filtering,
    })
}

/// Parse a boolean claim. An unrecognised value is an error, never `false`.
fn parse_flag(key: &str, value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        other => Err(format!(
            "{key} value {} is not one of yes/no, true/false, 1/0",
            quote(other)
        )),
    }
}

/// Parse a privacy policy claim.
fn parse_privacy_policy(value: &str) -> Result<DnsPrivacyPolicy, String> {
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "no-logging" | "no-log" | "nolog" => Ok(DnsPrivacyPolicy::DocumentedNoLogging),
        "undisclosed" => Ok(DnsPrivacyPolicy::Undisclosed),
        "unknown" => Ok(DnsPrivacyPolicy::Unknown),
        _ => {
            if let Some(days) = normalized.strip_prefix("retention:") {
                let days = days.trim();
                let parsed = days.parse::<u32>().map_err(|_| {
                    format!(
                        "retention days {} is not a non-negative integer",
                        quote(days)
                    )
                })?;
                Ok(DnsPrivacyPolicy::DocumentedRetention { days: parsed })
            } else {
                Err(format!(
                    "privacy value {} is not one of no-logging, retention:<days>, undisclosed, unknown",
                    quote(value)
                ))
            }
        }
    }
}

/// Parse a comma-separated address list and sanity-check its shape.
fn parse_addresses(value: &str, ipv4: bool) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let kind = if ipv4 { "IPv4" } else { "IPv6" };
    let mut addresses: Vec<String> = Vec::new();
    for raw in value.split(',') {
        let address = raw.trim();
        if address.is_empty() {
            return Err(format!("{kind} list contains an empty entry"));
        }
        if !fits(address, MAX_DNS_ADDRESS_CHARS) {
            return Err(format!(
                "{kind} address is longer than the {MAX_DNS_ADDRESS_CHARS}-character limit"
            ));
        }
        let plausible = if ipv4 {
            looks_like_ipv4(address)
        } else {
            looks_like_ipv6(address)
        };
        if !plausible {
            return Err(format!(
                "{} is not a plausible {kind} address",
                quote(address)
            ));
        }
        addresses.push(address.to_string());
    }
    Ok(addresses)
}

/// A structural IPv4 check: four decimal octets, each 0..=255.
fn looks_like_ipv4(value: &str) -> bool {
    let mut octets = 0usize;
    for part in value.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let octet = match part.parse::<u16>() {
            Ok(octet) => octet,
            Err(_) => return false,
        };
        if octet > 255 {
            return false;
        }
        octets = octets.saturating_add(1);
    }
    octets == 4
}

/// A structural IPv6 check, not a full RFC 4291 validator.
///
/// It accepts hex groups, a single `::` compression, an optional `%zone`,
/// and an optional embedded IPv4 tail. It does not resolve or contact the
/// address; `coresight-webtools` treats addresses as caller claims.
fn looks_like_ipv6(value: &str) -> bool {
    let (address, zone) = match value.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (value, None),
    };
    if let Some(zone) = zone {
        if zone.is_empty()
            || !zone
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return false;
        }
    }
    if !address.contains(':') || address.contains(":::") {
        return false;
    }
    if address.matches("::").count() > 1 {
        return false;
    }
    if !address
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
    {
        return false;
    }

    let mut units = 0usize;
    for part in address.split(':') {
        if part.is_empty() {
            continue;
        }
        if part.contains('.') {
            if !looks_like_ipv4(part) {
                return false;
            }
            units = units.saturating_add(2);
        } else {
            if part.len() > 4 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return false;
            }
            units = units.saturating_add(1);
        }
    }

    if address.contains("::") {
        units < 8
    } else {
        units == 8
    }
}

/// Sort candidates so input order cannot affect the result.
fn sort_candidates(candidates: &mut [DnsCandidate]) {
    candidates.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.ipv4.cmp(&right.ipv4))
            .then_with(|| left.ipv6.cmp(&right.ipv6))
            .then_with(|| left.operator.cmp(&right.operator))
            .then_with(|| left.privacy_policy.cmp(&right.privacy_policy))
            .then_with(|| left.supports_dnssec.cmp(&right.supports_dnssec))
            .then_with(|| left.supports_filtering.cmp(&right.supports_filtering))
    });
}

/// Quote a short excerpt of caller text for a reason string.
fn quote(text: &str) -> String {
    format!("\"{}\"", excerpt(text))
}
