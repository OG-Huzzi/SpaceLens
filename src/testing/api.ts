/**
 * A counting wrapper around the deterministic mock API.
 *
 * The performance suite needs to answer one question precisely: "did this
 * render cause another call to the engine?". Wrapping the mock lets a test
 * count calls per method without touching `src/api.ts`, which is owned by
 * another worker.
 */

import {
  createMockApi,
  type CleanupConfirmation,
  type CleanupData,
  type CleanupExecution,
  type CleanupPreview,
  type CoreSightApi,
  type DnsRanking,
  type Drive,
  type DrivesData,
  type DuplicateReport,
  type BigFileReport,
  type GlobalStatus,
  type HistoryData,
  type HomeSummary,
  type MockApiOptions,
  type PrivacySummary,
  type StartupReport,
  type StorageData,
  type BlockListPlan
} from "../api";

export interface CountingApi extends CoreSightApi {
  /** How many times each method was called. */
  readonly calls: Readonly<Record<string, number>>;
  /** How many calls are still in flight. */
  readonly inFlight: number;
  /** Total number of calls made so far. */
  readonly totalCalls: number;
  /** Calls made to one method. */
  callsTo(method: keyof CoreSightApi): number;
}

export function createCountingApi(
  options: MockApiOptions = {},
  base?: CoreSightApi
): CountingApi {
  const source = base ?? createMockApi(options);
  const calls: Record<string, number> = {};
  let inFlight = 0;
  let totalCalls = 0;

  function track<T>(name: string, run: () => Promise<T>): Promise<T> {
    calls[name] = (calls[name] ?? 0) + 1;
    totalCalls += 1;
    inFlight += 1;
    return run().finally(() => {
      inFlight -= 1;
    });
  }

  const api: CoreSightApi = {
    getGlobalStatus: () => track("getGlobalStatus", () => source.getGlobalStatus()),
    getHomeSummary: () => track("getHomeSummary", () => source.getHomeSummary()),
    getStorage: () => track("getStorage", () => source.getStorage()),
    getCleanup: () => track("getCleanup", () => source.getCleanup()),
    previewCleanup: (ids: string[]) =>
      track("previewCleanup", () => source.previewCleanup(ids)),
    executeCleanup: (confirmation: CleanupConfirmation) =>
      track("executeCleanup", () => source.executeCleanup(confirmation)),
    getHistory: () => track("getHistory", () => source.getHistory()),
    listDrives: () => track("listDrives", () => source.listDrives()),
    getDuplicateReport: () =>
      track("getDuplicateReport", () => source.getDuplicateReport()),
    getBigFileReport: () => track("getBigFileReport", () => source.getBigFileReport()),
    getStartupReport: () => track("getStartupReport", () => source.getStartupReport()),
    getDnsRanking: () => track("getDnsRanking", () => source.getDnsRanking()),
    getBlockListPlan: () => track("getBlockListPlan", () => source.getBlockListPlan()),
    getPrivacySummary: () => track("getPrivacySummary", () => source.getPrivacySummary())
  };

  const counting: CountingApi = {
    ...api,
    get calls(): Readonly<Record<string, number>> {
      return { ...calls };
    },
    get inFlight(): number {
      return inFlight;
    },
    get totalCalls(): number {
      return totalCalls;
    },
    callsTo(method: keyof CoreSightApi): number {
      return calls[method] ?? 0;
    }
  };

  return counting;
}

/**
 * An API that answers every read with the supplied fixture and never writes.
 * Used to feed a screen a dataset larger than the shipped fixtures.
 */
