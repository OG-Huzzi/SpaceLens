# CoreSight — Phase Map & Roadmap

## Launch-platform priority (Phase 6.1, 2026-10-06)

**macOS is the PRIMARY implementation and launch platform.** Development
priority order from here on:

```
macOS backend/core
    ↓
macOS capabilities (inventory, footprints, startup/launchd, volumes, diagnostics)
    ↓
macOS safety (typed pipeline hardening against real actions)
    ↓
macOS product completeness
    ↓
frontend / release work
    ↓
Windows/Linux expansion (later; abstractions stay intact meanwhile)
```

Windows/Linux abstractions and existing per-OS behavior are NOT deleted.
CoreSight is NOT a "Mac cleaner" — it is a macOS system intelligence +
power-tools application; storage is one pillar (docs/MACOS_ARCHITECTURE.md).

## Gate rule

A phase is VERIFIED only when its status file checks every acceptance criterion
with real observed evidence (commands + results). IMPLEMENTED ≠ VERIFIED.
No phase starts without authorization recorded in `CURRENT_PHASE.md`.

Historical note: documents of the SpaceLens era (Phase 0–5 status files) keep
their original product name — they are records of what was done then, not
current-state claims. Current-state documents use CoreSight.

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
  - [x] **PHASE 3.1 — identity correctness hardening.** No-follow content
    opens (link-TOCTOU closed), observed-vs-opened object verification,
    same-length mutation brackets, globally bounded staging with exact
    skip accounting. Success: adversarial suite green on all three OSes
    in CI; docs claim nothing the code does not provide.
  - [x] **PHASE 3.2 — Windows scan-time object identity.** `FILE_ID_INFO`
    via a query-only handle, ancestor-chain link guard, scan→open mtime
    bracket. Status: `progress/PHASE_3_2_STATUS.md`.
- [ ] **PHASE 4 — Cleanup + safety engine.** Planner, safety validator with veto
  tests, Trash/quarantine adapters, preview + verify. Success: adversarial
  safety suite green on all three OSes in CI.
- [ ] **PHASE 5 — Indexing + storage history.** Snapshots, deltas, drive memory,
  retention. Success: "+80 GB why?" answered on fixtures with per-category deltas.
  - [x] **PHASE 5.1 — historical identity & event semantics repair.**
    Full 128-bit identity end-to-end (migration v3 + v4), `Modified`
    requires same-object proof, path-level deletion independent of
    object survival, 1:1-provable moves only, lossless tagged path
    persistence, platform-aware scope comparison, strict corruption
    rejection, newer-schema refusal. Audited again 2026-10-06
    (lossless event ids, extreme-value roundtrips, typed decode errors);
    41 regression tests. Status: `progress/PHASE_5_STATUS.md`.
- [ ] **PHASE 6 — Platform-specific intelligence.** Steam/Xcode/Snap/Flatpak/
  cloud-placeholder tables per OS. Success: platform-matrix parity review signed.
  - [ ] **PHASE 6.0 (foundation) — application intelligence `coresight-apps`.**
    IMPLEMENTED + audited (domain, Win32 uninstall discovery, MSIX
    abstracted, footprint evidence, ownership strength, explanations;
    five-state source coverage; bounded/deterministic; 60 tests). CI
    gate green on all three OSes + frontend (run 37483779232). Still
    NOT VERIFIED as a phase: persistence is not built and the phase's
    own success criteria (platform-matrix parity review) are unmet.
    See `progress/CURRENT_PHASE.md`.
  - [x] **PHASE 6.1 — Mac-first power-tools foundation & product
    architecture.** CoreSight re-architected as a macOS system
    intelligence + power-tools application (7 pillars): new shared
    contracts crate `coresight-capabilities` (pillars, typed capability
    contracts A–H with pinned honest statuses, observation honesty
    envelope, path-access truth model, safety action pipeline with
    unskippable stages + policy-gated veto point) and new macOS boundary
    crate `coresight-macos` (15-source classified catalog, bounded
    read-only observation, honest `Unsupported` on non-macOS hosts).
    Scanner honesty repairs: `FollowWithCycleGuard` explicitly rejected
    with typed `Unsupported` (never silently record-only), root links
    get child-link semantics, link-target/metadata read errors typed.
    Linux mount paths decoded byte-exactly (lossless). Status: see
    `progress/CURRENT_PHASE.md` (macOS runtime observation awaits CI).
  - [x] **PHASE 6.1 (verification) — independent audit & hardening.**
    P0 identity unification (`ObjectIdentity`: full wide identity through
    pipeline → relationships → history, narrow/legacy never fabricated),
    application-identity rule made explicit (id = merge key; source is
    provenance), canonical merge/footprint precedence (no arrival-order
    ties), true boundedness (streaming bounded PathProber + registry
    enumeration, bounded inventory/footprint admission), lossless
    classifier path handling, `PRAGMA integrity_check` on store open,
    docs reconciled. CI VERIFIED: run 37589662213 for `94c3164`
    (ubuntu/windows/macos/frontend all SUCCESS). Status:
    `progress/CURRENT_PHASE.md`.
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
