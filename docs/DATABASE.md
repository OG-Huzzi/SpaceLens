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
- A file that is a valid SQLite database but NOT a store this build can
  recognize — no `schema_version` table, unusable version metadata, a
  recorded version of 0, or a recorded version whose required structure
  (the tables AND signature columns its migrations guarantee) is incomplete
  — is refused UNWRITTEN with `StoreError::UnrecognizedSchema` (Phase 6.5.1;
  the structure contract completed in Phase 6.5.2). It is not initialized,
  backed up, migrated, or quarantined: the mutating bootstrap never runs on
  a database CoreSight does not own. A brand-new EMPTY file is the one
  shape that IS initialized — refusing it would refuse a first run.
- Each migration step runs in ONE transaction with its version bump, so a
  failed migration rolls back rather than leaving a half-migrated
  database (asserted by test). Migrations are idempotent per state.
- Rule/policy tables carry their own versions (`rule_version`, `policy_version`)
  so classification and safety verdicts are reproducible and auditable per scan.
- An automatic pre-migration backup is IMPLEMENTED (Phase 6.5) — see
  "Pre-migration backup and corrupt-store quarantine" below. The per-step
  transaction and the integrity check remain the other half of the safety
  net; neither replaces the other.

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

### Snapshot numeric rejections are caller-input errors (Phase 6.4.6)

When one of those helpers rejects a value, the result is
`StoreError::InvalidSnapshotInput { table, column, run_id, value }` —
NOT `StoreError::Corrupt`. The rejected value is caller input (an
`Inventory`/`FootprintReport` counter, an artifact `size`, an application
`estimated_size_bytes`), so it is never evidence of database damage:
reporting it as corruption would send an operator hunting for damage that
does not exist. `value` is a `u128`, wide enough to report a `u64` domain
value or a `usize` ordinal without narrowing, so `u64::MAX` is reported
exactly rather than truncated.

`Corrupt` remains reserved for genuinely malformed or impossible
PERSISTED data: strict unsigned decode (`u64_nonneg`), identity decode
(`snap_identity`), unknown-enum rejection, parallel-vector mismatch,
footprint/app-attribution mismatch, inventory-vs-facts set mismatch,
footprint-report mismatch, and `integrity_check`. The legacy run API keeps
its own typed `InvalidRunCount` and `InvalidTimestamp` for the same
caller-input reason; the families agree in policy and stay distinct types
per API.

### The snapshot preflight (Phase 6.4.6)

`commit_system_snapshot` performs a true preflight. Every fallible numeric
conversion — artifact sizes, estimated sizes, and every ordinal domain
including nested collections (provenance, views, install roots, ownership
evidence, footprint candidates and evidence, coverage, relationships and
members, historical context) — is computed BEFORE the write transaction
opens and before any `DELETE FROM app_snapshot_*` statement runs. A
rejected replacement therefore never deletes or partially replaces a
previously valid snapshot; it does not rely on rollback to preserve it.

Ordinals are `0..len` of a caller-sized collection, so the preflight
checks each collection's BOUND (`len - 1` fits, or `0` when empty), which
covers every position the write loop can bind. The transaction then binds
those pre-checked values verbatim: no checked value is converted a second
time after mutation begins. Object identity (device / inode / high-bit
columns) is deliberately not converted, preserving full-width round-trip.

### Footprint policy (Phase 6.4.1)

Footprint candidates are keyed by `(path bytes, application, kind)` — one
*scope* per application. Confidence and evidence describe how well that
scope is known, so two same-key candidates are two descriptions of ONE
scope, reconciled by a commutative, deterministic rule consistent with the
producer: **stronger confidence wins, then the larger count of DISTINCT
evidence items, then the canonically-ordered evidence content as a
deterministic tie-breaker** (ranked by the explicit strength order —
`Confirmed` is declared FIRST in `Confidence`, so its derived `Ord` puts it
lowest and using it directly would invert "stronger wins"). The rank is
defined ONCE in `coresight_apps::footprint::candidate_rank` and called by
both the producer and persistence, so the two cannot drift. Reconciliation happens
at BOTH commit and load, so stored rows and reloaded candidates are
identical. Candidates differing in path, application, or kind are distinct
facts and are all preserved. A footprint's `app` must match its enclosing
application fact, otherwise the commit is rejected (a misattributed scope
would relocate onto the wrong software).

**Evidence is a SET**, normalized identically at all three points:
discovery admission, commit, and reload (`normalize_candidate_evidence`).
The read path normalizes AFTER applying the section cap, so a genuinely
oversized section still reports truncation and `rebuild_system_model`
refuses it. Because a legitimate database can never contain duplicate
evidence rows (the write path deduplicates), duplicates can only arrive
from tampering; the chosen tampered-store policy is to **normalize them
away** rather than reject the read, which preserves the set contract
without weakening truncation detection.

