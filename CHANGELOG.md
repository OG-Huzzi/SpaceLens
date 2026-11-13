# Changelog

All notable changes to CoreSight are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.1.0] - 2026-11-13

The first versioned build. The product code is what previously existed under
`0.0.0`; this release gives the tree a real version, a changelog and a release
runbook. It is **not a public release**: the installers are unsigned and no
installed build has been launched.

### Added

- **Scanning.** A real filesystem walk with category classification, partial
  coverage reporting (unreadable directories are counted, never hidden), and a
  content-hash identity pass over same-size candidates.
- **Cleanup planning.** Category-level opportunities carrying evidence,
  expected savings, what stays, and Safe / Review / Off-limits tiers enforced by
  a vetoing validator.
- **Duplicate and big-file analysis.** A duplicate requires a verified hash;
  unknown age never counts as old.
- **Uninstall planning.** Leftover reconstruction from recorded evidence. It
  inspects and plans only; it removes nothing.
- **Web and privacy planning.** Domain block-list and DNS-ranking plans that
  count every rejected, duplicate and whitelisted entry with a reason. Nothing
  writes the hosts file or changes DNS.
- **18 jv16-class tool areas** with code and tests in the tree (see
  `docs/JV16_PARITY.md`). Two are planning/policy-only and remove nothing:
  File Wiper (`coresight-cleanup::wipe`) and Screenshots (`coresight-capture`).
  File Deleter is reversible-Trash only; permanent deletion is deliberately
  absent.
- **Five-screen UI** — Home, Storage, Cleanup, History, Drives — plus Settings,
  with explicit empty, loading, error and partial states. A `null` value renders
  as "Unknown", never as `0`.
- **Localisation.** English and German catalogues with compile-time key parity.
  Engine-supplied data (category names, opportunity text, paths, hashes) stays
  English by design.
- **Release engineering.** `CHANGELOG.md`, `docs/RELEASE.md`, and a macOS
  universal (arm64 + x86_64) bundle in `.github/workflows/bundle.yml`.
- **Unsigned installers.** CI builds Windows NSIS/MSI installers and a macOS
  universal DMG. Every artifact is named `-unsigned` and carries an
  `UNSIGNED-BUILD.txt`; none is code-signed or notarised.

### Changed

- Version `0.0.0` → `0.1.0` in `package.json`, `src-tauri/tauri.conf.json` and
  `src-tauri/Cargo.toml`. The internal engine crates remain `0.0.0`; they are
  unpublished workspace libraries, and changing them is not part of the app
  version.

### Security

- Reversible Trash/quarantine execution exists (`coresight-actions`) but is
  gated behind the read-only execution policy: `confirmPlan` and `executePlan`
  return `PLAN_REJECTED_UNSAFE` in this build. Nothing is deleted, wiped,
  renamed, uninstalled, or written to the registry or hosts file.
- The Screenshots policy crate has no network surface; upload is impossible
  rather than disabled.
- The Tauri content-security policy is unchanged:
  `default-src 'self'; img-src 'self' asset: https://asset.localhost`.

### Known limitations

- **Unsigned, not notarised, never launched.** Windows installers are not
  Authenticode-signed (SmartScreen and an "Unknown publisher" UAC prompt);
  macOS bundles are not Developer ID-signed or notarised (Gatekeeper refuses the
  first launch). See `docs/PACKAGING.md`.
- **No auto-update.** No updater plugin, update signing key or update endpoint
  is configured. The app cannot check for or install updates.
- **No history store and only partial live data.** `getHistory` reports
  `HISTORY_UNAVAILABLE`; startup, DNS, block-list and privacy summaries report
  `SESSION_UNAVAILABLE` when no inventory is attached. The duplicate report is
  honestly partial because only same-size candidates are hashed.
- **No installer has been launched.** The first green `Bundle (unsigned)` run
  and an installed launch are still required before publishing anything.

[Unreleased]: https://github.com/OG-Huzzi/SpaceLens/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/OG-Huzzi/SpaceLens/releases/tag/v0.1.0
