//! The ownership-evidence model (Phase 6.2).
//!
//! ```text
//! OwnershipEvidence ─┐ (kind, source, strength, correlation group,
//! OwnershipEvidence ─┼   observed path, matched attribute)
//! OwnershipEvidence ─┘            │
//!                                 ▼  assess_groups(): one vote per
//!                          OwnershipAssessment   CORRELATION GROUP
//! ```
//!
//! ## Correlated-evidence ceiling (the anti-inflation contract)
//!
//! Evidence items are NOT independent merely because they have different
//! kinds. Each item names the *root signal* it derives from
//! ([`CorrelationGroup`]): a publisher-directory match, an application-
//! directory match and a file-name match that all come from the same
//! normalized application name are ONE `NameDerived` signal.
//!
//! 1. **Clamp at construction.** The stored strength is
//!    `min(requested, kind ceiling, group ceiling)`
//!    ([`EvidenceKind::max_strength`], [`CorrelationGroup::ceiling`]) — an
//!    inflated item cannot be built.
//! 2. **One vote per group.** Aggregation keeps the strongest item of each
//!    group; repeating a signal never adds weight.
//! 3. **Corroboration is capped.** Two or more *independent* groups lift a
//!    `Weak` best signal to `Moderate` ("multiple independent metadata
//!    signals agree") and nothing else. `Strong`/`Direct` assessments exist
//!    only when a single authoritative group supplies them. Consequently
//!    name-derived heuristics alone can never exceed `Weak`.
//!
//! ## Complexity
//!
//! [`EvidenceAccumulator`] holds ≤ one strength per correlation group
//! (a small fixed-size set) plus a [`BoundedTopK`] of at most `capacity`
//! evidence items: **O(capacity)** memory regardless of how many items are
//! offered. The assessment is computed from the per-group accumulator, so it
//! is exact even when retained evidence is truncated, and it does not depend
//! on arrival order (max per group, then a commutative combination).

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::bounded::BoundedTopK;
use crate::domain::ApplicationSource;
use crate::evidence::{AssociationScope, Confidence, EvidenceKind};
use crate::observe::PathKey;

/// How strong ONE item of evidence is. Ascending: `Weak < … < Direct`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceStrength {
    /// Name/similarity/sibling heuristics.
    Weak,
    /// Structural containment, known layouts, corroborated signals.
    Moderate,
    /// Exact references recorded by application-owned/authoritative
    /// metadata.
    Strong,
    /// Recorded outright by the installer/platform (e.g. the install
    /// location).
    Direct,
}

impl EvidenceStrength {
    /// The Phase 6.1 vocabulary this strength corresponds to.
    pub fn to_confidence(self) -> Confidence {
        match self {
            EvidenceStrength::Direct => Confidence::Confirmed,
            EvidenceStrength::Strong => Confidence::Strong,
            EvidenceStrength::Moderate => Confidence::Probable,
            EvidenceStrength::Weak => Confidence::Possible,
        }
    }

    /// `None` for [`Confidence::Unknown`]: unknown is not a strength.
    pub fn from_confidence(c: Confidence) -> Option<Self> {
        match c {
            Confidence::Confirmed => Some(EvidenceStrength::Direct),
            Confidence::Strong => Some(EvidenceStrength::Strong),
            Confidence::Probable => Some(EvidenceStrength::Moderate),
            Confidence::Possible => Some(EvidenceStrength::Weak),
            Confidence::Unknown => None,
        }
    }
}

/// Where a piece of evidence came from (provenance of the observation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceSource {
    /// A merged inventory record (source unspecified).
    InventoryRecord,
    /// Win32 uninstall registry metadata.
    RegistryMetadata,
    /// Package metadata (MSIX/AppX repository).
    PackageMetadata,
    /// Application bundle metadata (`Info.plist`).
    BundleMetadata,
    /// `.desktop` entry metadata.
    DesktopEntry,
    /// Metadata of an executable artifact.
    ExecutableMetadata,
    /// A direct filesystem observation (stat/listing).
    FilesystemObservation,
    /// A path-name heuristic.
    FilesystemPathHeuristic,
    // ---- Phase 6.3 additions (appended; existing order unchanged).
    /// A content digest produced by the identity engine.
    ContentHash,
    /// A stored historical observation (history subsystem, read-only).
    HistoryObservation,
    /// A classification rule's verdict or parent-context corroboration.
    ClassificationRule,
}