### Query-limit contract (Phase 6.4.2, refined 6.4.3)

One shared, typed conversion policy is used by EVERY query API in this
crate — snapshot reads, the older history reads (`list_runs`,
`history_for_path`, `history_for_object`, `history_for_content`,
`relationship_history`), and snapshot listing:

- `QueryLimits::MAX_SUPPORTED` is `i64::MAX - 1` — deliberately ONE BELOW
  the largest SQLite `LIMIT`, because snapshot reads detect a cap hit
  EXACTLY by probing with `limit + 1` rows and that sum must stay
  representable.
- `QueryLimits::to_sql_limit()` converts or fails with the typed
  `StoreError::LimitOutOfRange { requested, supported }`.
- `QueryLimits::to_probe_sql_limit()` returns `limit + 1` — never
  saturated, never negative.

SQLite reads a NEGATIVE `LIMIT` as **unbounded**. `usize::MAX as i64` is
`-1` on every supported platform, so an unchecked cast would silently
disable the caller's bound AND its detection. Every entry point now
rejects an unrepresentable limit with a typed error BEFORE executing SQL —
and **including on the absent-record path**: `load_system_snapshot`,
`load_snapshot_applications`, and `rebuild_system_model` validate the
bound before probing whether the run has a snapshot, so an invalid limit
can never masquerade as "no snapshot exists".

Nothing saturates silently. At an earlier `MAX_SUPPORTED` of `i64::MAX`
the probe would have had to saturate, silently degrading detection
exactly at the boundary, so `i64::MAX` is now the first REFUSED value.

Invalid limit, corrupted data, and detected truncation remain three
DISTINCT states: `LimitOutOfRange`, `Corrupt`, and the truncation signal
(`is_load_truncated()` / `SnapshotBounded`).

### Legacy numeric compatibility (Phase 6.4.3)

`scan_runs.rel_truncated` was originally written with an UNCHECKED
`u64 as i64` conversion, so a domain value at or above `2^63` was stored
as a NEGATIVE SQLite integer — a valid legacy bit pattern, not corruption.
The read path reconstructs it faithfully (`stored as u64`, the exact
inverse of the legacy write).

**Documented limitation:** a negative stored value is AMBIGUOUS. A legacy
wrapped counter and a tampered value that never came from this writer are
indistinguishable in the stored format, so the store does NOT claim to
tell them apart. It reconstructs the round-tripped value — the only
reading that preserves every historical value the old writer could
produce. What it must never do is clamp to zero, which is what the
previous implementations did: a legacy `rel_truncated` above `2^63` read
back as "nothing truncated", erasing the incompleteness signal and making
a truncated run look complete.

`relationship_obs.recoverable` follows the same rule (its read paths were
already correct). New writes are unaffected — Phase 6.4.2's checked
conversion rejects an unrepresentable counter before any mutation, so no
NEW row can store a wrapped value.

### Application-identity validation at commit (Phase 6.4.2)

The commit path now runs an identity preflight using the shared
`snapshot_codec::verify_application_id` (the same verifier the loader
uses), before the transaction opens. Matching parallel vectors prove only
that the two input copies agree with each other; they say nothing about
whether the id they carry is derivable from its own name/publisher. A forged
id supplied consistently everywhere used to commit successfully and then
FAIL on reload — the worst kind of failure, a snapshot that cannot be read.
The name is validated AFTER normalization, so a whitespace-only name
(non-empty as a raw string, empty after trimming) is rejected rather than
collapsing onto a shared empty-named identity.

### Numeric persistence and TRUE preflight (Phase 6.4.2 + 6.4.3 + 6.4.4 + 6.4.5)

- `commit_run` and `begin_run` perform a TRUE preflight: every
  caller-controlled conversion that can fail is computed BEFORE the
  transaction opens. A rejected input never even begins modifying existing
  state, rather than relying on transaction rollback.
- The checked set covers **all nine `RunCounts` fields**
  (`entries_examined`, `files`, `dirs`, `links`, `other_entries`, `bytes`,
  `observation_errors`, `candidates_untracked`, `hash_failures`),
  `ObservedEntry::size`, relationship `size` / `member_count` /
  `recoverable_bytes`, the relationship truncation counter, and every
  caller-supplied timestamp (`started_at`, `completed_at`, and each
  observation's `modified`).
- **Invalid CALLER input is a distinct typed error from persisted
  corruption**: `StoreError::InvalidRunCount { table, column, value }`
  for counts/sizes and `StoreError::InvalidTimestamp { table, column,
  run_id, nanos }` for timestamps. Mislabeling valid caller data as a
  corrupt store would send callers looking for database damage that does
  not exist. Timestamps additionally carry the run they belonged to and
  the offending nanosecond count, so the rejection is diagnosable.
- Every checked value is BOUND VERBATIM after the preflight succeeds, so
  no second conversion can fail between the UPDATE and the INSERTs.
- Timestamps convert through `checked_timestamp`, which names the ACTUAL
  owning table and column:
  - `scan_runs.started_at`
  - `scan_runs.completed_at`
  - `observations.modified`
- The store's own clock reading (`SystemTime::now()`, written to
  `scan_runs.completed_at` by `recover_stale_runs` and `abandon_run`) is
  checked the same way and propagates a typed error instead of panicking:
  a defective platform clock can never crash a caller mid-operation.
