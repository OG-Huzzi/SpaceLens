# CoreSight — Technical Architecture

Status: Phase 0 contract; capability layer added in Phase 6.1. Evaluated
against the proposed stack; stack is KEPT. The product is a macOS system
intelligence + power-tools application (see docs/MACOS_ARCHITECTURE.md) —
storage is one pillar, not the whole product.

## Stack decision

| Layer | Choice | Verdict |
|---|---|---|
| Core engine | Rust | KEEP. Correct: performance, safety, single codebase for scanning/hashing/classification across OSes. |
| Desktop shell | Tauri 2 | KEEP. Correct: small binaries, OS webviews, Rust-native IPC, no Electron bloat — fits "premium + trustworthy." |
| Frontend | React + TypeScript | KEEP. Correct: hiring, component model for the 5-screen app, strict typing at the IPC boundary. |
| Local database | SQLite | KEEP. Correct: serverless, single-file, proven at millions of rows, WAL mode for concurrent scan-write/UI-read. rusqlite on the Rust side owns all access. |

No change recommended. Alternatives considered and rejected: Electron (binary size
+ memory + supply-chain weight contradict the trust story), C++/Qt (smaller hiring
pool, slower iteration for the explanation layer), cloud backend (contradicts
privacy-first; there is no server).

**Phase 0 environment note (verified):** Rust 1.98.1 GNU toolchain installed and
working on this Windows 11 machine (see `progress/PHASE_0_STATUS.md`). Full Tauri
`build`/`bundle` additionally requires the MSVC toolchain + Windows SDK, which are
NOT installable here (C: has ~258 MB free; VS Build Tools need gigabytes). So:
Rust-compile + test is verified locally; Tauri bundling is contract-defined here
and must be CI-verified (GitHub Actions windows/macos/linux) from Phase 1 onward.

## Layered architecture

```
React + TypeScript (views, 5 screens, NO filesystem logic)
        ↕  typed IPC (commands / events — docs/API_CONTRACTS.md)
Tauri 2 shell (window, updater, per-OS webview, license plumbing later)
        ↕  application services (Rust: scan orchestration, jobs, settings)
Rust core engine (pure logic: walk, hash, classify, recommend, plan, validate)
        ↕  platform abstraction trait (fs ops, trash, metadata per OS)
Windows / macOS / Linux implementations
        ↕
SQLite (rusqlite, WAL; scans, entries, hashes, snapshots, ops log)
```

## Hard boundaries

1. **The UI never touches the filesystem.** No Node fs calls for product data,
   no path manipulation beyond display. Every byte of filesystem truth comes
   through IPC from Rust. (Rationale: one enforcement point for safety.)
2. **The engine never touches the network.** Scanning, hashing, classification
   are pure offline functions. License checks (later phase) live in the Tauri
   shell, isolated from the engine.
3. **All destructive intent flows through the safety pipeline:**
   recommend → user review → plan → safety-validate → confirm →
   quarantine/trash → verify. No shortcut path exists in code; tests assert this.
4. **SQLite is owned by Rust.** The frontend never opens the DB file. Migrations
   are versioned, forward-only, tested against fixtures (docs/DATABASE.md).
5. **Platform code hides behind traits.** `PlatformFs`, `Trash`, `DriveInfo`,
   `SysDirs` traits with per-OS modules. Shared logic never `cfg!`-branches on
   OS behavior; only the trait impls do (docs/CROSS_PLATFORM.md).

## Core engine module boundaries (implemented so far)

- `scanner` — parallel walk, metadata collection, progress/cancel. Knows nothing
  of categories or UI. (Phase 1: `crates/coresight-engine`.)
- `classifier` — maps entries → human categories + app attribution. Pure function
  over metadata; rule tables versioned and testable without a disk.
  (Phase 2/2.1: `crates/coresight-classifier`.)
