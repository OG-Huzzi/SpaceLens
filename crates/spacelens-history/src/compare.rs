//! The pure comparison engine (Objective 20): `Snapshot A + Snapshot B →
//! ChangeSet`. Independent of any database — persistence loads two
//! snapshots and the engine derives typed, evidence-backed events
//! deterministically.
//!
//! ## Evidence ordering (Objective 21)
//!
//! Continuity is decided by **filesystem object identity first**; content
//! identity is never used to claim a move; filename, directory, size, and
//! timestamps are never proof. Deletion is claimed only when the target
//! run `observes_full_scope()`.
//!
//! ## Incomplete-scan safety (Objective 12 — hard invariant)
//!
//! `Created` and `Deleted` events require **both** runs to have observed
//! their full scope. A partial (cancelled/failed) run produces a
//! `ChangeSet` with `completeness: Partial` in which neither created nor
//! deleted paths are claimed — an inaccessible subtree can never become a
//! mass deletion. Object-identity continuity (moves) and proven
//! per-path facts (modifications, replacements) remain derivable: they
//! compare two *observed* facts and need no scope completeness.
//!
//! ## Determinism
//!
//! Events are canonically ordered (kind → path bytes → previous path
//! bytes); event ids are content-addressed (SHA-256 over the canonical
//! event tuple); identical input produces identical `ChangeSet`s.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::model::{ObservedEntry, RunId, RunRecord, RunStatus, Snapshot};

/// A run together with its snapshot — the comparison input. The
/// relationship derivation is optional: a run that never ran the
/// relationship layer simply contributes no relationship events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSnapshot {
    pub run: RunRecord,
    pub snapshot: Snapshot,
    pub relationships: Option<spacelens_identity::RelationshipReport>,
}

impl RunSnapshot {
    pub fn new(run: RunRecord, snapshot: Snapshot) -> Self {
        RunSnapshot {
            run,
            snapshot,
            relationships: None,
        }
    }

    /// Attach the run's relationship derivation.
    pub fn with_relationships(
        mut self,
        relationships: spacelens_identity::RelationshipReport,
    ) -> Self {
        self.relationships = Some(relationships);
        self
    }
}

/// Comparison completeness (Objective 12): whether the comparison may
/// claim created/deleted facts. `Partial` suppresses exactly those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComparisonCompleteness {
    /// Both runs observed their full declared scope.
    Complete,
    /// At least one run is partial: created/deleted claims are suppressed;
    /// continuity- and observation-proven events remain.
    Partial,
}

/// Typed change events. Only cleanly provable kinds exist — there is no
/// generic "something changed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    /// A path present in `to` with no continuity evidence for the run
    /// pair (requires full-scope `from`).
    Created,
    /// A path present in `from`, absent in `to`, with no object
    /// continuity elsewhere (requires full-scope `to`).
    Deleted,
    /// Same filesystem object, parent directory changed.
    Moved,
    /// Same filesystem object, same parent, file name changed.
    Renamed,
    /// Same object, verified content identity changed.
    Modified,
    /// Same object (or unproven identity), observed size changed.
    SizeChanged,
    /// Same path, stored classification changed (stored facts compared —
    /// never re-derived with newer rules).
    ClassificationChanged,
    /// A relationship exists in `to` but not in `from`.
    RelationshipAdded,
    /// A relationship existed in `from` but not in `to`.
    RelationshipRemoved,
    /// Same relationship id, different member set.
    RelationshipMembershipChanged,
    /// Same path, different filesystem object (replacement — not a
    /// modification of the old object; Objective 22).
    ObjectIdentityChanged,
    /// Observed without error before, observed with an error now.
    BecameInaccessible,
    /// Observed with an error before, observed cleanly now.
    BecameAccessible,
}

