//! Store-opening safety: pre-migration backup and corrupt-store
//! quarantine (Phase 6.5).
//!
//! Two guarantees live here, both about what happens to the file ON DISK
//! while [`HistoryStore::open`](crate::store::HistoryStore::open) is
//! running.
//!
//! ## 1. A migration never proceeds without a verified backup
//!
//! Migrations are transactional, so a failure inside the transaction
//! rolls the *logical* database back. What a transaction cannot protect
//! against is a failure that is not recoverable at all — a process kill,
//! a power loss, or a bug that misreads partial state — because the
//! pre-migration bytes on disk are gone. Phase 6.5 closes that gap: when
//! an existing, healthy store genuinely needs a migration, a **verified
//! backup is completed first**, through SQLite's Online Backup API (not a
//! raw file copy, which can miss uncheckpointed WAL frames).
//!
//! ## 2. A corrupt store is preserved, never replaced
//!
//! When `PRAGMA integrity_check` positively establishes corruption, the
//! damaged database **and** its `-wal` / `-shm` sidecars are relocated to
//! a collision-safe quarantine destination instead of being deleted or
//! left at a path a future open would keep failing on. `open` then
//! returns a typed error naming the quarantine location. No replacement
//! database is created, no repair is attempted, and no recovery of
//! historical data is claimed.
//!
//! ## 3. Only a recognizable store is ever touched (Phase 6.5.1, completed
//! in 6.5.2)
//!
//! "No existing file → create, existing file → inspect" is not a
//! recognition rule: any foreign SQLite file with a few tables would be
//! inspected, called current (it records no version), and then handed to
//! the mutating bootstrap, which would ADD CoreSight tables to it. The
//! recognition helpers here gather what an existing file actually is —
//! its integrity, whether it has a `schema_version` table at all, what
//! that table records, and whether the FULL structure a real store at
//! that version has (every table and signature column migrations v1–v6
//! guarantee; [`recognition_gaps`]) — and [`crate::store::HistoryStore`]
//! refuses anything it cannot recognize, leaving the database file and
//! its `-wal` untouched and deleting nothing. (SQLite's own read path may
//! rebuild a derived `-shm` WAL index while it reads a file that carries
//! a `-wal`; that cache holds no data and byte-identity is deliberately
//! not claimed for it.)
//!
//! What this establishes: the file has the structure of this build's
//! store at the version it records, and its integrity check said `ok`.
//! What it deliberately does NOT establish: PROVENANCE. A foreign
//! database deliberately forged to carry the full v1–v6 structure and a
//! valid `schema_version` row is indistinguishable from a genuine store
//! by structure alone. The check refuses files that cannot be this
//! build's store; it does not prove that a file which can be is one.
//!
//! ## 4. Integrity fails CLOSED (Phase 6.5.2)
//!
//! A `PRAGMA integrity_check` that returns NO rows has established
//! nothing: it is not a verdict of health, and it is not evidence of
//! corruption either. [`classify_integrity_rows`] reports it as a typed
//! failure ("integrity could not be established", an ordinary
//! non-corruption error with no SQLite corruption code), so every caller
//! — the store-open preflight, the post-bootstrap check, and backup
//! verification — aborts instead of proceeding, and no store is
//! quarantined on a decision that was never made.
//!
//! ## 5. Quarantine acts on the file whose corruption was OBSERVED
//! (Phase 6.5.3)
//!
//! The integrity verdict and the quarantine rename are separated by the
//! close Windows requires before a rename, so the pathname is briefly
//! unattended. The relocation is therefore bound to a [`FileIdentity`]
//! captured while the read-only connection still pinned the file the check
//! ran against, and re-established immediately before each move. If the
//! path now names a different file — or no identity could be captured — the
//! relocation is REFUSED, nothing is moved, and the error says so; the
//! caller reports the corruption without claiming the damaged file was
//! preserved. A file substituted at the path is thereby never labelled as,
//! or moved in place of, another file's corruption.
//!
//! **Threat model (honest).** Protected against: concurrent CoreSight
//! processes (each bound to the same identity; a second relocation of an
//! already-gone file fails as `prepare`), and ordinary filesystem activity
//! — a backup tool, a restore, a user replacing the file — which the
//! identity re-check detects and refuses. NOT fully protected: an
//! adversarial same-user process that renames its own file onto the path in
//! the window *between* the identity re-check and the `rename` syscall
//! itself. No portable primitive closes that window (the alternative —
//! holding a handle open across the rename — is exactly what Windows
//! forbids, and what the close-before-rename rule exists to avoid). The
//! window is a single syscall wide, the check is repeated as the last act
//! before the move, and the consequence of losing the race is bounded: a
//! file that was not the observed damaged one is relocated (not deleted,
//! not repaired), so the harm is a misplaced file, never data loss and
//! never a false claim that the damaged file was preserved.
//!
//! ## What this module deliberately does NOT do
//!
//! - No network, no subprocess, no privilege escalation, no GUI. This is a
//!   filesystem + SQLite concern only.
//! - No logging or telemetry. The crate has no log macros and this module
//!   adds none; paths reach only the returned error, which the caller
//!   decides how to present.
//! - No deletion of anything it did not create. Each backup attempt
//!   reserves its OWN temporary and destination names (a stale artifact
//!   from an earlier attempt is never adopted, reused, or deleted), and a
//!   failed attempt removes only what that attempt created.
//!
//! ## Test seams
//!
//! [`Hazard`] and [`PostBootstrapFault`] inject deterministic failures
//! (each naming a real fault a filesystem can produce) so the fail-closed
//! paths are exercised by test rather than described. Production passes
//! [`Hazard::None`]. A hazard fails ONLY the stage it names: a hazard
//! aimed at a quarantine relocation or a post-bootstrap integrity result
//! can never accidentally fail an earlier backup.
//!
//! Since Phase 6.5.2 these seams are CRATE-INTERNAL (`pub(crate)` or
//! private) and the `store_safety` module is private: the public entry
//! point is [`HistoryStore::open`](crate::store::HistoryStore::open)
//! alone. No public caller can name a hazard, force a quarantine on a
//! healthy store, or drive the low-level backup/quarantine helpers
//! directly — the hazard-driven tests live in crate-internal test modules
//! (`store::hazard_tests`).

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::store::StoreError;

/// Files whose SQLite state matters when relocating a database. The main
/// database file is always handled; the sidecars are included whenever
/// they exist so no uncheckpointed WAL frames (and no shared-memory
/// index state) are silently discarded.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Suffix for the temporary file a backup is built into before it is
/// verified and published.
const BACKUP_TEMP_SUFFIX: &str = ".backup-in-progress";

/// Suffix for a published pre-migration backup.
const BACKUP_SUFFIX: &str = ".pre-migration-backup";

/// Directory the quarantine destination lives in, next to the damaged
/// store. Keeping it adjacent means the relocated files stay on the same
/// filesystem (a rename cannot cross filesystems) and an operator looking
/// at the store finds the quarantined copy there.
const QUARANTINE_DIR_SUFFIX: &str = ".quarantined";

/// The `step` name a quarantine error carries when the partial-failure
/// ROLLBACK could not put every artifact back. The caller keys its
/// honest-report wording on this, so it lives here as a named constant
/// rather than as a string literal repeated in two modules.
pub(crate) const QUARANTINE_RESTORE_STEP: &str = "restore";

/// The `step` name a quarantine error carries when the relocation was
/// REFUSED because the file at the store path could not be proven to be the
/// file whose corruption was observed (Phase 6.5.3). The caller keys its
/// honest-report wording on this, so it lives here as a named constant
/// rather than as a string literal repeated in two modules.
pub(crate) const QUARANTINE_IDENTITY_STEP: &str = "verify file identity";

/// The identity of a filesystem entry, as the platform's own metadata
/// reports it (Unix `st_dev` + `st_ino`; Windows volume serial + the 128-bit
/// `FILE_ID_INFO.FileId`).
///
/// Phase 6.5.3 uses this to BIND a quarantine decision to the file whose
/// corruption was observed: the corruption verdict and the relocation are
/// separated by the close that Windows requires before a rename, and a file
/// substituted at the path in that interval must not be moved in place of
/// the damaged one. `None` means the platform could not prove an identity —
/// in which case relocation is refused rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    /// Unix `st_dev` / Windows volume serial number.
    device: Option<u64>,
    /// Unix `st_ino` / Windows low 64 bits of the file identifier.
    inode: Option<u64>,
    /// Windows high 64 bits of a >64-bit file identifier (ReFS-class).
    file_id_hi: Option<u64>,
}

