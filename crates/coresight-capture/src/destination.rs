//! Destination policy: where a capture may be written, and why.
//!
//! The rule is deliberately narrow. A capture may only be written to a
//! directory the user chose that lies **inside** one of the allowed roots,
//! or to the platform's own Screenshots/Desktop folder when the user chose
//! nothing at all. A requested path outside every root is reported as not
//! writable — it is never quietly redirected to the platform default,
//! because "saved somewhere I did not ask for" is exactly the surprise a
//! privacy-first tool must not spring on its user.
//!
//! Containment is decided **lexically**: no directory is canonicalized, no
//! symlink is followed, and no filesystem call is made. The comparison is
//! component-wise, `..` is resolved before comparing, and a path that climbs
//! above its own root is refused rather than guessed at. When the filesystem
//! is case-insensitive a case variant of a root is therefore refused rather
//! than accepted: the planner fails closed, and the shell must canonicalize
//! (and re-check junction/symlink targets) before it opens the file.
//!
//! The note is part of the contract, not decoration: every plan states which
//! destination was chosen and why, in language the UI can show verbatim.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where a planned capture would be written.
///
/// The two variants are kept distinct — instead of collapsing to a bare
/// path — so the shell and the UI can always tell a user choice from an OS
/// default. That distinction is what makes a silent substitution
/// impossible to hide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CaptureDestination {
    /// A directory the user chose. The only writable destination.
    UserChosenDirectory {
        /// The directory exactly as supplied by the user; never rewritten
        /// by this crate.
        path: PathBuf,
    },
    /// The system Screenshots/Desktop folder for this platform.
    PlatformDefault {
        /// The folder the shell resolved from the OS known-folder API.
        path: PathBuf,
    },
}

impl CaptureDestination {
    /// The directory this destination points at.
    ///
    /// Both variants are directories, so callers that only need the target
    /// (for display, joining, or logging) do not have to match twice.
    pub fn path(&self) -> &Path {
        match self {
            CaptureDestination::UserChosenDirectory { path }
            | CaptureDestination::PlatformDefault { path } => path,
        }
    }
}

/// The result of planning a destination.
///
/// `writable` is a statement about this plan only: the shell must still
/// confirm the destination with the user before the first capture, and must
/// still respect the platform's own permission errors afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationPlan {
    /// Which directory was chosen, and whether it was the user's or the
    /// platform default.
    pub destination: CaptureDestination,
    /// Whether CoreSight may write a capture there under this plan.
    pub writable: bool,
    /// Plain-language explanation of the choice: what was chosen and why.
    /// Always non-empty; the UI shows it next to the confirmation.
    pub note: String,
}

impl DestinationPlan {
    /// The directory this plan would write to.
    pub fn path(&self) -> &Path {
        self.destination.path()
    }

    /// Whether the destination came from the user rather than the platform.
    ///
    /// The UI uses this to label the confirmation ("your folder" versus
    /// "system Screenshots folder"); it is never used to weaken a check.
    pub fn is_user_chosen(&self) -> bool {
        matches!(
            &self.destination,
            CaptureDestination::UserChosenDirectory { .. }
        )
    }
}

/// Plan where a capture may be written.
///
/// * `requested` — the directory the user picked, if any. `Some` is always
///   honoured as the *choice*; it is refused (not replaced) when it falls
///   outside every allowed root.
/// * `platform_default` — the OS-provided Screenshots/Desktop folder, used
///   only when nothing was requested.
/// * `allowed_roots` — the directories CoreSight is willing to write inside.
///
/// The returned note always names the chosen destination and the reason, so
/// a fallback can never be silent. A requested path that is empty, resolves
/// to nothing, or escapes its root via `..` is not writable.
pub fn plan_destination(
    requested: Option<&Path>,
    platform_default: &Path,
    allowed_roots: &[PathBuf],
) -> DestinationPlan {
    match requested {
        Some(path) => plan_user_chosen(path, allowed_roots),
        None => plan_platform_default(platform_default),
    }
}

