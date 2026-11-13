//! The ten aggregate view-model payloads of `coresight.v1.*`.
//!
//! These types mirror `src/api.ts` **field for field**. The TypeScript module
//! is the specification: key names, optionality, and nullability are taken
//! from it verbatim, and no extra field is invented. Every byte total that the
//! interface types as `number | null` is an `Option<u64>` here and is `None`
//! whenever a contributing observation is unknown — never `0`.
//!
//! The payloads are projections, not new facts:
//!
//! * scan-derived views (`getGlobalStatus`, `getHomeSummary`, `getStorage`,
//!   `getCleanup`, `getDuplicateReport`, `getBigFileReport`) are computed from
//!   the latest completed scan registered in the [`Session`](crate::Session);
//! * startup, DNS, block-list, and privacy views are projections of the
//!   inventories/rankings/plans registered by the shell — this crate never
//!   enumerates a machine, queries a resolver, or writes a hosts file.
//!
//! Nothing here performs I/O or reads a clock. Ages are measured from the
//! scan's completion instant, and `generatedAt` is that same registered
//! instant, because this crate has no clock of its own.

use serde::{Deserialize, Serialize};

/// The contract major this build speaks, mirrored from `CONTRACT_VERSION`.
pub const CONTRACT_VERSION: &str = "v1";

/// Source completeness of an aggregate (`complete|partial|unknown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataCompleteness {
    /// Every contributing observation succeeded and was measured.
    Complete,
    /// Some contributing observations are missing or unmeasured.
    Partial,
    /// The build cannot state how complete the aggregate is.
    Unknown,
}

/// Source-quality metadata attached to every aggregate that could be partial.
///
/// `unreadable_entries: None` means the count itself is unknown, not zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataCoverage {
    /// Overall completeness of the aggregate.
    pub completeness: DataCompleteness,
    /// Entries that were seen but could not be fully read. `None` = unknown.
    pub unreadable_entries: Option<u64>,
    /// Entries whose size was never observed. `None` = unknown.
    pub unknown_size_entries: Option<u64>,
    /// Human labels for areas the producing run excluded. Never paths.
    pub excluded_areas: Vec<String>,
    /// Optional plain-language caveat.
    pub note: Option<String>,
}

/// Lifecycle state of the scan shown by `getGlobalStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanStateView {
    /// No scan is running and none has completed.
    Idle,
    /// A scan is running.
    Scanning,
    /// A scan completed over its full declared scope.
    Complete,
    /// A scan finished but some observations are missing or unmeasured.
    Partial,
    /// A scan failed.
    Failed,
}

/// The current scan status (`ScanStatus` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatusView {
    /// Scan id, when a scan exists in this session.
    pub scan_id: Option<String>,
    /// Lifecycle state.
    pub state: ScanStateView,
    /// Start instant. `None` when the producing run did not record one.
    pub started_at: Option<String>,
    /// Completion instant, when one was recorded.
    pub completed_at: Option<String>,
    /// Entries seen so far, when a scan exists.
    pub files_seen: Option<u64>,
    /// Bytes seen with a known size, when the scan is complete.
    pub bytes_seen: Option<u64>,
    /// Area currently being scanned; this build records none.
    pub current_area: Option<String>,
    /// Estimated seconds remaining; this build records none.
    pub eta_seconds: Option<u64>,
    /// Coverage of the scan.
    pub coverage: DataCoverage,
}

/// The `getGlobalStatus` payload (`GlobalStatus` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStatusView {
    /// Contract major (`v1`).
    pub contract_version: String,
    /// Status of the most recent scan registered in this session.
    pub scan: ScanStatusView,
    /// True only when the payload is deterministic fixture data. This
    /// dispatcher never returns fixture data, so it is `false`.
    pub simulated_data: bool,
}

/// One display location attached to a category for progressive disclosure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageLocation {
    /// Human label for the location.
    pub label: String,
    /// Engine-provided display text. Never opened, joined, or resolved.
    pub display_path: String,
    /// Known bytes at this location. `None` when the entry's size is unknown.
    pub bytes: Option<u64>,
}

