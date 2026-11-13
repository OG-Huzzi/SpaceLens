//! CoreSight **web and privacy tools** — the pure planning layer for three
//! jv16 PowerTools-style features, rebuilt around evidence and
//! reversibility (Phase 4.2).
//!
//! jv16 PowerTools offers a hosts-file adware blocker, an "Internet
//! Optimizer" that benchmarks DNS servers, and a browser-cleanup tool.
//! This crate deliberately stops one step earlier than each of them:
//!
//! * **Web blocker plan, never a hosts-file write.** [`plan_block_list`]
//!   turns caller-supplied [`BlockRule`]s into a reviewable
//!   [`BlockListPlan`]. Every blocked domain maps to **both** loopback
//!   addresses (`127.0.0.1` and `::1`) because a hosts file that only maps
//!   IPv4 is bypassed whenever the OS resolves the name over IPv6.
//!   [`plan_unblock`] names exactly the entries a later adapter would
//!   remove. Writing the hosts file is **not** in this crate: it is a
//!   later, separately authorized step with its own preview and undo
//!   evidence.
//! * **DNS ranking, never a DNS switch.** [`rank_dns`] ranks measured
//!   candidates with a published weighting and excludes candidates for
//!   stated reasons. Switching DNS changes which resolver sees every
//!   lookup on the device, so the choice remains the user's; this crate
//!   never changes it.
//! * **Privacy evidence, never a blind delete list.** [`summarize_privacy`]
//!   keeps finding kinds distinct and refuses to present a partial byte
//!   total as complete. Removing session cookies logs the user out, and the
//!   summary says so.
//!
//! ## Contracts honored here
//!
//! * **No I/O, no process, no network, no persistence.** There is no
//!   filesystem, hosts-file, registry, socket, subprocess, or database
//!   primitive in this crate. It accepts plain data and returns plain
//!   plans. A source-scan guard test proves it.
//! * **Never silently drops data.** Invalid domains are listed with a
//!   reason in [`BlockListPlan::invalid_removed`]; duplicates are counted;
//!   whitelist exclusions are named; plan truncation is counted; DNS
//!   candidates without usable evidence are listed in
//!   [`DnsRanking::excluded`] with a reason.
//! * **Whitelist wins.** A whitelisted domain is excluded even when a rule
//!   explicitly lists it for blocking, and the exclusion appears in
//!   [`BlockListPlan::whitelisted`].
//! * **Partial ≠ complete.** [`PrivacySummary::total_estimated_bytes`] is
//!   `None` when *any* finding lacks a size estimate. A partial byte total
//!   is never presented as the total.
//! * **Saturating arithmetic.** Every counter and byte total uses
//!   `saturating_add` / `saturating_mul`; no total can wrap to a smaller
//!   number.
//! * **Deterministic ordering.** Block entries are sorted by domain, DNS
//!   results by score then documented tie-breaks, and privacy findings by
//!   kind then detail. Permuted input produces identical output.
//! * **No panic paths in production code.** No panicking helper calls,
//!   macros, or indexing that can fail. Invalid caller data becomes
//!   reported evidence, never a crash.
//!
//! The UI (Phase 8) and the future hosts-file adapter consume these types;
//! they do not reimplement the rules here.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Longest legal DNS name, in bytes (RFC 1035 presentation limit).
pub const MAX_DOMAIN_LENGTH: usize = 253;

/// Longest legal DNS label, in bytes.
pub const MAX_LABEL_LENGTH: usize = 63;

/// The IPv4 loopback address every planned block entry maps to.
pub const BLOCK_IPV4: &str = "127.0.0.1";

/// The IPv6 loopback address every planned block entry maps to. Omitting
/// this is the classic hosts-blocker bypass: IPv6 resolution continues.
pub const BLOCK_IPV6: &str = "::1";

// ---------------------------------------------------------------------------
// Web blocker: categories, rules, and plan
// ---------------------------------------------------------------------------

/// Why a domain is proposed for blocking.
///
/// The category is carried into the plan so the user can review the mix
/// (and so a future UI can group entries) without treating every block as
/// equivalent. `Custom` is for rules the user wrote for their own reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DomainCategory {
    /// Advertising and ad-delivery domains.
    Advertising,
    /// Cross-site tracking, analytics, and fingerprinting domains.
    Tracking,
    /// Malware, phishing, and other clearly hostile domains.
    Malware,
    /// Adult content the user asked to block.
    Adult,
    /// Gambling domains the user asked to block.
    Gambling,
    /// Social-media domains the user asked to block (a focus choice,
    /// never an automatic judgement that the site is harmful).
    SocialMedia,
    /// A user-defined category outside the built-in taxonomy.
    Custom,
}

