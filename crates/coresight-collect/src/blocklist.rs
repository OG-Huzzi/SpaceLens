//! Block-list rule loading.
//!
//! ## Input format
//!
//! One rule per line: `domain[,category]`. Blank lines and `#` comments are
//! ignored; `#` starts a comment anywhere on the line, so a trailing comment
//! is accepted:
//!
//! ```text
//! # CoreSight custom list
//! ads.example.com,advertising
//! tracker.example.net,tracking   # analytics vendor
//! malware.example.org,malware
//! my-own-list.example,custom
//! ```
//!
//! Categories are ASCII case-insensitive: `advertising`, `tracking`,
//! `malware`, `adult`, `gambling`, `social_media` (also `social-media` and
//! `socialmedia`), and `custom`. An **unknown** category token is rejected
//! with its line number and the accepted list; it is never coerced to
//! `Custom` or any other permissive category. Omitting the category is a
//! different case — the grammar makes it optional and the only honest
//! reading is "the user did not categorize this rule", which is exactly
//! `DomainCategory::Custom`. That default is reported, not silent: the rule's
//! `reason` says so, and [`BlockParseResult::defaulted_categories`] counts
//! it.
//!
//! ## Domain validation belongs to `coresight-webtools`
//!
//! This loader does not re-implement domain rules. It splits the line and
//! passes every parsed rule to
//! [`plan_block_list`] with
//! [`BlockListOptions::default`]. The plan is the single authority on whether
//! a domain is valid, and every refused domain appears in
//! [`BlockListPlan::invalid_removed`](coresight_webtools::BlockListPlan::invalid_removed)
//! with the plan's reason. [`BlockParseResult::rejected`] therefore contains
//! only lines that never became a rule (bad shape, unknown category, NUL
//! byte, or an over-long line); a well-formed rule with an invalid domain is
//! present in `rules` and reported by the plan, not duplicated here.
//!
//! ## What this loader does not do
//!
//! It reads and writes no hosts file, no registry, and no other persistent
//! state. It prepares a reviewable plan; applying it is a separate,
//! explicitly authorized step in another layer. No I/O happens here.

use coresight_webtools::{
    plan_block_list, BlockListOptions, BlockListPlan, BlockRule, DomainCategory, RuleSource,
};
use serde::{Deserialize, Serialize};

use crate::text::{excerpt, fits, text_lines, LineReject};

/// Longest physical line this loader will consider.
///
/// A line longer than this is refused before it can reach the domain
/// validator, so a megabyte of junk does not become a megabyte of rejected
/// domain text in a report.
pub const MAX_BLOCK_LINE_CHARS: usize = 4096;

/// The result of loading block-list rules.
///
/// `rules` is ordered by domain and category, so permuting input lines cannot
/// change it. `plan` is the validated, de-duplicated, IPv4-and-IPv6-correct
/// plan produced by `coresight-webtools` with default options; callers that
/// need a whitelist, category filter, or cap should re-run `plan_block_list`
/// over `rules` with their own options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockParseResult {
    /// Rules that parsed, in canonical order.
    pub rules: Vec<BlockRule>,
    /// The validated plan for those rules, built with default options.
    pub plan: BlockListPlan,
    /// Lines that produced no rule, with line number and reason.
    pub rejected: Vec<LineReject>,
    /// How many accepted rules had no category and were recorded as `Custom`.
    pub defaulted_categories: usize,
}

