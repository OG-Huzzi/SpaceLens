# SpaceLens — File Identity, Hashing & Duplicate Relationships (Phase 3)

Status: implemented and locally verified on Windows; CI matrix verification
recorded in `progress/PHASE_3_STATUS.md`. This document describes the
implementation as it exists — every claim here is backed by a test.

## What "identity" means (three distinct concepts)

| Concept | Lives in | Meaning |
|---|---|---|
| **Path identity** | `FsEntry::path`, `FsEntry::id` | The scanned location. Two entries at different paths are always distinct entries. |
| **Filesystem object identity** | `FileIdentity` (engine) | Which file object a path refers to: `(device, inode)` + link count. Hard links share it. Proven from an *open handle*, not a path. |
| **Content identity** | `ContentHash` (spacelens-identity) | Which bytes an entry holds: SHA-256 over content. Two different objects can hold identical bytes. |

Example: `A:\Photos\a.jpg`, `B:\Backup\a.jpg`, `C:\Old\a.jpg` may be three
entries (3 path identities), possibly three objects, but **one** content
identity if the bytes match.

## Hash algorithm

- **SHA-256** via the RustCrypto `sha2` crate (v0.10, pure Rust,
  MIT OR Apache-2.0). No custom cryptography is written.
- Chosen because the Phase 0 architecture contract names SHA-256
  (docs/ARCHITECTURE.md) and pure Rust keeps the CI matrix identical.
- Output: `ContentHash` — strongly typed raw 32-byte digest; ordering
  derives from byte-wise comparison. Hex rendering (`as_hex`) exists only
  for IPC/display.
- **Compatibility:** an identity is meaningless without its algorithm. The
  algorithm tag (`HashAlgorithm::Sha256`, `tag() == "sha256"`) travels in
  every `DuplicateGroup`. Changing the algorithm later invalidates all
  persisted identity and must be a visible contract break.
- Digest correctness is pinned by NIST known-answer tests
  (`hash.rs::sha256_known_answers`), including empty input and irregular
  chunk boundaries (streaming ≡ one-shot).

## Eligibility contract (`eligibility.rs`)

Exactly the entries the observer proved to be regular files with *clean*
metadata are candidates:

| Entry | Hashable? | Reason |
|---|---|---|
| `EntryKind::File`, no error | yes | — |
| `EntryKind::File` with `error` | no | `ObservationError` (size unreliable) |
| `EntryKind::Dir` | no | `Directory` |
| `EntryKind::Link(_)` (symlink/junction/reparse) | no | `Link` — **links are recorded, never followed** (Phase 1 rule preserved) |
| `EntryKind::Other` | no | `Special` |

The duplicate layer never crawls the filesystem and never opens files by
itself: content is read only through the engine's `PlatformFs::read_content`
boundary. There is no second traversal mechanism.

## Pipeline (`pipeline.rs`)

```text
Observed FsEntry
      ↓  ingest: eligibility contract
Size grouping             (same size ⇒ candidacy only, never equality)
      ↓  only groups with ≥2 members; singletons cost zero read bytes
Bounded hashing pool      (fixed worker count, bounded job queue)
      ↓
Content identity grouping (same (size, hash) ⇒ same bytes)
      ↓  deterministic ordering
DuplicateReport + typed events + typed failures
```

- **Candidate filtering:** files whose size matches no other eligible file
  are never hashed (`singleton_files` counts them). Same size is *candidacy*,
  never equality — same-size different-content files are hashed and then not
  grouped (`size_groups_without_duplicates` counts them).
- **Concurrency:** a fixed bounded worker pool (default `clamp(cpus, 2, 4)`,
  never thread-per-file), fed pre-grouped jobs through a bounded channel
  (natural backpressure). The queue holds paths, never content. Memory stays
  flat against file count; hard caps (`max_candidates_per_group`,
  `max_group_members_reported`) bound even hostile trees, with overflow
  counted, never silent.

## Mutation policy: file changed during hashing

Policy: **Reject** (`MutationPolicy::Reject` — the default and only
implemented policy). A file is accepted only when ALL hold:

1. the open handle's length equals the size observed at scan time,
2. every chunk is read with per-chunk cancellation checks,
3. total bytes read equals the observed length.

Any mismatch → typed `HashFailureKind::Changed` and the file is excluded.
A vanished file → `Vanished`. Cancellation → the run unwinds to
`DuplicateStatus::Cancelled`; **no partial state is ever published as a
completed report**. Automatic retries are deliberately not performed — a
deterministic single-pass result is explainable and testable.

## Error semantics

A file that cannot be hashed is a **typed per-file failure** — never an
empty hash, never a silent skip, never a false relationship:

