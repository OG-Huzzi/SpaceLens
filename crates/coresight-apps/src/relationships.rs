//! Application-level relationships (Phase 6): the deterministic,
//! evidence-backed association between an application and a filesystem
//! entry it may (or may not) own — shared resources are never claimed
//! as exclusively owned.

use serde::{Deserialize, Serialize};

use crate::domain::ApplicationId;
use crate::evidence::{Confidence, FootprintEvidence};

/// How an application relates to a filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssociationKind {
    InstalledAt,
    Owns,
    References,
    Generates,
    Caches,
    LogsTo,
    ConfiguredBy,
    StartsWith,
    Shares,
    UnknownAssociation,
}

/// How strongly ownership is established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OwnershipStrength {
    Definite,
    Probable,
    Possible,
    CannotDetermine,
}

impl OwnershipStrength {
    /// Map a footprint confidence to the honest ownership claim —
    /// never upgrade evidence to ownership.
    pub fn from_confidence(c: Confidence) -> Self {
        match c {
            Confidence::Confirmed => OwnershipStrength::Definite,
            Confidence::Strong => OwnershipStrength::Probable,
            Confidence::Probable => OwnershipStrength::Possible,
            Confidence::Possible | Confidence::Unknown => OwnershipStrength::CannotDetermine,
        }
    }
}

/// One application ↔ filesystem entry relationship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAssociation {
    pub app_id: ApplicationId,
    pub path: std::path::PathBuf,
    pub kind: AssociationKind,
    pub strength: OwnershipStrength,
    pub evidence: Vec<FootprintEvidence>,
}

impl AppAssociation {
    pub fn new(
        app_id: ApplicationId,
        path: std::path::PathBuf,
        kind: AssociationKind,
        strength: OwnershipStrength,
        evidence: Vec<FootprintEvidence>,
    ) -> Self {
        AppAssociation {
            app_id,
            path,
            kind,
            strength,
            evidence,
        }
    }
}
