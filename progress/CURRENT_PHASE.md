# SpaceLens — Current Phase

- **Current phase:** PHASE 3 — File identity, hashing & duplicate
  relationships (on top of verified Phases 1, 2, 2.1)
- **Status:** PHASE 3 — VERIFIED (full local gate green; CI run
  `34454636871` for `e453dd1` success — all 4 jobs; see PHASE_3_STATUS.md)
- **Last updated:** 2026-09-10 (Phase 3 identity engine)

## What happened (2026-09-10, Phase 3)

The RELATE layer: content identity (streaming SHA-256 behind the new
`PlatformFs::read_content` boundary), eligibility contract, size-group
candidacy (singletons never hashed), bounded worker pool, mutation-checked
hashing (`Changed`/`Vanished` typed, never a false relationship),
deterministic duplicate groups, and honest storage accounting
(`logical_duplicate_bytes` exact; `recoverable_bytes` only with provable
object identity — hard-link aliases count zero). Zero-byte files are counted,
not grouped, by default (explicit opt-in). Links are never followed. No DB,
no UI, no recommendations — nothing beyond Identity → Relationships.
53 new tests including a release-mode virtual >4 GiB streaming proof and a
ci-enforced perf smoke (10k/100k/1M, 4 hostile workloads, linear-scaling
guard). Commits `46b5dd1` → `929e400` → `15e3365` → `e453dd1`; CI green on
the final SHA after three diagnosed-and-repaired failures (unix root-entry
test counts, sparse-test hygiene, workflow YAML). Full record in
PHASE_3_STATUS.md.

## Previous pass (2026-09-09, Phase 2.1)

A focused semantic-hardening pass over the verified Phase 2 classifier:
`InstallerExtension` is now gated `Under(Downloads)` (an extension says what
bytes are, never where a file came from — `Program Files/App/setup.msi` is
`Applications`, a bare `blob.msi` is honest `Other`), `.appimage` moved to
the executable table (an AppImage *is* the application), macOS system-wide
`/Library/Caches` and `/Library/Logs` are recognised (`Cache`/`Logs`),
`~/Applications` is a per-user install location, `.` path components are
transparent in anchored location matching (`..` deliberately is not), and
`ParentContextTracker::with_capacity` clamps to the documented hard bound.
Unknown/Other, evidence fidelity, confidence caps, rule precedence, `.app`
contextual semantics, LRU core, and host-independent parsing were audited as
correct and pinned with tests. 23 new regression tests in
`tests/phase21_tests.rs`. Commit `bfb0452`; CI all green.

## Previous pass (2026-09-09, Phase 2 audit repair)

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

## Completed work (Phase 2 + 2.1, current state)

- `crates/spacelens-classifier` — pure, offline, explainable classification.
- 18 semantic categories (incl. `ApplicationData` ≠ `Applications`) +
  7 subcategories; `Unknown` (broken observation) ≠ `Other` (normal fallback).
- Two strengths of knowledge: rooted `LOCATION_RULES` (authoritative, may
  reach High) vs bare-name heuristics (capped at Low, Medium when
  corroborated). `UserHome` is a pure container.
- Gated `InstallerName` **and** `InstallerExtension` (`Under(Downloads)`);
  content-typed extensions outrank installer names, so `setup.zip` keeps
  archive semantics everywhere, and installer packages only claim `Downloads`
  where an authoritative download location vouches for them.
- `.appimage` classifies as `Applications` (it is the application, not an
  installer artifact); macOS `/Library/Caches`+`/Library/Logs` are
  `Cache`/`Logs`; `~/Applications` is a per-user install location; `.` path
  components are transparent in location matching; the LRU hard bound cannot
  be bypassed via `with_capacity`.
- Evidence kinds captured at match time (`RuleMatch`), bounded ≤8, ordered,
  never path text.
- Single mechanically enforced confidence policy (`RuleKind` →
  `Confidence::cap_for`); context raises one band inside the cap.
- Genuine LRU `ParentContextTracker` (refresh-on-hit, fixed slot pool).
- Streaming `CategoryAggregator` (u64 saturating, O(18) memory).
- Host-independent path analysis (both separators, drive tokens dropped);
  one `cfg!` site.
- Tests: 186 classifier tests (95 unit + 24 integration + 23 Phase 2.1
  regression + 43 audit-regression + 1 always-on perf companion) + ignored
  perf benchmark (10k/100k/1M, linear scaling).

## Verified work (post-repair, local Windows)

- `cargo fmt --check` → clean.
- `cargo clippy -j 2 --workspace --all-targets -- -D warnings` → clean.
- `cargo test -j 2 --workspace` → 200 passed / 0 failed / 2 ignored.
- Classifier perf smoke: linear 10k→1M, deterministic, bounded memory
  (throughput A/B-attributed against the pristine baseline: no regression
  from Phase 2.1; the previous ~56k/s record was a different session/host
  state).
- Phase 1 perf smoke: passes. `npm ci` + `npm run build`: exit 0 (CI).
- Security grep audit: the classifier crate performs zero I/O.

## Known issues / blockers

1. No MSVC toolchain — Tauri link/bundle requires CI (unchanged).
2. C: free space ~6.5 GB — all dev data stays on D: (unchanged).
3. (Resolved in Phase 2.1) Installer extensions no longer map to `Downloads`
   everywhere: they are gated to authoritative download locations, like
   installer names.
4. Local Windows host silently drops symlink creation (filter driver/AV);
   CI runners are unaffected, and the link test now skips gracefully.

## Next authorized task

- STOP after the Phase 2.1 hardening/reverification report. Do NOT start
  Phase 3 without explicit authorization.

## Forbidden tasks

- No hashing/duplicate detection (Phase 3). No cleanup/executor (Phase 4).
- No recommendations. No history/snapshots. No full UI. No relationship graph.
- No renames / no product-behavior changes outside the classifier crate.

## Verification log

- 2026-09-09 (Phase 2.1): semantic hardening pass. Six confirmed weaknesses
  fixed with 23 regression tests; verified-correct behavior pinned and
  documented. Local gate green (fmt/clippy/204 tests/perf smokes). Commit
  `bfb0452` pushed; CI run `34341554863` **success — all 4 jobs** (all job
  conclusions verified individually via the GitHub API). PHASE 2.1 — VERIFIED.
- 2026-09-09 (audit repair): all ten findings confirmed and fixed with
  regression tests; three second-order defects fixed; docs rewritten to
  describe the implementation that actually exists. Local gate green
  (fmt/clippy/200 tests/perf smokes/npm). Final CI run recorded in
  PHASE_2_STATUS.md. Repair commit `6e3ecc0` pushed; CI run for that commit
  **success — all 4 jobs** (fmt + clippy -D warnings + tests + both perf
  smokes on ubuntu/windows/macos; npm ci + build). PHASE 2 — VERIFIED.
- 2026-09-08 (Phase 2 original): built at `d49609e`, CI fix `0827f84`,
  CI record `a7bbf2f` (run `34251905113` all 4 jobs green). The audit
  superseded this verdict.
- 2026-09-08 (Phase 1): VERIFIED — see PHASE_1_STATUS.md. Unaffected by the
  repair (re-run green).
