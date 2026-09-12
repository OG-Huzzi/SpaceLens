//! The System Memory domain model (Phase 5) — what "memory" means.
//!
//! Six distinct concepts, never merged into one vague "file record":
//!
//! | Concept | Type | Meaning |
//! |---|---|---|
//! | **Run / observation** | [`RunRecord`] | What a specific scan/run observed, when, under which configuration, and whether it completed. |
//! | **Stable object** | `(volume, file id)` inside [`ObservedEntry`] | The underlying filesystem object, proven from handles (Phase 3.2 identity). |
//! | **Path** | [`ObservedEntry::path`] | Where the object (or some object) was observed. Tracked independently of object identity. |
//! | **Content** | [`ObservedEntry::content_sha256`] | Verified content identity — stored ONLY where Phase 3/4 actually produced one (verified duplicate candidates). Unknown stays unknown. |
//! | **Classification** | [`ClassificationRef`] | The Phase 2 category observed in that run, with the rules version that produced it. |
//! | **Historical event** | [`crate::compare::ChangeEvent`] | A derived, evidence-backed statement about what changed BETWEEN two runs (see `compare`). |
//!
//! Facts only: every `Option` is an honest unknown. Nothing is inferred
//! from timestamps alone, and no record claims more than its run observed.
//!
//! Everything here is plain data (serde, `PartialEq`, `Eq`) so the
//! comparison engine can stay pure and the persistence layer a thin
//! serializer. Contract namespace: `spacelens.v1.history.*`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use spacelens_engine::FsEntry;
use spacelens_identity::hash::HashAlgorithm;

/// Unique identity of one stored run. Generated once when a run begins
/// (timestamp + process entropy); constructed explicitly in tests.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub String);

impl RunId {
    /// Generate a unique run id: unix-nanos timestamp plus a short
    /// process-entropy suffix (from `RandomState`'s per-process seed). The
    /// id only needs uniqueness within one history store, not
    /// cryptographic strength.
    pub fn generate() -> Self {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(nanos as u64);
        let salt = hasher.finish() & 0xFFFF;
        RunId(format!("r{nanos:024x}{salt:04x}"))
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lifecycle of a stored run. Reuses the engine's conventions
/// (`Completed`/`Cancelled`/`Failed` mirror `ScanStatus`;
/// `CompletedWithLimits` mirrors the Phase 3/4 pipeline status) plus
/// `Running` for a run whose persistence is still in flight.
///
/// **Invariant:** a run that is not `Completed`/`CompletedWithLimits` can
/// never serve as a complete baseline for change comparisons — a
/// cancelled or failed run's missing paths are NOT deletions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    /// Persisted at run start; the snapshot is not yet committed.
    Running,
    /// Observation + derivation + persistence all committed.
    Completed,
    /// Committed, and the run finished, but at least one engine cap
    /// truncated candidate work (Phase 3/4 semantics). Observation of
    /// paths is still complete, so historical comparisons are valid.
    CompletedWithLimits,
    /// Cancelled mid-run: the observed subset is partial by definition.
    Cancelled,
    /// Failed (including runs recovered as interrupted after a crash).
    Failed,
}

impl RunStatus {
    /// True when this run observed its full declared scope — the only
    /// runs whose missing paths may be interpreted as deletions.
    pub fn observes_full_scope(self) -> bool {
        matches!(self, RunStatus::Completed | RunStatus::CompletedWithLimits)
    }
}

/// The configuration a run was produced under (Objective 14). Persisted
/// per run so future code changes can never make old records
/// semantically ambiguous: historical facts are always read together
/// with the rules that produced them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFingerprint {
    /// Observation (scanner) model version — the normalized `FsEntry`
    /// shape and semantics.
    pub observation_model: u32,
    /// Classification schema tag (`spacelens.v1.classification`).
    pub classifier_schema: String,
    /// Classification rule-table version (classifier `RULES_VERSION`).
    pub classifier_rules: u32,
    /// Content-identity hash algorithm tag (`"sha256"`).
    pub hash_algorithm: String,
    /// Relationship model schema version.
    pub relationship_schema: u32,
    /// History model schema version.
    pub history_schema: u32,
}

