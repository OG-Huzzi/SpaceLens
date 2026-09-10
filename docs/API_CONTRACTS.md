# SpaceLens — IPC / API Contracts

Status: Phase 0. Versioned contract (`v1`). Breaking changes require a version
bump and a migration note. The frontend must never need filesystem knowledge.

## Transport

Tauri 2 commands (request/response) + events (Rust→UI streams).
All payloads JSON, `camelCase`, sizes in bytes (integers), timestamps RFC 3339 UTC.
API namespace: `spacelens.v1.*`.

## Commands (UI → Rust)

- `spacelens.v1.startScan { driveId, fullRescan? } → { scanId }`
- `spacelens.v1.cancelScan { scanId } → { cancelled: true }`
- `spacelens.v1.getDriveSummary { driveId } → DriveSummary`
  (totals, exclusions note, last scan, snapshot count)
- `spacelens.v1.getCategories { scanId, parentCategory? } → Category[]`
  `{ id, name, bytes, shareOfParent, deltaSinceLast?, itemCount }` — no paths.
- `spacelens.v1.getOpportunities { scanId } → Opportunity[]`
  `{ id, tier: safe|review, title, whatItIs, whyRecommended, bytes,
     recoverableBytes, staysUntouched, consequence, preview }`
- `spacelens.v1.previewPlan { opportunityIds } → PlanPreview`
  (exact items-or-rules, bytes, destination, undo path, warnings)
- `spacelens.v1.confirmPlan { planId, ackedWarnings } → { accepted, rejected[] }`
  (safety validation runs HERE, synchronously, before any action)
- `spacelens.v1.executePlan { planId } → { results, freedBytes, leftovers[] }`
- `spacelens.v1.getHistory { driveId } → SnapshotDelta[]`
- `spacelens.v1.listDrives → Drive[]` (connected + remembered-offline)

## Events (Rust → UI)

- `spacelens.v1.scanProgress { scanId, filesSeen, bytesSeen, currentCategory?, etaSec? }`
  throttled ≤4/sec.
- `spacelens.v1.scanComplete { scanId, status, summary }`
- `spacelens.v1.planVerified { planId, freedBytes, leftovers[] }`

## Errors

Typed everywhere: `{ code, message, detail? }`. Stable codes, e.g.
`SCAN_IN_PROGRESS`, `DRIVE_OFFLINE`, `PLAN_REJECTED_UNSAFE { reasons[] }`,
`NOTHING_TO_DO`, `PERMISSION_GAP { unscannedPathsCount }`, `DB_CORRUPT_RESCANNED`.
UI renders `message` verbatim for known codes (copy reviewed with safety doc);
unknown codes show a generic safe fallback, never a stack trace.

## Identity contracts (Phase 3)

Namespace: `spacelens.v1.identity.*` (engine-side in `crates/spacelens-identity`;
IPC payload shapes follow the same camelCase + bytes-as-integers rules as
above). Content identity = SHA-256 (`HashAlgorithm::Sha256`, tag `"sha256"`);
a content identity is meaningless without its algorithm tag. Duplicate groups
report `logicalDuplicateBytes` exactly and `recoverableBytes` only with
`Exact`/`Estimated` accounting evidence — see docs/IDENTITY.md for the full
semantics (hard links, mutation policy, zero-byte policy, deterministic
ordering). No IPC command surfaces these yet; the types are the contract for
future phases and are covered by engine tests.

## Rules for evolution

- Additive changes only within `v1` (new optional fields, new commands).
- Renames/removals → `v2` + documented migration. Frontend sends its contract
  version on init; Rust rejects mismatches with a typed error, not a crash.
- No command performs a destructive action except `executePlan`, and it only
  accepts a `planId` previously returned by `confirmPlan` in the same session.
- Frontend types are generated from (or checked against) the Rust-side schema
  in CI from Phase 2 onward — drift fails the build.