impl FileIdentity {
    /// Observe the identity of the entry at `path` through the shared,
    /// platform-neutral metadata boundary (`coresight-engine`'s `PlatformFs`
    /// — never a `cfg` branch in this crate), or `None` when the platform
    /// cannot prove one (or the path cannot be inspected at all).
    pub(crate) fn observe(path: &Path) -> Option<FileIdentity> {
        // `coresight-engine`'s platform-neutral metadata boundary: on Unix
        // the (device, inode) pair, on Windows the volume serial and the
        // 128-bit FILE_ID_INFO file identifier.
        let md = coresight_engine::platform::std_fs().metadata(path).ok()?;
        if md.device.is_none() && md.inode.is_none() {
            return None;
        }
        Some(FileIdentity {
            device: md.device,
            inode: md.inode,
            file_id_hi: md.file_id_hi,
        })
    }
}

/// A deterministic failure injected at one named stage.
///
/// Each variant stands for a fault a real filesystem or a full disk can
/// produce. They exist so the fail-closed paths are *tested* rather than
/// asserted in a comment — an untested failure path is a claim, not a
/// guarantee.
///
/// **Not public API** (Phase 6.5.2): `pub(crate)` so only crate-internal
/// tests can name a hazard. Production callers open a store with
/// [`crate::store::HistoryStore::open`] and can never force a
/// quarantine, a backup failure, or a fabricated integrity result.
///
/// Constructed only by the crate-internal failure-path tests in
/// `store::hazard_tests`; a production build contains no constructor for
/// most variants, so dead-code analysis is silenced deliberately.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Hazard {
    /// No injected failure (production).
    #[default]
    None,
    /// The backup copy into the temporary destination fails.
    BackupCopy,
    /// The temporary backup fails verification (as if its content were
    /// unreadable).
    BackupVerify,
    /// Publishing the verified backup under its final name fails.
    BackupPublish,
    /// Relocating the main database file into quarantine fails.
    QuarantineRelocate,
    /// Relocating the LAST existing sidecar into quarantine fails, after
    /// the main database (and any earlier sidecar) has already really
    /// moved. Exercises the rollback with at least one move to undo.
    QuarantineRelocateSidecar,
    /// The same sidecar relocation failure as
    /// [`Hazard::QuarantineRelocateSidecar`], PLUS a failure to restore
    /// the main database during the rollback. Exercises the honest
    /// partial-failure report: the main file stays in quarantine and the
    /// error says exactly which artifact is where.
    QuarantineRestore,
    /// The PREFLIGHT `PRAGMA integrity_check` returns NO result rows at
    /// all — not `ok`, not a corruption report, nothing. Exercises the
    /// fail-closed rule: an empty result establishes nothing, so it must
    /// abort the open WITHOUT quarantining.
    IntegrityReportsNoRows,
    /// The BACKUP-VERIFICATION `PRAGMA integrity_check` returns NO result
    /// rows at all. Exercises the same rule at the backup gate: an
    /// unverifiable backup must never be published.
    BackupVerifyReportsNoRows,
    /// The post-bootstrap integrity check behaves as the named fault
    /// describes (it reports corruption, its result cannot be read, or it
    /// reports no rows at all).
    PostBootstrapIntegrity(PostBootstrapFault),
    /// The mutation connection WAITS at most this long for a lock held by a
    /// concurrent process, instead of the production [`OPEN_BUSY_TIMEOUT`].
    /// Exercises the lock-contention path deterministically without making
    /// the test wait out the production bound; a holder that outlasts it is
    /// still a hard `SQLITE_BUSY` failure, never a silent success.
    ///
    /// [`OPEN_BUSY_TIMEOUT`]: coresight_core::db::OPEN_BUSY_TIMEOUT
    MutationBusyTimeout(Duration),
}

/// How the post-bootstrap integrity check fails under
/// [`Hazard::PostBootstrapIntegrity`].
///
/// Constructed only by the crate-internal failure-path tests in
/// `store::hazard_tests`; dead-code analysis is silenced deliberately.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostBootstrapFault {
    /// The check runs and reports corruption rows.
    ReportsCorruption,
    /// The check runs and reports NO result rows at all (as if the pragma
    /// answered with nothing).
    ReportsNoRows,
    /// Reading the check's result fails with an ordinary I/O error — an
    /// ENVIRONMENT problem, not corruption.
    UnreadableIoError,
    /// Reading the check's result fails with SQLite's corruption code.
    UnreadableCorruption,
}

impl Hazard {
    /// Inject this hazard at a named backup stage, if this hazard is the
    /// one that targets that stage.
    ///
    /// Only a matching `Backup*` variant fails, so a hazard aimed at a
    /// later stage (a quarantine relocation, a post-bootstrap integrity
    /// result) can never accidentally fail a backup, and each injected
    /// failure is exactly the failure its test names.
    fn inject(self, stage: &str) -> Result<(), StoreError> {
        let fires = match self {
            Hazard::BackupCopy => stage == "copy",
            Hazard::BackupVerify => stage == "verify",
            Hazard::BackupPublish => stage == "publish",
            _ => false,
        };
        if fires {
            Err(StoreError::BackupFailed {
                step: stage.to_string(),
                detail: format!("injected test failure ({self:?})"),
            })
        } else {
            Ok(())
        }
    }
}

/// Suffix for the in-progress backup a failure may clean up.
fn backup_temporary(path: &Path) -> PathBuf {
    with_suffix(path, BACKUP_TEMP_SUFFIX)
}

/// Quarantine directory next to `path`.
fn quarantine_directory(path: &Path) -> PathBuf {
    with_suffix(path, QUARANTINE_DIR_SUFFIX)
}

/// Append `suffix` to the whole file name (`store.db` → `store.db.x`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// Add a numeric discriminator to a *whole* file name.
///
/// `store.db.pre-migration-backup` → `store.db.pre-migration-backup.1`.
/// Appending to the full name (rather than replacing an extension) keeps the
/// artifact recognisable: it is visibly still a backup of `store.db`.
fn numbered(base: &Path, n: usize) -> PathBuf {
    let mut name = base.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{n}"));
    match base.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// A collision-safe destination that does not exist yet.
///
/// Every candidate is probed with [`File::create_new`] (or
/// [`fs::create_dir`]), which succeeds ONLY when the path does not exist,
/// so a name that is already in use can never be handed out. The counter
/// makes this deterministic rather than random: the first free name wins
/// on every platform, and a second attempt cannot reuse the first
/// attempt's name.
///
/// The returned path is *reserved* — for a file that means an empty file
/// now exists at it. A caller that then renames a finished artifact into
/// place must remove the reservation first, which is exactly the window
/// [`publish_backup`] documents.
fn exclusive_path(base: PathBuf, is_directory: bool) -> io::Result<PathBuf> {
    for attempt in 0..10_000 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            numbered(&base, attempt)
        };
        if is_directory {
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(candidate),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        } else {
            match File::create_new(&candidate) {
                Ok(_) => return Ok(candidate),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused destination name available",
    ))
}

/// Collision-safe backup destination next to `path`.
fn exclusive_backup_destination(path: &Path) -> io::Result<PathBuf> {
    exclusive_path(with_suffix(path, BACKUP_SUFFIX), false)
}

/// This attempt's OWN temporary backup path — a fresh, empty file created
/// exclusively for this attempt.
///
/// A leftover `…backup-in-progress` from a crashed or concurrent earlier
/// attempt can therefore never become this attempt's destination (its own
/// name would collide with the leftover, so the counter advances) and is
/// never deleted: nothing this attempt did not create is touched.
fn reserve_backup_temporary(path: &Path) -> Result<PathBuf, StoreError> {
    exclusive_path(backup_temporary(path), false)
        .map_err(|e| backup_io_error("reserve temporary name", e))
}

/// Collision-safe quarantine directory next to `path`.
fn exclusive_quarantine_directory(path: &Path) -> io::Result<PathBuf> {
    exclusive_path(quarantine_directory(path), true)
}

/// What the `schema_version` table of an open connection records.
///
/// The three cases are kept apart because the caller must treat them
/// differently: a recorded version is inspectable, a missing table means
/// "no CoreSight schema at all", and a present-but-unusable table means
/// "not a store this build wrote" — none of them silently falls back to
/// version 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordedVersion {
    /// Exactly one `INTEGER` version row was recorded.
    Version(u32),
    /// There is no `schema_version` table in the database.
    NoTable,
    /// The `schema_version` table exists but does not hold exactly one
    /// usable non-negative integer version. The string says what was found.
    Unusable(String),
}

