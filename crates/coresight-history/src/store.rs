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
    /// A snapshot load hit the caller's bound, so the facts returned were
    /// a PREFIX of what the run stored. This is not corruption — it is an
    /// honest statement that bounded knowledge is not complete knowledge.
    /// Callers must raise `QueryLimits` rather than build a model from a
    /// partial fact set (which would understate the run's observations).
    SnapshotBounded {
        run_id: String,
        sections: Vec<&'static str>,
        limit: usize,
    },
    /// A query's `max_results` is too large to be represented as SQLite's
    /// signed `LIMIT`. SQLite interprets a NEGATIVE `LIMIT` as unbounded,
    /// so accepting such a value would silently disable the caller's
    /// bound; it is rejected instead (Phase 6.4.2, Workstream B).
    LimitOutOfRange {
        requested: usize,
        supported: usize,
    },
    /// A retention policy would violate the API's core guarantee — the
    /// newest committed run is the baseline for future comparisons and is
    /// never removed. Checked BEFORE any row is read or deleted.
    InvalidRetentionPolicy(String),
    /// A caller-supplied value does not fit the store's signed 64-bit
    /// `INTEGER` domain. This is CALLER INPUT, not persisted corruption —
    /// the existing error contract distinguishes the two, and mislabeling
    /// valid caller data as a corrupt store would send callers looking for
    /// database damage that does not exist.
    ///
    /// Checked in the numeric PREFLIGHT (Phase 6.4.4, Workstream A), before
    /// any transaction opens, so a rejected value cannot partially mutate
    /// existing data.
    InvalidRunCount {
        table: &'static str,
        column: &'static str,
        value: u64,
    },
    /// A timestamp lies outside the store's signed 64-bit nanosecond
    /// storage domain. This is CALLER INPUT, not persisted corruption —
    /// the same distinction [`StoreError::InvalidRunCount`] draws: a value
    /// the caller handed us is never evidence of database damage, and
    /// mislabeling it as a corrupt store would send callers hunting for
    /// damage that does not exist.
    ///
    /// `nanos` is the rejected time expressed as nanoseconds after the
    /// Unix epoch (a `u128`, because `Duration::as_nanos()` is wider than
    /// `i64`), so the rejection is diagnosable without re-deriving which
    /// value failed. `run_id` names the run the value belonged to where
    /// one is known.
    ///
    /// Checked in the numeric PREFLIGHT (Phase 6.4.4, Workstream A;
    /// completed for timestamps in Phase 6.4.5), before any transaction
    /// opens, so a rejected value cannot partially mutate existing data.
    /// The one caller-less use is the store's own clock reading, whose
    /// failure would be a platform defect rather than caller data — it is
    /// surfaced as this typed error instead of a panic.
    InvalidTimestamp {
        table: &'static str,
        column: &'static str,
        run_id: Option<String>,
        nanos: u128,
    },
    /// A caller-supplied application-snapshot value does not fit the
    /// store's signed 64-bit `INTEGER` domain. This is CALLER INPUT, not
    /// persisted corruption — the same caller-input-vs-corruption
    /// distinction [`StoreError::InvalidRunCount`] and
    /// [`StoreError::InvalidTimestamp`] draw for the legacy run API: a
    /// value the caller handed us is never evidence of database damage,
    /// and mislabeling it as a corrupt store would send callers hunting
    /// for damage that does not exist.
    ///
    /// `value` is a `u128`, wide enough to report both `u64` domain
    /// values and `usize` ordinals on every supported platform without
    /// narrowing or losing diagnostic information. (On supported
    /// platforms `usize` fits in `u64` today, but recording the value in
    /// the wider domain keeps the error honest if a future platform ever
    /// widens it.) `run_id` names the run the value belonged to where one
    /// is known.
    ///
    /// Checked in the snapshot PREFLIGHT (Phase 6.4.6), before the write
    /// transaction opens or any destructive statement runs: a rejected
    /// replacement cannot delete or partially replace a previously valid
    /// snapshot. [`StoreError::Corrupt`] remains reserved for genuinely
    /// malformed or impossible PERSISTED data.
    InvalidSnapshotInput {
        table: &'static str,
        column: &'static str,
        run_id: Option<String>,
        value: u128,
    },
    /// A pre-migration backup could not be created, verified, or published
    /// (Phase 6.5). The store REFUSES to migrate without one: a forward
    /// migration rewrites the file, and the transaction that makes it atomic
    /// cannot protect the pre-migration bytes from an unrecoverable failure
    /// (a process kill, a power loss, a disk fault). Failing closed here is
    /// deliberate — an un-migrated store is still a readable store, whereas a
    /// half-migrated one may not be.
    ///
    /// This is NOT corruption and NOT bad caller data: the store on disk may
    /// be perfectly healthy, and the failure is in the safety machinery or
    /// the environment (permissions, free space). `step` names which stage
    /// failed so the cause is diagnosable without a log.
    BackupFailed {
        step: String,
        detail: String,
    },
    /// A corrupt store could not be relocated to quarantine (Phase 6.5).
    ///
    /// This is the FAILURE of the quarantine attempt, not of the integrity
    /// check: the store IS corrupt either way. The distinction matters
    /// because `detail` names which artifacts remain where they are — the
    /// caller must be able to tell "moved aside, here is where" from
    /// "still in place, nothing was lost".
    QuarantineFailed {
        step: String,
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
            StoreError::SnapshotBounded {
                run_id,
                sections,
                limit,
            } => write!(
                f,
                "snapshot load for run {run_id} hit the caller's bound of {limit} rows in \
                 section(s) {sections:?}; the facts returned are incomplete, so no model was \
                 built — raise QueryLimits and retry"
            ),
            StoreError::LimitOutOfRange {
                requested,
                supported,
            } => write!(
                f,
                "query limit {requested} exceeds the representable maximum {supported}; \
                 refusing to run a query whose bound could become unlimited"
            ),
            StoreError::InvalidRetentionPolicy(detail) => {
                write!(f, "invalid retention policy: {detail}")
            }
            StoreError::InvalidRunCount {
                table,
                column,
                value,
            } => write!(
                f,
                "{table}.{column}: caller value {value} exceeds the store's signed 64-bit \
                 INTEGER domain; this is invalid input, not a corrupt store"
            ),
            StoreError::InvalidTimestamp {
                table,
                column,
                run_id,
                nanos,
            } => match run_id {
                Some(id) => write!(
                    f,
                    "{table}.{column} (run {id}): caller timestamp {nanos} ns after the Unix \
                     epoch exceeds the store's signed 64-bit nanosecond domain (~year 2262 and \
                     later are unrepresentable); this is invalid input, not a corrupt store"
                ),
                None => write!(
                    f,
                    "{table}.{column}: timestamp {nanos} ns after the Unix epoch exceeds the \
                     store's signed 64-bit nanosecond domain (~year 2262 and later are \
                     unrepresentable); this is an unrepresentable value, not a corrupt store — \
                     refused rather than wrapped or clamped"
                ),
            },
            StoreError::InvalidSnapshotInput {
                table,
                column,
                run_id,
                value,
            } => match run_id {
                Some(id) => write!(
                    f,
                    "{table}.{column} (run {id}): caller value {value} exceeds the store's \
                     signed 64-bit INTEGER domain; this is invalid snapshot input, not a \
                     corrupt store"
                ),
                None => write!(
                    f,
                    "{table}.{column}: value {value} exceeds the store's signed 64-bit \
                     INTEGER domain — an unrepresentable input, refused rather than wrapped \
                     or clamped"
                ),
            },
            StoreError::BackupFailed { step, detail } => write!(
                f,
                "pre-migration backup failed at the {step} stage: {detail} — the store was NOT \
                 migrated, so it remains openable at its current version"
            ),
            StoreError::QuarantineFailed { step, detail } => write!(
                f,
                "corrupt-store quarantine failed at the {step} stage: {detail}"
            ),
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
/// system-model inputs survive a reload deterministically; v6 (Phase
/// 6.4.1) re-keys application ids persisted under the Phase 6.4
/// delimiter-joined encoding, which was ambiguous at component
/// boundaries (see [`MIGRATION_V6`]).
pub const HISTORY_SCHEMA_VERSION: u32 = 6;

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

/// Forward-only migration: v5 → v6 (Phase 6.4.1, Workstream A).
///
/// The Phase 6.4 application identity hashed `"{name}|{publisher}"`,
/// which is ambiguous at the component boundary: `("A|B","C")` and
/// `("A","B|C")` derive the SAME id, so two distinct logical
/// applications could share one `ApplicationId`. Phase 6.4.1 replaces
/// that with a length-prefixed encoding (injective for all inputs), and
/// this migration re-keys ids persisted under the legacy encoding.
///
/// The DDL below only adds the `id_encoding` provenance column; the
/// re-keying itself is done in Rust by [`rekey_legacy_application_ids`],
/// because the correct id depends on each row's own stored facts and no
/// SQL expression can recompute a SHA-256. The rules that function
/// enforces:
///
/// - **Never a global replace.** One legacy id may legitimately cover
///   several distinct `(name, publisher)` pairs (exactly the ambiguity
///   being repaired). Each row is re-keyed from ITS OWN stored
///   `name`/`publisher`, so a legacy id splits back into the distinct
///   identities it was conflating — never merged into one new id.
/// - **Children follow their parent row.** `app_id` is a component of
///   every child key (`provenance`, `views`, `roots`, `evidence`,
///   `footprints`, `footprint_evidence`), so each child row is re-keyed
///   to the id recomputed for the parent row it references. Attribution
///   can therefore never drift to a different application.
/// - **The probe column is authoritative.** `id_encoding` records which
///   encoding a row was written with (`2` = current, `1` = legacy). The
///   loader verifies it, so a v6 store whose rows still carry legacy ids
///   is refused rather than silently trusted.
/// - **Conflicting facts stay distinct.** Rows are re-keyed per
///   `(run_id, app_id, fact_ord)`, so contradictory facts committed under
///   one legacy id keep their own ordinals and remain separate after
///   migration — no conflict is silently merged away.
/// - **Idempotence-safe.** Re-running against a v6 database finds no
///   legacy rows and performs no writes.
const MIGRATION_V6: &str = "
ALTER TABLE app_snapshot_apps
    ADD COLUMN id_encoding INTEGER NOT NULL DEFAULT 2;
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

impl std::fmt::Debug for HistoryStore {
    /// Deliberately opaque: the store holds no user-facing state, and a
    /// `Debug` that rendered the connection would invite logging paths.
    /// Implemented so `Result<HistoryStore, _>::expect_err` works in tests.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HistoryStore(..)")
    }
}

