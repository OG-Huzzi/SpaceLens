# CoreSight — Current State

- **Current phase:** PHASE 6 — Application intelligence (foundation complete)
- **Status:** Phase 6 foundation COMPLETE (domain, discovery, footprint,
  evidence, relationships, explain; 23 tests). Phase 5.1 REPAIRED.
- **Last updated:** 2026-10-04.

## What happened (2026-09-12 → 2026-10-04)

1. Phase 5.1 repair (audit findings): full 128-bit Windows file identity
   preserved through history (`file_id_hi` end-to-end, migration v3);
   `Modified` now requires same-object proof + two verified differing
   hashes; alias/hard-link path deletion reported at path level while
   object survival is reported independently; move/rename pairing only
   when 1:1 provable, otherwise ambiguous conservatively reported;
   lossless tagged path persistence (`path_encoding.rs`) with legacy
   lossy rows preserved as legacy; Windows component-wise case-insensitive
   scope comparison. 25 regression tests green.
2. Product rename SpaceLens → CoreSight across crates, workspace,
   Tauri config (`com.coresight.app`), frontend, docs, and CI.
3. Phase 6 foundation: new `coresight-apps` crate — platform-neutral
   application domain model (id, identity, record, source, kind,
   coverage, limits), provider-based discovery with Windows Win32
   uninstall registry implementation (HKLM-64/HKLM-32/HKCU views via
   `windows-sys`, no extra deps), MSIX/AppX provider trait with honest
   `Unsupported` error, evidence-typed footprint candidates with
   confidence, ownership strength levels, human-readable explanations.
   23 tests green including real-registry determinism and duplicate
   merging.

## Verification

- `cargo test --workspace` — green (all crates, incl. 25 Phase 5.1
  regressions and 23 Phase 6 tests).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  — clean.
- `cargo fmt --check` — clean.

## CI

`.github/workflows/ci.yml` runs the full gate on Windows/Linux/macOS.
No CI run has been recorded for Phase 6 yet — record it in the next
push (honest note: local verification only until CI proves otherwise).

## Known limitations

- MSIX/AppX enumeration is abstracted but returns an explicit
  `Unsupported` error on Windows (not silently empty).
- Footprint discovery is name/evidence based; it does not yet read
  package manifests, shortcut targets, or process-write observations.
- Phase 6 does not persist application graph to SQLite yet, and does
  not yet feed observed entries from `coresight-history` snapshots.
- No uninstaller behavior — foundation only.
