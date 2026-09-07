# SpaceLens — Testing Strategy

Status: Phase 0. Philosophy + required suites. Implemented progressively from
Phase 1; adversarial + perf gates harden by Phase 10.

## Philosophy

Testing is how we earn the right to touch user files. Coverage numbers are
secondary; the primary metric is: **every safety promise in
SECURITY_AND_SAFETY.md has at least one test that fails if the promise breaks.**

## Mandatory engineering loop (no exceptions)

```
IMPLEMENT → BUILD → TEST → FAIL? → DIAGNOSE → FIX → BUILD AGAIN
→ TEST AGAIN → (repeat until clean) → VERIFY → AUDIT → COMPLETE
```

There is NO: IMPLEMENT → "looks good" → COMPLETE.
"Implemented" ≠ "verified." Only green runs on the real toolchain count.

## Required suites

- **Rust unit tests** — per-module logic (classifier rules, delta math, policy
  tables, plan validation). Fast, hermetic, run on every commit.
- **Rust integration tests** — engine pipelines over generated fixture trees
  (deterministic generators, committed to repo): scan→classify→recommend→plan→
  validate→execute→verify end to end.
- **Filesystem fixture tests** — synthetic trees only (created + destroyed by the
  test in a temp dir): deep nesting, Unicode/long paths, hardlink farms, symlink
  loops, junction-style reparse (Windows), permission-denied dirs, files that
  change mid-scan. NEVER real home dirs, NEVER personal data.
- **Safety tests** — each protection domain gets veto tests: system paths,
  boot files, user-doc dirs, link/mount escapes, network/cloud stubs, offline
  drives, DB/quarantine self-protection. A plan touching these MUST be rejected
  with reasons. Any failure blocks merge AND release.
- **Platform tests** — shared trait-conformance suite run on Windows, macOS,
  Linux CI runners; OS-specific fixtures reviewed for verdict parity.
- **Frontend tests** — component tests for the 5 screens (lists, deltas,
  preview copy), contract tests against mocked IPC payloads, no-filesystem rule
  enforced (lint: no `fs` imports in product code).
- **Tauri integration tests** — command/event round-trips, cancel semantics,
  error-code rendering, version-mismatch rejection.
- **End-to-end tests** — scripted app runs against fixture volumes (Phase 9+):
  scan a generated tree, accept an opportunity, verify Trash + freed bytes.
- **Performance benchmarks** — Criterion benches on 100k/1M generated trees;
  CI perf-smoke budgets from Phase 1 (fail with profile on regression).
- **Adversarial tests** (Phase 10 gate) — full disk during quarantine, vanishing
  files mid-hash, clock skew, corrupt DB, hostile filenames, TOCTOU races on
  validate→execute. Each a named fixture with an expected safe outcome.

## Rules

- Tests are deterministic: seeded RNG, fixed clocks where time matters, no
  network, no wall-clock timeouts as pass criteria.
- Flaky tests are bugs: quarantine-and-file-issue within one working day,
  fix-or-delete within one cycle. No retries-as-policy.
- Every bug fix ships with the regression test that would have caught it.
- Verification honesty (see DEVELOPMENT_RULES.md): paste real command + real
  result. "Looks good" is not a result.
