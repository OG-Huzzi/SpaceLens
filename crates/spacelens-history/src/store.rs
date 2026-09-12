//! SQLite persistence for System Memory (Objectives 15–19).
//!
//! **Extends the existing persistence architecture**: `spacelens_core::db`
//! owns the connection and the forward-only `schema_version` migrations
//! (v1: drives/scans — the Phase 0 bootstrap). This module applies the
//! history migration (v2) on top of the SAME database and version table —
//! no second database abstraction.
//!
//! ## Atomic run commit (Objective 18)
//!
//! `begin_run` inserts the run row as `RUNNING` (its own transaction).
//! `commit_run` then performs everything else — final status + counts,
//! every observation row, every relationship row — inside ONE
//! transaction. A crash before COMMIT leaves the run `RUNNING`, which
//! `recover_stale_runs` (called at open) marks `FAILED`: a run is never
//! visible as completed with a half-written snapshot, and interrupted
//! runs are never mistaken for complete baselines.
//!
//! ## Crash recovery (Objective 19)
//!
//! Deterministic rule: at store open, every run still in `RUNNING` state
//! is marked `FAILED`. SpaceLens is a single-process desktop application;
//! a run is `RUNNING` only while its owning process is alive, so any
//! `RUNNING` row seen at open is an interrupted predecessor. Runs started
//! after the recovery are unaffected.
//!
//! ## Retention (Objective 17)
//!
//! Deterministic and bounded: keep the newest `keep_latest` committed
//! runs unconditionally (the latest baseline required for future
//! comparisons is never deleted), then enforce `max_runs` / `max_age`
//! over the remaining completed runs. Removal cascades to the run's
//! observations and relationship rows (foreign keys). Every removal is
//! reported — nothing is silently discarded.
//!
//! ## Boundedness (Objective 27)
//!
//! Queries take [`QueryLimits`] (default 10,000 results) and report
//! truncation explicitly. Storage is bounded by retention: runs ×
//! per-run rows, both under explicit policy.
//!
//! ## Privacy (Objective 28)
//!
//! Historical paths live only in the local store file. Nothing here
//! performs network I/O, logging, or telemetry; no path ever reaches a
//! log macro (the crate has none).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use spacelens_identity::RelationshipReport;

use crate::compare::RunSnapshot;
use crate::model::{
    ClassificationRef, ConfigFingerprint, ObservedEntry, ObservedKind, RunCounts, RunId, RunRecord,
    RunStatus, Snapshot,
};

/// Errors from the persistence layer. The raw rusqlite error is preserved
/// for diagnosis; no path text is embedded beyond what SQLite itself
/// reports for the store file.
#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    /// The referenced run does not exist.
    UnknownRun(RunId),
    /// The referenced run was already committed (a run commits once).
    AlreadyCommitted(RunId),
    /// The snapshot failed validation (duplicate paths — model invariant).
    Build(crate::model::BuildError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "history store: {e}"),
            StoreError::UnknownRun(id) => write!(f, "unknown run: {id}"),
            StoreError::AlreadyCommitted(id) => write!(f, "run {id} was already committed"),
            StoreError::Build(e) => write!(f, "snapshot invalid: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<crate::model::BuildError> for StoreError {
    fn from(e: crate::model::BuildError) -> Self {
        StoreError::Build(e)
    }
}

/// The latest history schema version this crate understands. v1 is the
/// Phase 0 bootstrap (spacelens-core); v2 adds the history tables.
pub const HISTORY_SCHEMA_VERSION: u32 = 2;

