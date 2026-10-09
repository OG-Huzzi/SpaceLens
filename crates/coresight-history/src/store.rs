//! SQLite persistence for System Memory (Objectives 15–19).
//!
//! **Extends the existing persistence architecture**: `coresight_core::db`
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
//! is marked `FAILED`. CoreSight is a single-process desktop application;
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

use coresight_identity::pipeline::DuplicateStatus;
use coresight_identity::RelationshipReport;

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
    /// The database records a schema version NEWER than this build
    /// understands. Opening it would downgrade or misread historical
    /// facts, so the store refuses instead. Forward-only means
    /// forward-only: an older binary never interprets a newer store.
    SchemaTooNew {
        found: u32,
        supported: u32,
    },
    /// A persisted value failed to decode and would otherwise have
    /// become a fabricated fact. Corruption is surfaced, never masked.
    Corrupt {
        table: &'static str,
        column: &'static str,
        run_id: Option<String>,
        detail: String,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "history store: {e}"),
            StoreError::UnknownRun(id) => write!(f, "unknown run: {id}"),
            StoreError::AlreadyCommitted(id) => write!(f, "run {id} was already committed"),
            StoreError::Build(e) => write!(f, "snapshot invalid: {e}"),
            StoreError::SchemaTooNew { found, supported } => write!(
                f,
                "store schema version {found} is newer than this build supports ({supported}); \
                 refusing to open (a newer CoreSight wrote this store)"
            ),
            StoreError::Corrupt {
                table,
                column,
                run_id,
                detail,
            } => match run_id {
                Some(id) => write!(
                    f,
                    "corrupt history data in {table}.{column} (run {id}): {detail}"
                ),
                None => write!(f, "corrupt history data in {table}.{column}: {detail}"),
            },
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
/// Phase 0 bootstrap (coresight-core); v2 adds the history tables; v3
/// (Phase 5.1) widens object identity with `file_id_hi` and tags every
/// persisted path with its storage encoding; v4 persists each run's
/// relationship-report status/truncation so a reloaded run can never
/// claim a relationship completeness it never had; v5 (Phase 6.4) adds
/// the normalized application/system snapshot tables
/// (`app_snapshot_*`) so canonical application-intelligence facts and
/// system-model inputs survive a reload deterministically.
pub const HISTORY_SCHEMA_VERSION: u32 = 5;

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

/// Forward-only migration: v2 → v3 (Phase 5.1). Applied atomically with
/// the version bump inside the same transaction:
///
/// - `observations.file_id_hi` / `relationship_members.file_id_hi` — the
///   wide-identity high bits. Every pre-existing row keeps `NULL`: the
///   high component was never stored by v2 and is NEVER fabricated.
/// - the object index widened to `(device, inode, file_id_hi)`.
/// - every pre-existing path value tagged with its storage encoding: rows
///   containing U+FFFD are `l:`-tagged (the legacy lossy spelling,
///   preserved exactly as the v2 store wrote it — those bytes are
///   unrecoverable and are not invented); all other rows are `u:`-tagged
///   (a v2 string without U+FFFD is provably the path verbatim).
///
/// Forward-only and idempotent-per-state: the version table gates it, and
/// each step is safe to re-apply only within the transaction that bumps
/// the version.
const MIGRATION_V3: &str = "
ALTER TABLE observations ADD COLUMN file_id_hi INTEGER;
ALTER TABLE relationship_members ADD COLUMN file_id_hi INTEGER;
DROP INDEX IF EXISTS idx_obs_object;
CREATE INDEX IF NOT EXISTS idx_obs_object ON observations(device, inode, file_id_hi);
UPDATE observations
   SET path = CASE WHEN instr(path, '\u{FFFD}') > 0
                   THEN 'l:' || path ELSE 'u:' || path END;
UPDATE relationship_members
   SET path = CASE WHEN instr(path, '\u{FFFD}') > 0
                   THEN 'l:' || path ELSE 'u:' || path END;
";

/// Forward-only migration: v3 → v4. Persists the relationship report's
/// status and truncation count on the run row:
///
/// - `rel_status` NULL means "no relationship report was recorded for
///   this run" — honest for every pre-v4 row (the old schema could not
///   distinguish "never ran the relationship layer" from "ran and found
///   nothing", and inventing `COMPLETED` on load would fabricate
///   relationship-completeness facts that drive change events).
/// - `rel_truncated` NULL is likewise "not recorded".
///
/// Legacy rows keep NULL; nothing is fabricated. Runs committed from v4
/// on always record the report's real status (including
/// `CANCELLED`/`UNSUPPORTED`), so comparisons can never treat a partial
/// relationship derivation as complete after a reload.
const MIGRATION_V4: &str = "
ALTER TABLE scan_runs ADD COLUMN rel_status TEXT;
ALTER TABLE scan_runs ADD COLUMN rel_truncated INTEGER;
";

