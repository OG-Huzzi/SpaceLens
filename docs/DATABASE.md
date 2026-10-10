# CoreSight — Database Architecture (SQLite)

Status: Phase 0 conceptual schema. Implemented from Phase 1. Owner: Rust
(`rusqlite`, WAL mode). The frontend NEVER opens the DB.
## Entities

- `drives` — id, stable volume identity (per-OS id), label, fs type, kind
  (internal/external/network), capacity, first/seen-last timestamps, offline flag.
- `scans` — id, drive_id, started/finished, status (complete/partial/cancelled/
  failed), root, file/dir counts, bytes, exclusions summary (what was NOT scanned).
- `entries` — one row per file/dir per scan: scan_id, parent path-id, name, kind,
  size (logical + physical where OS reports both), mtimes, attributes, symlink/
  junction target, volume boundary flags. Current history path storage is tagged
  and lossless: `u:` UTF-8, `e:` lowercase hex of platform-encoded bytes for
  non-UTF-8 paths, and `l:` for irreversibly lossy legacy rows. It performs no
  normalization, case-folding, or separator conversion; lookups use the exact
  stored path semantics. `e:` decoding validates Windows WTF-8 before forming
  UTF-16 and returns a typed decode error for malformed hex/encoding, never an
  unchecked OS-string conversion. Phase 6.3 added no schema change; Phase 6.4
  added only the new `app_snapshot_*` tables (v5), never altering these rows.
- `hashes` — entry identity → SHA-256, algorithm version, hashed-at, bytes hashed.
  Cache key includes size+mtime so stale hashes invalidate without re-read.
- `snapshots` — immutable per-scan rollups: per-category bytes, totals, created-at.
  Snapshots are never mutated; deltas compute between pairs.
- `changes` — derived per-category deltas between snapshots (cached for History).
- `classifications` — entry → category path + rule id + rule version + confidence.
  Re-runnable when rules update without rescanning.
- `operations` — append-only log of cleanup actions: plan id, items, destination
  (trash/quarantine), verdicts, freed bytes, timestamps. Basis for undo + support.
- `settings` — key/value (exclusions, retention, safety policy version).

## Phase 6.4 — application intelligence + system-model snapshots (schema v5)

Status: IMPLEMENTED. Schema version **5**. Forward-only migration
`v4 → v5`, one transaction with its version bump, `CREATE TABLE IF NOT
EXISTS` (idempotent per state, same convention as v2–v4). No second
database: these tables live in the same store as `scan_runs` and
`observations`.

**What is persisted** (canonical facts only — each its own queryable
column; there is deliberately NO `system_model_json` blob):

| Table | Holds | Key |
|---|---|---|
| `app_snapshot_meta` | inventory/footprint truncation counters | `run_id` |
| `app_snapshot_artifacts` | path (lossless), kind, object identity, digest, size, **access state**, classification | `(run_id, artifact_ord)` |
| `app_snapshot_apps` | the full application record + candidate executable | `(run_id, app_id, fact_ord)` |
| `app_snapshot_provenance` | unioned application sources | `(run_id, app_id, fact_ord, prov_ord)` |
| `app_snapshot_views` | raw registry/scope views | `(run_id, app_id, fact_ord, view_ord)` |
| `app_snapshot_roots` | install-root observations | `(run_id, app_id, fact_ord, root_ord)` |
| `app_snapshot_evidence` | ownership evidence: kind, source, strength, correlation group, scope, observed path, matched attribute/value/path | `(run_id, app_id, fact_ord, artifact_path, evidence_ord)` |
| `app_snapshot_footprints` / `_footprint_evidence` | footprint candidates + their evidence | per-fact ordinals |
| `app_snapshot_coverage` | per-source status + note | `(run_id, coverage_ord)` |
| `app_snapshot_relationships` / `_rel_members` | identity-engine relationship facts + members | `(run_id, rel_ord)` |
| `app_snapshot_history` | quoted historical context (run, path, identity, category) | `(run_id, hist_ord)` |

**Run association.** Every row carries `run_id` and references
`scan_runs(run_id) ON DELETE CASCADE`, so retention prunes snapshots with
their runs — one coherent machine memory, not a parallel timeline. A run
that owns no snapshot is *absent* (`load_*` returns `None`), never an
empty snapshot. The configuration fingerprint travels on the run row
(`scan_runs.config`), extended with `appSnapshotSchema` so 6.4-era
snapshots are comparable: `0` honestly means "this run recorded no
snapshot facts" — the same convention as the v4 `rel_status` NULL.

