# SpaceLens — Current Phase

- **Current phase:** PHASE 2 — System Intelligence Foundation (Classification)
- **Status:** audit repair complete, final verification in progress — see
  PHASE_2_STATUS.md for the authoritative record
- **Last updated:** 2026-09-09 (independent audit repair pass)

## What happened (2026-09-09)

An independent source-level audit of the previously-"VERIFIED" Phase 2 crate
confirmed **all ten findings** (installer-name overreach, application-data
conflation, FIFO-not-LRU tracker, mislabelled evidence, unenforced confidence
caps, context-free basename matching, no real Unknown/Other contract, missing
Clippy in CI, host-path dependence, plus a second-order sweep). Every
confirmed defect was repaired in `crates/spacelens-classifier`, each with a
regression test in `tests/semantics_tests.rs` that fails against the
pre-repair behavior. Three additional second-order defects were found and
fixed (`bin`/`env` false positives, substring user-profile detection,
duplicate `matched_rules` ids). CI now enforces Clippy on every platform.

Phase 1 engine code is untouched (one test *helper* in
`real_fs_tests.rs` was made robust to hosts where symlink creation reports
success but creates nothing — engine code and assertions unchanged; verified
pre-existing on the pristine baseline).

## Completed work (Phase 2, current state)

- `crates/spacelens-classifier` — pure, offline, explainable classification.
- 18 semantic categories (incl. `ApplicationData` ≠ `Applications`) +
  7 subcategories; `Unknown` (broken observation) ≠ `Other` (normal fallback).
- Two strengths of knowledge: rooted `LOCATION_RULES` (authoritative, may
  reach High) vs bare-name heuristics (capped at Low, Medium when
  corroborated). `UserHome` is a pure container.
- Gated `InstallerName` (`Under(Downloads)`); content-typed extensions
  outrank installer names, so `setup.zip` keeps archive semantics everywhere.
- Evidence kinds captured at match time (`RuleMatch`), bounded ≤8, ordered,
  never path text.
- Single mechanically enforced confidence policy (`RuleKind` →
  `Confidence::cap_for`); context raises one band inside the cap.
- Genuine LRU `ParentContextTracker` (refresh-on-hit, fixed slot pool).
- Streaming `CategoryAggregator` (u64 saturating, O(18) memory).
- Host-independent path analysis (both separators, drive tokens dropped);
  one `cfg!` site.
- Tests: 161 classifier tests (93 unit + 24 integration + 43 audit-regression
  + 1 always-on perf companion) + ignored perf benchmark (10k/100k/1M,
  ~56k entries/sec, linear).

## Verified work (post-repair, local Windows)

- `cargo fmt --check` → clean.
- `cargo clippy -j 2 --workspace --all-targets -- -D warnings` → clean.
- `cargo test -j 2 --workspace` → 200 passed / 0 failed / 2 ignored.
- Classifier perf smoke: linear 10k→1M, deterministic, bounded memory.
- Phase 1 perf smoke: passes. `npm ci` + `npm run build`: exit 0.
- Security grep audit: the classifier crate performs zero I/O.

## Known issues / blockers

1. No MSVC toolchain — Tauri link/bundle requires CI (unchanged).
2. C: free space ~6.5 GB — all dev data stays on D: (unchanged).
3. Installer extensions map to `Downloads` everywhere (inherited v1 choice,
   pinned by tests, documented — a taxonomy decision deferred by design).
4. Local Windows host silently drops symlink creation (filter driver/AV);
   CI runners are unaffected, and the link test now skips gracefully.

## Next authorized task

- STOP after the Phase 2 repair/reverification report. Do NOT start Phase 3
  without explicit authorization.

## Forbidden tasks

- No hashing/duplicate detection (Phase 3). No cleanup/executor (Phase 4).
- No recommendations. No history/snapshots. No full UI. No relationship graph.
- No renames / no product-behavior changes outside the classifier crate.

## Verification log

- 2026-09-09 (audit repair): all ten findings confirmed and fixed with
  regression tests; three second-order defects fixed; docs rewritten to
  describe the implementation that actually exists. Local gate green
  (fmt/clippy/200 tests/perf smokes/npm). Final CI run recorded in
  PHASE_2_STATUS.md.
- 2026-09-08 (Phase 2 original): built at `d49609e`, CI fix `0827f84`,
  CI record `a7bbf2f` (run `34251905113` all 4 jobs green). The audit
  superseded this verdict.
- 2026-09-08 (Phase 1): VERIFIED — see PHASE_1_STATUS.md. Unaffected by the
  repair (re-run green).