/// A category row plus the evidence needed for progressive disclosure.
///
/// Mirrors `CategoryTotals` in `src/api.ts`. `bytes` and `logicalSize` are
/// `None` when any entry in the bucket had an unknown size or an observation
/// error, because a partial byte sum must never be presented as the bucket's
/// total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotals {
    /// Stable category or subcategory code.
    pub id: String,
    /// Human-readable name for display.
    pub name: String,
    /// Measured logical bytes. `None` when the bucket is incomplete.
    pub bytes: Option<u64>,
    /// `bytes` divided by the parent total. `None` when either side is
    /// unknown or the parent is zero.
    pub share_of_parent: Option<f64>,
    /// Change versus the previous full-scope scan. `None` when no comparable
    /// baseline exists or either side is incomplete.
    pub delta_since_last: Option<i64>,
    /// Number of entries in this bucket, when known.
    pub item_count: Option<u64>,
    /// Total entries in this bucket.
    pub entries: Option<u64>,
    /// File entries in this bucket.
    pub files: Option<u64>,
    /// Directory entries in this bucket.
    pub directories: Option<u64>,
    /// Same byte value as `bytes`; kept because `CategoryTotals` extends
    /// `Category` and the interface exposes both.
    pub logical_size: Option<u64>,
    /// Child rows for stored subcategories.
    pub children: Vec<CategoryTotals>,
    /// Entries in this bucket whose size was unknown.
    pub entries_without_size: Option<u64>,
    /// Entries in this bucket with an observation error.
    pub observation_errors: Option<u64>,
    /// Deterministic sample locations from the snapshot.
    pub sample_locations: Vec<StorageLocation>,
}

/// One member of a hash-verified duplicate group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMemberView {
    /// Display label from the engine. Never opened or resolved.
    pub display_path: String,
    /// Known size shared by the group.
    pub size_bytes: Option<u64>,
    /// True when this member is another name for an already-counted object.
    pub is_hardlink_alias: bool,
}

/// One set of entries proven to hold identical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupView {
    /// Deterministic group id derived from the verified content hash.
    pub id: String,
    /// Verified SHA-256 content hash shared by every member.
    pub content_sha256: String,
    /// Always true: a group is emitted only from verified content identity.
    pub hash_verified: bool,
    /// Known size shared by every member.
    pub size_bytes: Option<u64>,
    /// Members in canonical path order.
    pub members: Vec<DuplicateMemberView>,
    /// Bytes recoverable from this group, or `None` when it cannot be
    /// stated honestly.
    pub recoverable_bytes: Option<u64>,
}

/// The duplicate report (`DuplicateReport` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReportView {
    /// Published groups, in the filetools crate's documented order.
    pub groups: Vec<DuplicateGroupView>,
    /// Number of qualifying groups before truncation.
    pub total_groups: Option<u64>,
    /// Recoverable bytes across all qualifying groups.
    pub total_recoverable_bytes: Option<u64>,
    /// Files whose content identity was never verified.
    pub unverified_entries: Option<u64>,
    /// Files with a verified hash but no observed size.
    pub unknown_size_entries: Option<u64>,
    /// Groups omitted by the publication cap.
    pub truncated_groups: Option<u64>,
    /// Coverage of the duplicate analysis.
    pub coverage: DataCoverage,
}

/// One file that satisfies the big-file criteria.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BigFileView {
    /// Deterministic display id for this row.
    pub id: String,
    /// Display label from the engine. Never opened or resolved.
    pub display_path: String,
    /// Observed logical size.
    pub size_bytes: Option<u64>,
    /// Observed modification instant, RFC 3339 UTC.
    pub modified_at: Option<String>,
    /// Age in whole days at the scan's completion instant. `None` when the
    /// modification time or the reference instant is unknown.
    pub age_days: Option<u64>,
    /// Human category name, or `None` when the run recorded no usable code.
    pub category_name: Option<String>,
}

/// The big-file report (`BigFileReport` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BigFileReportView {
    /// Candidates in descending size order.
    pub files: Vec<BigFileView>,
    /// Minimum size a file needed to be listed.
    pub threshold_bytes: Option<u64>,
    /// Listed candidates whose age could not be established.
    pub unknown_age_entries: Option<u64>,
    /// Candidates omitted by the result cap.
    pub truncated: Option<u64>,
    /// Coverage of the underlying scan.
    pub coverage: DataCoverage,
}

/// The `getStorage` payload (`StorageData` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageDataView {
    /// Every category, in `Category::ALL` order.
    pub categories: Vec<CategoryTotals>,
    /// Total observed bytes. `None` when the scan is incomplete.
    pub total_observed_bytes: Option<u64>,
    /// Every entry seen, classified or not.
    pub total_observed_items: Option<u64>,
    /// Duplicate report projected from the same snapshot.
    pub duplicate_report: DuplicateReportView,
    /// Big-file report projected from the same snapshot.
    pub big_file_report: BigFileReportView,
    /// Coverage of the underlying scan.
    pub coverage: DataCoverage,
}

