# SpaceLens — Differentiation Analysis

Status: Phase 0. Each candidate concept scored on user value, differentiation,
feasibility, performance, safety, and commercial value. Verdicts bind Phase 1+
scoping (revisit only with new evidence).

## Verdicts (highest priority first)

### P0 — must define the product

1. **B. Storage intelligence (explain, don't expose).** The core thesis.
   Human categories first, paths on demand. High value + high differentiation
   (nobody does it well) + safe (read-only presentation) + marketable.
   Risk: rule quality — bad categories are worse than raw paths. Mitigation:
   versioned rule tables with fixture tests from Phase 2.
2. **C+D. Cleanup explanations + preview.** WHAT/WHY/HOW-MUCH/WHAT-STAYS/
   WHAT-HAPPENS per recommendation, with exact preview before action. This is
   where fear converts to trust converts to purchase. Feasibility high (pure UI
   over existing plan data); safety high (preview is inherently read-only).
3. **H. Safe cleanup (reversible by default).** Trash/quarantine-first,
   verify-after. Non-negotiable foundation rather than a feature — without it,
   C+D are just prettier ways to be afraid. Moderate complexity (per-OS trash
   adapters), very high commercial value (the reason to pay vs free tools).

### P1 — the moat after the core works

4. **A+E. Storage forensics + history.** "Why +80 GB?" via snapshot deltas.
   No consumer competitor owns this. Feasibility good (snapshots fall out of
   the DB design); perf bounded (rollups, not full re-walks). Ship in Phase 5,
   market from day one of Pro.
5. **G. Context-aware categories (games/dev/media/apps/...).** The visible face
   of B. Needs per-OS knowledge tables (Steam, Xcode, Snap/Flatpak…) — Phase 6
   work, but the classifier interface must anticipate it in Phase 2.

### P2 — valuable, deferred, scoped carefully

6. **F. Drive memory.** Remember external drives + their indexes. Real value for
   photographers/creators with shelves of disks; risk is stale-data confusion.
   Rule: offline drives are read-only records, never action targets. Phase 5+.
7. Duplicate detection (supporting B/C): ship ONLY hash-verified (SHA-256) with
   hardlink collapse and cache. A wrong "duplicate" deletion is the nightmare
   scenario — this is safety-critical code wearing a convenience feature's clothes.

## Explicitly rejected / out of v1

- One-click "clean everything" (contradicts the trust story; enables catastrophe).
- Background auto-cleaning daemons (unpredictable, untrustworthy, support sink).
- Cloud/LLM-powered analysis of file trees (contradicts privacy-first; offline
  rules get us 90% of the value with 0% of the exposure).
- "Optimizer/booster" features (registry, RAM) — snake oil adjacent; brand poison.

## Bottom line

Build order: explain (B+G) → recommend with reasons (C) → preview (D) → act
reversibly (H) → remember and compare (E+A+F). Each layer is independently
shippable and independently testable; no layer is allowed to weaken the safety
rules of the one below it.