- Full-width `ObjectIdentity` components keep their intentional
  bit-pattern conversions, documented and tested separately — `u64::MAX`
  identity components round-trip exactly and are deliberately NOT
  range-checked.
- `Duration::as_nanos()` is a `u128` wider than `i64`, so narrowing it
  wraps any timestamp above ~2262 CE into a NEGATIVE number. Unrepresentable
  timestamps therefore fail loudly on write; what the reader does with a
  negative it finds in an OLD database is a separate, documented policy
  (below).

### Persisted timestamp decoding — legacy compatibility policy (Phase 6.4.5)

Every writer generation encodes a timestamp as **nanoseconds since the
Unix epoch in a signed `INTEGER`**, and every one of them maps a
PRE-EPOCH time to `0` (the documented pre-epoch convention). The writers
differ only in how they narrow the wider `u128` nanosecond count:

- **v2 through pre-6.4.2**: an UNCHECKED `u128 as i64` truncation. A time
  from ~year 2262 up to ~year 2554 wrapped into the negative half of the
  signed domain; a time beyond ~year 2554 wrapped modulo 2^64 (bits above
  64 are discarded).
- **6.4.2 onward**: `checked_timestamp` — a pre-epoch time is stored as
  `0`, an in-domain time is stored exactly, and an out-of-domain time is
  REJECTED before any mutation. No new write can store a negative value.

The read policy (`time_from_nanos`), proven by direct tamper tests
across every read path:

| Stored value | Reading | Why |
|---|---|---|
| `0` | the Unix epoch | the pre-epoch convention, producible by every writer |
| `1 … i64::MAX` | epoch + that many nanoseconds | the exact time for every writer |
| negative | epoch + the stored bits read as an UNSIGNED nanosecond count | the exact inverse of the legacy unchecked write |

A negative value is therefore a **legitimate legacy bit pattern, not
corruption**: it reconstructs the original time exactly for any time the
old writer could produce below 2^64 nanoseconds (~year 2554). It is
neither rejected (a supported writer produced it — a value safely
rejected on new writes is not necessarily corrupt in an older database)
nor clamped to the epoch (the previous reader's silent conversion into
the plausible date 1970-01-01, which erased the far-future signal).

**Documented limitations (not claimed capabilities):**

- A negative stored value is AMBIGUOUS: a legacy wrapped far-future time
  and a tampered value are indistinguishable in the stored format. The
  store does not claim to tell them apart; it applies the one reading
  that preserves every historical value the old writer could produce.
- The reconstruction is lossy for originals at or past ~year 2554: the
  truncation discarded the bits above 64, so such a value reads back as
  epoch + (low 64 bits) — roughly 584 years early — and an original whose
  wrapped bits happen to be non-negative is indistinguishable from an
  ordinary timestamp. No read policy can repair that.
- A Windows `SystemTime` is 100-nanosecond FILETIME-based, so a
  sub-100ns remainder truncates on every conversion — the same
  granularity every timestamp read on that platform has always had,
  positive values included.

### Relationship report metadata contract (Phase 6.4.4)

`scan_runs` carries the relationship report as TWO columns written from the
same `Option<&RelationshipReport>` — `rel_status` and nullable
`rel_truncated` — so they are always a PAIR. The v4 migration added both as
nullable columns with no default, so pre-v4 rows carry both as NULL.

**The valid nullability contract:**

| `rel_status` | `rel_truncated` | Meaning | Accepted |
|---|---|---|---|
| NULL | NULL | no report was recorded | **yes** — the absent case |
| present | present | a report was recorded, with its count | **yes** |
| present | NULL | no writer can produce this | **no** — inconsistent metadata |
| NULL | present | no writer can produce this | **no** — inconsistent metadata |

The two mixed combinations are rejected as inconsistent metadata
(`StoreError::Corrupt`) naming the offending column and run. Rejecting
them is safe: no writer version can produce them, so no legitimate
historical state is refused.

A *present* count is never inferred to be zero when absent, because a
present status means a report existed — inventing zero would fabricate a
completeness the run never recorded. What IS preserved is the distinction
between "no report was recorded" (both NULL) and "a report was recorded
with zero truncations" (status present, count `0`).

