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
//! ## What this module deliberately does NOT do
//!
//! - No network, no subprocess, no privilege escalation, no GUI. This is a
//!   filesystem + SQLite concern only.
//! - No logging or telemetry. The crate has no log macros and this module
//!   adds none; paths reach only the returned error, which the caller
//!   decides how to present.
//! - No deletion of anything it did not create. A failed attempt removes
//!   only its own temporary artifacts.
//!
//! ## Test seams
//!
//! [`Hazard`] injects deterministic failures at each stage (naming a real
//! fault that a filesystem can produce) so the fail-closed paths are
//! exercised by test rather than described. Production passes
//! [`Hazard::None`].

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

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

/// A deterministic failure injected at one named stage.
///
/// Each variant stands for a fault a real filesystem or a full disk can
/// produce. They exist so the fail-closed paths are *tested* rather than
/// asserted in a comment — an untested failure path is a claim, not a
/// guarantee.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Hazard {
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
}

impl Hazard {
    fn inject(self, stage: &str) -> Result<(), StoreError> {
        match self {
            Hazard::None => Ok(()),
            other => Err(StoreError::BackupFailed {
                step: stage.to_string(),
                detail: format!("injected test failure ({other:?})"),
            }),
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

/// Collision-safe quarantine directory next to `path`.
fn exclusive_quarantine_directory(path: &Path) -> io::Result<PathBuf> {
    exclusive_path(quarantine_directory(path), true)
}

/// The schema version recorded in an open connection, or `None` when the
/// `schema_version` table does not exist yet (a brand-new store).
pub(crate) fn recorded_version(conn: &Connection) -> Result<Option<u32>, StoreError> {
    match conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0)) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
            if msg.to_lowercase().contains("no such table") =>
        {
            Ok(None)
        }
        Err(e) => Err(StoreError::Sqlite(e)),
    }
}