/// The user chose a directory: accept it only inside an allowed root.
fn plan_user_chosen(path: &Path, allowed_roots: &[PathBuf]) -> DestinationPlan {
    let destination = CaptureDestination::UserChosenDirectory {
        path: path.to_path_buf(),
    };

    if path.as_os_str().is_empty() {
        return DestinationPlan {
            destination,
            writable: false,
            note: "Destination: the directory you chose is an empty path, so it cannot be used. \
                   Writable: no. Choose a directory inside an allowed root; CoreSight will not \
                   guess one, and it will not silently fall back to the platform default."
                .to_string(),
        };
    }

    let requested_parts = match split_components(path) {
        Some(parts) if !parts.is_empty() => parts,
        _ => {
            return DestinationPlan {
                destination,
                writable: false,
                note: format!(
                    "Destination: the directory you chose ({}), which does not resolve to a \
                     usable location: it is empty after normalisation or it climbs above its own \
                     root with `..`. Writable: no. CoreSight refuses the path instead of guessing \
                     where it lands, and it does not silently fall back to the platform default.",
                    path.display()
                ),
            };
        }
    };

    // Pick the *most specific* matching root so the note names the closest
    // boundary and the result does not depend on the order of `allowed_roots`.
    let mut chosen_root: Option<(&PathBuf, usize)> = None;
    for root in allowed_roots {
        if let Some(depth) = containment_depth(root, &requested_parts) {
            let is_more_specific = match chosen_root {
                Some((_, best_depth)) => depth > best_depth,
                None => true,
            };
            if is_more_specific {
                chosen_root = Some((root, depth));
            }
        }
    }

    match chosen_root.map(|(root, _)| root) {
        Some(root) => DestinationPlan {
            destination,
            writable: true,
            note: format!(
                "Destination: the directory you chose ({}), because it is inside the allowed \
                 root {}. Writable: yes. The first capture still requires you to confirm this \
                 destination before anything is written.",
                path.display(),
                root.display()
            ),
        },
        None => {
            let reason = if allowed_roots.is_empty() {
                "no allowed roots are configured".to_string()
            } else {
                format!(
                    "it is outside every allowed root ({})",
                    join_paths(allowed_roots)
                )
            };
            DestinationPlan {
                destination,
                writable: false,
                note: format!(
                    "Destination: the directory you chose ({}). Writable: no, because {reason}. \
                     CoreSight will not save a screenshot there, and it does not silently fall \
                     back to the platform default: choose a folder inside an allowed root.",
                    path.display()
                ),
            }
        }
    }
}

/// No directory was requested: say so, and name the platform default.
fn plan_platform_default(platform_default: &Path) -> DestinationPlan {
    DestinationPlan {
        destination: CaptureDestination::PlatformDefault {
            path: platform_default.to_path_buf(),
        },
        writable: true,
        note: format!(
            "Destination: the platform default folder ({}), chosen because no directory was \
             requested. Writable: yes. The first capture still requires you to confirm this \
             destination, and you can choose a different folder at any time.",
            platform_default.display()
        ),
    }
}

/// One lexical path component, kept comparable without touching the disk.
#[derive(Debug, PartialEq, Eq)]
enum PathPart {
    Prefix(OsString),
    Root,
    Normal(OsString),
}

/// Split `path` into normalised components.
///
/// `None` means the path climbs above its own root (for example `..` at the
/// start, or `a/../..`). There is no honest lexical answer for that case, so
/// the caller refuses the path instead of inventing one.
fn split_components(path: &Path) -> Option<Vec<PathPart>> {
    let mut parts: Vec<PathPart> = Vec::new();
    let mut normal_count: usize = 0;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                parts.push(PathPart::Prefix(prefix.as_os_str().to_os_string()));
            }
            Component::RootDir => parts.push(PathPart::Root),
            Component::CurDir => {}
            Component::ParentDir => {
                if normal_count == 0 {
                    return None;
                }
                let last_normal = parts
                    .iter()
                    .rposition(|part| matches!(part, PathPart::Normal(_)))?;
                parts.remove(last_normal);
                normal_count = normal_count.saturating_sub(1);
            }
            Component::Normal(name) => {
                parts.push(PathPart::Normal(name.to_os_string()));
                normal_count = normal_count.saturating_add(1);
            }
        }
    }

    Some(parts)
}

/// Whether `requested` is `root` itself or lies below it, and how deep the
/// match is.
///
/// An empty or unresolvable root matches nothing: an empty allowed root must
/// never turn the whole filesystem into a writable destination. The depth is
/// the number of components in the root, which lets the caller choose the
/// most specific of several matching roots without depending on their order.
fn containment_depth(root: &Path, requested: &[PathPart]) -> Option<usize> {
    match split_components(root) {
        Some(root_parts) if !root_parts.is_empty() && requested.starts_with(&root_parts) => {
            Some(root_parts.len())
        }
        _ => None,
    }
}

/// Render roots for the note in the order the caller supplied them.
fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<String>>()
        .join(", ")
}