**What (NULL, NULL) means — and what it does NOT mean (Phase 6.4.5,
Workstream C).** The relationship TABLES predate v4, while the metadata
columns were introduced by the v4 migration. A legacy database can
therefore contain `relationship_obs` / `relationship_members` rows while
both metadata columns are NULL. `(NULL, NULL)` states only that no
relationship-report status or truncation metadata was recorded for the
run — it does NOT prove that no relationship rows ever existed. A
genuine pre-v4 fixture (a real v2 database carrying relationship rows)
migrates through the actual v2 → v6 chain with both columns still NULL,
both rows retained, and no fabricated completeness: `load_run_snapshot`
attaches no report, while the retained facts stay queryable through the
public `relationship_history` query. The migration itself adds the
columns as nullable with no default and touches no relationship row.

**Legacy counters keep their faithful signed→unsigned reconstruction**, per
the Phase 6.4.3 compatibility policy: a negative stored value round-trips
exactly, and the store does not claim to distinguish a legacy-wrapped
counter from tampering. The same policy and the same honest limitation
now apply to persisted timestamps (see above).

### Retention policy (Phase 6.4.3)

`RetentionPolicy::keep_latest` must be at least 1. A value of 0 would
permit deleting the newest committed run — the baseline for future
comparisons — so it is rejected with `StoreError::InvalidRetentionPolicy`
BEFORE any row is read or deleted. The doc comment and tests agree with
this.

The age comparison stays in the wide domain: `Duration::as_nanos()` is a
`u128` wider than `i64`, so narrowing it wraps any duration above ~292
years into a NEGATIVE number, inverting the rule and deleting genuinely
fresh history. Elapsed time and the policy duration are compared as
`u128`, so `Duration::MAX` and multi-century policies mean "keep
everything at least this long".

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

## Pre-migration backup and corrupt-store quarantine (Phase 6.5)

Guarantees about the FILE, enforced by `HistoryStore::open` in
`crates/coresight-history/src/store.rs`, with the backup and quarantine
helpers (and the test-only failure seams) in
`crates/coresight-history/src/store_safety.rs` — a **crate-private** module
since Phase 6.5.2 (see "API surface" in `docs/API_CONTRACTS.md`). Phase 6.5
established the guarantees; 6.5.1 and 6.5.2 closed the gaps recorded at the
end of this section.

### Ordering: two gates, and nothing is mutated before both pass

`coresight_core::db::apply_bootstrap` runs `PRAGMA journal_mode=WAL` and
applies the v1 bootstrap — both of which WRITE. The open sequence therefore
has TWO gates, and a path must pass both before any write.

**Gate 1 — the read-only preflight.** An EXISTING store is inspected through
a **read-only** connection (`SQLITE_OPEN_READ_ONLY`), which cannot
checkpoint, cannot create sidecars, and cannot alter the file:

1. No file at the path → brand new. Nothing to protect, nothing to back up.
2. `PRAGMA integrity_check` returns anything other than `ok` → corruption
   established → quarantine (below).
3. A file holding no schema objects at all → a store this build CREATES
   (gate 2 re-confirms on the connection that would write). A brand-new
   EMPTY file is this case, not the refusal case below; refusing it would
   refuse a first run.
4. Recorded version NEWER than this build supports → `SchemaTooNew`, and the
   file is left **completely untouched**: not migrated, not backed up, not
   quarantined. A newer store is not this build's to modify. Decided before
   the shape check below, so a newer store's missing or renamed tables can
   never be misread as "not ours".
5. A readable SQLite file that is not recognizable as a store this build has
   ever written → `UnrecognizedSchema`, left **unwritten**: not initialized,
   not backed up, not migrated, not quarantined (below).
6. Recorded version OLDER than supported → proceed to gate 2, which
   re-confirms the verdict and takes the verified backup ON the validated
   connection before migrating.
7. Recorded version == supported and recognizable → open it as-is (gate 2
   re-checks).

The read-only handle is **closed before** anything else. This is
load-bearing, not tidiness: SQLite holds the file open for the connection's
lifetime and Windows cannot rename an open file. A quarantine running while
the handle was alive would fail on Windows and succeed on Unix — a
platform-dependent safety guarantee. The preflight therefore returns owned
data, never a `Connection`.

**Gate 2 — the mutation gate.** The path is re-opened READ-WRITE through
`coresight_core::db::open_connection`, which writes NOTHING (not even the
WAL pragma; that is part of `apply_bootstrap`), and gate 1's full check set
— integrity, object count, recorded version, recognition — runs AGAIN on
that very connection, the one that would perform the writes. Only then does
`apply_bootstrap` run, followed by the post-bootstrap integrity check
(below).

