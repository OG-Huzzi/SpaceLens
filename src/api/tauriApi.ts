/**
 * Live Tauri transport for the CoreSight UI contract.
 *
 * TRANSPORT POLICY
 * ----------------
 * `createApi()` selects exactly one transport, once, from the runtime:
 *   - inside a Tauri webview (`window.__TAURI_INTERNALS__` exists) it returns
 *     `TauriCoreSightApi`;
 *   - everywhere else (tests, `vite dev`, a plain browser) it returns the
 *     deterministic in-memory mock.
 *
 * There is deliberately no per-call fallback. If a live command fails, the
 * rejection is normalised into an `ApiError` and shown to the user. Replacing a
 * failed real reading with fixture bytes would present invented data as the
 * user's disk, which is the worst failure this product can have.
 *
 * COMMAND NAMES
 * -------------
 * Commands from `docs/API_CONTRACTS.md` are used verbatim where the UI
 * operation maps to one:
 *   previewCleanup  -> `coresight.v1.previewPlan`
 *   executeCleanup  -> `coresight.v1.confirmPlan`, then
 *                      `coresight.v1.executePlan` (the safety check runs at
 *                      confirmation, exactly as the contract requires)
 *   getHistory      -> `coresight.v1.getHistory`
 *   listDrives      -> `coresight.v1.listDrives`
 *
 * The five screens also read aggregate view models that the Phase 0 command
 * list does not include (Home/Storage/Cleanup summaries and the later-phase
 * reports). Those are additive `coresight.v1.*` commands, permitted by the
 * contract's "Rules for evolution" section. Their names mirror the view-model
 * names below; every one is validated field by field.
 *
 * VALIDATION POLICY
 * -----------------
 * No response is cast. Every field is read through `read*`, which rejects a
 * missing key, `undefined`, a non-finite number, or a wrong type with a typed
 * `INVALID_RESPONSE` error. A nullable field accepts only an explicit `null`
 * or its declared type — `undefined` is never turned into `null` and never
 * into `0`.
 */

import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  createMockApi,
  type ApiError,
  type BigFile,
  type BigFileReport,
  type BlockListEntry,
  type BlockListPlan,
  type CategoryDelta,
  type CategoryTotals,
  type ChangeSinceLastScan,
  type CleanupConfirmation,
  type CleanupData,
  type CleanupEvidence,
  type CleanupExecution,
  type CleanupItem,
  type CleanupPreview,
  type CoreSightApi,
  type DataCoverage,
  type DnsExcludedCandidate,
  type DnsRankedCandidate,
  type DnsRanking,
  type Drive,
  type DrivesData,
  type DuplicateGroup,
  type DuplicateMember,
  type DuplicateReport,
  type GlobalStatus,
  type HistoryData,
  type HistorySnapshot,
  type HomeSummary,
  type MockApiOptions,
  type PreviewItem,
  type PrivacyFinding,
  type PrivacySummary,
  type RejectedDomain,
  type ScanStatus,
  type StartupItem,
  type StartupReport,
  type StorageData,
  type StorageLocation
} from "../api";

export const TAURI_COMMAND_PREFIX = "coresight.v1." as const;

export const TAURI_COMMANDS = {
  getGlobalStatus: `${TAURI_COMMAND_PREFIX}getGlobalStatus`,
  getHomeSummary: `${TAURI_COMMAND_PREFIX}getHomeSummary`,
  getStorage: `${TAURI_COMMAND_PREFIX}getStorage`,
  getCleanup: `${TAURI_COMMAND_PREFIX}getCleanup`,
  previewPlan: `${TAURI_COMMAND_PREFIX}previewPlan`,
  confirmPlan: `${TAURI_COMMAND_PREFIX}confirmPlan`,
  executePlan: `${TAURI_COMMAND_PREFIX}executePlan`,
  getHistory: `${TAURI_COMMAND_PREFIX}getHistory`,
  listDrives: `${TAURI_COMMAND_PREFIX}listDrives`,
  getDuplicateReport: `${TAURI_COMMAND_PREFIX}getDuplicateReport`,
  getBigFileReport: `${TAURI_COMMAND_PREFIX}getBigFileReport`,
  getStartupReport: `${TAURI_COMMAND_PREFIX}getStartupReport`,
  getDnsRanking: `${TAURI_COMMAND_PREFIX}getDnsRanking`,
  getBlockListPlan: `${TAURI_COMMAND_PREFIX}getBlockListPlan`,
  getPrivacySummary: `${TAURI_COMMAND_PREFIX}getPrivacySummary`
} as const;

/**
 * The Tauri handler the desktop shell actually registers for a contract
 * command.
 *
 * # Why this table exists
 *
 * A `#[tauri::command]` function name must be a Rust identifier, so it cannot
 * contain the dots of the `coresight.v1.*` namespace. The shell therefore
 * registers snake_case handlers and carries the fully-qualified contract name
 * inside the response envelope as `ok.kind`.
 *
 * So the namespace is honoured in the *payload*, not in the invoke name, and
 * `request` verifies that the `kind` the shell reports is the command that was
 * asked for — a mismatch is rejected rather than parsed, which catches a shell
 * that routes one command to another's handler.
 *
 * A contract command absent from this table has no shell handler yet. Calling
 * it fails with `UNSUPPORTED_IN_THIS_BUILD` instead of invoking a name the
 * shell would reject; nothing is ever invented to fill the gap.
 */
