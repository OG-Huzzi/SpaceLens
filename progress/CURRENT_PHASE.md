# CoreSight — Current State

- **Current phase:** PHASE 6.1 — independent verification & architectural
  hardening (audit pass over commit `54f0e7f`).
- **Status:** Phase 6.1 code AUDITED independently; the audit findings were
  REPAIRED in this phase. VERIFIED status requires the CI run of the
  verification commit to be recorded green here (see CI section).
- **Product direction (binding):** CoreSight is a **macOS system
  intelligence + power-tools application** — NOT a "Mac cleaner". macOS is
  the primary implementation and launch platform; Windows/Linux remain
  architectural targets with their abstractions intact. Storage is one
  pillar of seven (docs/MACOS_ARCHITECTURE.md).
- **Last updated:** 2026-10-07.

## What happened in this phase (2026-10-07)

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

## CI

**CI VERIFIED (2026-10-07): run 37589662213 for commit `94c3164` —
rust ubuntu SUCCESS, rust windows SUCCESS, rust macos SUCCESS,
frontend SUCCESS.**
https://github.com/OG-Huzzi/SpaceLens/actions/runs/37589662213

(The first verification push `d88b98b` failed CI clippy on
ubuntu/macos — unix-only test fixtures missing a trait import, a class
of failure invisible on the Windows dev host; repaired in `94c3164`,
all gates re-run locally, cross-target clippy clean.)

## Next authorized work

- NOTHING starts until this verification phase passes independent review
  and its CI run is recorded green above. Phase 6.2 and application-
  intelligence persistence remain NOT STARTED.
