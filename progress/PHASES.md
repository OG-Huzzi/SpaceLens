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
- [ ] **PHASE 6.5 — SQLite pre-migration backup & corrupt-store quarantine.**
  IMPLEMENTED + VERIFIED (from `5da54e9cd57b15c4bbdde8a4c175ce9dc66477b4`).
  A verified, SQLite-consistent backup of an existing healthy store is completed
  and published before any forward migration mutates it; a store whose
  corruption is positively established is relocated to a collision-safe
  quarantine (including `-wal`/`-shm` sidecars) instead of being replaced or
  deleted. Schema stays v6; migration SQL v2–v6 unchanged. Status:
  `progress/CURRENT_PHASE.md`.
  - [ ] **PHASE 6.5.1 — safety closure of the 6.5 open path.**
    IMPLEMENTED + VERIFIED (from `1b458e82095e51031da46978d94f7f282ebb3297`,
    implementation CI run 38722753282 all four jobs green).
    Concurrent backup attempts own their artifact names and never adopt or
    delete another attempt's; a valid SQLite file this build cannot recognize
    is refused with its database file and `-wal` byte-identical instead of
    being initialized over; corruption introduced by the bootstrap's own
    writes is quarantined like the preflight's; a partially failed quarantine
    reports exactly which files moved and which did not, proven with real
    moves to roll back. Schema stays v6; migration SQL v2–v6 byte-identical.
    Status: `progress/CURRENT_PHASE.md`.
  - [ ] **PHASE 6.5.2 — remaining 6.5 safety gaps (test-only API surface,
    fail-closed integrity, full per-version recognition, preflight-to-write
    race).** IMPLEMENTED + VERIFIED (from
    `d62665a87e5c0eb58b4a2e31ad193dba8f58cf13`, implementation CI run
    38739906275 all four jobs green).
    The hazard/backup/quarantine seams are crate-private (no
    `open_with_hazard`, no `#[doc(hidden)] pub`, no test-only feature); an
    `integrity_check` that returns NO rows is a failure to establish
    integrity, never health (store left in place, no quarantine, no backup
    published); the corruption classifier compares SQLite's RAW result
    codes; recognition enforces the full per-version structure (tables +
    signature columns, v1–v6) with adversarial fixtures proving untouched
    refusal; and the read-write connection that writes is re-validated by a
    mutation gate on that same connection, so a database swapped in after
    the read-only gate is either handled under its own verdict or refused
    without being written. Schema stays v6; migration SQL v2–v6
    byte-identical. Status: `progress/CURRENT_PHASE.md`.
  - [ ] **PHASE 6.5.3 — concurrent open serialization & file-identity
    safety.** IMPLEMENTED. Concurrent opens of the same existing store are
    serialized on SQLite's own cross-process write lock: each migration step
    takes `BEGIN IMMEDIATE` and re-reads the recorded version inside it, so a
    second opener observes the completed step and skips it instead of applying
    a stale plan; the v1 bootstrap decision is re-made under the same lock; the
    WAL conversion (which the busy handler does NOT cover) is retried within a
    bounded timeout and then fails closed. Quarantine relocation is bound to
    the file whose corruption was observed (a platform metadata identity
    captured while the read-only handle pinned it, re-checked before the
    rename), so a substituted file is never moved as another file's
    corruption. Schema stays v6; migration SQL v2–v6 byte-identical. Status:
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
