# SpaceLens — Phase 5 Status

- **Phase:** 5 — System Memory & Change History
- **Verdict:** **PHASE 5 VERIFIED** (full local gate green on Windows;
  CI matrix green on the implementation SHA — runs recorded below; every
  job verified individually from the Actions API)
- **Date:** 2026-09-12 · **Machine:** Windows 11 Pro x64, 8 GB RAM
- **Starting SHA:** `273ee8c` (Phase 4 record, clean tree)

## Purpose and scope discipline

Phase 5 adds the REMEMBER layer: SpaceLens remembers previous
observations and can explain what changed between them. It provides
**trustworthy historical evidence only** — no recommendations, no cleanup
ranking, no destructive operations, no network/telemetry/AI, and no
generic SQLite-CRUD application design (a purpose-built normalized
history model on the existing database).

## What was built

**Persistence choice (Objective 1):** SQLite already existed
(`spacelens-core`, rusqlite bundled, forward-only `schema_version`
migrations, Phase 0 bootstrap schema). The new `spacelens-history` crate
**extends** that architecture — migration v2 adds `scan_runs`,
`observations`, `relationship_obs`, `relationship_members` to the SAME
database and version table. No second abstraction; no database access in
observation code (history consumes phase outputs).

**Domain model (Objective 2):** six distinct concepts — run/observation
(`RunRecord`), stable object (handle-proven `(volume, file id)`), path,
verified content (stored ONLY where Phase 3/4 produced it), stored
classification (rules-versioned), and derived historical events. Never
one vague "file record"; every `Option` is an honest unknown.

**Run/snapshot identity (Objective 3):** `RunId` (timestamp + process
entropy); statuses reuse engine conventions (`RUNNING`, `COMPLETED`,
`COMPLETED_WITH_LIMITS`, `CANCELLED`, `FAILED`); `observes_full_scope()`
gates deletion claims. Run records carry canonical roots, platform, and
the `ConfigFingerprint` (Objective 14: observation model, classifier
schema + NEW additive `RULES_VERSION` in the classifier, hash algorithm
tag, relationship/history schemas).

**Snapshots (Objectives 4, 15, 16):** normalized rows — one observation
per path per run (kind, size, object identity, modified, classification,
verified content hash, observation error) with indexes on path, object,
and content. NOT a JSON blob; keyed for the historical queries. Verified
content identities are REUSED from the Phase 3 pipeline (never re-hashed).

**Pure comparison engine (Objectives 10–13, 20–23):** `compare()` derives
typed events via keyed maps — O(n log n + m log m), database-free,
deterministic, with content-addressed event ids:

- `Created`, `Deleted`, `Moved`, `Renamed`, `Modified`, `SizeChanged`,
  `ClassificationChanged`, `RelationshipAdded`/`Removed`/
  `MembershipChanged`, `ObjectIdentityChanged`, `BecameInaccessible`/
  `BecameAccessible`. No generic "something changed".
- **Evidence ordering (Objective 21):** object identity is the sole
  continuity proof → `Moved`/`Renamed`; an object GAINING a path is an
  added alias (`Created` with continuity evidence), not a move; names,
  sizes, timestamps are never proof.
- **Modification semantics (Objective 22):** `Modified` requires verified
  content identities on both sides (same object, differing digest);
  size-only evidence is `SizeChanged`; a different object at the same
  path is `ObjectIdentityChanged` (replacement), never a modification of
  the old object.
- **Incomplete-scan safety (Objective 12 — hard invariant):** created/
  deleted claims require BOTH runs to have observed full scope; partial
  runs suppress exactly those kinds. Scenario A (1000 → 100 observed)
  produces ZERO deletion events.
- **Scope (Objective 13):** comparisons are rejected unless the target
  roots cover the source scope.
- **Relationship history (Objective 23):** compared by stable Phase 4
  ids (content/object derived — identical facts yield identical ids, no
  churn); added/removed/membership-changed typed separately; alias-vs-
  copy transitions detectable by the kind split.
- **Boundedness:** `CompareOptions::max_events` (default 100k) with an
  exact truncation counter; per-kind counts describe the derived set.

**Store (Objectives 15–19, 27, 28):**

- Atomic run commit: `begin_run` (RUNNING header) then `commit_run`
  (status + counts + full snapshot + relationships in ONE transaction) —
  a run is visible as completed only after its whole baseline committed.
