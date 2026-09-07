# SpaceLens — Phase Map & Roadmap

## Gate rule

A phase is VERIFIED only when its status file checks every acceptance criterion
with real observed evidence (commands + results). IMPLEMENTED ≠ VERIFIED.
No phase starts without authorization recorded in `CURRENT_PHASE.md`.

## Phases

- [x] **PHASE 0 — Product validation + foundation** (this file's era).
  Research, challenge, thesis, UX/architecture/safety/perf/DB/API/testing/dev-rules
  docs, differentiation verdicts, business model, minimal scaffold, toolchain
  verification. Status: `progress/PHASE_0_STATUS.md`.
- [ ] **PHASE 1 — Filesystem engine.** Parallel walker, metadata model, cancel/
  progress, fixture suites, platform-trait skeleton. Success: 1M-file fixture
  scan within perf-smoke budget on Windows + CI Linux.
- [ ] **PHASE 2 — Storage analysis + intelligence.** Classifier rule engine v1,
  categories, app attribution. Success: fixture trees classify ≥ agreed accuracy
  bar; rules versioned + re-runnable without rescan.
- [ ] **PHASE 3 — Duplicate detection.** SHA-256 candidate hashing, hardlink
  collapse, hash cache. Success: zero false-positive tolerance suite green;
  cache-hit rescan of unchanged tree.
- [ ] **PHASE 4 — Cleanup + safety engine.** Planner, safety validator with veto
  tests, Trash/quarantine adapters, preview + verify. Success: adversarial
  safety suite green on all three OSes in CI.
- [ ] **PHASE 5 — Indexing + storage history.** Snapshots, deltas, drive memory,
  retention. Success: "+80 GB why?" answered on fixtures with per-category deltas.
- [ ] **PHASE 6 — Platform-specific intelligence.** Steam/Xcode/Snap/Flatpak/
  cloud-placeholder tables per OS. Success: platform-matrix parity review signed.
- [ ] **PHASE 7 — Application integration.** Per-app footprints + leftover
  attribution, uninstall planning (reversible). Success: top-50 common apps
  fixture-verified.
- [ ] **PHASE 8 — Professional frontend.** Five screens per UX/design contracts,
  a11y pass, empty/loading/error states. Success: design-review + contract tests.
- [ ] **PHASE 9 — Full integration.** Engine↔Tauri↔UI wired, E2E on fixtures,
  operations log + undo flows. Success: scripted fixture cleanups verify byte-exact.
- [ ] **PHASE 10 — Performance + security + adversarial testing.** MFT fast-path
  evaluation, full adversarial suite, perf budgets enforced. Success: release gates green.
- [ ] **PHASE 11 — Packaging + distribution.** Installers per OS, updater,
  licensing (one-time), notarization/signing. Success: clean-room installs on all 3 OSes.
- [ ] **PHASE 12 — Production polish.** Copy, onboarding, support flows, docs site.
  Success: release candidate + rollout plan.

Adjustments require a note here with rationale — this file is the roadmap's
changelog.