export const TAURI_HANDLERS: Readonly<Record<string, string>> = {
  "coresight.v1.startScan": "start_scan",
  "coresight.v1.cancelScan": "cancel_scan",
  "coresight.v1.getDriveSummary": "get_drive_summary",
  "coresight.v1.getCategories": "get_categories",
  "coresight.v1.getOpportunities": "get_opportunities",
  "coresight.v1.previewPlan": "preview_plan",
  "coresight.v1.confirmPlan": "confirm_plan",
  "coresight.v1.executePlan": "execute_plan",
  "coresight.v1.getHistory": "get_history",
  "coresight.v1.listDrives": "list_drives"
};

/**
 * Stable codes whose copy is part of the reviewed UI contract. Anything else is
 * treated as unknown and replaced by a generic, non-technical message.
 */
export const KNOWN_API_ERROR_CODES: ReadonlySet<string> = new Set([
  "SCAN_IN_PROGRESS",
  "DRIVE_OFFLINE",
  "PLAN_REJECTED_UNSAFE",
  "NOTHING_TO_DO",
  "PERMISSION_GAP",
  "DB_CORRUPT_RESCANNED",
  "UNKNOWN_COMMAND",
  "SCAN_NOT_FOUND",
  "SCAN_UNAVAILABLE",
  "HISTORY_UNAVAILABLE",
  "NOT_IMPLEMENTED_IN_THIS_BUILD",
  "UNSUPPORTED_IN_THIS_BUILD",
  "INVALID_REQUEST",
  "UNKNOWN_OPPORTUNITY",
  "PLAN_NOT_FOUND",
  "PLAN_NOT_CONFIRMED",
  "DRIVE_NOT_FOUND",
  "PLAN_UNRESOLVABLE",
  "INVALID_ARGUMENTS",
  "SESSION_UNAVAILABLE",
  "ENVELOPE_ENCODE_FAILED",
  "SNAPSHOT_UNAVAILABLE",
  "OPPORTUNITY_NOT_FOUND",
  "OFF_LIMITS_CATEGORY",
  "PLAN_BLOCKED_PARTIAL_KNOWLEDGE",
  "INVALID_RESPONSE",
  "ENGINE_UNREACHABLE",
  "UNKNOWN_API_ERROR"
]);

export class CoreSightApiError extends Error implements ApiError {
  public readonly code: string;
  public readonly detail?: string;

  public constructor(code: string, message: string, detail?: string) {
    super(message);
    this.name = "CoreSightApiError";
    this.code = code;
    if (detail !== undefined) this.detail = detail;
  }
}

/**
 * Accepts either a raw payload or the desktop shell's typed envelope
 * (`{ ok: { kind, payload } }` / `{ error: ApiError }`). An enveloped error is
 * normalised before any response parser runs, so a known code keeps its
 * reviewed message even when the host resolves rather than rejects.
 */
function unwrapTransportEnvelope(value: unknown, expectedKind: string): unknown {
  const record = readObject(value, "response");
  const hasOk = Object.prototype.hasOwnProperty.call(record, "ok") && record.ok !== undefined;
  const hasError =
    Object.prototype.hasOwnProperty.call(record, "error") && record.error !== undefined;

  if (hasOk && hasError) {
    throw invalidResponse("response", "must contain exactly one of ok or error");
  }
  if (hasError) throw toTransportError(record.error);
  if (!hasOk) return value;

  const ok = readObject(record.ok, "response.ok");
  const kind = readString(ok.kind, "response.ok.kind");
  if (kind !== expectedKind) {
    // The shell answered a different question than the one asked. Parsing it
    // would silently attribute another command's data to this screen.
    throw invalidResponse(
      "response.ok.kind",
      `must be "${expectedKind}" but the shell reported "${kind}"`
    );
  }
  if (ok.payload === undefined) {
    throw invalidResponse("response.ok.payload", "must be present");
  }
  return ok.payload;
}

function invalidResponse(path: string, expectation: string): CoreSightApiError {
  return new CoreSightApiError(
    "INVALID_RESPONSE",
    "CoreSight received an unreadable response from the local engine.",
    `Field "${path}" ${expectation}. No value was displayed.`
  );
}

type JsonRecord = Record<string, unknown>;

function readObject(value: unknown, path: string): JsonRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalidResponse(path, "must be an object");
  }
  return value as JsonRecord;
}

function readString(value: unknown, path: string): string {
  if (typeof value !== "string") throw invalidResponse(path, "must be a string");
  return value;
}

function readNullableString(value: unknown, path: string): string | null {
  if (value === null) return null;
  return readString(value, path);
}

function readNumber(value: unknown, path: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw invalidResponse(path, "must be a finite number");
  }
  return value;
}

/** Only an explicit `null` is accepted here; `undefined` fails validation. */
function readNullableNumber(value: unknown, path: string): number | null {
  if (value === null) return null;
  return readNumber(value, path);
}

function readBoolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") throw invalidResponse(path, "must be a boolean");
  return value;
}

function readNullableBoolean(value: unknown, path: string): boolean | null {
  if (value === null) return null;
  return readBoolean(value, path);
}

/** For contract literals that must be exactly `true` (verified claims). */
function readTrue(value: unknown, path: string): true {
  if (value !== true) throw invalidResponse(path, "must be true");
  return true;
}

function readLiteral<T extends string>(
  value: unknown,
  path: string,
  allowed: readonly T[]
): T {
  if (typeof value !== "string" || !allowed.includes(value as T)) {
    throw invalidResponse(path, `must be one of ${allowed.join(", ")}`);
  }
  return value as T;
}