impl ConfigFingerprint {
    /// The fingerprint values for the current build. A bump of any
    /// component is a visible contract event for historical comparisons.
    pub fn current() -> Self {
        ConfigFingerprint {
            observation_model: 1,
            classifier_schema: spacelens_classifier::classify::Classification::SCHEMA.to_string(),
            classifier_rules: spacelens_classifier::RULES_VERSION,
            hash_algorithm: HashAlgorithm::Sha256.tag().to_string(),
            relationship_schema: 1,
            history_schema: 1,
        }
    }

    /// Deterministic fingerprint string (SHA-256 over the canonical JSON
    /// form) — used as the compact comparison key in persistence.
    pub fn fingerprint(&self) -> String {
        let canonical = serde_json::to_string(self).unwrap_or_default();
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(canonical.as_bytes());
        let mut hex = String::with_capacity(64);
        for b in digest {
            hex.push_str(&format!("{b:02x}"));
        }
        hex
    }
}

/// Fixed-width per-run counters copied from the engine/phase reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCounts {
    pub entries_examined: u64,
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub other_entries: u64,
    pub bytes: u64,
    pub observation_errors: u64,
    /// Eligible candidates never hashed because an engine cap bit
    /// (Phase 3 `candidatesUntrackedTotal`).
    pub candidates_untracked: u64,
    /// Files whose hashing failed (typed reasons in the run's reports).
    pub hash_failures: u64,
}

/// One stored observation run: identity, scope, configuration, status,
/// counts. The snapshot itself lives in [`Snapshot`] (the per-path facts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: RunId,
    pub started_at: SystemTime,
    /// Set when the run reaches a terminal status; `None` while running.
    pub completed_at: Option<SystemTime>,
    /// Canonical scan roots (the run's scope). Comparisons require the
    /// target run's roots to cover the source run's roots.
    pub roots: Vec<PathBuf>,
    /// `os/arch` of the observing process.
    pub platform: String,
    pub config: ConfigFingerprint,
    pub status: RunStatus,
    pub counts: RunCounts,
}

impl RunRecord {
    /// True when this run's roots cover `scope` (every scope root is at or
    /// under some run root, compared case-insensitively on Windows-style
    /// components via exact prefix matching on the component sequences).
    pub fn covers(&self, scope: &[PathBuf]) -> bool {
        scope
            .iter()
            .all(|s| self.roots.iter().any(|r| path_covers(r, s)))
    }
}

/// `root` covers `p` when `p` equals `root` or lies beneath it
/// (component-wise prefix — no string-prefix ambiguity like
/// `C:\a` vs `C:\ab`).
pub fn path_covers(root: &std::path::Path, p: &std::path::Path) -> bool {
    if root == p {
        return true;
    }
    let mut rc = root.components().peekable();
    let mut pc = p.components().peekable();
    loop {
        match (rc.peek(), pc.peek()) {
            (Some(r), Some(pr)) if r == pr => {
                rc.next();
                pc.next();
            }
            (None, Some(_)) => return true, // p continues beneath root
            _ => return false,
        }
    }
}

/// Entry kind as observed (mirror of the engine's `EntryKind` without
/// link payload — history records the fact, not the target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservedKind {
    File,
    Dir,
    Link,
    Other,
}

/// The stored classification of one entry in one run: the serialized
/// Phase 2 category (+ optional subcategory). The classification is
/// stored, never re-derived with later rules (Objective 8); the run's
/// [`ConfigFingerprint`] carries the rules version that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationRef {
    /// Serialized category name (e.g. `"CACHE"`).
    pub category: String,
    /// Serialized subcategory name when the rules produced one.
    pub subcategory: Option<String>,
}

impl ClassificationRef {
    pub fn from_parts(
        category: spacelens_classifier::Category,
        subcategory: Option<spacelens_classifier::Subcategory>,
    ) -> Self {
        fn serde_name<T: serde::Serialize>(v: &T) -> String {
            serde_json::to_value(v)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        }
        ClassificationRef {
            category: serde_name(&category),
            subcategory: subcategory.as_ref().map(serde_name),
        }
    }
}

