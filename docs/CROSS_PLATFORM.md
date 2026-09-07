# SpaceLens — Cross-Platform Strategy

Status: Phase 0. Rule: shared by default, isolated by trait. Never a Windows-only
core with ports bolted on later.

## Principle

All product logic (walk orchestration, classification, recommendations, safety
policy *structure*) is platform-agnostic Rust. All OS *behavior* lives behind
four traits implemented per OS: `PlatformFs`, `Trash`, `DriveInfo`, `SysDirs`.
Shared code never branches on `cfg!(target_os)` for behavior — only trait impls do.

## Shared (one implementation)

Scanner orchestration, metadata model, classifier rules engine, hashing +
duplicate grouping, recommender, planner, safety validator core, snapshot/delta
math, SQLite schema + migrations, IPC command/event shapes.

## Per-OS implementations

### Windows
- Filesystems: NTFS (primary; MFT-aware fast path is a Phase-10 optimization,
  NOT v1 behavior — v1 walks the API like every other OS for correctness),
  ReFS/exFAT/FAT32 correctness.
- Deletion path: Recycle Bin via shell API; long-path (`\\?\`) handling; ACLs
  and elevation boundaries (report unscanned, never silently skip).
- Reparse points: junctions, symlinks, mount points — never followed blindly;
  mounted volumes attributed to their volume, not the parent.
- System knowledge: `C:\Windows`, boot files, `Program Files`, per-user
  `AppData` (Local/LocalLow/Roaming) classification, WinSxS/Installer
  off-limits rules, Windows Apps containers.
- Cloud placeholders: OneDrive/Dropbox on-demand files detected and never
  force-hydrated by scan or hash.

### macOS
- APFS: clones/sparse files reported honestly (logical vs physical); firmlinks
  (System/Data volume split) handled; Time Machine snapshots listed, never
  touched; purgeable space reported as the OS sees it.
- Deletion path: Trash (`~/.Trash`, per-volume `.Trashes`); SIP-protected
  locations off-limits; TCC permissions (Full Disk Access) requested once with
  a plain-language explanation; denied areas labeled unscanned.
- System knowledge: `/System`, `/Library`, `~/Library` (Application Support,
  Containers, Caches, Group Containers) classification; app bundles + leftovers
  attribution; Xcode/derived-data rules.

### Linux
- Filesystems: ext4, Btrfs (subvolumes, snapshots, reflinks — shared extents
  must not double-count), XFS; others degrade gracefully to generic walk.
- Deletion path: freedesktop Trash spec (`~/.local/share/Trash`, topdir
  `.Trash-$uid`); permission-denied areas reported, never escalated silently.
- Packaging sprawl: Snap (`/snap`, `~/snap`), Flatpak
  (`~/.local/share/flatpak`, `/var/lib/flatpak`), AppImage locations
  classified as Applications with their data attributed.
- Distro variance: no dependency on a specific distro layout; XDG dirs resolved
  at runtime; `/proc /sys /dev /run` excluded by policy, not by hope.

## Conformance mechanism (from Phase 1)

- `platform-matrix` test suite: every trait gets the same behavioral tests on
  all three OSes in CI (GitHub Actions runners).
- Fixture trees simulating junctions/symlinks/firmlinks/snapshots run on all
  platforms; OS-specific fixtures live in per-OS dirs and are reviewed for
  policy parity (same danger → same verdict everywhere).
- Any `cfg!(target_os)` outside a trait impl fails code review by rule
  (docs/DEVELOPMENT_RULES.md).

## Phase 0 honesty note

This machine is Windows 11. macOS/Linux impls are contract-defined here and
must be implemented and CI-tested by owners on those platforms (or CI runners)
in Phase 1+. No platform-specific behavior is claimed as verified in Phase 0.
