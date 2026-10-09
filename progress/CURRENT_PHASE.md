# CoreSight — Current State

PHASE 6.4 — Persistent Application Intelligence + System-Model Snapshot
Integration

**Status: VERIFIED**

- **Final verified commit:** `5b18ef32b59d426bb18914e2ae081c4944b4db60`
  ("fix: require parallel snapshot application facts"), pushed to `main`.
  Exact-SHA CI run
  [37936917961](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37936917961),
  attempt 1, conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

- **Implementation commit:** `aaff0a3bb16701f9c0b19327911fa358ade08a77`
  ("feat: add phase 6.4 persistence and snapshot integration") — also
  verified green on its exact SHA: CI run
  [37934408329](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37934408329),
  attempt 1, all four jobs success.
- **Migration / schema version:** **5** (`HISTORY_SCHEMA_VERSION` 4 → 5),
  forward-only migration `v4 → v5` in one transaction with its version
  bump.
- **Verified baseline (pre-6.4):** `2e7999251fb4df24b1fa6f638be34228d6b1ebde`.
- **Product direction (binding):** CoreSight is a **macOS system
  intelligence + power-tools application** — NOT a "Mac cleaner". macOS is
  the primary implementation and launch platform; Windows/Linux remain
  architectural targets with their abstractions intact. Storage is one
  pillar of seven (docs/MACOS_ARCHITECTURE.md).
- **Last updated:** 2026-10-09.

**Destructive execution remains unimplemented.** Phase 6.4 added no GUI,
no IPC, no frontend surface, no cleanup/uninstall/delete/kill, no
subprocess, no network, no cloud sync, no licensing, and no package-manager
execution. Persisted candidates stay inert: `can_authorize_execution` and
`candidate_is_authorized` return `false` for every reloaded model and
candidate, asserted by test. Persisting a snapshot can never imply that an
observation succeeded or that any action is safe.

## Phase 6.4 — what was implemented

**Objective:** durable storage of canonical application-intelligence facts
and persisted system-model snapshot inputs, with deterministic rehydration
into the same validated system model.

- **Schema v5 (forward-only, transactional).** One migration
  `v4 → v5` in `coresight-history`, applied with its version bump inside a
  single transaction. `HISTORY_SCHEMA_VERSION` 4 → 5. Thirteen new
  normalized `app_snapshot_*` tables; the existing v2–v4 migrations and
  their SQL are untouched.
- **Normalized persistence, no blob.** Artifact facts, the full
  `ApplicationRecord`, unioned provenance, raw views, install roots,
  structured ownership evidence, footprint candidates + evidence, source
  coverage, identity-relationship facts, quoted historical context, and
  the inventory/footprint truncation counters each persist as their own
  columns/rows. There is no `system_model_json` table.
- **Per-run association.** Every row carries `run_id` with
  `ON DELETE CASCADE` to `scan_runs`, so snapshots live in the existing
  history timeline and are pruned with their run. A run with no snapshot
  reloads as *absent* (`None`), never as an empty snapshot.
- **One canonical construction path.** The persistence layer returns
  validated `SystemModelInput` facts; both a fresh build and a reload end
  at `build_system_model` → `finalize` → `check_invariants`. Derived
  state (indexes, edges, claims, resolution states, insights, candidates)
  is never stored.
- **Ceilings survive the database.** Reload re-clamps every stored
  evidence strength through `OwnershipEvidence::new`
  (`min(requested, kind ceiling, group ceiling)`); a tampered over-claim
  returns weakened. Application ids are re-verified against normalized
  `(name, publisher)`, so no second identity definition can enter.
- **Honesty preserved.** `AccessState` (7 states), source
  `Unsupported`/`Unavailable`/`Failed`/`Partial`, full-width
  `ObjectIdentity`, lossless `u:/e:/l:` paths, and every truncation
  counter round-trip verbatim. Incomplete knowledge never reloads as
  `Unassociated`, `Orphan`, `Complete`, `Resolved`, or `Safe`.
