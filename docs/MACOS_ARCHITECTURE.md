# CoreSight — Mac-First Capability Architecture

Status: Phase 6.1. Defines the product's capability boundaries, the honest
state models, the safety pipeline, and the macOS discovery catalog as
implemented in `coresight-capabilities` (shared, platform-neutral) and
`coresight-macos` (the macOS discovery boundary).

## What CoreSight is now

CoreSight is a **macOS system intelligence + power-tools application**.
It is NOT a "Mac cleaner". Storage cleanup is one capability of one pillar.
The product understands the Mac: it observes the system, explains what it
finds, connects evidence across apps, files, storage and system state,
remembers historical state, recommends safe actions, and — eventually,
under a safety gate — executes reversible actions.

```
CoreSight — macOS system intelligence + power tools
├── Storage Intelligence          scanner/classifier/identity/history (implemented)
├── Application Intelligence      coresight-apps (Windows providers; macOS planned)
├── System Intelligence           coresight-capabilities: contracts; coresight-macos: discovery
├── Privacy / Housekeeping        contracts only — actions require separate authorization
├── Performance / Diagnostics     DiagnosticSignal contract (no producer yet)
├── Software Management           contracts only — no execution anywhere
└── History / Forensics           coresight-history (implemented, audited in 5.1)
```

## Layer boundaries (shared core vs macOS implementation)

| Layer | Crate(s) | Contains | Must NEVER contain |
|---|---|---|---|
| Shared core | `coresight-capabilities`, `coresight-engine`, `coresight-classifier`, `coresight-identity`, `coresight-history`, `coresight-apps` | Deterministic logic, models, policy, evidence, safety, history, query/analysis, cross-platform contracts | macOS/Windows/Linux API details; `cfg!(target_os)`/`#[cfg(target_os)]`-style OS branching in shared logic (enforced by a source-scan test) |
| macOS implementation | `coresight-macos` | macOS discovery source catalog, permission-aware observation, APFS/launchd/login-item knowledge as it lands | Shared deterministic logic; Windows/Linux behavior |
| Windows / Linux | existing crates behind `coresight-engine` platform traits | Per-OS trait impls (`PlatformFs`, `DriveInfo`, `SysDirs`) | macOS knowledge |

Dependency direction: `coresight-macos` → `coresight-capabilities` →
(nothing internal). The shared layer never depends on a platform
implementation.

## Honest state models

Two complementary vocabularies; both are part of the type, not conventions.

### `Observation<T>` — result provenance (Task 3)

Every capability result distinguishes: **observed / inferred / unsupported /
unavailable / failed**. The state is part of the enum, so an unsupported,
unavailable, or failed observation structurally cannot carry a payload —
unavailable data can never masquerade as "nothing found". An `Observed`
payload may be an empty collection; that is a fact, and it is a different
fact from "we could not look".

### `AccessState` — path-access truth (Task 6)

Every path-access fact distinguishes: **ReadSucceeded / Empty /
DoesNotExist / ExistsButInaccessible / NotApplicable / Unsupported /
Failed**. Permission denial is NEVER collapsed into an empty result:

- `Empty` = a successful read that found nothing (a fact about contents);
- `ExistsButInaccessible` = existence PROVEN (metadata succeeded), read denied;
- `Failed` = the attempt errored and even existence may be unproven;
- `Unsupported` = this build cannot service the source at all.

On macOS, metadata-denied paths report `Failed` (existence unproven) rather
than overclaiming `ExistsButInaccessible`. No privilege escalation, no
security circumvention, ever: a denied location stays denied and is
reported as such.

## Capability contracts (Task 3, A–H plus two)

Each contract has a stable id, a pillar, and a pinned status
(`Implemented` / `Partial` / `Planned` / `Deferred`) in
`coresight-capabilities::capability::CONTRACTS`. The registry is a
truthfulness contract: overclaiming is a visible, failing diff.

| Contract | Pillar | Status (pinned) | Payload home |
|---|---|---|---|
| A `application-inventory` | Applications | Partial (Windows providers; macOS planned) | `coresight-apps::Inventory` |
| B `application-footprint` | Applications | Partial (Windows evidence; macOS kinds planned) | `coresight-apps::FootprintReport` |
| C `startup-items` | System | Planned — model only (`StartupItem`) | `coresight-capabilities::startup` |
| D `launch-agents` | System | Planned — catalog classifies the launchd dirs | future provider |
| E `volume-system-inventory` | System | Partial (Windows full; Linux /proc/mounts; macOS root-only) | `coresight-engine::platform::VolumeInfo` |
| F `diagnostics` | Performance | Planned — model only (`DiagnosticSignal`) | `coresight-capabilities::diagnostics` |
| G `storage-analysis` | Storage | Partial (APFS-specific facts pending) | engine/classifier/identity/history |
| H `historical-observations` | History | Implemented (Phase 5/5.1, audited) | `coresight-history` |
| `privacy-housekeeping` | Privacy | **Deferred** — any action needs separate authorization | — |
| `software-management` | Software Management | Planned — no uninstall/cleanup execution exists | `coresight-apps` relationships |

