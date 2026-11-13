# CoreSight — jv16 PowerTools Feature Parity Matrix

Status: Phase 4.1 (in progress). This file is the authoritative mapping between
the [jv16 PowerTools](https://jv16powertools.com/features) feature set (18 tools
+ 4 platform features, as published on the vendor's official features page) and
CoreSight's implementation.

**Design rule: parity of scope, not parity of approach.** CoreSight ships an
equivalent for every jv16 tool, but each one must be *provably* better on the
axis the user named: quality of the work done, not quantity of buttons. The
"quality bar" column is the acceptance criterion for that tool — a tool is not
done until the bar is met and tested.

## Status vocabulary

| Term | Meaning |
|---|---|
| **PLANNED** | Contract/scaffold only. Nothing executable exists. |
| **IMPLEMENTED** | Code exists and its own tests pass. |
| **VERIFIED** | Implemented + crossed a recorded gate with exact-SHA CI evidence. |

## The 18 tools

| # | jv16 tool | CoreSight equivalent | Status | Quality bar (what "better" means concretely) |
|---|---|---|---|---|
| 1 | System Fixer | Diagnostics engine (`coresight-systools::fixer`) | **IMPLEMENTED** | 9 evidence-only rules over caller-supplied facts. **A skipped check is not a passing check** — a rule whose observation is absent/failed/blank is counted in `skipped`, never reported as clean. No rule claims `repairable`; every repair step is `InspectOnly`. |
| 2 | System Cleaner | Cleanup planner (`coresight-cleanup` + `coresight-insights`) | **IMPLEMENTED** | Category-level opportunities with what/why/how-much/what-stays/consequence; Safe/Review/OffLimits tiers enforced by a vetoing validator; partial knowledge blocks safe-tier plans. |
| 3 | Software Uninstaller | `coresight-apps::uninstall` — leftover reconstruction + plan | **IMPLEMENTED (planning)** | Leftovers built from recorded evidence, never from what the app self-reports. Only a strict child of the recorded install root is `Safe`; a contended path is always `OffLimits`; a name-only match is `Review` and its `why` says so. `total_bytes` is `None` if any item's size is unknown. **This build removes nothing** — `plan_action` is always `InspectOnly`, and the vendor uninstaller is recorded, never claimed to have run. |
| 4 | Finder | `coresight-filetools::search` | **IMPLEMENTED** | Semantic, self-explaining search: every hit lists the criteria it actually satisfied. Unknown size/age never satisfies a bound in either mode. Name-only text matching (never a parent directory). |
| 5 | Startup Timer | Startup inventory + explanation (`coresight-systools`) | **IMPLEMENTED** | Each startup item explained: what it does, why it runs, what disabling it costs. Privileged sources are OffLimits, never offered. |
| 6 | System Examiner | Structured system report (`coresight-systools`) | **IMPLEMENTED** | Typed `Option` facts (unknown ≠ zero) + explicit source coverage so a partial report cannot read as complete. |
| 7 | Screenshots | Local screenshot + export | PLANNED | Local-only by default. No cloud upload of user content. |
| 8 | Update Fixer | `coresight-systools::fixer::update_findings` | **IMPLEMENTED** | `Unknown` update state is neither healthy nor broken — its own finding with an explicit "could not determine". An error without a reported code says the code was not reported rather than inventing one. |
| 9 | Big File Finder | `coresight-filetools::find_big_files` | **IMPLEMENTED** | Age evidence is required to call something "old"; unknown mtime is excluded and counted, never treated as old. |
| 10 | Duplicate Finder | `coresight-filetools::find_duplicates` | **IMPLEMENTED** | **Hash-verified only.** No hash ⇒ not a duplicate. Hard-link aliases collapse to one object (recoverable space 0). Never groups by name/size/time. |
| 11 | File Renamer | `coresight-filetools::plan_rename` | **IMPLEMENTED** | A full before/after plan, rejected on duplicate targets, duplicate sources, empty names, separators in names, and rename cycles. |
| 12 | File Splitter | `coresight-filetools::plan_split` | **IMPLEMENTED** | Plan states exact part count, offsets and sizes; the executor must checksum-verify. |
| 13 | File Merger | `coresight-filetools::plan_merge` | **IMPLEMENTED** | Rejects zero-size parts, duplicates, unordered parts, and `u64` overflow instead of saturating a wrong total. |
| 14 | File Deleter | Locked-file inspection + Trash move (`coresight-actions`) | PARTIAL (reversible moves IMPLEMENTED, permanent delete deliberately absent) | Protected/system paths are refused; the default is Trash, not unlink. `authorize()` refuses every action kind except `MoveToTrash`. |
| 15 | File Wiper | Secure wipe | PLANNED | Must verify the post-wipe read-back; SSD/TRIM reality documented, not glossed over. Blocked by the current execution policy. |
| 16 | Task Manager | Application task inventory (`coresight-systools`) | **IMPLEMENTED** | Apps vs processes is a modelled distinction; unattributable processes are counted, never dropped; two app ids sharing a display name are never merged. |
| 17 | Web Blocker | Block-list planner (`coresight-webtools`) | **IMPLEMENTED** | Strict domain validation; every rejected/duplicate/whitelisted entry is counted with a reason; reversible unblock plan. |
| 18 | Internet Optimizer | DNS candidate ranking (`coresight-webtools`) | **IMPLEMENTED** | Ranking explains its weighting (latency, reliability, DNSSEC, documented privacy) and states plainly that switching DNS changes who sees every lookup. Never auto-switches. |

## The 4 platform features

| jv16 feature | CoreSight equivalent | Status | Note |
|---|---|---|---|
| Widget | Ambient scan indicator | PLANNED | Fits the "scanning is ambient, never blocking" UX rule; no floating window sprawl. |
| Dark Mode | System-theme dark mode via CSS custom properties | **IMPLEMENTED** | `prefers-color-scheme` driven; colours are tokens, not scattered hex literals. |
| Multilingual UI | Contract-first strings | PLANNED | All user-facing strings live behind the `coresight.v1.*` contract, never hard-coded in components. |
| Fully configurable | Typed settings | PLANNED | Preferences are typed; hiding a tool is a view preference, not a code change. |

## Phase 8 UI status

The five screens exist (`src/screens/`): **Home, Storage, Cleanup, History,
Drives**, each with explicit empty / loading / error / partial states.

Invariants the UI enforces, and which a reviewer should re-check on every change:

| Invariant | Where it lives |
|---|---|
| Actions live in Cleanup only | No `<button>` in Home/Storage/History/Drives |
| Unknown ≠ zero | `src/lib/format.ts` renders `"Unknown"` for every `null`, never `0 B` |
| Off-limits categories are shown with a reason, not hidden | `CleanupScreen.tsx` off-limits panel |
| Partial scans say they are partial | Coverage banner fed by the API's `coverage` field |
| No filesystem logic in the UI | The UI displays paths; it never builds them |

The UI is backed by `createMockApi()` — a deterministic in-memory fixture.
**No live IPC exists yet**; Phase 9 replaces the mock with Tauri commands.

## Deliberate divergences (where CoreSight refuses to copy jv16)

These are not gaps — they are the reason to choose CoreSight.

1. **No "clean everything" button.** jv16's headline promise is one-click
   cleaning. CoreSight's thesis is that trust comes from knowing exactly what
   moves. Every action is per-opportunity and previewed.
2. **No registry mutation in the current build.** `ActionKind::ModifyRegistry`
   exists so it can be *classified and blocked*, not so it can run. Widening
   `ExecutionPolicy` is a separate, authorized change.
3. **No permanent deletion path.** `ActionKind::Delete` and `ActionKind::Wipe`
   exist for classification only. Until a phase explicitly authorizes them,
   every execution path ends in Trash/quarantine.
4. **No cloud/LLM analysis.** jv16 is Windows-local; CoreSight goes further and
   guarantees nothing about a file tree ever leaves the machine.
5. **No "speed up my PC" claims.** Diagnostics are descriptive. CoreSight does
   not assert that a change will make the machine faster unless it can be
   measured and shown.

## Execution-layer safety model

All destructive intent crosses the same three gates, in order:

```
coresight-cleanup::plan_cleanup      → proposal with reasoning
coresight-cleanup::validate_plan     → veto (off-limits / partial / preview / bytes / gate)
coresight-actions::authorize         → typed token, only from an allowed verdict
coresight-actions::execute           → reversible move + rollback on any failure
coresight-actions::verify            → re-stat what moved; report honestly
```

`AuthorizedPlan` has private fields and exactly one constructor, so no caller
can manufacture permission. `execute` additionally requires a
`ConfirmedPreview` whose plan id and byte total match — the user having seen
the preview is a type-level requirement, not a convention.

## Change record

| Date | Change | Reason |
|---|---|---|
| 2026-11-10 | File created; jv16 scope enumerated from the vendor's official features page. | User directed full jv16 scope at higher quality. |