impl EvidenceSource {
    /// The evidence source corresponding to an inventory provenance.
    pub fn for_application_source(source: &ApplicationSource) -> Self {
        match source {
            ApplicationSource::RegistryUninstall => EvidenceSource::RegistryMetadata,
            ApplicationSource::PackagedApp => EvidenceSource::PackageMetadata,
            ApplicationSource::BundleInfoPlist => EvidenceSource::BundleMetadata,
            ApplicationSource::DesktopEntry => EvidenceSource::DesktopEntry,
            ApplicationSource::FilesystemPresence => EvidenceSource::InventoryRecord,
        }
    }
}

/// The ROOT signal an item derives from. Items in one group are one vote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "group", content = "source", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CorrelationGroup {
    /// Everything read from one inventory source record (a registry key, a
    /// bundle plist, a `.desktop` file, a package entry).
    SourceRecord(ApplicationSource),
    /// A canonical [`coresight_identity::ObjectIdentity`] equality proof.
    ObjectIdentity,
    /// The application's declared bundle/package identifier.
    BundleIdentifier,
    /// Path structure relative to an install root.
    InstallRootStructure,
    /// Anything derived from the normalized application/publisher NAME.
    NameDerived,
    // ---- Phase 6.3 additions (appended; existing order unchanged).
    /// A proven content-digest equality (byte identity of objects).
    ContentIdentity,
    /// A classification rule's verdict / corroborating parent context.
    ClassificationDerived,
    /// A stored historical observation of the same object.
    HistoricalObservation,
}

impl CorrelationGroup {
    /// The strongest strength this root signal may ever claim.
    pub fn ceiling(&self) -> EvidenceStrength {
        match self {
            CorrelationGroup::SourceRecord(_) | CorrelationGroup::ObjectIdentity => {
                EvidenceStrength::Direct
            }
            CorrelationGroup::BundleIdentifier => EvidenceStrength::Strong,
            CorrelationGroup::InstallRootStructure => EvidenceStrength::Moderate,
            CorrelationGroup::NameDerived => EvidenceStrength::Weak,
            // A content digest proves byte identity of two objects — a
            // strong structural fact, but it never proves OWNERSHIP (two
            // different applications' files can be byte-identical), so the
            // group ceiling is Strong, not Direct.
            CorrelationGroup::ContentIdentity => EvidenceStrength::Strong,
            // A classification verdict is a descriptive label; it may
            // corroborate but never assert ownership.
            CorrelationGroup::ClassificationDerived => EvidenceStrength::Moderate,
            // A historical observation proves the object existed and was
            // seen; it is authoritative about the PAST, not about current
            // ownership, so it corroborates at Strong.
            CorrelationGroup::HistoricalObservation => EvidenceStrength::Strong,
        }
    }
}