/// One observed filesystem entry in one run — the normalized snapshot
/// row. Every field is a fact observed during that run; `None` = unknown,
/// never inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedEntry {
    pub path: PathBuf,
    pub kind: ObservedKind,
    /// Logical size (files). `None` for non-files or unknown sizes.
    pub size: Option<u64>,
    /// Handle-proven filesystem object identity `(volume, file id)` where
    /// the platform proved it (Phase 3.2: both platforms for files and
    /// directories); `None` = identity unprovable for this entry.
    pub object: Option<(u64, u64)>,
    pub modified: Option<SystemTime>,
    /// The run's stored classification, when the entry was classifiable.
    pub classification: Option<ClassificationRef>,
    /// Verified content identity — present ONLY where Phase 3/4 actually
    /// produced one (verified duplicate candidates). Never re-hashed to
    /// construct history; never inferred from timestamps.
    pub content_sha256: Option<String>,
    /// Typed observation error (the entry was seen but not fully
    /// readable) — the `ErrorCategoryRef` serialized name.
    pub observation_error: Option<String>,
}

/// The normalized state observed during one run (Objective 4): the full
/// per-path fact set. Entries are canonically ordered by path bytes — an
/// invariant of [`SnapshotBuilder::build`], not a caller convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub run_id: RunId,
    pub entries: Vec<ObservedEntry>,
}

/// Errors from building a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Two entries claimed the same path — the scanner guarantees
    /// path-unique entries; a violation is rejected rather than silently
    /// merged.
    DuplicatePath(PathBuf),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::DuplicatePath(p) => {
                write!(f, "duplicate path in snapshot: {}", p.display())
            }
        }
    }
}

/// Assembles a [`Snapshot`] from observed entries (Objective 4/7): facts
/// copied from the engine model, classification from Phase 2, and verified
/// content identities attached afterwards from the Phase 3/4 reports —
/// no re-hashing, no re-derivation.
#[derive(Debug, Default)]
pub struct SnapshotBuilder {
    entries: Vec<ObservedEntry>,
    /// Verified content identity per path (Phase 3/4 members only).
    content: std::collections::BTreeMap<PathBuf, String>,
}

impl SnapshotBuilder {
    pub fn new() -> Self {
        SnapshotBuilder::default()
    }

    /// Record one observed entry. Classification is optional (the caller
    /// runs the classifier per entry and passes the result).
    pub fn push_entry(
        &mut self,
        entry: &FsEntry,
        classification: Option<ClassificationRef>,
    ) -> &mut Self {
        let kind = match &entry.kind {
            spacelens_engine::EntryKind::File => ObservedKind::File,
            spacelens_engine::EntryKind::Dir => ObservedKind::Dir,
            spacelens_engine::EntryKind::Link(_) => ObservedKind::Link,
            spacelens_engine::EntryKind::Other => ObservedKind::Other,
        };
        let classification = classification.or_else(|| {
            entry.error.map(|e| ClassificationRef {
                category: serde_name_of(&e),
                subcategory: None,
            })
        });
        self.entries.push(ObservedEntry {
            path: entry.path.clone(),
            kind,
            size: (kind == ObservedKind::File).then_some(entry.size),
            object: entry.device.zip(entry.inode),
            modified: entry.modified,
            classification,
            content_sha256: None,
            observation_error: entry.error.map(|e| serde_name_of(&e)),
        });
        self
    }

    /// Attach a VERIFIED content identity (from the Phase 3 duplicate
    /// pipeline's accepted members — bytes the pipeline actually hashed
    /// under its full check sequence). Paths without verified content
    /// stay content-less: unknown stays unknown.
    pub fn set_content(&mut self, path: &Path, sha256_hex: String) -> &mut Self {
        self.content.insert(path.to_path_buf(), sha256_hex);
        self
    }

    /// Finalize: attach content identities and canonically order by path
    /// bytes. Fails on duplicate paths (a scanner invariant violation).
    pub fn build(self, run_id: RunId) -> Result<Snapshot, BuildError> {
        let mut entries = self.entries;
        entries.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_encoded_bytes()
                .cmp(b.path.as_os_str().as_encoded_bytes())
        });
        // Duplicate-path rejection (checked AFTER the canonical sort so a
        // single linear scan suffices).
        for pair in entries.windows(2) {
            if pair[0].path == pair[1].path {
                return Err(BuildError::DuplicatePath(pair[0].path.clone()));
            }
        }
        let content = self.content;
        for entry in &mut entries {
            if let Some(hex) = content.get(&entry.path) {
                entry.content_sha256 = Some(hex.clone());
            }
        }
        Ok(Snapshot { run_id, entries })
    }
}

fn serde_name_of<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

use std::path::Path;