/// Parse block-list text into rules and a validated plan.
///
/// This function is total: empty text, whitespace, NUL bytes, a megabyte of
/// junk, and deeply nested brackets all return a [`BlockParseResult`]. It
/// performs no I/O and cannot modify a hosts file.
pub fn parse_block_rules(text: &str, source: RuleSource) -> BlockParseResult {
    let mut rules: Vec<BlockRule> = Vec::new();
    let mut rejected: Vec<LineReject> = Vec::new();
    let mut defaulted_categories = 0usize;

    for (line_number, raw) in text_lines(text) {
        if raw.contains('\0') {
            rejected.push(LineReject::new(
                line_number,
                raw,
                "line contains a NUL byte",
            ));
            continue;
        }
        if !fits(raw, MAX_BLOCK_LINE_CHARS) {
            rejected.push(LineReject::new(
                line_number,
                raw,
                format!("line is longer than the {MAX_BLOCK_LINE_CHARS}-character limit"),
            ));
            continue;
        }

        let without_comment = match raw.split_once('#') {
            Some((before, _)) => before,
            None => raw,
        };
        let line = without_comment.trim();
        if line.is_empty() {
            continue;
        }

        match parse_block_line(line, &source) {
            Ok(parsed) => {
                if parsed.category_defaulted {
                    defaulted_categories = defaulted_categories.saturating_add(1);
                }
                rules.push(parsed.rule);
            }
            Err(reason) => rejected.push(LineReject::new(line_number, raw, reason)),
        }
    }

    sort_rules(&mut rules);
    let plan = plan_block_list(&rules, &BlockListOptions::default());

    rejected.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.reason.cmp(&right.reason))
    });

    BlockParseResult {
        rules,
        plan,
        rejected,
        defaulted_categories,
    }
}

/// One parsed line, plus whether its category was omitted.
struct ParsedBlockLine {
    rule: BlockRule,
    category_defaulted: bool,
}

/// Parse one non-blank, comment-stripped line.
fn parse_block_line(line: &str, source: &RuleSource) -> Result<ParsedBlockLine, String> {
    let mut fields = line.split(',');
    let domain = fields.next().unwrap_or("").trim().to_string();
    let category_field = fields.next().map(str::trim);
    if fields.next().is_some() {
        return Err(
            "expected domain[,category]; the line contains more than one comma".to_string(),
        );
    }

    let (category, category_defaulted) = match category_field {
        Some(token) if !token.is_empty() => {
            let category = parse_category(token).ok_or_else(|| {
                format!(
                    "unknown category {}; expected advertising, tracking, malware, adult, gambling, social_media, or custom",
                    quote(token)
                )
            })?;
            (category, false)
        }
        _ => (DomainCategory::Custom, true),
    };

    let reason = if category_defaulted {
        "Listed in the supplied block-list text; no category was supplied, so the rule is recorded as Custom (a user-defined category), not guessed into a built-in one.".to_string()
    } else {
        "Listed in the supplied block-list text.".to_string()
    };

    Ok(ParsedBlockLine {
        rule: BlockRule {
            domain,
            category,
            source: source.clone(),
            reason,
        },
        category_defaulted,
    })
}

/// Map one category token, or `None` when it is not a documented category.
fn parse_category(token: &str) -> Option<DomainCategory> {
    let normalized = token.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "advertising" => Some(DomainCategory::Advertising),
        "tracking" => Some(DomainCategory::Tracking),
        "malware" => Some(DomainCategory::Malware),
        "adult" => Some(DomainCategory::Adult),
        "gambling" => Some(DomainCategory::Gambling),
        "social_media" | "socialmedia" => Some(DomainCategory::SocialMedia),
        "custom" => Some(DomainCategory::Custom),
        _ => None,
    }
}

/// Sort rules so input order cannot affect the result.
fn sort_rules(rules: &mut [BlockRule]) {
    rules.sort_by(|left, right| {
        left.domain
            .to_ascii_lowercase()
            .cmp(&right.domain.to_ascii_lowercase())
            .then_with(|| left.domain.cmp(&right.domain))
            .then_with(|| {
                left.category
                    .protective_rank()
                    .cmp(&right.category.protective_rank())
            })
            .then_with(|| left.reason.cmp(&right.reason))
    });
}

/// Quote a short excerpt of caller text for a reason string.
fn quote(text: &str) -> String {
    format!("\"{}\"", excerpt(text))
}
