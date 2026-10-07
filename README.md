# CoreSight

> **Complex engine. Simple experience.**

macOS system intelligence + power-tools desktop app. **macOS is the primary
launch platform**; Windows/Linux abstractions remain in place for later
expansion. One-time purchase. Privacy-first. Offline by design.

Storage intelligence is one pillar of seven — see
`docs/MACOS_ARCHITECTURE.md` for the capability map.

See `progress/CURRENT_PHASE.md` for the current state.

## What it does now

- Phase 0 scaffold, Phase 1 filesystem engine, Phase 2 classifier,
  Phase 3/3.1/3.2 identity + content identity, Phase 4 relationships,
  Phase 5 system memory/change history (audited and repaired in 5.1),
  Phase 6 application intelligence foundation (`coresight-apps`),
  Phase 6.1 Mac-first capability architecture: shared contracts
  (`coresight-capabilities` — pillars, honest observation states, safety
  action pipeline, permission model) and the macOS discovery boundary
  (`coresight-macos` — source catalog + permission-aware observation).

## Layout

- `crates/coresight-core/` — Rust core scaffold: `v1` IPC contract types + SQLite bootstrap. Built + tested.
- `crates/coresight-capabilities/` — shared capability contracts: pillars, honest states, safety pipeline. Built + tested.
- `crates/coresight-macos/` — macOS discovery boundary: source catalog + bounded read-only observation. Built + tested.
- `src/` — React + TypeScript scaffold (`npm run build`).
- `src-tauri/` — Tauri 2 shell contract (config + capabilities). First compiled in MSVC CI (Phase 1+).
- `docs/` — product + architecture foundation (20 documents; start at `docs/MACOS_ARCHITECTURE.md`).
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