/// Forward-only migration: v4 → v5 (Phase 6.4). Adds the normalized
/// application/system snapshot tables (`app_snapshot_*`).
///
/// Design notes (see [`crate::snapshot`] for the persistence boundary):
///
/// - Every snapshot row carries its `run_id`: snapshot knowledge is
///   per-run history, never globally mutable "current" state. All child
///   rows reference `scan_runs(run_id) ON DELETE CASCADE`, so retention
///   prunes snapshots together with their runs — one coherent memory.
/// - No JSON blobs: every domain fact (application fields, provenance,
///   install roots, evidence columns, coverage, relationships, footprints)
///   is its own column, individually queryable and strictly decodable.
/// - Ordinal columns (`fact_ord`, `artifact_ord`, …) are stable row join
///   keys, never semantics: the builder's inputs are MULTISETS (duplicate
///   facts merge commutatively), so verbatim facts — including duplicates
///   — persist under ordinals assigned after a canonical sort. Reload
///   re-sorts canonically, so ordinal VALUES never affect the model.
/// - No derived state: edges, indexes, insights, candidates, and
///   authorization are NOT stored — they are rebuilt by the canonical
///   builder on reload.
/// - `CREATE TABLE IF NOT EXISTS` matches the existing migration
///   convention (safe re-entry within the gating transaction).
const MIGRATION_V5: &str = "
CREATE TABLE IF NOT EXISTS app_snapshot_meta (
    run_id TEXT PRIMARY KEY REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    records_truncated INTEGER NOT NULL,
    records_rejected INTEGER NOT NULL,
    fp_candidates_truncated INTEGER NOT NULL,
    fp_children_truncated INTEGER NOT NULL,
    fp_apps_truncated INTEGER NOT NULL,
    fp_evidence_truncated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS app_snapshot_artifacts (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    artifact_ord INTEGER NOT NULL,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    size INTEGER,
    device INTEGER,
    inode INTEGER,
    file_id_hi INTEGER,
    content_sha256 TEXT,
    access TEXT NOT NULL,
    category TEXT,
    subcategory TEXT,
    confidence TEXT,
    PRIMARY KEY (run_id, artifact_ord)
);
CREATE INDEX IF NOT EXISTS idx_app_snap_artifacts_path ON app_snapshot_artifacts(run_id, path);
CREATE TABLE IF NOT EXISTS app_snapshot_apps (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    name TEXT NOT NULL,
    version TEXT,
    publisher TEXT,
    install_location TEXT,
    install_date TEXT,
    estimated_size INTEGER,
    uninstall_string TEXT,
    quiet_uninstall_string TEXT,
    modify_path TEXT,
    install_source TEXT,
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    system_component INTEGER NOT NULL,
    bundle_identifier TEXT,
    executable_path TEXT,
    executable_candidate TEXT,
    PRIMARY KEY (run_id, app_id, fact_ord)
);
CREATE INDEX IF NOT EXISTS idx_app_snap_apps_id ON app_snapshot_apps(run_id, app_id);
CREATE TABLE IF NOT EXISTS app_snapshot_provenance (
    run_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    prov_ord INTEGER NOT NULL,
    source TEXT NOT NULL,
    PRIMARY KEY (run_id, app_id, fact_ord, prov_ord),
    FOREIGN KEY (run_id, app_id, fact_ord)
        REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS app_snapshot_views (
    run_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    view_ord INTEGER NOT NULL,
    view TEXT NOT NULL,
    PRIMARY KEY (run_id, app_id, fact_ord, view_ord),
    FOREIGN KEY (run_id, app_id, fact_ord)
        REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS app_snapshot_roots (
    run_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    root_ord INTEGER NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (run_id, app_id, fact_ord, root_ord),
    FOREIGN KEY (run_id, app_id, fact_ord)
        REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS app_snapshot_evidence (
    run_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    artifact_path TEXT NOT NULL,
    evidence_ord INTEGER NOT NULL,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    strength TEXT NOT NULL,
    group_tag TEXT NOT NULL,
    group_source TEXT,
    scope TEXT NOT NULL,
    observed_path TEXT NOT NULL,
    matched_attribute TEXT NOT NULL,
    matched_value TEXT,
    matched_path TEXT,
    PRIMARY KEY (run_id, app_id, fact_ord, artifact_path, evidence_ord),
    FOREIGN KEY (run_id, app_id, fact_ord)
        REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS app_snapshot_coverage (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    coverage_ord INTEGER NOT NULL,
    source TEXT NOT NULL,
    status TEXT NOT NULL,
    note TEXT,
    PRIMARY KEY (run_id, coverage_ord)
);
CREATE TABLE IF NOT EXISTS app_snapshot_relationships (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    rel_ord INTEGER NOT NULL,
    kind TEXT NOT NULL,
    object_device INTEGER,
    object_inode INTEGER,
    object_hi INTEGER,
    content_sha256 TEXT,
    PRIMARY KEY (run_id, rel_ord)
);
CREATE TABLE IF NOT EXISTS app_snapshot_rel_members (
    run_id TEXT NOT NULL,
    rel_ord INTEGER NOT NULL,
    member_ord INTEGER NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (run_id, rel_ord, member_ord),
    FOREIGN KEY (run_id, rel_ord)
        REFERENCES app_snapshot_relationships(run_id, rel_ord) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS app_snapshot_history (
    run_id TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    hist_ord INTEGER NOT NULL,
    hist_run_id TEXT NOT NULL,
    path TEXT NOT NULL,
    device INTEGER,
    inode INTEGER,
    file_id_hi INTEGER,
    category TEXT,
    PRIMARY KEY (run_id, hist_ord)
);
CREATE TABLE IF NOT EXISTS app_snapshot_footprints (
    run_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    footprint_ord INTEGER NOT NULL,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    confidence TEXT NOT NULL,
    PRIMARY KEY (run_id, app_id, fact_ord, footprint_ord),
    FOREIGN KEY (run_id, app_id, fact_ord)
        REFERENCES app_snapshot_apps(run_id, app_id, fact_ord) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS app_snapshot_footprint_evidence (
    run_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    fact_ord INTEGER NOT NULL,
    footprint_ord INTEGER NOT NULL,
    evidence_ord INTEGER NOT NULL,
    kind TEXT NOT NULL,
    confidence TEXT NOT NULL,
    source TEXT NOT NULL,
    scope TEXT NOT NULL,
    why TEXT NOT NULL,
    PRIMARY KEY (run_id, app_id, fact_ord, footprint_ord, evidence_ord),
    FOREIGN KEY (run_id, app_id, fact_ord, footprint_ord)
        REFERENCES app_snapshot_footprints(run_id, app_id, fact_ord, footprint_ord)
        ON DELETE CASCADE
);
";

/// A live history store. Wraps the rusqlite connection; all mutating
/// operations are transactional.
pub struct HistoryStore {
    pub(crate) conn: Connection,
}

impl HistoryStore {
    /// Open (or create) the store at `path`, applying pending migrations
    /// (core v1 → history v2 → …, each atomic with its version bump) and
    /// recovering stale `RUNNING` runs as `FAILED` (Objective 19 —
    /// deterministic crash recovery).
    ///
    /// `PRAGMA integrity_check` runs FIRST: a corrupt store is refused
    /// ([`StoreError::Corrupt`]) before anything reads or migrates it.
    ///
    /// A store written by a NEWER build is refused
    /// ([`StoreError::SchemaTooNew`]) rather than partially interpreted:
    /// forward-only migrations never run backwards.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut conn = coresight_core::db::open(path)?;
        // docs/DATABASE.md: integrity is checked on open BEFORE anything
        // reads or migrates the file. A corrupt store is refused as a
        // typed error — never partially interpreted, never "recovered" by
        // guessing. (The pre-migration file backup and the explicit
        // quarantine flow remain PLANNED — documented in docs/DATABASE.md.)
        let problems: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA integrity_check")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if problems.iter().any(|p| p != "ok") {
            return Err(StoreError::Corrupt {
                table: "store",
                column: "integrity_check",
                run_id: None,
                detail: format!(
                    "database integrity check failed on open: {}",
                    problems.join("; ")
                ),
            });
        }
        let current = coresight_core::db::schema_version(&conn)?;
        if current > HISTORY_SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: current,
                supported: HISTORY_SCHEMA_VERSION,
            });
        }
        if current < 2 {
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION_V2)?;
            tx.execute("UPDATE schema_version SET version = 2", params![])?;
            tx.commit()?;
        }
        if current < 3 {
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION_V3)?;
            retag_legacy_roots(&tx)?;
            tx.execute("UPDATE schema_version SET version = 3", params![])?;
            tx.commit()?;
        }
        if current < 4 {
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION_V4)?;
            tx.execute("UPDATE schema_version SET version = 4", params![])?;
            tx.commit()?;
        }
        if current < 5 {
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION_V5)?;
            tx.execute("UPDATE schema_version SET version = 5", params![])?;
            tx.commit()?;
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
                candidates_untracked = ?11, hash_failures = ?12,
                rel_status = ?13, rel_truncated = ?14
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
                relationships.map(|r| serde_duplicate_status(r.status)),
                relationships.map(|r| r.relationships_truncated as i64),
            ],
        )?;
        if updated == 0 {
            return Err(StoreError::AlreadyCommitted(record.run_id.clone()));
        }
        let mut obs = tx.prepare(
            "INSERT INTO observations
             (run_id, path, kind, size, device, inode, file_id_hi, modified, category,
              subcategory, content_sha256, obs_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        for entry in &snapshot.entries {
            obs.execute(params![
                record.run_id.0,
                crate::path_encoding::encode(&entry.path),
                serde_kind(entry.kind),
                entry.size,
                entry.object.map(|o| o.device as i64),
                entry.object.map(|o| o.inode as i64),
                entry.object.and_then(|o| o.file_id_hi.map(|hi| hi as i64)),
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
                "INSERT INTO relationship_members (run_id, rel_id, path, device, inode, file_id_hi)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
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
                        crate::path_encoding::encode(&m.path),
                        m.object.map(|o| o.volume as i64),
                        m.object.map(|o| o.file_id as i64),
                        member_file_id_hi(snapshot, &m.path),
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
            "SELECT path, kind, size, device, inode, file_id_hi, modified, category, subcategory,
                    content_sha256, obs_error
             FROM observations WHERE run_id = ?1 ORDER BY path",
        )?;
        let rows = stmt.query_map(params![run_id.0], map_obs_row)?;
        let mut entries = Vec::new();
        for r in rows {
            entries.push(r?);
        }
        // Canonical path order is a Snapshot invariant (the tagged storage
        // spelling orders differently); re-establish it after decode.
        entries.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
        });
        // The relationship report's own status was persisted with the
        // run (v4). A run whose relationship derivation was partial must
        // reload as partial — comparisons never see a completeness the
        // run never had.
        let rel_status = self.rel_status_for_run(run_id)?;
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
                "SELECT rel_id, path, device, inode, file_id_hi FROM relationship_members
                 WHERE run_id = ?1 ORDER BY rel_id, path",
            )?;
            let rows = m.query_map(params![run_id.0], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })?;
            for row in rows {
                let (rel_id, path, device, inode, file_id_hi) = row?;
                members_by_rel
                    .entry(rel_id)
                    .or_default()
                    .push((path, device, inode, file_id_hi));
            }
        }
        let relationships = reconstruct_relationship_report(
            relationships,
            members_by_rel,
            rel_status.status,
            rel_status.truncated.unwrap_or(0).max(0) as u64,
        )
        .map_err(|detail| StoreError::Corrupt {
            table: "relationship_obs",
            column: "kind/accounting/path",
            run_id: Some(run_id.0.clone()),
            detail,
        })?;
        // A run with NO persisted relationship status either never ran
        // the relationship layer, or was written before v4. Absence is
        // not `Completed`: the report is only attached when the store
        // actually recorded one, so comparisons cannot fabricate
        // relationship completeness.
        let relationships = rel_status.status.map(|_| relationships);
        Ok(Some(RunSnapshot {
            run,
            snapshot: Snapshot {
                run_id: run_id.clone(),
                entries,
            },
            relationships,
        }))
    }

    /// The persisted relationship-report status/truncation for a run
    /// (v4 columns). `None` status = no report was recorded; an
    /// unrecognized persisted status is a typed corruption error.
    fn rel_status_for_run(&self, run_id: &RunId) -> Result<RelStatusRow, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT rel_status, rel_truncated FROM scan_runs WHERE run_id = ?1",
                params![run_id.0],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?;
        let Some((status_raw, truncated)) = row else {
            return Ok(RelStatusRow {
                status: None,
                truncated: None,
            });
        };
        let status = match status_raw {
            Some(raw) => {
                Some(
                    decode_duplicate_status(&raw).ok_or_else(|| StoreError::Corrupt {
                        table: "scan_runs",
                        column: "rel_status",
                        run_id: Some(run_id.0.clone()),
                        detail: format!("unknown relationship status {raw:?}"),
                    })?,
                )
            }
            None => None,
        };
        Ok(RelStatusRow { status, truncated })
    }

    /// History of one path across all runs, newest first (Objective 25:
    /// "what was the state of this path N days ago" becomes a straight
    /// indexed lookup). Bounded. The path is looked up by its lossless
    /// storage encoding — non-UTF-8 paths are found exactly.
    pub fn history_for_path(
        &self,
        path: &Path,
        limits: &QueryLimits,
    ) -> Result<Vec<PathHistoryPoint>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.device, o.inode, o.file_id_hi,
                    o.modified, o.category, o.subcategory, o.content_sha256, o.obs_error
             FROM observations o JOIN scan_runs r ON r.run_id = o.run_id
             WHERE o.path = ?1
             ORDER BY r.started_at DESC, r.run_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![
                crate::path_encoding::encode(path),
                limits.max_results as i64
            ],
            map_history_point,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// History of one filesystem object across runs (by proven identity).
    /// Rows that never stored a `file_id_hi` are returned only when the
    /// caller's `file_id_hi` is `None` (legacy pair-only identity).
    pub fn history_for_object(
        &self,
        volume: u64,
        file_id: u64,
        file_id_hi: Option<u64>,
        limits: &QueryLimits,
    ) -> Result<Vec<PathHistoryPoint>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.device, o.inode, o.file_id_hi,
                    o.modified, o.category, o.subcategory, o.content_sha256, o.obs_error
             FROM observations o JOIN scan_runs r ON r.run_id = o.run_id
             WHERE o.device = ?1 AND o.inode = ?2 AND o.file_id_hi IS ?3
             ORDER BY r.started_at DESC, r.run_id DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                volume as i64,
                file_id as i64,
                file_id_hi.map(|hi| hi as i64),
                limits.max_results as i64
            ],
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
            "SELECT r.run_id, r.started_at, o.kind, o.size, o.device, o.inode, o.file_id_hi,
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
                // Committed run beyond the always-keep floor: it is removed
                // when ANY retention rule demands it — including the case
                // where neither optional bound exists (then keep_latest IS
                // the retention count, not merely a floor).
                let over_max_runs = match policy.max_runs {
                    Some(m) => keep >= m,
                    None => false,
                };
                let over_age = match policy.max_age {
                    Some(age) => now.saturating_sub(*started) > age.as_nanos() as i64,
                    None => false,
                };
                let no_bounds_beyond_floor = policy.max_runs.is_none() && policy.max_age.is_none();
                if over_max_runs || over_age || no_bounds_beyond_floor {
                    tx.execute("DELETE FROM scan_runs WHERE run_id = ?1", params![run_id])?;
                    removed.push(RunId(run_id.clone()));
                    continue;
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
    /// Full proven object identity (high bits included; `None` when the
    /// storing run could not prove identity at all).
    pub object: Option<crate::model::ObjectId>,
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

/// The persisted relationship-report status/truncation of one run.
struct RelStatusRow {
    status: Option<DuplicateStatus>,
    truncated: Option<i64>,
}

/// Serde label for the relationship report's own status (v4). Decoding
/// is strict via [`decode_duplicate_status`]: an unknown persisted status
/// is corruption, never silently "Completed".
fn serde_duplicate_status(s: DuplicateStatus) -> &'static str {
    match s {
        DuplicateStatus::Completed => "COMPLETED",
        DuplicateStatus::CompletedWithLimits => "COMPLETED_WITH_LIMITS",
        DuplicateStatus::Cancelled => "CANCELLED",
        DuplicateStatus::Unsupported => "UNSUPPORTED",
    }
}

