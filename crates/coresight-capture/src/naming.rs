//! Naming: deterministic, collision-free capture file names.
//!
//! A capture name has three jobs: it must be predictable (the same inputs
//! always produce the same name), it must never overwrite an existing file,
//! and it must be a legal file name on Windows, macOS, and Linux. All three
//! are decided here from caller-supplied facts — the timestamp is an
//! argument, never a clock read, so the plan is reproducible in tests and
//! reviewable before the shell writes anything.
//!
//! The extension is always `.png` in this build (see
//! [`PNG_EXTENSION`]); there is no format chooser.
//!
//! Collision handling bumps a numeric suffix — `name.png`, `name (1).png`,
//! `name (2).png` — and stops at [`MAX_COLLISION_INDEX`] with
//! [`NameError::NoFreeName`]. It never reuses, truncates, or overwrites a
//! name. Comparison is case-insensitive because Windows and macOS filesystems
//! are: `Shot.png` and `shot.PNG` are the same file there, so treating them
//! as different would silently overwrite.
//!
//! The prefix is validated as a *stem*, not a path: separators, NUL, the
//! other characters Windows forbids, a trailing space or dot, and reserved
//! device names are all refused with a reason the UI can show. The goal is
//! that a rejected name fails loudly at planning time instead of producing a
//! file the user cannot find, open, or delete.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::PNG_EXTENSION;

/// The highest collision suffix this crate will try.
///
/// The base name plus suffixes `(1)` through `(10 000)` are attempted before
/// [`NameError::NoFreeName`] is returned. The bound exists so a pathological
/// directory can never make the planner loop forever; 10 000 is far beyond
/// any realistic screenshot folder and still finishes instantly.
pub const MAX_COLLISION_INDEX: u32 = 10_000;

/// How capture file names are composed.
///
/// The settings are data (not environment) so the UI can show the exact name
/// a capture will get before it is taken, and so the same settings reproduce
/// the same name on every run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureNaming {
    /// The leading token, e.g. `CoreSight`. Validated as a file-name stem.
    pub prefix: String,
    /// Append the RFC 3339 timestamp passed to [`plan_name`]. Timestamps keep
    /// captures distinguishable and sortable.
    pub include_timestamp: bool,
    /// Append the foreground application's name when the shell knows it,
    /// which helps the user recognise a shot without opening it.
    pub include_app: bool,
}

impl Default for CaptureNaming {
    /// The shipped default: `CoreSight`, timestamp and app name included.
    ///
    /// All three tokens are useful and none is sensitive by itself; the
    /// user can turn either suffix off without losing the prefix.
    fn default() -> Self {
        CaptureNaming {
            prefix: "CoreSight".to_string(),
            include_timestamp: true,
            include_app: true,
        }
    }
}

/// A planned file name, with the collision index that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureName {
    /// The file name only (never a path). Always ends in `.png`.
    pub file_name: String,
    /// `0` means the base name was free; `n` means `(n)` was appended.
    pub collision_index: u32,
}

impl CaptureName {
    /// Join this name to the directory it was planned for.
    ///
    /// The name has already been validated as a single path component, so
    /// the join cannot escape the directory.
    pub fn path_in(&self, directory: &Path) -> PathBuf {
        directory.join(&self.file_name)
    }
}

