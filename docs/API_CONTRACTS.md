# CoreSight — IPC / API Contracts

Status: Phase 0. Versioned contract (`v1`). Breaking changes require a version
bump and a migration note. The frontend must never need filesystem knowledge.

## Transport

Tauri 2 commands (request/response) + events (Rust→UI streams).
All payloads JSON, `camelCase`, sizes in bytes (integers), timestamps RFC 3339 UTC.
API namespace: `coresight.v1.*`.

## Commands (UI → Rust)

- `coresight.v1.startScan { driveId, fullRescan? } → { scanId }`
- `coresight.v1.cancelScan { scanId } → { cancelled: true }`
- `coresight.v1.getDriveSummary { driveId } → DriveSummary`
  (totals, exclusions note, last scan, snapshot count)
- `coresight.v1.getCategories { scanId, parentCategory? } → Category[]`
  `{ id, name, bytes, shareOfParent, deltaSinceLast?, itemCount }` — no paths.
- `coresight.v1.getOpportunities { scanId } → Opportunity[]`
  `{ id, tier: safe|review, title, whatItIs, whyRecommended, bytes,
     recoverableBytes, staysUntouched, consequence, preview }`
- `coresight.v1.previewPlan { opportunityIds } → PlanPreview`
  (exact items-or-rules, bytes, destination, undo path, warnings)
- `coresight.v1.confirmPlan { planId, ackedWarnings } → { accepted, rejected[] }`
  (safety validation runs HERE, synchronously, before any action)
- `coresight.v1.executePlan { planId } → { results, freedBytes, leftovers[] }`
- `coresight.v1.getHistory { driveId } → SnapshotDelta[]`
- `coresight.v1.listDrives → Drive[]` (connected + remembered-offline)

## Events (Rust → UI)

- `coresight.v1.scanProgress { scanId, filesSeen, bytesSeen, currentCategory?, etaSec? }`
  throttled ≤4/sec.
- `coresight.v1.scanComplete { scanId, status, summary }`
- `coresight.v1.planVerified { planId, freedBytes, leftovers[] }`

## Errors

Typed everywhere: `{ code, message, detail? }`. Stable codes, e.g.
`SCAN_IN_PROGRESS`, `DRIVE_OFFLINE`, `PLAN_REJECTED_UNSAFE { reasons[] }`,
`NOTHING_TO_DO`, `PERMISSION_GAP { unscannedPathsCount }`, `DB_CORRUPT_RESCANNED`.
UI renders `message` verbatim for known codes (copy reviewed with safety doc);
unknown codes show a generic safe fallback, never a stack trace.

## Identity contracts (Phase 3 / 3.1)

Namespace: `coresight.v1.identity.*` (engine-side in `crates/coresight-identity`;
IPC payload shapes follow the same camelCase + bytes-as-integers rules as
above). Content identity = SHA-256 (`HashAlgorithm::Sha256`, tag `"sha256"`);
a content identity is meaningless without its algorithm tag. Duplicate groups
report `logicalDuplicateBytes` exactly and `recoverableBytes` only with
`Exact`/`Estimated` accounting evidence — see docs/IDENTITY.md for the full
semantics (hard links, mutation policy, zero-byte policy, deterministic
ordering, observed-vs-opened object verification, no-follow content opens).

Phase 3.1 contract additions (all additive within `v1`):

- `DuplicateStatus` adds `CompletedWithLimits`: any candidate skipped by a
  global bound (distinct-size tracking cap, global record cap, per-group
  cap overflow is reported via counters) makes the report incomplete —
  `Completed` is only ever emitted when nothing was skipped. The exact
  skip counts travel in `PipelineStats`
  (`candidatesSkippedByCap`, `candidatesSkippedSizeTracking`,
  `candidatesSkippedGlobalCap`, `candidatesUntrackedTotal`).
- `HashFailureKind` adds `Replaced`: the opened object is not the object
  the scanner observed (provable on Unix via st_dev/st_ino at scan time;
  Windows path-stats cannot prove observation-side identity and degrade
  honestly — documented limitation, never fabricated).
- `FsEntry` adds `changed` (observation-time metadata-change stamp, Unix
  `st_ctime`): used by the scan→open mutation bracket; `null` where
  unprovable.
- `FsEntry` adds `fileIdHi` (Phase 3.2): high 64 bits of a >64-bit file
  identifier (Windows `FILE_ID_INFO`, non-zero on ReFS-class filesystems);
  compared by the identity layer only where both scan and hash sides
  proved it. Additive; `null` on Unix and where the OS proves no wider
  identifier.