**What is NOT persisted.** Derived state is rebuilt on every reload:
model indexes, edges, claims, resolution states, insights, candidates and
query results. Authorization state is never stored, so a snapshot can
never imply that any action is safe. Ephemeral state (UI, process
handles, in-flight jobs, scan progress) is never stored.

**Ordinal columns are row join keys, not semantics.** The builder's
inputs are multisets (duplicate facts merge commutatively), so verbatim
facts — including duplicates and conflicting history rows — persist under
ordinals assigned after a canonical sort, and reload re-sorts canonically.
Database row order, insertion order, and ordinal values therefore never
affect the rebuilt model; a permutation test commits the same fact set in
reversed arrival order and requires an identical canonical model.

**Evidence re-validation.** Reload re-clamps every stored strength
through the one construction path (`min(requested, kind ceiling, group
ceiling)`), so a tampered over-claim returns weakened rather than trusted.
An application id that does not match normalized `(name, publisher)` is
rejected, so persistence never introduces a second identity definition.

**Boundedness.** Every load is `WHERE run_id = ?` with an explicit
canonical `ORDER BY`. `QueryLimits` caps the top-level sections
(artifacts, applications, source coverage, relationships, history) and
each per-application detail read (provenance, views, install roots,
evidence, footprints) independently, so total materialized rows are
bounded by the limits times the application count — never by the size of
the database, and unrelated runs are never materialized. Bulk insert uses
prepared statements inside the commit transaction.

A cap hit is **detected exactly** (rows are fetched with
`LIMIT limit + 1`, and `probe_limit` saturates at `i64::MAX` rather than
wrapping, so an extreme caller bound can never become a NEGATIVE SQLite
`LIMIT` — which SQLite reads as "unlimited" and would silently disable
both the bound and its detection) and **reported**, never silent: the
returned `SystemSnapshotInput` names the capped sections
(`load_truncated_sections`, `is_load_truncated()`), and
`rebuild_system_model` REFUSES a bounded load with the typed
`StoreError::SnapshotBounded { run_id, sections, limit }` — because a
model built from a prefix of the stored facts would understate what the
run observed (dropping claimants, edges, and history context) and then
present the remainder as the whole truth. Bounded knowledge is never
upgraded into a complete-looking model; the caller raises the limit and
retries. The same signal travels with the inventory-only read
(`load_snapshot_applications` → `LoadedApplications::is_load_truncated`).
A zero limit is a real cap (it reports and refuses), never an empty
success.

**Not wired into a pipeline yet.** Phase 6.4 delivers the storage and
rehydration API and proves it by test; no scan/analysis pipeline calls it
yet, because the workspace has no orchestrator that owns a run end-to-end
(the GUI/IPC layer is a later phase and the Tauri shell is not a
workspace member). Wiring a caller is additive and does not change this
contract.

**Corruption handling.** Every new field decodes strictly: unknown enum
values, malformed `u:/e:/l:` paths, half and impossible object identities
(high bits without a low pair, device without inode), negative sizes or
counts, partial classifications, impossible `system_component` flags,
malformed correlation-group shapes, and orphaned foreign keys are typed
`StoreError::Corrupt`/SQLite failures with table + column context — never
a defaulted `0`, `None`, or empty value.

## Migrations & versioning

- Forward-only, numbered SQL migrations embedded in the binary; `schema_version`
  table. Current version: **6** (v1 core bootstrap, v2 history tables,
  v3 wide identity + lossless tagged paths, v4 relationship-report
  status, v5 Phase 6.4 application/system snapshots, v6 Phase 6.4.1
  application-id re-keying).
- The app refuses to open a NEWER DB (`StoreError::SchemaTooNew`, tells
  the user to upgrade); older stores migrate forward automatically.
- Each migration step runs in ONE transaction with its version bump, so a
  failed migration rolls back rather than leaving a half-migrated
  database (asserted by test). Migrations are idempotent per state.
- Rule/policy tables carry their own versions (`rule_version`, `policy_version`)
  so classification and safety verdicts are reproducible and auditable per scan.
- An automatic pre-migration file backup copy remains PLANNED; the safety
  net today is the per-step transaction plus the integrity check below.

### Application-identity re-keying (migration v6, Phase 6.4.1)

The Phase 6.4 `ApplicationId` hashed `"{name}|{publisher}"`, which is
ambiguous at the component boundary: `("A|B","C")` and `("A","B|C")`
derived the SAME id, so two distinct logical applications could share one
identity. Phase 6.4.1 replaces that with a **length-prefixed** encoding
(`len(name) || name || len(publisher) || publisher`), which is injective
for all inputs because each component's byte prefix states exactly how
many bytes belong to it.

Compatibility (mandatory, because v5 rows already exist):