function readArray<T>(
  value: unknown,
  path: string,
  parseItem: (item: unknown, itemPath: string) => T
): T[] {
  if (!Array.isArray(value)) throw invalidResponse(path, "must be an array");
  return value.map((item, index) => parseItem(item, `${path}[${index}]`));
}

const COMPLETENESS = ["complete", "partial", "unknown"] as const;
const SCAN_STATES = ["idle", "scanning", "complete", "partial", "failed"] as const;
const SAFETY_TIERS = ["SAFE", "REVIEW", "OFF_LIMITS"] as const;
const EVIDENCE_KINDS = ["observation", "verification", "safety"] as const;
const DRIVE_KINDS = ["internal", "external"] as const;
const DRIVE_CONNECTIONS = ["connected", "remembered"] as const;
const DRIVE_HEALTH = ["healthy", "attention", "unknown"] as const;
const SNAPSHOT_TRIGGERS = ["first-run", "manual", "scheduled"] as const;
const VERIFICATION_STATES = ["verified", "partial", "simulated"] as const;
const STARTUP_TIERS = ["SAFE_TO_DISABLE", "REVIEW_FIRST", "OFF_LIMITS"] as const;
const DNS_PRIVACY_POLICIES = [
  "DOCUMENTED_NO_LOGGING",
  "DOCUMENTED_RETENTION",
  "UNDISCLOSED",
  "UNKNOWN"
] as const;
const DNS_EXCLUSION_REASONS = [
  "INSUFFICIENT_EVIDENCE",
  "TOO_UNRELIABLE",
  "UNDISCLOSED_PRIVACY",
  "NOT_MEASURED"
] as const;
const PRIVACY_FINDING_KINDS = [
  "TRACKING_COOKIE",
  "SESSION_COOKIE",
  "BROWSING_HISTORY",
  "CACHE_ENTRY",
  "SITE_STORAGE",
  "UNKNOWN"
] as const;

export function parseDataCoverage(value: unknown, path = "coverage"): DataCoverage {
  const record = readObject(value, path);
  return {
    completeness: readLiteral(record.completeness, `${path}.completeness`, COMPLETENESS),
    unreadableEntries: readNullableNumber(
      record.unreadableEntries,
      `${path}.unreadableEntries`
    ),
    unknownSizeEntries: readNullableNumber(
      record.unknownSizeEntries,
      `${path}.unknownSizeEntries`
    ),
    excludedAreas: readArray(record.excludedAreas, `${path}.excludedAreas`, readString),
    note: readNullableString(record.note, `${path}.note`)
  };
}

function parseStorageLocation(value: unknown, path: string): StorageLocation {
  const record = readObject(value, path);
  return {
    label: readString(record.label, `${path}.label`),
    displayPath: readString(record.displayPath, `${path}.displayPath`),
    bytes: readNullableNumber(record.bytes, `${path}.bytes`)
  };
}

function parseCategoryTotals(value: unknown, path: string): CategoryTotals {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    name: readString(record.name, `${path}.name`),
    bytes: readNullableNumber(record.bytes, `${path}.bytes`),
    shareOfParent: readNullableNumber(record.shareOfParent, `${path}.shareOfParent`),
    deltaSinceLast: readNullableNumber(record.deltaSinceLast, `${path}.deltaSinceLast`),
    itemCount: readNullableNumber(record.itemCount, `${path}.itemCount`),
    entries: readNullableNumber(record.entries, `${path}.entries`),
    files: readNullableNumber(record.files, `${path}.files`),
    directories: readNullableNumber(record.directories, `${path}.directories`),
    logicalSize: readNullableNumber(record.logicalSize, `${path}.logicalSize`),
    children: readArray(record.children, `${path}.children`, parseCategoryTotals),
    entriesWithoutSize: readNullableNumber(
      record.entriesWithoutSize,
      `${path}.entriesWithoutSize`
    ),
    observationErrors: readNullableNumber(
      record.observationErrors,
      `${path}.observationErrors`
    ),
    sampleLocations: readArray(
      record.sampleLocations,
      `${path}.sampleLocations`,
      parseStorageLocation
    )
  };
}

function parseScanStatus(value: unknown, path: string): ScanStatus {
  const record = readObject(value, path);
  return {
    scanId: readNullableString(record.scanId, `${path}.scanId`),
    state: readLiteral(record.state, `${path}.state`, SCAN_STATES),
    startedAt: readNullableString(record.startedAt, `${path}.startedAt`),
    completedAt: readNullableString(record.completedAt, `${path}.completedAt`),
    filesSeen: readNullableNumber(record.filesSeen, `${path}.filesSeen`),
    bytesSeen: readNullableNumber(record.bytesSeen, `${path}.bytesSeen`),
    currentArea: readNullableString(record.currentArea, `${path}.currentArea`),
    etaSeconds: readNullableNumber(record.etaSeconds, `${path}.etaSeconds`),
    coverage: parseDataCoverage(record.coverage, `${path}.coverage`)
  };
}

function parseGlobalStatus(value: unknown): GlobalStatus {
  const record = readObject(value, "globalStatus");
  return {
    contractVersion: readString(record.contractVersion, "globalStatus.contractVersion"),
    simulatedData: readBoolean(record.simulatedData, "globalStatus.simulatedData"),
    scan: parseScanStatus(record.scan, "globalStatus.scan")
  };
}