/// Strict decode for the persisted relationship status.
fn decode_duplicate_status(s: &str) -> Option<DuplicateStatus> {
    match s {
        "COMPLETED" => Some(DuplicateStatus::Completed),
        "COMPLETED_WITH_LIMITS" => Some(DuplicateStatus::CompletedWithLimits),
        "CANCELLED" => Some(DuplicateStatus::Cancelled),
        "UNSUPPORTED" => Some(DuplicateStatus::Unsupported),
        _ => None,
    }
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

type MemberRows = Vec<(String, Option<i64>, Option<i64>, Option<i64>)>;

/// The proven wide-identity high bits for a relationship member path,
/// taken from the run's snapshot (the relationship member row persists the
/// full identity, including the wide high bits where the observation
/// proved them). The snapshot is path-ordered (a `Snapshot` invariant), so
/// this is a bounded binary search per member, and `None` is returned
/// whenever the member was not observed in the snapshot — never fabricated.
fn member_file_id_hi(snapshot: &Snapshot, member_path: &Path) -> Option<i64> {
    let bytes = member_path.as_os_str().as_encoded_bytes();
    snapshot
        .entries
        .binary_search_by(|e| e.path.as_os_str().as_encoded_bytes().cmp(bytes))
        .ok()
        .and_then(|idx| snapshot.entries[idx].object)
        .and_then(|o| o.file_id_hi)
        .map(|hi| hi as i64)
}

fn reconstruct_relationship_report(
    rows: Vec<RelRow>,
    members: BTreeMap2<String, MemberRows>,
    status: Option<DuplicateStatus>,
    relationships_truncated: u64,
) -> Result<RelationshipReport, String> {
    use coresight_identity::{
        ContentRef, MemberRef, ObjectIdentity, Relationship, RelationshipKind, RelationshipStats,
        StorageAccounting, Undetermined,
    };
    // Parse one persisted alias id's object fragment back into the full
    // identity. The fragment is engine-generated ("{volume:016x}-
    // {file_id:016x}" or, for wide identities, "…-hi:{hi:016x}"); anything
    // else is corruption and a typed error — never a silently lost object.
    fn parse_alias_object(id: &str) -> Result<ObjectIdentity, String> {
        let fragment = id
            .strip_prefix("alias-")
            .ok_or_else(|| format!("unknown alias id shape {id:?}"))?;
        let (volume_hex, rest) = fragment
            .split_once('-')
            .ok_or_else(|| format!("unknown alias id shape {id:?}"))?;
        let (file_hex, hi_hex) = match rest.split_once("-hi:") {
            Some((f, hi)) => (f, Some(hi)),
            None => (rest, None),
        };
        let volume = u64::from_str_radix(volume_hex, 16)
            .map_err(|_| format!("malformed alias id volume in {id:?}"))?;
        let file_id = u64::from_str_radix(file_hex, 16)
            .map_err(|_| format!("malformed alias id file id in {id:?}"))?;
        let file_id_hi = match hi_hex {
            Some(h) => Some(
                u64::from_str_radix(h, 16)
                    .map_err(|_| format!("malformed alias id high bits in {id:?}"))?,
            ),
            None => None,
        };
        Ok(ObjectIdentity {
            volume,
            file_id,
            file_id_hi,
        })
    }

    let mut relationships = Vec::new();
    for row in &rows {
        // Strict decode: an unrecognized persisted kind/accounting is
        // corruption. The previous fallback silently relabeled unknown
        // kinds as ContentDuplicate and unknown accounting as Estimated
        // — fabricated facts about historical relationships.
        let kind = if row.kind == serde_relationship_kind(RelationshipKind::HardLinkAlias) {
            RelationshipKind::HardLinkAlias
        } else if row.kind == serde_relationship_kind(RelationshipKind::ContentDuplicate) {
            RelationshipKind::ContentDuplicate
        } else {
            return Err(format!("unknown relationship kind {:?}", row.kind));
        };
        let accounting = if row.accounting == serde_accounting(StorageAccounting::Exact) {
            StorageAccounting::Exact
        } else if row.accounting == serde_accounting(StorageAccounting::Estimated) {
            StorageAccounting::Estimated
        } else {
            return Err(format!("unknown storage accounting {:?}", row.accounting));
        };
        let mut rel_members = Vec::new();
        for (path, device, inode, file_id_hi) in members.get(&row.id).into_iter().flatten() {
            let decoded = crate::path_encoding::decode(path)
                .map_err(|e| format!("malformed member path: {e}"))?
                .ok_or_else(|| "undecodable member path".to_string())?;
            // Restore EXACTLY the identity that was persisted: the wide
            // high bits are part of the member row (migration v4) and are
            // never dropped on reconstruction. A row without high bits
            // stays explicitly narrow (`None`) — no fabricated widening.
            let object = device.zip(*inode).map(|(d, i)| ObjectIdentity {
                volume: d as u64,
                file_id: i as u64,
                file_id_hi: file_id_hi.map(|h| h as u64),
            });
            rel_members.push(MemberRef {
                entry_id: 0,
                path: decoded,
                object,
            });
        }
        rel_members.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
        });
        let distinct = if rel_members.iter().all(|m| m.object.is_some()) {
            // Exact only when every member identity is proven AND no low
            // (volume, file id) pair appears under mixed high-bit
            // provability — the same honesty rule the derivation applies.
            let ids: Vec<ObjectIdentity> = rel_members.iter().filter_map(|m| m.object).collect();
            let mut unique: std::collections::BTreeSet<ObjectIdentity> =
                std::collections::BTreeSet::new();
            let mut low_pairs: std::collections::BTreeMap<
                (u64, u64),
                std::collections::BTreeSet<Option<u64>>,
            > = std::collections::BTreeMap::new();
            for id in &ids {
                unique.insert(*id);
                low_pairs
                    .entry((id.volume, id.file_id))
                    .or_default()
                    .insert(id.file_id_hi);
            }
            if low_pairs
                .values()
                .any(|prov| prov.len() > 1 && prov.contains(&None))
            {
                None
            } else {
                Some(unique.len() as u64)
            }
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
                    vec![coresight_identity::Evidence::ObjectIdentityEqual]
                }
                RelationshipKind::ContentDuplicate => vec![
                    coresight_identity::Evidence::ContentHashEqual,
                    coresight_identity::Evidence::SizeEqual,
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
                RelationshipKind::HardLinkAlias => Some(parse_alias_object(&row.id)?),
                RelationshipKind::ContentDuplicate => None,
            },
            alias_sets: Vec::new(),
            logical_duplicate_bytes: row.size.saturating_mul(row.member_count.saturating_sub(1)),
            recoverable_bytes: row.recoverable.map(|v| v as u64),
            accounting,
            detail_truncated: false,
        });
    }
    Ok(RelationshipReport {
        // The stored status verbatim. The caller only attaches the
        // report when one was actually recorded, so this fallback is
        // never observable — it exists to satisfy the type.
        status: status.unwrap_or(coresight_identity::DuplicateStatus::Unsupported),
        relationships,
        relationships_truncated,
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
    })
}