/// What a read-only inspection concluded about an EXISTING store file.
///
/// Deliberately owns no `Connection`: the inspection must be able to CLOSE
/// the database before the caller acts on the verdict, because a quarantine
/// rename fails on Windows while SQLite still holds the file open.
enum Inspection {
    /// Already at the supported version: nothing to migrate, nothing to back
    /// up.
    Current,
    /// Older than supported, and healthy: a migration is required, so a
    /// verified backup must exist first. Carries the recorded version so the
    /// backup can be checked against it.
    NeedsMigration(u32),
}

impl HistoryStore {
    /// Open (or create) the store at `path`, applying pending migrations
    /// (core v1 → history v2 → …, each atomic with its version bump) and
    /// recovering stale `RUNNING` runs as `FAILED` (Objective 19 —
    /// deterministic crash recovery).
    ///
    /// `PRAGMA integrity_check` runs FIRST: a corrupt store is refused
    /// ([`StoreError::Corrupt`]) before anything reads or migrates it.
    /// A store whose corruption is established is then QUARANTINED — its
    /// file and sidecars are moved aside, never deleted and never replaced
    /// with a fresh empty store (Phase 6.5).
    ///
    /// A store written by a NEWER build is refused
    /// ([`StoreError::SchemaTooNew`]) rather than partially interpreted:
    /// forward-only migrations never run backwards. It is left COMPLETELY
    /// UNTOUCHED — not migrated, not backed up, not quarantined — because
    /// a newer store is not this build's to modify.
    ///
    /// When an EXISTING healthy store genuinely needs a forward migration,
    /// a verified pre-migration backup is completed and published BEFORE
    /// the first migration statement runs; a backup that cannot be created,
    /// verified, or published aborts the open rather than migrating
    /// unprotected (Phase 6.5).
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        // Phase 6.5: establish the store's original state BEFORE any helper
        // can mutate it. `coresight_core::db::open` sets `journal_mode=WAL`
        // and applies the v1 bootstrap, both of which WRITE to the file —
        // so the corruption gate, the too-new-schema refusal, and the
        // pre-migration backup all have to run first.
        Self::preflight_existing_store(path)?;