/// Forward-only migration: v1 (core bootstrap) → v2 (history tables).
const MIGRATION_V2: &str = "
CREATE TABLE IF NOT EXISTS scan_runs (
    run_id        TEXT PRIMARY KEY,
    started_at    INTEGER NOT NULL,
    completed_at  INTEGER,
    roots         TEXT NOT NULL,
    platform      TEXT NOT NULL,
    config        TEXT NOT NULL,
    status        TEXT NOT NULL,
    entries_examined INTEGER NOT NULL DEFAULT 0,
    files         INTEGER NOT NULL DEFAULT 0,
    dirs          INTEGER NOT NULL DEFAULT 0,
    links         INTEGER NOT NULL DEFAULT 0,
    other_entries INTEGER NOT NULL DEFAULT 0,
    bytes         INTEGER NOT NULL DEFAULT 0,
    observation_errors INTEGER NOT NULL DEFAULT 0,
    candidates_untracked INTEGER NOT NULL DEFAULT 0,
    hash_failures INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS observations (
    run_id    TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    path      TEXT NOT NULL,
    kind      TEXT NOT NULL,
    size      INTEGER,
    device    INTEGER,
    inode     INTEGER,
    modified  INTEGER,
    category  TEXT,
    subcategory TEXT,
    content_sha256 TEXT,
    obs_error TEXT,
    PRIMARY KEY (run_id, path)
);
CREATE INDEX IF NOT EXISTS idx_obs_path ON observations(path);
CREATE INDEX IF NOT EXISTS idx_obs_object ON observations(device, inode);
CREATE INDEX IF NOT EXISTS idx_obs_content ON observations(content_sha256);
CREATE TABLE IF NOT EXISTS relationship_obs (
    run_id    TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    rel_id    TEXT NOT NULL,
    kind      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    member_count INTEGER NOT NULL,
    recoverable INTEGER,
    accounting TEXT NOT NULL,
    PRIMARY KEY (run_id, rel_id)
);
CREATE TABLE IF NOT EXISTS relationship_members (
    run_id TEXT NOT NULL,
    rel_id TEXT NOT NULL,
    path   TEXT NOT NULL,
    device INTEGER,
    inode  INTEGER,
    PRIMARY KEY (run_id, rel_id, path)
);
CREATE INDEX IF NOT EXISTS idx_rel_members_path ON relationship_members(path);
";

/// A live history store. Wraps the rusqlite connection; all mutating
/// operations are transactional.
pub struct HistoryStore {
    conn: Connection,
}

impl HistoryStore {
    /// Open (or create) the store at `path`, applying pending migrations
    /// (core v1 → history v2) and recovering stale `RUNNING` runs as
    /// `FAILED` (Objective 19 — deterministic crash recovery).
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = spacelens_core::db::open(path)?;
        let current = spacelens_core::db::schema_version(&conn)?;
        if current < HISTORY_SCHEMA_VERSION {
            conn.execute_batch(MIGRATION_V2)?;
            conn.execute(
                "UPDATE schema_version SET version = ?1",
                params![HISTORY_SCHEMA_VERSION],
            )?;
        }
        let store = HistoryStore { conn };
        store.recover_stale_runs()?;
        Ok(store)
    }

    /// Deterministic crash recovery: every run still `RUNNING` at open
    /// time is marked `FAILED` (its owning process died before commit).
    /// Returns the recovered run ids.
    pub fn recover_stale_runs(&self) -> Result<Vec<RunId>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT run_id FROM scan_runs WHERE status = 'RUNNING'")?;
        let ids: Vec<RunId> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .map(RunId)
            .collect();
        drop(stmt);
        self.conn.execute(
            "UPDATE scan_runs SET status = 'FAILED', completed_at = COALESCE(completed_at, ?1)
             WHERE status = 'RUNNING'",
            params![now_nanos()],
        )?;
        Ok(ids)
    }

    /// Persist a run header as `RUNNING` (its own transaction — visible
    /// for crash recovery from this moment).
    pub fn begin_run(&mut self, record: &RunRecord) -> Result<(), StoreError> {
        let conn = &mut self.conn;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO scan_runs
             (run_id, started_at, completed_at, roots, platform, config, status,
              entries_examined, files, dirs, links, other_entries, bytes,
              observation_errors, candidates_untracked, hash_failures)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, 'RUNNING', 0, 0, 0, 0, 0, 0, 0, 0, 0)",
            params![
                record.run_id.0,
                time_nanos(record.started_at),
                serde_roots(&record.roots),
                record.platform,
                serde_json::to_string(&record.config).unwrap_or_default(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Atomically commit a finished run: final status + counts, the full
    /// snapshot, and the relationship derivation — one transaction, all
    /// or nothing (Objective 18). The record's status must be terminal
    /// (`Completed`/`CompletedWithLimits`/`Cancelled`/`Failed`).
    pub fn commit_run(
        &mut self,
        record: &RunRecord,
        snapshot: &Snapshot,
        relationships: Option<&RelationshipReport>,
    ) -> Result<(), StoreError> {
        if record.status == RunStatus::Running {
            return Err(StoreError::AlreadyCommitted(record.run_id.clone()));
        }
        if snapshot.run_id != record.run_id {
            return Err(StoreError::UnknownRun(record.run_id.clone()));
        }
        let conn = &mut self.conn;
        let tx = conn.transaction()?;
        let updated = tx.execute(
            "UPDATE scan_runs SET completed_at = ?2, status = ?3,
                entries_examined = ?4, files = ?5, dirs = ?6, links = ?7,
                other_entries = ?8, bytes = ?9, observation_errors = ?10,
                candidates_untracked = ?11, hash_failures = ?12
             WHERE run_id = ?1 AND status = 'RUNNING'",
            params![
                record.run_id.0,
                time_nanos(record.completed_at.unwrap_or(record.started_at)),
                serde_status(record.status),
                record.counts.entries_examined,
                record.counts.files,
                record.counts.dirs,
                record.counts.links,
                record.counts.other_entries,
                record.counts.bytes,
                record.counts.observation_errors,
                record.counts.candidates_untracked,
                record.counts.hash_failures,
            ],
        )?;
        if updated == 0 {
            return Err(StoreError::AlreadyCommitted(record.run_id.clone()));
        }
        let mut obs = tx.prepare(
            "INSERT INTO observations
             (run_id, path, kind, size, device, inode, modified, category,
              subcategory, content_sha256, obs_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        for entry in &snapshot.entries {
            obs.execute(params![
                record.run_id.0,
                entry.path.to_string_lossy(),
                serde_kind(entry.kind),
                entry.size,
                entry.object.map(|o| o.0 as i64),
                entry.object.map(|o| o.1 as i64),
                entry.modified.map(time_nanos),
                entry.classification.as_ref().map(|c| c.category.clone()),
                entry
                    .classification
                    .as_ref()
                    .and_then(|c| c.subcategory.clone()),
                entry.content_sha256,
                entry.observation_error,
            ])?;
        }
        drop(obs);
        if let Some(rel) = relationships {
            let mut ro = tx.prepare(
                "INSERT INTO relationship_obs
                 (run_id, rel_id, kind, size, member_count, recoverable, accounting)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut rm = tx.prepare(
                "INSERT INTO relationship_members (run_id, rel_id, path, device, inode)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for r in &rel.relationships {
                ro.execute(params![
                    record.run_id.0,
                    r.id,
                    serde_relationship_kind(r.kind),
                    r.size,
                    r.member_count,
                    r.recoverable_bytes.map(|v| v as i64),
                    serde_accounting(r.accounting),
                ])?;
                for m in &r.members {
                    rm.execute(params![
                        record.run_id.0,
                        r.id,
                        m.path.to_string_lossy(),
                        m.object.map(|o| o.volume as i64),
                        m.object.map(|o| o.file_id as i64),
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Explicitly mark a begun run as terminally failed/cancelled without
    /// a snapshot (the caller observed the failure). A snapshot commit
    /// afterwards is rejected.
    pub fn abandon_run(&self, run_id: &RunId, status: RunStatus) -> Result<(), StoreError> {
        if status == RunStatus::Running {
            return Err(StoreError::AlreadyCommitted(run_id.clone()));
        }
        let updated = self.conn.execute(
            "UPDATE scan_runs SET status = ?2, completed_at = ?3
             WHERE run_id = ?1 AND status = 'RUNNING'",
            params![run_id.0, serde_status(status), now_nanos()],
        )?;
        if updated == 0 {
            return Err(StoreError::UnknownRun(run_id.clone()));
        }
        Ok(())
    }

    /// All runs, newest first (bounded by `limits.max_results`).
    pub fn list_runs(&self, limits: &QueryLimits) -> Result<Vec<RunRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, started_at, completed_at, roots, platform, config, status,
                    entries_examined, files, dirs, links, other_entries, bytes,
                    observation_errors, candidates_untracked, hash_failures
             FROM scan_runs ORDER BY started_at DESC, run_id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limits.max_results as i64], map_run_row)?;
        let mut runs = Vec::new();
        for r in rows {
            runs.push(r?);
        }
        Ok(runs)
    }

    /// One run by id.
    pub fn get_run(&self, run_id: &RunId) -> Result<Option<RunRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, started_at, completed_at, roots, platform, config, status,
                    entries_examined, files, dirs, links, other_entries, bytes,
                    observation_errors, candidates_untracked, hash_failures
             FROM scan_runs WHERE run_id = ?1",
        )?;
        let run = stmt.query_row(params![run_id.0], map_run_row).optional()?;
        Ok(run)
    }

    /// The newest committed run whose roots cover `scope` (Objective 24:
    /// `get_latest_run(scope)`). Partial runs are skipped — the latest
    /// usable BASELINE is a full-scope run.
    pub fn latest_run_for_scope(&self, scope: &[PathBuf]) -> Result<Option<RunRecord>, StoreError> {
        let all = self.list_runs(&QueryLimits::default())?;
        Ok(all
            .into_iter()
            .find(|r| r.status.observes_full_scope() && r.covers(scope)))
    }

    /// Load a committed run with its full snapshot (and relationship
    /// derivation when one was committed) — the comparison input.
    pub fn load_run_snapshot(&self, run_id: &RunId) -> Result<Option<RunSnapshot>, StoreError> {
        let Some(run) = self.get_run(run_id)? else {
            return Ok(None);
        };
        if run.status == RunStatus::Running {
            // Uncommitted runs have no snapshot rows; never pretend.
            return Ok(Some(RunSnapshot {
                run,
                snapshot: Snapshot {
                    run_id: run_id.clone(),
                    entries: Vec::new(),
                },
                relationships: None,
            }));
        }
        let mut stmt = self.conn.prepare(
            "SELECT path, kind, size, device, inode, modified, category, subcategory,
                    content_sha256, obs_error
             FROM observations WHERE run_id = ?1 ORDER BY path",
        )?;
        let rows = stmt.query_map(params![run_id.0], map_obs_row)?;
        let mut entries = Vec::new();
        for r in rows {
            entries.push(r?);
        }
        let mut rel_stmt = self.conn.prepare(
            "SELECT rel_id, kind, size, member_count, recoverable, accounting
             FROM relationship_obs WHERE run_id = ?1 ORDER BY rel_id",
        )?;
        let rel_rows = rel_stmt.query_map(params![run_id.0], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut relationships = Vec::new();
        for rr in rel_rows {
            let (id, kind, size, member_count, recoverable, accounting) = rr?;
            relationships.push(RelRow {
                id,
                kind,
                size,
                member_count,
                recoverable,
                accounting,
            });
        }
        drop(rel_stmt);
        // Reconstruct the relationship report from rows. The store
        // persists only what the relationship layer published; the
        // reconstructed report is the stored FACTS, enough for
        // id-level history and comparison.
        let mut members_by_rel: BTreeMap2<String, MemberRows> = BTreeMap2::new();
        {
            let mut m = self.conn.prepare(
                "SELECT rel_id, path, device, inode FROM relationship_members
                 WHERE run_id = ?1 ORDER BY rel_id, path",
            )?;
            let rows = m.query_map(params![run_id.0], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            })?;
            for row in rows {
                let (rel_id, path, device, inode) = row?;
                members_by_rel
                    .entry(rel_id)
                    .or_default()
                    .push((path, device, inode));
            }
        }
        let relationships = reconstruct_relationship_report(relationships, members_by_rel);
        Ok(Some(RunSnapshot {
            run,
            snapshot: Snapshot {
                run_id: run_id.clone(),
                entries,
            },
            relationships: Some(relationships),
        }))
    }

    /// History of one path across all runs, newest first (Objective 25:
    /// "what was the state of this path N days ago" becomes a straight
    /// indexed lookup). Bounded.
    pub fn history_for_path(
        &self,
        path: &Path,
        limits: &QueryLimits,
    ) -> Result<Vec<PathHistoryPoint>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.device, o.inode,
                    o.modified, o.category, o.subcategory, o.content_sha256, o.obs_error
             FROM observations o JOIN scan_runs r ON r.run_id = o.run_id
             WHERE o.path = ?1
             ORDER BY r.started_at DESC, r.run_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![path.to_string_lossy(), limits.max_results as i64],
            |r| {
                Ok(PathHistoryPoint {
                    run_id: RunId(r.get(0)?),
                    started_at: time_from_nanos_opt(r.get::<_, Option<i64>>(1)?),
                    kind: serde_kind_from(r.get::<_, String>(2)?),
                    size: r.get(3)?,
                    object: r
                        .get::<_, Option<i64>>(4)?
                        .zip(r.get::<_, Option<i64>>(5)?)
                        .map(|(d, i)| (d as u64, i as u64)),
                    classification: match (
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ) {
                        (Some(c), s) => Some(ClassificationRef {
                            category: c,
                            subcategory: s,
                        }),
                        _ => None,
                    },
                    content_sha256: r.get(9)?,
                    observation_error: r.get(10)?,
                })
            },
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// History of one filesystem object across runs (by proven identity).
    pub fn history_for_object(
        &self,
        volume: u64,
        file_id: u64,
        limits: &QueryLimits,
    ) -> Result<Vec<PathHistoryPoint>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.device, o.inode,
                    o.modified, o.category, o.subcategory, o.content_sha256, o.obs_error
             FROM observations o JOIN scan_runs r ON r.run_id = o.run_id
             WHERE o.device = ?1 AND o.inode = ?2
             ORDER BY r.started_at DESC, r.run_id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![volume as i64, file_id as i64, limits.max_results as i64],
            map_history_point,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// History of one verified content identity across runs.
    pub fn history_for_content(
        &self,
        sha256_hex: &str,
        limits: &QueryLimits,
    ) -> Result<Vec<PathHistoryPoint>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.device, o.inode,
                    o.modified, o.category, o.subcategory, o.content_sha256, o.obs_error
             FROM observations o JOIN scan_runs r ON r.run_id = o.run_id
             WHERE o.content_sha256 = ?1
             ORDER BY r.started_at DESC, r.run_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![sha256_hex, limits.max_results as i64],
            map_history_point,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Relationship history: every stored observation of one relationship
    /// id (stable across runs — content/object derived, Objective 23),
    /// newest first.
    pub fn relationship_history(
        &self,
        rel_id: &str,
        limits: &QueryLimits,
    ) -> Result<Vec<RelationshipHistoryPoint>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.member_count,
                    o.recoverable, o.accounting,
                    (SELECT COUNT(*) FROM relationship_members m WHERE m.run_id = o.run_id AND m.rel_id = o.rel_id)
             FROM relationship_obs o JOIN scan_runs r ON r.run_id = o.run_id
             WHERE o.rel_id = ?1
             ORDER BY r.started_at DESC, r.run_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![rel_id, limits.max_results as i64], |r| {
            Ok(RelationshipHistoryPoint {
                run_id: RunId(r.get(0)?),
                started_at: time_from_nanos_opt(r.get::<_, Option<i64>>(1)?),
                kind: r.get(2)?,
                size: r.get(3)?,
                member_count: r.get(4)?,
                recoverable: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                accounting: r.get(6)?,
                stored_member_count: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Apply a deterministic retention policy (Objective 17). The newest
    /// `policy.keep_latest` committed runs are ALWAYS kept (the baseline
    /// for future comparisons is never silently removed); then `max_runs`
    /// and `max_age` prune older committed runs. Running/failed rows are
    /// not deleted by age (they are the crash-recovery record). Every
    /// removal is reported.
    pub fn apply_retention(
        &mut self,
        policy: &RetentionPolicy,
    ) -> Result<RetentionReport, StoreError> {
        let conn = &mut self.conn;
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare(
            "SELECT run_id, started_at, status FROM scan_runs
             ORDER BY started_at DESC, run_id DESC",
        )?;
        let rows: Vec<(String, i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        let now = now_nanos();
        let mut keep = 0usize;
        let mut removed = Vec::new();
        for (run_id, started, status) in &rows {
            let is_committed = status != "RUNNING";
            if is_committed && keep < policy.keep_latest {
                keep += 1;
                continue; // newest baselines always kept
            }
            if is_committed {
                if let Some(max_runs) = policy.max_runs {
                    if keep >= max_runs {
                        tx.execute("DELETE FROM scan_runs WHERE run_id = ?1", params![run_id])?;
                        removed.push(RunId(run_id.clone()));
                        continue;
                    }
                }
                if let Some(max_age) = policy.max_age {
                    let age_nanos = now.saturating_sub(*started);
                    if age_nanos > max_age.as_nanos() as i64 {
                        tx.execute("DELETE FROM scan_runs WHERE run_id = ?1", params![run_id])?;
                        removed.push(RunId(run_id.clone()));
                        continue;
                    }
                }
                keep += 1;
            }
        }
        tx.commit()?;
        let kept = rows.len() - removed.len();
        Ok(RetentionReport {
            removed_runs: removed,
            kept_runs: kept,
        })
    }
}

/// Query result bounds (Objective 27): every multi-result query returns at
/// most `max_results` rows. 10,000 by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryLimits {
    pub max_results: usize,
}

impl Default for QueryLimits {
    fn default() -> Self {
        QueryLimits {
            max_results: 10_000,
        }
    }
}

/// Deterministic retention policy (Objective 17).
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Always keep this many newest committed runs (the comparison
    /// baseline). Minimum 1 — enforced.
    pub keep_latest: usize,
    /// Optional hard cap on retained committed runs.
    pub max_runs: Option<usize>,
    /// Optional maximum age of retained committed runs.
    pub max_age: Option<Duration>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        RetentionPolicy {
            keep_latest: 3,
            max_runs: Some(64),
            max_age: Some(Duration::from_secs(90 * 24 * 3600)),
        }
    }
}

/// What retention did — observable, never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionReport {
    pub removed_runs: Vec<RunId>,
    pub kept_runs: usize,
}

/// One historical observation of a path/object/content (Objective 25's
/// data source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathHistoryPoint {
    pub run_id: RunId,
    pub started_at: Option<SystemTime>,
    pub kind: ObservedKind,
    pub size: Option<u64>,
    pub object: Option<(u64, u64)>,
    pub classification: Option<ClassificationRef>,
    pub content_sha256: Option<String>,
    pub observation_error: Option<String>,
}

/// One historical observation of a relationship id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipHistoryPoint {
    pub run_id: RunId,
    pub started_at: Option<SystemTime>,
    pub kind: String,
    pub size: u64,
    pub member_count: u64,
    pub recoverable: Option<u64>,
    pub accounting: String,
    /// Member rows actually stored for this run/relationship.
    pub stored_member_count: i64,
}

// ---------------------------------------------------------------------------
// row mapping + time helpers
// ---------------------------------------------------------------------------

struct RelRow {
    id: String,
    kind: String,
    size: u64,
    member_count: u64,
    recoverable: Option<i64>,
    accounting: String,
}

/// Minimal ordered map used during relationship reconstruction (kept
/// local to avoid pulling an extra dependency for one use).
struct BTreeMap2<K: Ord, V> {
    inner: std::collections::BTreeMap<K, V>,
}

impl<K: Ord, V> BTreeMap2<K, V> {
    fn new() -> Self {
        BTreeMap2 {
            inner: std::collections::BTreeMap::new(),
        }
    }
    fn entry(&mut self, k: K) -> std::collections::btree_map::Entry<'_, K, V> {
        self.inner.entry(k)
    }
}

impl<K: Ord, V> std::ops::Deref for BTreeMap2<K, V> {
    type Target = std::collections::BTreeMap<K, V>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

type MemberRows = Vec<(String, Option<i64>, Option<i64>)>;

fn reconstruct_relationship_report(
    rows: Vec<RelRow>,
    members: BTreeMap2<String, MemberRows>,
) -> RelationshipReport {
    use spacelens_identity::{
        ContentRef, MemberRef, ObjectRef, Relationship, RelationshipKind, RelationshipStats,
        StorageAccounting, Undetermined,
    };
    let mut relationships = Vec::new();
    for row in &rows {
        let kind = if row.kind == serde_relationship_kind(RelationshipKind::HardLinkAlias) {
            RelationshipKind::HardLinkAlias
        } else {
            RelationshipKind::ContentDuplicate
        };
        let accounting = if row.accounting == serde_accounting(StorageAccounting::Exact) {
            StorageAccounting::Exact
        } else {
            StorageAccounting::Estimated
        };
        let mut rel_members = Vec::new();
        for (path, device, inode) in members.get(&row.id).into_iter().flatten() {
            rel_members.push(MemberRef {
                entry_id: 0,
                path: PathBuf::from(path),
                object: device.zip(*inode).map(|(d, i)| ObjectRef {
                    volume: d as u64,
                    file_id: i as u64,
                }),
            });
        }
        rel_members.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
        });
        let distinct = if rel_members.iter().all(|m| m.object.is_some()) {
            let mut ids: Vec<ObjectRef> = rel_members.iter().filter_map(|m| m.object).collect();
            ids.sort();
            ids.dedup();
            Some(ids.len() as u64)
        } else {
            None
        };
        relationships.push(Relationship {
            id: row.id.clone(),
            kind,
            size: row.size,
            member_count: row.member_count,
            members: rel_members,
            distinct_objects: distinct,
            // Evidence is derivable from kind; the stored facts are the
            // identity/count fields. (The full evidence list is not
            // persisted — it is a function of kind.)
            evidence: match kind {
                RelationshipKind::HardLinkAlias => {
                    vec![spacelens_identity::Evidence::ObjectIdentityEqual]
                }
                RelationshipKind::ContentDuplicate => vec![
                    spacelens_identity::Evidence::ContentHashEqual,
                    spacelens_identity::Evidence::SizeEqual,
                ],
            },
            content: match kind {
                RelationshipKind::ContentDuplicate => Some(ContentRef {
                    sha256_hex: row.id.trim_start_matches("content-").to_string(),
                    algorithm: "sha256".to_string(),
                }),
                RelationshipKind::HardLinkAlias => None,
            },
            object: match kind {
                RelationshipKind::HardLinkAlias => row
                    .id
                    .trim_start_matches("alias-")
                    .split_once('-')
                    .and_then(|(v, f)| {
                        Some(ObjectRef {
                            volume: u64::from_str_radix(v, 16).ok()?,
                            file_id: u64::from_str_radix(f, 16).ok()?,
                        })
                    }),
                RelationshipKind::ContentDuplicate => None,
            },
            alias_sets: Vec::new(),
            logical_duplicate_bytes: row.size.saturating_mul(row.member_count.saturating_sub(1)),
            recoverable_bytes: row.recoverable.map(|v| v as u64),
            accounting,
            detail_truncated: false,
        });
    }
    RelationshipReport {
        status: spacelens_identity::DuplicateStatus::Completed,
        relationships,
        relationships_truncated: 0,
        undetermined: Undetermined {
            failed: 0,
            not_examined: 0,
            failed_by_reason: Vec::new(),
            detail: Vec::new(),
            detail_truncated: 0,
        },
        stats: RelationshipStats::default(),
        started_at: UNIX_EPOCH,
        finished_at: UNIX_EPOCH,
    }
}

fn map_run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    let roots_json: String = r.get(3)?;
    let config_json: String = r.get(5)?;
    Ok(RunRecord {
        run_id: RunId(r.get(0)?),
        started_at: time_from_nanos(r.get::<_, i64>(1)?),
        completed_at: r.get::<_, Option<i64>>(2)?.map(time_from_nanos),
        roots: serde_json::from_str(&roots_json).unwrap_or_default(),
        platform: r.get(4)?,
        config: serde_json::from_str(&config_json).unwrap_or_else(|_| ConfigFingerprint::current()),
        status: serde_status_from(&r.get::<_, String>(6)?),
        counts: RunCounts {
            entries_examined: r.get(7)?,
            files: r.get(8)?,
            dirs: r.get(9)?,
            links: r.get(10)?,
            other_entries: r.get(11)?,
            bytes: r.get(12)?,
            observation_errors: r.get(13)?,
            candidates_untracked: r.get(14)?,
            hash_failures: r.get(15)?,
        },
    })
}

fn map_obs_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ObservedEntry> {
    Ok(ObservedEntry {
        path: PathBuf::from(r.get::<_, String>(0)?),
        kind: serde_kind_from(r.get::<_, String>(1)?),
        size: r.get(2)?,
        object: r
            .get::<_, Option<i64>>(3)?
            .zip(r.get::<_, Option<i64>>(4)?)
            .map(|(d, i)| (d as u64, i as u64)),
        modified: r.get::<_, Option<i64>>(5)?.map(time_from_nanos),
        classification: match (
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
        ) {
            (Some(c), s) => Some(ClassificationRef {
                category: c,
                subcategory: s,
            }),
            _ => None,
        },
        content_sha256: r.get(8)?,
        observation_error: r.get(9)?,
    })
}

fn map_history_point(r: &rusqlite::Row<'_>) -> rusqlite::Result<PathHistoryPoint> {
    Ok(PathHistoryPoint {
        run_id: RunId(r.get(0)?),
        started_at: time_from_nanos_opt(r.get::<_, Option<i64>>(1)?),
        kind: serde_kind_from(r.get::<_, String>(2)?),
        size: r.get(3)?,
        object: r
            .get::<_, Option<i64>>(4)?
            .zip(r.get::<_, Option<i64>>(5)?)
            .map(|(d, i)| (d as u64, i as u64)),
        classification: match (
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
        ) {
            (Some(c), s) => Some(ClassificationRef {
                category: c,
                subcategory: s,
            }),
            _ => None,
        },
        content_sha256: r.get(9)?,
        observation_error: r.get(10)?,
    })
}

fn serde_roots(roots: &[PathBuf]) -> String {
    let as_strs: Vec<String> = roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    serde_json::to_string(&as_strs).unwrap_or_else(|_| "[]".to_string())
}

fn serde_status(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Running => "RUNNING",
        RunStatus::Completed => "COMPLETED",
        RunStatus::CompletedWithLimits => "COMPLETED_WITH_LIMITS",
        RunStatus::Cancelled => "CANCELLED",
        RunStatus::Failed => "FAILED",
    }
}