/// The schema version recorded in an open connection.
///
/// A recognizable store records EXACTLY one non-negative integer in its
/// `schema_version` table. Anything else — no table, no row, several rows,
/// a non-integer value, a value outside `u32` — is reported as
/// [`RecordedVersion::Unusable`] rather than being coerced or defaulted to
/// 0. The only failures returned as `Err` are reads that could not be
/// COMPLETED (e.g. an I/O or corruption error): those are classified by
/// the caller, which never treats them as "no version".
pub(crate) fn recorded_version(conn: &Connection) -> Result<RecordedVersion, StoreError> {
    // Does the table exist at all? `sqlite_master` is readable even when
    // the table is missing, so this distinguishes "brand-new store" from
    // "some other SQLite database".
    let table_exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    if table_exists == 0 {
        return Ok(RecordedVersion::NoTable);
    }

    let mut stmt = match conn.prepare("SELECT version FROM schema_version") {
        Ok(stmt) => stmt,
        // The table exists but has no `version` column to read. rusqlite
        // validates statements at prepare time, so this surfaces here.
        Err(rusqlite::Error::SqlInputError { msg, .. })
            if msg.to_lowercase().contains("no such column") =>
        {
            return Ok(RecordedVersion::Unusable(
                "the schema_version table has no version column".to_string(),
            ))
        }
        Err(e) => return Err(StoreError::Sqlite(e)),
    };
    let rows: Result<Vec<u32>, rusqlite::Error> = stmt
        .query_map([], |r| r.get::<_, u32>(0))
        .map_err(StoreError::Sqlite)?
        .collect();
    drop(stmt);

    match rows {
        Ok(rows) if rows.len() == 1 => Ok(RecordedVersion::Version(rows[0])),
        Ok(rows) if rows.is_empty() => Ok(RecordedVersion::Unusable(
            "the schema_version table records no version row".to_string(),
        )),
        Ok(rows) => Ok(RecordedVersion::Unusable(format!(
            "the schema_version table records {} version rows; a recognizable store records \
             exactly one",
            rows.len()
        ))),
        Err(rusqlite::Error::InvalidColumnType(_, _, ty)) => Ok(RecordedVersion::Unusable(
            format!("the recorded version is not an integer (stored as SQLite type {ty:?})"),
        )),
        Err(rusqlite::Error::IntegralValueOutOfRange(_, value)) => Ok(RecordedVersion::Unusable(
            format!("the recorded version {value} is outside the valid 0..=4294967295 range"),
        )),
        // The table exists but has no `version` column to read.
        Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
            if msg.to_lowercase().contains("no such column") =>
        {
            Ok(RecordedVersion::Unusable(
                "the schema_version table has no version column".to_string(),
            ))
        }
        // Everything else is a failure to READ, which the caller must
        // classify (corruption quarantines; environment problems do not).
        Err(e) => Err(StoreError::Sqlite(e)),
    }
}

// ---------------------------------------------------------------------------
// The recognition contract (Phase 6.5.2, Workstream C)
//
// What a store at each recorded version must structurally possess, derived
// from the migration history this build actually shipped: v1 is the
// Phase-0 bootstrap (coresight-core); v2 created the history tables; v3
// widened object identity with `file_id_hi`; v4 added the persisted
// relationship status; v5 created the normalized `app_snapshot_*` tables;
// v6 added the application-id encoding column.
//
// A recorded version PROMISES that its migrations ran, so the structure of
// those migrations must be present. A store missing any of it is refused
// as `UnrecognizedSchema` — never handed to the mutating bootstrap.
// ---------------------------------------------------------------------------

/// The Phase-0 bootstrap tables: present at every recorded version.
const V1_TABLES: [&str; 2] = ["drives", "scans"];

/// Signature columns of the v1 bootstrap. A file with a `drives` table
/// that is not this build's `drives` is not a store this build wrote.
const V1_COLUMNS: [(&str, &str); 3] = [("drives", "id"), ("scans", "id"), ("scans", "drive_id")];

/// The history tables migration v2 created: present at every v2+ store.
const V2_TABLES: [&str; 4] = [
    "scan_runs",
    "observations",
    "relationship_obs",
    "relationship_members",
];

/// Signature columns of the v2 history tables (the identity columns the
/// history API reads them by).
const V2_COLUMNS: [(&str, &str); 15] = [
    ("scan_runs", "run_id"),
    ("scan_runs", "started_at"),
    ("scan_runs", "roots"),
    ("scan_runs", "platform"),
    ("scan_runs", "config"),
    ("scan_runs", "status"),
    ("observations", "run_id"),
    ("observations", "path"),
    ("observations", "kind"),
    ("relationship_obs", "run_id"),
    ("relationship_obs", "rel_id"),
    ("relationship_obs", "kind"),
    ("relationship_members", "run_id"),
    ("relationship_members", "rel_id"),
    ("relationship_members", "path"),
];

/// v3 (Phase 5.1) widened object identity with the high half of the file
/// id. Present at every v3+ store.
const V3_COLUMNS: [(&str, &str); 2] = [
    ("observations", "file_id_hi"),
    ("relationship_members", "file_id_hi"),
];

/// v4 persists each run's relationship-report status and truncation
/// flag. Present at every v4+ store.
const V4_COLUMNS: [(&str, &str); 2] = [("scan_runs", "rel_status"), ("scan_runs", "rel_truncated")];

/// The v5 (Phase 6.4) normalized application/system snapshot tables.
/// Present at every v5+ store.
const V5_TABLES: [&str; 13] = [
    "app_snapshot_meta",
    "app_snapshot_artifacts",
    "app_snapshot_apps",
    "app_snapshot_provenance",
    "app_snapshot_views",
    "app_snapshot_roots",
    "app_snapshot_evidence",
    "app_snapshot_coverage",
    "app_snapshot_relationships",
    "app_snapshot_rel_members",
    "app_snapshot_history",
    "app_snapshot_footprints",
    "app_snapshot_footprint_evidence",
];

/// v6 (Phase 6.4.1) records the application-id encoding on each row.
/// Present at every v6 store.
const V6_COLUMNS: [(&str, &str); 1] = [("app_snapshot_apps", "id_encoding")];

/// Whether `name` is absent from `sqlite_master` as a table.
fn table_missing(conn: &Connection, name: &str) -> Result<bool, StoreError> {
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    Ok(count == 0)
}

/// Whether `table` exists but has no column named `column`.
fn column_missing(conn: &Connection, table: &str, column: &str) -> Result<bool, StoreError> {
    // `pragma_table_info` is SQLite's table-valued pragma: the table name
    // is a bound parameter, never interpolated SQL.
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |r| r.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    Ok(count == 0)
}

/// The pieces of the required structure that are missing for a store
/// recording `version`: tables a real store at that version always has,
/// and — for tables that exist — the signature columns those tables
/// always have.
///
/// Each finding is a description ("missing table drives", "missing column
/// observations.file_id_hi") so the refusal names exactly what failed to
/// match. Columns of a MISSING table are not listed separately: the table
/// itself is the finding.
///
/// The contract is the MINIMUM STRUCTURE the migration history guarantees,
/// not a full schema comparison: migrations are forward-only
/// `CREATE TABLE IF NOT EXISTS`/`ADD COLUMN` steps, and this is what a
/// genuine store at `version` cannot lack. See the module docs for what
/// this establishes (structure, not provenance).
pub(crate) fn recognition_gaps(conn: &Connection, version: u32) -> Result<Vec<String>, StoreError> {
    let mut tables: Vec<&'static str> = V1_TABLES.to_vec();
    let mut columns: Vec<(&'static str, &'static str)> = V1_COLUMNS.to_vec();
    if version >= 2 {
        tables.extend(V2_TABLES);
        columns.extend(V2_COLUMNS);
    }
    if version >= 3 {
        columns.extend(V3_COLUMNS);
    }
    if version >= 4 {
        columns.extend(V4_COLUMNS);
    }
    if version >= 5 {
        tables.extend(V5_TABLES);
    }
    if version >= 6 {
        columns.extend(V6_COLUMNS);
    }

    let mut gaps = Vec::new();
    for name in tables {
        if table_missing(conn, name)? {
            gaps.push(format!("missing table {name}"));
        }
    }
    for (table, column) in columns {
        if table_missing(conn, table)? {
            // Already reported as a missing table; a missing column of a
            // missing table is not a second finding.
            continue;
        }
        if column_missing(conn, table, column)? {
            gaps.push(format!("missing column {table}.{column}"));
        }
    }
    Ok(gaps)
}

