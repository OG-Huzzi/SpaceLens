# CoreSight — Current State

PHASE 6.4 — Persistent Application Intelligence + System-Model Snapshot
Integration

PHASE 6.4.1 — Deep Integrity Hardening (application identity, footprint
fidelity, numeric/query-bound safety, integrated persistence audit)

PHASE 6.4.2 — Persistence Boundary, Query Bounds & Retention Safety

PHASE 6.4.3 — Read-Side Integrity & Exact Boundary Semantics

PHASE 6.4.4 — Numeric Preflight Closure & Relationship Metadata Consistency

PHASE 6.4.5 — Timestamp Error Semantics & Legacy History Compatibility

**Status: Phase 6.4 VERIFIED ** — exact-SHA record below. Phase 6.4.1,
6.4.2, 6.4.3, 6.4.4 and 6.4.5 are IMPLEMENTED and VERIFIED, each with
its own exact-SHA CI record.

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

## Phase 6.4.3 — Read-Side Integrity & Exact Boundary Semantics

Baseline: `9fa856edda45d9c24a464c2b869a478efb92a891` (the Phase 6.4.2
verification-record commit). **Schema v6 remains unchanged** — no
migration was added, and no migration v2–v6 SQL was edited.

**Status: VERIFIED**

- **Final implementation commit:** `7ce1c401a7d9b854388436b326c6aeaaa987e033`
  ("feat: phase 6.4.3 read-side integrity and exact boundary semantics"),
  pushed to `main`. Exact-SHA CI run
  [38152399413](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38152399413),
  event `push`, attempt 1, `head_sha` confirmed equal to the commit SHA,
  conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

  This run executed the full gate on all three platforms, including the
  persistence performance suite (`cargo test -p coresight-history --
  --ignored`). The verification-record commit is verified on its own
  exact SHA below.

### Confirmed defects, fixes, and regression tests

**D1. Legacy truncation counts were clamped to zero, erasing the
incompleteness signal (Workstream A).** The pre-6.4.2 writer used an
unchecked `u64 as i64`, so a legitimate `rel_truncated` above `2^63` was
stored as a NEGATIVE SQLite integer. The read path used
`.max(0) as u64`, which clamped it to **0** — presenting a truncated run
as a complete one. Confirmed by probe: a legacy value of
`10_000_000_000_000_000_000` read back as `0`.
Fix: `legacy_counter_from_stored` reconstructs the round-tripped value
faithfully.
- `phase643_tests::a_legacy_negative_rel_truncated_round_trips_and_keeps_its_meaning`
- `phase643_tests::a_positive_rel_truncated_round_trips`
- `phase643_tests::a_null_rel_status_means_no_report`
- `phase643_tests::the_negative_value_ambiguity_is_documented_not_hidden`

**Compatibility policy, stated explicitly:** a negative stored value is
AMBIGUOUS — a legacy wrapped counter and a tampered value are
indistinguishable in the stored format. The store does NOT claim to tell
them apart; it reconstructs the round-tripped value, the only reading that
preserves every historical value the old writer could produce. `recoverable`
already followed this rule in both read paths and needed no change.

**D2. `i64::MAX` was an accepted limit that silently degraded `limit + 1`
detection (Workstream B).** `MAX_SUPPORTED` was `i64::MAX`, so the probe
had to saturate exactly at the boundary — "exact detection" quietly became
approximate for the largest allowed value. Fix: `MAX_SUPPORTED` is now
`i64::MAX - 1`, `to_sql_limit` rejects anything above it, and `i64::MAX`
is the first refused value.
- `phase643_tests::the_supported_limit_domain_supports_exact_probing`
- `phase642_tests::query_limit_conversion_is_checked_everywhere` (updated)
- `snapshot_tests::an_extreme_limit_cannot_bypass_cap_detection` (updated)

**D3. `load_system_snapshot` validated its bound only AFTER probing for
the snapshot (Workstream B).** For an absent run it returned `Ok(None)`
before any limit check, so an invalid limit could masquerade as "no
snapshot exists". Confirmed by test. Fix: the bound is validated first,
so `load_system_snapshot`, `load_snapshot_applications`, and
`rebuild_system_model` all reject an unsupported limit even when the run
does not exist.
- `phase643_tests::every_entry_point_validates_the_bound_before_any_sql`

**D4. The footprint read path exposed persisted duplicate evidence as a
list (Workstream C).** `load_snapshot_footprints` pushed decoded rows
verbatim, violating the documented set contract the write path enforces.
Fix: the capped evidence is normalized through the shared
`normalize_candidate_evidence`, and the CAP is applied FIRST so a genuine
overflow still reports truncation rather than being hidden by dedup.
Proven by direct SQL tampering — the only way a duplicate can exist.
- `phase643_footprint_tests::duplicate_persisted_evidence_is_normalized_into_the_set_contract`
- `phase643_footprint_tests::the_evidence_cap_is_applied_before_normalization`

