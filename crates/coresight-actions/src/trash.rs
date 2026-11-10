//! Trash and quarantine relocation primitives.
//!
//! Everything this module does is one of three things: create a directory,
//! reserve a destination name, or rename an entry into it. It never deletes,
//! never truncates, never copies, and never follows a symlink out of the tree
//! it was handed.
//!
//! ## Never overwrite
//!
//! A destination name is *proven* free by creating it exclusively:
//! [`std::fs::OpenOptions::create_new`] for a file, [`std::fs::create_dir`]
//! for a directory. Only a name that creation proves absent is ever handed to
//! [`std::fs::rename`], and a taken name advances the counter
//! (`name (1).ext`, `name (2).ext`, …) up to [`MAX_DESTINATION_ATTEMPTS`]
//! before the move gives up with [`MoveError::NoFreeName`]. A pre-existing
//! file at the destination is therefore never replaced.
//!
//! ### The reservation window (honest limitation)
//!
//! `create_new` proves the name was free at the instant it ran; it cannot
//! hold the name until `rename` takes it. The zero-byte placeholder this
//! module creates for a file is replaced by the rename itself, so a
//! successful move leaves no trace of it. Two residual facts remain:
//!
//! * For a **file**, if the rename fails, the empty placeholder stays behind
//!   at a name that did not exist before. It holds no data and touches no
//!   user file, but it is litter and the caller may want to report it.
//! * For a **directory**, the empty placeholder is removed before the rename
//!   (Windows cannot rename a directory over an existing directory), so the
//!   window in which the name is free is one syscall wider.
//!
//! A concurrent process that targets the exact fresh name inside that window
//! can have its entry replaced on Unix, where `rename` overwrites. Two
//! CoreSight processes cannot collide with each other: neither is ever handed
//! a name the other created, and both derive names deterministically. Closing
//! the window outright needs `renameat2(RENAME_NOREPLACE)`, which is not
//! portable to Windows or macOS.
//!
//! ## Sidecars
//!
//! When a target has a sibling `<name>-wal` or `<name>-shm`, the sidecar is
//! moved into the same destination directory under the **same destination
//! file name plus suffix** (`data (1).db` → `data (1).db-wal`). A sidecar
//! destination is *not* collision-suffixed: a `-wal` that belongs to one
//! database must keep that database's name, so a taken sidecar name fails
//! closed with [`MoveError::SidecarBlocked`] and the whole operation —
//! including the already-moved main file — is rolled back.
//!
//! ## Straight renaming, no copy fallback
//!
//! [`std::fs::rename`] cannot cross filesystems. A trash directory on another
//! volume therefore makes the move fail and roll back. That is deliberate:
//! copy-then-delete would create a window in which the data exists twice and
//! a failure could destroy the only good copy.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rollback::{restore_all, RollbackFailure};

/// How many destination names one move will try before giving up.
///
/// A bound exists so a directory full of colliding names can never turn into
/// an unbounded loop. Ten thousand is far beyond any plausible trash
/// directory and small enough to fail in bounded time.
pub const MAX_DESTINATION_ATTEMPTS: usize = 10_000;

/// SQLite sidecars that must travel with their database.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Where a moved entry should go.
///
/// The trait exists for two reasons:
///
/// * **Platform honesty.** There is no portable "OS trash" API, so the
///   destination is injected instead of guessed. Production wires the real
///   Finder/Explorer integration here; [`DirectoryTrash`] is the portable,
///   testable fallback.
/// * **Testability.** Tests supply a directory they own (a fresh
///   `tempfile::tempdir()`), so no test can ever touch a real home directory.
///
/// An implementation returns the *preferred* destination. Collisions are
/// resolved by the caller (`name (1).ext`, …), never by the resolver, so a
/// resolver must not pre-check existence and must not invent a different name
/// per call. It is responsible for creating the destination's parent
/// directory when absent — [`DirectoryTrash`] does.
pub trait TrashDestination {
    /// The preferred destination for `source`.
    fn destination_for(&self, source: &Path) -> io::Result<PathBuf>;
}

