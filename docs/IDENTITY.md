# SpaceLens — File Identity, Hashing & Duplicate Relationships (Phase 3 / 3.1 / 3.2)

Status: Phase 3.2 hardened and verified. CI matrix verification recorded in
`progress/PHASE_3_2_STATUS.md`. This document describes the implementation
as it exists — every claim here is backed by a test, and every limitation
is stated where it applies.

## What "identity" means (three distinct concepts)

| Concept | Lives in | Meaning |
|---|---|---|
| **Path identity** | `FsEntry::path`, `FsEntry::id` | The scanned location. Two entries at different paths are always distinct entries. |
| **Filesystem object identity** | `FileIdentity` (engine) | Which file object a path refers to. Proven from an *open handle* (or a query-only handle at scan time), never from a path string. Hard links share it. |
| **Content identity** | `ContentHash` (spacelens-identity) | Which bytes an entry holds: SHA-256 over content. Two different objects can hold identical bytes. |

Example: `A:\Photos\a.jpg`, `B:\Backup\a.jpg`, `C:\Old\a.jpg` may be three
entries (3 path identities), possibly three objects, but **one** content
identity if the bytes match.

## Object identity per platform (Phase 3.2)

| Platform | Identity | Captured how | Notes |
|---|---|---|---|
| **Unix** | `(st_dev, st_ino)` | path-stat (`lstat`) at scan time; `fstat` on the open handle at hash time | `st_ino` reuse after deletion is a documented residual (below); the mtime bracket backstops it |
| **Windows** | `(volume serial, 128-bit file id)` | query-only handle at scan time (`CreateFileW` `FILE_READ_ATTRIBUTES`, share-all, `OPEN_REPARSE_POINT`, `BACKUP_SEMANTICS`) reading `FILE_ID_INFO`; the same derivation from the open handle at hash time | On NTFS the file id embeds the MFT record reference *including its sequence number*, which increments every time a record is freed and reused — reuse-resistant. Filesystems without `FileIdInfo` fall back to the 64-bit `BY_HANDLE_FILE_INFORMATION` index; where even that fails, identity is `None` — never fabricated |

- `FsEntry.device` / `FsEntry.inode` carry `(volume serial, file id low 64)`;
  `FsEntry.file_id_hi` carries the high 64 bits where the platform proved a
  wider identifier (non-zero on ReFS-class filesystems; always compared when
  both sides prove it).
- Windows scan-time capture opens every regular file and directory once with
  query-only access — no data read, no write, no target traversal
  (`OPEN_REPARSE_POINT` observes a reparse point itself, matching the
  record-only link policy). Failure degrades to `None` honestly.
- **The identity layer's question is exactly:** "is the object I am hashing
  the same filesystem object that was observed during the scan?" — answered
  by comparing proven identities, never by "does this path still exist?" or
  "are the bytes identical?". Content equality is never proof of object
  identity.

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
| `EntryKind::Link(_)` (symlink/junction/reparse) | no | `Link` — **links are recorded, never followed** |
| `EntryKind::Other` | no | `Special` |

The duplicate layer never crawls the filesystem and never opens files by
itself: content is read only through the engine's `PlatformFs::read_content`
boundary. There is no second traversal mechanism.

## Content-open safety: links are never followed, at scan *and* at hash

Phase 3 guaranteed "links are never followed" at *scan* time only: the
scanner records links without recursing. Phase 3.1 closed the second window —
**a path that became a link after the scan is refused at hash time too** —
and Phase 3.2 extended that refusal to the **whole path chain**:

- **Final component, Unix:** the content open uses
  `open(O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)` (`libc` constants — raw values
  differ per Unix flavor, so hand-rolled constants would silently apply the
  wrong flags on an unlisted target). A final component that is now a
  symlink fails with `ELOOP` → typed refusal; its target is never touched.
  `O_NONBLOCK` means a FIFO that replaced the file cannot block a hashing
  worker; the post-open fstat then rejects it as a non-regular object.
- **Final component, Windows:** the content open uses
  `FILE_FLAG_OPEN_REPARSE_POINT` (+ the required `FILE_FLAG_BACKUP_SEMANTICS`),
  which opens a reparse point *itself* rather than traversing to its target.
  The handle is immediately inspected: a reparse attribute refuses the open
  (typed refusal), a directory attribute refuses it as a non-regular object.