/// The raw rows `PRAGMA integrity_check` returns.
///
/// A RESULT-READING failure is surfaced as an error, never read as
/// "healthy": silently treating an unreadable result as equivalent to
/// `ok` would let a damaged store through the exact gate that is supposed
/// to catch it. `collect::<Result<..>>()` rather than
/// `filter_map(|r| r.ok())`: a row that cannot be READ is a failure to
/// decide, and must not be silently dropped into a "no problems"
/// conclusion.
pub(crate) fn integrity_rows(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Classify the raw `PRAGMA integrity_check` rows into "problems found".
///
/// **Fail closed on emptiness** (Phase 6.5.2): a check that produced NO
/// rows established NOTHING. It is reported as a typed failure to
/// establish integrity — an ordinary, non-corruption error that carries
/// no SQLite corruption code — so no caller can read it as health
/// (healthy) and no caller can read it as damage (quarantine). The pragma
/// is documented to answer with at least `ok`; this code does not rely on
/// that documentation holding, because "documented" is not "guaranteed by
/// our own control".
///
/// Rows other than `ok` are positive evidence of corruption and are
/// returned. An all-`ok`, non-empty result is the ONLY healthy outcome.
pub(crate) fn classify_integrity_rows(rows: Vec<String>) -> Result<Vec<String>, StoreError> {
    if rows.is_empty() {
        return Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::Unknown,
                extended_code: rusqlite::ffi::ErrorCode::Unknown as i32,
            },
            Some(
                "PRAGMA integrity_check returned no result rows, so integrity could not be \
                 established (this is not a verdict of health)"
                    .to_string(),
            ),
        )));
    }
    Ok(rows.into_iter().filter(|r| r != "ok").collect())
}

/// Every row `PRAGMA integrity_check` reports other than `ok`, with the
/// empty-result rule applied (see [`classify_integrity_rows`]).
pub(crate) fn integrity_problems(conn: &Connection) -> Result<Vec<String>, StoreError> {
    classify_integrity_rows(integrity_rows(conn)?)
}

/// Everything a recognition verdict needs from an open connection,
/// gathered in one place.
///
/// Owns no `Connection`: the caller must be able to CLOSE the database
/// before it acts on the verdict (Windows cannot rename an open file), so
/// the snapshot is plain owned data.
pub(crate) struct InspectionSnapshot {
    /// Rows `PRAGMA integrity_check` reported other than `ok`.
    pub problems: Vec<String>,
    /// What the `schema_version` table records.
    pub version: RecordedVersion,
    /// Required structure missing for the recorded version (empty when
    /// what is missing cannot be judged, e.g. no version is recorded).
    pub recognition_gaps: Vec<String>,
    /// Total rows in `sqlite_master` (tables, indexes, views, triggers):
    /// zero means the file holds no schema at all.
    pub object_count: i64,
}

/// Gather every fact a recognition verdict needs, before the connection is
/// closed.
///
/// Integrity runs FIRST and its failure is returned for the caller to
/// classify: corruption codes quarantine, environment failures do not,
/// an empty result is "could not establish" (fail closed, neither of the
/// other two), and none of them is smoothed into a verdict here.
pub(crate) fn inspect_snapshot(
    conn: &Connection,
    hazard: Hazard,
    after_integrity: &mut dyn FnMut(),
) -> Result<InspectionSnapshot, StoreError> {
    let problems = if hazard == Hazard::IntegrityReportsNoRows {
        // The injected outcome is fed through the REAL classifier, so this
        // seam exercises the production decision rather than a shortcut
        // around it.
        classify_integrity_rows(Vec::new())?
    } else {
        integrity_problems(conn)?
    };
    // The Phase 6.5.3 I/O seam: integrity has been read, and the recorded
    // version / recognition facts have NOT yet been gathered. A concurrent
    // writer releasing the write lock at exactly this point is the race the
    // serialized migration path must survive.
    after_integrity();
    let object_count: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .map_err(StoreError::Sqlite)?;
    let version = recorded_version(conn)?;
    let recognition_gaps = match &version {
        RecordedVersion::Version(version) => recognition_gaps(conn, *version)?,
        RecordedVersion::NoTable | RecordedVersion::Unusable(_) => Vec::new(),
    };
    Ok(InspectionSnapshot {
        problems,
        version,
        recognition_gaps,
        object_count,
    })
}

/// Whether `path` names an existing regular file.
fn is_existing_file(path: &Path) -> bool {
    path.is_file()
}

/// Convert a filesystem failure into a typed backup error naming the
/// stage, so the caller's failure is diagnosable without a log that could
/// contain paths.
fn backup_io_error(step: &str, e: io::Error) -> StoreError {
    StoreError::BackupFailed {
        step: step.to_string(),
        detail: e.to_string(),
    }
}

/// Convert an SQLite failure during backup into the same typed shape, so a
/// backup problem is always one error type regardless of which layer
/// produced it.
fn backup_sqlite_error(step: &str, e: rusqlite::Error) -> StoreError {
    StoreError::BackupFailed {
        step: step.to_string(),
        detail: e.to_string(),
    }
}