impl DomainCategory {
    /// Deterministic protective precedence used when one domain appears
    /// under several categories.
    ///
    /// Collapsing duplicates must not depend on input order, so the label
    /// with the highest rank wins: a domain known to be malware is never
    /// downgraded to "advertising" merely because the advertising rule
    /// arrived later. Ties are impossible across distinct categories.
    pub fn protective_rank(self) -> u8 {
        match self {
            DomainCategory::Malware => 6,
            DomainCategory::Adult => 5,
            DomainCategory::Gambling => 4,
            DomainCategory::Tracking => 3,
            DomainCategory::Advertising => 2,
            DomainCategory::SocialMedia => 1,
            DomainCategory::Custom => 0,
        }
    }
}

/// Where a blocking rule came from. Provenance is part of the rule; a plan
/// never hides that an entry was imported or supplied by a vendor list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum RuleSource {
    /// Written by the user in this app.
    UserDefined,
    /// Supplied by a named vendor list at a named version.
    VendorList {
        /// Human-readable list name, e.g. "CoreSight ads".
        list_name: String,
        /// Version identifier the rule was read from.
        list_version: String,
    },
    /// Imported from an external file or service.
    Imported {
        /// Where the rule came from (path, URL, or export name) for audit.
        origin: String,
    },
}

/// One requested block rule. Rules are input claims; [`plan_block_list`]
/// validates and normalizes them, and never trusts the domain as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockRule {
    /// The domain as supplied. It is validated and lowercased by the planner.
    pub domain: String,
    /// Why this rule exists.
    pub category: DomainCategory,
    /// Where the rule came from.
    pub source: RuleSource,
    /// Plain-language reason shown to the user.
    pub reason: String,
}

/// A domain the planner refused, with the reason it was refused.
///
/// Rejected input is reported, never silently dropped. Whitelist entries
/// that fail validation are also reported here, prefixed with
/// `invalid whitelist entry:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedDomain {
    /// The original text as supplied (not normalized).
    pub domain: String,
    /// Why it was not accepted.
    pub reason: String,
}

/// One entry a block plan would add to a hosts file.
///
/// Both loopback forms are present so the plan is IPv6-correct. The
/// planner writes neither line; it only proposes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockEntry {
    /// Normalized (lowercase) domain, without a trailing dot.
    pub domain: String,
    /// IPv4 loopback address for the domain.
    pub ipv4: String,
    /// IPv6 loopback address for the domain.
    pub ipv6: String,
    /// The most protective category supplied for this domain.
    pub category: DomainCategory,
}

/// Knobs for [`plan_block_list`]. Defaults include every category, no
/// whitelist, and no cap.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockListOptions {
    /// Domains that must never be blocked by this plan. Entries are
    /// validated like rule domains; invalid entries are reported in
    /// [`BlockListPlan::invalid_removed`]. Matching is exact after
    /// normalization — a whitelisted parent domain does not silently
    /// whitelist its subdomains.
    pub whitelist: Vec<String>,
    /// Categories to include. An empty list means **all** categories.
    /// A non-empty list is an explicit scope choice: rules outside it are
    /// intentionally not part of this plan (they are not invalid).
    pub include_categories: Vec<DomainCategory>,
    /// Maximum entries in the finished plan. `0` means no cap. When a cap
    /// applies, the omitted count is reported in
    /// [`BlockListPlan::truncated`]; the plan is truncated only after
    /// sorting, so the kept entries are deterministic.
    pub max_entries: usize,
}

/// A reviewable, reversible block-list plan. It contains no I/O and no
/// hosts-file text: it is the set of facts a later, separately authorized
/// writer would need.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockListPlan {
    /// Entries in ascending domain order. Each has both loopback addresses.
    pub entries: Vec<BlockEntry>,
    /// Number of entries in this plan (after validation, dedupe, category
    /// filtering, whitelist exclusion, and any cap). Equal to
    /// `entries.len()`.
    pub total: usize,
    /// Exact number of redundant rules collapsed into an existing entry.
    /// Only rules that survived validation, whitelisting, and category
    /// filtering can be duplicates.
    pub duplicates_removed: usize,
    /// Every rejected rule or whitelist entry, with its reason, sorted by
    /// original text then reason.
    pub invalid_removed: Vec<RejectedDomain>,
    /// Normalized whitelist domains that actually excluded at least one
    /// rule, sorted. Whitelist entries that matched nothing are not listed
    /// as exclusions because nothing was excluded.
    pub whitelisted: Vec<String>,
    /// Exact number of valid, distinct entries omitted because
    /// [`BlockListOptions::max_entries`] capped the plan. Zero when no cap
    /// applied. This exists so a bounded plan can never hide what it left
    /// out.
    #[serde(default)]
    pub truncated: usize,
}

