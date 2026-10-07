//! Application discovery (Phase 6, Objective 13): a platform-neutral
//! provider model with a Windows Win32-registry implementation.

use std::collections::BTreeMap;

use crate::domain::{
    ApplicationRecord, ApplicationSource, DiscoveryLimits, Inventory, PackageKind, SourceCoverage,
    SourceStatus,
};

/// A source of application records.
pub trait ApplicationProvider {
    /// Source tag (e.g. `"win32-uninstall"`).
    fn source_tag(&self) -> &'static str;
    /// Enumerate. `Ok` may be an honestly-empty list only when the
    /// returned coverage says the source was genuinely read; a source
    /// that could not be read must return `Err` (or override
    /// [`ApplicationProvider::enumerate_outcome`] to report its honest
    /// partial coverage) — never a silent empty success.
    fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError>;

    /// The honest full result: records together with the coverage that
    /// describes how they were obtained. The default implementation maps
    /// `Ok` to `Complete` coverage and each error kind to its explicit
    /// non-success status. Providers whose "success" may itself be
    /// partial (some views read, others failed) override this.
    fn enumerate_outcome(&self) -> ProviderOutcome {
        match self.enumerate() {
            Ok(records) => ProviderOutcome {
                records,
                coverage: SourceCoverage::complete(self.source_tag()),
            },
            Err(ProviderError::Unsupported(note)) => ProviderOutcome {
                records: Vec::new(),
                coverage: SourceCoverage::with_status(
                    self.source_tag(),
                    SourceStatus::Unsupported,
                    Some(note),
                ),
            },
            Err(ProviderError::Failed(note)) => ProviderOutcome {
                records: Vec::new(),
                coverage: SourceCoverage::with_status(
                    self.source_tag(),
                    SourceStatus::Failed,
                    Some(note),
                ),
            },
        }
    }
}

/// Packaged applications (MSIX/AppX) provider — abstracted so the
/// domain model never becomes Windows-registry-only.
pub trait PackagedAppProvider: ApplicationProvider {
    fn package_source(&self) -> &'static str;
}

/// Why enumeration failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The platform cannot service this source (e.g. Appx on a
    /// non-Windows host, or a Windows build whose Appx inventory is
    /// not yet implemented).
    Unsupported(String),
    /// The source was queried and could not be read. Never an empty
    /// success.
    Failed(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Unsupported(m) => write!(f, "unsupported: {m}"),
            ProviderError::Failed(m) => write!(f, "failed: {m}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// One provider's output: records plus the coverage that describes how
/// they were obtained. `merge_inventory` consumes only this type, so a
/// failed or unsupported source can never enter an inventory as an
/// unqualified empty result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutcome {
    pub records: Vec<ApplicationRecord>,
    pub coverage: SourceCoverage,
}

/// Canonical precedence between two records competing for one logical
/// application: the more complete metadata wins; exact completeness ties
/// break on a fixed field-by-field content order. Provider call order is
/// NEVER a tie-breaker — both arguments produce the same winner under any
/// arrival permutation.
/// Canonical content rank of a record: completeness first, then a fixed
/// field-by-field order (see [`prefer_record`]).
type RecordRank<'a> = (
    usize,
    Option<&'a str>,
    Option<&'a [u8]>,
    Option<&'a str>,
    Option<u64>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    ApplicationSource,
    PackageKind,
    bool,
);

fn record_rank(r: &ApplicationRecord) -> RecordRank<'_> {
    (
        completeness(r),
        r.version.as_deref(),
        r.install_location
            .as_ref()
            .map(|p| p.as_os_str().as_encoded_bytes()),
        r.install_date.as_deref(),
        r.estimated_size_bytes,
        r.uninstall_string.as_deref(),
        r.quiet_uninstall_string.as_deref(),
        r.modify_path.as_deref(),
        r.install_source.as_deref(),
        r.source.clone(),
        r.kind,
        r.system_component,
    )
}

/// `true` when `new` should replace `existing` as the winning record of a
/// merged logical application (canonical precedence; see [`record_rank`]).
fn prefer_record(new: &ApplicationRecord, existing: &ApplicationRecord) -> bool {
    record_rank(new) > record_rank(existing)
}

/// The logical-application merge key: normalized (name, publisher) — the
/// same rule [`crate::domain::ApplicationId`] derives the id from, so the
/// id and the merge key cannot disagree.
type MergeKey = (String, String);

fn merge_key(rec: &ApplicationRecord) -> MergeKey {
    (
        rec.name.trim().to_lowercase(),
        rec.publisher.as_deref().unwrap_or("").trim().to_lowercase(),
    )
}

