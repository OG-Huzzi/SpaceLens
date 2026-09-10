//! SpaceLens identity engine — Phase 3 (docs/IDENTITY.md).
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
//!   `PlatformFs::read_content` boundary (links are never followed here; the
//!   eligibility contract rejects everything the observer did not prove to be
//!   a regular file).
//! - **Bounded memory**: the streaming hasher holds a fixed digest buffer;
//!   candidate staging is bounded by explicit hard caps; the report keeps
//!   exact counts and bounded detail.
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

pub use duplicate::{
    DuplicateGroup, DuplicateMember, MemberOrder, StorageAccounting, DUPLICATE_GROUP_DETAIL_CAP,
};
pub use eligibility::{Eligibility, EligibilityStats};
pub use error::{HashError, HashFailure, HashFailureKind};
pub use hash::{ContentHash, HashAlgorithm, HASH_CHUNK_LEN};
pub use pipeline::{
    run_duplicates, DuplicateOptions, DuplicateProgressEvent, DuplicateReport, DuplicateStatus,
    PipelineStats,
};
pub use policy::{ContentReaderFactory, DefaultReaderFactory, MutationPolicy};