- The migration **never globally replaces** an id. One legacy id may
  legitimately cover several distinct `(name, publisher)` pairs — that is
  exactly the ambiguity being repaired — so each row is re-keyed from ITS
  OWN stored `name`/`publisher`, splitting the legacy id back into the
  distinct identities it conflated.
- Every child row (provenance, views, roots, evidence, footprints,
  footprint evidence) is re-keyed by joining on its parent's
  `(run_id, app_id, fact_ord)`, so a child can never be adopted by a
  different application.
- `app_snapshot_apps` gained an `id_encoding` column recording which
  encoding each row was written with. The loader verifies it: a row
  claiming the legacy encoding in a v6 store is refused rather than
  silently trusted — re-keying requires the forward migration.
- A stored id matching NEITHER encoding is typed corruption; the
  migration refuses rather than guessing (which would silently reinterpret
  a historical fact).
- The re-key runs inside the migration transaction with foreign-key
  checks deferred to commit (`PRAGMA defer_foreign_keys=ON`), because
  swapping a parent key while children still reference the old one is
  exactly the intermediate state immediate FK enforcement would reject.
  Either the whole re-key lands consistently or none of it does.
- Normalization is unchanged: name and publisher are trimmed and
  lowercased, `None` publisher is the empty string, and NO Unicode
  normalization is applied (documented, not hidden) — so identity is
  insensitive to case and outer whitespace, sensitive to interior
  characters, and NFC/NFD spellings stay distinct by design.

### Checked numeric conversions (Phase 6.4.1)

Every domain value written into a signed SQLite `INTEGER` is converted
through checked helpers (`checked_u64_i64`, `checked_counter_i64`,
`checked_ord_i64`) that reject anything above `i64::MAX` **before the
snapshot transaction mutates the previous snapshot** — never wrapped,
narrowed, saturated, or deferred until reload. Object identity is
deliberately exempt: those columns are intentional bit-pattern storage, so
`u64::MAX` identity components round-trip exactly.

### Footprint policy (Phase 6.4.1)

Footprint candidates are keyed by `(path bytes, application, kind)` — one
*scope* per application. Confidence and evidence describe how well that
scope is known, so two same-key candidates are two descriptions of ONE
scope, reconciled by a commutative, deterministic rule consistent with the
producer: **stronger confidence wins, then the fuller evidence list**
(ranked by an explicit strength order — `Confirmed` is declared FIRST in
`Confidence`, so its derived `Ord` puts it lowest and using it directly
would invert "stronger wins"). Reconciliation happens at BOTH commit and
load, so stored rows and reloaded candidates are identical. Candidates
differing in path, application, or kind are distinct facts and are all
preserved. A footprint's `app` must match its enclosing application fact,
otherwise the commit is rejected (a misattributed scope would relocate
onto the wrong software).

## Indexes (initial set; extend by measured query, not guess)

- `entries(scan_id, parent, name)`, `entries(scan_id, size)` (duplicate candidates),
  `hashes(digest)`, `snapshots(drive_id, created_at)`, `operations(created_at)`.
- Foreign keys ON; synchronous=NORMAL + WAL for scan-write/UI-read concurrency.

## Retention & privacy

- Default retention: last N snapshots per drive (N set in Phase 5; principle:
  enough for meaningful history, bounded disk cost). Old scan `entries` pruned
  with their snapshot; `operations` log kept longer (user's safety record).
- DB lives in the OS-appropriate app-data dir, restricted permissions, never
  synced, never uploaded. Delete-account == delete file (documented path).

## Corruption & recovery

- `PRAGMA integrity_check` runs on EVERY open, before anything reads or
  migrates the file (IMPLEMENTED, Phase 6.1 audit: `HistoryStore::open`
  refuses a corrupt store with the typed `StoreError::Corrupt` — never
  guess, never half-read).
- PLANNED (not yet implemented — the check refuses instead of hiding):
  quarantining the corrupt file aside, starting fresh, and offering a
  rescan flow. Also PLANNED: an automatic pre-migration backup copy of
  the store file (migrations themselves are already atomic per step,
  version bump inside the same transaction — a failed migration rolls
  back rather than leaving a half-migrated database).
- Scans are idempotent and resumable-by-restart: a crashed scan leaves a
  `failed` record and zero partial visibility (snapshot publishes atomically).
- Phase 6.4 snapshots follow the same rule: commit is one transaction, and
  re-committing the same run's snapshot deletes its prior rows inside that
  transaction — retrying never creates semantic duplicates. Foreign-key
  violations (a child row for a missing application fact) are refused by
  the schema rather than silently orphaned.
