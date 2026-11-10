/**
 * CoreSight Phase 8 UI contract.
 *
 * This module is the only place the UI learns about stored data. It mirrors the
 * camelCase IPC shapes described in `docs/API_CONTRACTS.md` and the Rust
 * contract crates, but it adds nothing the engine does not own.
 *
 * HONESTY INVARIANT — unknown is not zero
 * ---------------------------------------
 * Numeric fields that can be unobserved are `number | null`. A `null` value
 * means "this build does not know", never `0`, never an empty set, and never a
 * safe default. Components must render null as "Unknown" (or an equivalent
 * honest phrase). Byte totals are lower bounds when their `DataCoverage` is
 * partial. Timestamps, counts, ages, shares, and scores follow the same rule.
 *
 * The UI never joins, normalises, or resolves filesystem paths. Any path-like
 * value in this contract is a display label returned by the engine; it is shown
 * verbatim at the deepest disclosure layer and is never used for I/O.
 */

export const CONTRACT_VERSION = "v1" as const;

/** The contract major this shell was compiled against. Minor additions are OK. */
export const SHELL_CONTRACT_MAJOR = 1 as const;

export type DataCompleteness = "complete" | "partial" | "unknown";

/**
 * Source-quality metadata attached to every aggregate that could be partial.
 * `unreadableEntries: null` means the count itself is unknown; it must not be
 * rendered as "0 unreadable".
 */
export interface DataCoverage {
  completeness: DataCompleteness;
  unreadableEntries: number | null;
  unknownSizeEntries: number | null;
  /** Human labels only. Never filesystem paths. */
  excludedAreas: string[];
  note: string | null;
}

export interface ApiError {
  code: string;
  message: string;
  detail?: string;
}

export interface ContractInfo {
  contractVersion: string;
}

export function isContractCompatible(version: string): boolean {
  const match = /^v(\d+)(?:\.\d+){0,2}$/.exec(version);
  if (match === null) return false;
  return Number(match[1]) === SHELL_CONTRACT_MAJOR;
}

export function isApiError(value: unknown): value is ApiError {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { code?: unknown; message?: unknown };
  return typeof candidate.code === "string" && typeof candidate.message === "string";
}

// ---------------------------------------------------------------------------
// Shared safety / navigation vocabulary
// ---------------------------------------------------------------------------

export type SafetyTier = "SAFE" | "REVIEW" | "OFF_LIMITS";

export type DriveKind = "internal" | "external";
export type DriveConnection = "connected" | "remembered";
export type DriveHealth = "healthy" | "attention" | "unknown";
export type ScanState = "idle" | "scanning" | "complete" | "partial" | "failed";

export interface ScanStatus {
  scanId: string | null;
  state: ScanState;
  startedAt: string | null;
  completedAt: string | null;
  filesSeen: number | null;
  bytesSeen: number | null;
  currentArea: string | null;
  etaSeconds: number | null;
  coverage: DataCoverage;
}

export interface GlobalStatus extends ContractInfo {
  scan: ScanStatus;
  /** True while this build is rendering deterministic fixture data. */
  simulatedData: boolean;
}

// ---------------------------------------------------------------------------
// Categories and storage
// ---------------------------------------------------------------------------

/**
 * Mirrors `coresight.v1.Category`. The original Phase 0 shape is preserved;
 * byte-derived fields are nullable because an observation can be incomplete.
 */
export interface Category {
  id: string;
  name: string;
  bytes: number | null;
  shareOfParent: number | null;
  deltaSinceLast: number | null;
  itemCount: number | null;
}

export interface StorageLocation {
  label: string;
  /** Engine-provided display text, shown only inside a disclosure. */
  displayPath: string;
  bytes: number | null;
}

/** A category plus the evidence needed for progressive disclosure. */
export interface CategoryTotals extends Category {
  children: CategoryTotals[];
  entriesWithoutSize: number | null;
  observationErrors: number | null;
  sampleLocations: StorageLocation[];
}

export interface DuplicateMember {
  /** Display label from the engine. It is never opened, joined, or resolved. */
  displayPath: string;
  sizeBytes: number | null;
  isHardlinkAlias: boolean;
}

/**
 * A duplicate claim is only valid when content identity was verified. The
 * literal `true` makes it impossible to construct an unverified group in the
 * UI's type surface.
 */
export interface DuplicateGroup {
  id: string;
  contentSha256: string;
  hashVerified: true;
  sizeBytes: number | null;
  members: DuplicateMember[];
  /** `null` means the engine could not account for savings; it is not zero. */
  recoverableBytes: number | null;
}

export interface DuplicateReport {
  groups: DuplicateGroup[];
  totalGroups: number | null;
  totalRecoverableBytes: number | null;
  unverifiedEntries: number | null;
  unknownSizeEntries: number | null;
  truncatedGroups: number | null;
  coverage: DataCoverage;
}

export interface BigFile {
  id: string;
  displayPath: string;
  sizeBytes: number | null;
  modifiedAt: string | null;
  /** Null when the age is unknown; unknown age is never treated as old. */
  ageDays: number | null;
  categoryName: string | null;
}

export interface BigFileReport {
  files: BigFile[];
  thresholdBytes: number | null;
  unknownAgeEntries: number | null;
  truncated: number | null;
  coverage: DataCoverage;
}

