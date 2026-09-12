//! SpaceLens identity engine — Phase 3 (docs/IDENTITY.md), hardened by
//! Phase 3.1 (mutation-consistency, observed-object verification,
//! no-follow content opens, globally bounded staging).
//!
//! The RELATE layer between the Phase 2 classifier and future
//! recommendation/safety phases: which filesystem entries represent the same
//! underlying content.
//!
//! Three distinct concepts, never conflated:
//! - **Path identity** — the scanned location (`FsEntry::path` / `FsEntry::id`).
//! - **Filesystem object identity** — which file object a path refers to
//!   ([`spacelens_engine::FileIdentity`]; hard links share it).
//! - **Content identity** — which bytes an entry holds ([`hash::ContentHash`]).
//!
//! Design contracts honored here:
//! - **Content identity derives from bytes** (SHA-256), never from names,
//!   paths, or metadata. Same size is only a candidacy filter.
//! - **The duplicate layer never crawls the filesystem**: it consumes
//!   observed `FsEntry` values and reads content only through the engine's
//!   `PlatformFs::read_content` boundary, which opens observed paths with
//!   no-follow semantics — links are never followed, even links that
//!   replaced an observed file after the scan.
//! - **Observed object == hashed object** where the platform can prove it
//!   (Unix `st_dev`/`st_ino` at scan time vs. handle-proven identity at
//!   hash time). A path that came to name a different object is typed
//!   `Replaced`, never silently hashed. Windows path-stats cannot prove
//!   observation-time identity (std limitation, documented); the check
//!   degrades honestly rather than fabricating identity.
//! - **Mutation-safe digests**: a published hash describes one stable state
//!   of the object — bracketed by handle-proven length + change timestamps
//!   before and after the read, and total bytes read.
//! - **Bounded memory, honestly**: ingest staging is bounded by *global*
//!   caps (distinct sizes and total records), the job channel is bounded,
//!   and results are bounded by the staging caps; every capped record is
//!   counted and a capped run reports `CompletedWithLimits` — never a
//!   silently truncated `Completed`.
//! - **Deterministic**: stable sort orders for groups, members, and evidence.
//! - **Typed failure**: a file that cannot be hashed is a typed per-file
//!   error — never an empty hash, never a false relationship.
//! - **Honest storage accounting**: logical duplicate bytes are reported
//!   exactly; *recoverable* bytes are reported only where object identity
//!   proves that removing one member would actually free the storage.
//!
//! Contract namespace: `spacelens.v1.identity.*` (docs/API_CONTRACTS.md).

pub mod duplicate;
pub mod eligibility;
pub mod error;
pub mod hash;
pub mod pipeline;
pub mod policy;
pub mod relationships;

pub use duplicate::{
    DuplicateGroup, DuplicateMember, MemberOrder, StorageAccounting, DUPLICATE_GROUP_DETAIL_CAP,
};
pub use eligibility::{Eligibility, EligibilityStats};
pub use error::{HashError, HashFailure, HashFailureKind};
pub use hash::{ContentHash, HashAlgorithm, HASH_CHUNK_LEN};
pub use pipeline::{
    run_duplicates, DuplicateOptions, DuplicateProgressEvent, DuplicateProgressSnapshot,
    DuplicateReport, DuplicateStatus, PipelineStats,
};
pub use policy::{
    ContentReaderFactory, DefaultReaderFactory, MutationPolicy, DEFAULT_MAX_CANDIDATES_PER_GROUP,
    DEFAULT_MAX_GROUP_MEMBERS_REPORTED, DEFAULT_MAX_TRACKED_CANDIDATES,
    DEFAULT_MAX_TRACKED_SIZE_GROUPS,
};
pub use relationships::{
    derive_relationships, AliasSet, ContentRef, Evidence, MemberRef, ObjectRef, Relationship,
    RelationshipIndex, RelationshipKind, RelationshipOptions, RelationshipReport,
    RelationshipStats, RelationshipStatus, Undetermined, UndeterminedDetail,
};