- **Corruption fails closed.** Strict typed decoding for unknown enums,
  malformed paths, half/impossible identities, negative sizes and counts,
  partial classifications, impossible flags, malformed correlation-group
  shapes, orphaned foreign keys, and impossible application identity —
  each surfaced as `StoreError::Corrupt` / a SQLite constraint failure
  with table + column context, never a defaulted value.
- **Bounded, deterministic IO.** Loads are `WHERE run_id = ?` with an
  explicit canonical `ORDER BY` and a `QueryLimits` cap per section;
  commits use prepared statements in one transaction with ordinals
  assigned after a canonical sort. Insertion order, row order, and ordinal
  values are never semantic (proven by a permutation test). A cap hit is
  detected exactly (`LIMIT limit + 1`), REPORTED
  (`is_load_truncated()` / `load_truncated_sections`), and REFUSED by
  `rebuild_system_model` with `StoreError::SnapshotBounded`: a model is
  never built from a prefix of the stored facts, so a bounded read can
  never be mistaken for a complete machine.
- **Performance coverage.** An ignored persistence suite (run by CI on all
  three platforms) measures batch insertion, snapshot load,
  application-heavy and evidence-heavy shapes, per-run load isolation
  across many stored snapshots, and re-commit stability. Measured locally:
  10k artifacts commit 144 ms / load 62 ms; 100k artifacts commit 2.0 s /
  load 0.72 s (linear); application-heavy (2,000 apps) 363 ms / 758 ms;
  evidence-heavy (3,200 evidence items) 164 ms / 155 ms; a per-run load
  with 12 stored snapshots 5 ms.
- **System-model crate untouched.** `coresight-system-model` gained no
  dependency and no persistence code; it remains pure, platform-neutral,
  and database-independent, still guarded by its source-scan test.
- **No execution, ever.** No GUI, no IPC, no destructive or subprocess
  capability, no network, no cleanup/uninstall/kill. Persisting a snapshot
  can never manufacture authorization: `can_authorize_execution` and
  `candidate_is_authorized` still return `false` for every reloaded model
  and candidate, asserted by test.

## Verification (local, 2026-10-09, Windows 11 GNU)

- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  — clean.
- `cargo test --workspace` and `cargo test --workspace --all-features` —
  **803 passed, 0 failed** (baseline was 661; +142, including 51 new
  Phase 6.4 persistence tests: 42 integration + 9 codec, plus 3 ignored
  performance tests).
- `cargo test --workspace -- --ignored` — all established performance
  suites green.
- `npm ci` (0 vulnerabilities) + `npm run build` — green.
- `git diff --check` — clean.

## CI verification (exact SHA)

Implementation commit `aaff0a3bb16701f9c0b19327911fa358ade08a77` — run
[37934408329](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37934408329),
attempt 1, **success**:

```text
frontend               success
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
```

Commit-contract hardening `5b18ef32b59d426bb18914e2ae081c4944b4db60` —
run
[37936917961](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37936917961),
attempt 1, **success**:

```text
frontend               success
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
```

Both runs verified the exact SHA (not merely the branch); no runner
infrastructure issues occurred and no source change was made to work
around a failure. This verification-record change is documentation-only.

## Phase 6.4 limitations (honest)

- **Snapshots inherit run retention.** A snapshot is pruned with its run
  (FK cascade); there is no separate snapshot-retention policy, and no
  cross-run "latest snapshot for an application" query yet — callers list
  runs and load the one they need.
- **`i64::MAX` ceiling on stored counts/sizes.** The store's `INTEGER`
  columns are signed (the repository-wide convention). A `u64` above
  `i64::MAX` fails loudly as corruption instead of narrowing. Object
  identity is unaffected: it persists as bit-patterns, so `u64::MAX`
  components round-trip.