export interface StorageData {
  categories: CategoryTotals[];
  totalObservedBytes: number | null;
  totalObservedItems: number | null;
  duplicateReport: DuplicateReport;
  bigFileReport: BigFileReport;
  coverage: DataCoverage;
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

export interface CleanupEvidence {
  kind: "observation" | "verification" | "safety";
  label: string;
  detail: string;
}

/**
 * A cleanup opportunity shown to the user. Off-limits entries use the same
 * type but are never previewable or actionable.
 */
export interface CleanupItem {
  id: string;
  categoryId: string;
  tier: SafetyTier;
  title: string;
  whatItIs: string;
  why: string;
  observedBytes: number | null;
  recoverableBytes: number | null;
  itemCount: number | null;
  /** Null is honest: age evidence was unavailable or was not needed. */
  estimatedAgeDays: number | null;
  whatStaysUntouched: string;
  consequence: string;
  evidence: CleanupEvidence[];
  previewable: boolean;
  offLimitsReason: string | null;
}

/** The earlier Phase 0 advisory shape, retained for consumers of this module. */
export interface Opportunity {
  id: string;
  tier: SafetyTier;
  title: string;
  whatItIs: string;
  whyRecommended: string;
  bytes: number | null;
  recoverableBytes: number | null;
  staysUntouched: string;
  consequence: string;
}

export interface CleanupData {
  scanId: string | null;
  lastScanAt: string | null;
  items: CleanupItem[];
  offLimits: CleanupItem[];
  safeRecoverableBytes: number | null;
  reviewRecoverableBytes: number | null;
  coverage: DataCoverage;
}

export interface PreviewItem {
  title: string;
  bytes: number | null;
  itemCount: number | null;
  destination: string | null;
  undo: string | null;
  reason: string;
}

/**
 * The exact statement the user must review before confirmation. `complete` is
 * false when any destination, undo route, or byte amount is unknown.
 */
export interface CleanupPreview {
  planId: string;
  opportunityIds: string[];
  items: PreviewItem[];
  totalBytes: number | null;
  whatStaysUntouched: string[];
  destination: string | null;
  howToUndo: string | null;
  consequence: string;
  complete: boolean;
  warnings: string[];
  coverage: DataCoverage;
}

/** The UI can only construct a confirmation after the user has acknowledged. */
export interface CleanupConfirmation {
  planId: string;
  confirmed: true;
  acknowledgedWarnings: string[];
}

export interface CleanupExecution {
  planId: string;
  accepted: boolean;
  simulated: boolean;
  movedItems: number | null;
  freedBytes: number | null;
  receipts: string[];
  leftovers: string[];
  verification: "verified" | "partial" | "simulated";
  note: string;
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

export type SnapshotTrigger = "first-run" | "manual" | "scheduled";

export interface HistorySnapshot {
  id: string;
  takenAt: string | null;
  trigger: SnapshotTrigger;
  totalBytes: number | null;
  deltaBytes: number | null;
  entries: number | null;
  coverage: DataCoverage;
}

export interface CategoryDelta {
  categoryId: string;
  name: string;
  /** Signed bytes; null means the change could not be computed. */
  bytesDelta: number | null;
  entriesDelta: number | null;
  newBytes: number | null;
  oldBytes: number | null;
  partial: boolean;
}

export interface ChangeSinceLastScan {
  fromSnapshotId: string | null;
  toSnapshotId: string | null;
  fromAt: string | null;
  toAt: string | null;
  totalBytesDelta: number | null;
  categoryDeltas: CategoryDelta[];
  coverage: DataCoverage;
  note: string | null;
}

export interface HistoryData {
  snapshots: HistorySnapshot[];
  changeSinceLastScan: ChangeSinceLastScan | null;
  coverage: DataCoverage;
}

// ---------------------------------------------------------------------------
// Drives
// ---------------------------------------------------------------------------

export interface Drive {
  id: string;
  name: string;
  kind: DriveKind;
  connection: DriveConnection;
  health: DriveHealth;
  totalBytes: number | null;
  usedBytes: number | null;
  freeBytes: number | null;
  /** A fraction in 0..1, or null when it cannot be calculated honestly. */
  usedShare: number | null;
  lastScanAt: string | null;
  snapshotCount: number | null;
  note: string | null;
}

export interface DrivesData {
  drives: Drive[];
  coverage: DataCoverage;
}

export interface HomeSummary {
  generatedAt: string;
  lastScanAt: string | null;
  safeReclaimableBytes: number | null;
  reviewReclaimableBytes: number | null;
  actionableOpportunityCount: number | null;
  drives: Drive[];
  topCategories: CategoryTotals[];
  changeSinceLastScan: ChangeSinceLastScan | null;
  coverage: DataCoverage;
}

// ---------------------------------------------------------------------------
// Startup / network / privacy contracts used by later screens
// ---------------------------------------------------------------------------

export type StartupTier = "SAFE_TO_DISABLE" | "REVIEW_FIRST" | "OFF_LIMITS";

export interface StartupItem {
  id: string;
  name: string;
  enabled: boolean;
  sourceLabel: string;
  commandLabel: string | null;
  owner: string | null;
  tier: StartupTier;
  why: string;
}

export interface StartupReport {
  items: StartupItem[];
  totalEnabled: number | null;
  truncated: number | null;
  coverage: DataCoverage;
}

export type DnsPrivacyPolicy =
  | "DOCUMENTED_NO_LOGGING"
  | "DOCUMENTED_RETENTION"
  | "UNDISCLOSED"
  | "UNKNOWN";

export interface DnsRankedCandidate {
  name: string;
  operator: string | null;
  score: number | null;
  latencyScore: number | null;
  reliabilityScore: number | null;
  dnssecScore: number | null;
  privacyScore: number | null;
  medianLatencyMs: number | null;
  p95LatencyMs: number | null;
  failureRate: number | null;
  samples: number | null;
  privacyPolicy: DnsPrivacyPolicy;
  supportsDnssec: boolean | null;
  supportsFiltering: boolean | null;
}

export interface DnsExcludedCandidate {
  name: string;
  reason: "INSUFFICIENT_EVIDENCE" | "TOO_UNRELIABLE" | "UNDISCLOSED_PRIVACY" | "NOT_MEASURED";
  detail: string;
}

export interface DnsRanking {
  ranked: DnsRankedCandidate[];
  excluded: DnsExcludedCandidate[];
  /** Must state that switching DNS changes who sees every lookup. */
  note: string;
  coverage: DataCoverage;
}

export interface BlockListEntry {
  domain: string;
  category: string;
  ipv4: string;
  ipv6: string;
}

export interface RejectedDomain {
  domain: string;
  reason: string;
}

export interface BlockListPlan {
  entries: BlockListEntry[];
  total: number | null;
  duplicatesRemoved: number | null;
  invalidRemoved: RejectedDomain[];
  whitelisted: string[];
  truncated: number | null;
  reversible: true;
  note: string;
  coverage: DataCoverage;
}

export type PrivacyFindingKind =
  | "TRACKING_COOKIE"
  | "SESSION_COOKIE"
  | "BROWSING_HISTORY"
  | "CACHE_ENTRY"
  | "SITE_STORAGE"
  | "UNKNOWN";

export interface PrivacyFinding {
  kind: PrivacyFindingKind;
  count: number | null;
  /** Estimates are labelled as estimates in the UI. */
  estimatedBytes: number | null;
  detail: string;
}

export interface PrivacySummary {
  findings: PrivacyFinding[];
  totalCount: number | null;
  /**
   * Null when any finding lacks a size estimate. A partial sum is never
   * promoted to a total.
   */
  totalEstimatedBytes: number | null;
  hasUnknownKinds: boolean;
  note: string;
  coverage: DataCoverage;
}

// ---------------------------------------------------------------------------
// The one API interface
// ---------------------------------------------------------------------------

export interface CoreSightApi {
  getGlobalStatus(): Promise<GlobalStatus>;
  getHomeSummary(): Promise<HomeSummary>;
  getStorage(): Promise<StorageData>;
  getCleanup(): Promise<CleanupData>;
  previewCleanup(opportunityIds: string[]): Promise<CleanupPreview>;
  executeCleanup(confirmation: CleanupConfirmation): Promise<CleanupExecution>;
  getHistory(): Promise<HistoryData>;
  listDrives(): Promise<DrivesData>;

