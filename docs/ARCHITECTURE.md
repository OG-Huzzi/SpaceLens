# SpaceLens — Technical Architecture

Status: Phase 0 contract. Evaluated against the proposed stack; stack is KEPT.
Minimal scaffold validation in Phase 0; full implementation starts Phase 1.

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

## Core engine module boundaries (future; names are contracts)

- `scanner` — parallel walk, metadata collection, progress/cancel. Knows nothing
  of categories or UI.
- `classifier` — maps entries → human categories + app attribution. Pure function
  over metadata; rule tables versioned and testable without a disk.
- `hasher` / `duplicates` — content hashing (SHA-256), exact-duplicate grouping
  with hardlink collapse. Hash cache in DB; never re-hash unchanged files.
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