This closes the inspection-to-mutation race: a database SUBSTITUTED at the
store path between the two gates is validated by the gate that would write
it, and is either handled under its OWN verdict — initialized if it is
empty, backed up (with its own verified backup) before its migration — or
refused without being written (foreign file, incomplete structure, too
new). A substitute that passes both gates while claiming a supported version
is treated as the store it claims to be; structure is not provenance
(below). Proven by
`store::hazard_tests::a_database_swapped_in_after_the_read_only_gate_is_gated_before_any_write`,
which moves the genuine store aside and drops a foreign database in at the
boundary observer (deterministic file operations at the seam — no sleeps, no
scheduling assumptions).

Cost: one open can run up to three integrity checks on the store path
(preflight, mutation gate, post-bootstrap), and a migrating open runs a
fourth on the completed backup copy before publishing it. That is the price
of deciding from read-only evidence and then re-deciding on the connection
that writes.

### The backup is SQLite-consistent, and published only once verified

- Taken by **gate 2, on the connection it just validated** (Phase 6.5.2): a
  backup is a decision to write, made only after the mutation gate has
  re-established the store's identity on the very connection the migration
  would use. Gate 1 decides *whether* a backup will be needed; gate 2
  re-confirms and takes it.
- Written through **SQLite's Online Backup API** (`rusqlite::backup`), never
  a raw file copy. A raw copy of the main file while a `-wal` holds
  uncheckpointed frames produces a stale image that silently omits committed
  history; the Online Backup API reads through the WAL and restarts if the
  source changes mid-copy. (Asserted by
  `store::hazard_tests::a_backup_captures_data_held_only_in_the_uncheckpointed_wal`,
  crate-internal.)
- Built into an attempt-specific temporary (`<store>.backup-in-progress`,
  `<store>.backup-in-progress.1`, … on a collision), then re-opened by a
  **separate** connection and checked for `PRAGMA integrity_check == ok`
  **and** the source's pre-migration schema version — before it is
  published. Verification is closed like every other integrity decision: a
  check that cannot establish integrity — anything other than a non-empty
  all-`ok` result — fails the backup
  (`StoreError::BackupFailed { step: "verify" }`), publishes NOTHING under
  the backup name, and leaves the source unmigrated.
- Published by renaming to `<store>.pre-migration-backup`. **Both** the
  temporary name and the final name are reserved per attempt with
  `create_new`, which succeeds only for an unused name, so a second attempt
  gets `.1`, `.2`, … names and can never overwrite, adopt, or share the
  first attempt's artifacts, whether that attempt is running concurrently
  or died before cleanup. A stale artifact is ignored and the counter
  advances; nothing a later attempt did not create is ever deleted. The
  reservations are adjacent to the source, so the rename is
  same-filesystem on every platform.
- **Failure is closed.** If the copy, the verification, or the publication
  fails, exactly the temporary *and* the still-empty reserved name THIS
  attempt created are removed and `open` returns
  `StoreError::BackupFailed { step, detail }`. A backup that does not exist
  is never claimed to exist, and no failure path removes a path the attempt
  did not create.
- **A failing attempt cannot delete a live one's work.** Proven with a
  deterministic barrier (not a sleep) by
  `store_safety::tests::concurrent_backup_attempts_never_share_or_delete_each_others_artifacts`:
  attempt A fails at publication while attempt B holds a verified
  temporary, and B still publishes its own backup intact.
- A backup is taken only when a migration is genuinely required: a new
  database and an already-current schema produce none.
- A migration that then fails rolls back its own transaction and **keeps** the
  verified backup, so the pre-migration state is still recoverable.

### Concurrent opens are serialized on SQLite's own lock (Phase 6.5.3)

CoreSight is a single-process application, but two processes CAN open the same
store at once (a second launch, a helper, a test). The open path therefore
serializes the decision-to-mutation sequence on SQLite's own cross-process
write lock — never on a process-local mutex, which two processes would not
share.

What each operation actually holds:

- The **read-only preflight** holds only read locks on its own connection; it
  writes nothing and is closed before anything else happens.
- `PRAGMA journal_mode=WAL` (in `apply_bootstrap`) is refused inside a
transaction and **does not use the busy handler**: a concurrent holder makes
it answer `SQLITE_BUSY` immediately. The WAL conversion is therefore retried
within the connection's own bound (`OPEN_BUSY_TIMEOUT`, also a bound on the
connection's busy timeout) and then fails closed with SQLite's own error.
- `apply_bootstrap`'s v1 install is a check-then-act sequence. The decision —
  the `schema_version` table's PRESENCE, never a coerced version number — is
  re-made INSIDE `BEGIN IMMEDIATE`, so exactly one of two processes opening
the same brand-new store installs v1 and the other observes the completed
install and does nothing. A store that already has the table takes no write
lock at all (an unlocked peek short-circuits the common case and decides
nothing).
- **Each migration step runs in `BEGIN IMMEDIATE` and re-reads the recorded
  version inside that lock.** A version read outside a lock is a snapshot
  another process may invalidate before the first mutation; re-reading under
