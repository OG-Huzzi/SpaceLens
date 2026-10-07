//! Typed footprint-association evidence (Phase 6).

use serde::{Deserialize, Serialize};

/// What kind of proof links a filesystem entry to an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceKind {
    /// The path equals the installer-recorded install location.
    InstallLocation,
    /// The OS uninstall registry record points at this path.
    RegistryReference,
    /// The path sits under a well-known application data root with a
    /// name that matches the installed application/publisher.
    KnownApplicationDirectory,
    /// An executable that belongs to the application lives here (or
    /// its parent).
    ExecutableReference,
    /// A Start Menu / desktop shortcut references the application.
    ShortcutReference,
    /// The directory name matches the recorded publisher name.
    PublisherDirectory,
    /// A package identity (MSIX/AppX) points at this location.
    PackageIdentity,
    /// Observed written by the application's process (future; reserved).
    ObservedWrite,
    // ---- Phase 6.2 additions (appended; canonical order of the existing
    // kinds is unchanged). Each kind documents WHY it supports ownership.
    /// The path equals the executable path EXACTLY as application-owned /
    /// installer metadata recorded it.
    ExactExecutablePath,
    /// The path lies inside a candidate install root (structural
    /// containment only — never proof of ownership by itself).
    InstallRootContainment,
    /// Known bundle/package layout (e.g. `X.app/Contents/MacOS/exe`).
    BundleStructure,
    /// The artifact name equals the application's declared bundle/package
    /// identifier (reverse-DNS), e.g. `~/Library/Caches/com.vendor.App`.
    BundleIdentifierReference,
    /// The artifact's canonical object identity equals that of an artifact
    /// the application's metadata names (e.g. a hard link).
    ObjectIdentityMatch,
    /// A `.desktop` entry names the artifact (`Exec`/`TryExec`).
    DesktopEntryReference,
    /// A file name resembles the application name (name-derived).
    FilenameSimilarity,
    /// A directory name resembles the application name (name-derived).
    DirectoryNameSimilarity,
    /// Heuristic sibling relationship (shares a parent with an associated
    /// artifact).
    SiblingHeuristic,
    // ---- Phase 6.3 additions (appended; existing canonical order is
    // unchanged). These let the unified system model carry cross-domain
    // evidence through the SAME vocabulary instead of inventing a parallel
    // one, so the Phase 6.2 correlation ceilings apply uniformly.
    /// A proven content-digest equality (the identity engine proved the
    /// objects hold byte-identical content).
    ContentDigestMatch,
    /// A stored historical observation names this artifact.
    HistoricalObservation,
    /// An existing classifier verdict describes this artifact.
    ClassificationReference,
}

/// How strong the association claim is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Confidence {
    /// Directly recorded by the installer/platform — proof.
    Confirmed,
    /// Multiple consistent signals.
    Strong,
    /// One weak-to-moderate signal.
    Probable,
    /// Name coincidence only.
    Possible,
    Unknown,
}

/// Who this association applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssociationScope {
    ThisMachine,
    CurrentUser,
    Unknown,
}

/// One piece of evidence supporting a candidate association.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FootprintEvidence {
    pub kind: EvidenceKind,
    pub confidence: Confidence,
    /// Human-readable provenance: which engine/source produced it.
    pub source: String,
    pub scope: AssociationScope,
    /// Why the evidence supports the association.
    pub why: String,
}

impl FootprintEvidence {
    pub fn new(
        kind: EvidenceKind,
        confidence: Confidence,
        source: &str,
        scope: AssociationScope,
        why: &str,
    ) -> Self {
        FootprintEvidence {
            kind,
            confidence,
            source: source.to_string(),
            scope,
            why: why.to_string(),
        }
    }
}
