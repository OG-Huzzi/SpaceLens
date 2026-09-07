# SpaceLens — Multi-AI Development Rules

Status: Phase 0. Binding on every agent (human or AI) touching this repo.
Violations are reverted on sight.

## The 15 rules

1. **Inspect before modifying.** Read the repo, the relevant docs, and the file
   under change before editing. Never assume contents.
2. **Never trust another agent's claims.** Re-run the build/tests yourself and
   read the output. "Agent X said it passes" is hearsay, not verification.
3. **The repository is the source of truth.** Docs describe intent; code + green
   runs describe reality. When they disagree, fix the code or the docs — and say
   which you changed.
4. **Do not fake verification.** Only commands you actually executed, with output
   you actually observed, count. Fabricated logs are a project-ending offense.
5. **Do not hide failures.** Report red output verbatim (trimmed, not rewritten),
   then fix it. A hidden failure invalidates everything downstream.
6. **Do not silently skip requirements.** If a requirement can't be met, say so
   in `progress/PHASE_0_STATUS.md` (or current phase file) with reason + owner.
7. **Do not perform unnecessary rewrites.** Smallest diff that satisfies the
   contract. No drive-by refactors, no stack swaps without an approved RFC.
8. **Preserve established contracts.** `docs/API_CONTRACTS.md`, DB schema,
   safety pipeline, and module boundaries change only via explicit contract
   update + migration note + reviewer sign-off.
9. **Build after implementation.** No "done" without a real build on the real
   toolchain.
10. **Test after building.** New behavior ships with tests; run the affected
    suites, not just the new test.
11. **Fix failures.** Red → diagnose → fix. No forward motion on a red baseline.
12. **Test again after fixes.** The full affected suites re-run green after every
    fix, not just the previously failing case.
13. **Perform independent verification.** Re-read your own diff as a hostile
    reviewer: check contract drift, safety bypasses, fixture honesty (no real
    user paths), and copy that overpromises.
14. **Update project state.** `progress/CURRENT_PHASE.md` + phase status file
    reflect reality after every work unit: what changed, what ran, what passed.
15. **Do not start future phases without authorization.** Finish, verify, and
    stop. Phase gates are in `progress/PHASES.md`.

## IMPLEMENTED vs VERIFIED

- **IMPLEMENTED** = code exists on disk. Means nothing about quality.
- **VERIFIED** = built + tested on the real toolchain with observed green output
  recorded (command, result, date, machine). Only VERIFIED work counts toward
  phase gates.

## Working agreements

- One work unit = one small diff + build + tests + progress update.
- Prefer `patch` over rewrites; prefer fixture tests over mocks for fs behavior.
- Ask (via handoff notes) when blocked; never invent requirements to stay busy.
- Leave the tree green and the docs truthful. The next agent inherits your
  honesty, not your optimism.