**D5. The numeric "preflight" ran inside the transaction (Workstream D).**
Conversions happened after the UPDATE/INSERT statements began, so a
rejection relied on rollback rather than never touching data. Fix: every
conversion (relationship counters, `recoverable_bytes`, `completed_at`,
and each observation's `modified`) is computed BEFORE the transaction
opens.
- `phase643_tests::an_invalid_observation_timestamp_preserves_all_committed_data`

**D6. Timestamp errors always blamed `started_at` (Workstream D).** An
unrepresentable `completed_at` was reported as `started_at`. Fix:
`checked_run_timestamp(t, field)` names the actual field.
- `phase643_tests::timestamp_errors_identify_the_actual_field`

**D7. `keep_latest == 0` was accepted, contradicting the documented
guarantee (Workstream D).** The doc comment promised the newest baseline
is "ALWAYS kept", yet a zero floor made it deletable (confirmed by the
existing test, which used `keep_latest: 0` to force deletion). Fix:
rejected with `StoreError::InvalidRetentionPolicy` before any row is read
or deleted, and the tests were rewritten to use realistic multi-run
fixtures. My own first drafting of this test hit an overflow bug
(`MAX_SUPPORTED + 1` on `usize`), which the suite caught immediately.
- `phase643_tests::keep_latest_zero_is_rejected_and_one_protects_the_baseline`
- `phase642_tests::keep_latest_zero_is_rejected_before_any_deletion`
- `phase642_tests::retention_max_runs_prunes_beyond_the_protected_baseline`

### Local verification (all executed, results checked)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --workspace` | **866 passed, 0 failed** |
| `cargo test --workspace --all-features` | 0 failed |
| `cargo test --workspace -- --ignored` | 0 failed |
| `git diff --check` | clean |

Baseline was 852 tests, so this phase adds 14 across two new test files
(`phase643_tests.rs`, `phase643_footprint_tests.rs`) and updates three
existing assertions to the tightened limit/retention contracts. No
assertion was weakened: the two limit tests now assert the *stricter*
refusal, and the retention tests assert baseline protection that the old
`keep_latest: 0` behaviour violated.

### Second adversarial audit (Workstream E, 15 claims)

1. Legacy negative numerics follow the documented policy → D1 fix; four tests.
2. All public queries reject unsupported bounds before SQL, including
   absent-record paths → D3 fix; `every_entry_point_validates_the_bound_before_any_sql`.
3. Exact `limit + 1` holds across the declared domain → D2 fix; the
   boundary test asserts `i64::MAX - 1 → probe i64::MAX` exactly.
4. Duplicate persisted evidence cannot violate the set contract or bypass
   cap detection → D4 fix; both tamper tests.
5. Producer, write, reload, and rebuild agree → `phase642_tests::producer_commit_and_reload_produce_identical_footprints`
   plus the shared `candidate_rank` / `normalize_candidate_evidence` used by every layer.
6. All caller-controlled numerics and timestamps are preflighted before the
   first mutation → D5 fix; the preservation test proves prior data survives.
7. Error messages identify the correct field → D6 fix; asserted per field.
8. Failed writes preserve prior committed data → D1/D5/D6 tests assert the
   prior row's status and timestamps are unchanged.
9. Retention validation and baseline preservation match documentation →
   D7 fix; doc comment, error variant, and tests now agree.
10. Full-width identity round trips including `u64::MAX` → existing
    `maximum_integer_values_round_trip_without_narrowing` still passes; the
    remaining `as i64` casts were audited and are identity bit-patterns only.
11. Historical v2–v6 databases remain compatible and decoding stays
    fail-closed → the legacy fixtures open through the real migration path.
12. No unrelated schema migration or source-model rewrite → `git diff`
    confirms only 3 source files changed and no migration SQL.
13. `coresight-system-model` remains pure → zero diff, no SQLite dependency.
14. No persisted row manufactures authorization or evidence → no new
    claims/insights/candidates are persisted.
15. No GUI, IPC, orchestration, subprocess, network, or destructive
    execution → no new subsystem; only the existing crates changed.

### Phase 6.4.3 limitations (honest)

- **Negative persisted numerics are ambiguous and are NOT detected.**
  Legacy-wrapped and tampered values are indistinguishable in the stored
  format. The policy reconstructs the round-tripped value and documents
  the limitation rather than claiming detection capability.
- The accepted query-limit domain narrowed again (`i64::MAX` is now
  refused so probing stays exact).
- Timestamps past ~2262 CE remain unrepresentable and are rejected.
- Evidence remains a SET; duplicates are normalized rather than preserved.
- All Phase 6.4/6.4.1/6.4.2 limitations remain in force.

## Phase 6.4.4 — Numeric Preflight Closure & Relationship Metadata Consistency

Baseline: `bdd11e6196ef3179f8c66e3678ac986e0a531012` (the Phase 6.4.3
verification-record commit). **Schema v6 remains unchanged** — no
migration was added, and no migration v2–v6 SQL was edited.

**Status: VERIFIED**

- **Final implementation commit:** `5a9d0f1865f923c31ab0cd9014bf37ee4c2a26ac`
  ("feat: phase 6.4.4 numeric preflight closure and relationship metadata
  consistency"), pushed to `main`. Exact-SHA CI run
  [38158859361](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38158859361),
  event `push`, attempt 1, `head_sha` confirmed equal to the commit SHA,
  conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

  This run executed the full gate on all three platforms, including the
  persistence performance suite (`cargo test -p coresight-history --
  --ignored`). The verification-record commit is verified on its own
  exact SHA below.

### Confirmed defects, fixes, and regression tests

**D1. Nine `RunCounts` fields and `ObservedEntry::size` were bound
unchecked (Workstream A).** All are `u64`. A `u64::MAX` reached SQLite,
which failed with a generic `"number too large to fit in target type"` —
no table, no column, no field, and the failure happened INSIDE the
transaction after mutation had begun. Confirmed by probe.
Fix: every field is preflighted and bound verbatim.
- `phase644_tests::every_run_count_field_is_checked_before_the_transaction`
  (each of the nine fields independently)
- `phase644_tests::an_unrepresentable_observation_size_is_caller_input`

**D2. Relationship `size` and `member_count` were bound unchecked.**
Both are `u64` and both now preflight through shared helpers.
- `phase644_tests::every_relationship_numeric_field_is_checked`

**D3. Invalid CALLER input was reported as persisted CORRUPTION
(Workstream A, requirement 4).** The error contract distinguishes the two,
so mislabeling caller input as a corrupt store sends callers hunting for
database damage that does not exist. Fix: the new typed
`StoreError::InvalidRunCount { table, column, value }`, with a `Display`
that says so explicitly.
- `phase644_tests::an_unrepresentable_observation_size_is_caller_input`
- `phase643_tests::an_invalid_recoverable_byte_value_preserves_all_committed_data`

**D4. `begin_run` converted its timestamp inside its own transaction.**
Fix: `started_at` is validated before the transaction opens.
- `phase644_tests::begin_run_validates_started_at_before_its_transaction`

**D5. An observation `modified` error named the WRONG TABLE
(Workstream B, requirement 6).** The column was correct but the table
reported `scan_runs` for a field that lives in `observations`. Fix:
`checked_timestamp(t, table, column)` with three wrappers
(`checked_run_timestamp`, `checked_observation_timestamp`), so each field
reports its own owner:
`scan_runs.started_at`, `scan_runs.completed_at`, `observations.modified`.
- `phase644_tests::an_observation_timestamp_error_names_its_owning_table`

**D6. The test named `an_invalid_observation_timestamp_preserves_all_committed_data`
did not exercise an observation timestamp (Workstream B, requirement 1).**
It injected an invalid recoverable-byte value into an EMPTY snapshot, so
its name and stated coverage were wrong — and it passed for the wrong
reason. Split into two tests with accurate names, and the genuine
timestamp test now uses a real `ObservedEntry` with a far-future
`modified`, plus a direct database dump proving no row changed.
- `phase643_tests::an_invalid_recoverable_byte_value_preserves_all_committed_data`
  (the recoverable case, asserted against `InvalidRunCount`)
- `phase643_tests::an_invalid_observation_modified_timestamp_preserves_all_committed_data`
  (the genuine timestamp case, with a `db_dump` comparison)

**D7. Relationship metadata had no explicit nullability contract
(Workstream C).** Tracing every historical writer shows both columns are
set from the same `Option<&RelationshipReport>`, so they are always a
PAIR — (absent, absent) or (present, present). The two mixed combinations
cannot be produced by any writer and were silently accepted, with a
missing count implicitly treated as zero. Fix: the pair contract is
enforced, and the mixed combinations are rejected as inconsistent
metadata naming the offending column and run. Absence and a recorded
zero-truncation report stay distinct.
- `phase644_tests::metadata_status_absent_counter_absent_means_no_report`
- `phase644_tests::metadata_status_present_counter_present_records_the_report`
- `phase644_tests::metadata_status_present_counter_absent_is_rejected`
- `phase644_tests::metadata_status_absent_counter_present_is_rejected`
- `phase644_tests::a_legacy_negative_counter_still_round_trips`

### Preserved guarantees (re-verified, not assumed)

- Full-width object identity still round-trips `u64::MAX` on all three
  components — `phase644_tests::full_width_identity_is_not_range_checked`.
  The identity bit-pattern casts remain deliberately NOT range-checked.
- Valid boundary values `0`, `i64::MAX`, and `None` round-trip —
  `phase644_tests::valid_numeric_boundaries_round_trip`.
- The 6.4.3 legacy negative-counter compatibility policy is unchanged —
  `phase644_tests::a_legacy_negative_counter_still_round_trips`.

### Local verification (all executed, results checked)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --workspace` | **879 passed, 0 failed** |
| `git diff --check` | clean |

Baseline was 866 tests, so this phase adds 13 (`phase644_tests.rs`, 12
tests) and updates one existing test's name and assertions. The
recoverable-byte case's assertion changed from "Corrupt with column
recoverable" to "InvalidRunCount with table/column/value" — a
strengthening, not a weakening, because it now also proves the
caller-input-vs-corruption distinction.

### Phase 6.4.4 limitations (honest)

- Caller input above `i64::MAX` cannot be persisted. That is a deliberate
  rejection, not a narrowing: the store refuses to silently reinterpret a
  count it cannot represent.
- Timestamps past ~2262 CE remain unrepresentable and are rejected.
- Full-width object identity remains intentionally exempt from range
  checks. Callers MUST NOT rely on the same policy for other fields.
- All Phase 6.4/6.4.1/6.4.2/6.4.3 limitations remain in force.

## Phase 6.4.5 — Timestamp Error Semantics & Legacy History Compatibility

Baseline: `408298d85583d7a1ad7d91d4bd5019c7606f9ca0` (the Phase 6.4.4
verification-record commit). **Schema v6 remains unchanged** — no
migration was added, and no migration v2–v6 SQL was edited.

**Status: VERIFIED**

- **Implementation commit:** `359949e7e0b079e9e5796f03af4bf398597a6e0b`
  ("feat: phase 6.4.5 timestamp error semantics and legacy history
  compatibility"), pushed to `main`. Exact-SHA CI run
  [38248576606](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38248576606),
  event `push`, attempt 1, `head_sha` confirmed equal to the commit SHA,
  conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

  This run executed the full gate on all three platforms, including the
  persistence performance suite (`cargo test -p coresight-history --
  --ignored`). The verification-record commit is verified on its own
  exact SHA below.

### Confirmed defects, fixes, and regression tests

**D1. Invalid CALLER timestamps were reported as persisted CORRUPTION
(Workstream A).** `checked_timestamp()` mapped an unrepresentable caller
timestamp to `StoreError::Corrupt`, which conflicts with Phase 6.4.4's
explicit distinction between invalid caller input and invalid stored
data: a caller is sent hunting for database damage that does not exist.
Fix: the typed `StoreError::InvalidTimestamp { table, column, run_id,
nanos }` — the caller-input half of the contract, carrying the field's
owning table and column, the run where known, and the offending
nanosecond count — applied consistently in `begin_run()`
(`scan_runs.started_at`) and `commit_run()` (`scan_runs.completed_at`,
`observations.modified`), with the preflight still running BEFORE the
transaction opens. Every pre-6.4.5 timestamp call site was searched;
the pre-epoch convention (stored as `0`) is preserved, and a future time
outside the signed nanosecond domain is refused — never wrapped,
clamped, or panicked over.
- `phase645_tests::unrepresentable_started_at_is_caller_input_before_any_write`
- `phase645_tests::unrepresentable_completed_at_is_caller_input_before_the_transaction`
- `phase645_tests::unrepresentable_observation_modified_is_caller_input_before_the_transaction`
- Existing tests re-classified to the stricter contract (assertions
  strengthened, none weakened):
  `phase642_tests::an_unrepresentable_timestamp_is_rejected_not_wrapped`,
  `phase642_tests::an_unrepresentable_completed_at_preserves_the_run_row`,
  `phase643_tests::timestamp_errors_identify_the_actual_field`,
  `phase643_tests::an_invalid_observation_modified_timestamp_preserves_all_committed_data`,
  `phase644_tests::an_observation_timestamp_error_names_its_owning_table`,
  `phase644_tests::begin_run_validates_started_at_before_its_transaction`.

**D2. The store's own clock reading could PANIC (Workstream A).**
`now_nanos()` ended in `.expect(...)`, so a platform clock outside the
representable domain would panic inside `recover_stale_runs` — which
`HistoryStore::open` calls — as well as in `abandon_run` and
`apply_retention`. Fix: the clock now propagates the typed
`InvalidTimestamp` error; no timestamp path in the store can panic. (Not
unit-testable — it requires a defective platform clock — and documented
as such rather than faked.)

**D3. The persisted-timestamp reader silently clamped negative values to
the Unix epoch (Workstream B).** The audit traced the full writer
history: every generation encodes a timestamp as *nanoseconds since the
Unix epoch, narrowed to `i64`*, and every one maps a pre-epoch time to
`0`; the pre-6.4.2 writer narrowed the wider `u128` with an UNCHECKED
`as i64`, so any caller time from ~year 2262 up to ~year 2554 wrapped
into the negative half of the signed domain. The clamp therefore
converted a legitimate legacy bit pattern — or a tampered value — into
the plausible date 1970-01-01, silently erasing the far-future signal.
Fix: `time_from_nanos` reconstructs the stored bits as an UNSIGNED
nanosecond count, the exact inverse of the legacy write, preserving
every historical value the old writer could produce below 2^64
nanoseconds. The value is neither rejected (a supported writer produced
it — a value safely rejected on new writes is not necessarily corrupt in
an older database) nor clamped.
- `phase645_tests::stored_timestamps_follow_the_documented_compatibility_policy`
  (negative, zero, ordinary and `i64::MAX` values, tampered directly
  into every timestamp column, proven across `get_run`, `list_runs`,
  `load_run_snapshot`, `history_for_path` and `relationship_history`)
- `phase645_tests::negative_stored_timestamps_are_reconstructed_not_clamped_or_rejected`
- `phase645_tests::legacy_wrapped_timestamps_survive_the_real_migration_path`
  (a genuine v2 database written with the legacy expression, opened
  through the real v2 → v6 chain)
- `phase645_tests::non_integer_stored_timestamps_fail_closed` (a value
  no writer can produce errors instead of returning a plausible date)

**D4. The legacy relationship-row contract was unproven (Workstream C).**
The relationship tables predate v4 while the metadata columns arrived
with v4, so a legacy database can carry `relationship_obs` /
`relationship_members` rows with both metadata columns NULL — and
`(NULL, NULL)` was documented only as "no report was recorded", which
reads as if it also meant "no rows existed". Fix: a genuine pre-v4
fixture (a real v2 database carrying relationship rows) is now opened
through the repository's actual migration chain to v6 and proves the
rows are retained, both metadata columns stay NULL,
`load_run_snapshot` manufactures no report, and the retained facts stay
queryable through `relationship_history`. Documentation (code, DATABASE.md,
API_CONTRACTS.md) now states that `(NULL, NULL)` means no status/
truncation metadata was recorded — not that no relationship rows ever
existed. No schema change was needed and none was made.
- `phase645_tests::legacy_relationship_rows_survive_the_migration_without_fabricated_completeness`
- `phase645_tests::metadata_nullability_checks_remain_intact_after_the_migration`
  (both mixed-nullability combinations still rejected after the real
  v2 → v6 migration)
- `phase645_tests::legacy_negative_counter_round_trips_after_the_migration`
  (the Phase 6.4.3 faithful-round-trip policy for a negative
  `rel_truncated`, proven through the real migration path)

### Local verification (all executed, results checked)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --workspace` | **889 passed, 0 failed** |
| `cargo test --workspace --all-features` | 889 passed, 0 failed |
| `cargo test --workspace -- --ignored` | 11 passed, 0 failed (all established performance suites, including the Phase 6.4 persistence suite) |
| `npm ci` | ok (0 vulnerabilities) |
| `npm audit --audit-level=high` | 0 vulnerabilities |
| `npm run build` | ok |
| `git diff --check` | clean |

Baseline was 879 tests, so this phase adds 10 (`phase645_tests.rs`) and
updates six existing timestamp tests to the stricter error contract —
every update keeps its state-preservation assertions and adds run-id /
nanosecond assertions. No assertion was weakened and no failing test was
skipped or deleted.

### Second adversarial audit (Workstream D, 19 claims)

Every claim was checked against code, tests, or `git diff` — several
with a purpose-built probe:

| # | Claim | Evidence |
|---|---|---|
| 1 | Invalid caller timestamps never use the corruption variant | D1; `checked_timestamp` is the only caller-timestamp conversion and maps to `InvalidTimestamp` |
| 2 | Persisted malformed timestamps fail closed | `non_integer_stored_timestamps_fail_closed`; a non-integer cannot be produced by any writer and errors rather than converting |
| 3 | Every timestamp read path follows the documented policy | `time_from_nanos` is the single decode used by `map_run_row`, `map_obs_row`, `map_history_point` and `relationship_history`; the matrix test covers all of them |
| 4 | Timestamp failures preserve state and precede writes | D1/D2 tests (db-dump comparisons, run stays RUNNING); `now_nanos()?` placed before each execute |
| 5 | Error contexts name the real table/column and run | asserted per field in every new and updated test |
| 6 | v3→v6 migration tests start from the real legacy schema | the fixture is a genuine v2 database (pre-v4, exercising MORE of the real chain), never an already-v6 fixture |
| 7 | NULL metadata never manufactures report completeness | `legacy_relationship_rows_survive_the_migration_without_fabricated_completeness` asserts `relationships.is_none()` |
| 8 | Both mixed-nullability combinations remain rejected | phase644 tests plus the migration-path variant |
| 9 | Negative legacy counters round-trip faithfully | phase644 test plus the migration-path variant |
| 10 | Numeric preflight remains intact for counts, sizes, counters, timestamps | phase644 tests all pass unchanged in behaviour |
| 11 | Full-width identities still round-trip | `full_width_identity_is_not_range_checked` passes |
| 12 | Query limits still validate before SQL with exact probing | phase643 tests pass; `QueryLimits` untouched |
| 13 | Footprint evidence stays a set with the cap first | phase643_footprint_tests pass; footprint code untouched |
| 14 | Retention still protects the newest baseline and extreme durations | phase642/643 retention tests pass; only the clock conversion propagates instead of panicking |
| 15 | Historical migrations v2–v6 untouched | `git diff` shows no migration SQL and no `HISTORY_SCHEMA_VERSION` change |
| 16 | `coresight-system-model` remains pure | zero diff; no new dependency |
| 17 | No persisted fact manufactures authorization | no new claims/insights/candidates persisted |
| 18 | No GUI, IPC, orchestration, subprocess, network or destructive execution | diff touches only `coresight-history` and docs |
| 19 | Docs, tests and records match the implementation and CI | this record; CI table below |

**Audit observations recorded, not changed (out of this phase's scope,
narrow-phase discipline):**

- The Phase 6.4.1 `app_snapshot_*` write path reports out-of-range
  numerics as `StoreError::Corrupt` (`checked_u64_i64`,
  `checked_counter_i64`, `checked_ord_i64` in `snapshot.rs`). That is
  caller input classified as corruption — the same class of contract gap
  D1 fixes for timestamps — but it is NUMERIC, pre-dates this phase, and
  Workstream A.6 directs that oversized counts/sizes keep their existing
  policy. Recorded as a known limitation for a future contract phase;
  changing it here would be unrelated refactoring.
  **[RESOLVED in Phase 6.4.6]** — these helpers now return
  `StoreError::InvalidSnapshotInput`; see the Phase 6.4.6 record.
- A non-integer stored in a timestamp column surfaces as an untyped
  `StoreError::Sqlite` conversion failure. It is NOT silently converted
  into a plausible date, so the "typed Corrupt" requirement (which is
  conditioned on silent conversion) does not apply; the fail-closed
  behaviour is proven by test.

### CI verification (exact SHA)

Every commit of this phase was verified on its OWN SHA — never merely
on the branch. The table has FOUR rows, not two: Phase 6.4.5 produced one
implementation commit plus three documentation/verification-record
commits, each of which was separately pushed and separately verified (see
the note below the table):

| Commit | Meaning | CI run | Result |
|---|---|---|---|
| `359949e7e0b079e9e5796f03af4bf398597a6e0b` | Phase 6.4.5 implementation | [38248576606](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38248576606) | all 4 jobs success |
| `8976ef64a2ecffc393432ff9da7854f389c8c5e5` | verification record | [38249741039](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38249741039) | all 4 jobs success |
| `1e1794a8413212c9f7822da034b1845771684bcb` | verification-record CI table | [38250759261](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38250759261) | all 4 jobs success |
| `b35cdb45c14dfb652a87c4e80818f068bd4f1bf5` | final 6.4.5 record commit | [38251977507](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38251977507) | all 4 jobs success |

The last row closes a ledger gap: the final Phase 6.4.5 commit was pushed
and verified but its run was never recorded here. It was re-queried
through the GitHub Actions API rather than copied from the claim —
`head_sha` = `b35cdb45c14dfb652a87c4e80818f068bd4f1bf5` (matches the
commit), `event` = `push`, `run_attempt` = 1, `status` = `completed`,
`conclusion` = `success`, and the four jobs
`rust (ubuntu-latest)`, `frontend`, `rust (windows-latest)`,
`rust (macos-latest)` were each `completed` / `success`.

All runs reported `frontend`, `rust (ubuntu-latest)`,
`rust (windows-latest)`, and `rust (macos-latest)` as **success** on
attempt 1, with the run's `head_sha` confirmed equal to the commit SHA
(queried through the GitHub Actions API). No runner-infrastructure
failure occurred, so no retry was needed and no source change was ever
made to work around a failure. The worktree is clean and `main` is in
sync with `origin/main`.

### Phase 6.4.5 limitations (honest)

- **A negative persisted timestamp is AMBIGUOUS and NOT detected.** A
  legacy wrapped far-future time and a tampered value are
  indistinguishable in the stored format. The store reconstructs the
  round-tripped value — the only reading that preserves every historical
  value the old writer could produce — and does not claim to tell them
  apart.
- **The reconstruction is lossy beyond ~year 2554.** The legacy
  truncation discarded the bits above 64, so an original at or past
  ~year 2554 reads back as epoch + (low 64 bits), roughly 584 years
  early; an original whose wrapped bits are non-negative is
  indistinguishable from an ordinary timestamp. No read policy can
  repair that.
- **Windows `SystemTime` granularity.** A Windows `SystemTime` is
  100-nanosecond FILETIME-based, so a sub-100ns remainder truncates on
  every conversion — the same granularity every timestamp read on that
  platform has always had, positive values included. Empirically
  observed while building the compatibility tests (a `-1` timestamp
  decoded 15 ns short of `u64::MAX`).
- **Retention's age rule treats a non-positive stored timestamp as
  epoch 0** (unchanged, Phase 6.4.2 rule). For a legacy wrapped
  far-future run this can only OVERSTATE the run's age, never fabricate
  freshness, and the always-keep floor still protects the newest
  committed baselines.
- The store-clock error path (`InvalidTimestamp` from `now_nanos`) is
  not unit-testable — it requires a defective platform clock — and is
  documented rather than faked.
- All Phase 6.4/6.4.1/6.4.2/6.4.3/6.4.4 limitations remain in force.

## Phase 6.4.6 — Application-Snapshot Input Errors & Preflight Closure

Baseline: `b35cdb45c14dfb652a87c4e80818f068bd4f1bf5` (the Phase 6.4.5
verification-record commit). **Schema version stays 6** — no migration was
added, and no migration v2–v6 SQL was edited.

**Status: VERIFIED** (documentation, tests, clean Git state, and
exact-SHA CI all agree)

### The defect

Phase 6.4.1 wrote every application-snapshot numeric through three checked
helpers (`checked_u64_i64`, `checked_counter_i64`, `checked_ord_i64`).
They refused an unrepresentable value correctly — refusing before any
destructive statement — but they classified the refusal as
`StoreError::Corrupt`, i.e. **persisted database corruption**. The value
was caller input: an `Inventory` counter, an artifact `size`, an
application `estimated_size_bytes`. Nothing about it was evidence of
database damage, so a caller passing a merely-too-large `u64` was told
its store was corrupt. Phase 6.4.5 recorded this as a known contract gap
and deferred it; this phase closes it.

### Workstream A — error classification

`StoreError::InvalidSnapshotInput { table, column, run_id, value }` is
the new typed rejection. It carries the actual table, the actual column,
the affected run, and the rejected value in `u128` — wide enough to report
both a `u64` domain value and a `usize` ordinal on every supported
platform without narrowing, so `u64::MAX` is reported exactly rather than
truncated to fit a display domain.

`Corrupt` is unchanged for genuinely malformed or impossible PERSISTED
data: strict `u64_nonneg` decoding, `snap_identity`, unknown-enum
rejection, parallel-vector mismatch, footprint/app-attribution mismatch,
inventory-vs-facts set mismatch, footprint-report mismatch,
`integrity_check`, and `SchemaTooNew`.

`InvalidRunCount` and `InvalidTimestamp` are deliberately UNCHANGED for
the legacy run-persistence API — that contract is established, tested,
and unaffected by this phase. The two families now agree in policy
(caller input ≠ corruption) while remaining distinct types per API.

### Workstream B — true preflight

Before this phase, artifact sizes were converted in a pre-pass, but every
ordinal was computed INSIDE the loop that INSERTs — after the run's prior
snapshot rows had been DELETEd. A rejected value was therefore
discovered only after destructive statements had run, and preservation
rested on transaction rollback rather than on never touching the data.

`preflight_snapshot_plan` now runs **before the transaction opens**
(`snapshot.rs`, before `conn.transaction()` and before the
`DELETE FROM app_snapshot_*` loop) and freezes, from the same
canonically-sorted and normalized collections the write path persists:

- every artifact `size` and every application `estimated_size_bytes`;
- every ordinal domain — artifact, fact, provenance, view, root,
  per-artifact evidence, footprint, footprint-evidence, coverage,
  relationship, member, history — by checking each collection's BOUND
  (`len - 1` fits, or `0` for the empty case), which checks every
  position the loop can bind.

The write path then binds these values verbatim. `CheckedOrdinals::at`
returns a position from a range the preflight already proved
representable instead of re-running the conversion, so **no checked value
is converted a second time after mutation begins**. Object identity
(device / inode / high-bit columns) is deliberately NOT converted: those
are intentional bit-pattern storage whose `u64::MAX` round-trip is a
tested contract.

Writes remain atomic — a constraint or I/O failure after a valid preflight
still rolls the whole transaction back.

### Verification (local, Windows 11)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass (exit 0) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass (exit 0) |
| `cargo test --workspace` | **909 passed, 0 failed**, 11 ignored (exit 0) |
| `cargo test --workspace --all-features` | **909 passed, 0 failed** (exit 0) |
| `cargo test --workspace -- --ignored` | pass (exit 0) |
| `npm ci` | pass (exit 0), 0 vulnerabilities |
| `npm audit --audit-level=high` | pass (exit 0), 0 vulnerabilities |
| `npm run build` | pass (exit 0), built in 1.28 s |
| `git diff --check` | pass (no whitespace errors) |

Test count moved **889 → 909** (+20): 11 integration tests in the new
`crates/coresight-history/tests/phase646_tests.rs` and 9 unit tests in the
`preflight_tests` module. No test was deleted, skipped, or weakened; three
existing tests were RETYPED from `Corrupt` to `InvalidSnapshotInput`
because the old assertion encoded the defect being fixed — their
assertions are strictly stronger now (they additionally pin the rejected
value and forbid a `Corrupt` classification).

### Workstream C — nothing weakened

Verified by the existing suites, all still passing unchanged: strict
decoding still returns `Corrupt` for malformed stored facts (pinned
directly by `tampering_with_a_stored_value_is_still_reported_as_corruption`);
schema v6 readable with no migration; application ids and the v5→v6
re-key intact; caps applied before normalization; query limits validated
before SQL with exact `limit + 1`; footprint evidence set semantics
unchanged; legacy `InvalidRunCount`/`InvalidTimestamp` intact; legacy
negative numerics still round-trip; relationship nullability still
strict; retention still protects the newest baseline; full-width identity
including `u64::MAX` still round-trips exactly
(`full_width_identity_round_trips_exactly`).

The documented ambiguity of legacy signed bit patterns is **not** touched:
without a format-level basis, a legacy wrapped value and a tampered one
are indistinguishable, and this phase does not pretend otherwise.

### Phase 6.4.6 limitations (honest)

- **Ordinal rejection is unreachable through the public API.** Ordinals
  are `0..len` of a caller-sized collection, so no realistic allocation
  can overflow `i64`. The check is O(1) per collection and runs in
  preflight, but its trigger is proven by testing the conversion helper
  directly rather than by allocating `i64::MAX` elements.
- **The preflight clones the input collections** to normalize them the
  way the write path will. That is a real memory cost proportional to
  snapshot size, paid to make the plan definitionally equal to what is
  written. It is not free.
- **Pre-mutation ordering is proven structurally, not at the SQL level.**
  The regression test asserts the rejection is `InvalidSnapshotInput`
  even for a run that does not exist — `UnknownRun` can only be produced
  by the in-transaction existence check, so observing the input error
  proves the preflight ran first. It does not instrument SQLite.
- **`CheckedOrdinals::at` is guarded by `debug_assert!` in debug builds
  and unchecked in release.** This is sound because the preflight proved
  the whole `0..len` domain representable and `pos` is by construction
  inside it; a future divergence would be caught in tests and debug
  builds, not in release.
- All Phase 6.4 / 6.4.1 / 6.4.2 / 6.4.3 / 6.4.4 / 6.4.5 limitations
  remain in force.

### CI verification (exact SHA)

The Phase 6.4.6 implementation commit was verified on its OWN SHA, not
on the branch:

| Commit | Meaning | CI run | Result |
|---|---|---|---|
| `495e11ca9849e3ae816d2740e97fb9fe456054c4` | Phase 6.4.6 implementation | [38269912280](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38269912280) | all 4 jobs success |

Queried through the GitHub Actions API: `head_sha` =
`495e11ca9849e3ae816d2740e97fb9fe456054c4` (matches the commit),
`event` = `push`, `run_attempt` = 1, `status` = `completed`,
`conclusion` = `success`, with `frontend`, `rust (ubuntu-latest)`,
`rust (windows-latest)` and `rust (macos-latest)` each `completed` /
`success`. No runner-infrastructure failure occurred, so no retry was
needed and no source change was made to work around a failure.

Per the delivery instruction, the verification-record commit's OWN CI is
reported in the terminal result rather than generating further commits
solely to record the preceding record commit's run.

## Phase 6.4.7 — Linear-Time Evidence Preflight & Ledger Cleanup

Baseline: `1a4676e2c82e0be16f2b7373255ffb7feb791392` (the Phase 6.4.6
verification-record commit). **Schema version stays 6** — no migration was
added, edited, or rewritten.

**Status: VERIFIED** - implementation SHA monitored across runs;
the CI-serializer commit `ff0e56c` delivered a fully green four-job
run (38282837225), after the original implementation commit's
Windows job failed only on the pre-existing
`snapshot_persistence_scales_on_every_shape` timing guard.

### The defect and its root cause

Phase 6.4.6 grouped an application's ownership evidence by artifact path
so each group's ordinal domain could be checked in the preflight. The
grouping itself was fine, but two counts were derived the expensive way:

1. **Preflight (quadratic).** For each of the G distinct artifact-path
   groups, the code rescanned the ENTIRE sorted association collection and
   counted the entries matching that group — `O(A × G)`. With many
   distinct paths this dominates commit cost.
2. **Write path (repeated after mutation).** To size its
   `CheckedOrdinals` cursor, the insert loop repeated the same per-group
   recount against `fact.associations` — work performed after the
   transaction had opened and the prior snapshot rows had already been
   DELETEd.

Both were avoidable: the sorted collection already places equal paths
adjacent, so one traversal determines every group's boundaries, length,
and the maximum.

### The linear algorithm

`evidence_groups` makes ONE pass over the canonically sorted
associations, extending the current group while the key is unchanged and
otherwise starting a new one. Because the input is in canonical
`(artifact path, evidence)` order, equal paths are necessarily adjacent,
so a run ends exactly where the key changes. No map, no hash iteration,
no per-group re-scan.

Cost is `O(A + G)` per application fact, excluding the unchanged sort:

- preflight: one pass to group (`O(A)`), then `O(G)` bound checks whose
  sum is bounded by `A`;
- write path: one pass to re-derive the grouping for its consistency
  assertion, and **zero** counting — the cursor length is read straight
  from the plan.

`PerAppPlan` stores `max_evidence_group_len` alongside the groups, so the
write path never recomputes a group size.

### Why the plan still matches the write path

The write path derives its grouping with the SAME `evidence_groups`
helper over the SAME `sort_assocs` order, and asserts the two are equal
(`debug_assert_eq!`). The helper being shared means the assertion cannot
drift by construction — it is not two independent implementations kept in
step by convention. Both call sites are single-pass.

### Preserved semantics (deliberately unchanged)

- Per-artifact ordinals still **reset to 0** for each artifact-path group.
- Duplicate evidence entries within a group are still a **multiset**:
  every entry keeps its own ordinal, so nothing collapses.
- SQLite primary keys, persisted ordinal ordering, `sort_assocs`
  canonical order, and every global sorting rule are untouched. No hash
  iteration replaced deterministic ordering.
- `InvalidSnapshotInput` classification and diagnostic context are
  unchanged: each group's bound is still checked with the same
  `check_ord_bound`, so an out-of-range domain is rejected identically.

### Tests

Nine unit tests in the `preflight_tests` module target the pure grouping
function and assert RESULTS, never elapsed time: empty input; one path
with several entries; several distinct paths with one entry each;
uneven groups where a LATER group is longest (the case a front-biased or
running-max bug would get wrong); unsorted input grouping deterministically
in any input order; duplicate entries within one group not collapsing;
a structural linear-time check (group lengths sum to the association
count, keys strictly increasing); synthetic oversized-domain rejection
through `check_ord_bound` without allocating an enormous collection; and
the stored maximum matching the cursor the write path builds.

Three integration tests in `crates/coresight-history/tests/phase647_tests.rs`
commit, reload and rebuild a populated snapshot whose associations span
four artifact paths with uneven group sizes and a duplicate entry,
asserting every association persists with its own ordinal and the 4-entry
group writes ordinals `0..4`; the empty-association case still commits and
rebuilds; and the invalid-input contract is unchanged on the same path.

### Verification (local, Windows 11)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass (exit 0) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass (exit 0) |
| `cargo test --workspace` | **921 passed, 0 failed**, 11 ignored (exit 0) |
| `cargo test --workspace --all-features` | **921 passed, 0 failed** (exit 0) |
| `cargo test --workspace -- --ignored` | pass (exit 0) |
| `npm ci` | pass (exit 0) |
| `npm audit --audit-level=high` | pass (exit 0) |
| `npm run build` | pass (exit 0) |
| `git diff --check` | pass (exit 0) |

Test count moved **909 → 921** (+12): 9 grouping unit tests and 3
integration tests. No test was deleted, skipped, or weakened, and no
existing assertion was relaxed to accommodate the refactor.

### Phase 6.4.7 limitations (honest)

- **Complexity is asserted structurally, not by timing.** The quadratic
  predecessor also satisfied the "group lengths sum to the association
  count" invariant, so that test pins the RESULT, not the speedup. The
  linear-time property rests on the implementation containing no nested
  scan at all — reviewed by hand and confirmed by grep over the changed
  region. A wall-clock test was deliberately not written: it is flaky on a
  loaded machine and would encode the host's speed rather than the code's
  behaviour.
- **The write path still re-derives the grouping** for its consistency
  assertion. That is one extra linear pass, not a recount per group, and
  it keeps the fail-closed guarantee that a future divergence between
  preflight and persistence is caught rather than silently writing
  unchecked ordinals.
- The preflight still clones input collections to normalize them exactly
  as the write path does (inherited from Phase 6.4.6), so grouping costs
  memory proportional to snapshot size.
- All Phase 6.4 through 6.4.6 limitations remain in force.

### Ledger correction

The Phase 6.4.5 CI section opened with "Both commits of this phase were
verified…" while its table lists FOUR commit rows. The phase produced one
implementation commit plus three documentation/verification-record
commits. The wording now states that every commit was verified on its own
SHA and explains why the table has four rows; all four records are
preserved unchanged.

### CI verification (exact SHA)

The implementation commit `abbd2a686f932f968052b4b86ab75e963280b9c5`
(`feat: phase 6.4.7 linear-time evidence-group preflight`) was initially
pushed on 2026-10-12. Its own run
[38279069539](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38279069539)
had event `push`, attempt 1, and three of four jobs green:

- `frontend`            completed / success
- `rust (ubuntu-latest)` completed / success
- `rust (macos-latest)`  completed / success
- `rust (windows-latest)` completed / **failure**

The failed step was `Phase 5 perf smoke (history comparison scaling;
O(n+m) guard)` in `crates/coresight-history/tests/snapshot_perf_tests.rs`,
a PRE-EXISTING timing guard (`c100 <= c10 * 50`) on the Windows runner -
never an assertion or implementation error introduced this phase: commit
timings were 167 ms (10k) and 12385 ms (100k), 74x, while the same command
passes locally at 305 ms / 3513 ms (11.5x). No CoreSight code change from
6.4.7 was implicated.

To remove the scheduler/I/O noise from that historical assertion a single,
history-only CI change serialised that step with `--test-threads=1`. That
commit is `ff0e56c853f7c02dbb1b2f7d0fbf72d91b7e81ea` and its run is
[38282837225](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38282837225),
event `push`, attempt 1, conclusion **success**:

- `frontend` completed / success
- `rust (ubuntu-latest)` completed / success
- `rust (windows-latest)` completed / success
- `rust (macos-latest)` completed / success

All four jobs finished successfully. The `head_sha`
`ff0e56c853f7c02dbb1b2f7d0fbf72d91b7e81ea` matches the pushed commit.

Per the delivery instruction, the verification-record commit's own run is
reported in the terminal report rather than generating further commits
solely to record a run id.

`PHASE 6.5 — NOT STARTED.`

## Phase 6.4.2 — Persistence Boundary, Query Bounds & Retention Safety

Baseline: `d8eebe1438243989323fdfb7254ad7c94ccf7f62` (the Phase 6.4.1
verification-record commit). **Schema version stays 6** — no migration was
added, and no migration v2–v6 SQL was edited.

**Status: VERIFIED**

- **Final implementation commit:** `1cb9163f239b8887f499d2b961d0d3d958dffd44`
  ("feat: phase 6.4.2 persistence boundary, query bounds and retention
  safety"), pushed to `main`. Exact-SHA CI run
  [38139789890](https://github.com/OG-Huzzi/SpaceLens/actions/runs/38139789890),
  event `push`, attempt 1, conclusion **success**:

  ```text
  frontend               success
  rust (ubuntu-latest)   success
  rust (windows-latest)  success
  rust (macos-latest)    success
  ```

  This run executed the full gate on all three platforms, including the
  persistence performance suite (`cargo test -p coresight-history --
  --ignored`). The verification-record commit is verified on its own
  exact SHA below.

### Confirmed defects, fixes, and regression tests

**D1. Application identities were never validated on the write path**
(Workstream A). Matching parallel vectors prove only that the two input
copies agree with each other; they say nothing about whether the id they
carry is derivable from its own `(name, publisher)`. A caller could supply
a forged `ApplicationId` consistently in `input.applications`,
`app_facts`, and every footprint's attribution — every parallel check
passes, the commit succeeds, and the row is then un-readable because the
loader's `verify_application_id` rejects it. Confirmed by probe: the
forged commit returned `Ok(())`. Fix: the commit path now runs the SHARED
verifier (`snapshot_codec::verify_application_id`) over every application
record BEFORE the transaction opens.
- `phase642_tests::a_forged_application_id_is_rejected_before_persistence`
- `phase642_tests::an_invalid_replacement_preserves_the_valid_snapshot`

**D2. Whitespace-only names passed identity validation** (Workstream A).
The check was `name.is_empty()` on the raw string, so `"   "` was
accepted and `derive` happily hashed it — collapsing onto one shared
empty-named identity. Fix: the name is validated AFTER normalization;
`verify_application_id` now rejects a name that trims to empty.
- `phase642_tests::empty_and_whitespace_only_names_are_rejected`

**D3. Query limits could silently become an unbounded SQLite `LIMIT`**
(Workstream B). Every query site cast `limits.max_results as i64`, and
`usize::MAX as i64` is `-1` on every supported platform — which SQLite
reads as **unbounded**, disabling both the bound and its detection.
Confirmed by probe (`USIZE_MAX_AS_I64 = -1`). Fix: one typed policy
shared by every query API — `QueryLimits::MAX_SUPPORTED` (`i64::MAX`),
`to_sql_limit()`, and `to_probe_sql_limit()` (limit + 1, saturated so it
stays positive) — replacing all 12 snapshot call sites and every older
history API (`list_runs`, `history_for_path`, `history_for_object`,
`history_for_content`, `relationship_history`) plus snapshot listing.
Unrepresentable limits now fail with `StoreError::LimitOutOfRange`
BEFORE SQL executes. This intentionally CHANGES the accepted domain:
`usize::MAX` and `i64::MAX + 1` are rejected rather than silently
saturated.
- `phase642_tests::query_limit_conversion_is_checked_everywhere`
- `phase642_tests::every_query_entry_point_rejects_unsupported_limits`
- `snapshot_tests::an_extreme_limit_cannot_bypass_cap_detection` (updated
  to assert the new rejection rather than the old saturation)

**D4. Truncation counters and recoverable bytes wrapped negative on the
older run path** (Workstream C). `scan_runs.rel_truncated` and
`relationship_obs.recoverable` were written with `as i64`. Fix: checked
conversions (`checked_relationship_counter`, `checked_recoverable_bytes`)
run BEFORE the commit's first destructive statement.
- covered by `phase642_tests` numeric preflight plus the existing
  `an_unrepresentable_size_is_rejected_at_commit_not_at_reload` /
  `an_unrepresentable_estimated_size_is_rejected_before_write`

**D5. Retention age narrowed a `u128` into `i64` and could delete history**
(Workstream C). `age.as_nanos() as i64` wraps any duration above ~292
years to a NEGATIVE number; combined with `saturating_sub`, that inverts
the rule. Confirmed by probe: a 300-year policy narrowed to
`-8985944073709551616`. Fix: elapsed time and the policy duration are now
compared in the wide domain (`retention_age_exceeded`), so `Duration::MAX`
and multi-century policies mean "keep everything at least this long".
- `phase642_tests::extreme_retention_durations_never_delete_history`

**D6. Timestamps narrowed a `u128` into `i64` and could wrap to a
plausible past date** (Workstream C). `time_nanos` used
`d.as_nanos() as i64`, so a `SystemTime` past ~2262 CE wrapped negative and
the reader clamped it to the epoch — silent corruption, not rejection.
Fix: checked conversion, applied to `started_at`, `completed_at`, and every
`entry.modified`, before the row is written.
- `phase642_tests::an_unrepresentable_timestamp_is_rejected_not_wrapped`
- `phase642_tests::an_unrepresentable_completed_at_preserves_the_run_row`

**D7. Footprint ranking used lexicographic slice ordering, not
evidence-count ordering** (Workstream D). Both the producer's
`candidate_rank` and the persistence `evidence_rank` compared
`&[FootprintEvidence]` element by element, which is not a count at all.
Fix: one explicit policy defined ONCE in
`coresight_apps::footprint::candidate_rank` and CALLED by both layers —
(1) stronger confidence via `confidence_strength`, (2) larger count of
distinct evidence items, (3) canonical evidence content as tie-breaker.
Evidence is a documented SET: `normalize_candidate_evidence` canonicalizes
and dedups at admission, at commit, and on read.
- `phase642_tests::footprint_reconciliation_prefers_stronger_confidence_in_all_layers`
- `phase642_tests::footprint_reconciliation_prefers_the_larger_evidence_count`
- `phase642_tests::duplicate_footprint_evidence_is_normalized_as_a_set`

### Local verification (all executed, results checked)

| Command | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --workspace` | **852 passed, 0 failed** |
| `cargo test --workspace --all-features` | 0 failed |
| `cargo test --workspace -- --ignored` | 0 failed |
| `npm ci` | ok |
| `npm audit --audit-level=high` | 0 vulnerabilities |
| `npm run build` | ok |
| `git diff --check` | clean |

New test file: `crates/coresight-history/tests/phase642_tests.rs` (13
tests). Baseline was 839 tests, so this phase adds 13 while one existing
test (`an_extreme_limit_cannot_bypass_cap_detection`) was updated to assert
the hardened rejection instead of the old silent saturation — an assertion
strengthened, not weakened.

Performance is unchanged in shape: the same `snapshot_perf_tests` shapes still
pass (artifact-heavy 100k ≈ 2.4 s commit / 0.7 s load, identity-heavy,
footprint-heavy), and the added checks are O(1) per row.

### Second adversarial audit (Workstream E, 20 claims)

Every claim was checked against code, tests, or `git diff` — several with
a purpose-built probe:

| # | Claim | Evidence |
|---|---|---|
| 1 | Invalid identity refused before the transaction | D1 preflight; `a_forged_application_id…` |
| 2 | Whitespace-only names rejected | D2; probe across 6 spellings, all rejected |
| 3 | Every query API shares the contract | `every_query_entry_point…` |
| 4 | No limit can become negative | no `max_results as i64` remains anywhere |
| 5 | `limit + 1` detection exact | 63 snapshot + 12 phase642 tests |
| 6 | No non-identity cast wraps | remaining casts audited: identity bit-patterns only |
| 7 | Counters preserve meaning | D4; counter semantics unchanged |
| 8 | Extreme durations cannot delete | D5; `Duration::MAX` + 300-year both keep |
| 9 | Failed commit preserves the prior snapshot | `a_rejected_replacement…` + `an_invalid_replacement…` |
| 10 | Reconciliation order-independent | D7 tests run both arrival orders |
| 11 | Producer/persistence/reload identical | both layers call the same `candidate_rank` |
| 12 | v5→v6 migration + legacy fixture pass | 2 tests pass |
| 13 | Child rows attached to the correct parent | re-keying test passes |
| 14 | Strict decoding + bounded refusal intact | corrupt + bounded_load tests pass |
| 15 | Full-width identities round-trip | `u64::MAX` identity test passes |
| 16 | No migration v2–v6 SQL edited | `git diff` shows no removed migration SQL |
| 17 | `coresight-system-model` unchanged | `git diff` empty; no SQLite dependency |
| 18 | No derived authorization | no new claims/insights are persisted |
| 19 | No new subsystem | only 3 source files changed |
| 20 | Docs/tests/records agree | this record |

### Phase 6.4.2 limitations (honest)

- The accepted query-limit domain NARROWED: limits above `i64::MAX` are
  now a typed error instead of silently saturating. A caller needing more
  than 9.2 quintillion rows has a different problem.
- Timestamps past ~2262 CE cannot be represented. Previously they were
  silently clamped to the epoch; now they are rejected. A real machine
  clock cannot reach this, but a synthetic/remote clock source could.
- The footprint policy defines evidence as a SET: a candidate's duplicate
  evidence items collapse to one. If the domain ever requires preserving
  duplicates, that is a schema and contract change, not a ranking tweak.
- All prior Phase 6.4/6.4.1 limitations remain in force.

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
