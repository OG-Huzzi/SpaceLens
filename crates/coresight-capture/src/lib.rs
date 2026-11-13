//! CoreSight **screenshot capture planning** — the pure policy layer for
//! the last jv16 PowerTools surface: Screenshots.
//!
//! jv16's screenshot tool captures the screen and offers to share the image
//! to a cloud service. CoreSight must do the honest opposite. A screenshot
//! can contain a password manager, a medical record, or a private message,
//! so the planning layer has to make the safe choice the default and make
//! the unsafe one impossible to reach silently.
//!
//! ## What this crate decides
//!
//! * **Destination.** Only a directory the user chose *inside* an allowed
//!   root is writable. A requested path outside every root is refused with
//!   the reason, not quietly redirected. Falling back to the platform
//!   Screenshots/Desktop folder is always named in the note, together with
//!   why the fallback happened.
//! * **Naming.** [`plan_name`] composes a deterministic, collision-free
//!   `.png` file name from a prefix, an RFC 3339 timestamp, and an optional
//!   application name. The timestamp is an argument, never a clock read, so
//!   the same inputs always produce the same name. Existing names are never
//!   overwritten: the planner increments a collision index and gives up with
//!   [`NameError::NoFreeName`] rather than reuse a name.
//! * **Privacy.** [`upload_supported`] is `false` in this build and
//!   [`default_policy`] sets `upload_enabled: false` with
//!   `require_destination_confirmation: true`. Any future sharing feature is
//!   a separate, explicitly authorized change — a new module, a consent
//!   flow, and a capability gate — **not a flag flip**. The field exists so
//!   the absence is a value the UI can display and a test can pin.
//! * **Retention.** [`plan_retention`] keeps the newest captures and
//!   proposes older ones for deletion. `keep_last == 0` means *keep
//!   everything*: a zero that silently deleted every capture would be a
//!   catastrophic misreading of "keep 0". Nothing is ever deleted here;
//!   `delete_candidates` is a proposal the shell may act on only after the
//!   user confirms.
//!
//! ## What this crate deliberately does NOT do
//!
//! * **No pixel capture.** There is no screen API, no window enumeration,
//!   and no image buffer here. The platform capture call belongs in the
//!   desktop shell; this crate only plans what the shell should do with the
//!   result.
//! * **No upload and no network path.** No HTTP client, no socket, no URL.
//!   The crate cannot move a byte off the machine.
//! * **No deletion and no filesystem access.** Nothing here lists, stats,
//!   creates, renames, or removes a file. Every function takes facts as
//!   arguments and returns a plan.
//! * **No clock.** Timestamps are inputs, so every plan is reproducible in a
//!   test and reviewable by a user.
//! * No subprocess, no persistence, no telemetry, no LLM analysis.
//!
//! The guard tests in `tests/architecture.rs` enforce the "no I/O, no
//! panics" half of these contracts by scanning the production sources.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod destination;
pub mod naming;
pub mod policy;
pub mod retention;

pub use destination::{plan_destination, CaptureDestination, DestinationPlan};
pub use naming::{plan_name, CaptureName, CaptureNaming, NameError, MAX_COLLISION_INDEX};
pub use policy::{
    default_policy, policy_violations, upload_supported, CapturePolicy, SensitiveRegion,
};
pub use retention::{plan_retention, CaptureEntry, RetentionPlan, RetentionPolicy};

/// The only image extension this build plans for.
///
/// The desktop shell writes PNG because it is lossless and ubiquitous; a
/// format chooser would add encoder surface (and failure modes) without
/// changing any safety property, so this build deliberately has none.
pub const PNG_EXTENSION: &str = "png";