function parseDuplicateMember(value: unknown, path: string): DuplicateMember {
  const record = readObject(value, path);
  return {
    displayPath: readString(record.displayPath, `${path}.displayPath`),
    sizeBytes: readNullableNumber(record.sizeBytes, `${path}.sizeBytes`),
    isHardlinkAlias: readBoolean(record.isHardlinkAlias, `${path}.isHardlinkAlias`)
  };
}

function parseDuplicateGroup(value: unknown, path: string): DuplicateGroup {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    contentSha256: readString(record.contentSha256, `${path}.contentSha256`),
    hashVerified: readTrue(record.hashVerified, `${path}.hashVerified`),
    sizeBytes: readNullableNumber(record.sizeBytes, `${path}.sizeBytes`),
    recoverableBytes: readNullableNumber(record.recoverableBytes, `${path}.recoverableBytes`),
    members: readArray(record.members, `${path}.members`, parseDuplicateMember)
  };
}

function parseDuplicateReport(value: unknown, path: string): DuplicateReport {
  const record = readObject(value, path);
  return {
    groups: readArray(record.groups, `${path}.groups`, parseDuplicateGroup),
    totalGroups: readNullableNumber(record.totalGroups, `${path}.totalGroups`),
    totalRecoverableBytes: readNullableNumber(
      record.totalRecoverableBytes,
      `${path}.totalRecoverableBytes`
    ),
    unverifiedEntries: readNullableNumber(
      record.unverifiedEntries,
      `${path}.unverifiedEntries`
    ),
    unknownSizeEntries: readNullableNumber(
      record.unknownSizeEntries,
      `${path}.unknownSizeEntries`
    ),
    truncatedGroups: readNullableNumber(record.truncatedGroups, `${path}.truncatedGroups`),
    coverage: parseDataCoverage(record.coverage, `${path}.coverage`)
  };
}

function parseBigFile(value: unknown, path: string): BigFile {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    displayPath: readString(record.displayPath, `${path}.displayPath`),
    sizeBytes: readNullableNumber(record.sizeBytes, `${path}.sizeBytes`),
    modifiedAt: readNullableString(record.modifiedAt, `${path}.modifiedAt`),
    ageDays: readNullableNumber(record.ageDays, `${path}.ageDays`),
    categoryName: readNullableString(record.categoryName, `${path}.categoryName`)
  };
}

function parseBigFileReport(value: unknown, path: string): BigFileReport {
  const record = readObject(value, path);
  return {
    files: readArray(record.files, `${path}.files`, parseBigFile),
    thresholdBytes: readNullableNumber(record.thresholdBytes, `${path}.thresholdBytes`),
    unknownAgeEntries: readNullableNumber(
      record.unknownAgeEntries,
      `${path}.unknownAgeEntries`
    ),
    truncated: readNullableNumber(record.truncated, `${path}.truncated`),
    coverage: parseDataCoverage(record.coverage, `${path}.coverage`)
  };
}

function parseCategoryDelta(value: unknown, path: string): CategoryDelta {
  const record = readObject(value, path);
  return {
    categoryId: readString(record.categoryId, `${path}.categoryId`),
    name: readString(record.name, `${path}.name`),
    bytesDelta: readNullableNumber(record.bytesDelta, `${path}.bytesDelta`),
    entriesDelta: readNullableNumber(record.entriesDelta, `${path}.entriesDelta`),
    newBytes: readNullableNumber(record.newBytes, `${path}.newBytes`),
    oldBytes: readNullableNumber(record.oldBytes, `${path}.oldBytes`),
    partial: readBoolean(record.partial, `${path}.partial`)
  };
}

function parseChangeSinceLastScan(value: unknown, path: string): ChangeSinceLastScan {
  const record = readObject(value, path);
  return {
    fromSnapshotId: readNullableString(record.fromSnapshotId, `${path}.fromSnapshotId`),
    toSnapshotId: readNullableString(record.toSnapshotId, `${path}.toSnapshotId`),
    fromAt: readNullableString(record.fromAt, `${path}.fromAt`),
    toAt: readNullableString(record.toAt, `${path}.toAt`),
    totalBytesDelta: readNullableNumber(
      record.totalBytesDelta,
      `${path}.totalBytesDelta`
    ),
    categoryDeltas: readArray(record.categoryDeltas, `${path}.categoryDeltas`, parseCategoryDelta),
    coverage: parseDataCoverage(record.coverage, `${path}.coverage`),
    note: readNullableString(record.note, `${path}.note`)
  };
}

function parseHomeSummary(value: unknown): HomeSummary {
  const record = readObject(value, "homeSummary");
  return {
    generatedAt: readString(record.generatedAt, "homeSummary.generatedAt"),
    lastScanAt: readNullableString(record.lastScanAt, "homeSummary.lastScanAt"),
    safeReclaimableBytes: readNullableNumber(
      record.safeReclaimableBytes,
      "homeSummary.safeReclaimableBytes"
    ),
    reviewReclaimableBytes: readNullableNumber(
      record.reviewReclaimableBytes,
      "homeSummary.reviewReclaimableBytes"
    ),
    actionableOpportunityCount: readNullableNumber(
      record.actionableOpportunityCount,
      "homeSummary.actionableOpportunityCount"
    ),
    drives: readArray(record.drives, "homeSummary.drives", parseDrive),
    topCategories: readArray(
      record.topCategories,
      "homeSummary.topCategories",
      parseCategoryTotals
    ),
    changeSinceLastScan:
      record.changeSinceLastScan === null
        ? null
        : parseChangeSinceLastScan(
            record.changeSinceLastScan,
            "homeSummary.changeSinceLastScan"
          ),
    coverage: parseDataCoverage(record.coverage, "homeSummary.coverage")
  };
}