        let mut conn = coresight_core::db::open(path)?;
        // docs/DATABASE.md: integrity is checked on open BEFORE anything
        // reads or migrates the file. A corrupt store is refused as a
        // typed error — never partially interpreted, never "recovered" by
        // guessing. This second check runs on the ALREADY-OPEN connection:
        // the preflight above proved the file was intact when we found it,
        // and this catches anything the bootstrap helper's own writes broke.
        let problems = crate::store_safety::integrity_problems(&conn)?;
        if !problems.is_empty() {
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
        Self::apply_migrations(&mut conn, current)?;
        let store = HistoryStore { conn };
        store.recover_stale_runs()?;
        Ok(store)
    }

    /// Open with an injected [`Hazard`]. **Test seam** — the hazard is
    /// threaded into the preflight so the fail-closed paths are exercised
    /// by test rather than described in a comment.
    ///
    /// This is NOT a production entry point: [`Hazard::None`] takes exactly
    /// the same path as [`HistoryStore::open`]. It exists because an
    /// untested failure path is a claim, not a guarantee, and the failures
    /// it injects (a disk that cannot be written, a rename that fails)
    /// cannot be provoked reliably on demand.
    pub fn open_with_hazard(
        path: &Path,
        hazard: crate::store_safety::Hazard,
    ) -> Result<Self, StoreError> {
        Self::preflight_existing_store_with(path, hazard)?;
        let mut conn = coresight_core::db::open(path)?;
        let problems = crate::store_safety::integrity_problems(&conn)?;
        if !problems.is_empty() {
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
        Self::apply_migrations(&mut conn, current)?;
        let store = HistoryStore { conn };
        store.recover_stale_runs()?;
        Ok(store)
    }

    /// Establish the store's original state before anything mutates it.
    ///
    /// Ordering matters and is the reason this is separate from `open`:
    /// [`coresight_core::db::open`] runs `PRAGMA journal_mode=WAL` and the
    /// v1 bootstrap, both of which WRITE. Checking corruption or backing up
    /// AFTER that would be checking a file this build had already touched.
    ///
    /// The sequence is:
    ///
    /// 1. **No existing file** → nothing to protect. A brand-new store is
    ///    created normally; a needless backup of an empty file is not.
    /// 2. **Existing file** → open it READ-ONLY first, so even this
    ///    inspection cannot modify it.
    /// 3. **Integrity check fails** → corruption is established. Close the
    ///    connection (SQLite holds file handles; Windows cannot rename an
    ///    open file) and quarantine. Return the corruption error naming
    ///    where the store now lives.
    /// 4. **Schema newer than supported** → refuse as `SchemaTooNew` and
    ///    leave the file completely untouched.
    /// 5. **Schema older than supported** → take a verified backup, which
    ///    must succeed before `open` proceeds to migrate.
    fn preflight_existing_store(path: &Path) -> Result<(), StoreError> {
        Self::preflight_existing_store_with(path, crate::store_safety::Hazard::None)
    }

    fn preflight_existing_store_with(
        path: &Path,
        hazard: crate::store_safety::Hazard,
    ) -> Result<(), StoreError> {
        if !path.is_file() {
            // A brand-new store has no pre-migration state to protect and no
            // corruption to establish: `open` simply creates it.
            return Ok(());
        }
        match Self::inspect_existing_store(path, hazard)? {
            Inspection::Current => Ok(()),
            Inspection::NeedsMigration(found) => {
                // A migration WILL run. Take the verified backup first; a
                // backup that cannot be created, verified, or published
                // aborts the open — the store stays exactly as it is.
                crate::store_safety::verified_pre_migration_backup(
                    &Self::open_for_backup(path)?,
                    path,
                    Some(found),
                    hazard,
                )?;
                Ok(())
            }
        }
    }

    /// Open `path` READ-ONLY, decide its state, and CLOSE the connection
    /// before returning.
    ///
    /// The early close is load-bearing, not tidiness. SQLite holds the file
    /// open for the lifetime of the connection, and on Windows an open file
    /// cannot be renamed. A quarantine that ran while this handle was still
    /// alive would fail on Windows and silently succeed on Unix — a
    /// platform-dependent safety guarantee, which is exactly what this
    /// phase must not ship. Returning owned data (not a `Connection`) makes
    /// the close happen before any caller can act on the verdict.
    fn inspect_existing_store(
        path: &Path,
        hazard: crate::store_safety::Hazard,
    ) -> Result<Inspection, StoreError> {
        let conn = match rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => conn,
            Err(e) => {
                // Only SQLite's own "this file is not a usable database"
                // codes establish corruption. A permission denial, a lock,
                // or an I/O failure is an ENVIRONMENT problem: quarantining
                // a healthy file because the disk was busy would be worse
                // than doing nothing, so the store is left untouched and
                // the error is returned as-is.
                return Err(Self::classify_inspection_error(
                    path,
                    StoreError::Sqlite(e),
                    hazard,
                ));
            }
        };

        // Read everything the verdict needs FIRST. No quarantine or file
        // operation may run while `conn` is alive — SQLite holds the file
        // open and Windows cannot rename it. Collecting the raw results
        // (rather than classifying inside `map_err`) is what guarantees the
        // connection is closed below before anything touches the file.
        let problems = crate::store_safety::integrity_problems(&conn);
        let version = if problems.is_ok() {
            crate::store_safety::recorded_version(&conn)
        } else {
            Ok(None)
        };

        // The connection closes HERE, before any handling below.
        drop(conn);

        let problems = problems.map_err(|e| Self::classify_inspection_error(path, e, hazard))?;
        let version = version.map_err(|e| Self::classify_inspection_error(path, e, hazard))?;

        // Any row other than `ok` is positive evidence of corruption.
        if !problems.is_empty() {
            return Err(Self::quarantine_after_corruption(
                path,
                &problems.join("; "),
                hazard,
            ));
        }

        match version {
            // A store with no recorded version at all is not a store this
            // build can interpret; treat it as brand-new only when the
            // file is absent, which `preflight` already handled.
            None => Ok(Inspection::Current),
            Some(found) if found > HISTORY_SCHEMA_VERSION => Err(StoreError::SchemaTooNew {
                found,
                supported: HISTORY_SCHEMA_VERSION,
            }),
            Some(found) if found < HISTORY_SCHEMA_VERSION => Ok(Inspection::NeedsMigration(found)),
            Some(_) => Ok(Inspection::Current),
        }
    }

