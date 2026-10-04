# CoreSight

> **Complex engine. Simple experience.**

Cross-platform storage intelligence desktop app (Windows / macOS / Linux).
One-time purchase. Privacy-first. Offline by design.

See `progress/CURRENT_PHASE.md` for the current state.

## What it does now

- Phase 0 scaffold, Phase 1 filesystem engine, Phase 2 classifier,
  Phase 3/3.1/3.2 identity + content identity, Phase 4 relationships,
  Phase 5 system memory/change history (audited and repaired in 5.1),
  Phase 6 application intelligence foundation (`coresight-apps`).

## Layout

- `crates/coresight-core/` — Rust core scaffold: `v1` IPC contract types + SQLite bootstrap. Built + tested.
- `src/` — React + TypeScript scaffold (`npm run build`).
- `src-tauri/` — Tauri 2 shell contract (config + capabilities). First compiled in MSVC CI (Phase 1+).
- `docs/` — product + architecture foundation (13 documents).
- `progress/` — phase tracking for multi-agent handoff.

## Verify

```sh
cargo test --workspace   # full Rust workspace (needs gcc for rusqlite bundled)
npm install && npm run build   # React + TypeScript + Vite
```

Toolchain notes (Windows): GNU Rust target `stable-x86_64-pc-windows-gnu`
installed under `D:/.cargo` (C: is full); Tauri bundling additionally needs the
MSVC toolchain + Windows SDK — see `progress/PHASE_0_STATUS.md`.

## Rules

Read `docs/DEVELOPMENT_RULES.md` before touching anything. The short version:
inspect first, never fake verification, smallest diff, build + test everything,
update `progress/`, don't start the next phase unasked.