/// Build a deterministic, reversible block plan from caller-supplied rules.
///
/// Validation is strict and mandatory: a domain must be non-empty, contain
/// no whitespace, `/`, `:`, or `*`, contain at least one dot, be at most
/// 253 bytes, and consist of labels that are 1..=63 bytes of ASCII
/// alphanumeric or hyphen and do not start or end with a hyphen. Uppercase
/// ASCII is normalized to lowercase. IDN labels must arrive in punycode
/// form because non-ASCII characters are rejected.
///
/// The function is pure: it performs no lookup, no DNS query, and no file
/// access. It returns a plan; applying it to a hosts file is a later,
/// separately authorized step.
pub fn plan_block_list(rules: &[BlockRule], options: &BlockListOptions) -> BlockListPlan {
    let mut invalid_removed: Vec<RejectedDomain> = Vec::new();

    // A whitelist entry that cannot be a domain can never match a rule.
    // Report it instead of pretending it participated.
    let mut whitelist: BTreeSet<String> = BTreeSet::new();
    for entry in &options.whitelist {
        match normalize_domain(entry) {
            Ok(domain) => {
                whitelist.insert(domain);
            }
            Err(reason) => invalid_removed.push(RejectedDomain {
                domain: entry.clone(),
                reason: format!("invalid whitelist entry: {reason}"),
            }),
        }
    }

    let category_filter_active = !options.include_categories.is_empty();
    let mut entries: BTreeMap<String, BlockEntry> = BTreeMap::new();
    let mut duplicates_removed = 0usize;
    let mut whitelisted: BTreeSet<String> = BTreeSet::new();

    for rule in rules {
        let domain = match normalize_domain(&rule.domain) {
            Ok(domain) => domain,
            Err(reason) => {
                invalid_removed.push(RejectedDomain {
                    domain: rule.domain.clone(),
                    reason,
                });
                continue;
            }
        };

        // Whitelist wins over everything, including an explicit block rule
        // and a category filter. The exclusion is reported.
        if whitelist.contains(&domain) {
            whitelisted.insert(domain);
            continue;
        }

        if category_filter_active && !options.include_categories.contains(&rule.category) {
            continue;
        }

        if let Some(existing) = entries.get_mut(&domain) {
            duplicates_removed = duplicates_removed.saturating_add(1);
            if rule.category.protective_rank() > existing.category.protective_rank() {
                existing.category = rule.category;
            }
        } else {
            entries.insert(
                domain.clone(),
                BlockEntry {
                    domain,
                    ipv4: BLOCK_IPV4.to_string(),
                    ipv6: BLOCK_IPV6.to_string(),
                    category: rule.category,
                },
            );
        }
    }

    // BTreeMap iteration is already domain-ascending.
    let mut entries: Vec<BlockEntry> = entries.into_values().collect();
    let mut truncated = 0usize;
    if options.max_entries > 0 && entries.len() > options.max_entries {
        truncated = entries.len().saturating_sub(options.max_entries);
        entries.truncate(options.max_entries);
    }
    let total = entries.len();

    // Deterministic regardless of rule order.
    invalid_removed.sort_by(|a, b| {
        a.domain
            .cmp(&b.domain)
            .then_with(|| a.reason.cmp(&b.reason))
    });

    BlockListPlan {
        entries,
        total,
        duplicates_removed,
        invalid_removed,
        whitelisted: whitelisted.into_iter().collect(),
        truncated,
    }
}

/// Validate and normalize one domain with the exact rules used by
/// [`plan_block_list`].
///
/// Returns the lowercase domain on success, or a human-readable reason on
/// failure. This is internal because the plan is the public evidence
/// surface; callers inspect [`BlockListPlan::invalid_removed`].
fn normalize_domain(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("empty domain".to_string());
    }
    if raw.chars().any(char::is_whitespace) {
        return Err("contains whitespace".to_string());
    }
    for forbidden in ['/', ':', '*'] {
        if raw.contains(forbidden) {
            return Err(format!("contains forbidden character {forbidden:?}"));
        }
    }

    let domain = raw.to_ascii_lowercase();
    if domain.len() > MAX_DOMAIN_LENGTH {
        let length = domain.len();
        return Err(format!(
            "domain is {length} bytes; the maximum is {MAX_DOMAIN_LENGTH}"
        ));
    }
    if !domain.contains('.') {
        return Err("must contain at least one dot".to_string());
    }
    for label in domain.split('.') {
        validate_label(label)?;
    }
    Ok(domain)
}

/// Validate one DNS label (the text between dots).
fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty() {
        return Err("contains an empty label".to_string());
    }
    let length = label.len();
    if length > MAX_LABEL_LENGTH {
        return Err(format!(
            "label {label:?} is {length} bytes; the maximum is {MAX_LABEL_LENGTH}"
        ));
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(format!(
            "label {label:?} must not start or end with a hyphen"
        ));
    }
    if let Some(character) = label
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-'))
    {
        return Err(format!(
            "label {label:?} contains invalid character {character:?}; only a-z, 0-9 and '-' are allowed"
        ));
    }
    Ok(())
}

/// The exact set of hosts-file entries a later adapter would remove to undo
/// a [`BlockListPlan`].
///
/// There is deliberately no byte buffer, no file handle, and no writing
/// here. The plan is the authorization subject, not the action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnblockPlan {
    /// Exactly the entries to remove, sorted by domain. No other domain is
    /// implied by this plan.
    pub entries: Vec<BlockEntry>,
    /// Number of entries to remove; equal to `entries.len()`.
    pub total: usize,
    /// Plain-language statement of scope, including the fact that writing
    /// the hosts file is a later, separately authorized step.
    pub note: String,
}