impl EvidenceKind {
    /// The strongest strength this KIND of evidence may ever claim,
    /// regardless of who offers it.
    pub fn max_strength(self) -> EvidenceStrength {
        match self {
            EvidenceKind::InstallLocation
            | EvidenceKind::RegistryReference
            | EvidenceKind::PackageIdentity => EvidenceStrength::Direct,
            EvidenceKind::ExactExecutablePath
            | EvidenceKind::DesktopEntryReference
            | EvidenceKind::BundleIdentifierReference
            | EvidenceKind::ObjectIdentityMatch
            | EvidenceKind::ExecutableReference
            | EvidenceKind::ObservedWrite => EvidenceStrength::Strong,
            EvidenceKind::InstallRootContainment | EvidenceKind::BundleStructure => {
                EvidenceStrength::Moderate
            }
            EvidenceKind::KnownApplicationDirectory
            | EvidenceKind::ShortcutReference
            | EvidenceKind::PublisherDirectory
            | EvidenceKind::FilenameSimilarity
            | EvidenceKind::DirectoryNameSimilarity
            | EvidenceKind::SiblingHeuristic => EvidenceStrength::Weak,
            // ---- Phase 6.3 additions. Conservative ceilings, calibrated to
            // what each kind can actually prove about OWNERSHIP.
            //
            // A content-digest equality proves two objects hold identical
            // bytes. It is a strong fact, but it never proves ownership:
            // unrelated applications can ship byte-identical files.
            EvidenceKind::ContentDigestMatch => EvidenceStrength::Strong,
            // A stored historical observation is authoritative about the
            // past, and only corroborates the present.
            EvidenceKind::HistoricalObservation => EvidenceStrength::Strong,
            // Classification is descriptive: it labels what an artifact IS,
            // never who owns it.
            EvidenceKind::ClassificationReference => EvidenceStrength::Moderate,
        }
    }

    /// True for evidence that is merely STRUCTURAL containment (an object
    /// lying inside a root). Containment never proves ownership: an
    /// artifact whose only evidence is structural is reported as
    /// `Contains`, not `Owns` (see `RelationKind`).
    pub fn is_structural(self) -> bool {
        matches!(self, EvidenceKind::InstallRootContainment)
    }
}

/// Which attribute of the application the evidence matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchedAttribute {
    InstallLocation,
    ExecutablePath,
    BundleIdentifier,
    PackageIdentity,
    ApplicationName,
    Publisher,
    ObjectIdentity,
    InstallRoot,
}

/// One machine-readable reason an artifact is associated with an
/// application. This IS the structured explanation: a presentation layer
/// may render it ([`OwnershipEvidence::render`]) but the facts live here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipEvidence {
    pub kind: EvidenceKind,
    pub source: EvidenceSource,
    /// Already clamped: `min(requested, kind ceiling, group ceiling)`.
    pub strength: EvidenceStrength,
    pub correlation_group: CorrelationGroup,
    pub scope: AssociationScope,
    /// The artifact path the evidence was observed at (lossless).
    pub observed_path: PathBuf,
    pub matched_attribute: MatchedAttribute,
    /// The attribute value that matched, when it is metadata text (a name,
    /// an identifier). Presentation/diagnostic only — never an identity.
    pub matched_value: Option<String>,
    /// The attribute value that matched, when it is a PATH. This is the
    /// lossless counterpart to [`Self::matched_value`]: a path-valued match
    /// is recorded as bytes, never as a display string, so it can be
    /// compared and traced without lossy conversion.
    #[serde(default)]
    pub matched_path: Option<PathBuf>,
}