/// Categorical proof carried by an event. Each event explains why it
/// exists; combinations are canonical (sorted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventEvidence {
    /// Both runs proved the same filesystem object identity.
    ObjectIdentityEqual,
    /// Both runs proved the path but the object identities differ.
    ObjectIdentityDiffering,
    /// The path was absent from the from-run's observations.
    PathAbsentInFromRun,
    /// The path was present in the from-run's observations.
    PathPresentInFromRun,
    /// The path was absent from the to-run's observations.
    PathAbsentInToRun,
    /// The path was present in the to-run's observations.
    PathPresentInToRun,
    /// The to-run observed its full declared scope (required for
    /// deletion claims).
    ToRunCompleteForScope,
    /// Verified content identities exist in both runs and differ.
    ContentIdentityDiffering,
    /// Verified content identities exist in both runs and are equal.
    ContentIdentityEqual,
    /// Observed sizes exist in both runs and differ.
    SizeDiffering,
    /// Stored classifications exist in both runs and differ.
    ClassificationDiffering,
    /// The error-observation state differs between the runs.
    ObservationErrorStateChanged,
    /// The relationship's member set differs between the runs.
    RelationshipMembershipDiffering,
    /// Both runs' relationship derivations completed.
    RelationshipsCompleteInBothRuns,
}

/// One typed, evidence-backed change between two runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEvent {
    /// Deterministic content-addressed id (SHA-256 prefix over the
    /// canonical event tuple) — stable across recomputation.
    pub event_id: String,
    pub kind: EventKind,
    /// The to-run path (the from-run path for deletions).
    pub path: PathBuf,
    /// The from-run path for moves/renames.
    pub previous_path: Option<PathBuf>,
    /// Filesystem object identity when proven (either run).
    pub object: Option<(u64, u64)>,
    pub previous_size: Option<u64>,
    pub new_size: Option<u64>,
    pub previous_classification: Option<crate::model::ClassificationRef>,
    pub new_classification: Option<crate::model::ClassificationRef>,
    pub previous_content: Option<String>,
    pub new_content: Option<String>,
    /// Canonical (sorted) evidence list — never empty.
    pub evidence: Vec<EventEvidence>,
}

/// Per-kind counts (exact even when the event list is truncated).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeCounts {
    pub created: u64,
    pub deleted: u64,
    pub moved: u64,
    pub renamed: u64,
    pub modified: u64,
    pub size_changed: u64,
    pub classification_changed: u64,
    pub relationship_added: u64,
    pub relationship_removed: u64,
    pub relationship_membership_changed: u64,
    pub object_identity_changed: u64,
    pub became_inaccessible: u64,
    pub became_accessible: u64,
}

impl ChangeCounts {
    fn bump(&mut self, kind: EventKind) {
        match kind {
            EventKind::Created => self.created += 1,
            EventKind::Deleted => self.deleted += 1,
            EventKind::Moved => self.moved += 1,
            EventKind::Renamed => self.renamed += 1,
            EventKind::Modified => self.modified += 1,
            EventKind::SizeChanged => self.size_changed += 1,
            EventKind::ClassificationChanged => self.classification_changed += 1,
            EventKind::RelationshipAdded => self.relationship_added += 1,
            EventKind::RelationshipRemoved => self.relationship_removed += 1,
            EventKind::RelationshipMembershipChanged => self.relationship_membership_changed += 1,
            EventKind::ObjectIdentityChanged => self.object_identity_changed += 1,
            EventKind::BecameInaccessible => self.became_inaccessible += 1,
            EventKind::BecameAccessible => self.became_accessible += 1,
        }
    }
}

/// Why a comparison was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareError {
    /// The target run's roots do not cover the source run's scope
    /// (Objective 13): comparing unrelated universes is rejected, never
    /// silently misread.
    ScopeMismatch {
        from_roots: Vec<PathBuf>,
        to_roots: Vec<PathBuf>,
    },
    /// A run still marked `Running` cannot be compared — its persistence
    /// was never committed (Objective 18).
    RunNotCommitted { run_id: RunId, status: RunStatus },
    /// Comparing a run with itself is meaningless.
    SameRun,
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompareError::ScopeMismatch {
                from_roots,
                to_roots,
            } => write!(
                f,
                "scope mismatch: from-roots {from_roots:?} are not covered by to-roots {to_roots:?}"
            ),
            CompareError::RunNotCommitted { run_id, status } => {
                write!(f, "run {run_id} was never committed (status {status:?})")
            }
            CompareError::SameRun => write!(f, "cannot compare a run with itself"),
        }
    }
}

impl std::error::Error for CompareError {}

/// Boundedness knob for the comparison output (Objective 27).
#[derive(Debug, Clone)]
pub struct CompareOptions {
    /// Hard cap on published events; overflow is counted exactly into
    /// [`ChangeSet::events_truncated`]. `None` = unbounded (the caller
    /// opted out knowingly).
    pub max_events: Option<usize>,
}