- **Every ancestor component (both platforms, Phase 3.2):** before the final
  open, each ancestor prefix of the observed path is opened no-follow
  (`O_NOFOLLOW|O_DIRECTORY` on Unix; `OPEN_REPARSE_POINT|BACKUP_SEMANTICS` +
  attribute inspection on Windows). A directory anywhere in the chain that
  became a symlink/junction/reparse point is refused (`UnexpectedLink` →
  typed failure) — a hostile intermediate swap cannot silently redirect
  resolution to another object tree. The scanner never descends into links
  (`SymlinkPolicy::RecordOnly`), so a legitimately observed path can never
  contain a link ancestor and this check cannot false-positive on
  engine-produced input. (A future scan policy that legitimately traverses
  directory links would need to extend this contract; none exists yet.)
  An ancestor that cannot be opened at all (ACL) is *not* treated as a link:
  the final open is authoritative.

The residual exposure between independent per-component checks and the final
open (a swap landing exactly inside that window) cannot manufacture a false
relationship: the authoritative proof is the **final object identity
comparison** against the scan-time observation (next section), which holds
for whatever object the open actually reached.

## Observed object vs opened object (Phase 3.1 / 3.2)

The pipeline receives an `FsEntry` observed earlier and later opens the same
path. Two identities are compared:

```text
observation-time identity   — recorded by the scanner
        (Unix: st_dev/st_ino via lstat; Windows: volume serial + 128-bit
         FILE_ID_INFO.FileId via a query-only handle)
        vs
handle-proven identity      — proven from the open handle at hash time
        (Unix: fstat; Windows: FILE_ID_INFO from the content handle — the
         same derivation as scan time, so comparisons are like-for-like)
```

- **Both provable and equal** → the hashed object is the observed object.
  (On Windows this now holds for every file/directory the scanner could
  stat — the Phase 3.1 "Windows runs degraded" limitation is GONE.)
- **Both provable and different** → the path now names a different object:
  typed `HashFailureKind::Replaced`; the impostor is never hashed into the
  observed file's identity. Hard links are NOT replacements — aliases share
  object identity, so they pass and group as the alias relationship they
  are. This also covers **same-content replacement**: a replacement file
  created while the original still exists (rename-over) has a distinct
  identity by construction; a delete+recreate impostor on NTFS gets a new
  MFT record sequence number, so its identity differs too.
- **High bits:** where both sides prove a >64-bit file identifier
  (`file_id_hi`, ReFS-class), the high bits must also agree — compared only
  on shared evidence, never fabricated.
- **Either side unprovable** → the comparison degrades honestly: the file is
  hashed under the remaining checks (ancestor-chain guard, length/mtime/
  change-time brackets, content grouping). Identity is never fabricated.

The published member `object_id` is the **handle-proven** identity (the
object the digest was actually computed from — hard-link accounting depends
on this), falling back to the observation identity when the handle could
not prove one.

## Mutation policy: file changed during hashing

Policy: **Reject** (`MutationPolicy::Reject` — the default and only
implemented policy). A digest is published only when the read is proven to
describe one stable state of the object. The full check sequence, all from
the open handle (fstat semantics — never a second path resolution):

1. **observed vs opened identity** (above) — else typed `Replaced`;
2. **pre-read length == observed size** — else typed `Changed`;
3. **scan→open mtime bracket**: the handle's modification stamp equals the
   scanner's observation of it, where both sides could prove one — else
   `Changed`. A same-content replacement (rename-over, delete+recreate)
   carries a fresh mtime, so this bracket catches replacements that
   identity evidence alone might miss on filesystems without
   reuse-resistant file ids;
4. **scan→open change bracket**: the handle's metadata-change stamp
   (`st_ctime` / NTFS `ChangeTime`) equals the scanner's observation of it,
   where both sides could prove one — else `Changed`. This catches the
   same-length rewrite that happens *between* scan and open and preserves
   its mtime;
5. every chunk is read with per-chunk cancellation checks; interrupted
   reads are retried inside the platform layer;
