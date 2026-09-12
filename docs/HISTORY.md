# SpaceLens — System Memory & Change History (Phase 5)

Status: implemented and verified. This document describes the
implementation as it exists — every claim is backed by a test, every
limitation is stated. **Phase 5 provides trustworthy historical evidence
only: no recommendations, no cleanup ranking, no destructive operations**
(those belong to the next intelligence layer).

## The identity stack, extended

```text
Observe → Classify → Identify → Relate
                                  ↓
                               Remember      ← Phase 5 (crates/spacelens-history)
```

## What "memory" means (six distinct concepts — never merged)

| Concept | Type | Notes |
|---|---|---|
| **Run / observation** | `RunRecord` | One scan/run: identity, scope (roots), configuration fingerprint, status, counts. |
| **Stable object** | `(volume, file id)` on `ObservedEntry` | Handle-proven filesystem object identity (Phase 3.2: Unix st_dev/st_ino, Windows FILE_ID_INFO). `None` = unprovable — never fabricated. |
| **Path** | `ObservedEntry::path` | Where something was observed. Tracked independently of object identity: a path disappearing is NOT a file deleting. |
| **Content** | `ObservedEntry::content_sha256` | Verified content identity — stored ONLY where Phase 3/4 actually produced one (accepted duplicate candidates). Never re-hashed for history; unknown stays unknown. |
| **Classification** | `ClassificationRef` | The Phase 2 category observed in that run. Stored, never re-derived with newer rules; the run's config fingerprint carries the rules version. |
| **Historical event** | `ChangeEvent` | A derived, evidence-backed statement about a change BETWEEN two runs. Derived on demand by the pure comparison engine — not persisted (they are recomputable from stored snapshots; storing them would create a second source of truth). |

## Run lifecycle & identity (Objectives 3, 18, 19)

- `RunId` is generated at run start (timestamp + process entropy) and is
  the primary key of `scan_runs`.
- Statuses: `RUNNING → {COMPLETED, COMPLETED_WITH_LIMITS, CANCELLED,
  FAILED}` — reusing the engine/pipeline conventions.
- **Atomic commit:** `begin_run` inserts the header as `RUNNING`;
  `commit_run` writes final status + counts + every observation row +
  every relationship row inside ONE transaction. A run is historically
  visible as completed only after its whole snapshot committed.
- **Crash recovery:** deterministic rule — at store open, every run still
  `RUNNING` is marked `FAILED` (single-process desktop app; a `RUNNING`
  row at open is an interrupted predecessor). Interrupted runs are never
  mistaken for complete baselines.

## Snapshot model (Objectives 4, 15, 16)

One `observations` row per observed entry per run — normalized rows, NOT
a JSON blob. Indexes make the historical questions direct lookups:
`observations(path)`, `(device, inode)`, `content_sha256`. Relationship
results are stored per run (`relationship_obs` + `relationship_members`)
keyed by the Phase 4 stable relationship ids. No premature global
normalization beyond that: correctness first, storage efficiency second
(verified content hashes and object ids are inherently deduplicating
keys).

## Comparison engine (Objectives 20–23)

`compare(from: RunSnapshot, to: RunSnapshot, options) -> ChangeSet` is
**pure**: no database, no clock, no randomness. Keyed maps (BTreeMap) on
path and object identity — O(n log n + m log m), never pairwise.

### Event types (only cleanly provable ones)

`Created`, `Deleted`, `Moved`, `Renamed`, `Modified`, `SizeChanged`,
`ClassificationChanged`, `RelationshipAdded`, `RelationshipRemoved`,
`RelationshipMembershipChanged`, `ObjectIdentityChanged`,
`BecameInaccessible`, `BecameAccessible`.

### Evidence & continuity rules

- **Object identity is the sole continuity proof.** Same object at a new
  path (with all old paths gone) → `Moved` (parent changed) or `Renamed`
  (same parent) — even when a run is partial.
- An object that *gains* a path (old locations survive) → the new path is
  `Created` with `OBJECT_IDENTITY_EQUAL` continuity evidence — an added
  alias, not a move.
- Delete + recreate with identical bytes → `Deleted` + `Created`
  (object identity differs — Scenario D). Never a move.
- **Modified** requires verified content identities on both sides that
  differ, for the same object. Size changes without verified content are
  `SizeChanged` — timestamps/sizes alone never prove a content
  modification.
- **Different object at the same path** → `ObjectIdentityChanged`
  (replacement), never `Modified` of the old object.
- **Accessibility**: error-state transitions are
  `BecameInaccessible`/`BecameAccessible` — an entry observed with an
  error still exists; it is not deleted.