/// The always-correct-to-test implementation: a plain directory.
///
/// This is **not** the OS Trash. It moves entries into `root`, creating the
/// directory if it does not exist, and nothing about the result is special:
/// the files are ordinary files in an ordinary folder. See
/// [`default_trash_directory`] for the production caveat.
///
/// Quarantine needs no second implementation: a quarantine root is the same
/// thing under a different name, so [`crate::execute`] is handed
/// `DirectoryTrash::new(quarantine_root)` for app-quarantine moves and
/// [`move_to_quarantine`] does the same for a single target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryTrash {
    root: PathBuf,
}

impl DirectoryTrash {
    /// A trash directory at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DirectoryTrash { root: root.into() }
    }

    /// The directory entries are moved into.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl TrashDestination for DirectoryTrash {
    fn destination_for(&self, source: &Path) -> io::Result<PathBuf> {
        let name = source.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the source path has no file name",
            )
        })?;
        fs::create_dir_all(&self.root)?;
        Ok(self.root.join(name))
    }
}

/// The directory-based trash fallback for this platform.
///
/// * macOS / Linux: `~/.Trash`
/// * Windows: `%USERPROFILE%\Recycle Bin`
/// * No home directory is known: `<system temp>/coresight-trash`
///
/// # This is NOT the operating system's Trash
///
/// A file moved here is **not** in the Finder's or Explorer's Trash: it will
/// not appear in "Empty Trash", the OS will not purge it, and no OS undo UI
/// knows about it. It is a plain file in a plain directory and it still
/// occupies disk space. The production path is the platform trash API
/// (Finder on macOS, `IFileOperation` / the shell Recycle Bin on Windows);
/// this function exists so the executor has a portable, honest, testable
/// destination when that integration is not wired in. Callers that present
/// this path to a user must say so in those words.
///
/// Linux has a freedesktop.org Trash specification that uses
/// `~/.local/share/Trash/files` with its own metadata; this fallback does not
/// implement it, which is one more reason to treat it as a placeholder for a
/// real platform adapter rather than as a trash implementation.
pub fn default_trash_directory() -> PathBuf {
    let leaf = if cfg!(windows) {
        "Recycle Bin"
    } else {
        ".Trash"
    };
    match home_directory() {
        Some(home) => home.join(leaf),
        None => std::env::temp_dir().join("coresight-trash"),
    }
}

/// `%USERPROFILE%` on Windows, `$HOME` elsewhere, when set and non-empty.
fn home_directory() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    match std::env::var_os(variable) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}

/// One entry that was really moved, and the size observed before the move.
///
/// `bytes` is measured at the **source** immediately before the rename (for a
/// directory: the recursive sum of its regular files, best effort). It is
/// what the caller may call "moved". [`crate::verify()`] re-measures the
/// destination and only its verdict may be presented as verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedFile {
    /// The path the entry came from. After a successful move nothing exists
    /// here (unless a rollback put it back).
    pub source: PathBuf,
    /// The path the entry now occupies.
    pub destination: PathBuf,
    /// Size observed at `source` immediately before the move.
    pub bytes: u64,
}