    /// Decide whether an inspection error is proven corruption (quarantine)
    /// or merely an inability to decide (preserve and report).
    fn classify_inspection_error(
        path: &Path,
        e: StoreError,
        hazard: crate::store_safety::Hazard,
    ) -> StoreError {
        let corruption = match &e {
            StoreError::Sqlite(sql) => crate::store_safety::sqlite_error_indicates_corruption(sql),
            _ => false,
        };
        if corruption {
            Self::quarantine_after_corruption(path, &e.to_string(), hazard)
        } else {
            e
        }
    }

    /// A short-lived WRITE connection used only to drive the online backup.
    ///
    /// The Online Backup API needs a normal connection as its source. This
    /// connection performs no migration and is closed immediately after the
    /// backup, so it cannot leave a store half-migrated.
    fn open_for_backup(path: &Path) -> Result<rusqlite::Connection, StoreError> {
        rusqlite::Connection::open(path).map_err(StoreError::Sqlite)
    }

    /// Quarantine a store whose corruption is established, then return an
    /// error describing the corruption AND where the store now lives.
    ///
    /// No replacement database is created at the original path and no
    /// repair is attempted: a caller that wants the old data back can find
    /// it, and nothing claims the history was recovered.
    fn quarantine_after_corruption(
        path: &Path,
        detail: &str,
        hazard: crate::store_safety::Hazard,
    ) -> StoreError {
        // The caller is REQUIRED to have closed every connection to `path`
        // before calling this: SQLite holds the file open, and on Windows an
        // open database file cannot be renamed. `inspect_existing_store`
        // guarantees that by returning owned data rather than a connection.
        let quarantine = crate::store_safety::quarantine_corrupt_store(path, hazard);
        let location = match &quarantine {
            Ok(q) => format!(
                " the damaged store was preserved (not deleted, not repaired) under {}",
                q.directory().display()
            ),
            Err(e) => format!(
                " the damaged store could NOT be moved aside ({e}); its files are unchanged \
                 where they were"
            ),
        };
        StoreError::Corrupt {
            table: "store",
            column: "integrity_check",
            run_id: None,
            detail: format!(
                "database integrity check failed on open: {detail}.{location} No replacement \
                 store was created and no repair was attempted — upgrade CoreSight or inspect \
                 the quarantined files yourself before trusting this store again"
            ),
        }
    }

    /// Apply every forward migration this build needs, each atomic with its
    /// version bump. Shared by `open` and the test seam so both take
    /// exactly the same path.
    fn apply_migrations(conn: &mut rusqlite::Connection, current: u32) -> Result<(), StoreError> {
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
        if current < 6 {
            let tx = conn.transaction()?;
            // Re-keying necessarily swaps a parent row's key while its
            // children still reference the old one (and vice versa).
            // Immediate FK enforcement would reject those intermediate
            // states, so the checks are DEFERRED to commit: either the
            // whole re-key lands consistently or none of it does.
            tx.execute_batch("PRAGMA defer_foreign_keys=ON;")?;
            tx.execute_batch(MIGRATION_V6)?;
            rekey_legacy_application_ids(&tx)?;
            tx.execute("UPDATE schema_version SET version = 6", params![])?;
            tx.commit()?;
        }
        Ok(())
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
        let now = now_nanos()?;
        self.conn.execute(
            "UPDATE scan_runs SET status = 'FAILED', completed_at = COALESCE(completed_at, ?1)
             WHERE status = 'RUNNING'",
            params![now],
        )?;
        Ok(ids)
    }