- `hasher` / `duplicates` — content hashing (SHA-256), exact-duplicate grouping
  with hardlink collapse. (Phase 3: `crates/coresight-identity`, docs/IDENTITY.md.)
  Phase 3.1 hardened the same layer: no-follow content opens (links that
  replace an observed file are refused, never followed), observed-vs-opened
  object verification (`Replaced`), handle-proven mutation brackets
  (length + change-time before/after the read), and globally bounded
  candidate staging with exact skip accounting (`CompletedWithLimits`).
  Phase 3.2 added Windows scan-time object identity (`FILE_ID_INFO` via a
  query-only handle — NTFS file ids embed the MFT record sequence number,
  so delete+recreate impostors are detectable), an ancestor-chain guard
  (a symlink/junction anywhere in the observed path's chain is refused at
  hash time), and a scan→open mtime bracket.
- `history` — the Phase 5 system-memory layer: persisted run records +
  normalized snapshots (SQLite via the existing core schema, forward-only
  migrations v1–v5: history tables, full 128-bit object identity +
  lossless tagged paths, persisted relationship-report status, and the
  Phase 6.4 `app_snapshot_*` application/system snapshot tables), a PURE
  comparison engine deriving typed evidence-backed change events
  (incomplete-scan safe: partial runs never fake deletions; scope
  boundaries enforced; configuration fingerprints preserved; event ids
  content-addressed over LOSSLESS path encodings), a query API
  (path/object/content/relationship history plus per-run snapshot
  commit/load/list/rebuild) with strict corruption
  rejection and newer-schema refusal, deterministic bounded retention,
  and crash recovery (`crates/coresight-history`, docs/HISTORY.md).
  Phase 6.4's persistence boundary is explicit: canonical facts are
  stored as normalized columns, derived model state is never stored, and
  reload rehydrates through the same `build_system_model` path
  (docs/DATABASE.md, docs/SYSTEM_MODEL.md §16).
- `applications` — the Phase 6 application-intelligence layer, deepened in
  Phase 6.2. A platform-neutral application domain (stable content-derived
  `ApplicationId` over normalized `(name, publisher)` — source is provenance,
  never identity; record; five-state source coverage; explicit bounds) plus:
  provider-based discovery (Windows Win32 uninstall registry across
  HKLM-64 / HKLM-32 / HKCU; MSIX/AppX honestly `Unsupported`; macOS
  `*.app/Contents/Info.plist` bundles; Linux/BSD `.desktop` entries — all
  local file metadata only); install-root detection from multiple independent
  signals; executable association distinguishing observed/inferred/candidate;
  a correlation-grouped ownership-evidence model with a documented
  anti-inflation ceiling; artifact relationships that keep *contains*,
  *owns*, *associated-with*, and *conflicting* distinct; shared/conflicting
  ownership that never silently resolves; structured (machine-readable)
  explanations; inert read-only recommendation candidates; and bounded,
  deterministic analysis where every collection admits through a top-K
  structure (**O(limit)** working memory, exact overflow accounting).
  `crates/coresight-apps`; no network, no subprocess, no executor, no
  persistence. See docs/APPLICATIONS.md.
- `capabilities` — the Phase 6.1 Mac-first capability architecture:
  the seven product pillars, the typed capability contracts with pinned
  honest statuses, the observation honesty envelope (observed / inferred /
  unsupported / unavailable / failed — a non-observed state structurally
  cannot carry a payload), the path-access truth model (exists-but-
  inaccessible ≠ empty, seven states), and the safety action pipeline
  (OBSERVE → ANALYZE → RECOMMEND → PREVIEW → VALIDATE → EXECUTE → VERIFY
  → ROLLBACK with an unskippable stage machine and a policy-gated veto
  point) (`crates/coresight-capabilities`; platform-neutral by source-scan
  test).