/// Build the reversible counterpart of a block plan.
///
/// The returned plan names exactly the entries that would be removed and
/// nothing else. It cannot and does not edit a hosts file: writing the file
/// is a later, separately authorized step with its own preview and undo
/// evidence. Keeping this as a pure data transformation makes the promise
/// testable — the unblock plan is a projection of the block plan.
pub fn plan_unblock(plan: &BlockListPlan) -> UnblockPlan {
    let mut entries = plan.entries.clone();
    entries.sort_by(|a, b| a.domain.cmp(&b.domain));
    let total = entries.len();
    let noun = if total == 1 { "entry" } else { "entries" };
    UnblockPlan {
        entries,
        total,
        note: format!(
            "Would remove exactly {total} hosts-file {noun}: the domains listed here and no others. \
             This is a plan only. CoreSight does not write, rewrite, or back up the hosts file in this crate; \
             writing the hosts file is a later, separately authorized step with its own preview and undo evidence. \
             Removing these entries restores the resolution path that existed before the block plan was applied."
        ),
    }
}

// ---------------------------------------------------------------------------
// DNS candidate ranking
// ---------------------------------------------------------------------------

/// Published weight of the latency component in [`rank_dns`] scores.
pub const DNS_LATENCY_WEIGHT: u32 = 40;

/// Published weight of the reliability (low failure-rate) component.
pub const DNS_RELIABILITY_WEIGHT: u32 = 30;

/// Published weight of the DNSSEC-support component.
pub const DNS_DNSSEC_WEIGHT: u32 = 15;

/// Published weight of the documented-privacy component.
pub const DNS_PRIVACY_WEIGHT: u32 = 15;

/// A latency at or below this value scores the full latency points.
pub const DNS_LATENCY_BEST_MS: f64 = 20.0;

/// A latency at or above this value scores zero latency points.
pub const DNS_LATENCY_WORST_MS: f64 = 300.0;

/// What a DNS operator publicly says about logging.
///
/// These are claims supplied by the caller, not something this crate can
/// verify (that would require network access, which is out of scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum DnsPrivacyPolicy {
    /// A published policy says query logs are not kept.
    DocumentedNoLogging,
    /// A published policy states a finite retention period.
    DocumentedRetention {
        /// Days of retention claimed by the policy.
        days: u32,
    },
    /// The operator does not disclose a policy. This is weaker evidence
    /// than a bad policy: it is simply unknown behaviour.
    Undisclosed,
    /// No information was supplied to CoreSight.
    Unknown,
}

/// One DNS resolver candidate. Addresses and capabilities are claims from
/// the caller; this crate never probes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsCandidate {
    /// Stable display name, also the key that matches [`DnsMeasurement`].
    pub name: String,
    /// Claimed IPv4 resolver addresses.
    pub ipv4: Vec<String>,
    /// Claimed IPv6 resolver addresses.
    pub ipv6: Vec<String>,
    /// Who operates the resolver, when known.
    pub operator: Option<String>,
    /// Claimed logging policy.
    pub privacy_policy: DnsPrivacyPolicy,
    /// Whether the resolver claims DNSSEC validation.
    pub supports_dnssec: bool,
    /// Whether the resolver claims content filtering. Not scored: filtering
    /// is a preference, not a quality measure.
    pub supports_filtering: bool,
}

/// One benchmark result for a named candidate.
///
/// `failure_rate` is a fraction in `0.0..=1.0`. Non-finite values are
/// treated as the worst possible measurement rather than trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsMeasurement {
    /// Candidate name; matched case-insensitively to [`DnsCandidate::name`].
    pub candidate_name: String,
    /// Median query latency in milliseconds.
    pub median_latency_ms: f64,
    /// 95th-percentile query latency in milliseconds.
    pub p95_latency_ms: f64,
    /// Fraction of failed queries (`0.0..=1.0`).
    pub failure_rate: f64,
    /// Number of samples behind the measurement.
    pub samples: usize,
}

/// Thresholds and privacy policy for [`rank_dns`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRankOptions {
    /// Minimum samples for a measurement to be usable evidence.
    pub min_samples: usize,
    /// Maximum tolerated failure rate. A measurement strictly above this
    /// is excluded as [`DnsExclusionReason::TooUnreliable`].
    pub max_failure_rate: f64,
    /// When true, candidates with [`DnsPrivacyPolicy::Undisclosed`] are
    /// excluded rather than ranked.
    pub require_documented_privacy: bool,
}

impl Default for DnsRankOptions {
    /// Conservative defaults: 10 samples, at most 5% failures, and a
    /// documented privacy policy required.
    fn default() -> Self {
        DnsRankOptions {
            min_samples: 10,
            max_failure_rate: 0.05,
            require_documented_privacy: true,
        }
    }
}