the lock makes "this migration is pending" and "run it" one indivisible
decision. A second opener of the same older store therefore observes the
winner's committed step and SKIPS it, instead of applying a stale plan and
failing on a duplicate `CREATE TABLE` / `ADD COLUMN`. The `current` value
passed in is only an optimization; it is never trusted for the decision.
  Per-migration transactionality is unchanged: each step is still one
  transaction that runs the migration SQL and its version bump together, and
  the v2–v6 SQL is byte-identical.
- If a concurrent opener completes the migration while this opener is
  building its **pre-migration backup**, the backup would no longer describe
  the state this opener would migrate. Rather than reporting that as a false
  backup failure (or migrating a state its backup does not match), the opener
  re-gates on the newer version — bounded by `MAX_BACKUP_REGATES` — and, if
  that version still needs migrating, takes THAT state's backup. When the
  store is already current, no migration of its own runs at all.
- An opener that cannot acquire a lock within `OPEN_BUSY_TIMEOUT` (5 s) fails
  closed with SQLite's own `SQLITE_BUSY`; nothing is migrated and the store
  is left valid and openable.

The invariants the phase-6.5.3 regression tests pin (`store::open_race_tests`,
crate-internal; barriers and seam callbacks, never sleeps): two concurrent
opens of a genuine v1 store; two of a supported intermediate store; two
openers held together at the exact point before their first mutation; a
writer committing between the integrity read and recognition; and a writer
committing between recognition and the first mutation. Each case asserts no
stale plan is applied, no duplicate schema change is left behind, the final
store is v6 and intact, and any published backup is itself integrity-checked
and records the pre-migration version it protects.

### Corruption is quarantined, never replaced or repaired

Only SQLite's own "this file is not a usable database" codes
(`SQLITE_CORRUPT`, `SQLITE_NOTADB`) establish corruption. A permission
denial, a busy/locked database, a read-only filesystem, a full disk, or an
ordinary I/O failure are ENVIRONMENT problems: quarantining a healthy file
because the disk was busy would be worse than doing nothing, so the files are
left untouched and the error is returned as-is. A failure to *read* the
integrity result is never smoothed into "healthy".

When corruption IS established:

- **The relocation acts only on the file whose corruption was OBSERVED.** The
  integrity verdict and the rename are separated by the close Windows
  requires, so the pathname is briefly unattended. The relocation is bound to
  a file identity captured while the read-only connection still pinned that
  file (Unix `st_dev`/`st_ino`; Windows volume serial + the 128-bit
  `FILE_ID_INFO` file identifier, through `coresight-engine`'s
  platform-neutral metadata boundary), re-established immediately before each
  move. A path that now names a DIFFERENT file — or a file whose identity
  could not be captured — is refused with `QuarantineFailed { step: "verify
  file identity" }`, and NOTHING is moved; the typed error says the damaged
  store was NOT quarantined, so it never falsely claims the observed damaged
  file was preserved. A healthy database substituted at the path is therefore
  never labelled as, or moved in place of, another file's corruption.
- Every connection to the file is closed first (see the ordering rule above).
- The database **and** any `-wal` / `-shm` sidecars are relocated into
  `<store>.quarantined` (or `<store>.quarantined.1`, … on a collision).
  Sidecars move too, so uncheckpointed WAL frames are preserved rather than
  silently discarded.
- A partial failure attempts to restore — in reverse order — exactly the
  files this attempt moved. Destination names are derived from the
  originals, so a restore can never overwrite a file this attempt did not
  move. If every restore succeeds, the store is back where it was and the
  error says its files are **unchanged where they were**; the emptied
  destination is removed (a directory still holding an artifact is left in
  place, so nothing is hidden). If a restore also fails, the error says the
  store **could NOT be quarantined cleanly and its files are NOT all at
  their original locations**, naming each artifact that could not be put
  back and where it remains. It never claims a successful quarantine.
- Both halves of that honesty are exercised with REAL moves to undo: the
  injected failures fire on the last existing sidecar, after the main
  database (and any earlier sidecar) has already moved, so the rollback
  runs with a non-empty move list
  (`store::hazard_tests::a_sidecar_relocation_failure_restores_everything_that_moved`,
  `store::hazard_tests::an_unrestorable_main_file_is_reported_honestly_and_stays_quarantined`,
  crate-internal).
- `open` then returns `StoreError::Corrupt` whose detail names the quarantine
  directory. **No replacement database is created at the original path, no
  repair is attempted, and no recovery of historical data is claimed.**

### Corruption the bootstrap itself introduces is quarantined too

Gate 1 proves the file was intact when it was FOUND, but its evidence is
read-only; gate 2 re-checks before writes; neither can see damage introduced
by the writes `apply_bootstrap` itself performs (the WAL pragma and the v1
bootstrap; a migration's own damage is still preceded by its verified
backup). `open` therefore runs the integrity check a THIRD time, on the
already-open connection, after the bootstrap:

- Clean → continue.
- Problems, or an integrity result unreadable with a corruption code →
  close the connection and quarantine, exactly like preflight corruption.
  (Previously this path returned the error WITHOUT moving the store aside,
  leaving a reliably corrupt store at a path every future open could keep
  failing on.)
- A result unreadable for a NON-corruption reason (an ordinary I/O failure)
  → the store is left exactly where it is and the error is returned; an
  environment problem is not evidence of damage.
- No rows at all → a failure to establish integrity (below): the open
  aborts, the store stays where it is, and nothing is quarantined.

### Integrity decisions are closed: no result is not a verdict

An `integrity_check` that returns NO rows has established NOTHING, so it is
never read as health and never as damage. Every decision site treats it as a
failure to establish integrity: the open aborts, the store is left exactly
where it is, nothing is moved, and the error says integrity could not be
established. The backup-verification site reports the same way
(`StoreError::BackupFailed { step: "verify" }`) and the source is left
unmigrated. Only a non-empty result whose every row is `ok` is health, and a
result unreadable with SQLite's own corruption code remains positive
corruption evidence and is quarantined as such. Every decision site
(read-only preflight, mutation gate, post-bootstrap check, and backup
verification) is covered deterministically by crate-internal seams
(`store::hazard_tests::an_empty_preflight_integrity_result_fails_closed_without_quarantine`,
`an_empty_backup_verification_result_never_publishes_a_backup`,
`an_empty_post_bootstrap_integrity_result_fails_closed_without_quarantine`).

### A file this build cannot recognize is refused, never initialized over

The bootstrap adds its schema to whatever file it is pointed at, so before
anything may touch an existing file, the gates must prove the file is a
store this build has ever written. A readable SQLite database that holds
objects but fails that proof is refused with
`StoreError::UnrecognizedSchema` when it:

- has no `schema_version` table (a foreign database);
- has a `schema_version` table that does not hold exactly one non-negative
  integer (no rows, several rows, a non-integer, a value beyond `u32`);
- records version 0, which no CoreSight build has ever written;
- records a version whose REQUIRED STRUCTURE is incomplete: the tables AND
  signature columns that version's migrations guarantee. The contract
  follows the recorded version through every step — v1: `drives` + `scans`
  and their key columns; v2 adds `scan_runs`, `observations`,
  `relationship_obs`, `relationship_members` and their signature columns;
  v3: the `file_id_hi` columns; v4: `scan_runs.rel_status` /
  `rel_truncated`; v5: the 13 `app_snapshot_*` tables; v6:
  `app_snapshot_apps.id_encoding`. The refusal names each gap ("missing
  table X" / "missing column T.C"). This is why an incomplete v2 or v6
  lookalike is refused even though it carries the history tables a blanket
  "core tables" rule would accept (Phase 6.5.2 completed this contract; the
  6.5.1 pair-based rule was too weak).

"Unusable" data is refused rather than coerced to 0, because 0 is exactly
what would hand the file to the bootstrap. The refusal leaves the
**database file and its `-wal` sidecar byte-identical**, deletes no sidecar,
takes no backup, and adds nothing
(`phase65_tests::a_foreign_sqlite_database_is_refused_and_left_byte_identical`;
tests 13–16 cover the versioned-foreign, unusable-metadata, version-0, and
empty-file cases; tests 18–20 cover the adversarial incomplete v2 store and
the v6 store missing a required column or table). The one empty shape that
IS initialized is a file holding no schema objects at all — refusing a
brand-new EMPTY file would refuse a first run.

**Structure is not provenance.** Passing the recognition check means the
file has the shape a CoreSight store at its recorded version always has; it
does not prove CoreSight wrote the file or that its content is authentic —
the format carries no such evidence. A full-structure lookalike is
therefore treated as the store it claims to be, and everything read from it
is judged by the normal decode rules (strict fields, typed
corruption/caller-input errors). The alternative — distrusting every store
because authorship cannot be proven — would refuse real stores too.

### Honest limitations

- The store is preserved, not recovered. Quarantine makes the data
  *findable*; nothing here can repair a damaged file.
- **Quarantine identity is bound, not atomically enforced.** The identity is
  re-checked as the last act before the rename, which detects a substituted
  file, a restored file, or ordinary filesystem activity. It cannot close the
  single-syscall window an ADVERSARIAL same-user process would need to rename
  its own file onto the path between that check and the `rename` itself: no
  portable primitive does (the alternative, holding a handle open across the
  rename, is exactly what Windows forbids). Losing that race relocates a file
  that was not the observed damaged one — it is moved, not deleted or
  repaired, and the error reports the relocation that actually happened — so
  the harm is bounded to a misplaced file, never data loss.
- **Concurrent opens wait at most `OPEN_BUSY_TIMEOUT` (5 s) for a lock.** A
  holder that outlasts that bound makes the opener fail closed with SQLite's
  own `SQLITE_BUSY`; the store is left valid and openable, and the caller may
  simply retry. The bound is deliberately finite rather than an unbounded
  wait.
- **The mutation-gate re-validation and migration are coherent, but the
  validation itself is not held under one lock the whole way.** The lock
  guarantees each migration decision and its SQL; the earlier recognition
  verdict is re-confirmed by the bootstrap and by that per-step re-read, so a
  store that changes mid-open is either migrated correctly or refused, never
  migrated from a stale plan.
- **Corruption found at the mutation gate is refused in place, not
  quarantined.** Gate 2's reads run on a read-write connection (the one
  that would write), and SQLite may discard or rebuild a `-wal`/`-shm` that
  does not match the database on such reads. Quarantine decisions are
  therefore made only from gate 1's read-only evidence; damage that only
  becomes visible at gate 2 is reported with the store left at its path.
  The window is narrow (the file was healthy at gate 1, and nothing but a
  substitution can change it before gate 2), and a store that is corrupt on
  every open still takes the normal path: gate 1 establishes it from
  read-only evidence and quarantines it.