- `macos` — the Phase 6.1 macOS discovery boundary: the classified source
  catalog (14+ macOS locations with read access, sensitivity, modification
  risk, phase availability), bounded read-only listing observation with
  honest access states, and `Unsupported` reporting on non-macOS hosts
  (`crates/coresight-macos`; never modifies anything, never bypasses TCC).
  See docs/MACOS_ARCHITECTURE.md.
- `system-model` — the Phase 6.3 unified, in-memory system model, independently
  hardened and verified on commit `df2e24c`. Phase 6.4 made its **snapshot
  inputs durable** without touching the crate: persistence lives at the
  history boundary (schema v5), and reload feeds the same builder.
  A pure, deterministic, bounded correlation
  of filesystem observations, canonical object identity, classification
  (copied, never re-derived), identity relationships, Phase 6.2 application
  intelligence, capability state (reported verbatim, never upgraded), and
  caller-projected history context — into one immutable artifact/application
  graph with private canonical storage and read-only accessors; typed edges
  (`Contains`, `LocatedUnder`, install-root/executable/data/cache/log/
  config, `OwnedBy`, `AssociatedWith`, `SharedBy`, `DuplicateOf`,
  `HardLinkAliasOf`); node-attached historical assertions (`SameObjectObserved`
  / `ObjectReplaced` / `IdentityUnproven` — history is quoted context, never
  a graph edge and never a self-loop); structured evidence with Phase 6.2
  correlation ceilings that survive module boundaries; proof-validated
  relationship joins (wrong identity/digest, same-object-as-duplicate, and
  missing proofs are rejected with exact accounting); preserved conflicts
  (duplicate applications resolve by a total-order precedence, duplicate
  history rows are both preserved); truncation-aware association status
  (`AssociationTruncated`: claim truncated ≠ no claim — a bound can never
  manufacture `Unassociated`/`Orphan`); bounded typed queries with honest
  per-query complexity; descriptive insights; and inert candidates. Indexes
  are derived implementation details — rebuilt on every construction route
  (including deserialization, which validates canonical structure first) and
  never trusted from input. No history inference, no executor, no
  subprocess, no network, no persistence (`crates/coresight-system-model`).
  See docs/SYSTEM_MODEL.md.
- `relationships` — the Phase 4 relationship-intelligence layer: typed
  relationship kinds (hard-link aliases vs content duplicates), categorical
  evidence, deterministic ordering, conservative recoverability, undetermined
  summaries, and a query index. A PURE derivation over the verified pipeline
  output — no I/O, no destructive actions, no recommendations yet
  (`crates/coresight-identity::relationships`, docs/RELATIONSHIPS.md).
  The persistent hash cache belongs with persistence (later phase); Phase 3
  deliberately added no database.
- `recommender` — produces opportunities with reasons + recovery estimates.
  Advisory only; cannot delete.
- `planner` + `safety` — turns accepted recommendations into a validated plan;
  `safety` holds veto power (system/boot/user-data/link/volume/cloud rules).
- `quarantine` / `trash-adapter` — reversible execution + result verification.
- `index` / `history` — snapshots, deltas, drive memory, retention.

Dependency rule: `scanner → classifier → recommender → planner → safety → executor`.
Nothing upstream depends on anything downstream. `safety` depends on nothing
except policy tables and the plan — so it can never be "convinced" by a caller.

## IPC shape (summary; full contract in docs/API_CONTRACTS.md)

- Commands: `start_scan`, `cancel_scan`, `get_categories`, `get_opportunities`,
  `preview_plan`, `confirm_plan`, `get_history`, `list_drives`.
- Events (Rust→UI): `scan_progress`, `scan_complete`, `plan_verified`.
- Errors: typed `{ code, message, detail }`, stable codes, versioned API (`v1`).

## Performance posture

Bounded concurrency, streaming progress, cancel-everything, hash caching,
incremental rescan, UI reads never block on scan writes (WAL + reader snapshots).
Budgets and measurement method in docs/PERFORMANCE.md.