/// Why a DNS candidate was excluded from the ranking. Each variant carries
/// a precise, user-readable reason in [`ExcludedDns::detail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DnsExclusionReason {
    /// The measurement exists but has fewer samples than required.
    InsufficientEvidence,
    /// The failure rate is above the configured maximum (or non-finite).
    TooUnreliable,
    /// Logging is undisclosed while a documented policy is required.
    UndisclosedPrivacy,
    /// No measurement was supplied for this candidate.
    NotMeasured,
}

/// One excluded candidate, with the reason and the numbers behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedDns {
    /// Candidate name as supplied.
    pub name: String,
    /// Which rule excluded it.
    pub reason: DnsExclusionReason,
    /// Human-readable explanation including the actual values.
    pub detail: String,
}

/// One ranked DNS candidate, including the component scores that produced
/// the total so the ranking is explainable rather than a black box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedDns {
    /// Candidate name.
    pub name: String,
    /// Operator, when supplied.
    pub operator: Option<String>,
    /// Claimed IPv4 addresses.
    pub ipv4: Vec<String>,
    /// Claimed IPv6 addresses.
    pub ipv6: Vec<String>,
    /// Weighted total score, `0..=100`.
    pub score: u32,
    /// Latency component, `0..=100` (median and p95 averaged).
    pub latency_score: u32,
    /// Reliability component, `0..=100`.
    pub reliability_score: u32,
    /// DNSSEC component: `100` when supported, else `0`.
    pub dnssec_score: u32,
    /// Documented-privacy component, `0..=100`.
    pub privacy_score: u32,
    /// Median latency from the chosen measurement.
    pub median_latency_ms: f64,
    /// P95 latency from the chosen measurement.
    pub p95_latency_ms: f64,
    /// Failure rate from the chosen measurement.
    pub failure_rate: f64,
    /// Samples behind the chosen measurement.
    pub samples: usize,
    /// Privacy policy claim.
    pub privacy_policy: DnsPrivacyPolicy,
    /// Whether DNSSEC validation is claimed.
    pub supports_dnssec: bool,
    /// Whether content filtering is claimed.
    pub supports_filtering: bool,
}

/// The result of one DNS ranking pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRanking {
    /// Ranked candidates, best first. Sorted by score descending, then by
    /// name ascending, then by address/operator and measurement fields so
    /// equal scores still have a deterministic order.
    pub ranked: Vec<RankedDns>,
    /// Every excluded candidate, sorted by name then reason, with a detail
    /// string naming the actual values. Nothing is omitted silently.
    pub excluded: Vec<ExcludedDns>,
    /// Plain statement that this is guidance only and that CoreSight never
    /// switches DNS. The note also publishes the score weights.
    pub note: String,
}