/// One piece of evidence supporting a cleanup item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupEvidence {
    /// Evidence family: `observation`, `verification`, or `safety`.
    pub kind: CleanupEvidenceKind,
    /// Short label for the evidence row.
    pub label: String,
    /// Plain-language detail.
    pub detail: String,
}

/// The evidence family of a cleanup item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupEvidenceKind {
    /// Something the scan observed.
    Observation,
    /// A verification step that was (or was not) performed.
    Verification,
    /// A safety property of the proposed action.
    Safety,
}

/// Safety tier exposed by the cleanup view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CleanupTier {
    /// Safe to act on with one confirmation.
    Safe,
    /// Requires the user to read the reasoning first.
    Review,
    /// Exposed for understanding, never actionable.
    OffLimits,
}

/// One cleanup opportunity or off-limits category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItemView {
    /// Deterministic item id.
    pub id: String,
    /// Stable category code this item covers.
    pub category_id: String,
    /// Safety tier.
    pub tier: CleanupTier,
    /// Short title for the UI card.
    pub title: String,
    /// What this item covers, in plain language.
    pub what_it_is: String,
    /// Why it is (or is not) proposed.
    pub why: String,
    /// Bytes observed in the bucket. `None` when the bucket is incomplete.
    pub observed_bytes: Option<u64>,
    /// Bytes this item could recover. `None` when that amount is unknown.
    pub recoverable_bytes: Option<u64>,
    /// Number of entries in the bucket.
    pub item_count: Option<u64>,
    /// Age evidence. `None` means no trustworthy age evidence was recorded.
    pub estimated_age_days: Option<u64>,
    /// What stays untouched if the user acts.
    pub what_stays_untouched: String,
    /// What happens after the move.
    pub consequence: String,
    /// Supporting evidence rows.
    pub evidence: Vec<CleanupEvidence>,
    /// Whether a preview can be built for this item.
    pub previewable: bool,
    /// Why an off-limits item can never be acted on.
    pub off_limits_reason: Option<String>,
}

/// The `getCleanup` payload (`CleanupData` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupDataView {
    /// Scan these opportunities come from.
    pub scan_id: Option<String>,
    /// Completion instant of that scan.
    pub last_scan_at: Option<String>,
    /// Actionable opportunities, in the planner's canonical order.
    pub items: Vec<CleanupItemView>,
    /// Categories exposed but never proposed, in `Category::ALL` order.
    pub off_limits: Vec<CleanupItemView>,
    /// Recoverable bytes across ready-tier items. `None` when the scan is
    /// partial.
    pub safe_recoverable_bytes: Option<u64>,
    /// Recoverable bytes across review-tier items. `None` when the scan is
    /// partial.
    pub review_recoverable_bytes: Option<u64>,
    /// Coverage of the underlying scan.
    pub coverage: DataCoverage,
}

/// One category's change between two full-scope scans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDeltaView {
    /// Stable category code.
    pub category_id: String,
    /// Human-readable category name.
    pub name: String,
    /// Signed byte change, or `None` when either side is incomplete.
    pub bytes_delta: Option<i64>,
    /// Signed entry-count change, or `None` when either side is incomplete.
    pub entries_delta: Option<i64>,
    /// Byte total in the newer scan, when known.
    pub new_bytes: Option<u64>,
    /// Byte total in the older scan, when known.
    pub old_bytes: Option<u64>,
    /// True when the comparison is a lower-bound statement.
    pub partial: bool,
}

/// The comparison between the latest two full-scope scans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSinceLastScanView {
    /// Id of the earlier scan.
    pub from_snapshot_id: Option<String>,
    /// Id of the later scan.
    pub to_snapshot_id: Option<String>,
    /// Completion instant of the earlier scan.
    pub from_at: Option<String>,
    /// Completion instant of the later scan.
    pub to_at: Option<String>,
    /// Signed total-byte change, or `None` when either side is incomplete.
    pub total_bytes_delta: Option<i64>,
    /// Per-category changes in `Category::ALL` order.
    pub category_deltas: Vec<CategoryDeltaView>,
    /// Combined coverage of both scans.
    pub coverage: DataCoverage,
    /// Plain-language caveat.
    pub note: Option<String>,
}

/// Volume family exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveKindView {
    /// Fixed internal volume.
    Internal,
    /// Everything else (removable, network, or unclassified).
    External,
}

