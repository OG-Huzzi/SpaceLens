# SpaceLens — Phase 2 Status

- **Phase:** 2 — System Intelligence Foundation (Entity Model + Deterministic Classification)
- **Verdict:** VERIFIED LOCALLY (Windows) — CI matrix execution pending push
- **Date:** 2026-09-08 · **Machine:** Windows 11 Pro x64, 8 GB RAM · **Agent run:** Phase 2

## Initial repository state (forensic inspection)

- Phase 1 intact on `main` at `77e9221` (== `origin/main`), working tree clean.
- Engine API read from source (`FsEntry`, `EntryKind`, `ScanOptions`,
  `CancelHandle`, streaming scanner) — not assumed from docs.
- No classification code existed. Phase 2 built additively in a new crate;
  zero Phase 1 files modified.

## Implemented crate (`crates/spacelens-classifier`)

| Module | Contents |
|---|---|
| `category.rs` | `Category` (17 semantic categories + stable IPC codes), `Subcategory` (7), Unknown≠Other distinction |
| `confidence.rs` | `Confidence` bands (Unknown<Low<Medium<High) + test-enforced caps (`EXTENSION_ONLY_CAP=Medium`, `HEURISTIC_CAP=Low`) |
| `evidence.rs` | `EvidenceKind` (11), `RuleId` (34 stable ids), bounded `EvidenceList` (`MAX_EVIDENCE=8`), no path text ever |
| `rules.rs` | 30-rule table (tiers 0–5), tier/needle/order precedence, conservative filename matching, exhaustive `rule_category` const fn, deterministic `evaluate()` |
| `context.rs` | `ParentContext` (pure) + `ParentContextTracker` (LRU-capped 4096), context raises confidence one band, never changes category, never rescues buckets |
| `classify.rs` | `classify()` / `classify_streaming()` → `Classification` (schema `spacelens.v1.classification`), evidence winner-first |
| `aggregate.rs` | `CategoryAggregator` — streaming, O(17) memory, `u64` saturating, canonical-order `CategoryReport` with coverage counters |
| `platform.rs` | `Platform` data enum (Windows/Mac/Linux); exactly one `cfg!` site in the crate; case-sensitivity semantics |
| `lib.rs` | Contracts, re-exports, degenerate-input smoke tests |

Dependencies: `serde` + path-dep on `spacelens-engine` only (dev: `serde_json`).
No I/O anywhere in the crate — classification is a pure function over
path/name/extension/metadata.

## Key design decisions

1. **New crate, additive.** Dependency chain stays `scanner → classifier`;
   Phase 1 untouched (verified by the full Phase 1 suite re-run).
2. **Rule table as versioned contract.** Table order = evidence order =
   tie-break order; adding rules appends, reordering is a contract change.
3. **Precedence is tier-based, documented, and test-enforced** — weak
   extension evidence can never override authoritative location evidence
   (master prompt §12).
4. **Honest uncertainty.** `Unknown` reserved (insufficient evidence);
   `Other` = understood but no useful primary category. Extension-only can
   never reach `High`; buckets are never rescued by context.
5. **Platform as data, not cfg branches** — platform rules selected by the
   `Platform` field; Windows/macOS case-insensitive dir matching, Linux
   case-sensitive (all three tested).
6. **Bounded memory everywhere**: evidence ≤8 items, tracker LRU-capped,
   aggregator fixed-size arrays. Streaming classification never turns the
   scanner into a whole-tree consumer.
7. **Conservative filename patterns**: word-ish prefix matching only
   (`setup.exe`, `Setup Wizard.exe`, `setup_2024.zip`); bare substring
   matches rejected (tested: `container` ≠ `install`).

## Tests executed (real commands + results, Windows local)