/// Rank DNS candidates using measurements supplied by the caller.
///
/// ## Exclusion (all reported in [`DnsRanking::excluded`])
///
/// In precedence order: no measurement → [`DnsExclusionReason::NotMeasured`];
/// fewer than [`DnsRankOptions::min_samples`] samples →
/// [`DnsExclusionReason::InsufficientEvidence`]; failure rate strictly
/// above [`DnsRankOptions::max_failure_rate`] (or non-finite) →
/// [`DnsExclusionReason::TooUnreliable`]; privacy policy `Undisclosed` while
/// [`DnsRankOptions::require_documented_privacy`] is set →
/// [`DnsExclusionReason::UndisclosedPrivacy`].
///
/// ## Scoring (published, not hidden)
///
/// `score = 40% latency + 30% reliability + 15% DNSSEC + 15% documented
/// privacy`, using integer points. Latency maps median and p95 from 20 ms
/// (100 points) to 300 ms (0 points), then averages them. Reliability is
/// `(1 - failure_rate) * 100` with the rate clamped to `0..=1`. DNSSEC is
/// 100 or 0. Privacy is 100 for documented no-logging, 60..=100 for
/// documented retention (`100 - min(days, 80) / 2`), 50 for unknown, and 0
/// for undisclosed. Scores are evidence summaries, not guarantees.
///
/// When several measurement records share a candidate name, the record
/// with the most samples wins; ties fall back to lower p95, then lower
/// failure rate, then lower median. This makes the choice independent of
/// input order. Measurements match candidate names ASCII
/// case-insensitively.
///
/// The function performs no network access and never changes a resolver.
pub fn rank_dns(
    candidates: &[DnsCandidate],
    measurements: &[DnsMeasurement],
    options: &DnsRankOptions,
) -> DnsRanking {
    let best_by_name = best_measurements(measurements);

    let mut ranked: Vec<RankedDns> = Vec::with_capacity(candidates.len());
    let mut excluded: Vec<ExcludedDns> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let key = candidate.name.to_ascii_lowercase();
        let measurement = best_by_name.get(&key);
        let Some(measurement) = measurement else {
            excluded.push(ExcludedDns {
                name: candidate.name.clone(),
                reason: DnsExclusionReason::NotMeasured,
                detail: format!(
                    "no measurement was supplied for candidate {:?}; it cannot be ranked on evidence",
                    candidate.name
                ),
            });
            continue;
        };

        if let Some((reason, detail)) = screening_reason(candidate, measurement, options) {
            excluded.push(ExcludedDns {
                name: candidate.name.clone(),
                reason,
                detail,
            });
            continue;
        }

        let components = score_components(candidate, measurement);
        ranked.push(RankedDns {
            name: candidate.name.clone(),
            operator: candidate.operator.clone(),
            ipv4: candidate.ipv4.clone(),
            ipv6: candidate.ipv6.clone(),
            score: components.total(),
            latency_score: components.latency,
            reliability_score: components.reliability,
            dnssec_score: components.dnssec,
            privacy_score: components.privacy,
            median_latency_ms: measurement.median_latency_ms,
            p95_latency_ms: measurement.p95_latency_ms,
            failure_rate: measurement.failure_rate,
            samples: measurement.samples,
            privacy_policy: candidate.privacy_policy,
            supports_dnssec: candidate.supports_dnssec,
            supports_filtering: candidate.supports_filtering,
        });
    }

    // Higher score first. Name ascending is the documented primary
    // tie-break; the remaining fields make equal-score orders independent
    // of caller input order.
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.ipv4.cmp(&b.ipv4))
            .then_with(|| a.ipv6.cmp(&b.ipv6))
            .then_with(|| a.operator.cmp(&b.operator))
            .then_with(|| a.privacy_policy.cmp(&b.privacy_policy))
            .then_with(|| a.supports_dnssec.cmp(&b.supports_dnssec))
            .then_with(|| a.supports_filtering.cmp(&b.supports_filtering))
            .then_with(|| a.median_latency_ms.total_cmp(&b.median_latency_ms))
            .then_with(|| a.p95_latency_ms.total_cmp(&b.p95_latency_ms))
            .then_with(|| a.failure_rate.total_cmp(&b.failure_rate))
            .then_with(|| a.samples.cmp(&b.samples))
    });

    excluded.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.reason.cmp(&b.reason))
            .then_with(|| a.detail.cmp(&b.detail))
    });

    let ranked_count = ranked.len();
    let candidate_count = candidates.len();
    let excluded_count = excluded.len();
    let latency_weight = DNS_LATENCY_WEIGHT;
    let reliability_weight = DNS_RELIABILITY_WEIGHT;
    let dnssec_weight = DNS_DNSSEC_WEIGHT;
    let privacy_weight = DNS_PRIVACY_WEIGHT;
    let note = format!(
        "Ranked {ranked_count} of {candidate_count} candidate(s); {excluded_count} excluded and listed with a reason. \
         This ranking is guidance only: switching DNS changes which resolver sees every lookup from this device, \
         so the choice is the user's — CoreSight never switches DNS automatically and never contacts a resolver. \
         Score = {latency_weight}% latency + {reliability_weight}% reliability + {dnssec_weight}% DNSSEC + \
         {privacy_weight}% documented privacy; a high score is not a guarantee of speed, privacy, or uptime."
    );

    DnsRanking {
        ranked,
        excluded,
        note,
    }
}

/// Choose one measurement per candidate name, deterministically.
fn best_measurements(measurements: &[DnsMeasurement]) -> BTreeMap<String, DnsMeasurement> {
    let mut best: BTreeMap<String, DnsMeasurement> = BTreeMap::new();
    for measurement in measurements {
        let key = measurement.candidate_name.to_ascii_lowercase();
        let replace = match best.get(&key) {
            None => true,
            Some(existing) => measurement_is_better(measurement, existing),
        };
        if replace {
            best.insert(key, measurement.clone());
        }
    }
    best
}

/// Whether `candidate` is a better measurement record than `current`.
/// Most samples wins; ties prefer lower p95, lower failure rate, and lower
/// median. Non-finite values sort as worst rather than poisoning the
/// comparison.
fn measurement_is_better(candidate: &DnsMeasurement, current: &DnsMeasurement) -> bool {
    if candidate.samples != current.samples {
        return candidate.samples > current.samples;
    }
    let candidate_p95 = sortable(candidate.p95_latency_ms);
    let current_p95 = sortable(current.p95_latency_ms);
    if candidate_p95 != current_p95 {
        return candidate_p95 < current_p95;
    }
    let candidate_failure = sortable(candidate.failure_rate);
    let current_failure = sortable(current.failure_rate);
    if candidate_failure != current_failure {
        return candidate_failure < current_failure;
    }
    sortable(candidate.median_latency_ms) < sortable(current.median_latency_ms)
}

/// Map a float to a total-orderable value for tie-breaks; non-finite values
/// are treated as worst (positive infinity).
fn sortable(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        f64::INFINITY
    }
}