  getDuplicateReport(): Promise<DuplicateReport>;
  getBigFileReport(): Promise<BigFileReport>;
  getStartupReport(): Promise<StartupReport>;
  getDnsRanking(): Promise<DnsRanking>;
  getBlockListPlan(): Promise<BlockListPlan>;
  getPrivacySummary(): Promise<PrivacySummary>;
}

// ---------------------------------------------------------------------------
// Deterministic, in-memory fixtures
// ---------------------------------------------------------------------------

export type MockScenario = "ready" | "empty" | "partial" | "error";

export interface MockApiOptions {
  scenario?: MockScenario;
  /** A fixed delay keeps loading states observable in the UI shell. */
  latencyMs?: number;
}

const GIB = 1024 ** 3;
const TIB = 1024 ** 4;
const gb = (value: number): number => Math.round(value * GIB);
const tb = (value: number): number => Math.round(value * TIB);

const COMPLETE_COVERAGE: DataCoverage = {
  completeness: "complete",
  unreadableEntries: 0,
  unknownSizeEntries: 0,
  excludedAreas: [],
  note: null
};

const PARTIAL_COVERAGE: DataCoverage = {
  completeness: "partial",
  unreadableEntries: 184,
  unknownSizeEntries: 37,
  excludedAreas: ["Protected system locations", "Offline external drive"],
  note: "Totals include only readable entries. They are lower bounds, not complete totals."
};

const UNVERIFIED_DUPLICATE_COVERAGE: DataCoverage = {
  completeness: "partial",
  unreadableEntries: 0,
  unknownSizeEntries: 0,
  excludedAreas: [],
  note: "Files without a verified content hash are excluded from duplicate claims."
};

function coverageFor(scenario: MockScenario, base: DataCoverage = COMPLETE_COVERAGE): DataCoverage {
  return structuredClone(scenario === "partial" ? PARTIAL_COVERAGE : base);
}

function makeCategory(
  id: string,
  name: string,
  bytes: number | null,
  shareOfParent: number | null,
  deltaSinceLast: number | null,
  itemCount: number | null,
  children: CategoryTotals[] = [],
  extra: Partial<
    Pick<CategoryTotals, "entriesWithoutSize" | "observationErrors" | "sampleLocations">
  > = {}
): CategoryTotals {
  return {
    id,
    name,
    bytes,
    shareOfParent,
    deltaSinceLast,
    itemCount,
    children,
    entriesWithoutSize: extra.entriesWithoutSize ?? 0,
    observationErrors: extra.observationErrors ?? 0,
    sampleLocations: extra.sampleLocations ?? []
  };
}

function buildCategories(partial: boolean): CategoryTotals[] {
  const unknownSizes = partial ? 37 : 0;
  const observationErrors = partial ? 184 : 0;

  return [
    makeCategory(
      "applications",
      "Applications",
      gb(112.4),
      0.2905,
      gb(1.2),
      84,
      [
        makeCategory("applications-design", "Design suite", gb(54.2), 0.482, gb(0.6), 18),
        makeCategory(
          "applications-xcode",
          "Xcode",
          gb(28.4),
          0.253,
          gb(0.9),
          4,
          [],
          {
            sampleLocations: [
              {
                label: "Xcode application",
                displayPath: "Applications/Xcode.app",
                bytes: gb(28.4)
              }
            ]
          }
        ),
        makeCategory("applications-utilities", "Utilities & drivers", gb(29.8), 0.265, gb(-0.3), 62)
      ],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "photos-videos",
      "Photos & Videos",
      gb(88.2),
      0.228,
      gb(2.7),
      64210,
      [
        makeCategory(
          "photos-library",
          "Photos library",
          gb(71.6),
          0.812,
          gb(2.1),
          51840,
          [],
          {
            sampleLocations: [
              {
                label: "System photo library",
                displayPath: "Pictures/Photos Library.photoslibrary",
                bytes: gb(71.6)
              }
            ]
          }
        ),
        makeCategory("video-projects", "Video projects", gb(16.6), 0.188, gb(0.6), 12370)
      ],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "development",
      "Development",
      gb(61.8),
      0.1597,
      gb(14.8),
      488302,
      [
        makeCategory(
          "development-derived",
          "Derived data",
          gb(21.4),
          0.346,
          gb(8.9),
          84221,
          [],
          {
            sampleLocations: [
              {
                label: "Build caches",
                displayPath: "Library/Developer/Xcode/DerivedData",
                bytes: gb(21.4)
              }
            ]
          }
        ),
        makeCategory("development-packages", "Package caches", gb(18.2), 0.294, gb(3.4), 302140),
        makeCategory("development-simulators", "Simulators", gb(22.2), 0.36, gb(2.5), 101941)
      ],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "games",
      "Games",
      gb(42.8),
      0.1106,
      0,
      12,
      [
        makeCategory("games-titles", "Installed titles", gb(38.6), 0.902, gb(0.4), 8),
        makeCategory("games-support", "Game support data", gb(4.2), 0.098, gb(-0.4), 4)
      ],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "system",
      "System",
      gb(23.7),
      0.0613,
      gb(1.1),
      84192,
      [],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "documents",
      "Documents",
      gb(18.6),
      0.0481,
      gb(-0.4),
      2341,
      [],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "downloads",
      "Downloads",
      gb(17.9),
      0.0463,
      gb(5.6),
      1118,
      [
        makeCategory("downloads-installers", "Installers", gb(12.1), 0.676, gb(4.9), 324),
        makeCategory("downloads-archives", "Archives", gb(5.8), 0.324, gb(0.7), 794)
      ],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "media",
      "Media",
      gb(12.4),
      0.0321,
      gb(0.3),
      1873,
      [],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "temporary",
      "Temporary",
      gb(6.3),
      0.0163,
      gb(0.1),
      29146,
      [
        makeCategory("temporary-browser", "Browser caches", gb(2.8), 0.444, gb(0.1), 18442),
        makeCategory("temporary-app", "App caches", gb(3.5), 0.556, 0, 10704)
      ],
      { entriesWithoutSize: unknownSizes, observationErrors }
    ),
    makeCategory(
      "other",
      "Other",
      gb(2.8),
      0.0072,
      gb(0.1),
      460,
      [],
      { entriesWithoutSize: unknownSizes, observationErrors }
    )
  ];
}

function buildDuplicateReport(partial: boolean): DuplicateReport {
  const coverage = partial
    ? PARTIAL_COVERAGE
    : UNVERIFIED_DUPLICATE_COVERAGE;
  return {
    groups: [
      {
        id: "dup-photo-0148",
        contentSha256: "9f3c5ea1b7d2480c2f5a6e97bc1246a9d7718eb3f05c415dae2fa9e5a81e6c3b",
        hashVerified: true,
        sizeBytes: 24_600_000,
        recoverableBytes: 24_600_000,
        members: [
          {
            displayPath: "Pictures/2019/Italy/DSC_0148.jpg",
            sizeBytes: 24_600_000,
            isHardlinkAlias: false
          },
          {
            displayPath: "Pictures/2019/Italy/DSC_0148 copy.jpg",
            sizeBytes: 24_600_000,
            isHardlinkAlias: false
          }
        ]
      },
      {
        id: "dup-atlas-master",
        contentSha256: "f4b2c160e99873a572c4dd6e1a38b424c71f2fdf5b8d7ea3c9d41cb7f9e04815",
        hashVerified: true,
        sizeBytes: gb(2.2),
        recoverableBytes: gb(2.2),
        members: [
          {
            displayPath: "Projects/Atlas/assets/master.mov",
            sizeBytes: gb(2.2),
            isHardlinkAlias: false
          },
          {
            displayPath: "Projects/Atlas/assets/master copy.mov",
            sizeBytes: gb(2.2),
            isHardlinkAlias: false
          },
          {
            displayPath: "Projects/Atlas/assets/master-link.mov",
            sizeBytes: gb(2.2),
            isHardlinkAlias: true
          }
        ]
      }
    ],
    totalGroups: 2,
    totalRecoverableBytes: 24_600_000 + gb(2.2),
    unverifiedEntries: partial ? 1_241 : 1_204,
    unknownSizeEntries: partial ? 37 : 6,
    truncatedGroups: 0,
    coverage
  };
}

function buildBigFileReport(partial: boolean): BigFileReport {
  return {
    files: [
      {
        id: "big-vm",
        displayPath: "Development/Virtual Machines/Research.vmwarevm",
        sizeBytes: gb(48),
        modifiedAt: "2026-09-09T14:20:00.000Z",
        ageDays: 62,
        categoryName: "Development"
      },
      {
        id: "big-capture",
        displayPath: "Video Projects/Product capture master.mov",
        sizeBytes: gb(12.4),
        modifiedAt: null,
        ageDays: null,
        categoryName: "Photos & Videos"
      }
    ],
    thresholdBytes: gb(10),
    unknownAgeEntries: 3,
    truncated: 0,
    coverage: structuredClone(partial ? PARTIAL_COVERAGE : COMPLETE_COVERAGE)
  };
}

function buildDriveFixtures(): Drive[] {
  return [
    {
      id: "drive-macintosh",
      name: "Macintosh HD",
      kind: "internal",
      connection: "connected",
      health: "healthy",
      totalBytes: gb(494.4),
      usedBytes: gb(386.9),
      freeBytes: gb(107.5),
      usedShare: 0.783,
      lastScanAt: "2026-11-10T15:18:00.000Z",
      snapshotCount: 12,
      note: "Startup volume. Free space is above the 10% headroom target."
    },
    {
      id: "drive-studio-archive",
      name: "Studio Archive",
      kind: "external",
      connection: "remembered",
      health: "unknown",
      totalBytes: tb(2),
      usedBytes: tb(1.21),
      freeBytes: null,
      usedShare: 0.605,
      lastScanAt: "2026-10-28T18:40:00.000Z",
      snapshotCount: 4,
      note: "Remembered external drive. Connect it to refresh; read-only while detached."
    }
  ];
}

const CLEANUP_ITEMS: CleanupItem[] = [
  {
    id: "opp-build-caches",
    categoryId: "development",
    tier: "SAFE",
    title: "Rebuildable build caches",
    whatItIs:
      "Derived data, module caches, and simulator diagnostics generated by local Xcode builds.",
    why:
      "84,221 cache files are at least 21 days old and no project has opened them during that period.",
    observedBytes: gb(8.1),
    recoverableBytes: gb(8.1),
    itemCount: 84_221,
    estimatedAgeDays: 21,
    whatStaysUntouched:
      "Source code, project files, exported builds, signing assets, and every cache newer than 21 days.",
    consequence:
      "The next build recreates these files. The first rebuild after cleanup will take longer; no source or exported build is moved.",
    evidence: [
      {
        kind: "observation",
        label: "Age observed",
        detail: "Oldest matching cache is 21 days old; newest is 4 days old."
      },
      {
        kind: "verification",
        label: "Regenerable",
        detail: "Every entry is classified as a build cache. Project sources are not included."
      },
      {
        kind: "safety",
        label: "Reversible",
        detail: "Items move to Trash with a restore receipt and 30-day retention."
      }
    ],
    previewable: true,
    offLimitsReason: null
  },
  {
    id: "opp-browser-caches",
    categoryId: "temporary",
    tier: "SAFE",
    title: "Browser caches",
    whatItIs: "Cached images, scripts, and fonts stored by the browser profile.",
    why:
      "18,442 entries are marked cache-only. History, bookmarks, passwords, cookies, and open sessions are excluded.",
    observedBytes: gb(3.1),
    recoverableBytes: gb(3.1),
    itemCount: 18_442,
    estimatedAgeDays: null,
    whatStaysUntouched:
      "Browsing history, bookmarks, passwords, installed extensions, cookies, and every signed-in session.",
    consequence:
      "Sites re-download cached assets on the next visit. Pages may feel slower once; you stay signed in.",
    evidence: [
      {
        kind: "observation",
        label: "Age not used",
        detail: "Browser cache metadata did not expose a trustworthy last-access time."
      },
      {
        kind: "verification",
        label: "Scope checked",
        detail: "Only cache-class entries are included. Session and credential stores are excluded."
      },
      {
        kind: "safety",
        label: "Reversible",
        detail: "Items move to Trash with a restore receipt and 30-day retention."
      }
    ],
    previewable: true,
    offLimitsReason: null
  },
  {
    id: "opp-diagnostic-logs",
    categoryId: "temporary",
    tier: "SAFE",
    title: "Old diagnostic logs",
    whatItIs: "Rotated application logs and crash reports kept for troubleshooting.",
    why: "1.2 GB of rotated logs are older than 30 days; the active log files are excluded.",
    observedBytes: gb(1.2),
    recoverableBytes: gb(1.2),
    itemCount: 2_814,
    estimatedAgeDays: 30,
    whatStaysUntouched:
      "Active logs, recent crash reports, and every file from the current troubleshooting window.",
    consequence:
      "Historical log detail becomes unavailable after Trash is emptied. Current diagnostics keep working.",
    evidence: [
      {
        kind: "observation",
        label: "Rotation observed",
        detail: "All selected files are rotated archives older than 30 days."
      },
      {
        kind: "safety",
        label: "Reversible",
        detail: "Items move to Trash with a restore receipt and 30-day retention."
      }
    ],
    previewable: true,
    offLimitsReason: null
  },
  {
    id: "opp-old-downloads",
    categoryId: "downloads",
    tier: "REVIEW",
    title: "Downloads untouched for a year",
    whatItIs: "Installer archives and exported files in Downloads with no recent modification.",
    why:
      "1,118 files have not been modified for at least 418 days. This is a review item because Downloads can hold user data.",
    observedBytes: gb(14.6),
    recoverableBytes: gb(14.6),
    itemCount: 1_118,
    estimatedAgeDays: 418,
    whatStaysUntouched:
      "Everything modified within the last year, plus documents, photos, and app support data outside Downloads.",
    consequence:
      "Items move to Trash and remain restorable for 30 days. If Trash is emptied, the files are gone.",
    evidence: [
      {
        kind: "observation",
        label: "Modification age",
        detail: "The newest selected file was last modified 418 days ago."
      },
      {
        kind: "verification",
        label: "User-data category",
        detail: "Downloads is permission-sensitive, so this plan requires the explicit review checkbox."
      },
      {
        kind: "safety",
        label: "Reversible",
        detail: "Items move to Trash with a restore receipt and 30-day retention."
      }
    ],
    previewable: true,
    offLimitsReason: null
  },
  {
    id: "opp-photo-duplicates",
    categoryId: "photos-videos",
    tier: "REVIEW",
    title: "Hash-verified photo duplicates",
    whatItIs: "Byte-identical photo copies across managed project folders.",
    why:
      "SHA-256 verification found 24 duplicate groups. Each group keeps its earliest original. Hard-link aliases are shown but contribute zero recoverable bytes.",
    observedBytes: gb(4),
    recoverableBytes: gb(4),
    itemCount: 24,
    estimatedAgeDays: null,
    whatStaysUntouched:
      "The earliest original in every group, every unique photo, and all metadata. Hard-link aliases are never counted as savings.",
    consequence:
      "Only verified extra copies move to Trash. The library still opens normally and originals remain in place.",
    evidence: [
      {
        kind: "verification",
        label: "SHA-256 verified",
        detail: "Hashes and sizes match for every member. A missing hash is never treated as a duplicate."
      },
      {
        kind: "safety",
        label: "Hard links collapsed",
        detail: "Aliases of the same filesystem object are identified and never counted twice."
      }
    ],
    previewable: true,
    offLimitsReason: null
  },
  {
    id: "opp-old-installers",
    categoryId: "downloads",
    tier: "REVIEW",
    title: "Superseded installer images",
    whatItIs: "Disk images and installers for app versions newer than the installed release.",
    why:
      "The currently installed versions are newer than these installers. The files are kept as review items because re-downloading may be inconvenient.",
    observedBytes: gb(1.3),
    recoverableBytes: gb(1.3),
    itemCount: 19,
    estimatedAgeDays: 96,
    whatStaysUntouched:
      "Installers for apps that are not installed, the latest installer for each app, and anything outside Downloads.",
    consequence:
      "Superseded installers move to Trash. Re-downloading an old version may not be possible from the vendor.",
    evidence: [
      {
        kind: "verification",
        label: "Version cross-check",
        detail: "Installed app versions are newer than every selected installer."
      }
    ],
    previewable: true,
    offLimitsReason: null
  }
];

const OFF_LIMITS_ITEMS: CleanupItem[] = [
  {
    id: "off-system",
    categoryId: "system",
    tier: "OFF_LIMITS",
    title: "System and boot files",
    whatItIs: "Operating-system files, boot support, kernel extensions, and protected caches.",
    why: "Protected by engine policy. This category is exposed for understanding, never proposed.",
    observedBytes: gb(23.7),
    recoverableBytes: null,
    itemCount: null,
    estimatedAgeDays: null,
    whatStaysUntouched: "Everything. This category cannot be selected or previewed.",
    consequence: "No action is available for this category.",
    evidence: [],
    previewable: false,
    offLimitsReason: "Protected system area — exposure only, never actionable."
  },
  {
    id: "off-photo-originals",
    categoryId: "photos-library",
    tier: "OFF_LIMITS",
    title: "Photo library originals",
    whatItIs: "Original media managed by the system photo library.",
    why: "No verified second copy exists, so removal would risk irreplaceable data.",
    observedBytes: gb(71.6),
    recoverableBytes: null,
    itemCount: 51_840,
    estimatedAgeDays: null,
    whatStaysUntouched: "Everything. Cleanup never proposes original media.",
    consequence: "No action is available. Duplicate copies can be reviewed separately.",
    evidence: [],
    previewable: false,
    offLimitsReason: "Irreplaceable originals without a verified second copy."
  },
  {
    id: "off-startup",
    categoryId: "startup",
    tier: "OFF_LIMITS",
    title: "Startup and login items",
    whatItIs: "Login items and background launch agents.",
    why: "Startup changes require a separate authorization path and are not part of storage cleanup.",
    observedBytes: null,
    recoverableBytes: null,
    itemCount: null,
    estimatedAgeDays: null,
    whatStaysUntouched: "Everything. No startup item is selected or changed.",
    consequence: "No action is available in this phase.",
    evidence: [],
    previewable: false,
    offLimitsReason: "Separate, explicitly authorized capability required."
  }
];

function sumRecoverable(items: CleanupItem[]): number | null {
  if (items.length === 0) return 0;
  let total = 0;
  for (const item of items) {
    if (item.recoverableBytes === null) return null;
    total += item.recoverableBytes;
  }
  return total;
}

function buildHomeSummary(scenario: MockScenario, categories: CategoryTotals[]): HomeSummary {
  const safe = sumRecoverable(CLEANUP_ITEMS.filter((item) => item.tier === "SAFE"));
  const review = sumRecoverable(CLEANUP_ITEMS.filter((item) => item.tier === "REVIEW"));
  return {
    generatedAt: "2026-11-10T15:30:00.000Z",
    lastScanAt: "2026-11-10T15:18:00.000Z",
    safeReclaimableBytes: safe,
    reviewReclaimableBytes: review,
    actionableOpportunityCount: CLEANUP_ITEMS.length,
    drives: buildDriveFixtures(),
    topCategories: categories.slice(0, 3),
    changeSinceLastScan: buildChangeSinceLastScan(coverageFor(scenario)),
    coverage: coverageFor(scenario)
  };
}

function buildChangeSinceLastScan(coverage: DataCoverage): ChangeSinceLastScan {
  return {
    fromSnapshotId: "snapshot-2026-11-03",
    toSnapshotId: "snapshot-2026-11-10",
    fromAt: "2026-11-03T09:12:00.000Z",
    toAt: "2026-11-10T15:18:00.000Z",
    totalBytesDelta: gb(25.5),
    categoryDeltas: [
      {
        categoryId: "development",
        name: "Development",
        bytesDelta: gb(14.8),
        entriesDelta: 38_204,
        newBytes: gb(61.8),
        oldBytes: gb(47),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "downloads",
        name: "Downloads",
        bytesDelta: gb(5.6),
        entriesDelta: 611,
        newBytes: gb(17.9),
        oldBytes: gb(12.3),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "photos-videos",
        name: "Photos & Videos",
        bytesDelta: gb(2.7),
        entriesDelta: 1_204,
        newBytes: gb(88.2),
        oldBytes: gb(85.5),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "applications",
        name: "Applications",
        bytesDelta: gb(1.2),
        entriesDelta: 2,
        newBytes: gb(112.4),
        oldBytes: gb(111.2),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "system",
        name: "System",
        bytesDelta: gb(1.1),
        entriesDelta: 8_421,
        newBytes: gb(23.7),
        oldBytes: gb(22.6),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "media",
        name: "Media",
        bytesDelta: gb(0.3),
        entriesDelta: 34,
        newBytes: gb(12.4),
        oldBytes: gb(12.1),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "temporary",
        name: "Temporary",
        bytesDelta: gb(0.1),
        entriesDelta: 1_022,
        newBytes: gb(6.3),
        oldBytes: gb(6.2),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "other",
        name: "Other",
        bytesDelta: gb(0.1),
        entriesDelta: 21,
        newBytes: gb(2.8),
        oldBytes: gb(2.7),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "documents",
        name: "Documents",
        bytesDelta: gb(-0.4),
        entriesDelta: -18,
        newBytes: gb(18.6),
        oldBytes: gb(19),
        partial: coverage.completeness !== "complete"
      },
      {
        categoryId: "games",
        name: "Games",
        bytesDelta: 0,
        entriesDelta: 0,
        newBytes: gb(42.8),
        oldBytes: gb(42.8),
        partial: coverage.completeness !== "complete"
      }
    ],
    coverage,
    note: "Compared with the snapshot from 3 Nov 2026, 09:12."
  };
}

function buildHistory(scenario: MockScenario): HistoryData {
  const coverage = coverageFor(scenario);
  return {
    snapshots: [
      {
        id: "snapshot-2026-11-10",
        takenAt: "2026-11-10T15:18:00.000Z",
        trigger: "scheduled",
        totalBytes: gb(386.9),
        deltaBytes: gb(25.5),
        entries: 1_204_822,
        coverage
      },
      {
        id: "snapshot-2026-11-03",
        takenAt: "2026-11-03T09:12:00.000Z",
        trigger: "scheduled",
        totalBytes: gb(361.4),
        deltaBytes: gb(17.2),
        entries: 1_166_410,
        coverage
      },
      {
        id: "snapshot-2026-10-27",
        takenAt: "2026-10-27T18:40:00.000Z",
        trigger: "first-run",
        totalBytes: gb(344.2),
        deltaBytes: null,
        entries: 1_151_204,
        coverage
      }
    ],
    changeSinceLastScan: buildChangeSinceLastScan(coverage),
    coverage
  };
}

function buildCleanup(scenario: MockScenario, cleanedIds: ReadonlySet<string>): CleanupData {
  const items = CLEANUP_ITEMS.filter((item) => !cleanedIds.has(item.id));
  return {
    scanId: "scan-2026-11-10-1518",
    lastScanAt: "2026-11-10T15:18:00.000Z",
    items,
    offLimits: OFF_LIMITS_ITEMS,
    safeRecoverableBytes: sumRecoverable(items.filter((item) => item.tier === "SAFE")),
    reviewRecoverableBytes: sumRecoverable(items.filter((item) => item.tier === "REVIEW")),
    coverage: coverageFor(scenario)
  };
}

function buildStartupReport(scenario: MockScenario): StartupReport {
  return {
    items: [
      {
        id: "startup-backup-helper",
        name: "Archive Backup Helper",
        enabled: true,
        sourceLabel: "User login item",
        commandLabel: "Archive Backup Helper",
        owner: "Local user",
        tier: "SAFE_TO_DISABLE",
        why: "User-owned login item. Disabling it stops automatic backups until the app is opened manually."
      },
      {
        id: "startup-vendor-updater",
        name: "Studio Updater",
        enabled: true,
        sourceLabel: "Vendor launch agent",
        commandLabel: "Studio Updater",
        owner: null,
        tier: "REVIEW_FIRST",
        why: "Checks for updates from a vendor process. Review the publisher and update policy before changing it."
      },
      {
        id: "startup-system-security",
        name: "Security agent",
        enabled: true,
        sourceLabel: "System launch daemon",
        commandLabel: null,
        owner: "System",
        tier: "OFF_LIMITS",
        why: "Privileged system source. It is never offered for change."
      }
    ],
    totalEnabled: 5,
    truncated: 0,
    coverage: coverageFor(scenario)
  };
}

function buildDnsRanking(scenario: MockScenario): DnsRanking {
  return {
    ranked: [
      {
        name: "Resolver A",
        operator: "Example privacy cooperative",
        score: 82,
        latencyScore: 88,
        reliabilityScore: 98,
        dnssecScore: 100,
        privacyScore: 75,
        medianLatencyMs: 18.4,
        p95LatencyMs: 42.1,
        failureRate: 0.012,
        samples: 120,
        privacyPolicy: "DOCUMENTED_NO_LOGGING",
        supportsDnssec: true,
        supportsFiltering: false
      },
      {
        name: "Resolver B",
        operator: "Example network",
        score: 71,
        latencyScore: 80,
        reliabilityScore: 95,
        dnssecScore: 100,
        privacyScore: 25,
        medianLatencyMs: 24.7,
        p95LatencyMs: 58.8,
        failureRate: 0.021,
        samples: 120,
        privacyPolicy: "DOCUMENTED_RETENTION",
        supportsDnssec: true,
        supportsFiltering: false
      }
    ],
    excluded: [
      {
        name: "Unmeasured resolver",
        reason: "NOT_MEASURED",
        detail: "No benchmark samples were supplied, so it cannot be ranked."
      }
    ],
    note:
      "Guidance only. CoreSight never switches DNS. Scores weight latency 40%, reliability 30%, DNSSEC 15%, and documented privacy 15%. Switching DNS changes who can see every lookup.",
    coverage: coverageFor(scenario)
  };
}

function buildBlockListPlan(scenario: MockScenario): BlockListPlan {
  return {
    entries: [
      {
        domain: "ads.example.test",
        category: "Advertising",
        ipv4: "0.0.0.0",
        ipv6: "::"
      },
      {
        domain: "tracker.example.test",
        category: "Tracking",
        ipv4: "0.0.0.0",
        ipv6: "::"
      }
    ],
    total: 2,
    duplicatesRemoved: 1,
    invalidRemoved: [{ domain: "not a domain", reason: "contains whitespace" }],
    whitelisted: ["bank.example.test"],
    truncated: 0,
    reversible: true,
    note:
      "Every rejected and duplicate entry is reported. The plan is reversible and writes nothing until a separately authorized action is confirmed.",
    coverage: coverageFor(scenario)
  };
}

function buildPrivacySummary(scenario: MockScenario): PrivacySummary {
  return {
    findings: [
      {
        kind: "TRACKING_COOKIE",
        count: 24,
        estimatedBytes: 1_240_000,
        detail: "Persistent cookies from analytics domains."
      },
      {
        kind: "SESSION_COOKIE",
        count: 8,
        estimatedBytes: null,
        detail: "Removing these would sign you out of sites."
      },
      {
        kind: "CACHE_ENTRY",
        count: 1_842,
        estimatedBytes: 84_200_000,
        detail: "Cached resources; evidence only."
      }
    ],
    totalCount: 1_874,
    totalEstimatedBytes: null,
    hasUnknownKinds: false,
    note:
      "Evidence only, not a delete list. The byte total is not reported because at least one finding has no size estimate. Session cookies are kept separate from tracking cookies.",
    coverage: coverageFor(scenario)
  };
}

function emptyDuplicateReport(): DuplicateReport {
  return {
    groups: [],
    totalGroups: 0,
    totalRecoverableBytes: 0,
    unverifiedEntries: 0,
    unknownSizeEntries: 0,
    truncatedGroups: 0,
    coverage: structuredClone(COMPLETE_COVERAGE)
  };
}

function emptyBigFileReport(): BigFileReport {
  return {
    files: [],
    thresholdBytes: gb(10),
    unknownAgeEntries: 0,
    truncated: 0,
    coverage: structuredClone(COMPLETE_COVERAGE)
  };
}

class MockCoreSightApi implements CoreSightApi {
  private readonly scenario: MockScenario;
  private readonly latencyMs: number;
  private readonly cleanedIds = new Set<string>();
  private readonly previews = new Map<string, CleanupPreview>();
  private previewSequence = 0;

