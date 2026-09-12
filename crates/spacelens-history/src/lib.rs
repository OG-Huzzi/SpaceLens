//! SpaceLens system memory — Phase 5 (docs/HISTORY.md).
//!
//! The REMEMBER layer on top of Observe → Classify → Identify → Relate:
//! SpaceLens remembers previous observations and can explain what changed
//! between them.
//!
//! Design contracts honored here:
//! - **Pure comparison engine**: `compare()` derives typed, evidence-backed
//!   change events from two snapshots — no database access, no clock, no
//!   randomness; deterministic output.
//! - **Incomplete-scan safety**: created/deleted claims require both runs
//!   to have observed their full declared scope. A cancelled or failed
//!   run's missing paths are UNCERTAIN — never mass deletions.
//! - **Scope awareness**: comparisons are refused unless the target run's
//!   roots cover the source run's scope.
//! - **Configuration versioning**: every run persists the configuration
//!   fingerprint (observation model, classifier schema + rules version,
//!   hash algorithm, schemas) that produced it; old records stay
//!   interpretable.
//! - **Object continuity**: moves/renames are claimed only from proven
//!   filesystem object identity — never from names, sizes, or timestamps.
//!   Delete+recreate with identical bytes is delete+create, not a move.
//! - **Transactional commit**: a run becomes visible as completed only
//!   when its entire snapshot + relationship set committed atomically;
//!   interrupted runs are recovered as `Failed`, never as completed.
//! - **Bounded, deterministic retention** with the latest baseline kept.
//! - **Privacy**: historical paths stay in the local store; no network,
//!   no telemetry, no AI; nothing logs paths.
//!
//! This layer reports facts and evidence only — no recommendations, no
//! cleanup, no destructive operations (later phases, per the master plan).
//!
//! Contract namespace: `spacelens.v1.history.*` (docs/HISTORY.md,
//! docs/API_CONTRACTS.md).

pub mod compare;
pub mod model;
pub mod store;

pub use compare::{
    compare, ChangeCounts, ChangeEvent, ChangeSet, CompareError, CompareOptions,
    ComparisonCompleteness, EventEvidence, EventKind, RunSnapshot,
};
pub use model::{
    path_covers, BuildError, ClassificationRef, ConfigFingerprint, ObservedEntry, ObservedKind,
    RunCounts, RunId, RunRecord, RunStatus, Snapshot, SnapshotBuilder,
};
pub use store::{
    HistoryStore, PathHistoryPoint, QueryLimits, RetentionPolicy, RetentionReport, StoreError,
};