6. **total bytes read == observed size** — else `Changed`;
7. **post-read length == pre-read length** — else `Changed`;
8. **post-read change stamp == pre-read change stamp** (where the platform
   maintains one) — else `Changed`. This catches the same-length rewrite
   *during* the read — the A→B same-length swap that a length-only check
   can never see.

The change stamp (`st_ctime`/`ChangeTime`) moves on content rewrites even
when mtime is deliberately preserved (`utimensat` bumps ctime) and cannot
be set independently from userspace — it is the strongest mid-read signal
each OS offers. Where the filesystem does not maintain it, the check
degrades to length + mtime + content grouping and the residual window is
the documented limitation below.

Any rejection → the file is excluded from grouping entirely (it can neither
create nor destroy a relationship) with a typed failure. A vanished file →
`Vanished`. Cancellation → the run unwinds to `DuplicateStatus::Cancelled`;
**no partial state is ever published as a completed report**. Automatic
retries are deliberately not performed — a deterministic single-pass result
is explainable and testable.

## Boundedness contract (Phase 3.1, Contract A)

Phase 3's staging held *every* eligible entry in a
`BTreeMap<size, Vec<Member>>` with only a per-group cap — many distinct
sizes scaled memory with input. Phase 3.1 replaces it with **streaming
ingest under two global caps**:

- `max_tracked_size_groups` (default 100,000): the number of *distinct
  sizes* the staging map may track,
- `max_tracked_candidates` (default 1,000,000): the total number of staged
  member records across all groups.

Every member beyond a cap increments an exact counter
(`candidatesSkippedSizeTracking` / `candidatesSkippedGlobalCap` /
`candidatesSkippedByCap`), and any capped run reports
**`DuplicateStatus::CompletedWithLimits`** — a `Completed` report is only
ever produced when nothing was skipped. Zero-byte files under the default
policy are counted with one counter, never staged.

Secondary buffers are bounded by the same caps:

- the job channel is bounded (`threads × 4`) with natural backpressure,
- the result buffer holds one record per *accepted* candidate — bounded by
  the staging caps,
- failure detail is capped at 256 entries with exact overflow counts,
- group member detail is capped at 64 with exact `member_count`.

A counting-allocator test proves the property mechanically: ingesting
100,000 hostile distinct-size entries under low caps peaks at the same
staging-memory order as 10,000 entries (and under a few MB absolute),
where uncapped staging would have grown ~10 MB per 100k records.

The deterministic job order (size ascending, then first-member path bytes)
makes cap decisions order-independent: the same input in any observation
order produces byte-identical reports, capped or not.

## Pipeline (`pipeline.rs`)

```text
Observed FsEntry  — streamed one at a time, never fully buffered
      ↓  ingest: eligibility contract + global caps (exact skip counters)
Size grouping             (same size ⇒ candidacy only, never equality)
      ↓  only groups with ≥2 members; singletons cost zero read bytes
Bounded hashing pool      (fixed worker count, bounded job queue)
      ↓  7-check mutation/identity policy (above)
Content identity grouping (same (size, hash) ⇒ same bytes)
      ↓  deterministic ordering
DuplicateReport + typed events + typed failures + CompletedWithLimits
```

- **Concurrency:** a fixed bounded worker pool (default `clamp(cpus, 2, 4)`,
  never thread-per-file), fed pre-grouped jobs through a bounded channel
  (natural backpressure). The queue holds paths, never content.
- **Candidate filtering:** files whose size matches no other eligible file
  are never hashed (`singleton_files` counts them). Same size is
  *candidacy*, never equality — same-size different-content files are
  hashed and then not grouped (`size_groups_without_duplicates` counts
  them).

## Error semantics

A file that cannot be hashed is a **typed per-file failure** — never an
empty hash, never a silent skip, never a false relationship:

| `HashFailureKind` | Meaning |
|---|---|
| `Hash { category }` | open/read failed; engine-categorized cause (permission denied, in use, transient, …) |
| `Changed` | the path no longer names a regular file (link/junction/directory/FIFO replaced it), or a mutation-policy rejection (length/bytes/change-stamp mismatch) |
| `Replaced` | the opened object is not the object the scanner observed (identity disagreement, where both are provable — Unix) |
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
sets are *counted* (`zero_byte_matches_ungrouped`, one counter, no staged
records), not grouped. Callers may opt in; opted-in groups carry
`ContentHash::empty()`, `logical_duplicate_bytes = 0`,
`recoverable_bytes = Some(0)` — an honest zero.