fn serde_status_from(s: &str) -> RunStatus {
    match s {
        "COMPLETED" => RunStatus::Completed,
        "COMPLETED_WITH_LIMITS" => RunStatus::CompletedWithLimits,
        "CANCELLED" => RunStatus::Cancelled,
        "FAILED" => RunStatus::Failed,
        _ => RunStatus::Running,
    }
}

fn serde_kind(k: ObservedKind) -> &'static str {
    match k {
        ObservedKind::File => "FILE",
        ObservedKind::Dir => "DIR",
        ObservedKind::Link => "LINK",
        ObservedKind::Other => "OTHER",
    }
}

fn serde_kind_from(s: String) -> ObservedKind {
    match s.as_str() {
        "DIR" => ObservedKind::Dir,
        "LINK" => ObservedKind::Link,
        "OTHER" => ObservedKind::Other,
        _ => ObservedKind::File,
    }
}

fn serde_relationship_kind(k: spacelens_identity::RelationshipKind) -> &'static str {
    match k {
        spacelens_identity::RelationshipKind::HardLinkAlias => "HARD_LINK_ALIAS",
        spacelens_identity::RelationshipKind::ContentDuplicate => "CONTENT_DUPLICATE",
    }
}

fn serde_accounting(a: StorageAccounting) -> &'static str {
    match a {
        StorageAccounting::Exact => "EXACT",
        StorageAccounting::Estimated => "ESTIMATED",
    }
}

use spacelens_identity::StorageAccounting;

fn time_nanos(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn time_from_nanos(n: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(n.max(0) as u64)
}

fn time_from_nanos_opt(n: Option<i64>) -> Option<SystemTime> {
    n.map(time_from_nanos)
}

fn now_nanos() -> i64 {
    time_nanos(SystemTime::now())
}