Every report flows through `CapabilityReport<T>`: one `Observation<T>` per
source, with exact per-state coverage. An empty report is NOT complete
("not attempted" ≠ "observed and found none"), and an unavailable source
blocks completeness.

## macOS discovery boundary and source catalog (Task 4)

`coresight-macos::catalog::SOURCES` classifies every candidate source along
four dimensions: read access, sensitivity, modification risk (for FUTURE
phases — nothing is ever modified in this build), and phase availability.

| Source | Location | Read | Sensitivity | If modified | This phase |
|---|---|---|---|---|---|
| applications-dir | `/Applications` | ReadableNow | Public | Privileged+destructive | **Probed** (listing) |
| user-applications | `~/Applications` | ReadableNow | Public | Destructive | **Probed** (listing) |
| user-app-support | `~/Library/Application Support` | ReadableNow | Sensitive | Destructive | **Probed** (listing) |
| user-caches | `~/Library/Caches` | ReadableNow | Sensitive | Recoverable | **Probed** (listing) |
| user-logs | `~/Library/Logs` | ReadableNow | Sensitive | Recoverable | **Probed** (listing) |
| user-containers | `~/Library/Containers` | ReadableNow | Privacy-sensitive | Destructive | **Probed** (listing) |
| user-group-containers | `~/Library/Group Containers` | ReadableNow | Privacy-sensitive | Destructive | **Probed** (listing) |
| user-preferences | `~/Library/Preferences` | ReadableNow | Sensitive | Destructive | **Probed** (listing) |
| user-launch-agents | `~/Library/LaunchAgents` | ReadableNow | Sensitive | Destructive | **Probed** (listing) |
| system-launch-agents | `/Library/LaunchAgents` | ReadableNow | Sensitive | Privileged+destructive | **Probed** (listing) |
| system-launch-daemons | `/Library/LaunchDaemons` | ReadableNow | Sensitive | Privileged+destructive | **Probed** (listing) |
| login-items | BTM/login-item records | UnsupportedForNow | Sensitive | Destructive | Deferred (needs OS API) |
| tcc-protected-user-data | Mail/Messages/Safari/... | **RequiresFullDiskAccess** | Privacy-sensitive | Destructive | Deferred — never probed without explicit user grant |
| mounted-volumes | `/Volumes` | ReadableNow | Public | Destructive | **Probed** (listing) |
| apfs-volume-info | APFS container/snapshot metadata | UnsupportedForNow | Public | Privileged+destructive | Deferred (no std API; no subprocesses) |

Rules of the catalog: only permission-free sources are probed; probed
means a bounded directory listing (`ProbeLimits`, default 4096 children,
truncation flagged exactly); no content reads, no recursion, no writes;
`Deferred` sources report `Unsupported` with the catalog's reason — never
an empty result.

## Safety action model (Task 5)

`coresight-capabilities::safety` defines the pipeline every future
capability action must flow through:

```
OBSERVE → ANALYZE → RECOMMEND → PREVIEW → VALIDATE → EXECUTE → VERIFY → ROLLBACK
```

- Actions are **explicitly classified**: exactly one effect
  (read-only / reversible / destructive) plus optional qualifiers
  (privileged / permission-sensitive). `ProposedAction::new` requires the
  classification; there is no unclassified action.
- `ActionPipeline` accepts **only the next stage** — skipping is a typed
  error. VALIDATE is gate-only, and EXECUTE requires an ALLOWED verdict.
- The `SafetyGate` is the veto point: it depends on nothing but the
  classification and `ExecutionPolicy::CURRENT_BUILD` (read-only only), so
  it cannot be persuaded by a caller. A blocked verdict permanently bars
  EXECUTE for that action.
- In this build **no action other than read-only can reach EXECUTE** — no
  destructive, reversible, privileged, or permission-sensitive execution
  exists anywhere. Executors are a separately authorized phase.

## What this phase deliberately does NOT build

No automatic cleaning, deletion, uninstall execution, startup disabling,
LaunchAgent disabling, process killing, privacy wiping, registry-like
hacks, "RAM cleaners", "speed boosters", system modifications, privileged
operations, frontend screens, licensing, payments, or cloud sync. Those
require their own separately authorized phases (docs/SECURITY_AND_SAFETY.md,
progress/PHASES.md).

## Verification notes

- Contract, state-model, catalog, and pipeline tests run on EVERY platform
  (synthetic fixtures only — never real user data, never a developer's Mac).
- Real-host catalog observation is verified by `coresight-macos` tests
  gated to macOS CI; on other hosts the crate reports every source
  `Unsupported` (tested on Windows/Linux).
- "Implemented" claims in this document are backed by the pinned status
  tests and the recorded runs in `progress/CURRENT_PHASE.md`.