function parseStorageData(value: unknown): StorageData {
  const record = readObject(value, "storage");
  return {
    categories: readArray(record.categories, "storage.categories", parseCategoryTotals),
    totalObservedBytes: readNullableNumber(
      record.totalObservedBytes,
      "storage.totalObservedBytes"
    ),
    totalObservedItems: readNullableNumber(
      record.totalObservedItems,
      "storage.totalObservedItems"
    ),
    duplicateReport: parseDuplicateReport(record.duplicateReport, "storage.duplicateReport"),
    bigFileReport: parseBigFileReport(record.bigFileReport, "storage.bigFileReport"),
    coverage: parseDataCoverage(record.coverage, "storage.coverage")
  };
}

function parseCleanupEvidence(value: unknown, path: string): CleanupEvidence {
  const record = readObject(value, path);
  return {
    kind: readLiteral(record.kind, `${path}.kind`, EVIDENCE_KINDS),
    label: readString(record.label, `${path}.label`),
    detail: readString(record.detail, `${path}.detail`)
  };
}

function parseCleanupItem(value: unknown, path: string): CleanupItem {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    categoryId: readString(record.categoryId, `${path}.categoryId`),
    tier: readLiteral(record.tier, `${path}.tier`, SAFETY_TIERS),
    title: readString(record.title, `${path}.title`),
    whatItIs: readString(record.whatItIs, `${path}.whatItIs`),
    why: readString(record.why, `${path}.why`),
    observedBytes: readNullableNumber(record.observedBytes, `${path}.observedBytes`),
    recoverableBytes: readNullableNumber(
      record.recoverableBytes,
      `${path}.recoverableBytes`
    ),
    itemCount: readNullableNumber(record.itemCount, `${path}.itemCount`),
    estimatedAgeDays: readNullableNumber(
      record.estimatedAgeDays,
      `${path}.estimatedAgeDays`
    ),
    whatStaysUntouched: readString(
      record.whatStaysUntouched,
      `${path}.whatStaysUntouched`
    ),
    consequence: readString(record.consequence, `${path}.consequence`),
    evidence: readArray(record.evidence, `${path}.evidence`, parseCleanupEvidence),
    previewable: readBoolean(record.previewable, `${path}.previewable`),
    offLimitsReason: readNullableString(
      record.offLimitsReason,
      `${path}.offLimitsReason`
    )
  };
}

function parseCleanupData(value: unknown): CleanupData {
  const record = readObject(value, "cleanup");
  return {
    scanId: readNullableString(record.scanId, "cleanup.scanId"),
    lastScanAt: readNullableString(record.lastScanAt, "cleanup.lastScanAt"),
    items: readArray(record.items, "cleanup.items", parseCleanupItem),
    offLimits: readArray(record.offLimits, "cleanup.offLimits", parseCleanupItem),
    safeRecoverableBytes: readNullableNumber(
      record.safeRecoverableBytes,
      "cleanup.safeRecoverableBytes"
    ),
    reviewRecoverableBytes: readNullableNumber(
      record.reviewRecoverableBytes,
      "cleanup.reviewRecoverableBytes"
    ),
    coverage: parseDataCoverage(record.coverage, "cleanup.coverage")
  };
}

function parsePreviewItem(value: unknown, path: string): PreviewItem {
  const record = readObject(value, path);
  return {
    title: readString(record.title, `${path}.title`),
    bytes: readNullableNumber(record.bytes, `${path}.bytes`),
    itemCount: readNullableNumber(record.itemCount, `${path}.itemCount`),
    destination: readNullableString(record.destination, `${path}.destination`),
    undo: readNullableString(record.undo, `${path}.undo`),
    reason: readString(record.reason, `${path}.reason`)
  };
}

export function parseCleanupPreview(value: unknown): CleanupPreview {
  const record = readObject(value, "preview");
  return {
    planId: readString(record.planId, "preview.planId"),
    opportunityIds: readArray(record.opportunityIds, "preview.opportunityIds", readString),
    items: readArray(record.items, "preview.items", parsePreviewItem),
    totalBytes: readNullableNumber(record.totalBytes, "preview.totalBytes"),
    whatStaysUntouched: readArray(
      record.whatStaysUntouched,
      "preview.whatStaysUntouched",
      readString
    ),
    destination: readNullableString(record.destination, "preview.destination"),
    howToUndo: readNullableString(record.howToUndo, "preview.howToUndo"),
    consequence: readString(record.consequence, "preview.consequence"),
    complete: readBoolean(record.complete, "preview.complete"),
    warnings: readArray(record.warnings, "preview.warnings", readString),
    coverage: parseDataCoverage(record.coverage, "preview.coverage")
  };
}

function parseCleanupExecution(value: unknown, planId: string): CleanupExecution {
  const record = readObject(value, "execution");
  if (!readBoolean(record.accepted, "execution.accepted")) {
    throw new CoreSightApiError(
      "PLAN_REJECTED_UNSAFE",
      "The cleanup was not accepted and nothing was moved.",
      "The engine refused the plan at execution time. No files were changed."
    );
  }
  return {
    planId,
    accepted: true,
    simulated: readBoolean(record.simulated, "execution.simulated"),
    movedItems: readNullableNumber(record.movedItems, "execution.movedItems"),
    freedBytes: readNullableNumber(record.freedBytes, "execution.freedBytes"),
    receipts: readArray(record.receipts, "execution.receipts", readString),
    leftovers: readArray(record.leftovers, "execution.leftovers", readString),
    verification: readLiteral(
      record.verification,
      "execution.verification",
      VERIFICATION_STATES
    ),
    note: readString(record.note, "execution.note")
  };
}

