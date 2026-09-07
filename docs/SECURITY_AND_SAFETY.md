# SpaceLens — Security & Safety Architecture

Status: Phase 0. Safety is a CORE PRODUCT REQUIREMENT, not a feature.
A storage manager bug can destroy irreplaceable data; this document is the
contract every later phase builds against.

## The pipeline (only legal path to removal)

```
SCAN → ANALYZE → CLASSIFY → RECOMMEND → USER REVIEW
→ CLEANUP PLAN → SAFETY VALIDATION → USER CONFIRMATION
→ QUARANTINE / TRASH → VERIFY RESULT
```

Rules:

- No code path may transition RECOMMEND → REMOVE without passing through
  PLAN → SAFETY VALIDATION → CONFIRMATION. Tests assert the absence of shortcuts.
- `safety` has veto power and zero dependencies on advisory modules: it cannot
  be persuaded, only obeyed. A plan failing validation is returned with reasons,
  never partially executed.
- Execution defaults to reversible: OS Trash / Recycle Bin, or app quarantine
  for items Trash cannot hold. Permanent deletion requires explicit, separate,
  twice-confirmed user intent and is never the default button.

## Protection domains (non-exhaustive; policy tables versioned in code)

- **System & boot:** OS install dirs, bootloaders, WinSxS/Installer, /System,
  kernel, drivers. Verdict: OFF-LIMITS, never listed as actionable.
- **User documents & irreplaceable data:** Documents/Photos/home libraries.
  Verdict: REVIEW-ONLY at most; duplicates inside them need per-file confirm.
- **Links & mounts:** symlinks, Windows junctions, hardlinks (collapse identity,
  never count or delete twice), mounted volumes, network drives (scan read-only,
  removal disabled v1), cloud placeholders (never hydrate, never delete the
  local stub blindly).
- **External drives:** removal actions scoped to the selected drive; remembered
  (offline) drives are read-only records — no action permitted while detached.
- **Permissions & integrity:** inaccessible files reported as unscanned and
  excluded from totals with the exclusion stated; no elevation surprises;
  hashing failures mark files unknown, never "identical."
- **Self-protection:** the app's own DB, quarantine store, and snapshots are
  off-limits to cleanup plans, including plans acting on parent directories.

## Preview contract (before every destructive action)

The user sees: exact item list (or exact rules for rule-based items), bytes to
be freed, what stays untouched, where items go (which Trash/quarantine), how to
undo, and the consequence in plain language. Confirm restates the consequence.
After execution, VERIFY re-stats the targets and reports freed bytes + any
leftovers. The operations log (append-only, in SQLite) records every action
for review and support.

## Privacy

- Offline by design. Scanning, hashing, classification never touch the network.
- Nothing leaves the machine: no paths, names, hashes, telemetry-by-default.
  Any future telemetry is opt-in, disclosed, and never includes file identities.
- DB file is local-only, permissions-restricted; corruption handling favors
  rescan-over-guess (docs/DATABASE.md).

## Test & fixture law

- Automated tests use **isolated synthetic fixtures only**. NEVER real home
  directories, NEVER personal data, NEVER deletion tests outside a temp fixture
  that the test itself creates and destroys.
- Adversarial suite (Phase 10): symlink loops, junction mazes, hardlink farms,
  permission walls, disappearing files mid-scan, full-disk-during-quarantine,
  Unicode/overlong paths, clock-skewed snapshots. Each has a fixture and an
  expected safety verdict.
- Any safety-test failure blocks release. No exceptions, no waivers.