impl OwnershipEvidence {
    /// Build evidence; the strength is clamped to the documented ceilings
    /// so over-claiming is impossible by construction.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: EvidenceKind,
        source: EvidenceSource,
        requested: EvidenceStrength,
        correlation_group: CorrelationGroup,
        scope: AssociationScope,
        observed_path: PathBuf,
        matched_attribute: MatchedAttribute,
        matched_value: Option<String>,
    ) -> Self {
        let strength = requested
            .min(kind.max_strength())
            .min(correlation_group.ceiling());
        OwnershipEvidence {
            kind,
            source,
            strength,
            correlation_group,
            scope,
            observed_path,
            matched_attribute,
            matched_value,
            matched_path: None,
        }
    }

    /// Attach the lossless path form of the matched attribute.
    pub fn with_matched_path(mut self, path: PathBuf) -> Self {
        self.matched_path = Some(path);
        self
    }

    /// Presentation-only sentence rendered from the structured facts.
    pub fn render(&self) -> String {
        let what = match self.kind {
            EvidenceKind::InstallLocation => "the path is the installer-recorded install location",
            EvidenceKind::RegistryReference => "the uninstall registry record points at the path",
            EvidenceKind::PackageIdentity => "package metadata points directly at the path",
            EvidenceKind::ExactExecutablePath => {
                "the path is the executable recorded by application metadata"
            }
            EvidenceKind::InstallRootContainment => "the path lies inside a candidate install root",
            EvidenceKind::BundleStructure => "the path follows the application's bundle layout",
            EvidenceKind::BundleIdentifierReference => {
                "the name equals the application's declared bundle identifier"
            }
            EvidenceKind::ObjectIdentityMatch => {
                "the filesystem object is the same object the application names"
            }
            EvidenceKind::DesktopEntryReference => "a desktop entry names the path",
            EvidenceKind::FilenameSimilarity => "the file name resembles the application name",
            EvidenceKind::DirectoryNameSimilarity => {
                "the directory name resembles the application name"
            }
            EvidenceKind::SiblingHeuristic => "the path is a sibling of an associated artifact",
            EvidenceKind::KnownApplicationDirectory => {
                "the directory name matches the application under a standard data root"
            }
            EvidenceKind::PublisherDirectory => "the parent directory matches the publisher name",
            EvidenceKind::ShortcutReference => "a shortcut name matches the application",
            EvidenceKind::ExecutableReference => "an executable of the application lives here",
            EvidenceKind::ObservedWrite => "the application was observed writing here",
            EvidenceKind::ContentDigestMatch => {
                "the objects hold byte-identical content (digest equality)"
            }
            EvidenceKind::HistoricalObservation => {
                "a stored historical observation names this artifact"
            }
            EvidenceKind::ClassificationReference => {
                "an existing classification describes this artifact"
            }
        };
        format!("Associated because {what} ({:?} strength).", self.strength)
    }
}

/// The aggregate verdict about one (application, artifact) association.
///
/// `Conflicting` is never produced by [`assess_groups`] for one application:
/// it is assigned by cross-application conflict detection. `Unknown` means
/// there is no usable evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OwnershipAssessment {
    Unknown,
    Weak,
    Moderate,
    Strong,
    Direct,
    /// Credible evidence points at more than one application.
    Conflicting,
}

impl OwnershipAssessment {
    pub fn from_strength(s: EvidenceStrength) -> Self {
        match s {
            EvidenceStrength::Weak => OwnershipAssessment::Weak,
            EvidenceStrength::Moderate => OwnershipAssessment::Moderate,
            EvidenceStrength::Strong => OwnershipAssessment::Strong,
            EvidenceStrength::Direct => OwnershipAssessment::Direct,
        }
    }

    /// The evidence strength behind this assessment (`None` for `Unknown`
    /// and `Conflicting`, which are not strengths).
    pub fn strength(self) -> Option<EvidenceStrength> {
        match self {
            OwnershipAssessment::Weak => Some(EvidenceStrength::Weak),
            OwnershipAssessment::Moderate => Some(EvidenceStrength::Moderate),
            OwnershipAssessment::Strong => Some(EvidenceStrength::Strong),
            OwnershipAssessment::Direct => Some(EvidenceStrength::Direct),
            _ => None,
        }
    }

    /// Credible = at least `Moderate`.
    pub fn is_credible(self) -> bool {
        self.strength()
            .is_some_and(|s| s >= EvidenceStrength::Moderate)
    }
}

/// Combine one best strength per independent correlation group.
///
/// Pure and commutative over the multiset of groups: the result depends only
/// on `max(group strengths)` and `number of independent groups`.
pub fn assess_groups(
    group_best: &BTreeMap<CorrelationGroup, EvidenceStrength>,
) -> OwnershipAssessment {
    let Some(best) = group_best.values().copied().max() else {
        return OwnershipAssessment::Unknown;
    };
    let independent = group_best.len();
    let combined = if best == EvidenceStrength::Weak && independent >= 2 {
        // "multiple independent metadata signals agree" → Moderate, and no
        // higher: corroboration alone never reaches Strong.
        EvidenceStrength::Moderate
    } else {
        best
    };
    OwnershipAssessment::from_strength(combined)
}

/// Canonical evidence ordering key: strongest first, then every field.
type EvidenceKey = (
    Reverse<EvidenceStrength>,
    EvidenceKind,
    CorrelationGroup,
    EvidenceSource,
    PathKey,
    MatchedAttribute,
    Option<String>,
);