- Windows object identity (Phase 3.2): scan-time `(volumeSerial,
  128-bit fileId)` captured via a query-only handle (`FILE_ID_INFO`),
  removing the Phase 3.1 degraded-identity limitation on Windows. On NTFS
  the file id embeds the MFT record sequence number, so delete+recreate
  impostors are detectable.

No IPC command surfaces these yet; the types are the contract for
future phases and are covered by engine tests.

## Relationship contracts (Phase 4)

Namespace: `coresight.v1.relationship.*` (engine-side in
`crates/coresight-identity::relationships`; payload shapes follow the same
camelCase + bytes-as-integers rules as above). The relationship layer is a
PURE derivation over one verified duplicate-pipeline run — no I/O, no
deletion, no recommendations. See docs/RELATIONSHIPS.md for full semantics.

- `RelationshipKind`: `hardLinkAlias` (same filesystem object, multiple
  paths — no second copy) | `contentDuplicate` (distinct objects,
  byte-identical content). Same size / same name / same path alone never
  produce a relationship.
- `Evidence` (per relationship, canonical order): `OBJECT_IDENTITY_EQUAL` |
  `CONTENT_HASH_EQUAL` | `SIZE_EQUAL` — categorical proof, never a
  confidence score.
- `Relationship`: deterministic id (identity-derived, no counters), kind,
  exact member count (paths), distinct objects where provable, members
  (scan-scoped entry refs + paths, no filesystem records copied), alias
  sets inside content duplicates, logical vs recoverable bytes
  (conservative; `null` where unprovable), accounting
  (`exact`|`estimated`), detail-truncation flag.
- `RelationshipReport`: reused `DuplicateStatus` (incl.
  `completedWithLimits`), relationships in canonical order, exact
  truncation counter, undetermined summary (typed Phase 3 failure kinds +
  exact not-examined counts), run provenance (timestamps).
- Content identity published as `sha256Hex` + `algorithm` tag; raw digests
  and platform structs never cross the boundary.

No IPC command surfaces these yet; the types are the contract for future
phases (storage search, history, cleanup recommendations — not built here)
and are covered by engine tests.

## History contracts (Phase 5)

Namespace: `coresight.v1.history.*` (engine-side in
`crates/coresight-history`; payload shapes follow the same camelCase +
bytes-as-integers rules as above). System Memory persists one
normalized snapshot per committed run and derives typed, evidence-backed
change events on demand — see docs/HISTORY.md for full semantics.

- `RunRecord`: stable run id, timestamps, canonical roots (scope),
  platform, `ConfigFingerprint` (observation model / classifier schema +
  rules version / hash algorithm / schemas), reused terminal statuses
  (`RUNNING`, `COMPLETED`, `COMPLETED_WITH_LIMITS`, `CANCELLED`,
  `FAILED`), fixed-width counts. A run that is not
  `COMPLETED`/`COMPLETED_WITH_LIMITS` never serves as a complete
  comparison baseline.
- `Snapshot`: one `ObservedEntry` row per observed path per run (path,
  kind, size, proven object identity, modified, stored classification,
  verified content hash when Phase 3/4 produced one, observation error).
  Unknown stays `null` — never inferred.
- `ChangeSet` (pure comparison of two committed runs): completeness
  (`COMPLETE`/`PARTIAL`), `configVersionsDiffer` flag, deterministic
  event ids, typed `EventKind`s with categorical `EventEvidence`
  (created/deleted require full-scope runs — a partial run can never
  produce mass deletions), per-kind counts, exact truncation counter.
- Persistence extends the existing SQLite schema (forward-only migration
  v2: `scan_runs`, `observations`, `relationship_obs`,
  `relationship_members`); the comparison engine itself is pure and
  database-free.
- Retention: deterministic policy (always-keep newest baseline, optional
  max runs/age) with an observable removal report; queries are bounded
  (`QueryLimits`) with explicit truncation.

No IPC command surfaces these yet; the types are the contract for future
phases and are covered by engine tests.

## Unified system model (Phase 6.3; internal only)

`coresight-system-model` is a Rust in-memory correlation API, not a `v1`
command/event or frontend wire contract. Phase 6.3 adds no IPC surface.
Its serde form is internal canonical graph data:
derived indexes are skipped/rebuilt, malformed graph/evidence semantics are
rejected, and candidate actions are always inert and unauthorized. Paths and
evidence remain lossless and provenance-bounded; source-specific resolution
semantics are documented in `docs/SYSTEM_MODEL.md`.