/// Whether a drive is mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveConnectionView {
    /// Currently mounted.
    Connected,
    /// Remembered but not mounted.
    Remembered,
}

/// Health band exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveHealthView {
    /// No health concern is known.
    Healthy,
    /// Something needs attention.
    Attention,
    /// The build has no health evidence.
    Unknown,
}

/// A drive row for the home screen (`Drive` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveView {
    /// Stable drive id.
    pub id: String,
    /// Display name (volume label, falling back to the mount root).
    pub name: String,
    /// Internal or external.
    pub kind: DriveKindView,
    /// Connected or remembered.
    pub connection: DriveConnectionView,
    /// Health band. This build records no health policy, so it is unknown.
    pub health: DriveHealthView,
    /// Capacity when the platform reported it.
    pub total_bytes: Option<u64>,
    /// Used bytes, derivable only when both capacity and free space are known.
    pub used_bytes: Option<u64>,
    /// Free bytes when the platform reported them.
    pub free_bytes: Option<u64>,
    /// Used fraction in `0..1`, when it can be computed honestly.
    pub used_share: Option<f64>,
    /// Completion instant of the drive's latest scan.
    pub last_scan_at: Option<String>,
    /// Number of retained snapshots for this drive.
    pub snapshot_count: Option<u64>,
    /// Optional note; this build records none.
    pub note: Option<String>,
}

/// The `getHomeSummary` payload (`HomeSummary` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeSummaryView {
    /// Instant this summary is generated from. This build has no clock, so it
    /// is the completion instant of the newest full-scope scan.
    pub generated_at: String,
    /// Completion instant of the newest full-scope scan.
    pub last_scan_at: Option<String>,
    /// Recoverable bytes across safe-tier items, or `None` when incomplete.
    pub safe_reclaimable_bytes: Option<u64>,
    /// Recoverable bytes across review-tier items, or `None` when incomplete.
    pub review_reclaimable_bytes: Option<u64>,
    /// Number of actionable opportunities.
    pub actionable_opportunity_count: Option<u64>,
    /// Known drives, connected first then by id.
    pub drives: Vec<DriveView>,
    /// The three largest categories with known bytes, in `Category::ALL`
    /// order.
    pub top_categories: Vec<CategoryTotals>,
    /// Comparison with the previous full-scope scan, when one exists.
    pub change_since_last_scan: Option<ChangeSinceLastScanView>,
    /// Coverage of the newest full-scope scan.
    pub coverage: DataCoverage,
}

/// Startup classification tier exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartupTierView {
    /// Safe to disable after one confirmation.
    SafeToDisable,
    /// Review the evidence first.
    ReviewFirst,
    /// Never offered for change.
    OffLimits,
}

/// One analysed startup item (`StartupItem` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupItemView {
    /// Stable identifier within the collecting source.
    pub id: String,
    /// Name shown by the source.
    pub name: String,
    /// Whether the source currently starts the item.
    pub enabled: bool,
    /// Human label for the source family.
    pub source_label: String,
    /// Recorded command line, when the source records one.
    pub command_label: Option<String>,
    /// Owning account, when known.
    pub owner: Option<String>,
    /// Classification tier.
    pub tier: StartupTierView,
    /// The evidence behind the tier, in plain language.
    pub why: String,
}

/// The `getStartupReport` payload (`StartupReport` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupReportView {
    /// Explained items in the systools crate's canonical order.
    pub items: Vec<StartupItemView>,
    /// Enabled items in the whole supplied inventory.
    pub total_enabled: Option<u64>,
    /// Findings omitted by a publication cap.
    pub truncated: Option<u64>,
    /// Coverage of the supplied inventory.
    pub coverage: DataCoverage,
}

/// Documented DNS logging policy (`DnsPrivacyPolicy` in `src/api.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DnsPrivacyPolicyView {
    /// A published policy says query logs are not kept.
    DocumentedNoLogging,
    /// A published policy states a finite retention period.
    DocumentedRetention,
    /// The operator does not disclose a policy.
    Undisclosed,
    /// No information was supplied.
    Unknown,
}