    /// Persist a run header as `RUNNING` (its own transaction — visible
    /// for crash recovery from this moment).
    pub fn begin_run(&mut self, record: &RunRecord) -> Result<(), StoreError> {
        // Checked timestamp BEFORE the transaction opens (Phase 6.4.4,
        // Workstream A): a caller-supplied time outside the representable
        // domain rejects the run without even beginning an INSERT.
        let started_at =
            checked_run_timestamp(record.started_at, "started_at", Some(&record.run_id.0))?;
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
                started_at,
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
        // ---- TRUE preflight (Phase 6.4.3 Workstream D; closed in 6.4.4) ----
        // Every caller-controlled conversion that can fail is computed
        // BEFORE the transaction opens, so a rejected input cannot even
        // BEGIN modifying existing state. Relying on transaction rollback
        // would satisfy the "no partial commit" guarantee but not the "no
        // mutation attempt" one, and it would make the failure order depend
        // on SQLite's error reporting rather than the caller's input.
        //
        // Relationship truncation counter: an incompleteness signal, not a
        // size — an unrepresentable value must fail loudly rather than wrap
        // into a negative count.
        let rel_truncated = match relationships {
            Some(r) => Some(checked_relationship_counter(r.relationships_truncated)?),
            None => None,
        };
        // Recoverable-byte upper bounds, and the relationship's own size and
        // member count (both unsigned, both previously unchecked).
        let relationship_rows: Vec<(i64, i64, Option<i64>)> = relationships
            .map(|rep| {
                rep.relationships
                    .iter()
                    .map(|r| {
                        let size = checked_u64_counter(r.size, "relationship_obs", "size")?;
                        let member_count = checked_u64_counter(
                            r.member_count,
                            "relationship_obs",
                            "member_count",
                        )?;
                        let recoverable = checked_recoverable_bytes(r)?;
                        Ok((size, member_count, recoverable))
                    })
                    .collect::<Result<Vec<_>, StoreError>>()
            })
            .transpose()?
            .unwrap_or_default();
        // Terminal timestamp.
        let completed_at = checked_run_timestamp(
            record.completed_at.unwrap_or(record.started_at),
            "completed_at",
            Some(&record.run_id.0),
        )?;
        // Every observation's modification time (owned by `observations`).
        let entry_timestamps: Vec<Option<i64>> = snapshot
            .entries
            .iter()
            .map(|e| match e.modified {
                Some(t) => checked_observation_timestamp(t, Some(&record.run_id.0)).map(Some),
                None => Ok(None),
            })
            .collect::<Result<_, _>>()?;
        // Every observation's size.
        let entry_sizes: Vec<Option<i64>> = snapshot
            .entries
            .iter()
            .map(|e| checked_optional_size(e.size, "observations", "size"))
            .collect::<Result<_, _>>()?;
        // Every per-run counter: all nine `RunCounts` fields.
        let count_entries = checked_u64_counter(
            record.counts.entries_examined,
            "scan_runs",
            "entries_examined",
        )?;
        let count_files = checked_u64_counter(record.counts.files, "scan_runs", "files")?;
        let count_dirs = checked_u64_counter(record.counts.dirs, "scan_runs", "dirs")?;
        let count_links = checked_u64_counter(record.counts.links, "scan_runs", "links")?;
        let count_other =
            checked_u64_counter(record.counts.other_entries, "scan_runs", "other_entries")?;
        let count_bytes = checked_u64_counter(record.counts.bytes, "scan_runs", "bytes")?;
        let count_errors = checked_u64_counter(
            record.counts.observation_errors,
            "scan_runs",
            "observation_errors",
        )?;
        let count_untracked = checked_u64_counter(
            record.counts.candidates_untracked,
            "scan_runs",
            "candidates_untracked",
        )?;
        let count_hash_failures =
            checked_u64_counter(record.counts.hash_failures, "scan_runs", "hash_failures")?;

        // Preflight passed: only now open the transaction.
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
                completed_at,
                serde_status(record.status),
                count_entries,
                count_files,
                count_dirs,
                count_links,
                count_other,
                count_bytes,
                count_errors,
                count_untracked,
                count_hash_failures,
                relationships.map(|r| serde_duplicate_status(r.status)),
                rel_truncated,
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
        for ((entry, modified), size) in snapshot
            .entries
            .iter()
            .zip(entry_timestamps.iter())
            .zip(entry_sizes.iter())
        {
            // The modification time and size were already checked in the
            // preflight; they are bound here verbatim so no conversion can
            // fail between the UPDATE above and this INSERT.
            obs.execute(params![
                record.run_id.0,
                crate::path_encoding::encode(&entry.path),
                serde_kind(entry.kind),
                size,
                entry.object.map(|o| o.device as i64),
                entry.object.map(|o| o.inode as i64),
                entry.object.and_then(|o| o.file_id_hi.map(|hi| hi as i64)),
                modified,
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
            for (r, (size, member_count, recoverable)) in
                rel.relationships.iter().zip(relationship_rows.iter())
            {
                // Size, member count, and recoverable bytes were all checked
                // in the preflight and are bound here verbatim, so no
                // conversion can fail after the run row has been updated.
                ro.execute(params![
                    record.run_id.0,
                    r.id,
                    serde_relationship_kind(r.kind),
                    size,
                    member_count,
                    recoverable,
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
        let completed_at = now_nanos()?;
        let updated = self.conn.execute(
            "UPDATE scan_runs SET status = ?2, completed_at = ?3
             WHERE run_id = ?1 AND status = 'RUNNING'",
            params![run_id.0, serde_status(status), completed_at],
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
        let rows = stmt.query_map(params![limits.to_sql_limit()?], map_run_row)?;
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
            legacy_counter_from_stored(rel_status.truncated),
        )
        .map_err(|detail| StoreError::Corrupt {
            table: "relationship_obs",
            column: "kind/accounting/path",
            run_id: Some(run_id.0.clone()),
            detail,
        })?;
        // A run with NO persisted relationship status either never ran
        // the relationship layer, or was written before v4 (when the
        // metadata columns did not exist — see
        // [`HistoryStore::rel_status_for_run`]). Absence is
        // not `Completed`: the report is only attached when the store
        // actually recorded one, so comparisons cannot fabricate
        // relationship completeness. The relationship ROWS themselves are
        // unaffected — they predate v4 and are migrated and retained
        // verbatim; they stay queryable through `relationship_history`.
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
    ///
    /// ## The nullability contract (Phase 6.4.4, Workstream C)
    ///
    /// Every historical writer sets BOTH columns from the same
    /// `Option<&RelationshipReport>`:
    /// `rel_status = relationships.map(|r| r.status)` and
    /// `rel_truncated = relationships.map(|r| r.relationships_truncated)`.
    /// They are therefore written as a PAIR — both present or both absent —
    /// and the v4 migration added both as nullable columns with no default,
    /// so pre-v4 rows carry both as NULL.
    ///
    /// | `rel_status` | `rel_truncated` | Meaning | Accepted |
    /// |---|---|---|---|
    /// | NULL | NULL | no report was recorded | **yes** — the absent case |
    /// | present | present | a report was recorded, with its count | **yes** |
    /// | present | NULL | no writer can produce this | **no** — inconsistent metadata |
    /// | NULL | present | no writer can produce this | **no** — inconsistent metadata |
    ///
    /// The two mixed combinations cannot arise from any writer version, so
    /// they are rejected as inconsistent metadata with the offending column
    /// and run named. Rejecting them is safe: it cannot reject a legitimate
    /// historical state.
    ///
    /// A *present* count is NOT inferred to be zero when absent, because a
    /// present status tells us a report existed — inventing a zero would
    /// fabricate a completeness the run never recorded. What is preserved is
    /// the distinction between "no report was recorded" and "a report was
    /// recorded with zero truncations", which the pair of NULLs vs a present
    /// `0` encodes exactly.
    ///
    /// ## What (NULL, NULL) does and does NOT mean (Phase 6.4.5, Workstream C)
    ///
    /// `(NULL, NULL)` means only that no relationship-report status or
    /// truncation METADATA was recorded for this run — either the run
    /// predates the v4 metadata columns, or it never ran the relationship
    /// layer. It does NOT prove that no relationship rows ever existed: the
    /// `relationship_obs` / `relationship_members` tables predate v4, so a
    /// pre-v4 database can carry relationship rows while both metadata
    /// columns are NULL. Those rows are migrated and retained verbatim, and
    /// the reader neither discards them nor manufactures a completeness
    /// claim for them: `load_run_snapshot` attaches no report when no
    /// status was recorded, and the retained facts stay queryable through
    /// `relationship_history`.
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
        // Enforce the pair contract above.
        match (&status, &truncated) {
            (Some(_), None) => {
                return Err(StoreError::Corrupt {
                    table: "scan_runs",
                    column: "rel_truncated",
                    run_id: Some(run_id.0.clone()),
                    detail: "relationship report status is recorded but the truncation \
                             count is missing; no writer produces this combination"
                        .to_string(),
                });
            }
            (None, Some(_)) => {
                return Err(StoreError::Corrupt {
                    table: "scan_runs",
                    column: "rel_status",
                    run_id: Some(run_id.0.clone()),
                    detail: "relationship truncation count is recorded but the report \
                             status is missing; no writer produces this combination"
                        .to_string(),
                });
            }
            (None, None) | (Some(_), Some(_)) => {}
        }
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
            params![crate::path_encoding::encode(path), limits.to_sql_limit()?],
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
                limits.to_sql_limit()?
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
            params![sha256_hex, limits.to_sql_limit()?],
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
        let rows = stmt.query_map(params![rel_id, limits.to_sql_limit()?], |r| {
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
    ///
    /// `keep_latest` must be at least 1: a value of 0 would permit
    /// deleting the newest baseline, which contradicts the guarantee this
    /// API exists to provide. It is rejected with
    /// [`StoreError::InvalidRetentionPolicy`] BEFORE any row is read or
    /// deleted (Phase 6.4.3, Workstream D).
    pub fn apply_retention(
        &mut self,
        policy: &RetentionPolicy,
    ) -> Result<RetentionReport, StoreError> {
        if policy.keep_latest == 0 {
            return Err(StoreError::InvalidRetentionPolicy(
                "keep_latest must be at least 1: the newest committed run is \
                 the baseline for future comparisons and is never removed"
                    .to_string(),
            ));
        }
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

        let now = now_nanos()?;
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
                    Some(age) => retention_age_exceeded(*started, age, now),
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

impl QueryLimits {
    /// The largest supported `max_results`.
    ///
    /// Two constraints shape this value:
    ///
    /// 1. SQLite reads a NEGATIVE `LIMIT` as "unbounded", so any limit that
    ///    does not fit a signed 64-bit integer would silently disable the
    ///    caller's bound (see [`StoreError::LimitOutOfRange`]).
    /// 2. Snapshot reads detect a cap hit EXACTLY by probing with
    ///    `limit + 1` rows ([`QueryLimits::to_probe_sql_limit`]). At
    ///    `i64::MAX` that probe would be unrepresentable and would have to
    ///    saturate, degrading detection precisely at the boundary — so the
    ///    accepted maximum is one below it, and a request for `i64::MAX` is
    ///    refused with a typed error rather than silently downgraded.
    ///
    /// The policy is uniform across every query API in this crate; the
    /// bound is checked BEFORE any SQL is issued, including on the
    /// no-record paths (a query for an absent run still rejects an
    /// unsupported limit).
    pub const MAX_SUPPORTED: usize = (i64::MAX - 1) as usize;

    /// Convert to the store's signed `INTEGER` `LIMIT` domain, rejecting
    /// any value outside the supported domain.
    ///
    /// The bound is [`MAX_SUPPORTED`], which is deliberately BELOW `i64::MAX`
    /// because snapshot reads must also be able to compute the probe value
    /// (`limit + 1`) exactly. Accepting `i64::MAX` here would let
    /// `to_probe_sql_limit` overflow, so the shared validation rejects it up
    /// front instead of panicking at the boundary.
    ///
    /// This is the ONE shared conversion policy for every query API in
    /// this crate — snapshot reads, history reads, and listing reads — so
    /// no call site can introduce its own narrowing rule.
    pub fn to_sql_limit(&self) -> Result<i64, StoreError> {
        if self.max_results > Self::MAX_SUPPORTED {
            return Err(StoreError::LimitOutOfRange {
                requested: self.max_results,
                supported: Self::MAX_SUPPORTED,
            });
        }
        // Guaranteed in range by the check above; `i64::try_from` cannot
        // fail, but the total conversion is kept for the typed error.
        i64::try_from(self.max_results).map_err(|_| StoreError::LimitOutOfRange {
            requested: self.max_results,
            supported: Self::MAX_SUPPORTED,
        })
    }

    /// The `LIMIT` value that makes a cap hit provable: one row beyond the
    /// caller's bound, so `len > max_results` proves the section was cut
    /// short (exact detection, never guessed).
    ///
    /// [`MAX_SUPPORTED`] guarantees the sum always fits a signed 64-bit
    /// integer, so this never saturates and can never become a
    /// negative/unlimited `LIMIT` for any accepted limit.
    pub fn to_probe_sql_limit(&self) -> Result<i64, StoreError> {
        // `MAX_SUPPORTED + 1 == i64::MAX`, so this cannot overflow.
        self.to_sql_limit().map(|limit| limit + 1)
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

/// Re-key every application row still carrying a legacy (Phase 6.4)
/// `ApplicationId`, together with all of its dependent child rows.
///
/// Called inside the v6 migration transaction, so the whole re-key is
/// atomic: any failure rolls the store back to v5 rather than leaving a
/// partially re-keyed database.
///
/// ## How a row is recognised as legacy
///
/// A row is legacy when its stored `app_id` equals the id the OLD
/// encoding derives from its own stored `(name, publisher)` **and** its
/// stored id does NOT equal the id the CURRENT encoding derives from the
/// same pair. Recomputing both from the row's own facts is what makes
/// the migration independent of any external registry: the row itself
/// states which identity it was written under.
///
/// ## Why per-row re-keying is safe
///
/// `app_id` alone is not unique — `(run_id, app_id, fact_ord)` is, which
/// is precisely the granularity at which facts (including conflicting
/// duplicates) are stored. Recomputing the id per row therefore:
///
/// - splits one legacy id that conflated several `(name, publisher)`
///   pairs into the distinct identities the fixed encoding derives;
/// - leaves a row whose legacy and new ids coincide untouched;
/// - never merges two facts that the legacy encoding separated.
///
/// ## How children are re-keyed
///
/// Each child row is re-keyed by *joining on the parent's
/// `(run_id, app_id, fact_ord)`*: the child's new id is its parent's new
/// id, recomputed in the same pass. Because the parent's old id and
/// `fact_ord` identify it uniquely, a child can never be adopted by a
/// different application, and children of two rows that previously
/// shared an id follow their own parent.
fn rekey_legacy_application_ids(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    // Snapshot of the parent rows first: the child UPDATEs below read
    // the same table, so it must not be mutated while iterated.
    struct ParentRow {
        run_id: String,
        app_id: String,
        fact_ord: i64,
        name: String,
        publisher: Option<String>,
    }
    let mut stmt = tx.prepare(
        "SELECT run_id, app_id, fact_ord, name, publisher
         FROM app_snapshot_apps ORDER BY run_id, app_id, fact_ord",
    )?;
    let parents: Vec<ParentRow> = stmt
        .query_map([], |r| {
            Ok(ParentRow {
                run_id: r.get(0)?,
                app_id: r.get(1)?,
                fact_ord: r.get(2)?,
                name: r.get(3)?,
                publisher: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    // Recompute the legacy id exactly as Phase 6.4 did, so a row written
    // by that build is recognised without consulting anything outside
    // its own facts.
    let legacy_id = |name: &str, publisher: Option<&str>| -> String {
        use sha2::Digest;
        let key = coresight_apps::ApplicationId::legacy_derivation_key(name, publisher);
        let digest = sha2::Sha256::digest(key.as_bytes());
        format!(
            "app-{}",
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    };

    let mut rekeyed = 0u64;
    for row in &parents {
        let current = coresight_apps::ApplicationId::derive(&row.name, row.publisher.as_deref());
        let legacy = legacy_id(&row.name, row.publisher.as_deref());
        // Already current (or an id this build cannot explain): leave it
        // alone and record which encoding it claims to be.
        if row.app_id == current.0 {
            continue;
        }
        if row.app_id != legacy {
            // A stored id that matches NEITHER encoding is corruption;
            // guessing would silently reinterpret a historical fact.
            return Err(StoreError::Corrupt {
                table: "app_snapshot_apps",
                column: "app_id",
                run_id: Some(row.run_id.clone()),
                detail: "stored application id matches neither the legacy nor the current \
                     identity encoding for its own (name, publisher); refusing to re-key"
                    .to_string(),
            });
        }

        // Children first (they reference this parent row's old id), then
        // the parent itself — all inside the migration transaction. Each
        // child table keys on the parent's (run_id, app_id, fact_ord), so
        // a key match can only select THIS parent's children.
        for table in [
            "app_snapshot_provenance",
            "app_snapshot_views",
            "app_snapshot_roots",
            "app_snapshot_evidence",
            "app_snapshot_footprints",
            "app_snapshot_footprint_evidence",
        ] {
            tx.execute(
                &format!(
                    "UPDATE {table} SET app_id = ?4
                     WHERE run_id = ?1 AND app_id = ?2 AND fact_ord = ?3"
                ),
                params![row.run_id, row.app_id, row.fact_ord, current.0],
            )?;
        }
        tx.execute(
            "UPDATE app_snapshot_apps SET app_id = ?4, id_encoding = 2
             WHERE run_id = ?1 AND app_id = ?2 AND fact_ord = ?3",
            params![row.run_id, row.app_id, row.fact_ord, current.0],
        )?;
        rekeyed += 1;
    }

    // Every surviving row must now claim the CURRENT encoding; anything
    // else would mean the pass missed a row (or a child drifted).
    let still_legacy: i64 = tx.query_row(
        "SELECT COUNT(*) FROM app_snapshot_apps WHERE id_encoding <> 2",
        [],
        |r| r.get(0),
    )?;
    if still_legacy != 0 {
        return Err(StoreError::Corrupt {
            table: "app_snapshot_apps",
            column: "id_encoding",
            run_id: None,
            detail: format!("{still_legacy} application row(s) left un-re-keyed"),
        });
    }
    let _ = rekeyed;
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

/// Convert a caller-supplied timestamp to the store's signed nanosecond
/// domain, naming the ACTUAL table and column so the error identifies the
/// field's owner rather than always blaming `scan_runs`.
///
/// `Duration::as_nanos()` returns a `u128` wider than `i64`, so an
/// unchecked cast wraps any timestamp past ~2262 CE into a NEGATIVE
/// number — which the reader would then have to interpret (see
/// [`time_from_nanos`]). The checked conversion REJECTS such a time
/// instead, so no new write can ever store a wrapped value.
///
/// ## Caller input, not persisted corruption (Phase 6.4.5)
///
/// An unrepresentable time is reported as [`StoreError::InvalidTimestamp`]
/// — the caller-input half of the error contract Phase 6.4.4 established
/// for counts and sizes. `run_id` carries the run the value belonged to
/// wherever one is known.
///
/// A time BEFORE the epoch is stored as 0 rather than a negative, which
/// is the documented, lossless-enough convention for pre-epoch clocks —
/// preserved from every writer generation (v2 onward).
fn checked_timestamp(
    t: SystemTime,
    table: &'static str,
    column: &'static str,
    run_id: Option<&str>,
) -> Result<i64, StoreError> {
    let duration = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    i64::try_from(duration.as_nanos()).map_err(|_| StoreError::InvalidTimestamp {
        table,
        column,
        run_id: run_id.map(|r| r.to_string()),
        nanos: duration.as_nanos(),
    })
}

/// The run-level timestamps (`scan_runs`).
fn checked_run_timestamp(
    t: SystemTime,
    column: &'static str,
    run_id: Option<&str>,
) -> Result<i64, StoreError> {
    checked_timestamp(t, "scan_runs", column, run_id)
}

/// An observation's modification timestamp (`observations.modified`).
fn checked_observation_timestamp(t: SystemTime, run_id: Option<&str>) -> Result<i64, StoreError> {
    checked_timestamp(t, "observations", "modified", run_id)
}

/// Convert a public `SystemTime` to the store's signed nanosecond domain.
///
/// `SystemTime::now()` is always within the representable nanosecond
/// domain on any real platform (year 2262 is far in the future), so this
/// cannot fail in practice; a failure would be a platform defect rather
/// than caller data. Both callers write it to `scan_runs.completed_at`,
/// and a failure is surfaced as the typed
/// [`StoreError::InvalidTimestamp`] rather than a panic, so even a
/// defective clock can never crash a caller mid-operation.
fn now_nanos() -> Result<i64, StoreError> {
    checked_run_timestamp(SystemTime::now(), "completed_at", None)
}

/// Decode a persisted nanosecond timestamp (Phase 6.4.5, Workstream B).
///
/// ## The compatibility policy
///
/// Every writer generation this store has shipped — the Phase 5 v2
/// writer through the pre-6.4.2 `time_nanos` — encoded a time as
/// "nanoseconds since the Unix epoch, narrowed to `i64`", and every one
/// of them mapped a PRE-EPOCH time to `0` (the documented pre-epoch
/// convention). A NON-NEGATIVE value (`0` … `i64::MAX`) is therefore the
/// exact time for every writer and round-trips unchanged; `0` is the
/// honest pre-epoch convention, not a corrupted value.
///
/// A NEGATIVE value is a legitimate legacy bit pattern, not corruption:
/// the pre-6.4.2 writer narrowed `Duration::as_nanos()` (a `u128`) with
/// an unchecked `as i64`, so any caller time from ~year 2262 up to
/// ~year 2554 wrapped into the negative half of the signed domain. The
/// faithful reading is the exact inverse of that legacy write — the
/// stored bits reinterpreted as an unsigned nanosecond count — which
/// reconstructs the original time for every value the old writer could
/// produce below 2^64 nanoseconds.
///
/// ## A documented limitation, not a claimed capability
///
/// A negative stored value is AMBIGUOUS by construction: "a legacy
/// wrapped far-future time" and "a tampered value that never came from
/// any writer" are indistinguishable in the stored format. This code
/// deliberately does not pretend to tell them apart; it applies the one
/// reading that preserves every historical value the old writer could
/// produce. The reconstruction is exact for originals below 2^64
/// nanoseconds (~year 2554) and lossy beyond — the truncation discards
/// the bits above 64, so an original at or past ~year 2554 reads back as
/// epoch + (low 64 bits), roughly 584 years early. The previous
/// clamp-to-epoch, by contrast, silently presented EVERY such value as
/// the plausible date 1970-01-01.
///
/// The reconstruction honours the platform's own `SystemTime` granularity:
/// a Windows `SystemTime` is 100-nanosecond FILETIME-based, so a
/// sub-100ns remainder truncates on every conversion — the same
/// granularity every timestamp read on that platform has always had,
/// positive values included. New writes can never create a negative
/// value: [`checked_timestamp`] rejects an unrepresentable time before
/// any mutation.
fn time_from_nanos(n: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(n as u64)
}

fn time_from_nanos_opt(n: Option<i64>) -> Option<SystemTime> {
    n.map(time_from_nanos)
}

/// Reinterpret a stored signed `INTEGER` counter as its unsigned domain
/// value.
///
/// ## Why this exists (Phase 6.4.3, Workstream A)
///
/// `scan_runs.rel_truncated` was originally written with an UNCHECKED
/// `u64 as i64` conversion, so a domain value at or above `2^63` was
/// stored as a NEGATIVE SQLite integer — a valid legacy bit pattern, not
/// corruption. The read path must reconstruct such values faithfully.
///
/// | Stored value | Meaning | Result |
/// |---|---|---|
/// | `NULL` | "not recorded" | `0` (no report was recorded) |
/// | `>= 0` | the value itself | `v as u64` |
/// | `< 0` | a legacy wrapped bit pattern | `v as u64` (exact round trip) |
///
/// ## A documented limitation, not a claimed capability
///
/// A negative stored value is AMBIGUOUS by construction: "a legacy wrapped
/// counter" and "a tampered value that never came from this writer" are
/// indistinguishable in the stored format. This code deliberately does not
/// pretend to tell them apart. It reconstructs the round-tripped value —
/// the only reading that preserves every historical value the old writer
/// could produce — and the limitation is documented rather than dressed up
/// as corruption detection.
///
/// What this must NEVER do is clamp to zero, which is what the previous
/// implementation did (`.max(0) as u64`): a legacy `rel_truncated` above
/// `2^63` silently read back as "nothing truncated", erasing the very
/// incompleteness signal the field carries.
///
/// New writes are unaffected — Phase 6.4.2's checked conversion rejects an
/// unrepresentable counter before any mutation, so no NEW row can store a
/// wrapped value.
fn legacy_counter_from_stored(stored: Option<i64>) -> u64 {
    // `as u64` is the exact inverse of the legacy `as i64` write.
    stored.unwrap_or(0) as u64
}

/// Checked conversion of the relationship truncation counter (Phase
/// 6.4.2, Workstream C). The counter records how many relationships the
/// engine could not publish under its caps, so it is an incompleteness
/// signal, not a size — an unrepresentable value must fail loudly rather
/// than wrap into a negative count.
fn checked_relationship_counter(value: u64) -> Result<i64, StoreError> {
    checked_u64_counter(value, "scan_runs", "rel_truncated")
}

/// Checked conversion of any caller-supplied run counter (Phase 6.4.4,
/// Workstream A). Every `RunCounts` field is a `u64` whose value comes
/// from the caller's reports, so it must be checked before the write
/// transaction rather than left to SQLite's "number too large" error
/// after mutation has begun.
fn checked_u64_counter(
    value: u64,
    table: &'static str,
    column: &'static str,
) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::InvalidRunCount {
        table,
        column,
        value,
    })
}

/// Checked conversion of an optional size (`ObservedEntry::size`,
/// recoverable-byte upper bounds). `None` is a legitimate absent value
/// and passes through unchanged.
fn checked_optional_size(
    value: Option<u64>,
    table: &'static str,
    column: &'static str,
) -> Result<Option<i64>, StoreError> {
    match value {
        None => Ok(None),
        Some(v) => i64::try_from(v)
            .map(Some)
            .map_err(|_| StoreError::InvalidRunCount {
                table,
                column,
                value: v,
            }),
    }
}

/// Checked conversion of a relationship's recoverable-byte upper bound
/// (Phase 6.4.2, Workstream C). A separate name keeps the call sites
/// readable and gives the column a relationship-specific home; the check
/// itself is the shared optional-size helper.
fn checked_recoverable_bytes(
    relationship: &coresight_identity::Relationship,
) -> Result<Option<i64>, StoreError> {
    checked_optional_size(
        relationship.recoverable_bytes,
        "relationship_obs",
        "recoverable",
    )
}

/// True when the run is older than the retention age, WITHOUT narrowing
/// the policy duration to a signed nanosecond count.
///
/// `Duration::as_nanos()` returns a `u128` that is wider than `i64`, so
/// `age.as_nanos() as i64` silently wraps any duration above ~292 years
/// (300 years becomes a large NEGATIVE number). Combined with the
/// `saturating_sub` on a possibly-nonzero elapsed time, that inversion
/// makes `over_age` fire for runs that are barely old — or fail to fire
/// for genuinely ancient ones. `Duration::MAX` must never read as a tiny
/// or negative age.
///
/// Instead the comparison is done in the wider domain: elapsed time (also
/// in nanoseconds, clamped to `u128`) is compared directly against the
/// policy's nanosecond count. A policy duration longer than any
/// measurable elapsed time can therefore never be satisfied, which is the
/// honest reading of "keep everything at least this long".
fn retention_age_exceeded(started_nanos: i64, age: Duration, now_nanos: i64) -> bool {
    // `started_nanos` is a stored timestamp; a non-positive value is
    // historical corruption, never a fresh run. Treating it as epoch 0
    // keeps the rule deterministic.
    let started = started_nanos.max(0) as u128;
    let now = now_nanos.max(0) as u128;
    let elapsed = now.saturating_sub(started);
    elapsed > age.as_nanos()
}
