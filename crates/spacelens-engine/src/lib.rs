//! SpaceLens filesystem engine — Phase 1.
//!
//! A safe, streaming, bounded-concurrency filesystem scanner. This crate is
//! the bottom of the engine dependency chain (docs/ARCHITECTURE.md): it knows
//! nothing of categories, recommendations, the database, the UI, or the
//! network. Later phases (classifier, hasher, recommender, planner, safety,
//! history) plug in downstream of it.
//!
//! Pipeline: scan request → traversal → metadata → normalized entries →
//! progress events → cancellation → typed final result.
//!
//! Design contracts honored here:
//! - Platform behavior hides behind [`platform::PlatformFs`] and
//!   [`platform::DriveInfo`]; shared scanner code never branches on the OS.
//! - Symlinks/junctions/reparse points are recorded, never followed by
//!   default (see [`options::SymlinkPolicy`]).
//! - One inaccessible entry never fails the scan; errors are typed and tallied.
//! - Cancellation is a first-class engine mechanism ([`cancel::CancelHandle`]).
//! - Concurrency is bounded ([`options::ScanOptions::threads`]); entries are
//!   streamed to the caller and never buffered into a whole-tree collection.

pub mod cancel;
pub mod error;
pub mod identity;
pub mod model;
pub mod options;
pub mod platform;
pub mod progress;
pub mod scanner;
pub mod summary;

pub use cancel::CancelHandle;
pub use error::{ErrorCategory, ScanError, ScanErrorReport};
pub use identity::FileIdentity;
pub use model::{EntryKind, ErrorCategoryRef, FsEntry, LinkInfo, LinkKind};
pub use options::{ScanOptions, SymlinkPolicy};
pub use platform::{DriveInfo, PlatformFs, SysDirs};
pub use progress::{Phase, ProgressSnapshot, ScanEvent};
pub use scanner::{default_threads, scan, scan_with};
pub use summary::{ScanStatus, ScanSummary};