fn evidence_key(e: &OwnershipEvidence) -> EvidenceKey {
    (
        Reverse(e.strength),
        e.kind,
        e.correlation_group.clone(),
        e.source,
        PathKey(e.observed_path.clone()),
        e.matched_attribute,
        e.matched_value.clone(),
    )
}

/// Streaming evidence aggregation with bounded memory. See the module docs
/// for complexity.
#[derive(Debug, Clone)]
pub struct EvidenceAccumulator {
    group_best: BTreeMap<CorrelationGroup, EvidenceStrength>,
    retained: BoundedTopK<EvidenceKey, OwnershipEvidence>,
    scopes: BTreeMap<AssociationScope, ()>,
}

impl EvidenceAccumulator {
    pub fn new(capacity: usize) -> Self {
        EvidenceAccumulator {
            group_best: BTreeMap::new(),
            retained: BoundedTopK::new(capacity),
            scopes: BTreeMap::new(),
        }
    }

    pub fn offer(&mut self, evidence: OwnershipEvidence) {
        let slot = self
            .group_best
            .entry(evidence.correlation_group.clone())
            .or_insert(evidence.strength);
        if evidence.strength > *slot {
            *slot = evidence.strength;
        }
        self.scopes.insert(evidence.scope, ());
        self.retained
            .offer(evidence_key(&evidence), evidence, |_, _| false);
    }

    pub fn assessment(&self) -> OwnershipAssessment {
        assess_groups(&self.group_best)
    }

    /// Per-group strongest strength (exact even when retention truncated).
    pub fn group_best(&self) -> &BTreeMap<CorrelationGroup, EvidenceStrength> {
        &self.group_best
    }

    /// `true` when every group is structural-containment only — i.e. the
    /// association rests on "lies inside a root" and nothing else.
    pub fn is_structural_only(&self) -> bool {
        !self.retained.is_empty()
            && self.retained.iter().all(|(_, e)| e.kind.is_structural())
            && self.retained.overflow() == 0
    }

    pub fn retained_len(&self) -> usize {
        self.retained.len()
    }