| `HashFailureKind` | Meaning |
|---|---|
| `Hash { category }` | open/read failed; engine-categorized cause (permission denied, in use, transient, …) |
| `Changed` | mutation-policy rejection (length mismatch before/after read) |
| `Vanished` | file disappeared between observation and hashing (open or mid-read ENOENT) |
| `Cancelled` | consumer-aborted read (only when not globally cancelled — that unwinds instead) |

Failed hashing removes the file from grouping entirely: it can neither
create nor destroy a duplicate relationship. The report keeps exact failure
counts plus at most 256 detail entries (`failures_truncated` for overflow).

## Hard links / file identity

- Object identity is proven **from the open handle** while content is read
  (Unix: fstat; Windows: `GetFileInformationByHandle`). A path stat taken
  earlier could describe a different object by hash time.
- A group whose members all share one object identity is still reported
  (the content relationship is real) but `recoverable_bytes` is `None`:
  removing one alias of a hard-linked set frees **nothing**.
- `recoverable_bytes` is `size × (distinct_objects − 1)` only when every
  member's identity is provable (`StorageAccounting::Exact`); if the
  platform cannot prove identity, it degrades to the upper bound
  `size × (member_count − 1)` with `StorageAccounting::Estimated` —
  honestly labeled, never silently claimed exact.

## Storage accounting: logical ≠ recoverable

- `logical_duplicate_bytes` = `size × (member_count − 1)`, summed per
  report. This is a fact about *bytes described*, always exact.
- `recoverable_bytes` is a claim about *storage freed by removing one
  member per distinct object*. It is `None` for single-object (hard-link
  alias) groups and `Some(n)` only with the `Exact`/`Estimated` evidence
  described above.
- **Phase 3 never claims "you can reclaim X GB."** Sparse files,
  compression, and copy-on-write are not modeled; those distinctions belong
  to later phases with filesystem-provided evidence.

## Zero-byte files

All zero-byte files share one content identity — correct, but a hostile
tree (a million empty files) would form an enormous group with zero storage
value. Default policy: `group_zero_byte_files: false` — same-size zero-byte
sets are *counted* (`zero_byte_matches_ungrouped`), not grouped. Callers may
opt in; opted-in groups carry `ContentHash::empty()`,
`logical_duplicate_bytes = 0`, `recoverable_bytes = Some(0)` — an honest
zero.

## Deterministic ordering

- Groups: size ascending, then content-hash bytes ascending.
- Members within a group: path bytes ascending (`MemberOrder::PathAscending`,
  locale-independent).
- The representative is the first member in that order.
- The pipeline stage maps are `BTreeMap`s; no HashMap iteration order ever
  reaches output. Repeated runs over the same input produce identical
  logical reports (asserted by tests, including the timestamps-are-not-part-
  of-the-contract distinction).

## Progress

Typed, staged snapshots (`DuplicateProgressSnapshot`) at a caller-set
interval, plus exactly one terminal event (`Completed`/`Cancelled`/
`Failed`). There is deliberately **no percent-complete**: before hashing
finishes the engine cannot honestly estimate remaining work, and byte-based
progress would require reading every byte candidate filtering is trying to
skip.

## API surface (`spacelens.v1.identity.*`)

`run_duplicates`, `DuplicateOptions`, `DuplicateReport`, `DuplicateGroup`,
`DuplicateMember`, `DuplicateStatus`, `DuplicateProgressEvent`,
`PipelineStats`, `EligibilityStats`, `ContentHash`, `HashAlgorithm`,
`HashFailure(Kind)`, `StorageAccounting`, `MutationPolicy`,
`ContentReaderFactory`, `DefaultReaderFactory`. Additive changes only
within `v1` (docs/API_CONTRACTS.md rules).

## Known limitations (honest)

1. Windows object identity uses `dwVolumeSerialNumber` + 32-bit file index;
   on filesystems where the index is not stable, identity degrades to
   `Estimated` (never fabricated).
2. A file that changes *and changes back* within one hash read while
   keeping its length is accepted; window is one sequential pass. Content
   identity still describes bytes that existed; a later scan would correct
   grouping. (Detection of mid-read torn writes needs OS-specific buffering
   guarantees — out of scope, documented here.)
3. Cross-device duplicate detection works, but `device`/`inode` are only
   proven on Unix and via the Windows file-index query; `Estimated`
   accounting is the honest default where they are absent.
4. No hash cache yet: unchanged files re-hash on a later Phase 3 run. The
   persistent cache belongs with persistence (a later phase, per the
   master plan — no SQLite was added here).