- **Evidence is re-clamped, not rejected, when only the strength is
  over-claimed.** A tampered strength returns at its legitimate ceiling
  (matching in-memory transport behavior); malformed *shape* (unknown
  enum, bad group, malformed path) is rejected outright.
- **Snapshot loads are per-section bounded.** `QueryLimits` caps each of
  artifacts/applications/evidence/relationships/history independently, so
  a snapshot larger than the limit loads truncated rather than failing —
  the repository's existing bounded-query convention.
- **No runtime platform validation beyond CI.** Phase 6.4 is
  platform-neutral Rust with synthetic fixtures; real-world macOS/Windows/
  Linux runtime evidence comes only from the CI jobs.
- **Destructive execution remains unimplemented.** No GUI, no IPC, no
  cleanup/uninstall/delete/kill, no subprocess, no network, no package
  manager. Persisted candidates stay inert and unauthorized.

## Historical record: Phase 6.1 (2026-10-07)

Independent audit of the Phase 6.1 commit (no prior report trusted). Code,
tests, dependency graph, docs, and git state were re-inspected; the
following findings were repaired:

1. **P0 — object identity was narrowed to 64 bits at the duplicate/-
   relationship boundary.** The hash pipeline published
   `object_id: Option<(u64, u64)>` (discarding `FILE_ID_INFO` high bits),
   and `relationships::ObjectRef` carried only the pair, while history's
   `ObjectId` carried the full `(device, inode, file_id_hi)` — two
   competing definitions. REPAIRED with one canonical type,
   `coresight_identity::ObjectIdentity { volume, file_id,
   file_id_hi: Option<u64> }`, now flowing pipeline → duplicate members →
   relationships → ids → index. Guarantees, all regression-tested:
   - `(1,2,hi=3)` and `(1,2,hi=4)` are distinct objects; their alias-id
     fragments and relationship ids cannot collide (`…-hi:…` fragment).
   - `(1,2,None)` (narrow/legacy) and `(1,2,Some(3))` (wide) NEVER
     compare equal; a group mixing provability on one low pair degrades
     to `Estimated` + `distinct_objects: None` instead of fabricating a
     count. Two members that both PROVED different high bits stay exact.
   - Narrow identities keep the historical id fragment shape (id
     stability for Unix/macOS).
   - History reconstruction now restores EXACTLY the persisted identity:
     `relationship_members.file_id_hi` (persisted since migration v4) had
     been silently ignored on read; malformed persisted alias ids are now
     typed errors instead of silent `None`.
2. **Application identity vs inventory merge (semantic conflict).**
   `ApplicationId` hashed (name, publisher, source) while `merge_inventory`
   keyed on (name, publisher) — the merged record's id depended on which
   record won. DECIDED and DOCUMENTED (no guessing): **a logical
   application is identified by its normalized (name, publisher); the
   discovery source is provenance, never identity.** `ApplicationId::derive`
   now hashes exactly the merge key; cross-source same-(name, publisher)
   records merge into one logical application with unioned provenance
   (regression-tested, replacing the old source-sensitive pin test).
3. **Determinism — arrival-order winner selection.** Equal-completeness
   inventory ties kept the first-arrived record. REPAIRED with a canonical
   precedence function (`record_rank`: completeness, then a fixed
   field-by-field content order); footprint same-key duplicates resolve
   canonically too. Permutation tests now require byte-identical output.
4. **True boundedness (bounds moved to where memory grows).**
   - `PathProber` materialized whole directories before capping → the
     trait is now `children_bounded`/`entries_bounded` (streaming top-K,
     O(max) memory, canonically-smallest subset, exact overflow).
   - Win32 registry enumeration materialized every subkey → the trait is
     now `subkeys_bounded` (streaming top-K over `RegEnumKeyExW`, exact
     truncation + oversized-name accounting; `key_present` asks for one
     key, not the world).
   - `discover_footprints` accumulated the full candidate fan-out before
     truncating → bounded admission (canonically-first `max_records`
     candidates, exact `candidates_truncated`).
   - `merge_inventory` accumulated every record before truncating →
     bounded admission with exact per-record accounting.
