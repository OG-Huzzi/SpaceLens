# CoreSight — Current State

- **Current phase:** PHASE 6 — Application intelligence (foundation hardened)
- **Status:** Phase 6 foundation COMPLETE and second-order audited.
  Phase 5.1 REPAIRED and second-order audited. CI regression repaired
  (see CI section for the recorded run).
- **Last updated:** 2026-10-06.

## What happened (2026-10-04 → 2026-10-06)

1. **CI regression repaired.** The `coresight-apps` push failed Rust CI
   on all three platforms under current stable Clippy
   (`needless_borrow`, `new_without_default`, `manual_map`,
   `chunks_exact_to_as_chunks`, plus `cmp_owned` and
   `cloned_ref_to_slice_refs` surfaced under `--all-targets`). Fixed by
   idiomatic rewrites — no `allow(...)` suppressions, no behavior change.
   Hive-path splitting and REG_SZ/EXPAND_SZ decoding were extracted into
   platform-neutral helpers (`split_hive_path`, `decode_registry_string`)
   with 16 new tests that run on every CI platform.
2. **Phase 5.1 second-order audit.** Verified and hardened:
   - 128-bit identity: extreme values (`0`, `1`,
     `0x7fff_ffff_ffff_ffff`, `0x8000_0000_0000_0000`, `u64::MAX`)
     round-trip model → SQLite → reload exactly (new tests).
   - Event ids: the canonical event tuple now uses the LOSSLESS path
     encoding instead of `Path::display()` — distinct non-UTF-8 paths
     that display-collapse to the same U+FFFD spelling no longer
     collide on event id (regression test).
   - Corruption handling: persisted status/kind/config/roots/path
     decoders are strict. Unknown persisted values are typed errors, not
     silent defaults (previously an unknown status reloaded as
     `Running`, unknown kind as `File`, corrupt config as the CURRENT
     config, corrupt roots as an empty scope, malformed tagged paths as
     raw strings). Legacy untagged paths still load as-represented.
   - Schema safety: a store whose `schema_version` is newer than the
     build is refused (`StoreError::SchemaTooNew`) without modifying it.
   - Relationship completeness: migration v4 persists each run's
     relationship-report status/truncation; a reloaded run can no longer
     claim `Completed` when the derivation was partial, and a run with
     no recorded report reloads with none.
3. **Phase 6 second-order audit.** Verified and hardened:
   - Source coverage is now a five-state model
     (`COMPLETE`/`PARTIAL`/`UNSUPPORTED`/`FAILED`/`UNAVAILABLE`).
     An absent registry view is `Partial`/`Unavailable`, MSIX is
     `Unsupported`; "source unavailable" can never read as "nothing
     found".
   - Inventory merging is bounded and deterministic: canonical source
     order, first-applied-view preference (call-order independent),
     over-long names rejected (not truncated) with exact counting.
   - Footprint discovery is bounded on every axis (apps, children per
     root, evidence per candidate, total candidates) with canonical
     ordering BEFORE capping and exact truncation counters.
   - Identity: deterministic, version-independent,
     publisher/source-sensitive (tests for duplicate records, version
     upgrades, same-name/different-publisher, cross-source).
   - Shared-resource safety: name-coincidence evidence maps to at most
     `Possible` ownership; a shared directory matching no app is never
     claimed; only installer-recorded install locations are `Confirmed`.

## Verification (local, 2026-10-06)

- `cargo fmt --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  — clean (Rust 1.99.0, the CI toolchain generation).
- `cargo test --workspace` — green: 515 passed, 0 failed (2 ignored
  perf suites run separately).
- `cargo check -p coresight-apps` for Linux and macOS targets — green.
- New tests: 20 app-audit regressions + 16 registry-decode + 16
  history-audit regressions (extreme identities, lossless event ids,
  corruption rejection, schema refusal, relationship-status persistence).

## CI

`.github/workflows/ci.yml` runs the full gate on Windows/Linux/macOS plus
the frontend. `--workspace` includes `coresight-apps` and
`coresight-history` automatically.

**CI VERIFIED (2026-10-06): run 37483779232 for commit `e697fec` —
rust ubuntu SUCCESS, rust windows SUCCESS, rust macos SUCCESS,
frontend SUCCESS.**
https://github.com/OG-Huzzi/SpaceLens/actions/runs/37483779232
The two follow-up commits after it (`3d2db26` docs, `5a4b5be` error-text
privacy fix) each re-ran the full gate and are green as well — current
HEAD `5a4b5be`: run 37487037197, conclusion SUCCESS.

Repair sequence recorded: `20778a1` (gate + audit repairs) exposed two
pre-existing platform-dependent test-fixture bugs on Linux/macOS that
Clippy had previously gated (`db1b612` phase-6 fake-fs fixtures,
`d782e4e` registry-key separators, `e697fec` path-coverage fixture) —
all fixed as fixture corrections; product code unchanged in those
commits.

## Known limitations

- MSIX/AppX enumeration is abstracted but returns an explicit
  `Unsupported` error on Windows (not silently empty).
- Footprint discovery is name/evidence based; it does not yet read
  package manifests, shortcut targets, or process-write observations.
- Phase 6 does not persist application graph to SQLite yet, and does
  not yet feed observed entries from `coresight-history` snapshots.
- `Win32RegistryView` enumerates subkeys with a fixed 260-UTF-16-unit
  buffer: names longer than that are skipped and COUNTED, and an OS
  enumeration error marks the view's coverage `Partial` (never a silent
  clean end). Real machines do not produce such keys in the uninstall
  views, but the accounting is exact either way.
- No uninstaller behavior — foundation only.

## Next authorized work

- Nothing is authorized until the repair push's CI run is recorded green
  here. The next candidate milestone is application-intelligence
  persistence (inventory → SQLite) once CI is verified.