/// Why a capture name could not be planned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NameError {
    /// The prefix cannot be used as a file-name stem. `reason` is written
    /// for the user, not for a log.
    InvalidPrefix {
        /// Plain-language explanation of the rejected prefix.
        reason: String,
    },
    /// The timestamp was requested but contributes no usable characters, so
    /// the planned name would silently lose its only ordering token.
    InvalidTimestamp {
        /// Plain-language explanation of the rejected timestamp.
        reason: String,
    },
    /// Every collision suffix up to [`MAX_COLLISION_INDEX`] is taken. The
    /// caller must not fall back to overwriting: it should ask the user to
    /// choose another folder or clear old captures.
    NoFreeName,
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::InvalidPrefix { reason } => {
                write!(formatter, "invalid capture name prefix: {reason}")
            }
            NameError::InvalidTimestamp { reason } => {
                write!(formatter, "invalid capture timestamp: {reason}")
            }
            NameError::NoFreeName => write!(
                formatter,
                "no free capture file name: every collision suffix up to {MAX_COLLISION_INDEX} is taken"
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Plan a capture file name.
///
/// * `timestamp_rfc3339` is used only when
///   [`CaptureNaming::include_timestamp`] is true. It is an input on purpose:
///   reading the clock here would make the crate untestable and the plan
///   unreviewable.
/// * `app_name` is used only when [`CaptureNaming::include_app`] is true and
///   a name is supplied. A missing app name is not an error — the shell may
///   legitimately not know the foreground application.
/// * `existing` is the list of file names already in the destination.
///   Case-insensitive comparison means a name that differs only in case is
///   treated as taken, so the planner never overwrites on a
///   case-insensitive filesystem.
///
/// Returns [`NameError::InvalidPrefix`] when the prefix cannot be a stem,
/// [`NameError::InvalidTimestamp`] when a requested timestamp has no usable
/// characters, and [`NameError::NoFreeName`] when the collision bound is
/// exhausted.
pub fn plan_name(
    naming: &CaptureNaming,
    timestamp_rfc3339: &str,
    app_name: Option<&str>,
    existing: &[String],
) -> Result<CaptureName, NameError> {
    let prefix = validated_prefix(&naming.prefix)?;

    let mut tokens: Vec<String> = vec![prefix];

    if naming.include_timestamp {
        let timestamp = sanitize_token(timestamp_rfc3339);
        if timestamp.is_empty() {
            return Err(NameError::InvalidTimestamp {
                reason: format!(
                    "the timestamp {timestamp_rfc3339:?} contains no usable characters; pass an \
                     RFC 3339 instant such as 2026-01-31T09:15:00Z"
                ),
            });
        }
        tokens.push(timestamp);
    }

    if naming.include_app {
        if let Some(app) = app_name {
            let app = sanitize_token(app);
            if !app.is_empty() {
                tokens.push(app);
            }
        }
    }

    let stem = tokens.join(" ");
    if stem.is_empty() || stem == "." || stem == ".." {
        return Err(NameError::InvalidPrefix {
            reason: "the prefix produces an empty or dot-only file stem, which no filesystem \
                     accepts as a file name"
                .to_string(),
        });
    }

    let taken: BTreeSet<String> = existing.iter().map(|name| name.to_lowercase()).collect();
    let mut index: u32 = 0;
    loop {
        let candidate = candidate_name(&stem, index);
        if !taken.contains(&candidate.to_lowercase()) {
            return Ok(CaptureName {
                file_name: candidate,
                collision_index: index,
            });
        }
        if index >= MAX_COLLISION_INDEX {
            return Err(NameError::NoFreeName);
        }
        index = index.saturating_add(1);
    }
}

/// Build the file name for one collision index.
fn candidate_name(stem: &str, index: u32) -> String {
    if index == 0 {
        format!("{stem}.{PNG_EXTENSION}")
    } else {
        format!("{stem} ({index}).{PNG_EXTENSION}")
    }
}

/// Validate the prefix and return it unchanged when it is a legal stem.
fn validated_prefix(prefix: &str) -> Result<String, NameError> {
    match prefix_problem(prefix) {
        Some(reason) => Err(NameError::InvalidPrefix { reason }),
        None => Ok(prefix.to_string()),
    }
}

/// Why `prefix` cannot be a capture-name stem, if it cannot be one.
fn prefix_problem(prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some("the prefix is empty".to_string());
    }
    if prefix.trim().is_empty() {
        return Some("the prefix is only whitespace".to_string());
    }

    for character in prefix.chars() {
        if character == '\0' {
            return Some(
                "the prefix contains a NUL character, which no filesystem accepts in a file name"
                    .to_string(),
            );
        }
        if character == '/' || character == '\\' {
            return Some(
                "the prefix contains a path separator, which would move the file out of the \
                 planned directory"
                    .to_string(),
            );
        }
        if character == ':' {
            return Some(
                "the prefix contains ':', which Windows reads as a drive or alternate-stream \
                 separator"
                    .to_string(),
            );
        }
        if matches!(character, '<' | '>' | '"' | '|' | '?' | '*') {
            return Some(format!(
                "the prefix contains {character:?}, which Windows does not allow in a file name"
            ));
        }
        if character.is_control() {
            return Some(format!(
                "the prefix contains the control character {character:?}, which would be \
                 invisible in the file name"
            ));
        }
    }

    if prefix.ends_with(' ') || prefix.ends_with('.') {
        return Some(
            "the prefix ends with a space or a dot; Windows strips those silently, so the file \
             would not be named what the user sees"
                .to_string(),
        );
    }

    if let Some(base) = prefix.split('.').next() {
        let base = base.trim_end_matches(' ');
        if is_reserved_device_name(base) {
            return Some(format!(
                "the prefix {prefix:?} resolves to the Windows reserved device name {:?}, which \
                 cannot be used as a file name",
                base.to_ascii_uppercase()
            ));
        }
    }

    None
}

/// Whether `base` is a Windows reserved device name (case-insensitive).
///
/// Windows reserves these names with *any* extension, so `CON.png` is just as
/// unopenable as `CON`. Only the part before the first dot matters, and
/// trailing spaces are ignored by the OS, so both are normalised here.
fn is_reserved_device_name(base: &str) -> bool {
    let upper = base.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }

    let bytes = upper.as_bytes();
    bytes.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && matches!(bytes[3], b'1'..=b'9')
}

/// Turn an arbitrary label into one file-name token.
///
/// Letters and digits are kept (Unicode included, because a file name is
/// allowed to be non-ASCII); spaces are kept but collapsed; every other
/// character becomes a single `-`. Leading and trailing filler is removed,
/// so the token can never end in a dot or a space. A value with nothing
/// usable left yields an empty string, which callers omit rather than paste
/// into a name.
fn sanitize_token(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_filler = false;

    for character in value.trim().chars() {
        if character.is_alphanumeric() {
            out.push(character);
            last_was_filler = false;
        } else if character == ' ' {
            if !out.is_empty() && !last_was_filler {
                out.push(' ');
                last_was_filler = true;
            }
        } else if !out.is_empty() && !last_was_filler {
            out.push('-');
            last_was_filler = true;
        }
    }

    while out.ends_with(' ') || out.ends_with('-') {
        out.pop();
    }

    out
}