fn map_run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    let roots_json: String = r.get(3)?;
    let config_json: String = r.get(5)?;
    let run_id: String = r.get(0)?;
    let status_raw: String = r.get(6)?;
    // Persisted-state decoders are strict (Phase 5.1 audit): a persisted
    // fact that cannot be decoded is corruption, not an invitation to
    // invent a default. Unknown status would fabricate "Running"
    // (silently un-completing a run); an undecodable config or roots
    // JSON would fabricate the CURRENT config/boundless scope. All three
    // are typed errors instead.
    let config = serde_json::from_str::<ConfigFingerprint>(&config_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(format!("config fingerprint: {e}"))),
        )
    })?;
    let roots = decode_roots_strict(&roots_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(format!("run roots: {e}"))),
        )
    })?;
    let status = decode_status_strict(&status_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(format!("unknown run status {status_raw:?}"))),
        )
    })?;
    Ok(RunRecord {
        run_id: RunId(run_id),
        started_at: time_from_nanos(r.get::<_, i64>(1)?),
        completed_at: r.get::<_, Option<i64>>(2)?.map(time_from_nanos),
        roots,
        platform: r.get(4)?,
        config,
        status,
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

/// Marker error type for row-level corruption surfaced through
/// `rusqlite::Error::FromSqlConversionFailure`.
#[derive(Debug)]
struct CorruptRow(String);

impl std::fmt::Display for CorruptRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CorruptRow {}

fn map_obs_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ObservedEntry> {
    let stored_path: String = r.get(0)?;
    // Storage decoding is strict: a malformed tagged value (only
    // reachable through direct tampering) is a typed error, never a
    // silently mistyped path.
    let path = crate::path_encoding::decode(&stored_path)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(CorruptRow(format!("malformed stored path: {e}"))),
            )
        })?
        .ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(CorruptRow("undecodable stored path".to_string())),
            )
        })?;
    let kind_raw: String = r.get(1)?;
    let kind = decode_kind_strict(&kind_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(format!("unknown entry kind {kind_raw:?}"))),
        )
    })?;
    Ok(ObservedEntry {
        path,
        kind,
        size: r.get(2)?,
        object: object_from_columns(
            r.get::<_, Option<i64>>(3)?, // device
            r.get::<_, Option<i64>>(4)?, // inode
            r.get::<_, Option<i64>>(5)?, // file_id_hi
        ),
        modified: r.get::<_, Option<i64>>(6)?.map(time_from_nanos),
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

/// Object identity from the storage columns: the pair is required (the
/// pre-v3 rows stored pair-only identity — an honest `file_id_hi: None`,
/// never fabricated), the high bits attach exactly where they were
/// proven and stored.
fn object_from_columns(
    device: Option<i64>,
    inode: Option<i64>,
    file_id_hi: Option<i64>,
) -> Option<crate::model::ObjectId> {
    device.zip(inode).map(|(d, i)| crate::model::ObjectId {
        device: d as u64,
        inode: i as u64,
        file_id_hi: file_id_hi.map(|hi| hi as u64),
    })
}

fn map_history_point(r: &rusqlite::Row<'_>) -> rusqlite::Result<PathHistoryPoint> {
    let kind_raw: String = r.get(2)?;
    let kind = decode_kind_strict(&kind_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(CorruptRow(format!("unknown entry kind {kind_raw:?}"))),
        )
    })?;
    Ok(PathHistoryPoint {
        run_id: RunId(r.get(0)?),
        started_at: time_from_nanos_opt(r.get::<_, Option<i64>>(1)?),
        kind,
        size: r.get(3)?,
        object: object_from_columns(
            r.get::<_, Option<i64>>(4)?, // device
            r.get::<_, Option<i64>>(5)?, // inode
            r.get::<_, Option<i64>>(6)?, // file_id_hi
        ),
        classification: match (
            r.get::<_, Option<String>>(8)?,
            r.get::<_, Option<String>>(9)?,
        ) {
            (Some(c), s) => Some(ClassificationRef {
                category: c,
                subcategory: s,
            }),
            _ => None,
        },
        content_sha256: r.get(10)?,
        observation_error: r.get(11)?,
    })
}

