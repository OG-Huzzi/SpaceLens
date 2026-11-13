/**
 * Ambient scan indicator — the "widget" platform feature.
 *
 * READ-ONLY BY DESIGN. This strip renders state and nothing else: no start,
 * pause, cancel, or dismiss control, because "actions live in Cleanup only".
 * A control here would be a cleanup/scan action reachable from every screen,
 * which is exactly the rule the app refuses to break. If a cancel affordance is
 * ever wanted, it belongs in the Cleanup screen, not in this strip.
 *
 * ACCESSIBILITY. The strip is a polite live region (`role="status"`,
 * `aria-live="polite"`, `aria-atomic="true"`) and is never focusable: it has no
 * tabindex, no interactive descendants, and never calls focus(). Screen readers
 * announce changes when they settle; keyboard focus is never moved.
 *
 * HONESTY. The contract exposes no "percent complete", so none is invented.
 * While scanning, the strip reports the counts the engine actually provides
 * (entries seen, current area, ETA when known) and draws an indeterminate meter.
 *
 * FRESHNESS. The strip reads `getGlobalStatus()` once per mount. The contract
 * has no subscription or push channel, so live progress during a running scan
 * needs the Phase 9 event bridge; polling here would replace the strip with
 * "reading…" on every tick and teach nothing.
 */

import { type CoreSightApi, type GlobalStatus, type ScanState } from "../api";
import { useApiData, type AsyncState } from "../hooks/useApiData";
import { formatBytes, formatDateTime } from "../lib/format";
import { t, tCount } from "../i18n";

type ScanTone = ScanState | "loading" | "unavailable";

function seenText(count: number | null): string {
  if (count === null) return t("state.unknown");
  return tCount(count, "unit.entry.one", "unit.entry.other");
}

function describe(state: AsyncState<GlobalStatus>): {
  text: string;
  tone: ScanTone;
  scanning: boolean;
  advanced: string | null;
} {
  if (state.status === "loading") {
    return { text: t("scan.loading"), tone: "loading", scanning: false, advanced: null };
  }
  if (state.status === "error") {
    return { text: t("scan.unavailable"), tone: "unavailable", scanning: false, advanced: null };
  }

  const scan = state.data.scan;
  const advanced =
    scan.scanId === null
      ? null
      : scan.bytesSeen === null
        ? t("scan.advancedBytesUnknown", { id: scan.scanId })
        : t("scan.advanced", { bytes: formatBytes(scan.bytesSeen), id: scan.scanId });

  switch (scan.state) {
    case "idle":
      return { text: t("scan.idle"), tone: "idle", scanning: false, advanced };
    case "scanning": {
      const seen = seenText(scan.filesSeen);
      return {
        text: scan.currentArea
          ? t("scan.scanningInArea", { seen, area: scan.currentArea })
          : t("scan.scanning", { seen }),
        tone: "scanning",
        scanning: true,
        advanced
      };
    }
    case "failed":
      return { text: t("scan.failed"), tone: "failed", scanning: false, advanced };
    case "partial":
      return {
        text: t("scan.partial", {
          count:
            scan.coverage.unreadableEntries === null
              ? t("state.unknown")
              : seenText(scan.coverage.unreadableEntries)
        }),
        tone: "partial",
        scanning: false,
        advanced
      };
    case "complete":
      return {
        text: t("scan.complete", {
          time:
            scan.completedAt === null ? t("state.unknown") : formatDateTime(scan.completedAt)
        }),
        tone: "complete",
        scanning: false,
        advanced
      };
    default:
      // Unrecognised state from an incompatible engine: say so, never guess.
      return { text: t("scan.unavailable"), tone: "unavailable", scanning: false, advanced };
  }
}

export function ScanStatusStrip({
  api,
  showAdvancedDetails = false
}: {
  api: CoreSightApi;
  showAdvancedDetails?: boolean;
}) {
  const state = useApiData(() => api.getGlobalStatus(), [api]);
  const view = describe(state);

  return (
    <div
      className={`scan-strip scan-strip--${view.tone}`}
      role="status"
      aria-live="polite"
      aria-atomic="true"
      aria-label={t("scan.label")}
      data-scan-state={view.tone}
    >
      <span className="scan-strip__dot" aria-hidden="true" />
      <span className="scan-strip__text">{view.text}</span>
      {view.scanning ? (
        <span className="scan-strip__meter" aria-hidden="true">
          <span />
        </span>
      ) : null}
      {showAdvancedDetails && view.advanced ? (
        <span className="scan-strip__detail">{view.advanced}</span>
      ) : null}
    </div>
  );
}
