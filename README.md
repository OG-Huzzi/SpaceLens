# SpaceLens

> **Complex engine. Simple experience.**

Cross-platform storage intelligence desktop app (Windows / macOS / Linux).
One-time purchase. Privacy-first. Offline by design.

**Phase 0** — product validation + foundation only. No product engine is built
here; see `progress/CURRENT_PHASE.md` and `progress/PHASE_0_STATUS.md`.

## Layout

- `crates/spacelens-core/` — Rust core scaffold: `v1` IPC contract types + SQLite bootstrap. Built + tested.
- `src/` — React + TypeScript scaffold (`npm run build`).
- `src-tauri/` — Tauri 2 shell contract (config + capabilities). First compiled in MSVC CI (Phase 1+).
- `docs/` — product + architecture foundation (13 documents).
- `progress/` — phase tracking for multi-agent handoff.

## Verify (Phase 0)

```sh
cargo test -p spacelens-core   # Rust + SQLite (needs gcc for rusqlite bundled)
npm install && npm run build   # React + TypeScript + Vite
```

Toolchain notes (Windows): GNU Rust target `stable-x86_64-pc-windows-gnu`
installed under `D:/.cargo` (C: is full); Tauri bundling additionally needs the
MSVC toolchain + Windows SDK — see `progress/PHASE_0_STATUS.md`.

## Rules

Read `docs/DEVELOPMENT_RULES.md` before touching anything. The short version:
inspect first, never fake verification, smallest diff, build + test everything,
update `progress/`, don't start the next phase unasked.