impl Default for CompareOptions {
    fn default() -> Self {
        CompareOptions {
            max_events: Some(100_000),
        }
    }
}

/// The derived difference between two runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    pub from_run_id: RunId,
    pub to_run_id: RunId,
    pub completeness: ComparisonCompleteness,
    /// True when the two runs were produced under different
    /// configuration fingerprints — every event remains a fact, but
    /// consumers must read it against the respective run's config.
    pub config_versions_differ: bool,
    pub events: Vec<ChangeEvent>,
    pub events_truncated: u64,
    pub counts: ChangeCounts,
}

/// Compare two committed runs. Deterministic and pure: no database, no
/// clock, no randomness.
pub fn compare(
    from: &RunSnapshot,
    to: &RunSnapshot,
    options: &CompareOptions,
) -> Result<ChangeSet, CompareError> {
    if from.run.run_id == to.run.run_id {
        return Err(CompareError::SameRun);
    }
    // Both runs must be committed (never Running).
    for rs in [from, to] {
        if rs.run.status == RunStatus::Running {
            return Err(CompareError::RunNotCommitted {
                run_id: rs.run.run_id.clone(),
                status: rs.run.status,
            });
        }
    }
    // Scope: the to-run must cover the from-run's declared scope.
    if !to.run.covers(&from.run.roots) {
        return Err(CompareError::ScopeMismatch {
            from_roots: from.run.roots.clone(),
            to_roots: to.run.roots.clone(),
        });
    }

    let completeness =
        if from.run.status.observes_full_scope() && to.run.status.observes_full_scope() {
            ComparisonCompleteness::Complete
        } else {
            ComparisonCompleteness::Partial
        };
    let config_versions_differ = from.run.config != to.run.config;

    // Keyed maps — O(n + m), never pairwise.
    let from_by_path: BTreeMap<&Path, &ObservedEntry> = from
        .snapshot
        .entries
        .iter()
        .map(|e| (e.path.as_path(), e))
        .collect();
    let to_by_path: BTreeMap<&Path, &ObservedEntry> = to
        .snapshot
        .entries
        .iter()
        .map(|e| (e.path.as_path(), e))
        .collect();
    let mut from_by_object: BTreeMap<(u64, u64), Vec<&Path>> = BTreeMap::new();
    let mut to_by_object: BTreeMap<(u64, u64), Vec<&Path>> = BTreeMap::new();
    for e in &from.snapshot.entries {
        if let Some(o) = e.object {
            from_by_object.entry(o).or_default().push(e.path.as_path());
        }
    }
    for e in &to.snapshot.entries {
        if let Some(o) = e.object {
            to_by_object.entry(o).or_default().push(e.path.as_path());
        }
    }

    let mut events: Vec<ChangeEvent> = Vec::new();
    let mut moved_paths: std::collections::BTreeSet<&Path> = std::collections::BTreeSet::new();

    // ---- PASS 1: paths present in both runs -----------------------------
    // Proven per-path facts: modification, size, classification,
    // accessibility, replacement.
    for (path, f) in &from_by_path {
        let Some(t) = to_by_path.get(path) else {
            continue;
        };
        if let (Some(of), Some(ot)) = (f.object, t.object) {
            if of != ot {
                // Replacement: a different object at the observed path —
                // never claimed as a modification of the old object.
                events.push(event(
                    EventKind::ObjectIdentityChanged,
                    from.run.run_id.clone(),
                    to.run.run_id.clone(),
                    path,
                    None,
                    Some(of),
                    &[
                        EventEvidence::ObjectIdentityDiffering,
                        EventEvidence::PathPresentInFromRun,
                        EventEvidence::PathPresentInToRun,
                    ],
                    Some(f),
                    Some(t),
                ));
                continue;
            }
        }
        if f.size.is_some() && t.size.is_some() && f.size != t.size {
            events.push(event(
                EventKind::SizeChanged,
                from.run.run_id.clone(),
                to.run.run_id.clone(),
                path,
                None,
                f.object,
                &[
                    EventEvidence::SizeDiffering,
                    EventEvidence::PathPresentInFromRun,
                    EventEvidence::PathPresentInToRun,
                ],
                Some(f),
                Some(t),
            ));
        }
        if let (Some(cf), Some(ct)) = (&f.content_sha256, &t.content_sha256) {
            if cf != ct {
                // Modified requires a same-object pair (Objective 22:
                // different object = replacement, handled above). Content
                // difference at the same path with unproven identity is
                // still a content change of whatever lives there.
                events.push(event(
                    EventKind::Modified,
                    from.run.run_id.clone(),
                    to.run.run_id.clone(),
                    path,
                    None,
                    f.object.or(t.object),
                    &[
                        EventEvidence::ContentIdentityDiffering,
                        EventEvidence::PathPresentInFromRun,
                        EventEvidence::PathPresentInToRun,
                    ],
                    Some(f),
                    Some(t),
                ));
            }
        }
        if f.classification.is_some()
            && t.classification.is_some()
            && f.classification != t.classification
        {
            events.push(event(
                EventKind::ClassificationChanged,
                from.run.run_id.clone(),
                to.run.run_id.clone(),
                path,
                None,
                f.object,
                &[
                    EventEvidence::ClassificationDiffering,
                    EventEvidence::PathPresentInFromRun,
                    EventEvidence::PathPresentInToRun,
                ],
                Some(f),
                Some(t),
            ));
        }
        if f.observation_error.is_none() && t.observation_error.is_some() {
            events.push(event(
                EventKind::BecameInaccessible,
                from.run.run_id.clone(),
                to.run.run_id.clone(),
                path,
                None,
                f.object,
                &[
                    EventEvidence::ObservationErrorStateChanged,
                    EventEvidence::PathPresentInFromRun,
                    EventEvidence::PathPresentInToRun,
                ],
                Some(f),
                Some(t),
            ));
        } else if f.observation_error.is_some() && t.observation_error.is_none() {
            events.push(event(
                EventKind::BecameAccessible,
                from.run.run_id.clone(),
                to.run.run_id.clone(),
                path,
                None,
                t.object,
                &[
                    EventEvidence::ObservationErrorStateChanged,
                    EventEvidence::PathPresentInFromRun,
                    EventEvidence::PathPresentInToRun,
                ],
                Some(f),
                Some(t),
            ));
        }
    }

    // ---- PASS 2: object continuity across differing paths ---------------
    // Same object, different path(s): Move (parent changed) or Rename
    // (same parent) — but ONLY when every old location of the object is
    // gone. If at least one old path still holds the object, the new path
    // is an ADDED ALIAS (a new path for a surviving object): pass 4
    // reports it as `Created` with continuity evidence, which is the
    // honest path-level fact. Object identity is the sole proof of
    // continuity; valid even when a run is partial.
    for (object, from_paths) in &from_by_object {
        let Some(to_paths) = to_by_object.get(object) else {
            continue;
        };
        // Continuity kind: all old locations gone ⇒ the object moved.
        let object_relocated = !from_paths.iter().any(|p| to_by_path.contains_key(*p));
        if !object_relocated {
            continue; // surviving alias/locations: pass 4 handles new paths
        }
        for new_path in to_paths {
            if from_paths.contains(new_path) {
                continue; // unchanged location
            }
            // The new path must genuinely be new to the run pair (a path
            // present in both runs with the same object was handled in
            // pass 1; with a different object in pass 1 as replacement).
            let previous = from_paths
                .iter()
                .min()
                .expect("object continuity implies at least one from-path");
            moved_paths.insert(new_path);
            let kind = move_or_rename(new_path, previous);
            let mut ev = event(
                kind,
                from.run.run_id.clone(),
                to.run.run_id.clone(),
                new_path,
                Some((*previous).to_path_buf()),
                Some(*object),
                &[
                    EventEvidence::ObjectIdentityEqual,
                    EventEvidence::PathPresentInFromRun,
                    EventEvidence::PathPresentInToRun,
                ],
                from_by_path.get(*previous).copied(),
                to_by_path.get(*new_path).copied(),
            );
            // A move's evidence references the previous entry's state.
            ev.previous_size = from_by_path.get(*previous).and_then(|e| e.size);
            ev.previous_content = from_by_path
                .get(*previous)
                .and_then(|e| e.content_sha256.clone());
            ev.previous_classification = from_by_path
                .get(*previous)
                .and_then(|e| e.classification.clone());
            events.push(ev);
        }
    }

    // ---- PASS 3: deletions (full-scope to-run ONLY — hard invariant) ----
    // An object (or path) seen in `from` and absent from `to` is deleted
    // only when the to-run observed its entire declared scope. A partial
    // run's missing paths are UNCERTAIN, never deletions.
    if to.run.status.observes_full_scope() {
        for (object, from_paths) in &from_by_object {
            if to_by_object.contains_key(object) {
                continue; // object survives somewhere: no deletion
            }
            for p in from_paths {
                if to_by_path.contains_key(*p) {
                    continue; // path exists with a different object: pass 1
                }
                events.push(event(
                    EventKind::Deleted,
                    from.run.run_id.clone(),
                    to.run.run_id.clone(),
                    p,
                    None,
                    Some(*object),
                    &[
                        EventEvidence::PathPresentInFromRun,
                        EventEvidence::PathAbsentInToRun,
                        EventEvidence::ToRunCompleteForScope,
                    ],
                    from_by_path.get(*p).copied(),
                    None,
                ));
            }
        }
        // Paths without provable object identity that vanished entirely.
        for (path, f) in &from_by_path {
            if f.object.is_some() {
                continue; // object-keyed deletion handled above
            }
            if !to_by_path.contains_key(*path) {
                events.push(event(
                    EventKind::Deleted,
                    from.run.run_id.clone(),
                    to.run.run_id.clone(),
                    path,
                    None,
                    None,
                    &[
                        EventEvidence::PathPresentInFromRun,
                        EventEvidence::PathAbsentInToRun,
                        EventEvidence::ToRunCompleteForScope,
                    ],
                    Some(f),
                    None,
                ));
            }
        }
    }

    // ---- PASS 4: creations (full-scope from-run ONLY) --------------------
    // A path new in `to` proves creation only when the from-run observed
    // its whole scope (a partial from-run may simply have missed it).
    // Paths whose object already existed in `from` are moves (pass 2),
    // not creations — except added ALIASES of a surviving object, which
    // are new paths for an existing object and are reported as Created
    // with continuity evidence.
    if from.run.status.observes_full_scope() {
        for (path, t) in &to_by_path {
            if from_by_path.contains_key(*path) {
                continue;
            }
            // A move/rename destination was already explained by object
            // continuity (pass 2) — never double-counted as a creation.
            if moved_paths.contains(*path) {
                continue;
            }
            let continuity = t.object.and_then(|o| from_by_object.get(&o)).is_some();
            let mut evidence = vec![
                EventEvidence::PathAbsentInFromRun,
                EventEvidence::PathPresentInToRun,
                EventEvidence::ToRunCompleteForScope,
            ];
            if continuity {
                evidence.push(EventEvidence::ObjectIdentityEqual);
            }
            evidence.sort();
            events.push(event(
                EventKind::Created,
                from.run.run_id.clone(),
                to.run.run_id.clone(),
                path,
                None,
                t.object,
                &evidence,
                None,
                Some(t),
            ));
        }
    }

    // ---- PASS 5: relationship changes ------------------------------------
    if let (Some(rel_from), Some(rel_to)) = (&from.relationships, &to.relationships) {
        let rel_complete = |st: spacelens_identity::DuplicateStatus| {
            matches!(
                st,
                spacelens_identity::DuplicateStatus::Completed
                    | spacelens_identity::DuplicateStatus::CompletedWithLimits
            )
        };
        if rel_complete(rel_from.status) && rel_complete(rel_to.status) {
            let mut ids: Vec<&str> = rel_from
                .relationships
                .iter()
                .map(|r| r.id.as_str())
                .collect();
            ids.extend(rel_to.relationships.iter().map(|r| r.id.as_str()));
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let a = rel_from.relationships.iter().find(|r| r.id == id);
                let b = rel_to.relationships.iter().find(|r| r.id == id);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        if a.member_count != b.member_count || a.members != b.members {
                            let mut ev = event(
                                EventKind::RelationshipMembershipChanged,
                                from.run.run_id.clone(),
                                to.run.run_id.clone(),
                                Path::new(id),
                                None,
                                None,
                                &[
                                    EventEvidence::RelationshipMembershipDiffering,
                                    EventEvidence::RelationshipsCompleteInBothRuns,
                                ],
                                None,
                                None,
                            );
                            ev.previous_size = Some(a.member_count);
                            ev.new_size = Some(b.member_count);
                            // The relationship id is the path reference for
                            // relationship events (a stable, content-derived
                            // key — Objective 23).
                            events.push(ev);
                        }
                    }
                    (Some(_), None) => events.push(event(
                        EventKind::RelationshipRemoved,
                        from.run.run_id.clone(),
                        to.run.run_id.clone(),
                        Path::new(id),
                        None,
                        None,
                        &[EventEvidence::RelationshipsCompleteInBothRuns],
                        None,
                        None,
                    )),
                    (None, Some(_)) => events.push(event(
                        EventKind::RelationshipAdded,
                        from.run.run_id.clone(),
                        to.run.run_id.clone(),
                        Path::new(id),
                        None,
                        None,
                        &[EventEvidence::RelationshipsCompleteInBothRuns],
                        None,
                        None,
                    )),
                    (None, None) => unreachable!(),
                }
            }
        }
    }

    // Canonical order + boundedness.
    events.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| {
                a.path
                    .as_os_str()
                    .as_encoded_bytes()
                    .cmp(b.path.as_os_str().as_encoded_bytes())
            })
            .then_with(|| {
                a.previous_path
                    .as_ref()
                    .map(|p| p.as_os_str().as_encoded_bytes())
                    .cmp(
                        &b.previous_path
                            .as_ref()
                            .map(|p| p.as_os_str().as_encoded_bytes()),
                    )
            })
            .then_with(|| a.event_id.cmp(&b.event_id))
    });
    let mut counts = ChangeCounts::default();
    for e in &events {
        counts.bump(e.kind);
    }
    let total = events.len() as u64;
    let truncated = total.saturating_sub(options.max_events.unwrap_or(total as usize) as u64);
    if let Some(cap) = options.max_events {
        events.truncate(cap);
    }
    Ok(ChangeSet {
        from_run_id: from.run.run_id.clone(),
        to_run_id: to.run.run_id.clone(),
        completeness,
        config_versions_differ,
        events,
        events_truncated: truncated,
        counts,
    })
}