- `cargo fmt --check` → exit 0.
- `cargo clippy -j 2 --workspace --all-targets -- -D warnings` → exit 0.
- `cargo test -j 2 --workspace` → **127 passed / 0 failed / 1 ignored** total:
  - classifier lib: **57/57** (taxonomy, evidence bounds/order, confidence
    caps, tier precedence, platform leakage, case sensitivity, determinism).
  - classifier integration (`classifier_tests`): **22/22** (all §26 basic
    categories, evidence typed/bounded/ordered/no-paths, conflict resolution,
    parent/child context incl. mixed-content specificity, edge cases
    (Unicode/space/hidden/no-ext/multi-ext/300+-char names), invariants
    (determinism across all fixtures × 3 platforms, honest fallback,
    aggregator saturating at `u64::MAX`, tracker bounded at 10k feeds,
    no-panic fuzz, serde round-trip)).
  - Phase 1 regression all green: core 6, engine unit 11, traversal 6,
    error 6, link 5, cancel 5(+1 ignored), real-fs 7.
- Classifier perf smoke (`cargo test -p spacelens-classifier -- --ignored
  --nocapture`): 10k=91ms (~110k/s), 100k=823ms (~122k/s), 1M=8878ms
  (~113k/s) — linear scaling, aggregation exactly-once asserted, determinism
  rerun asserted. No throughput gate (correctness only, per
  docs/TESTING_STRATEGY.md).
- Phase 1 perf smoke re-run: 10k files in ~315–488 ms — passes.
- `npm ci` → exit 0 (0 vulnerabilities); `npm run build` → exit 0
  (tsc + vite, 27 modules).

## Security audit (grep + source inspection, this crate)

- No network/HTTP/socket/telemetry APIs — **the crate performs zero I/O**.
- No process spawning, no shell, no destructive filesystem APIs, no registry.
- No file-content reads (no `std::fs` usage at all in `src/`).
- No credentials/secrets handling; evidence never stores path text.

## Cross-platform audit

- Shared rule engine platform-neutral; platform knowledge isolated in
  `platform.rs` (1 `cfg!` site) + tier 0–1 platform rules in the table.
- Tested: Windows dir names case-insensitive; Linux case-sensitive; platform
  rules do not leak (`Program Files` inert on Linux, `.config` inert on
  Windows, `Library` inert on Windows); home markers per-OS
  (`\Users\`, `/Users/`, `/home/`).
- Linux/macOS runtime verification happens in GitHub CI (matrix) — pending
  first push of this phase; will be recorded here after run.

## Known limitations (honest)

1. macOS `.app` bundles: `MacAppBundle` id reserved, no suffix-matching rule
   yet — bundles fall through to name logic (documented, no panic).
2. Games category limited to Steam conventions (`steamapps`/`steamlibrary`).
3. `.ts` ambiguity resolved deterministically to TypeScript/Development;
   excluded from video table (documented trade-off).
4. Rule set deliberately small (~30 rules) — quality over quantity.
5. Timestamps/size not yet evidence inputs (reserved for future phase).
6. CI execution for this phase pending push; local verification is
   Windows-only until then.

## Phase-2 acceptance gate

- Deterministic classifier separate from scanner ✓ (pure crate, 0 I/O).
- 17 primary categories; Unknown ≠ Other ✓ (tested).
- Raw observations preserved (classification never mutates `FsEntry`) ✓.
- Explainable: evidence + winning rule on every result ✓ (typed, bounded,
  ordered, no paths).
- Confidence deterministic with hard caps ✓ (test-enforced).
- Rule engine: explicit table, defined precedence, deterministic conflict
  resolution, platform rules isolated ✓.
- Context: parent influence tested; no whole-tree buffering (LRU cap) ✓.
- Aggregation: streaming, `u64` saturating, O(categories) memory ✓.
- Privacy: no content reads, no network, no telemetry, no credentials, no
  destructive ops ✓.
- Performance: 1M-entry synthetic completes; memory bounded; deterministic ✓.
- Regression: Phase 1 tests + perf smoke + clippy + fmt + frontend build ✓.

## Commit SHA

Phase 2: recorded in `progress/CURRENT_PHASE.md` after commit.
