//! Human-readable explanations for footprint associations (Phase 6).
//! Explanations never overclaim: strong evidence gets strong language,
//! weak evidence gets explicit uncertainty.

use crate::domain::ApplicationRecord;
use crate::evidence::Confidence;
use crate::footprint::FootprintCandidate;

/// A rendered explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub headline: String,
    pub confidence: Confidence,
    pub bullets: Vec<String>,
    /// True when the association is weak enough that the UI should
    /// say "possible", not "is".
    pub tentative: bool,
}

/// Render why `candidate` is associated with `app`.
pub fn explain(app: &ApplicationRecord, candidate: &FootprintCandidate) -> Explanation {
    let mut bullets = Vec::new();
    for e in &candidate.evidence {
        bullets.push(format!("{} ({})", e.why, e.source));
    }
    if candidate.confidence <= Confidence::Possible {
        bullets.push("association is name/coincidence based; ownership could not be verified from installation metadata".to_string());
    }
    let headline = match candidate.confidence {
        Confidence::Confirmed => format!("This is part of {}.", app.name),
        Confidence::Strong => format!("Very likely belongs to {}.", app.name),
        Confidence::Probable => format!("Likely belongs to {}.", app.name),
        Confidence::Possible | Confidence::Unknown => format!("Possible {} data.", app.name),
    };
    let tentative = matches!(
        candidate.confidence,
        Confidence::Possible | Confidence::Unknown
    );
    Explanation {
        headline,
        confidence: candidate.confidence,
        bullets,
        tentative,
    }
}