/// Apply the exclusion rules in documented precedence. Returns the first
/// failure only; the detail names the actual values so the user can see why.
fn screening_reason(
    candidate: &DnsCandidate,
    measurement: &DnsMeasurement,
    options: &DnsRankOptions,
) -> Option<(DnsExclusionReason, String)> {
    let samples = measurement.samples;
    let minimum = options.min_samples;
    if samples < minimum {
        return Some((
            DnsExclusionReason::InsufficientEvidence,
            format!("only {samples} sample(s); at least {minimum} are required"),
        ));
    }

    let failure_rate = measurement.failure_rate;
    if !failure_rate.is_finite() {
        return Some((
            DnsExclusionReason::TooUnreliable,
            "failure rate is not a finite number; the measurement cannot be trusted".to_string(),
        ));
    }
    // A failure rate is a fraction in 0..=1. A negative rate is not a great
    // reliability score — it is a malformed measurement, and scoring it as
    // `1 - (-0.5) = 150% reliable` would rank a broken server first.
    if !(0.0..=1.0).contains(&failure_rate) {
        return Some((
            DnsExclusionReason::TooUnreliable,
            format!(
                "failure rate {failure_rate} is outside the 0..=1 range a fraction must occupy"
            ),
        ));
    }
    let maximum = options.max_failure_rate;
    // A non-finite or out-of-range ceiling makes the comparison meaningless:
    // `rate > NaN` is false for every rate, so a NaN ceiling would silently
    // accept a server that fails every query. Refuse instead of accepting.
    if !maximum.is_finite() || !(0.0..=1.0).contains(&maximum) {
        return Some((
            DnsExclusionReason::TooUnreliable,
            format!(
                "the configured maximum failure rate {maximum} is not a fraction in 0..=1, so reliability cannot be judged"
            ),
        ));
    }
    if failure_rate > maximum {
        let rate_percent = failure_rate * 100.0;
        let maximum_percent = maximum * 100.0;
        return Some((
            DnsExclusionReason::TooUnreliable,
            format!("failure rate {rate_percent:.2}% exceeds the maximum {maximum_percent:.2}%"),
        ));
    }

    if options.require_documented_privacy
        && candidate.privacy_policy == DnsPrivacyPolicy::Undisclosed
    {
        return Some((
            DnsExclusionReason::UndisclosedPrivacy,
            "privacy policy is UNDISCLOSED while a documented policy is required".to_string(),
        ));
    }

    None
}

/// The four published score components for one candidate.
#[derive(Debug, Clone, Copy)]
struct ScoreComponents {
    latency: u32,
    reliability: u32,
    dnssec: u32,
    privacy: u32,
}

impl ScoreComponents {
    /// Weighted total using the published constants, integer arithmetic.
    fn total(&self) -> u32 {
        let weighted = self
            .latency
            .saturating_mul(DNS_LATENCY_WEIGHT)
            .saturating_add(self.reliability.saturating_mul(DNS_RELIABILITY_WEIGHT))
            .saturating_add(self.dnssec.saturating_mul(DNS_DNSSEC_WEIGHT))
            .saturating_add(self.privacy.saturating_mul(DNS_PRIVACY_WEIGHT));
        let weight_total =
            DNS_LATENCY_WEIGHT + DNS_RELIABILITY_WEIGHT + DNS_DNSSEC_WEIGHT + DNS_PRIVACY_WEIGHT;
        weighted / weight_total
    }
}

/// Compute the published component scores for a candidate that passed
/// screening.
fn score_components(candidate: &DnsCandidate, measurement: &DnsMeasurement) -> ScoreComponents {
    let median_points = latency_points(measurement.median_latency_ms);
    let p95_points = latency_points(measurement.p95_latency_ms);
    let latency = median_points.saturating_add(p95_points).saturating_add(1) / 2;

    let reliability = reliability_points(measurement.failure_rate);
    let dnssec = if candidate.supports_dnssec { 100 } else { 0 };
    let privacy = privacy_points(candidate.privacy_policy);

    ScoreComponents {
        latency,
        reliability,
        dnssec,
        privacy,
    }
}

/// Map one latency value to `0..=100` points. 20 ms or faster is 100;
/// 300 ms or slower (including non-finite or negative values) is 0.
fn latency_points(latency_ms: f64) -> u32 {
    if !latency_ms.is_finite() || latency_ms < 0.0 {
        return 0;
    }
    if latency_ms <= DNS_LATENCY_BEST_MS {
        return 100;
    }
    if latency_ms >= DNS_LATENCY_WORST_MS {
        return 0;
    }
    let span = DNS_LATENCY_WORST_MS - DNS_LATENCY_BEST_MS;
    let ratio = (DNS_LATENCY_WORST_MS - latency_ms) / span;
    (ratio * 100.0).round() as u32
}

/// Map a failure rate to reliability points. The rate is clamped to
/// `0..=1`; non-finite rates are treated as total failure.
fn reliability_points(failure_rate: f64) -> u32 {
    let rate = if failure_rate.is_finite() {
        failure_rate.clamp(0.0, 1.0)
    } else {
        1.0
    };
    ((1.0 - rate) * 100.0).round() as u32
}