export function createStaticApi(overrides: {
  storage?: StorageData;
  cleanup?: CleanupData;
  history?: HistoryData;
  drives?: DrivesData;
  home?: HomeSummary;
  globalStatus?: GlobalStatus;
  startup?: StartupReport;
  dns?: DnsRanking;
  blockList?: BlockListPlan;
  privacy?: PrivacySummary;
  duplicates?: DuplicateReport;
  bigFiles?: BigFileReport;
}): CoreSightApi {
  const storage: StorageData = overrides.storage ?? {
    categories: [],
    totalObservedBytes: null,
    totalObservedItems: null,
    duplicateReport: {
      groups: [],
      totalGroups: null,
      totalRecoverableBytes: null,
      unverifiedEntries: null,
      unknownSizeEntries: null,
      truncatedGroups: null,
      coverage: {
        completeness: "complete",
        unreadableEntries: 0,
        unknownSizeEntries: 0,
        excludedAreas: [],
        note: null
      }
    },
    bigFileReport: {
      files: [],
      thresholdBytes: null,
      unknownAgeEntries: null,
      truncated: null,
      coverage: {
        completeness: "complete",
        unreadableEntries: 0,
        unknownSizeEntries: 0,
        excludedAreas: [],
        note: null
      }
    },
    coverage: {
      completeness: "complete",
      unreadableEntries: 0,
      unknownSizeEntries: 0,
      excludedAreas: [],
      note: null
    }
  };

  const drives: Drive[] = overrides.drives?.drives ?? [];

  return {
    getGlobalStatus: async () =>
      overrides.globalStatus ?? {
        contractVersion: "v1",
        simulatedData: true,
        scan: {
          scanId: null,
          state: "complete",
          startedAt: null,
          completedAt: null,
          filesSeen: null,
          bytesSeen: null,
          currentArea: null,
          etaSeconds: null,
          coverage: {
            completeness: "complete",
            unreadableEntries: 0,
            unknownSizeEntries: 0,
            excludedAreas: [],
            note: null
          }
        }
      },
    getHomeSummary: async () =>
      overrides.home ?? {
        generatedAt: "2026-11-10T15:30:00.000Z",
        lastScanAt: null,
        safeReclaimableBytes: null,
        reviewReclaimableBytes: null,
        actionableOpportunityCount: null,
        drives,
        topCategories: [],
        changeSinceLastScan: null,
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    getStorage: async () => storage,
    getCleanup: async () =>
      overrides.cleanup ?? {
        scanId: null,
        lastScanAt: null,
        items: [],
        offLimits: [],
        safeRecoverableBytes: null,
        reviewRecoverableBytes: null,
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    previewCleanup: async () => {
      throw { code: "NOTHING_TO_DO", message: "not used in this fixture" };
    },
    executeCleanup: async () => {
      throw { code: "NOTHING_TO_DO", message: "not used in this fixture" };
    },
    getHistory: async () =>
      overrides.history ?? {
        snapshots: [],
        changeSinceLastScan: null,
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    listDrives: async () =>
      overrides.drives ?? {
        drives,
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    getDuplicateReport: async () => storage.duplicateReport,
    getBigFileReport: async () => storage.bigFileReport,
    getStartupReport: async () =>
      overrides.startup ?? {
        items: [],
        totalEnabled: null,
        truncated: null,
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    getDnsRanking: async () =>
      overrides.dns ?? {
        ranked: [],
        excluded: [],
        note: "not used in this fixture",
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    getBlockListPlan: async () =>
      overrides.blockList ?? {
        entries: [],
        total: null,
        duplicatesRemoved: null,
        invalidRemoved: [],
        whitelisted: [],
        truncated: null,
        reversible: true,
        note: "not used in this fixture",
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      },
    getPrivacySummary: async () =>
      overrides.privacy ?? {
        findings: [],
        totalCount: null,
        totalEstimatedBytes: null,
        hasUnknownKinds: false,
        note: "not used in this fixture",
        coverage: {
          completeness: "complete",
          unreadableEntries: 0,
          unknownSizeEntries: 0,
          excludedAreas: [],
          note: null
        }
      }
  };
}

/** A category tree with `count` top-level categories, for scale tests. */
export function makeCategories(count: number): StorageData["categories"] {
  const gb = 1024 ** 3;
  return Array.from({ length: count }, (_, index) => ({
    id: `category-${index}`,
    name: `Category ${index}`,
    bytes: Math.round(gb * (1 + (index % 50))),
    shareOfParent: 1 / count,
    deltaSinceLast: 0,
    itemCount: index,
    entries: index,
    files: null,
    directories: null,
    logicalSize: null,
    children: [],
    entriesWithoutSize: 0,
    observationErrors: 0,
    sampleLocations: []
  }));
}

/** A complete storage payload with `count` top-level categories. */
export function makeStorageData(count: number): StorageData {
  const coverage = {
    completeness: "complete" as const,
    unreadableEntries: 0,
    unknownSizeEntries: 0,
    excludedAreas: [],
    note: null
  };
  return {
    categories: makeCategories(count),
    totalObservedBytes: 1024 ** 4,
    totalObservedItems: count,
    duplicateReport: {
      groups: [],
      totalGroups: 0,
      totalRecoverableBytes: 0,
      unverifiedEntries: 0,
      unknownSizeEntries: 0,
      truncatedGroups: 0,
      coverage
    },
    bigFileReport: {
      files: [],
      thresholdBytes: 1024 ** 3,
      unknownAgeEntries: 0,
      truncated: 0,
      coverage
    },
    coverage
  };
}

export type { CleanupPreview, CleanupExecution };