/// One ranked DNS candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRankedCandidate {
    /// Candidate name.
    pub name: String,
    /// Operator, when supplied.
    pub operator: Option<String>,
    /// Weighted total score.
    pub score: Option<u32>,
    /// Latency component.
    pub latency_score: Option<u32>,
    /// Reliability component.
    pub reliability_score: Option<u32>,
    /// DNSSEC component.
    pub dnssec_score: Option<u32>,
    /// Documented-privacy component.
    pub privacy_score: Option<u32>,
    /// Median latency in milliseconds.
    pub median_latency_ms: Option<f64>,
    /// P95 latency in milliseconds.
    pub p95_latency_ms: Option<f64>,
    /// Failure rate in `0..1`.
    pub failure_rate: Option<f64>,
    /// Samples behind the measurement.
    pub samples: Option<u64>,
    /// Logging policy claim.
    pub privacy_policy: DnsPrivacyPolicyView,
    /// Whether DNSSEC validation is claimed.
    pub supports_dnssec: Option<bool>,
    /// Whether content filtering is claimed.
    pub supports_filtering: Option<bool>,
}

/// Why a DNS candidate was excluded from the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DnsExclusionReasonView {
    /// Too few samples to rank.
    InsufficientEvidence,
    /// Failure rate above the threshold.
    TooUnreliable,
    /// Logging policy undisclosed while a documented policy is required.
    UndisclosedPrivacy,
    /// No measurement was supplied.
    NotMeasured,
}

/// One excluded DNS candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsExcludedCandidate {
    /// Candidate name.
    pub name: String,
    /// Screening rule that excluded it.
    pub reason: DnsExclusionReasonView,
    /// Human-readable explanation including the measured values.
    pub detail: String,
}

/// The `getDnsRanking` payload (`DnsRanking` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRankingView {
    /// Ranked candidates, best first.
    pub ranked: Vec<DnsRankedCandidate>,
    /// Excluded candidates, each with a reason.
    pub excluded: Vec<DnsExcludedCandidate>,
    /// Guidance-only statement including the score weights.
    pub note: String,
    /// Coverage of the supplied candidates.
    pub coverage: DataCoverage,
}

/// One planned hosts-file entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockListEntryView {
    /// Normalised (lowercase) domain.
    pub domain: String,
    /// Most protective category supplied for the domain.
    pub category: String,
    /// IPv4 loopback address.
    pub ipv4: String,
    /// IPv6 loopback address.
    pub ipv6: String,
}

/// One rejected rule or whitelist entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedDomainView {
    /// Original text as supplied.
    pub domain: String,
    /// Why it was refused.
    pub reason: String,
}

/// The `getBlockListPlan` payload (`BlockListPlan` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockListPlanView {
    /// Entries in ascending domain order.
    pub entries: Vec<BlockListEntryView>,
    /// Number of entries in this plan.
    pub total: Option<u64>,
    /// Redundant rules collapsed into an existing entry.
    pub duplicates_removed: Option<u64>,
    /// Every rejected rule, with its reason.
    pub invalid_removed: Vec<RejectedDomainView>,
    /// Whitelist domains that excluded at least one rule.
    pub whitelisted: Vec<String>,
    /// Valid entries omitted by the plan cap.
    pub truncated: Option<u64>,
    /// Always true: the plan is reversible and writes nothing.
    pub reversible: bool,
    /// Plain-language statement of the plan's scope.
    pub note: String,
    /// Coverage of the supplied rules.
    pub coverage: DataCoverage,
}

/// What a privacy finding describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrivacyFindingKindView {
    /// Persistent cross-site or analytics cookie.
    TrackingCookie,
    /// Session cookie; removing it signs the user out.
    SessionCookie,
    /// Browsing-history record.
    BrowsingHistory,
    /// Cached resource entry.
    CacheEntry,
    /// Site storage such as localStorage or IndexedDB.
    SiteStorage,
    /// The kind could not be established.
    Unknown,
}

/// One privacy finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyFindingView {
    /// What was found.
    pub kind: PrivacyFindingKindView,
    /// How many items of this kind were found.
    pub count: Option<u64>,
    /// Estimated bytes, or `None` when no honest estimate exists.
    pub estimated_bytes: Option<u64>,
    /// Specific context shown to the user.
    pub detail: String,
}

/// The `getPrivacySummary` payload (`PrivacySummary` in `src/api.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacySummaryView {
    /// Findings in the webtools crate's canonical order.
    pub findings: Vec<PrivacyFindingView>,
    /// Sum of every finding's count.
    pub total_count: Option<u64>,
    /// Sum of known byte estimates. `None` when any finding has no estimate.
    pub total_estimated_bytes: Option<u64>,
    /// Whether any finding had an unknown kind.
    pub has_unknown_kinds: bool,
    /// Plain-language caveats; evidence only, never a delete list.
    pub note: String,
    /// Coverage of the supplied findings.
    pub coverage: DataCoverage,
}