    /// Canonically ordered retained evidence plus the exact number of
    /// offered items that were not retained.
    pub fn into_parts(self) -> (Vec<OwnershipEvidence>, u64) {
        let (items, overflow) = self.retained.into_sorted();
        (items.into_iter().map(|(_, e)| e).collect(), overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(
        kind: EvidenceKind,
        requested: EvidenceStrength,
        group: CorrelationGroup,
        path: &str,
    ) -> OwnershipEvidence {
        OwnershipEvidence::new(
            kind,
            EvidenceSource::FilesystemPathHeuristic,
            requested,
            group,
            AssociationScope::ThisMachine,
            PathBuf::from(path),
            MatchedAttribute::ApplicationName,
            None,
        )
    }

    #[test]
    fn construction_clamps_to_group_and_kind_ceilings() {
        let e = ev(
            EvidenceKind::InstallLocation,
            EvidenceStrength::Direct,
            CorrelationGroup::NameDerived,
            "/x",
        );
        assert_eq!(e.strength, EvidenceStrength::Weak, "group ceiling wins");
        let e = ev(
            EvidenceKind::FilenameSimilarity,
            EvidenceStrength::Direct,
            CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
            "/x",
        );
        assert_eq!(e.strength, EvidenceStrength::Weak, "kind ceiling wins");
    }

    #[test]
    fn same_group_evidence_is_one_vote() {
        let mut acc = EvidenceAccumulator::new(16);
        for (i, k) in [
            EvidenceKind::PublisherDirectory,
            EvidenceKind::DirectoryNameSimilarity,
            EvidenceKind::FilenameSimilarity,
        ]
        .into_iter()
        .enumerate()
        {
            acc.offer(ev(
                k,
                EvidenceStrength::Direct,
                CorrelationGroup::NameDerived,
                &format!("/p{i}"),
            ));
        }
        assert_eq!(acc.assessment(), OwnershipAssessment::Weak);
    }

    #[test]
    fn independent_weak_groups_corroborate_to_moderate_and_no_higher() {
        let mut m = BTreeMap::new();
        m.insert(CorrelationGroup::NameDerived, EvidenceStrength::Weak);
        assert_eq!(assess_groups(&m), OwnershipAssessment::Weak);
        m.insert(
            CorrelationGroup::SourceRecord(ApplicationSource::DesktopEntry),
            EvidenceStrength::Weak,
        );
        assert_eq!(assess_groups(&m), OwnershipAssessment::Moderate);
        m.insert(
            CorrelationGroup::InstallRootStructure,
            EvidenceStrength::Moderate,
        );
        m.insert(CorrelationGroup::ObjectIdentity, EvidenceStrength::Moderate);
        assert_eq!(
            assess_groups(&m),
            OwnershipAssessment::Moderate,
            "corroboration never reaches Strong"
        );
    }

    #[test]
    fn empty_is_unknown_not_weak() {
        assert_eq!(
            assess_groups(&BTreeMap::new()),
            OwnershipAssessment::Unknown
        );
    }

    #[test]
    fn assessment_is_independent_of_arrival_order() {
        let items = [
            ev(
                EvidenceKind::InstallRootContainment,
                EvidenceStrength::Moderate,
                CorrelationGroup::InstallRootStructure,
                "/a",
            ),
            ev(
                EvidenceKind::DirectoryNameSimilarity,
                EvidenceStrength::Weak,
                CorrelationGroup::NameDerived,
                "/a",
            ),
            ev(
                EvidenceKind::ExactExecutablePath,
                EvidenceStrength::Strong,
                CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
                "/a",
            ),
        ];
        let mut forward = EvidenceAccumulator::new(2);
        let mut backward = EvidenceAccumulator::new(2);
        for e in items.iter() {
            forward.offer(e.clone());
        }
        for e in items.iter().rev() {
            backward.offer(e.clone());
        }
        assert_eq!(forward.assessment(), OwnershipAssessment::Strong);
        assert_eq!(forward.assessment(), backward.assessment());
        assert_eq!(forward.into_parts(), backward.into_parts());
    }

    #[test]
    fn retained_evidence_is_bounded_but_assessment_stays_exact() {
        let mut acc = EvidenceAccumulator::new(3);
        for i in 0..1000 {
            acc.offer(ev(
                EvidenceKind::FilenameSimilarity,
                EvidenceStrength::Weak,
                CorrelationGroup::NameDerived,
                &format!("/f{i:04}"),
            ));
            assert!(acc.retained_len() <= 3);
        }
        acc.offer(ev(
            EvidenceKind::InstallLocation,
            EvidenceStrength::Direct,
            CorrelationGroup::SourceRecord(ApplicationSource::RegistryUninstall),
            "/root",
        ));
        assert_eq!(acc.assessment(), OwnershipAssessment::Direct);
        let (kept, overflow) = acc.into_parts();
        assert_eq!(kept.len(), 3);
        assert_eq!(
            kept[0].strength,
            EvidenceStrength::Direct,
            "strongest first"
        );
        assert_eq!(overflow, 998);
    }

    #[test]
    fn confidence_round_trip_and_unknown_is_not_a_strength() {
        for s in [
            EvidenceStrength::Weak,
            EvidenceStrength::Moderate,
            EvidenceStrength::Strong,
            EvidenceStrength::Direct,
        ] {
            assert_eq!(
                EvidenceStrength::from_confidence(s.to_confidence()),
                Some(s)
            );
        }
        assert_eq!(EvidenceStrength::from_confidence(Confidence::Unknown), None);
    }

    #[test]
    fn structured_facts_survive_rendering() {
        let e = ev(
            EvidenceKind::DirectoryNameSimilarity,
            EvidenceStrength::Weak,
            CorrelationGroup::NameDerived,
            "/Apps/Foo",
        );
        assert_eq!(e.observed_path, PathBuf::from("/Apps/Foo"));
        assert!(e.render().contains("resembles"));
    }
}