/// Move vs rename: the parent directory changed vs the file name changed
/// (both proven by the same object identity).
fn move_or_rename(new_path: &Path, previous_path: &Path) -> EventKind {
    let new_parent = new_path.parent();
    let old_parent = previous_path.parent();
    if new_parent == old_parent {
        EventKind::Renamed
    } else {
        EventKind::Moved
    }
}

/// Event constructor: builds the canonical event tuple, derives the
/// content-addressed id, and copies previous/new state from the entries.
#[allow(clippy::too_many_arguments)]
fn event(
    kind: EventKind,
    from_run: RunId,
    to_run: RunId,
    path: &Path,
    previous_path: Option<PathBuf>,
    object: Option<(u64, u64)>,
    evidence: &[EventEvidence],
    previous: Option<&ObservedEntry>,
    new: Option<&ObservedEntry>,
) -> ChangeEvent {
    let mut evidence = evidence.to_vec();
    evidence.sort();
    evidence.dedup();
    let object_str = object
        .map(|(d, i)| format!("{d:016x}-{i:016x}"))
        .unwrap_or_default();
    let canonical = format!(
        "{kind:?}|{from_run}|{to_run}|{}|{}|{object_str}",
        path.display(),
        previous_path
            .as_deref()
            .map(Path::display)
            .unwrap_or_else(|| Path::new("").display())
    );
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    let id = format!(
        "ev-{}{:02x}{:02x}",
        hex16(&digest.as_slice()[..8]),
        digest.as_slice()[8],
        digest.as_slice()[9]
    );
    ChangeEvent {
        event_id: id,
        kind,
        path: path.to_path_buf(),
        previous_path,
        object,
        previous_size: previous.and_then(|e| e.size),
        new_size: new.and_then(|e| e.size),
        previous_classification: previous.and_then(|e| e.classification.clone()),
        new_classification: new.and_then(|e| e.classification.clone()),
        previous_content: previous.and_then(|e| e.content_sha256.clone()),
        new_content: new.and_then(|e| e.content_sha256.clone()),
        evidence,
    }
}

fn hex16(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