/// Roots serialization for the run row: each root is stored losslessly
/// under its path-storage tag (see [`crate::path_encoding`]), so scope
/// comparisons after a reload see the exact declared roots.
fn serde_roots(roots: &[PathBuf]) -> String {
    let as_strs: Vec<String> = roots
        .iter()
        .map(|p| crate::path_encoding::encode(p))
        .collect();
    serde_json::to_string(&as_strs).unwrap_or_else(|_| "[]".to_string())
}

/// Decode roots written by this store (tagged) or by the pre-repair
/// store (untagged legacy strings — preserved as-represented).
///
/// Strict (Phase 5.1 audit): undecodable JSON or malformed tagged values
/// are errors. The previous `unwrap_or_default()` silently converted a
/// corrupt roots column into "no roots", which would make every scope
/// comparison behave differently than the recorded run actually did.
fn decode_roots_strict(json: &str) -> Result<Vec<PathBuf>, String> {
    let stored: Vec<String> =
        serde_json::from_str(json).map_err(|e| format!("not a string array: {e}"))?;
    let mut roots = Vec::with_capacity(stored.len());
    // Error text never quotes the stored value: paths stay inside the
    // store file, and the store's error contract embeds no path text.
    for (index, value) in stored.iter().enumerate() {
        let decoded = crate::path_encoding::decode(value)
            .map_err(|e| format!("malformed value at index {index}: {e}"))?
            .ok_or_else(|| format!("undecodable value at index {index}"))?;
        roots.push(decoded);
    }
    Ok(roots)
}