/// Why a move (or the rollback that followed it) failed.
#[derive(Debug)]
pub enum MoveError {
    /// The source, or one of its sidecars, could not be inspected. Nothing
    /// was moved by the step that produced this error.
    SourceUnavailable {
        /// The entry that could not be read.
        source: PathBuf,
        /// The underlying I/O description.
        detail: String,
    },
    /// The source path has no file name, so it cannot be given a destination
    /// name.
    NoFileName {
        /// The offending source path.
        source: PathBuf,
    },
    /// A destination directory or name could not be created.
    DestinationUnavailable {
        /// The destination that failed.
        destination: PathBuf,
        /// The underlying I/O description.
        detail: String,
    },
    /// The [`TrashDestination`] resolver refused to name a destination.
    DestinationUnresolved {
        /// The source the resolver was asked about.
        source: PathBuf,
        /// The resolver's description.
        detail: String,
    },
    /// Every candidate name was taken, up to [`MAX_DESTINATION_ATTEMPTS`].
    NoFreeName {
        /// The source that could not be placed.
        source: PathBuf,
        /// How many names were tried.
        attempts: usize,
    },
    /// A sidecar's required destination name (same name plus suffix) is
    /// already occupied. Sidecars keep their name, so no alternative is
    /// derived and the operation must roll back.
    SidecarBlocked {
        /// The sidecar that could not be placed.
        sidecar: PathBuf,
        /// The occupied destination name.
        destination: PathBuf,
        /// The underlying I/O description, when there was one.
        detail: String,
    },
    /// The rename itself failed. Nothing was moved by this step; anything the
    /// caller moved earlier is restored by the caller.
    Move {
        /// Where the entry was.
        from: PathBuf,
        /// Where it was going.
        to: PathBuf,
        /// The underlying I/O description.
        detail: String,
    },
    /// The move failed **and** at least one already-moved entry could not be
    /// put back. Never returned for a clean failure: this variant exists so a
    /// partial restore can never be disguised as an ordinary error.
    RollbackIncomplete {
        /// The target whose move failed.
        failed: PathBuf,
        /// The original failure's description.
        detail: String,
        /// Every entry that is still not at its source path.
        unrestored: Vec<RollbackFailure>,
    },
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveError::SourceUnavailable { source, detail } => {
                write!(f, "cannot read {}: {detail}", source.display())
            }
            MoveError::NoFileName { source } => {
                write!(f, "{} has no file name to move", source.display())
            }
            MoveError::DestinationUnavailable {
                destination,
                detail,
            } => write!(
                f,
                "cannot prepare destination {}: {detail}",
                destination.display()
            ),
            MoveError::DestinationUnresolved { source, detail } => write!(
                f,
                "no destination could be resolved for {}: {detail}",
                source.display()
            ),
            MoveError::NoFreeName { source, attempts } => write!(
                f,
                "no free destination name for {} after {attempts} attempts",
                source.display()
            ),
            MoveError::SidecarBlocked {
                sidecar,
                destination,
                detail,
            } => write!(
                f,
                "sidecar {} cannot use its required destination {}: {detail}",
                sidecar.display(),
                destination.display()
            ),
            MoveError::Move { from, to, detail } => write!(
                f,
                "could not move {} to {}: {detail}",
                from.display(),
                to.display()
            ),
            MoveError::RollbackIncomplete {
                failed,
                detail,
                unrestored,
            } => {
                write!(
                    f,
                    "moving {} failed ({detail}) and {} already-moved entr{} could not be \
                     restored: ",
                    failed.display(),
                    unrestored.len(),
                    if unrestored.len() == 1 { "y" } else { "ies" }
                )?;
                for failure in unrestored {
                    write!(
                        f,
                        "{} is at {} ({}); ",
                        failure.original.display(),
                        failure.quarantined.display(),
                        failure.detail
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for MoveError {}

/// Move `path` into the platform's directory-based trash fallback.
///
/// Thin wrapper over [`move_to_trash_with`] using
/// `DirectoryTrash::new(default_trash_directory())`. Read
/// [`default_trash_directory`] before presenting this to a user: it is a
/// directory, not the OS Recycle Bin.
pub fn move_to_trash(path: &Path) -> Result<MovedFile, MoveError> {
    move_to_trash_with(path, &DirectoryTrash::new(default_trash_directory()))
}

/// Move `path` (and its SQLite sidecars) to the destination the resolver
/// names.
///
/// On any failure the entries already moved by this call are restored, newest
/// first. If a restore fails the returned error is
/// [`MoveError::RollbackIncomplete`], which names exactly what stayed where —
/// an ordinary [`MoveError::Move`] is returned only when everything is back.
///
/// The returned [`MovedFile`] describes the **main** entry. Any `-wal` /
/// `-shm` sidecar that moved with it sits in the same directory at
/// `<destination file name>` plus its suffix; [`crate::execute()`] reports each
/// sidecar as its own [`MovedFile`] because it accumulates every pair for
/// rollback and verification.
pub fn move_to_trash_with(
    path: &Path,
    destination: &dyn TrashDestination,
) -> Result<MovedFile, MoveError> {
    ensure_source_exists(path)?;
    let preferred =
        destination
            .destination_for(path)
            .map_err(|e| MoveError::DestinationUnresolved {
                source: path.to_path_buf(),
                detail: e.to_string(),
            })?;
    let (directory, name) =
        destination_parts(&preferred).map_err(|detail| MoveError::DestinationUnavailable {
            destination: preferred.clone(),
            detail,
        })?;
    relocate_and_roll_back(path, &directory, &name)
}

/// Move `path` (and its SQLite sidecars) into `quarantine_root`, creating the
/// directory if absent.
///
/// Quarantine is a directory for exactly the same reason as trash: it is a
/// reversible relocation, not a deletion. Nothing about the moved entry is
/// interpreted, repaired, or claimed to be recovered.
pub fn move_to_quarantine(path: &Path, quarantine_root: &Path) -> Result<MovedFile, MoveError> {
    ensure_source_exists(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| MoveError::NoFileName {
            source: path.to_path_buf(),
        })?
        .to_os_string();
    fs::create_dir_all(quarantine_root).map_err(|e| MoveError::DestinationUnavailable {
        destination: quarantine_root.to_path_buf(),
        detail: e.to_string(),
    })?;
    relocate_and_roll_back(path, quarantine_root, &name)
}

/// Split a resolver's preferred path into (directory, file name).
///
/// A bare file name (`"x.txt"`, no parent) is placed in the current directory
/// rather than rejected, because that is what [`Path`] means by it.
pub(crate) fn destination_parts(target: &Path) -> Result<(PathBuf, OsString), String> {
    let name = target
        .file_name()
        .ok_or_else(|| "the destination has no file name".to_string())?;
    let directory = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Ok((directory, name.to_os_string()))
}

/// Fail before touching anything when the source cannot be inspected.
///
/// Both public move functions call this *before* they ask a resolver for a
/// destination, so a move that cannot start — a mistyped path, a file already
/// gone — creates no trash directory and changes nothing at all.
pub(crate) fn ensure_source_exists(path: &Path) -> Result<(), MoveError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(()),
        Err(e) => Err(MoveError::SourceUnavailable {
            source: path.to_path_buf(),
            detail: e.to_string(),
        }),
    }
}

/// The bytes of an entry: a file's length, or a directory's recursive total.
///
/// The directory walk is best effort and never follows symlinks: an
/// unreadable subtree contributes nothing, so the result is a LOWER BOUND for
/// a directory. That is the honest direction for size accounting — it can
/// under-report, never invent bytes.
pub(crate) fn measure_entry(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        Ok(directory_bytes(path))
    } else {
        Ok(metadata.len())
    }
}

/// Recursive sum of regular-file lengths under `root` (best effort).
fn directory_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_dir() => stack.push(path),
                Ok(metadata) => total = total.saturating_add(metadata.len()),
                Err(_) => {}
            }
        }
    }
    total
}