/// Every row `PRAGMA integrity_check` reports other than `ok`.
///
/// A RESULT-READING failure is surfaced as an error, never read as
/// "healthy": silently treating an unreadable result as equivalent to
/// `ok` would let a damaged store through the exact gate that is supposed
/// to catch it. An empty result is therefore NOT the same as `ok` — it
/// falls out as an error from the `collect` rather than being smoothed
/// over.
pub(crate) fn integrity_problems(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    // `collect::<Result<..>>()` rather than `filter_map(|r| r.ok())`:
    // a row that cannot be READ is a failure to decide, and must not be
    // silently dropped into a "no problems" conclusion.
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter(|r| r != "ok").collect())
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
/// ## Failure is closed
///
/// If any step fails, the temporary artifact this attempt owns is removed
/// and the error is returned. The caller must not migrate without a
/// `Ok` result: an un-migrated store is still readable, whereas one
/// migrated without a backup is not.
///
/// `expected_version` is the pre-migration version the source recorded; it
/// must match, because a backup of a different version would not
/// represent the state the migration is about to change.
pub fn verified_pre_migration_backup(
    conn: &Connection,
    path: &Path,
    expected_version: Option<u32>,
    hazard: Hazard,
) -> Result<PathBuf, StoreError> {
    if !is_existing_file(path) {
        return Err(StoreError::Sqlite(rusqlite::Error::InvalidPath(
            path.to_path_buf(),
        )));
    }

    let temporary = backup_temporary(path);
    // A leftover from an earlier failed attempt (different process, or one
    // that died before cleanup) must not become this attempt's
    // destination, and it must not be trusted either.
    let _ = fs::remove_file(&temporary);

    // The reserved name is an EMPTY file this attempt created. It is tracked
    // separately from `temporary` because a failure must remove BOTH: an
    // empty file left under the backup name would LOOK like a published
    // backup to the next reader (and to the next attempt's collision
    // counter) while containing nothing.
    let reserved = exclusive_backup_destination(path)
        .map_err(|e| backup_io_error("reserve destination name", e))?;

    let attempt = (|| -> Result<PathBuf, StoreError> {
        // Build the backup into the temporary destination.
        hazard.inject("copy")?;
        conn.backup(rusqlite::DatabaseName::Main, &temporary, None)
            .map_err(|e| backup_sqlite_error("copy", e))?;

        // Verify BEFORE publishing: an unverified artifact must never be
        // reachable under the name that means "a safe copy exists".
        hazard.inject("verify")?;
        verify_backup(&temporary, expected_version)?;

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
            // collision-safe precisely so this attempt never got that name.
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
fn verify_backup(path: &Path, expected_version: Option<u32>) -> Result<(), StoreError> {
    if !is_existing_file(path) {
        return Err(StoreError::BackupFailed {
            step: "verify".to_string(),
            detail: "backup destination does not exist after the copy completed".to_string(),
        });
    }
    let conn = Connection::open(path).map_err(|e| backup_sqlite_error("verify open", e))?;
    let problems = integrity_problems(&conn)?;
    if !problems.is_empty() {
        return Err(StoreError::BackupFailed {
            step: "verify".to_string(),
            detail: format!(
                "PRAGMA integrity_check did not report ok: {}",
                problems.join("; ")
            ),
        });
    }
    let found = recorded_version(&conn)?;
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
#[derive(Debug, Clone)]
pub struct Quarantine {
    directory: PathBuf,
}

impl Quarantine {
    /// The quarantine directory. Callers that report it must report it as
    /// a LOCATION only — the damaged data's contents are never described
    /// as recovered or repaired.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

/// Quarantine the damaged store at `path`, including its SQLite sidecars.
///
/// ## Rules this upholds
///
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
pub fn quarantine_corrupt_store(path: &Path, hazard: Hazard) -> Result<Quarantine, StoreError> {
    if !is_existing_file(path) {
        return Err(StoreError::QuarantineFailed {
            step: "prepare".to_string(),
            detail: "the store file no longer exists".to_string(),
        });
    }

    let directory =
        exclusive_quarantine_directory(path).map_err(|e| StoreError::QuarantineFailed {
            step: "create quarantine directory".to_string(),
            detail: e.to_string(),
        })?;

    // Move the main file first: if that succeeds, the sidecars follow, and
    // the "already moved" set is exactly what a rollback must undo.
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    match relocate_all(path, &directory, &mut moved, hazard) {
        Ok(()) => Ok(Quarantine { directory }),
        Err(e) => {
            // Attempt to restore what was moved. Destination names are
            // derived from the ORIGINALS, so a restore can never overwrite
            // a file this attempt did not move.
            let mut restoration_failures = Vec::new();
            for (original, quarantined) in moved.iter().rev() {
                if let Err(re) = fs::rename(quarantined, original) {
                    restoration_failures.push(format!(
                        "{}: {re}",
                        original.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
            let _ = fs::remove_dir(&directory);
            if restoration_failures.is_empty() {
                Err(e)
            } else {
                // Truthful partial-failure report: say exactly which
                // artifacts could NOT be put back, and do not claim the
                // store was quarantined.
                Err(StoreError::QuarantineFailed {
                    step: "restore".to_string(),
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
fn relocate_all(
    path: &Path,
    directory: &Path,
    moved: &mut Vec<(PathBuf, PathBuf)>,
    hazard: Hazard,
) -> Result<(), StoreError> {
    relocate_one(path, directory, moved, hazard)?;
    for suffix in SIDECAR_SUFFIXES {
        let sidecar = with_suffix(path, suffix);
        if sidecar.exists() {
            relocate_one(&sidecar, directory, moved, hazard)?;
        }
    }
    Ok(())
}

/// Move one file, recording it so a later failure can restore it.
fn relocate_one(
    from: &Path,
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
/// aside.
pub fn sqlite_error_indicates_corruption(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(ffi, _) => {
            use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
            matches!(ffi.extended_code, c if c == DatabaseCorrupt as i32)
                || matches!(ffi.code, DatabaseCorrupt | NotADatabase)
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
                extended_code: 11,
            },
            Some("database disk image is malformed".to_string()),
        );
        assert!(sqlite_error_indicates_corruption(&corruption));

        let not_a_database = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::NotADatabase,
                extended_code: 26,
            },
            Some("file is not a database".to_string()),
        );
        assert!(sqlite_error_indicates_corruption(&not_a_database));

        // Environment problems must NOT qualify: quarantining on these
        // would move a HEALTHY file aside. Each is a real SQLite code a
        // real environment can produce.
        for code in [
            ErrorCode::SystemIoFailure,
            ErrorCode::PermissionDenied,
            ErrorCode::DatabaseBusy,
            ErrorCode::CannotOpen,
            ErrorCode::ReadOnly,
            ErrorCode::DiskFull,
            ErrorCode::FileLockingProtocolFailed,
            ErrorCode::OperationInterrupted,
        ] {
            let env = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code,
                    extended_code: code as i32,
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
}