- **Structure is not provenance.** A substitute that carries the full
  structure of a supported version is validated and handled as that store,
  not refused: the file format cannot prove authorship (see "Structure is
  not provenance" above). What a swap cannot do is reach the writes without
  passing the gate that owns them.
- An integrity check that returns no rows aborts the open with the store
  left where it is: nothing is quarantined and nothing is backed up, and
  the error says integrity could not be established. (Quarantining on an
  empty result would move a store on a decision that was never made.)
- A `-shm` beside a `-wal` is SQLite's derived WAL index. Reading a
  database through a `-wal` can rebuild it (its bytes are not stable by
  construction), and it holds no committed data. The byte-identity contract
  therefore covers the database file and its `-wal` — for the `-shm` this
  code promises only that it is never deleted and never interpreted.
- Publication is `remove` + `rename`, not one atomic step, so a crash in
  between can leave a stray EMPTY file under the backup name. The name is
  only ever published by renaming an already-verified file, so a name that
  exists via this path always held a verified backup — except in that
  crash window. `renameat2(RENAME_NOREPLACE)` would close it but is not
  portable; no dependency is added for it.
- Quarantine relocates files next to the store. It does not encrypt, upload,
  or otherwise transmit anything, and the crate still contains no logging or
  telemetry — paths reach only the returned error.
- A store that fails to open for a reason that is none of corruption, a
  schema mismatch, or an unrecognized file is simply reported; nothing is
  moved.

## Corruption & recovery

- `PRAGMA integrity_check` runs on EVERY open, before anything reads or
  migrates the file (IMPLEMENTED, Phase 6.1 audit: `HistoryStore::open`
  refuses a corrupt store with the typed `StoreError::Corrupt` — never
  guess, never half-read); AGAIN at the mutation gate, on the read-write
  connection and before any write (Phase 6.5.2); and a THIRD time after the
  bootstrap helper's own writes (IMPLEMENTED, Phase 6.5.1), so corruption
  those writes introduce is quarantined like any other instead of being
  left at a path that keeps failing. Every site is closed: only a non-empty
  all-`ok` result is health (Phase 6.5.2).
- A valid SQLite file that is not a recognizable CoreSight store is refused
  UNWRITTEN (`StoreError::UnrecognizedSchema`, Phase 6.5.1; full
  per-version structure contract since 6.5.2) instead of having CoreSight
  tables added to it — see "A file this build cannot recognize is refused,
  never initialized over" above.
- Corrupt stores are now QUARANTINED (IMPLEMENTED, Phase 6.5) rather than
  only refused: see "Pre-migration backup and corrupt-store quarantine"
  above. Still PLANNED, and deliberately out of scope for 6.5: starting a
  fresh store automatically and offering a rescan/repair UI flow.
- Migrations remain atomic per step, with the version bump inside the same
  transaction — a failed migration rolls back rather than leaving a
  half-migrated database. Each step is serialized against concurrent writers
  on SQLite's write lock and re-reads the recorded version inside it
  (Phase 6.5.3), so two openers apply each migration exactly once. Phase 6.5
  adds a verified backup in front of that, because a rollback protects the
  *logical* state, not the pre-migration bytes on disk against an
  unrecoverable failure.
- Scans are idempotent and resumable-by-restart: a crashed scan leaves a
  `failed` record and zero partial visibility (snapshot publishes atomically).
- Phase 6.4 snapshots follow the same rule: commit is one transaction, and
  re-committing the same run's snapshot deletes its prior rows inside that
  transaction — retrying never creates semantic duplicates. Foreign-key
  violations (a child row for a missing application fact) are refused by
  the schema rather than silently orphaned.