/// A named stage of a backup attempt, reported to a test observer
/// ([`verified_pre_migration_backup_observed`]). Production passes an
/// observer that ignores every stage.
#[derive(Debug)]
enum BackupStage<'a> {
    /// The attempt has reserved its own temporary path (an empty file this
    /// attempt created) and will copy into it.
    ///
    /// The path is carried for TEST observers; production discards the
    /// stage, so the field is deliberately unread outside tests.
    TemporaryReserved(#[allow(dead_code)] &'a Path),
    /// The temporary artifact has passed verification. The path is carried
    /// for TEST observers, as above.
    TemporaryVerified(#[allow(dead_code)] &'a Path),
}

/// A verified pre-migration backup for the store at `path`.
///
/// ## The copy is SQLite-consistent
///
/// The destination is written through SQLite's Online Backup API
/// (`rusqlite::backup`), driven from the live source connection. SQLite
/// restarts the backup when the source changes mid-copy, and a WAL source
/// is read through its WAL, so the result reflects a single consistent
/// point in the source database's history — unlike a raw file copy, which
/// can race uncheckpointed WAL frames and produce a torn or stale image.
///
/// ## Publish only what has been verified
///
/// The backup is built into a temporary destination, re-opened by a
/// SEPARATE connection (so verification reads what is actually on disk),
/// and checked for both structural integrity and the expected schema
/// version BEFORE it is published under its final name.
///
/// ## Every attempt owns its own names
///
/// Both the temporary path and the final name are reserved per attempt
/// with `create_new` (advancing through `.1`, `.2`, … on collisions), so
/// two concurrent attempts — or an attempt following a crashed one —
/// never share a temporary, never adopt a stale artifact, and never
/// delete one. A failure removes exactly the two names this attempt
/// created.
///
/// ## Failure is closed
///
/// If any step fails, the temporary artifact and the still-empty reserved
/// name this attempt created are removed and the error is returned. The
/// caller must not migrate without a `Ok` result: an un-migrated store is
/// still readable, whereas one migrated without a backup is not.
///
/// `expected_version` is the pre-migration version the source recorded; it
/// must match, because a backup of a different version would not
/// represent the state the migration is about to change.
pub(crate) fn verified_pre_migration_backup(
    conn: &Connection,
    path: &Path,
    expected_version: Option<u32>,
    hazard: Hazard,
) -> Result<PathBuf, StoreError> {
    verified_pre_migration_backup_observed(conn, path, expected_version, hazard, &mut |_| {})
}

/// [`verified_pre_migration_backup`] with a stage observer — the unit-test
/// seam for deterministic interleaving of two concurrent attempts.
fn verified_pre_migration_backup_observed(
    conn: &Connection,
    path: &Path,
    expected_version: Option<u32>,
    hazard: Hazard,
    observe: &mut dyn FnMut(BackupStage<'_>),
) -> Result<PathBuf, StoreError> {
    if !is_existing_file(path) {
        return Err(StoreError::Sqlite(rusqlite::Error::InvalidPath(
            path.to_path_buf(),
        )));
    }

    // Reserve the FINAL name first: it is an EMPTY file this attempt
    // created, tracked separately from the temporary because a failure must
    // remove BOTH — an empty file left under the backup name would LOOK
    // like a published backup to the next reader (and to the next
    // attempt's collision counter) while containing nothing. If the
    // temporary reservation then fails, the reserved name is released.
    let reserved = exclusive_backup_destination(path)
        .map_err(|e| backup_io_error("reserve destination name", e))?;
    let temporary = match reserve_backup_temporary(path) {
        Ok(temporary) => temporary,
        Err(e) => {
            let _ = fs::remove_file(&reserved);
            return Err(e);
        }
    };

    let attempt = (|| -> Result<PathBuf, StoreError> {
        observe(BackupStage::TemporaryReserved(&temporary));

        // Build the backup into the temporary destination.
        hazard.inject("copy")?;
        conn.backup(rusqlite::DatabaseName::Main, &temporary, None)
            .map_err(|e| backup_sqlite_error("copy", e))?;

        // Verify BEFORE publishing: an unverified artifact must never be
        // reachable under the name that means "a safe copy exists".
        hazard.inject("verify")?;
        verify_backup(&temporary, expected_version, hazard)?;
        observe(BackupStage::TemporaryVerified(&temporary));

        hazard.inject("publish")?;
        publish_backup(&temporary, &reserved)?;
        Ok(reserved.clone())
    })();

    match attempt {
        Ok(destination) => Ok(destination),
        Err(e) => {
            // Clean up ONLY what this attempt created: the temporary copy and
            // the still-empty reserved name. A backup published by an
            // EARLIER attempt is never touched — the reservation is
            // collision-safe precisely so this attempt never got that name —
            // and neither is a stale temporary another attempt left behind.
            let _ = fs::remove_file(&temporary);
            let _ = fs::remove_file(&reserved);
            Err(e)
        }
    }
}

/// Atomically publish a verified backup by renaming it into place.
///
/// The destination was reserved by `exclusive_backup_destination`, which
/// left an EMPTY file there. That reservation is removed first so the
/// rename can take its place.
///
/// ## Known, accepted limitations
///
/// - `remove` + `rename` is not one atomic step. A crash between them
///   leaves a stray EMPTY file under the backup name. The consequence is
///   bounded: the name means "a backup exists" but the file contains
///   nothing. This is why the backup is verified BEFORE publication and
///   the rename is the last step — a name is only ever published by a
///   rename of an already-verified file.
/// - The reservation is a plain rename, so on a filesystem where a
///   concurrent writer could place its own file at that exact name between
///   the remove and the rename, `rename` would replace it on Unix. The
///   name is collision-safe against *this* code (which picks a fresh name
///   rather than reusing a taken one), so two CoreSight processes cannot
///   collide. Only a non-CoreSight writer deliberately targeting the exact
///   backup path could, and the window is a single rename wide. A
///   `renameat2(RENAME_NOREPLACE)` would close it, but that syscall is not
///   portable to Windows or macOS and this phase adds no dependencies.
fn publish_backup(temporary: &Path, destination: &Path) -> Result<(), StoreError> {
    let _ = fs::remove_file(destination);
    fs::rename(temporary, destination).map_err(|e| backup_io_error("publish", e))
}

/// Re-open `path` with a SEPARATE connection and verify it is
/// structurally intact and carries `expected_version`.
///
/// Integrity fails closed here too (Phase 6.5.2): a verification that
/// cannot establish integrity — including one whose `PRAGMA
/// integrity_check` returned no rows at all — must never let the artifact
/// be published under the name that means "a safe copy exists". Both the
/// integrity failure and a version that cannot be read are reported as
/// [`StoreError::BackupFailed`] with step `verify`, so a backup problem is
/// one error type regardless of which layer produced it (the migration
/// then aborts with the source unchanged).
fn verify_backup(
    path: &Path,
    expected_version: Option<u32>,
    hazard: Hazard,
) -> Result<(), StoreError> {
    if !is_existing_file(path) {
        return Err(StoreError::BackupFailed {
            step: "verify".to_string(),
            detail: "backup destination does not exist after the copy completed".to_string(),
        });
    }
    let conn = Connection::open(path).map_err(|e| backup_sqlite_error("verify open", e))?;
    let problems = if hazard == Hazard::BackupVerifyReportsNoRows {
        // Injected empty outcome, fed through the real classifier: the
        // same decision the production path takes.
        classify_integrity_rows(Vec::new())
    } else {
        integrity_problems(&conn)
    }
    .map_err(|e| StoreError::BackupFailed {
        step: "verify".to_string(),
        detail: format!("the backup's integrity could not be established: {e}"),
    })?;
    if !problems.is_empty() {
        return Err(StoreError::BackupFailed {
            step: "verify".to_string(),
            detail: format!(
                "PRAGMA integrity_check did not report ok: {}",
                problems.join("; ")
            ),
        });
    }
    let found = match recorded_version(&conn).map_err(|e| StoreError::BackupFailed {
        step: "verify".to_string(),
        detail: format!("the backup's recorded schema version could not be read: {e}"),
    })? {
        RecordedVersion::Version(found) => Some(found),
        RecordedVersion::NoTable => None,
        // A backup whose own version cannot be read is not verifiable:
        // refusing it is the only closed outcome — publishing it would
        // claim a verified backup exists when the verification could not
        // decide.
        RecordedVersion::Unusable(detail) => {
            return Err(StoreError::BackupFailed {
                step: "verify".to_string(),
                detail: format!("the backup's recorded schema version is unusable: {detail}"),
            })
        }
    };
    match (found, expected_version) {
        // Both known and different: the backup does not represent the
        // state the migration is about to change.
        (Some(found), Some(expected)) if found != expected => {
            return Err(StoreError::BackupFailed {
                step: "verify".to_string(),
                detail: format!(
                    "schema version mismatch: source records {expected}, backup records {found}"
                ),
            })
        }
        // A freshly created source has no version recorded yet; the
        // backup must match that lack.
        (None, None) => {}
        (Some(_), None) | (None, Some(_)) => {
            return Err(StoreError::BackupFailed {
                step: "verify".to_string(),
                detail: "backup and source disagree about whether a schema version is recorded"
                    .to_string(),
            })
        }
        (Some(_), Some(_)) => {}
    }
    Ok(())
}

/// A quarantine destination directory holding relocated store files.
///
/// The relocated files keep their ORIGINAL names inside the directory, so
/// listing it describes exactly what was preserved. Nothing here is
/// interpreted, repaired, or claimed to be recovered.
///
/// **Not public API** (Phase 6.5.2): quarantine relocations happen only
/// inside [`crate::store::HistoryStore::open`], on evidence the open path
/// established. No public caller can name this type, drive a relocation,
/// or point it at an arbitrary store.
#[derive(Debug, Clone)]
pub(crate) struct Quarantine {
    directory: PathBuf,
}

impl Quarantine {
    /// The quarantine directory. Callers that report it must report it as
    /// a LOCATION only — the damaged data's contents are never described
    /// as recovered or repaired.
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
}

/// Quarantine the damaged store at `path`, including its SQLite sidecars.
///
/// ## Rules this upholds
///
/// - The relocation acts only on the file whose corruption was OBSERVED
///   (Phase 6.5.3). `observed` is the identity captured while the read-only
///   connection still pinned that file; it is re-established immediately
///   before the relocation. A path that now names a different file — the
///   substitute a hostile or ordinary writer could drop in between the
///   integrity verdict and the rename — is refused with
///   [`QUARANTINE_IDENTITY_STEP`], and NOTHING is moved. When no identity
///   could be captured the relocation is refused too: an unprovable file is
///   never guessed at.
/// - The destination directory is created exclusively, so an existing
///   quarantined artifact is never overwritten.
/// - Every sidecar that exists is moved, so uncheckpointed WAL frames are
///   preserved rather than silently discarded.
/// - A partial failure attempts to restore the files that were already
///   moved, so a caller's error can promise the store is either fully
///   quarantined or still in place.
/// - If restoration also fails, every remaining artifact is left where it
///   is and the error names the destination, so nothing is hidden and no
///   successful quarantine is claimed.
///
/// The caller is responsible for closing its live connection BEFORE
/// calling this: SQLite holds file handles, and on Windows an open
/// database file cannot be renamed.
///
/// **Not public API** (Phase 6.5.2): `pub(crate)` so a relocation can only
/// be driven by the store-open path's own, established corruption
/// evidence — never by a public caller pointing it at an arbitrary file.
pub(crate) fn quarantine_corrupt_store(
    path: &Path,
    observed: Option<FileIdentity>,
    hazard: Hazard,
) -> Result<Quarantine, StoreError> {
    if !is_existing_file(path) {
        return Err(StoreError::QuarantineFailed {
            step: "prepare".to_string(),
            detail: "the store file no longer exists".to_string(),
        });
    }

    // Bind the relocation to the file whose corruption was OBSERVED. This is
    // the check that stops a file substituted at the store path (after the
    // read-only connection closed and before the rename) from being moved
    // aside as though it were the damaged one. Both halves fail closed:
    // an identity that was never captured, and an identity that no longer
    // describes the path, both refuse to move anything.
    let expected = match observed {
        Some(expected) => {
            if FileIdentity::observe(path) != Some(expected) {
                return Err(StoreError::QuarantineFailed {
                    step: QUARANTINE_IDENTITY_STEP.to_string(),
                    detail: "the file now at the store path is not the file whose corruption \
                             was observed, so nothing was moved"
                        .to_string(),
                });
            }
            expected
        }
        None => {
            return Err(StoreError::QuarantineFailed {
                step: QUARANTINE_IDENTITY_STEP.to_string(),
                detail: "the identity of the file whose corruption was observed could not be \
                         established, so it cannot be proven that the file at the store path \
                         is the damaged one; nothing was moved"
                    .to_string(),
            });
        }
    };

    let directory =
        exclusive_quarantine_directory(path).map_err(|e| StoreError::QuarantineFailed {
            step: "create quarantine directory".to_string(),
            detail: e.to_string(),
        })?;

    // Move the main file first: if that succeeds, the sidecars follow, and
    // the "already moved" set is exactly what a rollback must undo.
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    match relocate_all(path, expected, &directory, &mut moved, hazard) {
        Ok(()) => Ok(Quarantine { directory }),
        Err(e) => {
            // Attempt to restore what was moved, newest name last so the
            // reverse order undoes the reverse of the move order.
            // Destination names are derived from the ORIGINALS, so a
            // restore can never overwrite a file this attempt did not move.
            let mut restoration_failures = Vec::new();
            for (original, quarantined) in moved.iter().rev() {
                let name = original
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if hazard == Hazard::QuarantineRestore && original.as_path() == path {
                    // The injected fault: this artifact CANNOT be put
                    // back, so it stays in the quarantine directory — the
                    // exact situation the honest partial-failure report
                    // exists for.
                    restoration_failures.push(format!(
                        "{name} could not be restored (injected test failure \
                         (QuarantineRestore)) and remains at {}",
                        quarantined.display()
                    ));
                    continue;
                }
                if let Err(re) = fs::rename(quarantined, original) {
                    restoration_failures.push(format!(
                        "{name} could not be restored ({re}) and remains at {}",
                        quarantined.display()
                    ));
                }
            }
            // Only an EMPTY directory (every artifact restored) is removed;
            // one that still holds an artifact stays, so nothing is hidden.
            let _ = fs::remove_dir(&directory);
            if restoration_failures.is_empty() {
                Err(e)
            } else {
                // Truthful partial-failure report: say exactly which
                // artifacts could NOT be put back, and do not claim the
                // store was quarantined.
                Err(StoreError::QuarantineFailed {
                    step: QUARANTINE_RESTORE_STEP.to_string(),
                    detail: format!(
                        "{e}; these artifacts could not be restored and remain in {}: {}",
                        directory.display(),
                        restoration_failures.join("; ")
                    ),
                })
            }
        }
    }
}

/// Move `path` and each existing sidecar into `directory`.
///
/// `expected` is the identity the MAIN file must still carry; it is
/// re-checked as the last act before that rename (see [`relocate_one`]).
/// Sidecars are derived artifacts of the same store and are relocated
/// without an identity check of their own.
fn relocate_all(
    path: &Path,
    expected: FileIdentity,
    directory: &Path,
    moved: &mut Vec<(PathBuf, PathBuf)>,
    hazard: Hazard,
) -> Result<(), StoreError> {
    relocate_one(path, Some(expected), directory, moved, hazard)?;
    // The sidecars that exist NOW, in relocation order, so an injected
    // failure can hit the LAST one — after the main database has really
    // moved — exercising the rollback with something real to undo.
    let sidecars: Vec<PathBuf> = SIDECAR_SUFFIXES
        .iter()
        .map(|suffix| with_suffix(path, suffix))
        .filter(|sidecar| sidecar.exists())
        .collect();
    let last_index = sidecars.len().saturating_sub(1);
    for (index, sidecar) in sidecars.iter().enumerate() {
        let fails_on_last_sidecar = matches!(
            hazard,
            Hazard::QuarantineRelocateSidecar | Hazard::QuarantineRestore
        );
        if fails_on_last_sidecar && index == last_index {
            return Err(StoreError::QuarantineFailed {
                step: "relocate".to_string(),
                detail: format!("injected test failure ({hazard:?})"),
            });
        }
        relocate_one(sidecar, None, directory, moved, hazard)?;
    }
    Ok(())
}

/// Move one file, recording it so a later failure can restore it.
///
/// When `expected` is `Some`, the file's identity is re-established here as
/// the LAST act before the rename, narrowing the window in which a writer
/// could substitute a different file at `from` between the earlier check
/// and the move itself. A mismatch moves nothing (and the caller's rollback
/// then has an empty move list to undo). The window is narrowed, not closed:
/// see the residual-limitation note on [`quarantine_corrupt_store`]'s
/// module documentation.
fn relocate_one(
    from: &Path,
    expected: Option<FileIdentity>,
    directory: &Path,
    moved: &mut Vec<(PathBuf, PathBuf)>,
    hazard: Hazard,
) -> Result<(), StoreError> {
    if hazard == Hazard::QuarantineRelocate {
        return Err(StoreError::QuarantineFailed {
            step: "relocate".to_string(),
            detail: "injected test failure (QuarantineRelocate)".to_string(),
        });
    }
    if let Some(expected) = expected {
        if FileIdentity::observe(from) != Some(expected) {
            return Err(StoreError::QuarantineFailed {
                step: QUARANTINE_IDENTITY_STEP.to_string(),
                detail: "the file changed between the identity check and the relocation, so \
                         nothing was moved"
                    .to_string(),
            });
        }
    }
    let file_name = match from.file_name() {
        Some(name) => name,
        None => {
            return Err(StoreError::QuarantineFailed {
                step: "relocate".to_string(),
                detail: "the store path has no file name".to_string(),
            })
        }
    };
    let to = directory.join(file_name);
    // The destination directory was created exclusively for this attempt
    // and is empty, so a collision here would mean a bug, not a real risk.
    // Checking anyway keeps the guarantee if that ever changes.
    if to.exists() {
        return Err(StoreError::QuarantineFailed {
            step: "relocate".to_string(),
            detail: "a quarantined artifact with that name already exists".to_string(),
        });
    }
    match fs::rename(from, &to) {
        Ok(()) => {
            moved.push((from.to_path_buf(), to));
            Ok(())
        }
        Err(_) => Err(StoreError::QuarantineFailed {
            step: "relocate".to_string(),
            detail: format!(
                "could not move {} into quarantine",
                from.file_name().unwrap_or_default().to_string_lossy()
            ),
        }),
    }
}

/// Whether an SQLite error is positive evidence of database corruption.
///
/// Only codes that SQLite itself defines as "this file is not a usable
/// database" qualify. Everything else — permission denial, lock
/// contention, a full disk, an ordinary I/O failure — is an environment
/// problem that quarantine would only make worse by moving a healthy file
/// aside. (The "no result rows" error [`classify_integrity_rows`]
/// produces carries `ErrorCode::Unknown` and deliberately does NOT
/// qualify: failing to establish integrity is not evidence of damage.)
///
/// **Not public API** (Phase 6.5.2): an internal classification primitive,
/// `pub(crate)` for the open path and its crate-internal tests.
pub(crate) fn sqlite_error_indicates_corruption(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(ffi, _) => {
            use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
            // `ffi.code` is SQLite's PRIMARY result code (`raw & 0xff`), so
            // it already covers the whole corruption family — the extended
            // variants (SQLITE_CORRUPT_VTAB, SQLITE_CORRUPT_INDEX, …) all
            // mask to SQLITE_CORRUPT. The raw extended-code comparisons are
            // belt-and-braces for an error built by hand, and MUST compare
            // against SQLite's own result-code constants: the Rust
            // `ErrorCode` enum has implicit discriminants that do NOT match
            // them (`DatabaseCorrupt as i32` is 9, while SQLITE_CORRUPT is
            // 11) — comparing against the discriminant also made
            // SQLITE_INTERRUPT's raw code (9) look like corruption, which it
            // is not.
            matches!(ffi.code, DatabaseCorrupt | NotADatabase)
                || ffi.extended_code == rusqlite::ffi::SQLITE_CORRUPT
                || ffi.extended_code == rusqlite::ffi::SQLITE_NOTADB
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_suffix_appends_to_file_name_only() {
        let path = Path::new("/tmp/coresight.db");
        assert_eq!(
            with_suffix(path, ".pre-migration-backup"),
            PathBuf::from("/tmp/coresight.db.pre-migration-backup")
        );
        assert_eq!(
            with_suffix(path, "-wal"),
            PathBuf::from("/tmp/coresight.db-wal")
        );
    }

    #[test]
    fn numbered_appends_a_discriminator_to_the_whole_name() {
        let base = Path::new("/tmp/coresight.db.pre-migration-backup");
        assert_eq!(
            numbered(base, 1),
            PathBuf::from("/tmp/coresight.db.pre-migration-backup.1")
        );
    }

    #[test]
    fn exclusive_path_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("a.db.pre-migration-backup");
        let first = exclusive_path(base.clone(), false).unwrap();
        assert!(File::create_new(&first).is_err());
        let second = exclusive_path(base, false).unwrap();
        assert_ne!(first, second);
        assert_eq!(second.file_name().unwrap(), "a.db.pre-migration-backup.1");
    }

    #[test]
    fn exclusive_directory_path_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("a.db.quarantined");
        let first = exclusive_path(base.clone(), true).unwrap();
        let second = exclusive_path(base, true).unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert_eq!(second.file_name().unwrap(), "a.db.quarantined.1");
    }

    #[test]
    fn only_defined_corruption_codes_qualify() {
        use rusqlite::ffi::ErrorCode;
        let corruption = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseCorrupt,
                extended_code: rusqlite::ffi::SQLITE_CORRUPT,
            },
            Some("database disk image is malformed".to_string()),
        );
        assert!(sqlite_error_indicates_corruption(&corruption));

        let not_a_database = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::NotADatabase,
                extended_code: rusqlite::ffi::SQLITE_NOTADB,
            },
            Some("file is not a database".to_string()),
        );
        assert!(sqlite_error_indicates_corruption(&not_a_database));

        // A corruption result whose primary code is unknown but whose RAW
        // extended code is SQLITE_CORRUPT still qualifies (the
        // belt-and-braces branch).
        let extended_only = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::Unknown,
                extended_code: rusqlite::ffi::SQLITE_CORRUPT,
            },
            Some("extended corruption code".to_string()),
        );
        assert!(sqlite_error_indicates_corruption(&extended_only));

        // The trap a discriminant-based comparison would set:
        // `ErrorCode::DatabaseCorrupt as i32` is 9, which equals
        // SQLITE_INTERRUPT's raw code — but an interrupt is NOT corruption
        // and must never quarantine a healthy store.
        let interrupt = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::OperationInterrupted,
                extended_code: rusqlite::ffi::SQLITE_INTERRUPT,
            },
            Some("interrupted".to_string()),
        );
        assert!(
            !sqlite_error_indicates_corruption(&interrupt),
            "SQLITE_INTERRUPT is not corruption"
        );

        // Environment problems must NOT qualify: quarantining on these
        // would move a HEALTHY file aside. Each carries its REAL raw
        // result code — constructed from the Rust discriminant, several
        // would collide with real corruption codes (DiskFull's implicit
        // discriminant is 11, SQLITE_CORRUPT) and mask exactly the defect
        // this test now pins.
        for (code, raw) in [
            (ErrorCode::SystemIoFailure, rusqlite::ffi::SQLITE_IOERR),
            (ErrorCode::PermissionDenied, rusqlite::ffi::SQLITE_PERM),
            (ErrorCode::DatabaseBusy, rusqlite::ffi::SQLITE_BUSY),
            (ErrorCode::CannotOpen, rusqlite::ffi::SQLITE_CANTOPEN),
            (ErrorCode::ReadOnly, rusqlite::ffi::SQLITE_READONLY),
            (ErrorCode::DiskFull, rusqlite::ffi::SQLITE_FULL),
            (
                ErrorCode::FileLockingProtocolFailed,
                rusqlite::ffi::SQLITE_PROTOCOL,
            ),
            (
                ErrorCode::OperationInterrupted,
                rusqlite::ffi::SQLITE_INTERRUPT,
            ),
        ] {
            let env = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code,
                    extended_code: raw,
                },
                Some("environment problem".to_string()),
            );
            assert!(
                !sqlite_error_indicates_corruption(&env),
                "{code:?} is an environment failure, not corruption"
            );
        }

        // Plain errors never qualify.
        assert!(!sqlite_error_indicates_corruption(
            &rusqlite::Error::QueryReturnedNoRows
        ));
    }

    #[test]
    fn backup_names_are_adjacent_to_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("coresight.db");
        assert_eq!(
            with_suffix(&store, BACKUP_SUFFIX),
            dir.path().join("coresight.db.pre-migration-backup")
        );
        assert_eq!(
            backup_temporary(&store),
            dir.path().join("coresight.db.backup-in-progress")
        );
        assert_eq!(
            quarantine_directory(&store),
            dir.path().join("coresight.db.quarantined")
        );
    }

    /// Two backup attempts running CONCURRENTLY interleave so that both
    /// own live temporary artifacts at the same moment; each attempt must
    /// use, and clean up, only its OWN names.
    ///
    /// The interleaving is orchestrated with channels (never sleeps):
    ///
    /// 1. A reserves its temporary and announces it, then waits until B
    ///    has verified ITS temporary.
    /// 2. B reserves, copies, verifies, announces that, and waits until A
    ///    has failed and cleaned up.
    /// 3. A fails at the publish stage (injected), removing exactly its own
    ///    two names; B then publishes its verified backup.
    ///
    /// Under the pre-Phase-6.5.1 scheme both attempts would have used the
    /// SAME temporary path and the second would have deleted the first's
    /// artifact mid-flight, so this test would fail the path-inequality
    /// assertion — and the file assertions could flake by timing. With
    /// per-attempt reservation both artifacts coexist deterministically.
    #[test]
    fn concurrent_backup_attempts_never_share_or_delete_each_others_artifacts() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        /// A channel wait that only expires on a deadlock bug: ordering is
        /// event-driven, so a timeout can never be part of a passing run.
        const GUARD: Duration = Duration::from_secs(60);

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version VALUES (2);
                 CREATE TABLE keeper(marker TEXT NOT NULL);
                 INSERT INTO keeper VALUES ('legacy-row');",
            )
            .unwrap();
        }
        let src_a = Connection::open(&db).unwrap();
        let src_b = Connection::open(&db).unwrap();

        let (b_verified_tx, b_verified_rx) = mpsc::channel::<()>();
        let (a_failed_tx, a_failed_rx) = mpsc::channel::<()>();

        let db_a = db.clone();
        let attempt_a = thread::spawn(move || {
            let mut own_temp = None;
            let result = verified_pre_migration_backup_observed(
                &src_a,
                &db_a,
                Some(2),
                // Fails at PUBLISH: copy and verification both complete, so
                // a real temporary artifact is live when this attempt fails.
                Hazard::BackupPublish,
                &mut |stage| match stage {
                    BackupStage::TemporaryReserved(path) => {
                        own_temp = Some(path.to_path_buf());
                        b_verified_rx.recv_timeout(GUARD).expect(
                            "ordering deadlock: B never verified its temporary while A waited",
                        );
                    }
                    BackupStage::TemporaryVerified(_) => {}
                },
            );
            let _ = a_failed_tx.send(());
            (result, own_temp)
        });

        let db_b = db.clone();
        let attempt_b = thread::spawn(move || {
            let mut own_temp = None;
            let result = verified_pre_migration_backup_observed(
                &src_b,
                &db_b,
                Some(2),
                Hazard::None,
                &mut |stage| match stage {
                    BackupStage::TemporaryReserved(path) => {
                        own_temp = Some(path.to_path_buf());
                    }
                    BackupStage::TemporaryVerified(path) => {
                        // Tell A its isolation premise holds (B holds a
                        // verified artifact too), then wait for A's failure
                        // and cleanup before publishing.
                        assert_eq!(path, own_temp.as_deref().unwrap());
                        b_verified_tx.send(()).unwrap();
                        a_failed_rx
                            .recv_timeout(GUARD)
                            .expect("ordering deadlock: A never failed while B held its temporary");
                    }
                },
            );
            (result, own_temp)
        });

        let (a_result, a_temp) = attempt_a.join().expect("attempt A must not panic");
        let (b_result, b_temp) = attempt_b.join().expect("attempt B must not panic");
        let a_temp = a_temp.expect("A must have reserved a temporary");
        let b_temp = b_temp.expect("B must have reserved a temporary");

        // Isolation: the two attempts never shared their temporary.
        assert_ne!(
            a_temp, b_temp,
            "two attempts must never share one temporary artifact"
        );

        // A failed at publish and cleaned ONLY its own names...
        match a_result {
            Err(StoreError::BackupFailed { ref step, .. }) => {
                assert_eq!(step, "publish", "A's injected failure is the publish stage")
            }
            other => panic!("A must fail at publish, got {other:?}"),
        }
        assert!(
            !a_temp.exists(),
            "A's failed attempt must remove its OWN temporary"
        );

        // ...while B's artifact was untouched by A's failure and published.
        let published = b_result.expect("B must publish while A's temporary is alive/cleanup runs");
        assert!(
            published
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("store.db.pre-migration-backup"),
            "B publishes under its reserved BACKUP name, never the temporary name: {published:?}"
        );
        assert_ne!(
            published, b_temp,
            "the published destination is the reserved name, not the temporary path"
        );
        assert!(
            !b_temp.exists(),
            "B's temporary is gone because it was RENAMED, not deleted"
        );
        assert!(published.exists());

        // Exactly one published backup exists (B's); A published nothing.
        let published_count = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("store.db.pre-migration-backup")
            })
            .count();
        assert_eq!(
            published_count, 1,
            "exactly one attempt published a backup; the failed one published nothing"
        );

        // The published artifact is a real, readable backup of the source.
        let b = Connection::open(&published).unwrap();
        let version: u32 = b
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2, "the published backup keeps the source version");
        let rows: i64 = b
            .query_row(
                "SELECT COUNT(*) FROM keeper WHERE marker = 'legacy-row'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "the published backup carries the source rows");
    }

    /// A stale temporary and a stale reserved name left by a crashed
    /// attempt are neither adopted as the new attempt's destination nor
    /// deleted: the new attempt advances to fresh names and publishes a
    /// real backup under its own.
    #[test]
    fn a_stale_backup_artifact_is_neither_adopted_nor_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version VALUES (2);",
            )
            .unwrap();
        }

        let stale_temporary = backup_temporary(&db);
        let stale_reserved = with_suffix(&db, BACKUP_SUFFIX);
        fs::write(
            &stale_temporary,
            b"half-written artifact from a crashed attempt",
        )
        .unwrap();
        File::create(&stale_reserved).unwrap();
        let stale_bytes = fs::read(&stale_temporary).unwrap();

        let conn = Connection::open(&db).unwrap();
        let published = verified_pre_migration_backup(&conn, &db, Some(2), Hazard::None)
            .expect("a fresh attempt must succeed even with stale artifacts present");

        // The new attempt used its OWN names, advancing past both stale
        // reservations...
        assert_eq!(
            published.file_name().unwrap().to_string_lossy(),
            "store.db.pre-migration-backup.1"
        );
        assert_eq!(
            backup_temporary(&db).file_name().unwrap().to_string_lossy(),
            "store.db.backup-in-progress"
        );

        // ...and touched nothing it did not create.
        assert!(
            stale_temporary.exists(),
            "a stale temporary must never be deleted by a later attempt"
        );
        assert_eq!(
            fs::read(&stale_temporary).unwrap(),
            stale_bytes,
            "a stale temporary must be byte-identical after a later attempt"
        );
        assert!(
            stale_reserved.exists(),
            "a stale reserved name must never be adopted or deleted"
        );
        assert_eq!(fs::metadata(&stale_reserved).unwrap().len(), 0);

        // The published artifact is a real, readable backup.
        let b = Connection::open(&published).unwrap();
        let version: u32 = b
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    /// `recorded_version` reports the three cases apart, and never coerces
    /// unusable metadata into a version.
    #[test]
    fn recorded_version_reports_missing_unusable_and_recorded_apart() {
        // No table at all.
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(recorded_version(&conn).unwrap(), RecordedVersion::NoTable);

        // Exactly one integer row.
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (6);",
        )
        .unwrap();
        assert_eq!(
            recorded_version(&conn).unwrap(),
            RecordedVersion::Version(6)
        );

        // Present but empty.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_version (version INTEGER NOT NULL);")
            .unwrap();
        assert_unusable(&conn, "no version row");

        // More than one row.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (1);
             INSERT INTO schema_version VALUES (2);",
        )
        .unwrap();
        assert_unusable(&conn, "2 version rows");

        // A non-integer value.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version TEXT NOT NULL);
             INSERT INTO schema_version VALUES ('six');",
        )
        .unwrap();
        assert_unusable(&conn, "not an integer");

        // Outside the u32 domain.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (4294967296);",
        )
        .unwrap();
        assert_unusable(&conn, "outside the valid");

        // A table without a version column.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (v INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (1);",
        )
        .unwrap();
        assert_unusable(&conn, "no version column");
    }

    fn assert_unusable(conn: &Connection, needle: &str) {
        match recorded_version(conn).unwrap() {
            RecordedVersion::Unusable(detail) => assert!(
                detail.contains(needle),
                "the unusable-version report must say what was found ({needle:?}): {detail}"
            ),
            other => panic!("expected Unusable containing {needle:?}, got {other:?}"),
        }
    }

    /// The minimum structure the recognition contract requires up to
    /// `version` — only the required tables and signature columns.
    fn minimal_store_at(version: u32) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let mut ddl = String::from(
            "CREATE TABLE drives (id TEXT);
             CREATE TABLE scans (id TEXT, drive_id TEXT);
             CREATE TABLE scan_runs (run_id TEXT, started_at INTEGER, roots TEXT,
                                     platform TEXT, config TEXT, status TEXT);
             CREATE TABLE observations (run_id TEXT, path TEXT, kind TEXT);
             CREATE TABLE relationship_obs (run_id TEXT, rel_id TEXT, kind TEXT);
             CREATE TABLE relationship_members (run_id TEXT, rel_id TEXT, path TEXT);",
        );
        if version >= 3 {
            ddl.push_str(
                "ALTER TABLE observations ADD COLUMN file_id_hi INTEGER;
                 ALTER TABLE relationship_members ADD COLUMN file_id_hi INTEGER;",
            );
        }
        if version >= 4 {
            ddl.push_str(
                "ALTER TABLE scan_runs ADD COLUMN rel_status TEXT;
                 ALTER TABLE scan_runs ADD COLUMN rel_truncated INTEGER;",
            );
        }
        if version >= 5 {
            for table in V5_TABLES {
                ddl.push_str(&format!("CREATE TABLE {table} (x INTEGER);"));
            }
        }
        if version >= 6 {
            ddl.push_str("ALTER TABLE app_snapshot_apps ADD COLUMN id_encoding TEXT;");
        }
        conn.execute_batch(&ddl).unwrap();
        conn
    }

    /// The recognition contract is the FULL v1–v6 structure — every table
    /// AND signature column the migrations guarantee — following the
    /// recorded version.
    #[test]
    fn recognition_gaps_follow_the_recorded_version() {
        for version in 1..=6 {
            let conn = minimal_store_at(version);
            assert!(
                recognition_gaps(&conn, version).unwrap().is_empty(),
                "a minimally-complete v{version} store must be recognized"
            );
        }

        // A v2 store missing the v1 bootstrap and half the v2 tables (the
        // adversarial shape: scan_runs + observations only) names every
        // missing piece.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE scan_runs (run_id TEXT, started_at INTEGER, roots TEXT,
                                     platform TEXT, config TEXT, status TEXT);
             CREATE TABLE observations (run_id TEXT, path TEXT, kind TEXT);",
        )
        .unwrap();
        let gaps = recognition_gaps(&conn, 2).unwrap();
        for expected in [
            "missing table drives",
            "missing table scans",
            "missing table relationship_obs",
            "missing table relationship_members",
        ] {
            assert!(
                gaps.contains(&expected.to_string()),
                "expected {expected:?} in {gaps:?}"
            );
        }

        // A v5-shaped store recording v6 is incomplete: the v6 column is
        // absent. Structure, not provenance: having every table is not
        // enough when the recorded version promises the column.
        let conn = minimal_store_at(5);
        assert_eq!(
            recognition_gaps(&conn, 6).unwrap(),
            vec!["missing column app_snapshot_apps.id_encoding".to_string()]
        );

        // A v2-shaped store recording v3 lacks the widened-identity
        // columns.
        let conn = minimal_store_at(2);
        assert_eq!(
            recognition_gaps(&conn, 3).unwrap(),
            vec![
                "missing column observations.file_id_hi".to_string(),
                "missing column relationship_members.file_id_hi".to_string(),
            ]
        );
    }

    /// (Phase 6.5.2, Workstream B) The classifier fails closed on an empty
    /// result; only a non-empty all-`ok` result is healthy; any other row
    /// is a problem.
    #[test]
    fn integrity_classification_fails_closed_on_an_empty_result() {
        let err = classify_integrity_rows(Vec::new())
            .expect_err("an empty result established nothing and must not mean healthy");
        match &err {
            StoreError::Sqlite(sql) => {
                assert!(
                    !sqlite_error_indicates_corruption(sql),
                    "an empty result is not evidence of damage either: {err:?}"
                );
            }
            other => panic!("expected a Sqlite error, got {other:?}"),
        }
        assert!(
            err.to_string()
                .contains("integrity could not be established"),
            "the failure must say what could not be established: {err}"
        );

        // The real pragma on a real (empty) database answers `ok` — the
        // empty-result rule is about an outcome we cannot rule out, not
        // about a pragma that normally misbehaves.
        let conn = Connection::open_in_memory().unwrap();
        assert!(
            integrity_problems(&conn).unwrap().is_empty(),
            "an empty database's integrity check answers ok"
        );

        assert!(classify_integrity_rows(vec!["ok".to_string()])
            .unwrap()
            .is_empty());
        assert_eq!(
            classify_integrity_rows(vec![
                "ok".to_string(),
                "row 3 missing from index idx_obs_path".to_string(),
            ])
            .unwrap(),
            vec!["row 3 missing from index idx_obs_path".to_string()]
        );
    }
}
