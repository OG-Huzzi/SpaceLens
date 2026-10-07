//! CoreSight macOS discovery boundary — Phase 6.1.
//!
//! This crate is the MACOS IMPLEMENTATION side of the capability
//! architecture (docs/MACOS_ARCHITECTURE.md): the catalog of macOS
//! discovery sources with their safety classification, and the
//! permission-aware observation contracts that read them.
//!
//! Contracts honored here:
//!
//! - **Observation only.** This phase performs bounded, read-only
//!   directory listing — never content reads, never recursion, never
//!   writes, never deletion. Every mutation classification in the catalog
//!   describes a FUTURE phase and requires separate authorization.
//! - **Honest access states.** Permission denial is never collapsed into
//!   an empty result; existence claims are exact (see
//!   [`coresight_capabilities::AccessState`]).
//! - **No privilege escalation, no security circumvention.** A TCC-protected
//!   location is never probed without an explicit user grant, and a
//!   denied location stays denied and is reported as such.
//! - **No subprocesses.** Sources that need APFS-aware or login-item APIs
//!   are classified `UnsupportedForNow`/`Deferred`, not approximated by
//!   shelling out.
//!
//! The crate compiles on every platform (pure data + std probes) so the
//! contracts are testable everywhere; real observation of the catalog runs
//! only on macOS, and non-macOS hosts report every source as
//! `UNSUPPORTED` — never as empty or absent.

pub mod catalog;
pub mod observation;
pub mod probe;

pub use catalog::{
    source, MacSourceId, MacSourceSpec, ModificationRisk, Sensitivity, SourceAccess,
    SourceAvailability, SourceLocation, SOURCES,
};
pub use observation::{
    observe_host_sources, observe_path, observe_source, observe_with_probe, ProbeLimits,
    SourceObservation,
};
pub use probe::{ChildCount, MacFileProbe, ProbeKind, StdProbe};

#[cfg(test)]
mod architecture_guard_tests {
    use std::fs;
    use std::path::Path;

    /// This crate may branch on `target_os = "macos"` ONLY: the whole point
    /// of the boundary is that macOS behavior lives here, and Windows/Linux
    /// behavior does not. The needles are assembled at runtime so this
    /// test's own source does not match them.
    #[test]
    fn only_macos_conditionals_appear() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut needles = Vec::new();
        for word in ["windows", "unix"] {
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
                            "non-macOS conditional {needle:?} found in {}",
                            path.display()
                        );
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked >= 4, "expected to scan the crate's source files");
    }
}
