# CoreSight — Current State

- **Current phase:** PHASE 6.1 — Mac-first power-tools foundation &
  product architecture.
- **Status:** Phase 6.1 COMPLETE on this machine (all local gates green,
  macOS/Linux cross-checks green). NOT yet VERIFIED as a phase: the
  macOS runtime observation path awaits macOS-CI execution, and this
  phase awaits independent verification before any further work.
- **Product direction (binding from this phase):** CoreSight is a
  **macOS system intelligence + power-tools application** — NOT a "Mac
  cleaner". macOS is the primary implementation and launch platform;
  Windows/Linux remain architectural targets with their abstractions
  intact. Storage is one pillar of seven
  (docs/MACOS_ARCHITECTURE.md).
- **Last updated:** 2026-10-06.

## What happened in this phase (2026-10-06 → 2026-10-07)

1. **Architecture audit.** Read every doc, all six crates and the
   progress files; verified each Phase 6.1 assumption in code rather
   than trusting prior reports.
2. **New shared contracts crate `coresight-capabilities`** (platform-
   neutral; a source-scan test bans OS conditionals in it):
   - Seven-pillar capability taxonomy (`Pillar`).
   - Typed capability contracts A–H plus privacy/software-management,
     each with a stable kebab id, pillar, and a PINNED honest status
     (`Implemented`/`Partial`/`Planned`/`Deferred`) in `CONTRACTS` —
     overclaiming is now a failing test.
   - `Observation<T>` honesty envelope: observed / inferred /
     unsupported / unavailable / failed as a tagged enum — a
     non-observed state structurally cannot carry a payload, and
     deserialization cannot smuggle one in (`deny_unknown_fields`).
   - `AccessState` path-access truth model (7 states): exists-but-
     inaccessible ≠ empty ≠ does-not-exist ≠ unsupported ≠ failed.
   - Safety action pipeline: explicit classification (exactly one
     effect + optional privileged/permission-sensitive qualifiers),
     OBSERVE→…→ROLLBACK with an unskippable stage machine, gate-only
     VALIDATE, and `ExecutionPolicy::CURRENT_BUILD` (read-only only) at
     the veto point. A blocked verdict permanently bars EXECUTE. No
     executor exists anywhere in this build.
3. **New macOS boundary crate `coresight-macos`** (compiles everywhere;
   observes only on macOS; `Unsupported` on other hosts — never empty):
   - 15-source catalog (`SOURCES`): read access / sensitivity /
     modification risk / phase availability for /Applications, ~/Library
     areas (Application Support, Caches, Logs, Containers, Group
     Containers, Preferences), LaunchAgents/LaunchDaemons (user +
     system), login items (deferred — needs OS API), TCC-protected user
     data (RequiresFullDiskAccess, deferred, never probed without an
     explicit user grant), mounted volumes, APFS volume info (deferred).
   - Bounded read-only listing observation with honest access-state
     mapping (metadata-denied ⇒ Failed, listing-denied-after-stat ⇒
     ExistsButInaccessible, empty listing ⇒ Empty). No content reads,
     no recursion, no writes, no subprocesses, no privilege escalation.
   - Capability↔source relation table; non-macOS hosts report every
     source `Unsupported` with a reason.
4. **Scanner honesty repairs (Task 8):**
   - `SymlinkPolicy::FollowWithCycleGuard` was declared but ignored by
     the scanner (a silent downgrade to record-only). It is now
     explicitly REJECTED: the scan fails with a typed `Unsupported`
     error before the filesystem is touched (`is_implemented()` added;
     docs + 3 tests).
   - Scan-root links now get child-link semantics (honest target +
     broken flags) via a shared `build_link_entry`; a root link is
     recorded, never followed (2 tests).
   - `read_link_target` errors are typed, never silent `.ok()` "no
     target": the entry carries the real category and the error is
     tallied (2 tests). A link whose own metadata fails keeps its real
     category and makes no broken claims (1 test).
   - `cfg!(windows)` removed from the shared `SysDirs` impl; platform
     selection now lives in cfg-selected modules (the `drive_info()`
     pattern).
   - Linux mount-path decoding is now byte-exact (lossless): /proc/mounts
     is read as bytes and paths are built from raw bytes via
     `OsString::from_vec`; a non-UTF-8 fs-type keeps the entry with
     `fs_type: None` instead of mangling or dropping it. This repairs a
     real path-losslessness violation (U+FFFD could collapse two mounts
     onto one fabricated path). 4 unit tests, plus parse extraction.

## Verification (local, 2026-10-07, Windows 11 GNU toolchain, Rust 1.99.0)

- `cargo fmt --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  — clean.
- `cargo test --workspace` — **577 passed, 0 failed** (2 ignored perf
  suites run separately; was 515 before this phase → +62 new tests).
- Cross-target compile checks (all targets incl. tests):
  `cargo check -p coresight-capabilities -p coresight-macos -p
  coresight-engine --all-targets --target x86_64-apple-darwin` — green;
  same for `x86_64-unknown-linux-gnu` — green.

## Honest limitations

- The macOS runtime observation path (`observe_host_sources` + the
  macOS-gated host tests) is compile-checked for
  `x86_64-apple-darwin` here but NOT runtime-verified on a Mac in this
  phase: it requires the macOS CI job (`.github/workflows/ci.yml` picks
  up the new crates automatically via `--workspace`). Per the phase
  rules, no macOS capability is claimed verified beyond compilation.
- The unix-gated Linux unit tests (mount parsing, lossless decoding)
  compile-checked for the Linux target; runtime execution likewise
  awaits Linux/macOS CI.
- No capability produces real data yet beyond the existing Windows
  application-intelligence providers; every new contract is honestly
  `Planned`/`Partial`/`Deferred` in the pinned `CONTRACTS` table.
- The safety pipeline is a boundary, not an executor: nothing in this
  build executes any state-changing action.

## Next authorized work

- NOTHING is authorized until this phase passes independent
  verification and its CI run is recorded green here. Stop after this
  phase (per the phase contract).
