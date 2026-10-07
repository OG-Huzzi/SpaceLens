//! CoreSight capability architecture — Phase 6.1 (shared contracts).
//!
//! CoreSight is a **macOS system intelligence + power-tools application**,
//! not a single-purpose cleaner. Storage intelligence is one pillar of
//! seven. This crate is the shared, platform-neutral contract layer that
//! every pillar builds against; macOS *behavior* lives in
//! `coresight-macos`, Windows/Linux implementations in their own crates or
//! behind the existing `coresight-engine` platform traits.
//!
//! ```text
//! CoreSight — macOS system intelligence + power tools
//! ├── Storage Intelligence        scanner/classifier/identity/history (implemented)
//! ├── Application Intelligence    coresight-apps (Windows providers; macOS planned)
//! ├── System Intelligence         this crate: contracts; coresight-macos: discovery
//! ├── Privacy / Housekeeping      contracts only — actions need separate authorization
//! ├── Performance / Diagnostics   this crate: DiagnosticSignal contract
//! ├── Software Management         contracts only — no execution anywhere
//! └── History / Forensics         coresight-history (implemented, audited)
//! ```
//!
//! Design contracts honored here:
//!
//! - **Honest states.** Every result distinguishes observed / inferred /
//!   unsupported / unavailable / failed ([`Observation`]), and every
//!   path-access fact distinguishes exists-but-inaccessible / does-not-exist
//!   / not-applicable / unsupported / empty / read-succeeded / failed
//!   ([`AccessState`]). Unavailable data can never masquerade as "nothing
//!   found": the state is part of the type, so an unsupported or failed
//!   observation structurally cannot carry a payload.
//! - **The safety pipeline is the veto point.** Actions are classified
//!   (read-only / reversible / destructive / privileged /
//!   permission-sensitive) and must pass every stage in order —
//!   OBSERVE → ANALYZE → RECOMMEND → PREVIEW → VALIDATE → EXECUTE → VERIFY
//!   → ROLLBACK. The pipeline has no API to skip a stage, and in the
//!   current build the gate authorizes read-only effects only.
//! - **Platform neutrality.** This crate contains no macOS/Windows/Linux
//!   conditionals at all (enforced by a source-scan test below); OS
//!   behavior lives behind the platform boundaries.

pub mod access;
pub mod capability;
pub mod diagnostics;
pub mod observation;
pub mod report;
pub mod safety;
pub mod startup;

pub use access::AccessState;
pub use capability::{
    contract, CapabilityContract, CapabilityId, CapabilityStatus, Pillar, CONTRACTS,
};
pub use diagnostics::{DiagnosticSeverity, DiagnosticSignal, DiagnosticSignalKind};
pub use observation::{Observation, ObservationState};
pub use report::{CapabilityOutcome, CapabilityReport, Coverage};
pub use safety::{
    ActionClass, ActionClassification, ActionEffect, ActionPipeline, ActionStage, BlockReason,
    ExecutionPolicy, GateVerdict, PipelineError, ProposedAction, SafetyGate, StageRecord,
};
pub use startup::{StartupItem, StartupItemId, StartupMechanism, StartupScope};

#[cfg(test)]
mod architecture_guard_tests {
    use std::fs;
    use std::path::Path;

    /// The shared capability layer must stay platform-neutral: no OS
    /// conditional compilation may appear in this crate
    /// (docs/CROSS_PLATFORM.md). This is the testable form of the
    /// "macOS capability modules can be represented without Windows/Linux
    /// behavior leaking into shared logic" requirement — macOS specifics
    /// belong in `coresight-macos`, never here.
    ///
    /// The needles are assembled at runtime so this test's own source does
    /// not match them.
    #[test]
    fn shared_contracts_contain_no_platform_conditionals() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut needles = Vec::new();
        for word in ["target_os", "windows", "unix"] {
            needles.push(format!("cfg!( {word}").replace(' ', ""));
            needles.push(format!("#[cfg( {word}").replace(' ', ""));
        }
        let mut checked = 0;
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("src tree is readable") {
                let entry = entry.expect("src tree is readable");
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let text = fs::read_to_string(&path).expect("source is UTF-8");
                    for needle in &needles {
                        assert!(
                            !text.contains(needle.as_str()),
                            "platform conditional {needle:?} found in {}",
                            path.display()
                        );
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked >= 8, "expected to scan the crate's source files");
    }
}
