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
  rule at both commit and load (stronger confidence, then fuller
  evidence), so stored and reloaded facts are identical; a footprint
  attributed to a foreign application is rejected.
- `ApplicationId` uses a length-prefixed encoding (injective at component
  boundaries). Ids persisted under the Phase 6.4 encoding are re-keyed by
  the v5→v6 migration, per stored fact rather than globally.

## Rules for evolution

- Additive changes only within `v1` (new optional fields, new commands).
- Renames/removals → `v2` + documented migration. Frontend sends its contract
  version on init; Rust rejects mismatches with a typed error, not a crash.
- No command performs a destructive action except `executePlan`, and it only
  accepts a `planId` previously returned by `confirmPlan` in the same session.
- Frontend types are generated from (or checked against) the Rust-side schema
  in CI from Phase 2 onward — drift fails the build.