- Crash recovery: deterministic — `RUNNING` rows at store open are
  marked `FAILED`. Interrupted runs are never complete baselines.
- Queries: `list_runs`, `get_run`, `latest_run_for_scope` (skips partial
  runs), `load_run_snapshot`, `history_for_path/object/content`,
  `relationship_history` — all indexed, all bounded by `QueryLimits`
  (10k default) with explicit truncation.
- Retention: deterministic policy — newest `keep_latest` committed runs
  ALWAYS kept (the comparison baseline is never silently removed),
  optional `max_runs`/`max_age`, observable `RetentionReport`.
- Privacy: local file only; no network, no logging (the crate has no
  log macros — no path ever reaches one), no telemetry.

**Tests (Objectives 29, 30): 42 new** — 28 comparison tests (scope/
completeness guards, the full change matrix, identity semantics,
relationship semantics, determinism, event-cap accounting, adversarial
scenarios A–F) + 14 store tests (run lifecycle, commit atomicity,
double-commit rejection, duplicate-path rejection, cancel/fail
persistence, crash recovery, scope-aware latest-run, cross-run path
history, object/content history, query limits, retention semantics ×2,
relationship round-trip, schema idempotence).

**Benchmark (Objective 26):** `comparison_scales_linearly` (ignored,
CI-run) — 10k/100k-entry snapshots with mixed changes; per-entry scaling
guard proves no pairwise comparison. Local: 10k → 139 ms, 100k → 1.8 s.

## Bugs found and fixed during development (the honest record)

1. **Retention floor bug** (found by the store-test suite): with both
   optional bounds `None`, `apply_retention` never pruned — `keep_latest`
   acted as a floor, not a cap. Fixed: beyond the always-keep floor, a
   committed run is removed when any rule demands it, or when no other
   bound exists (keep_latest is then the retention count).
2. **Move/create double-counting**: a moved object produced both `Moved`
   and `Created` events; pass 4 now skips pass-2-claimed destinations.
3. **Alias-gain mislabeling**: an object GAINING a path (old locations
   surviving) was mislabeled `Moved`; the engine now requires ALL old
   locations gone for a move — added aliases are `Created` with
   continuity evidence.

## Verification

- **Local (Windows):** `cargo fmt --check` ✓ · `cargo clippy --workspace
  --all-targets -- -D warnings` ✓ · `cargo test --workspace` **406
  passed / 0 failed** (364 at Phase 4; +42 Phase 5) · Phase 1–4 perf
  smokes ✓ · Phase 5 comparison-scaling smoke ✓ · release >4 GiB proof ✓.
- **CI:** recorded below after the matrix run.

## Final self-audit (answered from the code)

1. Incomplete scan → false deletions? **No** — `observes_full_scope()`
   gates both deletion and creation claims (pinned).
2. Two different scopes compared incorrectly? **No** — `ScopeMismatch`
   rejection; component-wise coverage check (pinned).
3. Moved object mistaken for delete+create? **No** — object identity is
   the sole continuity proof (pinned).
4. Delete+recreate with identical bytes mistaken for a move? **No** —
   identity differs → delete + create (pinned, Scenario D).
5. Content changes detected without timestamps? **Yes** — `Modified`
   requires differing verified hashes; size/timestamps alone give
   `SizeChanged` (pinned).
6. Relationship history distinguishes aliases from copies? **Yes** —
   kind split with object- vs content-derived ids (pinned).
7. Classification rule changes make old history ambiguous? **No** —
   stored classifications + per-run config fingerprint; differences
   surfaced (`pinned`).
8. Crashes produce falsely completed runs? **No** — single-transaction
   commit + deterministic RUNNING→FAILED recovery (pinned).
9. Retention deletes the only baseline? **No** — `keep_latest` floor
   always wins (pinned).
10. Historical storage unbounded? **No** — retention caps runs; each run
    is bounded by the engine's own caps (pinned).
11. Query results unbounded? **No** — `QueryLimits` on every
    multi-result query (pinned).
12. Old records expose sensitive paths unnecessarily? **No** — local
    store only, no logging, no network surface in the crate.
13. Comparison engine depends on the database? **No** — pure function of
    two snapshots (pinned).
14. Change output deterministic? **Yes** — shuffled-order and repeat-run
    identity tests (pinned).
15. Phase 1–4 invariants regressed? **No** — full workspace suite green.
16. Recommendations/destructive actions introduced? **No** — the layer
    only stores facts and derives evidence-backed events.