  constructor(options: MockApiOptions) {
    this.scenario = options.scenario ?? "ready";
    this.latencyMs = options.latencyMs ?? 220;
  }

  private async respond<T>(value: T | (() => T)): Promise<T> {
    if (this.latencyMs > 0) {
      await new Promise<void>((resolve) => {
        setTimeout(resolve, this.latencyMs);
      });
    }
    if (this.scenario === "error") {
      throw {
        code: "SNAPSHOT_UNAVAILABLE",
        message: "CoreSight could not read the local snapshot for this screen.",
        detail: "The mock API is simulating a recoverable read failure. No files were touched."
      } satisfies ApiError;
    }
    const resolved = typeof value === "function" ? (value as () => T)() : value;
    return structuredClone(resolved);
  }

  public async getGlobalStatus(): Promise<GlobalStatus> {
    const isEmpty = this.scenario === "empty";
    const partial = this.scenario === "partial";
    return this.respond({
      contractVersion: CONTRACT_VERSION,
      simulatedData: true,
      scan: {
        scanId: isEmpty ? null : "scan-2026-11-10-1518",
        state: isEmpty ? "idle" : partial ? "partial" : "complete",
        startedAt: isEmpty ? null : "2026-11-10T15:12:42.000Z",
        completedAt: isEmpty ? null : "2026-11-10T15:18:00.000Z",
        filesSeen: isEmpty ? null : 1_204_822,
        bytesSeen: isEmpty ? null : gb(386.9),
        currentArea: null,
        etaSeconds: null,
        coverage: isEmpty
          ? structuredClone(COMPLETE_COVERAGE)
          : coverageFor(this.scenario)
      }
    });
  }