function parseHistorySnapshot(value: unknown, path: string): HistorySnapshot {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    takenAt: readNullableString(record.takenAt, `${path}.takenAt`),
    trigger: readLiteral(record.trigger, `${path}.trigger`, SNAPSHOT_TRIGGERS),
    totalBytes: readNullableNumber(record.totalBytes, `${path}.totalBytes`),
    deltaBytes: readNullableNumber(record.deltaBytes, `${path}.deltaBytes`),
    entries: readNullableNumber(record.entries, `${path}.entries`),
    coverage: parseDataCoverage(record.coverage, `${path}.coverage`)
  };
}

function parseHistoryData(value: unknown): HistoryData {
  const record = readObject(value, "history");
  return {
    snapshots: readArray(record.snapshots, "history.snapshots", parseHistorySnapshot),
    changeSinceLastScan:
      record.changeSinceLastScan === null
        ? null
        : parseChangeSinceLastScan(record.changeSinceLastScan, "history.changeSinceLastScan"),
    coverage: parseDataCoverage(record.coverage, "history.coverage")
  };
}

function parseDrive(value: unknown, path: string): Drive {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    name: readString(record.name, `${path}.name`),
    kind: readLiteral(record.kind, `${path}.kind`, DRIVE_KINDS),
    connection: readLiteral(record.connection, `${path}.connection`, DRIVE_CONNECTIONS),
    health: readLiteral(record.health, `${path}.health`, DRIVE_HEALTH),
    totalBytes: readNullableNumber(record.totalBytes, `${path}.totalBytes`),
    usedBytes: readNullableNumber(record.usedBytes, `${path}.usedBytes`),
    freeBytes: readNullableNumber(record.freeBytes, `${path}.freeBytes`),
    usedShare: readNullableNumber(record.usedShare, `${path}.usedShare`),
    lastScanAt: readNullableString(record.lastScanAt, `${path}.lastScanAt`),
    snapshotCount: readNullableNumber(record.snapshotCount, `${path}.snapshotCount`),
    note: readNullableString(record.note, `${path}.note`)
  };
}

function parseDrivesData(value: unknown): DrivesData {
  const record = readObject(value, "drives");
  return {
    drives: readArray(record.drives, "drives.drives", parseDrive),
    coverage: parseDataCoverage(record.coverage, "drives.coverage")
  };
}

function parseStartupItem(value: unknown, path: string): StartupItem {
  const record = readObject(value, path);
  return {
    id: readString(record.id, `${path}.id`),
    name: readString(record.name, `${path}.name`),
    enabled: readBoolean(record.enabled, `${path}.enabled`),
    sourceLabel: readString(record.sourceLabel, `${path}.sourceLabel`),
    commandLabel: readNullableString(record.commandLabel, `${path}.commandLabel`),
    owner: readNullableString(record.owner, `${path}.owner`),
    tier: readLiteral(record.tier, `${path}.tier`, STARTUP_TIERS),
    why: readString(record.why, `${path}.why`)
  };
}

function parseStartupReport(value: unknown): StartupReport {
  const record = readObject(value, "startup");
  return {
    items: readArray(record.items, "startup.items", parseStartupItem),
    totalEnabled: readNullableNumber(record.totalEnabled, "startup.totalEnabled"),
    truncated: readNullableNumber(record.truncated, "startup.truncated"),
    coverage: parseDataCoverage(record.coverage, "startup.coverage")
  };
}

function parseDnsRankedCandidate(value: unknown, path: string): DnsRankedCandidate {
  const record = readObject(value, path);
  return {
    name: readString(record.name, `${path}.name`),
    operator: readNullableString(record.operator, `${path}.operator`),
    score: readNullableNumber(record.score, `${path}.score`),
    latencyScore: readNullableNumber(record.latencyScore, `${path}.latencyScore`),
    reliabilityScore: readNullableNumber(
      record.reliabilityScore,
      `${path}.reliabilityScore`
    ),
    dnssecScore: readNullableNumber(record.dnssecScore, `${path}.dnssecScore`),
    privacyScore: readNullableNumber(record.privacyScore, `${path}.privacyScore`),
    medianLatencyMs: readNullableNumber(record.medianLatencyMs, `${path}.medianLatencyMs`),
    p95LatencyMs: readNullableNumber(record.p95LatencyMs, `${path}.p95LatencyMs`),
    failureRate: readNullableNumber(record.failureRate, `${path}.failureRate`),
    samples: readNullableNumber(record.samples, `${path}.samples`),
    privacyPolicy: readLiteral(
      record.privacyPolicy,
      `${path}.privacyPolicy`,
      DNS_PRIVACY_POLICIES
    ),
    supportsDnssec: readNullableBoolean(record.supportsDnssec, `${path}.supportsDnssec`),
    supportsFiltering: readNullableBoolean(
      record.supportsFiltering,
      `${path}.supportsFiltering`
    )
  };
}

function parseDnsExcludedCandidate(value: unknown, path: string): DnsExcludedCandidate {
  const record = readObject(value, path);
  return {
    name: readString(record.name, `${path}.name`),
    reason: readLiteral(record.reason, `${path}.reason`, DNS_EXCLUSION_REASONS),
    detail: readString(record.detail, `${path}.detail`)
  };
}

