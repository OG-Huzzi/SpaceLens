//! CoreSight **action execution** — the only crate in this workspace that
//! moves a user's files (Phase 4.2).
//!
//! `coresight-cleanup` decides *what* should be cleaned and proves that the
//! plan is safe to propose. This crate is the single place where a plan
//! becomes a filesystem change, and it performs exactly one kind of change:
//! a **reversible relocation** into a directory the caller names (a trash
//! directory or an app-quarantine directory). There is no deletion here, no
//! secure wipe, no registry access, no process, no network.
//!
//! ## The three gates between a plan and a moved file
//!
//! 1. **[`authorize()`] is the only way to obtain an [`AuthorizedPlan`].** It
//!    runs [`validate_with_policy`](coresight_cleanup::validate_with_policy)
//!    and returns the validator's binding reasons as
//!    [`AuthorizeError::Blocked`] when the verdict is blocked, so a plan that
//!    contains a destructive action can never hold a token under a policy
//!    that forbids destructive actions. It then refuses any item whose action
//!    is not [`ActionKind::MoveToTrash`](coresight_cleanup::ActionKind): this
//!    crate has no deletion primitive, so a token for a `Delete` plan would
//!    authorize something the executor cannot honestly perform. The fields
//!    are private and there is no other constructor, no builder, and no
//!    mutable accessor.
//! 2. **[`execute()`] requires a [`ConfirmedPreview`]** whose plan id and byte
//!    total match the authorized plan. A mismatch is
//!    [`ExecuteError::NotConfirmed`] and no file is touched: execution cannot
//!    begin on a plan the caller has not confirmed. The intended flow is
//!    `CleanupPreview::from_plan(&plan)` → render → the user confirms →
//!    `ConfirmedPreview::new(preview.plan_id, preview.total_bytes)`.
//! 3. **Every move is recorded and every failure rolls back.** On the first
//!    failure, every pair recorded so far is restored in reverse order. If
//!    all restores succeed the error is [`ExecuteError::RolledBack`]; if any
//!    restore fails it is [`ExecuteError::PartialRollback`], which names the
//!    files that are still out of place. This crate never reports success
//!    over a half-moved set and never hides where a file is.
//!
//! ## What this crate guarantees
//!
//! * **No overwrite, ever.** A destination name is proven free by creating
//!   it exclusively (`create_new` / `create_dir`) before it is used; a taken
//!   name advances to `name (1).ext`, `name (2).ext`, … A pre-existing file
//!   at the destination is preserved, byte for byte.
//! * **No permanent deletion.** The crate contains no unlink primitive at
//!   all (enforced by a source-scan test): the only mutations are `rename`,
//!   `create_dir`, `create_dir_all`, `remove_dir` of an empty directory this
//!   crate created, and `create_new` of a zero-byte reservation that the
//!   rename replaces.
//! * **Sidecars travel with their database.** `<name>-wal` / `<name>-shm`
//!   move into the same destination directory under the same destination
//!   file name plus suffix. A sidecar that cannot be moved fails the whole
//!   operation and rolls the main file back.
//! * **Failure is reported, not smoothed.** Every error is a typed value
//!   carrying the paths involved. Rollback counts are exact.
//!
//! ## What this crate does NOT guarantee (read this before shipping it)
//!
//! * **This is not the operating system's Trash.** [`default_trash_directory`]
//!   is a *directory-based fallback*: files land in a plain folder. They do
//!   not appear in Finder's or Explorer's Trash, "Empty Trash" does not
//!   remove them, and no OS undo UI knows about them. The production path is
//!   the platform trash API (Finder / `IFileOperation`); this module is the
//!   portable, testable, honest substitute. A file parked in this directory
//!   still occupies disk space.
//! * **Path resolution is the caller's responsibility.** [`CleanupPlan`](coresight_cleanup::CleanupPlan)
//!   (see `coresight-cleanup`) is category-level: it carries action kinds,
//!   tiers, byte totals, and reasoning, but **no path list**. `execute`
//!   therefore cannot prove that a target belongs to the plan and does not
//!   pretend to: `targets` are the caller-resolved concrete paths for the
//!   plan's items. The token authorizes the plan's *action kinds and tiers*
//!   (only [`ActionKind::MoveToTrash`](coresight_cleanup::ActionKind) items),
//!   not a set of paths. A caller that resolves a wider path set than the
//!   preview showed would move files the user never reviewed; that failure
//!   lives above this crate, and the confirmation check does not catch it.
//! * **No atomic multi-file transaction.** Each move is an independent
//!   `rename`; a process killed mid-execute leaves files split between source
//!   and destination with no rollback, because rollback runs only for
//!   failures this process observes. A crash between two moves is not
//!   recoverable by this crate; the caller must re-run verification against
//!   the recorded report.
//! * **No cross-volume move.** `rename` cannot cross filesystems. If the
//!   trash/quarantine directory lives on another volume the move fails and
//!   rolls back; there is deliberately no copy-then-delete fallback, because
//!   that would create a window in which a copy exists and the original is
//!   gone.
//! * **[`ExecutionReport::freed_bytes`] is a source-side observation, not a
//!   verified result.** It is the sum of the sizes measured immediately
//!   before each move. [`verify()`] re-establishes what actually arrived at the
//!   destinations; only `verified_bytes` from that report may be presented as
//!   verified. Neither number proves the space was reclaimed by the OS: a
//!   file sitting in a trash directory is not free space.
//!
//! ## Style and safety discipline
//!
//! Mirrors `coresight-history`'s `store_safety` module: a short list of
//! invariants at the top, `///` on every public item explaining *why* it
//! exists, explicit error enums implementing [`std::fmt::Display`] and
//! [`std::error::Error`], and a call graph in which no helper can widen the
//! caller's authority. No `unsafe` ([`#![forbid(unsafe_code)]`]), no
//! `unwrap`/`expect`/`panic!` in production paths, no logging of paths
//! (paths reach only the returned error, which the caller decides how to
//! present).

#![forbid(unsafe_code)]

pub mod authorize;
pub mod execute;
pub mod trash;
pub mod verify;

mod rollback;

pub use authorize::{authorize, Authorization, AuthorizeError, AuthorizedPlan, ConfirmedPreview};
pub use execute::{execute, ExecuteError, ExecutionReport};
pub use rollback::RollbackFailure;
pub use trash::{
    default_trash_directory, move_to_quarantine, move_to_trash, move_to_trash_with, DirectoryTrash,
    MoveError, MovedFile, TrashDestination, MAX_DESTINATION_ATTEMPTS,
};
pub use verify::{verify, VerificationReport};