  public async getHomeSummary(): Promise<HomeSummary> {
    if (this.scenario === "empty") {
      return this.respond({
        generatedAt: "2026-11-10T15:30:00.000Z",
        lastScanAt: null,
        safeReclaimableBytes: null,
        reviewReclaimableBytes: null,
        actionableOpportunityCount: null,
        drives: [],
        topCategories: [],
        changeSinceLastScan: null,
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    const categories = buildCategories(this.scenario === "partial");
    return this.respond(buildHomeSummary(this.scenario, categories));
  }

  public async getStorage(): Promise<StorageData> {
    if (this.scenario === "empty") {
      return this.respond({
        categories: [],
        totalObservedBytes: null,
        totalObservedItems: null,
        duplicateReport: emptyDuplicateReport(),
        bigFileReport: emptyBigFileReport(),
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    const partial = this.scenario === "partial";
    return this.respond({
      categories: buildCategories(partial),
      totalObservedBytes: gb(386.9),
      totalObservedItems: 1_087_920,
      duplicateReport: buildDuplicateReport(partial),
      bigFileReport: buildBigFileReport(partial),
      coverage: coverageFor(this.scenario)
    });
  }

  public async getCleanup(): Promise<CleanupData> {
    if (this.scenario === "empty") {
      return this.respond({
        scanId: null,
        lastScanAt: null,
        items: [],
        offLimits: [],
        safeRecoverableBytes: null,
        reviewRecoverableBytes: null,
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    return this.respond(buildCleanup(this.scenario, this.cleanedIds));
  }

  public async previewCleanup(opportunityIds: string[]): Promise<CleanupPreview> {
    if (this.scenario === "empty" || opportunityIds.length === 0) {
      return this.respond(() => {
        throw {
          code: "NOTHING_TO_DO",
          message: "Choose one cleanup opportunity before previewing.",
          detail: "There is no clean-everything path."
        } satisfies ApiError;
      });
    }
    if (this.scenario === "error") {
      return this.respond({} as CleanupPreview);
    }

    const allItems = [...CLEANUP_ITEMS, ...OFF_LIMITS_ITEMS];
    const selected = opportunityIds.map((id) => allItems.find((item) => item.id === id));
    if (selected.some((item) => item === undefined)) {
      throw {
        code: "OPPORTUNITY_NOT_FOUND",
        message: "That cleanup opportunity is no longer available.",
        detail: "Refresh the cleanup list and choose another item."
      } satisfies ApiError;
    }
    const items = selected as CleanupItem[];
    if (items.some((item) => item.tier === "OFF_LIMITS" || !item.previewable)) {
      throw {
        code: "OFF_LIMITS_CATEGORY",
        message: "Off-limits categories cannot be previewed or changed.",
        detail: "They remain visible so the boundary is clear."
      } satisfies ApiError;
    }

    const coverage = coverageFor(this.scenario);
    this.previewSequence += 1;
    const preview: CleanupPreview = {
      planId: `plan-${opportunityIds.join("--")}-${this.previewSequence}`,
      opportunityIds,
      items: items.map((item) => ({
        title: item.title,
        bytes: item.recoverableBytes,
        itemCount: item.itemCount,
        destination: "System Trash (CoreSight cleanup batch)",
        undo: "Restore from Trash within 30 days using the cleanup receipt.",
        reason: item.why
      })),
      totalBytes: sumRecoverable(items),
      whatStaysUntouched: items.map((item) => item.whatStaysUntouched),
      destination: "System Trash (CoreSight cleanup batch)",
      howToUndo: "Open Trash, select the CoreSight cleanup batch, and choose Put Back within 30 days.",
      consequence: items.map((item) => item.consequence).join(" "),
      complete:
        coverage.completeness === "complete" &&
        items.every((item) => item.recoverableBytes !== null),
      warnings:
        coverage.completeness === "complete"
          ? []
          : [
              "The scan is incomplete. Safe-tier cleanup is blocked until a complete scan succeeds.",
              ...coverage.excludedAreas.map((area) => `Excluded from this scan: ${area}.`)
            ],
      coverage
    };
    this.previews.set(preview.planId, preview);
    return this.respond(preview);
  }

  public async executeCleanup(confirmation: CleanupConfirmation): Promise<CleanupExecution> {
    if (this.scenario === "error") {
      return this.respond({} as CleanupExecution);
    }
    const preview = this.previews.get(confirmation.planId);
    if (preview === undefined || !confirmation.confirmed) {
      throw {
        code: "PLAN_NOT_CONFIRMED",
        message: "The cleanup was not confirmed and nothing was moved.",
        detail: "Open a fresh preview before confirming."
      } satisfies ApiError;
    }
    if (!preview.complete || preview.coverage.completeness !== "complete") {
      throw {
        code: "PLAN_BLOCKED_PARTIAL_KNOWLEDGE",
        message: "This cleanup is blocked because the scan is incomplete.",
        detail: "A complete scan is required before any cleanup can move files."
      } satisfies ApiError;
    }
    for (const id of preview.opportunityIds) this.cleanedIds.add(id);
    this.previews.delete(confirmation.planId);
    return this.respond({
      planId: preview.planId,
      accepted: true,
      simulated: true,
      movedItems: preview.items.reduce<number | null>((total, item) => {
        if (total === null || item.itemCount === null) return null;
        return total + item.itemCount;
      }, 0),
      freedBytes: preview.totalBytes,
      receipts: preview.opportunityIds.map((id) => `receipt-${id}`),
      leftovers: [],
      verification: "simulated",
      note:
        "UI shell only: the mock adapter recorded the confirmation in memory. No files were moved; Phase 9 replaces this adapter with the authorized IPC command."
    });
  }

  public async getHistory(): Promise<HistoryData> {
    if (this.scenario === "empty") {
      return this.respond({
        snapshots: [],
        changeSinceLastScan: null,
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    return this.respond(buildHistory(this.scenario));
  }

  public async listDrives(): Promise<DrivesData> {
    if (this.scenario === "empty") {
      return this.respond({ drives: [], coverage: structuredClone(COMPLETE_COVERAGE) });
    }
    return this.respond({ drives: buildDriveFixtures(), coverage: coverageFor(this.scenario) });
  }

  public async getDuplicateReport(): Promise<DuplicateReport> {
    if (this.scenario === "empty") return this.respond(emptyDuplicateReport());
    return this.respond(buildDuplicateReport(this.scenario === "partial"));
  }

  public async getBigFileReport(): Promise<BigFileReport> {
    if (this.scenario === "empty") return this.respond(emptyBigFileReport());
    return this.respond(buildBigFileReport(this.scenario === "partial"));
  }

  public async getStartupReport(): Promise<StartupReport> {
    if (this.scenario === "empty") {
      return this.respond({
        items: [],
        totalEnabled: null,
        truncated: 0,
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    return this.respond(buildStartupReport(this.scenario));
  }

  public async getDnsRanking(): Promise<DnsRanking> {
    if (this.scenario === "empty") {
      return this.respond({
        ranked: [],
        excluded: [],
        note:
          "No resolver measurements are available. CoreSight never switches DNS and will not guess a ranking.",
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    return this.respond(buildDnsRanking(this.scenario));
  }

  public async getBlockListPlan(): Promise<BlockListPlan> {
    if (this.scenario === "empty") {
      return this.respond({
        entries: [],
        total: 0,
        duplicatesRemoved: 0,
        invalidRemoved: [],
        whitelisted: [],
        truncated: 0,
        reversible: true,
        note: "No block-list rules were supplied. Nothing will be written.",
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    return this.respond(buildBlockListPlan(this.scenario));
  }

  public async getPrivacySummary(): Promise<PrivacySummary> {
    if (this.scenario === "empty") {
      return this.respond({
        findings: [],
        totalCount: 0,
        totalEstimatedBytes: 0,
        hasUnknownKinds: false,
        note: "No privacy findings were observed in this scan.",
        coverage: structuredClone(COMPLETE_COVERAGE)
      });
    }
    return this.respond(buildPrivacySummary(this.scenario));
  }
}

/** No network, no timers beyond the deterministic loading delay, no filesystem. */
export function createMockApi(options: MockApiOptions = {}): CoreSightApi {
  return new MockCoreSightApi(options);
}