function parseDnsRanking(value: unknown): DnsRanking {
  const record = readObject(value, "dnsRanking");
  return {
    ranked: readArray(record.ranked, "dnsRanking.ranked", parseDnsRankedCandidate),
    excluded: readArray(
      record.excluded,
      "dnsRanking.excluded",
      parseDnsExcludedCandidate
    ),
    note: readString(record.note, "dnsRanking.note"),
    coverage: parseDataCoverage(record.coverage, "dnsRanking.coverage")
  };
}

function parseBlockListEntry(value: unknown, path: string): BlockListEntry {
  const record = readObject(value, path);
  return {
    domain: readString(record.domain, `${path}.domain`),
    category: readString(record.category, `${path}.category`),
    ipv4: readString(record.ipv4, `${path}.ipv4`),
    ipv6: readString(record.ipv6, `${path}.ipv6`)
  };
}

function parseRejectedDomain(value: unknown, path: string): RejectedDomain {
  const record = readObject(value, path);
  return {
    domain: readString(record.domain, `${path}.domain`),
    reason: readString(record.reason, `${path}.reason`)
  };
}

function parseBlockListPlan(value: unknown): BlockListPlan {
  const record = readObject(value, "blockListPlan");
  return {
    entries: readArray(record.entries, "blockListPlan.entries", parseBlockListEntry),
    total: readNullableNumber(record.total, "blockListPlan.total"),
    duplicatesRemoved: readNullableNumber(
      record.duplicatesRemoved,
      "blockListPlan.duplicatesRemoved"
    ),
    invalidRemoved: readArray(
      record.invalidRemoved,
      "blockListPlan.invalidRemoved",
      parseRejectedDomain
    ),
    whitelisted: readArray(record.whitelisted, "blockListPlan.whitelisted", readString),
    truncated: readNullableNumber(record.truncated, "blockListPlan.truncated"),
    reversible: readTrue(record.reversible, "blockListPlan.reversible"),
    note: readString(record.note, "blockListPlan.note"),
    coverage: parseDataCoverage(record.coverage, "blockListPlan.coverage")
  };
}

function parsePrivacyFinding(value: unknown, path: string): PrivacyFinding {
  const record = readObject(value, path);
  return {
    kind: readLiteral(record.kind, `${path}.kind`, PRIVACY_FINDING_KINDS),
    count: readNullableNumber(record.count, `${path}.count`),
    estimatedBytes: readNullableNumber(record.estimatedBytes, `${path}.estimatedBytes`),
    detail: readString(record.detail, `${path}.detail`)
  };
}

function parsePrivacySummary(value: unknown): PrivacySummary {
  const record = readObject(value, "privacySummary");
  return {
    findings: readArray(record.findings, "privacySummary.findings", parsePrivacyFinding),
    totalCount: readNullableNumber(record.totalCount, "privacySummary.totalCount"),
    totalEstimatedBytes: readNullableNumber(
      record.totalEstimatedBytes,
      "privacySummary.totalEstimatedBytes"
    ),
    hasUnknownKinds: readBoolean(
      record.hasUnknownKinds,
      "privacySummary.hasUnknownKinds"
    ),
    note: readString(record.note, "privacySummary.note"),
    coverage: parseDataCoverage(record.coverage, "privacySummary.coverage")
  };
}

export type InvokeTransport = (
  command: string,
  args?: Record<string, unknown>
) => Promise<unknown>;

function parseConfirmPlan(value: unknown): { accepted: boolean; rejectedCount: number } {
  const record = readObject(value, "confirmation");
  const accepted = readBoolean(record.accepted, "confirmation.accepted");
  const rejected = readArray(record.rejected, "confirmation.rejected", (item, path) => {
    if (typeof item === "string") return item;
    readObject(item, path);
    return "";
  });
  return { accepted, rejectedCount: rejected.length };
}

/**
 * Turns anything thrown by `invoke` into an `ApiError` the UI can render.
 * Unknown codes never reach the user, and neither do error messages, `detail`
 * payloads, or stack traces from the runtime.
 */
export function toTransportError(error: unknown): CoreSightApiError {
  if (error instanceof CoreSightApiError) return error;

  if (typeof error === "object" && error !== null) {
    const candidate = error as { code?: unknown; message?: unknown; detail?: unknown };
    if (
      typeof candidate.code === "string" &&
      typeof candidate.message === "string" &&
      candidate.message.trim().length > 0
    ) {
      if (KNOWN_API_ERROR_CODES.has(candidate.code)) {
        const detail =
          typeof candidate.detail === "string" && candidate.detail.trim().length > 0
            ? candidate.detail
            : undefined;
        return new CoreSightApiError(candidate.code, candidate.message, detail);
      }
      return new CoreSightApiError(
        "UNKNOWN_API_ERROR",
        "CoreSight could not complete this request.",
        "The local engine returned an error this build does not recognise. Nothing was changed."
      );
    }
  }

  return new CoreSightApiError(
    "ENGINE_UNREACHABLE",
    "CoreSight could not reach the local engine.",
    "The request did not complete. Nothing was changed."
  );
}

class TauriCoreSightApi implements CoreSightApi {
  private readonly invokeFn: InvokeTransport;

  public constructor(invokeFn: InvokeTransport) {
    this.invokeFn = invokeFn;
  }