5. **Lossless path audit (sweep of every lossy conversion).** Semantic
   repairs: `classifier::classify::split_name` (a non-UTF-8 path previously
   collapsed to an empty name → misleading `Unknown`; now byte-level
   splitting, regression tests); `classifier::pathctx::analyze` (non-UTF-8
   paths previously lost ALL location context; now whole-path lossy
   rendering preserves every valid component). Presentation-only and
   documented-boundary uses (registry UTF-16 decode, volume labels,
   history's lossless tagged encoder) classified and left as-is.
6. **History/SQLite second-order audit.** Migrations are per-step
   transactional with the version bump inside the transaction; newer
   schemas are refused; extreme-value identity round-trips and
   display-collapse event-id tests exist and pass. docs/DATABASE.md
   promised `PRAGMA integrity_check` + pre-migration backup + quarantine
   that did not exist — `PRAGMA integrity_check` is NOW run on every open
   before anything reads/migrates (typed `StoreError::Corrupt` refusal);
   backup + quarantine are marked PLANNED in the doc (no more conceptual
   promises).
7. **Safety / honesty / boundary audits.** Observation envelope (tagged
   enum + `deny_unknown_fields`) cannot represent payloads on
   unsupported/unavailable/failed states, in memory or on the wire;
   `AccessState` keeps denied ≠ empty ≠ absent ≠ unsupported; the macOS
   TCC source is pinned as a `Mechanism` that is `RequiresFullDiskAccess` +
   `Deferred` (no path to probe even by accident); the capability status
   table is pinned by tests; the no-executor and no-network sweeps over
   the new crates are clean (only serde `rename_all` attribute matches);
   shared-crate source-scan tests ban OS conditionals.

## Verification (local, 2026-10-07, Windows 11 GNU, Rust 1.99.0)

- `cargo fmt --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  — clean.
- `cargo test --workspace` and `cargo test --workspace --all-features` —
  **588 passed, 0 failed** (2 ignored perf suites run separately).
- `cargo check -p coresight-capabilities -p coresight-macos
  -p coresight-engine --all-targets --target x86_64-apple-darwin` — green;
  same for `x86_64-unknown-linux-gnu` — green (compile-level only; macOS
  runtime paths await macOS CI — never claimed verified from compilation).
- `npm ci` + `npm run build` — green.
- `git diff --check` — clean.

## Known limitations

- macOS runtime observation is compile-checked locally; runtime evidence
  can only come from the macOS CI job (never from Windows/Linux builds).
- Unix-gated unit tests (mount decoding, non-UTF-8 classifier fixtures)
  run on Linux/macOS CI only.
- Pre-migration file backup and corrupt-store quarantine flow: PLANNED
  (docs/DATABASE.md) — the integrity check refuses corrupt stores today.
- Pre-Phase-6.1 capability machinery is unbounded only where the phase
  contracts say so (e.g. relationship reports are transitively bounded by
  pipeline caps; the boundedness sweep covered apps/macOS/capabilities).

## Historical Phase 6.1 CI

**CI VERIFIED (2026-10-07): run 37589662213 for commit `94c3164` —
rust ubuntu SUCCESS, rust windows SUCCESS, rust macos SUCCESS,
frontend SUCCESS.**
https://github.com/OG-Huzzi/SpaceLens/actions/runs/37589662213

(The first verification push `d88b98b` failed CI clippy on
ubuntu/macos — unix-only test fixtures missing a trait import, a class
of failure invisible on the Windows dev host; repaired in `94c3164`,
all gates re-run locally, cross-target clippy clean.)

## Historical Phase 6.1 gate status

At that checkpoint, later work had not been authorized until its review and
CI record completed; Phase 6.2 persistence was NOT STARTED. This is archival
history only. The current Phase 6.3 gate status is recorded at the top of
this file.