/// v2→v3 roots retag: the run-row `roots` JSON of pre-v3 stores holds
/// untagged (possibly lossy) root strings. Tag them with the same rule
/// as the observation paths so the v3 decoder reads them honestly:
/// U+FFFD-bearing values are legacy-lossy, all others are the verbatim
/// root. Applied within the v3 migration transaction only.
///
/// A roots column that cannot be parsed as a string array FAILS the
/// migration (typed error, transaction rolls back): silently retagging
/// it as "no roots" would permanently fabricate an empty scope for that
/// run.
fn retag_legacy_roots(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    let mut stmt = tx.prepare("SELECT run_id, roots FROM scan_runs")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    let mut update = tx.prepare("UPDATE scan_runs SET roots = ?2 WHERE run_id = ?1")?;
    for (run_id, roots_json) in rows {
        let parsed: Vec<String> = serde_json::from_str(&roots_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(CorruptRow(format!(
                    "legacy roots JSON for run {run_id}: {e}"
                ))),
            )
        })?;
        let tagged: Vec<String> = parsed
            .into_iter()
            .map(|root| {
                if root.contains('\u{FFFD}') {
                    format!("l:{root}")
                } else if root.starts_with("u:") || root.starts_with("e:") || root.starts_with("l:")
                {
                    root
                } else {
                    format!("u:{root}")
                }
            })
            .collect();
        update.execute(params![
            run_id,
            serde_json::to_string(&tagged).unwrap_or_default()
        ])?;
    }
    Ok(())
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