/// Append `suffix` to the whole file name (`data.db` → `data.db-wal`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// `name` for attempt 0, otherwise `stem (n).ext`.
///
/// `Path::file_stem`/`extension` split at the last dot, so `a.tar.gz` becomes
/// `a.tar (1).gz` — the counter goes where a human would put it, next to the
/// extension that decides what the file is.
fn candidate_name(name: &OsStr, attempt: usize) -> OsString {
    if attempt == 0 {
        return name.to_os_string();
    }
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or(name);
    let mut candidate = OsString::from(stem);
    candidate.push(format!(" ({attempt})"));
    if let Some(extension) = path.extension() {
        candidate.push(".");
        candidate.push(extension);
    }
    candidate
}

/// Reserve one name, or report that it is taken.
///
/// `Ok(true)` means the name is free *and* was just created by this call: for
/// a file a zero-byte placeholder now sits there (the rename replaces it), for
/// a directory the placeholder was created and removed so the rename has an
/// empty slot. `Ok(false)` means the name was taken (or vanished under us) and
/// the caller should try the next one. `Err` is a real failure: permission
/// denied, invalid name, missing parent.
fn try_reserve(candidate: &Path, is_directory: bool) -> io::Result<bool> {
    if is_directory {
        match fs::create_dir(candidate) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => return Err(e),
        }
        match fs::remove_dir(candidate) {
            Ok(()) => Ok(true),
            // Something was placed inside between creation and removal: the
            // name is not ours to use, so advance to the next candidate.
            Err(_) => Ok(false),
        }
    } else {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(candidate)
        {
            Ok(placeholder) => {
                // Close before the rename: Windows refuses to replace a file
                // this process still holds open without FILE_SHARE_DELETE,
                // and the rename is what must own the name from here on.
                drop(placeholder);
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// A collision-safe destination name inside `directory`.
fn reserve_destination(
    source: &Path,
    directory: &Path,
    name: &OsStr,
    is_directory: bool,
) -> Result<PathBuf, MoveError> {
    for attempt in 0..MAX_DESTINATION_ATTEMPTS {
        let candidate = directory.join(candidate_name(name, attempt));
        match try_reserve(&candidate, is_directory) {
            Ok(true) => return Ok(candidate),
            Ok(false) => continue,
            Err(e) => {
                return Err(MoveError::DestinationUnavailable {
                    destination: candidate,
                    detail: e.to_string(),
                })
            }
        }
    }
    Err(MoveError::NoFreeName {
        source: source.to_path_buf(),
        attempts: MAX_DESTINATION_ATTEMPTS,
    })
}

/// The one destination name a sidecar is allowed to use.
///
/// Sidecars keep the database's name; deriving `data (1).db-wal` for a second
/// `-wal` would silently detach it from its database. A taken name is
/// therefore a hard failure, and the caller rolls the operation back.
fn reserve_sidecar_destination(
    sidecar: &Path,
    directory: &Path,
    name: &OsStr,
    is_directory: bool,
) -> Result<PathBuf, MoveError> {
    let candidate = directory.join(name);
    match try_reserve(&candidate, is_directory) {
        Ok(true) => Ok(candidate),
        Ok(false) => Err(MoveError::SidecarBlocked {
            sidecar: sidecar.to_path_buf(),
            destination: candidate,
            detail: "the sidecar's required destination name is already in use".to_string(),
        }),
        Err(e) => Err(MoveError::SidecarBlocked {
            sidecar: sidecar.to_path_buf(),
            destination: candidate,
            detail: e.to_string(),
        }),
    }
}

/// Rename one entry into `destination_directory` under `name`.
fn move_one(
    from: &Path,
    destination_directory: &Path,
    name: &OsStr,
    is_directory: bool,
    bytes: u64,
    collision_safe: bool,
) -> Result<MovedFile, MoveError> {
    let to = if collision_safe {
        reserve_destination(from, destination_directory, name, is_directory)?
    } else {
        reserve_sidecar_destination(from, destination_directory, name, is_directory)?
    };
    fs::rename(from, &to).map_err(|e| MoveError::Move {
        from: from.to_path_buf(),
        to: to.clone(),
        detail: e.to_string(),
    })?;
    Ok(MovedFile {
        source: from.to_path_buf(),
        destination: to,
        bytes,
    })
}

/// Move `source` and every existing sidecar, recording each success.
///
/// The caller owns `moved` and is responsible for restoring it on error; this
/// function never rolls back on its own, so `execute` can roll back across
/// *several* targets with one reverse pass.
pub(crate) fn relocate_with_sidecars(
    source: &Path,
    destination_directory: &Path,
    destination_name: &OsStr,
    moved: &mut Vec<MovedFile>,
) -> Result<MovedFile, MoveError> {
    let metadata = fs::symlink_metadata(source).map_err(|e| MoveError::SourceUnavailable {
        source: source.to_path_buf(),
        detail: e.to_string(),
    })?;
    let bytes = measure_entry(source).map_err(|e| MoveError::SourceUnavailable {
        source: source.to_path_buf(),
        detail: e.to_string(),
    })?;

    let main = move_one(
        source,
        destination_directory,
        destination_name,
        metadata.file_type().is_dir(),
        bytes,
        true,
    )?;
    moved.push(main.clone());

    // Sidecars follow the destination name the MAIN file actually got, not the
    // name that was requested: if `data.db` collided and became `data (1).db`,
    // its `-wal` must become `data (1).db-wal` or it no longer belongs to the
    // database it describes.
    let main_name = match main.destination.file_name() {
        Some(name) => name.to_os_string(),
        None => destination_name.to_os_string(),
    };

    for suffix in SIDECAR_SUFFIXES {
        let sidecar = with_suffix(source, suffix);
        let sidecar_metadata = match fs::symlink_metadata(&sidecar) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(MoveError::SourceUnavailable {
                    source: sidecar,
                    detail: e.to_string(),
                })
            }
        };
        let sidecar_bytes = measure_entry(&sidecar).map_err(|e| MoveError::SourceUnavailable {
            source: sidecar.clone(),
            detail: e.to_string(),
        })?;
        let mut sidecar_name = main_name.clone();
        sidecar_name.push(suffix);
        let moved_sidecar = move_one(
            &sidecar,
            destination_directory,
            &sidecar_name,
            sidecar_metadata.file_type().is_dir(),
            sidecar_bytes,
            false,
        )?;
        moved.push(moved_sidecar);
    }

    Ok(main)
}

/// [`relocate_with_sidecars`] for a single target: rolls this call's own
/// moves back when any step fails.
fn relocate_and_roll_back(
    source: &Path,
    destination_directory: &Path,
    destination_name: &OsStr,
) -> Result<MovedFile, MoveError> {
    let mut moved: Vec<MovedFile> = Vec::new();
    match relocate_with_sidecars(source, destination_directory, destination_name, &mut moved) {
        Ok(main) => Ok(main),
        Err(original) => {
            let outcome = restore_all(&moved);
            if outcome.failures.is_empty() {
                Err(original)
            } else {
                Err(MoveError::RollbackIncomplete {
                    failed: source.to_path_buf(),
                    detail: original.to_string(),
                    unrestored: outcome.failures,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_names_keep_the_extension_and_count_up() {
        let name = OsStr::new("data.db");
        assert_eq!(candidate_name(name, 0), OsString::from("data.db"));
        assert_eq!(candidate_name(name, 1), OsString::from("data (1).db"));
        assert_eq!(candidate_name(name, 42), OsString::from("data (42).db"));
    }

    #[test]
    fn candidate_names_handle_a_missing_extension_and_dotfiles() {
        assert_eq!(
            candidate_name(OsStr::new("README"), 3),
            OsString::from("README (3)")
        );
        assert_eq!(
            candidate_name(OsStr::new(".hidden"), 1),
            OsString::from(".hidden (1)")
        );
        assert_eq!(
            candidate_name(OsStr::new("a.tar.gz"), 1),
            OsString::from("a.tar (1).gz")
        );
    }
}
