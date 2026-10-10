# CoreSight — Current State

PHASE 6.4 — Persistent Application Intelligence + System-Model Snapshot
Integration

PHASE 6.4.1 — Deep Integrity Hardening (application identity, footprint
fidelity, numeric/query-bound safety, integrated persistence audit)

**Status: Phase 6.4 VERIFIED ** — exact-SHA record below. Phase 6.4.1 is
IMPLEMENTED and VERIFIED with its own exact-SHA CI record.

- **Final verified commit (Phase 6.4):**
  `e05177bb2dca8a60007ba994c3a3acd62ba1edd4`
  ("fix: enforce snapshot input consistency and report every capped
  section"), pushed to `main`. Exact-SHA CI run
  [38034695355](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38034695355),
  attempt 1, conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

  This run executed the full gate on all three platforms, including the
  Phase 6.4 persistence performance suite
  (`cargo test -p coresight-history -- --ignored`).

- **Implementation commit:** `aaff0a3bb16701f9c0b19327911fa358ade08a77`
  ("feat: add phase 6.4 persistence and snapshot integration") — exact-SHA
  CI run
  [37934408329](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37934408329),
  all four jobs success.
- **Follow-up commits, each exact-SHA verified green:** `5b18ef32b59d426bb18914e2ae081c4944b4db60`
  ("fix: require parallel snapshot application facts") — run
  [37936917961](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37936917961);
  `1725465f48dac322a490c2905c557cbf9d1710fd`
  ("docs: record Phase 6.4 verification") — run
  [37939190779](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37939190779).
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
- **Commit inputs must describe the same facts being stored.** The two
  application vectors a commit accepts are verified PARALLEL and equal
  field-for-field (record, install roots, executable, associations), and
  the footprint report's candidates must equal the canonical union of the
  per-application footprints. A divergence is rejected before the
  transaction opens — only `app_facts` is persisted, so without this a
  caller could silently store a snapshot that does not describe the facts
  the model was built from.
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
  explicit canonical `ORDER BY` and a `QueryLimits` cap; commits use
  prepared statements in one transaction with ordinals assigned after a
  canonical sort. Insertion order, row order, and ordinal values are never
  semantic (proven by a permutation test). A cap hit in ANY section —
  including the per-application details (provenance, views, install roots,
  evidence, footprints) — is detected exactly (`LIMIT limit + 1`),
  REPORTED (`is_load_truncated()` / `load_truncated_sections`), and
  REFUSED by `rebuild_system_model` with `StoreError::SnapshotBounded`: a
  model is never built from a prefix of the stored facts, so a bounded
  read can never be mistaken for a complete machine. The inventory-only
  read carries the same signal (`LoadedApplications`).
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
  **807 passed, 0 failed** (661 at the pre-6.4 baseline). Phase 6.4 adds
  **54 test functions**: 47 integration (`tests/snapshot_tests.rs`; 46 run
  on Windows — one is Unix-gated for non-UTF-8 paths), 4 strict-codec unit
  tests, and 3 ignored performance tests.
- `cargo test --workspace -- --ignored` — all established performance
  suites green, including the new Phase 6.4 persistence suite (batch
  insertion, snapshot load, application-heavy, evidence-heavy, per-run
  load isolation, re-commit stability).
- `npm ci` (0 vulnerabilities) + `npm run build` — green.
- `git diff --check` — clean.

## CI verification (exact SHA)

Every commit of this phase was verified on its own SHA — never merely on
the branch:

| Commit | Meaning | CI run | Result |
|---|---|---|---|
| `aaff0a3bb16701f9c0b19327911fa358ade08a77` | Phase 6.4 implementation | [37934408329](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37934408329) | all 4 jobs success |
| `5b18ef32b59d426bb18914e2ae081c4944b4db60` | parallel-facts contract | [37936917961](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37936917961) | all 4 jobs success |
| `1725465f48dac322a490c2905c557cbf9d1710fd` | verification record | [37939190779](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37939190779) | all 4 jobs success |
| `c8223d5fe2d3fbb98ac371175412c033fbc4a59a` | bounded-load honesty + persistence perf suite | [37953360824](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37953360824) | all 4 jobs success |
| `61b3ad8de4af5fa84085129226c152f8f8eb4d6f` | verification record | [37955117327](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37955117327) | all 4 jobs success |
| `6f1f1d3ca8729b6fe42816aa2d95fa67690beadf` | capability-state note (docs) | [37956977056](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37956977056) | all 4 jobs success |
| `b209327d99ceeb2b5a2f7bbb4272a5c0f87edd5b` | verification record | [37958800982](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37958800982) | all 4 jobs success |
| `e05177bb2dca8a60007ba994c3a3acd62ba1edd4` | independent-audit fixes (**final**) | [38034695355](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38034695355) | all 4 jobs success |

Every run reported `frontend`, `rust (ubuntu-latest)`,
`rust (windows-latest)`, and `rust (macos-latest)` as **success** on
attempt 1. No runner-infrastructure failure occurred, so no retry was
needed and no source change was ever made to work around a failure.

**Independent audit.** After the phase was first marked verified, a
read-only adversarial audit of the persistence path was run against the
frozen code. It confirmed the isolation, evidence-ceiling, determinism,
and 6.3-regression claims, and found five real gaps that are now FIXED and
regression-tested in `e05177b`: capped per-application detail reads were
not reported (a partial load could reach the builder); the commit's
parallel-input check compared only ids, not records/roots/executable; the
footprint report's candidates could disagree with the stored per-app
facts; `load_snapshot_applications` returned a truncated list with no
signal; and `probe_limit` could overflow to a negative SQLite `LIMIT`
(silently unbounded). A documentation test-count error was corrected too.

Phase 6.4's implementation code is therefore frozen at
`e05177bb2dca8a60007ba994c3a3acd62ba1edd4`; any later commit in this
phase is documentation-only and is still run through the full gate on its
own SHA.

## Phase 6.4.1 — Deep Integrity Hardening

Verified against baseline `e05177bb2dca8a60007ba994c3a3acd62ba1edd4`
(the Phase 6.4 record above is preserved unchanged).

**Status: VERIFIED**

- **Implementation commit:** `c6b5ad64fe975a16b57ba7347e443fbec7746dae`
  ("feat: phase 6.4.1 deep integrity hardening"). Exact-SHA CI run
  [38054012906](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38054012906),
  attempt 1, conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

- **Final commit:** `a8d823a89357d64e227ef09204c541b3d5a33845`
  ("fix: validate inventory records and hoist numeric checks before the
  transaction"), pushed to `main`. Exact-SHA CI run
  [38055710246](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38055710246),
  attempt 1, conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

  Both runs executed the full gate on all three platforms, including the
  persistence performance suite (`cargo test -p coresight-history --
  --ignored`).

### Migration version

**6** (`HISTORY_SCHEMA_VERSION` 5 → 6). Schema v6 exists solely to
record which identity encoding each application row was written with and
to re-key ids persisted under the Phase 6.4 delimiter-joined encoding.

### Found and fixed

Each finding was verified empirically before any change.

1. **Ambiguous application identity (Workstream A).** `"A|B"|"C"` and
   `"A"|"B|C"` hashed identically (confirmed by probe), so two distinct
   logical applications shared one `ApplicationId`. Replaced with a
   length-prefixed encoding, which is injective for all inputs. A
   **safe forward-only migration** re-keys per stored fact (never a
   global replace), so a legacy id that conflated several
   `(name, publisher)` pairs splits back into the distinct identities it
   was conflating, and every child row follows its own parent. Rows whose
   stored id matches NEITHER encoding are refused as corruption.
   `id_encoding` records the encoding and the loader verifies it, so a
   v6 store cannot silently trust a legacy id.
2. **Arrival-order-dependent footprint reconciliation (Workstream B).**
   Same-key footprint candidates were reconciled by `dedup_by` after a
   sort that did not include the rank, so the survivor depended on the
   input order (confirmed by probe) — and, worse, the rank itself used
   `Confidence`'s derived `Ord`, which puts `Confirmed` LOWEST, so
   "stronger confidence wins" selected `Unknown`. Now: one explicit
   strength order shared with the producer, a total-order sort, and the
   same reconciliation at commit and load. Distinct-key candidates are
   all preserved; a footprint attributed to a foreign application is
   **rejected** rather than silently relocating a scope.
3. **Unchecked numeric conversions (Workstream C).** Sizes, estimated
   sizes, counters, and ordinals were written with `as i64`, which wraps
   silently. Now checked (`checked_u64_i64`, `checked_counter_i64`,
   `checked_ord_i64`) before the transaction mutates the previous
   snapshot, with the prior snapshot preserved on rejection. Object
   identity remains an intentional bit-pattern conversion.
4. **Query-bound edge cases.** Cap detection is now proven exact at
   `limit`, `limit+1`, `limit-1`, `0`, and `i64::MAX`/`usize::MAX` (the
   probe row can never become a negative SQLite `LIMIT`), and every
   nested section (including relationship members and footprint
   evidence) reports through the same truncation signal that refuses the
   rebuild.

### Tests

Local gate for this work (run, not assumed):

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace`,
  `cargo test --workspace --all-features`, `cargo test --workspace --
  --ignored`, `npm ci`, `npm audit --audit-level=high` (0 vulnerabilities),
  `npm run build`, `git diff --check`.
- New regression tests: collision-free identity (injectivity,
  normalization, Unicode, empty/absent publisher, cross-source merges,
  inventory/model key agreement, legacy-key recognition), legacy v5
  migration fixtures with child-row attribution and re-key verification,
  footprint reconciliation order-independence, distinct-fact
  preservation, foreign attribution rejection, inventory agreement, and
  the full numeric and query-bound matrix.
- Extended the performance suite with identity-heavy and footprint-heavy
  shapes alongside the existing application/evidence/many-snapshot ones.

Measured locally (Windows 11 GNU, debug profile, isolated runs):

| Workload | Commit | Load |
|---|---|---|
| artifact-heavy 10k artifacts | 202 ms | 86 ms |
| artifact-heavy 100k artifacts | 2,399 ms | 702 ms |
| application-heavy 2,000 apps | 494 ms | 859 ms |
| evidence-heavy 3,200 items | 207 ms | 92 ms |
| identity-heavy 1,500 apps | 329 ms | 789 ms |
| footprint-heavy 6,000 candidates | 675 ms | 661 ms |
| per-run load with 12 stored snapshots | — | 8 ms |

Scaling is linear in the artifact count (10× the data for ~8× the load
time); the scaling guard is deliberately loose because these tests run
concurrently with the other ignored suites and therefore carry scheduler
noise a solo run does not.

### Second independent audit

After the first green gate the diff was re-audited as if written by
another engineer. Findings that were confirmed and fixed:

5. **`Inventory::records` was accepted but never validated** — a caller
   whose inventory disagreed with the committed application facts would
   have had its truncation counters persisted describing records that
   were never stored. Now the inventory's records must describe exactly
   the set of application ids the snapshot stores (a SET comparison,
   because the inventory is the deduplicated merged result while the
   model input is a fact multiset — two facts under one id are two
   records). Regression-tested both ways.
6. **Numeric conversions happened inside the snapshot transaction** —
   the values were checked before the first row was WRITTEN, but after
   the previous snapshot's rows had been DELETED, so a rejection relied
   on transaction rollback rather than never touching the data. All
   conversions now run in a pre-pass BEFORE the transaction opens, and a
   test proves a rejected replacement preserves the previous snapshot
   byte-for-byte.

Audit points confirmed clean: identity injectivity (property test over
component boundaries, embedded separators, Unicode, empty components);
migration child attribution (the legacy-id-conflation fixture proves both
children follow their own parent); footprint order-independence; numeric
representability; extreme limits; capped-snapshot rejection; strict
decode of every persisted field; lossless paths and full-width identity;
untouched historical migrations; no manufactured evidence or
authorization; `coresight-system-model` purity (zero diff, no SQLite
dependency, source-scan guard intact).


### Phase 6.4.1 limitations (honest)

- Same Phase 6.4 limitations above remain in force (no pipeline caller,
  run-retention coupling, i64::MAX size ceiling, re-clamped rather than
  rejected over-claimed strength).
- Application ids derived BEFORE this phase remain valid in the store and
  are re-keyed by the forward migration; a v5 store cannot be read by a
  build older than v6 (forward-only — such a store is refused with
  `SchemaTooNew`).
- `Confidence`'s derived `Ord` still puts `Confirmed` first (declaration
  order). Every consumer that needs "stronger wins" must use the explicit
  `footprint::confidence_strength` ranking; the enum's `Ord` is unchanged
  because changing it would alter canonical ordering elsewhere.
- Footprint reconciliation collapses same-key descriptions into the
  strictly-better one, so a weaker description's DISTINCT evidence is not
  retained separately. That is the documented set semantics (one scope);
  conflicting scopes (different path/app/kind) are never merged.

`PHASE 6.5 — NOT STARTED.`

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
- **Snapshot loads are per-section bounded, and the bound is reported.**
  `QueryLimits` caps each of artifacts/applications/evidence/relationships/
  history/footprints independently. A capped load returns the prefix AND
  names the capped sections (`is_load_truncated()`), and
  `rebuild_system_model` refuses it with `StoreError::SnapshotBounded`
  rather than building a model from partial facts. The only way to obtain
  a model is a limit large enough for the whole snapshot — so there is no
  "peek at a partial model" API, by design.
- **No runtime platform validation beyond CI.** Phase 6.4 is
  platform-neutral Rust with synthetic fixtures; real-world macOS/Windows/
  Linux runtime evidence comes only from the CI jobs.
- **Not yet wired into a pipeline.** The storage/rehydration API exists and
  is proven by test, but no scan/analysis pipeline commits or rehydrates a
  snapshot yet: the workspace has no orchestrator that owns a run
  end-to-end (the Tauri shell is a config-level contract, not a workspace
  member, and the GUI/IPC layer is a later phase). Wiring a caller is
  additive and does not change the persistence contract.
- **Application-detail reads are bounded per application fact**, not per
  run: a snapshot with many applications multiplies the per-app limit, so
  a caller sizing `QueryLimits` for a huge inventory should account for
  that (the cap is still reported exactly).
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