/// Strict status decode: an unrecognized persisted status is corruption,
/// not "Running". The old default silently changed a run's meaning.
fn decode_status_strict(s: &str) -> Option<RunStatus> {
    match s {
        "RUNNING" => Some(RunStatus::Running),
        "COMPLETED" => Some(RunStatus::Completed),
        "COMPLETED_WITH_LIMITS" => Some(RunStatus::CompletedWithLimits),
        "CANCELLED" => Some(RunStatus::Cancelled),
        "FAILED" => Some(RunStatus::Failed),
        _ => None,
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

/// Strict entry-kind decode: an unrecognized persisted kind is
/// corruption, not "File". The old default silently relabeled unknown
/// historical entries as files.
fn decode_kind_strict(s: &str) -> Option<ObservedKind> {
    match s {
        "FILE" => Some(ObservedKind::File),
        "DIR" => Some(ObservedKind::Dir),
        "LINK" => Some(ObservedKind::Link),
        "OTHER" => Some(ObservedKind::Other),
        _ => None,
    }
}

fn serde_relationship_kind(k: coresight_identity::RelationshipKind) -> &'static str {
    match k {
        coresight_identity::RelationshipKind::HardLinkAlias => "HARD_LINK_ALIAS",
        coresight_identity::RelationshipKind::ContentDuplicate => "CONTENT_DUPLICATE",
    }
}

fn serde_accounting(a: StorageAccounting) -> &'static str {
    match a {
        StorageAccounting::Exact => "EXACT",
        StorageAccounting::Estimated => "ESTIMATED",
    }
}

use coresight_identity::StorageAccounting;

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