## Application/system snapshots (Phase 6.4; internal only)

Persistence for the system model is a **Rust-side, engine-internal API**
(`coresight-history`), not a `v1` command/event: still no IPC surface, no
frontend access, and the frontend never opens the DB (docs/DATABASE.md).

The API is typed and bounded, and returns domain facts — never SQLite
rows:

```text
commit_system_snapshot(run_id, input, app_facts, inventory, footprint)
has_system_snapshot(run_id) -> bool
list_system_snapshots(limits) -> [SnapshotSummary]
load_system_snapshot(run_id, limits) -> Option<SystemSnapshotInput>
load_snapshot_applications(run_id, limits) -> Option<[ApplicationRecord]>
rebuild_system_model(run_id, limits, model_limits) -> Option<SystemModel>
```

Semantics: `None` means no snapshot was recorded for that run — absence,
never an empty snapshot. Every load is per-run and capped by
`QueryLimits`; commit is one transaction and re-committing a run is
idempotent. A capped load is REPORTED
(`is_load_truncated()` / `load_truncated_sections`), and
`rebuild_system_model` fails closed with `StoreError::SnapshotBounded`
rather than building a model from a prefix of the stored facts. Stored
evidence is re-clamped to the Phase 6.2 ceilings on
reload, application ids are re-verified against normalized
`(name, publisher)`, and the rebuilt model passes the same
`check_invariants` as a fresh build. Errors are `StoreError`
(`Corrupt { table, column, run_id, detail }`, `SnapshotBounded`,
`SchemaTooNew`, `UnknownRun`, …) — never a defaulted value.

Phase 6.4.1 additions:

- Values are range-checked **before** the snapshot transaction, so an
  unrepresentable size/counter/ordinal rejects the commit and preserves
  the previous snapshot — never wrapped or narrowed.
- Footprint candidates are reconciled by a commutative, deterministic
  rule at both commit and load (stronger confidence, then the larger
  distinct-evidence count, then canonical content), so stored and
  reloaded facts are identical; a footprint attributed to a foreign
  application is rejected.
- `ApplicationId` uses a length-prefixed encoding (injective at component
  boundaries). Ids persisted under the Phase 6.4 encoding are re-keyed by
  the v5→v6 migration, per stored fact rather than globally.

Phase 6.4.2 additions:

- **Query limits are validated, never narrowed.** One shared policy for
  every query API: `QueryLimits::MAX_SUPPORTED` is `i64::MAX - 1`, and
  `to_sql_limit()` / `to_probe_sql_limit()` either convert exactly or fail
  with `StoreError::LimitOutOfRange`. A `usize::MAX` request is rejected
  because `usize::MAX as i64` is `-1`, which SQLite reads as UNBOUNDED.
  The domain deliberately stops one below `i64::MAX` so `limit + 1`
  probing stays exact at every accepted value.
- **Identity is validated at commit, before the transaction opens**, using
  the same verifier the loader uses. A forged `ApplicationId` supplied
  consistently across all parallel inputs used to commit and then fail on
  reload; it is now refused. Whitespace-only names are rejected because
  they normalize to an empty component.

Phase 6.4.3 additions:

- **Legacy negative numerics round-trip, and the ambiguity is documented.**
  `scan_runs.rel_truncated` was written with an unchecked `u64 as i64`, so
  a value at or above `2^63` is a NEGATIVE stored integer. Reads
  reconstruct it exactly instead of clamping to 0 (which erased the
  truncation signal). A negative value is NOT claimed to be detectable as
  legacy-vs-tampered — that distinction is not in the format.
- **The limit bound is validated before the existence probe.** An
  unsupported limit is refused even when the run has no snapshot, so an
  invalid bound can never masquerade as absence.
- **Footprint evidence is normalized on read** to the documented set
  contract, AFTER the section cap is applied (so a genuine overflow still
  reports truncation). Tampered duplicate rows are normalized away.
- **commit_run performs a true preflight**: every numeric conversion and
  caller-supplied timestamp is computed BEFORE the transaction opens.
  Timestamp errors name the actual field (started_at / completed_at /
  modified).
- **RetentionPolicy::keep_latest must be >= 1**, else
  `StoreError::InvalidRetentionPolicy` before any row is read or deleted.

Phase 6.4.4 additions:

- **Numeric preflight is complete.** Every caller-controlled unsigned value
  reaching a signed SQLite INTEGER is checked BEFORE the transaction opens:
  all nine RunCounts fields, ObservedEntry.size, relationship size /
  member_count / recoverable_bytes, the relationship truncation counter,
  and every caller timestamp. Invalid input is typed as
  StoreError.InvalidRunCount { table, column, value } - deliberately
  distinct from persisted corruption, so callers are not sent hunting for
  database damage that does not exist. Valid boundaries (0, i64::MAX,
  None) still round-trip, and full-width ObjectIdentity bit patterns remain
  intentionally unchecked.
- **Timestamp errors name the owning table.** scan_runs.started_at,
  scan_runs.completed_at, and observations.modified each report their own
  table and column; previously a modified error named scan_runs.
- **Relationship report metadata has one nullability contract.**
  rel_status and rel_truncated are written as a pair from the same report,
  so (absent, absent) and (present, present) are valid and the two mixed
  combinations are rejected as inconsistent metadata. A present status
  never causes a missing count to be inferred as zero, so "no report
  recorded" and "recorded with zero truncations" stay distinct.

Phase 6.4.5 additions:

- **Timestamps complete the caller-input error contract.** An
  unrepresentable caller timestamp (started_at / completed_at /
  observations.modified, and the store's own clock) is rejected BEFORE
  the write transaction opens with the typed
  StoreError.InvalidTimestamp { table, column, run_id, nanos } — the
  caller-input half of the contract, carrying the owning table/column,
  the run where known, and the offending nanosecond count. Invalid
  caller timestamps never use the persisted-corruption variant, and the
  rejection leaves existing committed state byte-for-byte unchanged. The
  pre-epoch convention (stored as 0) is preserved; future values that
  cannot fit the signed nanosecond domain are refused, never wrapped,
  clamped, or panicked over — the store clock now propagates a typed
  error instead of panicking on a defective platform clock.
- **Persisted timestamps decode by one documented compatibility
  policy.** Non-negative stored values round-trip exactly (0 is the
  pre-epoch convention; i64::MAX is the last representable nanosecond,
  ~2262 CE). A NEGATIVE value is a legitimate legacy bit pattern — the
  pre-6.4.2 writer's unchecked u128-as-i64 wrap of a far-future time —
  and is reconstructed as the stored bits read unsigned, exactly
  recovering the original for any time below ~year 2554. It is neither
  rejected (a supported writer produced it) nor clamped to the epoch
  (which silently presented it as the plausible date 1970-01-01). A
  negative is ambiguous by construction — legacy-wrapped and tampered
  values are indistinguishable — and that limitation is documented, not
  claimed as detection. The policy is proven by direct SQL tampering
  across get_run, list_runs, load_run_snapshot, history_for_path and
  relationship_history, and by a genuine v2 fixture migrated through the
  real v2 → v6 chain.
- **Legacy relationship rows survive the migration without fabricated
  completeness.** The relationship tables predate v4; the metadata
  columns arrived with v4. A genuine pre-v4 database carrying
  relationship_obs / relationship_members rows migrates through the real
  chain to v6 with the rows retained, both metadata columns still NULL,
  and no report manufactured by load_run_snapshot. The retained facts
  stay queryable through relationship_history. (NULL, NULL) means no
  status/truncation metadata was recorded — not that no relationship
  rows ever existed. Schema v6 remains unchanged; no migration was
  added.

Phase 6.4.6 additions:

- **Caller numerics are invalid INPUT, not corruption.** An
  unrepresentable caller-supplied application-snapshot value — an artifact
  `size`, an application `estimated_size_bytes`, an `Inventory`
  `records_truncated`/`records_rejected` counter, or a
  `FootprintReport` counter — is rejected with

  ```text
  StoreError.InvalidSnapshotInput { table, column, run_id, value }
  ```

  and NEVER with `StoreError.Corrupt`. The value came from the caller, so
  it is not evidence of database damage; reporting it as corruption would
  point an operator at damage that does not exist. `value` is a `u128`,
  wide enough to carry both a `u64` domain value and a `usize` ordinal
  without narrowing, so a rejected `u64::MAX` is reported exactly.
  `StoreError.Corrupt` is unchanged for genuinely malformed or impossible
  PERSISTED data (strict unsigned decode, identity decode, unknown enums,
  parallel-vector mismatch, footprint/app-attribution mismatch,
  inventory-vs-facts mismatch, footprint-report mismatch, integrity check).
- **The rejection is a PREFLIGHT, before the transaction opens.** Every
  fallible conversion — sizes and every ordinal domain, including nested
  collections (provenance, views, install roots, ownership evidence,
  footprint candidates and evidence, coverage, relationships and members,
  historical context) — is computed before the write transaction opens
  and before any `DELETE FROM app_snapshot_*` runs. A rejected replacement
  therefore cannot delete or partially replace a previously valid
  snapshot; preservation does not depend on rollback. The transaction
  binds the pre-checked values verbatim, so no value is converted twice
  after mutation begins, and writes stay atomic for constraint or I/O
  failures. The legacy run API keeps its own `InvalidRunCount` /
  `InvalidTimestamp` for the same caller-input reason.

Phase 6.5.1 / 6.5.2 additions (no new public API; a public-API
correction):

- The store's open-time safety seams are **crate-internal**: `store_safety`
  (failure hazards, quarantine and backup helpers, integrity
  classification) is a private module — not `pub`, no `#[doc(hidden)] pub`
  items, no test-only feature — and the public
  `HistoryStore::open_with_hazard` entry point added in Phase 6.5.1 was
  DELETED in 6.5.2. The public surface is `HistoryStore::open`; no public
  API (and no other crate) can name a hazard, force a quarantine on a
  healthy store, or reach the low-level backup/quarantine helpers.
- Correction to the Phase 6.5.1 record: "nothing outside the history-store
  open path changed" described PRODUCTION behavior; the 6.5.1 commits had
  also added `pub mod store_safety` and `HistoryStore::open_with_hazard`
  (test-only seams) to the crate's public API. Phase 6.5.2 removes those
  two, and the hazard-driven failure tests now live in a crate-internal
  `#[cfg(test)]` module (`src/store_hazard_tests.rs`). The typed errors the
  open path reports to callers (`StoreError::UnrecognizedSchema`,
  `StoreError::BackupFailed`, `StoreError::Corrupt`) remain the public
  contract.
- Behavior added in 6.5.2, with no API change: the open path validates the
  connection it is about to write **on that connection** (the mutation
  gate), and every integrity decision is closed — a check that returns no
  rows is a failure to establish integrity, never a verdict of health (see
  docs/DATABASE.md).

Phase 6.5.3 (a public-API correction for `coresight-core`; `coresight-history`
still gains no public API):

- **`coresight-history`: no new public API.** The only public entry point
  remains `HistoryStore::open`. The Phase 6.5.3 concurrency seams
  (`OpenSeam`), the file-identity type used to bind a quarantine to the
  observed file, and the bounded backup re-gate are all crate-internal; the
  new tests live in a crate-internal `#[cfg(test)]` module. The typed
  errors callers see are unchanged, and no public/test-only claim exists
  that the Rust visibility does not match.
- **`coresight-core::db` gained two `pub` helpers in Phase 6.5.2, and the
  earlier text in this document incorrectly said there was no new public
  API.** They are real public Rust API of `coresight-core`:
  `open_connection(path) -> Result<Connection>` opens a connection and
  writes NOTHING (not even the WAL pragma), and
  `apply_bootstrap(&Connection) -> Result<()>` is the write half (WAL +
  foreign keys + the v1 install). `coresight-history`'s store open path
  calls them cross-crate, so they cannot be made private without breaking a
  legitimate call. The pre-existing `coresight_core::db::open(path)` stays
  as a compatibility wrapper (`open_connection` + `apply_bootstrap`).
  Phase 6.5.3 adds the public constant `OPEN_BUSY_TIMEOUT`.
- **Which paths carry the full history-store safety gates.** ONLY
  `HistoryStore::open` runs the two-gate sequence (read-only preflight +
  mutation gate), the verified pre-migration backup, the quarantine and the
  serialized migrations. The `coresight_core::db` helpers are RAW
  connectivity/bootstrap primitives: they perform no integrity check, no
  recognition, no backup and no quarantine, and applying the bootstrap to a
  file this build does not own is the caller's responsibility. That is
  exactly why the history store validates on the very connection it writes,
  and why the helpers must not be treated as a safe way to open a store.
- **Structure is not provenance.** The recognition check establishes
  STRUCTURAL compatibility with the schema a version's migrations produce.
  It is not evidence of authorship: a database forged to carry the full
  structure and a valid version is handled as the store it claims to be
  (see docs/DATABASE.md).

## Rules for evolution

- Additive changes only within `v1` (new optional fields, new commands).
- Renames/removals → `v2` + documented migration. Frontend sends its contract
  version on init; Rust rejects mismatches with a typed error, not a crash.
- No command performs a destructive action except `executePlan`, and it only
  accepts a `planId` previously returned by `confirmPlan` in the same session.
- Frontend types are generated from (or checked against) the Rust-side schema
  in CI from Phase 2 onward — drift fails the build.