/// Map a privacy policy claim to `0..=100` points, per the published table:
/// documented retention never scores below 60, so a published long
/// retention policy still outranks an undisclosed one.
fn privacy_points(policy: DnsPrivacyPolicy) -> u32 {
    match policy {
        DnsPrivacyPolicy::DocumentedNoLogging => 100,
        DnsPrivacyPolicy::DocumentedRetention { days } => 100u32.saturating_sub(days.min(80) / 2),
        DnsPrivacyPolicy::Unknown => 50,
        DnsPrivacyPolicy::Undisclosed => 0,
    }
}

// ---------------------------------------------------------------------------
// Privacy findings
// ---------------------------------------------------------------------------

/// What a privacy finding describes.
///
/// Session cookies are deliberately a distinct kind from tracking cookies:
/// removing a session cookie signs the user out, while removing a
/// persistent tracking cookie only requests consent again. Reporting them
/// as one "cookie" number would hide that consequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrivacyFindingKind {
    /// Persistent cross-site or analytics cookie.
    TrackingCookie,
    /// Session cookie: removing it logs the user out of a site.
    SessionCookie,
    /// Browsing-history record.
    BrowsingHistory,
    /// Cached resource entry.
    CacheEntry,
    /// Site storage such as localStorage, IndexedDB, or a service worker.
    SiteStorage,
    /// The kind could not be established; it is never guessed into
    /// another kind.
    Unknown,
}

/// One privacy finding. `estimated_bytes` is an estimate, not a
/// measurement, and `None` means "unknown" — not zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyFinding {
    /// What was found.
    pub kind: PrivacyFindingKind,
    /// How many items of this kind were found.
    pub count: u64,
    /// Estimated bytes, or `None` when no honest estimate exists.
    pub estimated_bytes: Option<u64>,
    /// Specific context shown to the user.
    pub detail: String,
}

/// A deterministic, evidence-only summary of privacy findings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacySummary {
    /// Findings sorted by kind, then detail, then byte estimate, then
    /// count so permuted input gives identical output.
    pub findings: Vec<PrivacyFinding>,
    /// Sum of every finding's count (saturating).
    pub total_count: u64,
    /// Sum of known byte estimates, or `None` when **any** finding has no
    /// estimate. `Some(0)` for an empty input is a complete total; `None`
    /// is never replaced by a partial sum.
    pub total_estimated_bytes: Option<u64>,
    /// Whether any finding had [`PrivacyFindingKind::Unknown`].
    pub has_unknown_kinds: bool,
    /// Plain-language caveats: evidence only, session-cookie consequence,
    /// and an explicit statement when the byte total is unavailable.
    pub note: String,
}

/// Summarize privacy findings without deleting anything.
///
/// The summary is evidence for a review screen. It never produces a delete
/// list and never treats an unknown size as zero: if any finding has
/// `estimated_bytes: None`, [`PrivacySummary::total_estimated_bytes`] is
/// `None` and the note says why. Session cookies stay separate from
/// tracking cookies, and the note states the sign-out consequence.
pub fn summarize_privacy(findings: &[PrivacyFinding]) -> PrivacySummary {
    let mut ordered: Vec<PrivacyFinding> = findings.to_vec();
    ordered.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.detail.cmp(&b.detail))
            .then_with(|| a.estimated_bytes.cmp(&b.estimated_bytes))
            .then_with(|| a.count.cmp(&b.count))
    });

    let mut total_count = 0u64;
    let mut total_estimated_bytes: Option<u64> = Some(0);
    let mut has_unknown_kinds = false;

    for finding in &ordered {
        total_count = total_count.saturating_add(finding.count);
        total_estimated_bytes = match finding.estimated_bytes {
            Some(bytes) => total_estimated_bytes.map(|total| total.saturating_add(bytes)),
            None => None,
        };
        if finding.kind == PrivacyFindingKind::Unknown {
            has_unknown_kinds = true;
        }
    }

    let mut note = String::from(
        "These are evidence counts, not a delete list: nothing is removed by this summary. \
         Session cookies are kept distinct from persistent/tracking cookies because removing session cookies \
         logs the user out of sites — it signs you out — while persistent tracking cookies usually require only \
         a fresh consent.",
    );
    if total_estimated_bytes.is_none() {
        note.push_str(
            " The byte total is not reported because at least one finding has no size estimate; \
             a partial sum must never be presented as complete.",
        );
    }
    if has_unknown_kinds {
        note.push_str(
            " Some findings have an unknown kind and are labelled UNKNOWN rather than guessed into another kind.",
        );
    }
    if ordered.is_empty() {
        note.push_str(" No findings were supplied.");
    }

    PrivacySummary {
        findings: ordered,
        total_count,
        total_estimated_bytes,
        has_unknown_kinds,
        note,
    }
}