## Deterministic ordering

- Groups: size ascending, then content-hash bytes ascending.
- Members within a group: path bytes ascending (`MemberOrder::PathAscending`,
  locale-independent).
- The representative is the first member in that order.
- All stage maps are `BTreeMap`s; job order is normalized before dispatch;
  cap decisions are order-independent. No HashMap iteration order, thread
  scheduling, or OS enumeration order ever reaches output. Repeated runs
  over the same input produce identical logical reports (asserted by
  tests, including under shuffled observation order and active caps).

## Progress

Typed, staged snapshots (`DuplicateProgressSnapshot`) at a caller-set
interval, plus exactly one terminal event (`Completed`/`Cancelled`/
`Failed`). There is deliberately **no percent-complete**: before hashing
finishes the engine cannot honestly estimate remaining work, and byte-based
progress would require reading every byte candidate filtering is trying to
skip.

## API surface (`spacelens.v1.identity.*`)

`run_duplicates`, `DuplicateOptions`, `DuplicateReport`, `DuplicateGroup`,
`DuplicateMember`, `DuplicateStatus` (incl. `CompletedWithLimits`),
`DuplicateProgressEvent`, `PipelineStats` (incl. the exact skip counters),
`EligibilityStats`, `ContentHash`, `HashAlgorithm`, `HashFailure(Kind)`
(incl. `Replaced`), `StorageAccounting`, `MutationPolicy`,
`ContentReaderFactory`, `DefaultReaderFactory`. Additive changes only
within `v1` (docs/API_CONTRACTS.md rules).

## Known limitations (honest)

1. **Mid-read torn writes where the FS does not maintain a change stamp:**
   a same-length rewrite during the read is caught by the change-time
   bracket (check 8) only where the platform provides `st_ctime`/NTFS
   `ChangeTime`. On filesystems without it the digest may describe a
   mixture of two states of a file that was rewritten mid-read *while
   preserving both length and mtime*. The window is one sequential read;
   a later scan corrects grouping. (Detection of arbitrary torn writes
   needs OS snapshot semantics — out of scope, documented here.)
2. **Identity reuse without sequence numbers:** Unix `st_ino` and the
   Windows 64-bit fallback index can be reused after deletion (ext4
   reallocates freed inodes immediately; FAT-class volumes have no
   generation numbers at all). A delete+recreate impostor with identical
   bytes and a *forged* original mtime could then pass the identity and
   mtime brackets — it would need deliberate `utimensat`/`SetFileTime`
   forgery. NTFS is not affected (the MFT sequence number is part of the
   file id). Where the impostor does not forge mtime, the scan→open mtime
   bracket rejects it; where identity is unavailable entirely, the run is
   visible as `Estimated` accounting and the ancestor-chain guard still
   applies.
3. **Windows filesystems without `FILE_ID_INFO`** (FAT/exFAT-class): the
   64-bit `BY_HANDLE_FILE_INFORMATION` index is the identity fallback; it
   is not guaranteed stable/unique on all such filesystems. Identity
   degrades exactly as far as the OS proves — comparisons run only on
   shared evidence, accounting degrades to `Estimated`, never fabricated.
4. **Swap window between per-component checks:** the ancestor-chain guard
   and the final open are independent opens; a swap landing exactly
   between them is not excluded *structurally*. It cannot produce a false
   relationship — the final object identity comparison is the
   authoritative proof — and in degraded mode (identity unavailable at
   scan time) the residual is documented here rather than claimed away.
   True handle-relative traversal (POSIX `openat(dirfd)` chains, NT native
   `NtOpenFile` with root-directory handles) is the airtight form and is
   deliberately not used: the identity comparison already answers the
   question the walk would answer, with less platform surface.
5. **Scan-time identity cost on Windows:** every scanned file/directory
   costs one extra query-only handle open (no data access, share-all).
   Measured as part of the perf smokes; no timing gate regressed.
6. **No hash cache yet:** unchanged files re-hash on a later run. The
   persistent cache belongs with persistence (a later phase, per the
   master plan — no SQLite was added here).