/// One admitted merge slot: the winning record plus the exact number of
/// input records absorbed under the key (for exact truncation accounting
/// when the slot is evicted).
struct Admitted {
    record: ApplicationRecord,
    absorbed: u64,
}

/// Merge provider outcomes into a deterministic, bounded [`Inventory`].
///
/// Semantics (explicit): records whose normalized (name, publisher) agree
/// are ONE logical application — the winning record is chosen by
/// [`record_rank`] and provenance (`observed_in_views`) is unioned.
///
/// Boundedness: working memory is bounded by ADMISSION, not by a final
/// truncate — at most `max_records` logical applications are held. When a
/// new key arrives at capacity, the canonically-LARGEST held key is
/// evicted (its absorbed records are counted into
/// [`Inventory::records_truncated`]); a key larger than everything held is
/// refused. Keys never change, so admission is call-order independent: the
/// published set is always the canonically-first `max_records` logical
/// applications. Ordering within the published set is canonical.
pub fn merge_inventory(outputs: Vec<ProviderOutcome>, limits: &DiscoveryLimits) -> Inventory {
    let mut sources: Vec<SourceCoverage> = Vec::new();
    let mut by_key: BTreeMap<MergeKey, Admitted> = BTreeMap::new();
    let mut rejected = 0u64;
    let mut truncated = 0u64;
    for outcome in outputs {
        sources.push(outcome.coverage);
        for rec in outcome.records {
            // A name longer than the declared bound is REJECTED (never
            // truncated: its id would change), and the rejection is counted
            // exactly.
            if rec.name.len() > limits.max_inventory_name_len {
                rejected += 1;
                continue;
            }
            let key = merge_key(&rec);
            match by_key.get_mut(&key) {
                Some(slot) => {
                    slot.absorbed += 1;
                    if prefer_record(&rec, &slot.record) {
                        let mut merged_views = slot.record.observed_in_views.clone();
                        merged_views.extend(rec.observed_in_views.iter().cloned());
                        merged_views.sort();
                        merged_views.dedup();
                        let mut replacement = rec;
                        replacement.observed_in_views = merged_views;
                        slot.record = replacement;
                    } else {
                        slot.record
                            .observed_in_views
                            .extend(rec.observed_in_views.iter().cloned());
                        slot.record.observed_in_views.sort();
                        slot.record.observed_in_views.dedup();
                    }
                }
                None => {
                    if by_key.len() < limits.max_records {
                        by_key.insert(
                            key,
                            Admitted {
                                record: rec,
                                absorbed: 1,
                            },
                        );
                    } else {
                        // At capacity: admit only when the new key is
                        // canonically smaller than the largest held key
                        // (the published set stays the canonically-first
                        // `max_records` logical applications). Otherwise
                        // the record is counted as truncated.
                        let largest = by_key.keys().next_back().cloned();
                        match largest {
                            Some(largest) if key < largest => {
                                if let Some(evicted) = by_key.remove(&largest) {
                                    truncated += evicted.absorbed;
                                }
                                by_key.insert(
                                    key,
                                    Admitted {
                                        record: rec,
                                        absorbed: 1,
                                    },
                                );
                            }
                            _ => truncated += 1,
                        }
                    }
                }
            }
        }
    }
    let mut records: Vec<ApplicationRecord> =
        by_key.into_values().map(|slot| slot.record).collect();
    records.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(
                a.publisher
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.publisher.as_deref().unwrap_or("")),
            )
            .then(a.id.0.cmp(&b.id.0))
    });
    // Canonical source order regardless of provider call order.
    sources.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.status.cmp(&b.status))
            .then(a.note.cmp(&b.note))
    });
    Inventory {
        records,
        sources,
        records_truncated: truncated,
        records_rejected: rejected,
    }
}

fn completeness(rec: &ApplicationRecord) -> usize {
    let mut n = 0;
    if rec.version.is_some() {
        n += 1;
    }
    if rec.publisher.is_some() {
        n += 1;
    }
    if rec.install_location.is_some() {
        n += 1;
    }
    if rec.estimated_size_bytes.is_some() {
        n += 1;
    }
    if rec.uninstall_string.is_some() {
        n += 1;
    }
    if rec.install_date.is_some() {
        n += 1;
    }
    n
}

/// Classify whether a raw uninstall record is a OS system component
/// from explicit flags only (never guessed from the name).
pub fn classify_kind(system_component: bool, kind_hint: Option<&str>) -> PackageKind {
    if system_component {
        PackageKind::SystemComponent
    } else if kind_hint
        .map(|h| h.eq_ignore_ascii_case("runtime") || h.eq_ignore_ascii_case("redistributable"))
        .unwrap_or(false)
    {
        PackageKind::SharedRuntime
    } else {
        PackageKind::Installed
    }
}
