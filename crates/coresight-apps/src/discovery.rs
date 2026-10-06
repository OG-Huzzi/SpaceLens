//! Application discovery (Phase 6, Objective 13): a platform-neutral
//! provider model with a Windows Win32-registry implementation.

use std::collections::BTreeMap;

use crate::domain::{
    ApplicationRecord, DiscoveryLimits, Inventory, PackageKind, SourceCoverage, SourceStatus,
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

/// Merge provider outcomes into a deterministic [`Inventory`]:
/// duplicates collapse by (normalized name, normalized publisher) with
/// provenance union; ordering is canonical; limits apply with exact
/// truncation/rejection counting.
pub fn merge_inventory(outputs: Vec<ProviderOutcome>, limits: &DiscoveryLimits) -> Inventory {
    let mut sources: Vec<SourceCoverage> = Vec::new();
    // Merge by (name, publisher) — case-insensitive, whitespace-trimmed.
    let mut by_key: BTreeMap<(String, String), ApplicationRecord> = BTreeMap::new();
    let mut rejected = 0u64;
    for outcome in outputs {
        sources.push(outcome.coverage);
        for rec in outcome.records {
            // A name longer than the declared bound is REJECTED (never
            // truncated: a truncated name would derive a different
            // ApplicationId), and the rejection is counted exactly.
            if rec.name.len() > limits.max_inventory_name_len {
                rejected += 1;
                continue;
            }
            let key = (
                rec.name.trim().to_lowercase(),
                rec.publisher.as_deref().unwrap_or("").trim().to_lowercase(),
            );
            match by_key.get_mut(&key) {
                Some(existing) => {
                    // Prefer the record with the most complete metadata.
                    if completeness(&rec) > completeness(existing) {
                        let mut merged_views = existing.observed_in_views.clone();
                        merged_views.extend(rec.observed_in_views.iter().cloned());
                        let mut replacement = rec.clone();
                        replacement.observed_in_views = merged_views;
                        replacement.observed_in_views.sort();
                        replacement.observed_in_views.dedup();
                        *existing = replacement;
                    } else {
                        existing
                            .observed_in_views
                            .extend(rec.observed_in_views.iter().cloned());
                        existing.observed_in_views.sort();
                        existing.observed_in_views.dedup();
                    }
                }
                None => {
                    by_key.insert(key, rec);
                }
            }
        }
    }
    let mut records: Vec<ApplicationRecord> = by_key.into_values().collect();
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
    let truncated = if records.len() > limits.max_records {
        let overflow = (records.len() - limits.max_records) as u64;
        records.truncate(limits.max_records);
        overflow
    } else {
        0
    };
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