- **Relationships**: compared by stable Phase 4 ids (content/object
  derived — identical facts produce identical ids, so no churn). Added /
  removed / membership-changed are typed separately.

### Incomplete-scan safety (Objective 12 — hard invariant)

`Created` and `Deleted` events require **both** runs to have observed
their full declared scope (`observes_full_scope()`: `Completed` or
`CompletedWithLimits` — the Phase 3/4 limits affect candidate work, not
path observation). A cancelled or failed run produces
`completeness: PARTIAL` and suppresses exactly those two event kinds —
an inaccessible subtree or a cancelled scan can NEVER become a mass
deletion. Continuity-proven events (moves) and two-sided observed facts
(modifications, replacements, accessibility) remain: they compare two
*observed* facts.

## Scope rules (Objective 13)

Comparisons are **rejected** (`CompareError::ScopeMismatch`) unless the
target run's roots cover the source run's scope (component-wise path
prefix). `C:\Users` and `C:\Users\Huzzi\Downloads` are different scopes;
a narrower-rooted run is never compared against a wider one as if they
were the same universe. Unscanned subtrees are never treated as empty.

## Configuration versioning (Objective 14)

Every run persists a `ConfigFingerprint`: observation model version,
classifier schema tag + rule-table version (`RULES_VERSION`), hash
algorithm tag, relationship schema, history schema. Comparisons across
different configurations surface `config_versions_differ: true` — the
events remain facts (stored classifications are compared as stored, per
Objective 8), but consumers read them against the respective configs.

## Persistence (Objectives 15–18)

- **Extends the existing architecture**: `spacelens_core::db` owns the
  connection and the forward-only `schema_version` migrations. History
  adds migration **v2** (`scan_runs`, `observations`,
  `relationship_obs`, `relationship_members`) to the same database.
  No second database abstraction; no DB access in observation code.
- WAL journal + foreign keys (inherited from core's open).
- Events are **not persisted**: they are pure derivations of stored
  snapshots (recomputable, deterministic), keeping one source of truth
  and storage bounded.

## Retention (Objective 17)

`RetentionPolicy { keep_latest, max_runs, max_age }` applied
deterministically: the newest `keep_latest` committed runs are ALWAYS
kept (the comparison baseline is never silently removed); `max_runs` /
`max_age` prune older committed runs with cascade deletes. Every removal
is reported in `RetentionReport { removed_runs, kept_runs }`. Defaults:
keep 3, cap 64 runs, 90 days.

## Boundedness (Objective 27)

- All multi-result queries take `QueryLimits` (default 10,000 rows).
- Storage is bounded by policy: retained runs × per-run rows.
- The comparison's own event list is capped (`CompareOptions::
  max_events`, default 100k) with an exact `events_truncated` counter;
  per-kind counts describe the DERIVED set (published + truncated), so
  numbers always reconcile.
- Nothing is silently discarded: caps and removals are explicit and
  counted.

## Privacy (Objective 28)

Historical paths are sensitive. The store is a local file; the crate
performs no network I/O, contains no logging (no path ever reaches a
log), no telemetry, no AI. Diagnostics that would carry historical paths
belong to a future explicit, user-initiated export feature — none exists
in Phase 5.

## Performance (Objective 26)

`comparison_scales_linearly` (CI perf smoke): 10k/100k-entry snapshots —
139 ms / 1.8 s with the per-entry scaling guard (≈1.3× per-entry growth,
far under the linear bound; BTreeMap locality explains the mild
superlinearity — effectively O(n log n + m log m)). All history lookups
are indexed; no pairwise comparison exists anywhere in the phase.

## Known limitations (honest)

1. **Content identity is sparse**: only verified duplicate candidates
   carry content hashes, so `Modified` detection works only for files
   the pipeline hashed. Files without verified content fall back to
   `SizeChanged` semantics (never fake `Modified`).
2. **Unproven object identities** (FAT-class volumes, ACL-blocked stats)
   degrade move detection: a moved file without identity on either side
   is reported as delete+create. Never guessed from names.
3. **Events are not persisted**: recomputed on demand. For very old run
   pairs the computation repeats (bounded by retention; caching belongs
   with the query layer of a later phase).
4. **Single-process recovery rule**: `RUNNING` rows at open are assumed
   interrupted (single-user desktop product). Two concurrent processes
   sharing one store file is outside the current design.
5. **Classification history compares stored facts**: a rules-version bump
   does not re-classify old snapshots; apparent "classification changes"
   across a version boundary may reflect rule evolution — the
   `config_versions_differ` flag makes this visible rather than hiding it.
6. **Relationship member detail** derives from Phase 4's capped reports
   (≤64 member paths per relationship); relationship history counts are
   exact, per-member lists may be truncated exactly as upstream.