  private async request<T>(
    command: string,
    args: Record<string, unknown> | undefined,
    parse: (value: unknown) => T
  ): Promise<T> {
    const handler = TAURI_HANDLERS[command];
    if (handler === undefined) {
      // No shell handler exists for this contract command in this build.
      // Failing here is deliberate: invoking a name the shell never
      // registered would surface as an opaque "command not found", and
      // inventing a payload would show the user data that is not their disk.
      throw new CoreSightApiError(
        "UNSUPPORTED_IN_THIS_BUILD",
        "This part of CoreSight is not available in this build.",
        `The local engine does not implement ${command} yet. Nothing was displayed.`
      );
    }

    let payload: unknown;
    try {
      payload = args === undefined ? await this.invokeFn(handler) : await this.invokeFn(handler, args);
    } catch (error: unknown) {
      throw toTransportError(error);
    }

    try {
      return parse(unwrapTransportEnvelope(payload, command));
    } catch (error: unknown) {
      if (error instanceof CoreSightApiError) throw error;
      throw invalidResponse("response", "could not be validated");
    }
  }

  public getGlobalStatus(): Promise<GlobalStatus> {
    return this.request(TAURI_COMMANDS.getGlobalStatus, undefined, parseGlobalStatus);
  }

  public getHomeSummary(): Promise<HomeSummary> {
    return this.request(TAURI_COMMANDS.getHomeSummary, undefined, parseHomeSummary);
  }

  public getStorage(): Promise<StorageData> {
    return this.request(TAURI_COMMANDS.getStorage, undefined, parseStorageData);
  }

  public getCleanup(): Promise<CleanupData> {
    return this.request(TAURI_COMMANDS.getCleanup, undefined, parseCleanupData);
  }

  public previewCleanup(opportunityIds: string[]): Promise<CleanupPreview> {
    if (opportunityIds.length === 0) {
      return Promise.reject(
        new CoreSightApiError(
          "NOTHING_TO_DO",
          "Choose one cleanup opportunity before previewing.",
          "There is no clean-everything path."
        )
      );
    }
    return this.request(TAURI_COMMANDS.previewPlan, { opportunityIds }, parseCleanupPreview);
  }

  public async executeCleanup(
    confirmation: CleanupConfirmation
  ): Promise<CleanupExecution> {
    if (confirmation.confirmed !== true) {
      throw new CoreSightApiError(
        "PLAN_NOT_CONFIRMED",
        "The cleanup was not confirmed and nothing was moved.",
        "Open a fresh preview before confirming."
      );
    }

    const checked = await this.request(
      TAURI_COMMANDS.confirmPlan,
      {
        planId: confirmation.planId,
        ackedWarnings: confirmation.acknowledgedWarnings
      },
      parseConfirmPlan
    );

    if (!checked.accepted) {
      throw new CoreSightApiError(
        "PLAN_REJECTED_UNSAFE",
        "CoreSight rejected this cleanup plan. Nothing was moved.",
        `${checked.rejectedCount} safety check(s) did not accept the plan. Open a fresh preview and review its warnings.`
      );
    }

    return this.request(TAURI_COMMANDS.executePlan, { planId: confirmation.planId }, (value) =>
      parseCleanupExecution(value, confirmation.planId)
    );
  }

  public getHistory(): Promise<HistoryData> {
    // The five-screen shell has no drive selector; the engine resolves the
    // active scope when no driveId is supplied. The response is still fully
    // validated, so an aggregate is never inferred from a missing field.
    return this.request(TAURI_COMMANDS.getHistory, undefined, parseHistoryData);
  }

  public listDrives(): Promise<DrivesData> {
    return this.request(TAURI_COMMANDS.listDrives, undefined, parseDrivesData);
  }

  public getDuplicateReport(): Promise<DuplicateReport> {
    return this.request(
      TAURI_COMMANDS.getDuplicateReport,
      undefined,
      (value) => parseDuplicateReport(value, "duplicateReport")
    );
  }

  public getBigFileReport(): Promise<BigFileReport> {
    return this.request(TAURI_COMMANDS.getBigFileReport, undefined, (value) =>
      parseBigFileReport(value, "bigFileReport")
    );
  }

  public getStartupReport(): Promise<StartupReport> {
    return this.request(TAURI_COMMANDS.getStartupReport, undefined, parseStartupReport);
  }

  public getDnsRanking(): Promise<DnsRanking> {
    return this.request(TAURI_COMMANDS.getDnsRanking, undefined, parseDnsRanking);
  }

  public getBlockListPlan(): Promise<BlockListPlan> {
    return this.request(TAURI_COMMANDS.getBlockListPlan, undefined, parseBlockListPlan);
  }

  public getPrivacySummary(): Promise<PrivacySummary> {
    return this.request(TAURI_COMMANDS.getPrivacySummary, undefined, parsePrivacySummary);
  }
}

export interface TauriApiOptions {
  /** Test seam; production uses the real `invoke` from `@tauri-apps/api`. */
  invoke?: InvokeTransport;
}

export function createTauriApi(options: TauriApiOptions = {}): CoreSightApi {
  return new TauriCoreSightApi(options.invoke ?? invoke);
}

/** True only inside a Tauri 2 webview. */
export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  return isTauri() || "__TAURI_INTERNALS__" in window;
}

export interface CreateApiOptions {
  /** Used only when no Tauri runtime is present (browser dev and tests). */
  mock?: MockApiOptions;
}

/**
 * Explicit transport selection. Exactly one transport is chosen for the whole
 * session; a failed Tauri call is never retried against the mock.
 */
export function createApi(options: CreateApiOptions = {}): CoreSightApi {
  if (isTauriRuntime()) return createTauriApi();
  return createMockApi(options.mock ?? {});
}

export { TauriCoreSightApi };
