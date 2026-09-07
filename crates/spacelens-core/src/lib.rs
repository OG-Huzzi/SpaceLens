//! SpaceLens core engine — Phase 0 scaffold.
//!
//! Contains ONLY:
//! - the versioned IPC contract types shared with the frontend (`contract`),
//! - the SQLite bootstrap used to validate the storage approach (`db`).
//!
//! The product engine (scanner, classifier, recommender, safety, …) is
//! explicitly NOT implemented here. See `docs/ARCHITECTURE.md` and
//! `progress/PHASES.md` — implementation starts in Phase 1.

pub mod contract;
pub mod db;
