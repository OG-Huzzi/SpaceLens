/**
 * ProportionBar — one labelled bar of a disk's composition.
 *
 * WHAT THE BAR CLAIMS
 * -------------------
 * Each measured segment gets its true share of the *known* total. The unknown
 * share is drawn as a separate hatched region, so the bar never implies that
 * everything on the disk was measured. When the caller supplies a total that
 * does not match the sum of the segments, the difference is drawn as an
 * "unaccounted" region and named in the legend — it is never folded into a
 * neighbour, which would invent a number nobody measured.
 *
 * The bar is a real element list, so every region is announced and every
 * segment is reachable; the legend repeats the same values in text.
 */

import { formatBytes, formatPercent } from "../../lib/format";
import { t } from "../../i18n";
import type { TreemapTier } from "./treemapLayout";
import "./viz.css";

export interface ProportionSegment {
  id: string;
  label: string;
  bytes: number | null;
  tier?: TreemapTier;
}

export interface ProportionBarProps {
  segments: ProportionSegment[];
  totalBytes?: number | null;
  showLegend?: boolean;
}

/**
 * Share of the bar reserved for segments whose size is unknown. The true
 * share is unknowable, so the bar shows a fixed, clearly marked region rather
 * than an invented percentage.
 */
const UNKNOWN_SHARE = 0.18;

function tierClass(tier: TreemapTier | undefined): string {
  if (tier === "SAFE") return "cs-viz-proportion__segment--safe";
  if (tier === "REVIEW") return "cs-viz-proportion__segment--review";
  if (tier === "OFF_LIMITS") return "cs-viz-proportion__segment--offlimits";
  return "cs-viz-proportion__segment--neutral";
}

export function ProportionBar({
  segments,
  totalBytes,
  showLegend = true
}: ProportionBarProps) {
  const known = segments.filter((segment) => segment.bytes !== null);
  const unknown = segments.filter((segment) => segment.bytes === null);
  const knownTotal = known.reduce(
    (sum, segment) => sum + (segment.bytes ?? 0),
    0
  );
  const total = totalBytes ?? null;

  // Bytes the reported total holds that no segment claims.
  const difference = total === null ? null : total - knownTotal;
  const unaccountedBytes = difference !== null && difference > 0 ? difference : null;
  const inconsistent = difference !== null && difference < 0;

  const unknownShare = unknown.length > 0 ? UNKNOWN_SHARE : 0;
  const unaccountedShare =
    unaccountedBytes !== null && total !== null && total > 0
      ? Math.min(unaccountedBytes / total, 1 - unknownShare)
      : 0;
  const knownShare = Math.max(0, 1 - unknownShare - unaccountedShare);

  const top = known.reduce<ProportionSegment | null>((best, segment) => {
    if (best === null) return segment;
    return (segment.bytes ?? 0) > (best.bytes ?? 0) ? segment : best;
  }, null);

  const summaryParts: string[] = [
    `Proportion bar of ${segments.length} segment${segments.length === 1 ? "" : "s"}.`
  ];
  if (top !== null && knownTotal > 0) {
    summaryParts.push(
      `Largest ${top.label}: ${formatBytes(top.bytes)} (${formatPercent(
        (top.bytes ?? 0) / knownTotal
      )})`
    );
  } else {
    summaryParts.push("No measured sizes.");
  }
  summaryParts.push(`Measured total ${formatBytes(knownTotal)}.`);
  if (total !== null) {
    summaryParts.push(`Reported total ${formatBytes(total)}.`);
  }
  if (unknown.length > 0) {
    summaryParts.push(
      `${unknown.length} segment${unknown.length === 1 ? "" : "s"} of unknown size, shown hatched.`
    );
  }
  if (unaccountedBytes !== null) {
    summaryParts.push(
      `${formatBytes(unaccountedBytes)} unaccounted for: the reported total holds bytes no segment claims.`
    );
  }
  if (inconsistent) {
    summaryParts.push(
      "The segments add up to more than the reported total; the numbers disagree."
    );
  }
  const summary = summaryParts.join(" ");

  return (
    <div className="cs-viz-proportion">
      <div className="cs-viz-proportion__bar" role="img" aria-label={summary}>
        {known.map((segment) => {
          const share = knownTotal > 0 ? (segment.bytes ?? 0) / knownTotal : 0;
          return (
            <span
              key={segment.id}
              className={`cs-viz-proportion__segment ${tierClass(segment.tier)}`}
              style={{ width: `${(share * knownShare * 100).toFixed(4)}%` }}
              data-segment-id={segment.id}
              data-region="segment"
            />
          );
        })}
        {unknown.length > 0 ? (
          <span
            className="cs-viz-proportion__segment cs-viz-proportion__segment--unknown"
            style={{ width: `${(unknownShare * 100).toFixed(4)}%` }}
            data-region="unknown"
          />
        ) : null}
        {unaccountedBytes !== null ? (
          <span
            className="cs-viz-proportion__segment cs-viz-proportion__segment--unaccounted"
            style={{ width: `${(unaccountedShare * 100).toFixed(4)}%` }}
            data-region="unaccounted"
          />
        ) : null}
      </div>

      {showLegend ? (
        <ul className="cs-viz-proportion__legend">
          {known.map((segment) => {
            const share = knownTotal > 0 ? (segment.bytes ?? 0) / knownTotal : null;
            return (
              <li key={segment.id}>
                <span
                  className={`cs-viz-proportion__swatch ${tierClass(segment.tier)}`}
                  aria-hidden="true"
                />
                <span className="cs-viz-proportion__legend-label">
                  {segment.label}
                </span>
                <span className="cs-viz-proportion__legend-value cs-num">
                  {formatBytes(segment.bytes)}
                </span>
                <span className="cs-viz-proportion__legend-share cs-num">
                  {share === null ? t("state.unknown") : formatPercent(share)}
                </span>
              </li>
            );
          })}
          {unknown.length > 0 ? (
            <li className="cs-viz-proportion__legend--unknown">
              <span
                className="cs-viz-proportion__swatch cs-viz-proportion__segment--unknown"
                aria-hidden="true"
              />
              <span className="cs-viz-proportion__legend-label">
                {unknown.map((segment) => segment.label).join(", ")}
              </span>
              <span className="cs-viz-proportion__legend-value">
                {t("state.unknown")}
              </span>
            </li>
          ) : null}
          {unaccountedBytes !== null ? (
            <li className="cs-viz-proportion__legend--unaccounted">
              <span
                className="cs-viz-proportion__swatch cs-viz-proportion__segment--unaccounted"
                aria-hidden="true"
              />
              <span className="cs-viz-proportion__legend-label">
                Unaccounted for
              </span>
              <span className="cs-viz-proportion__legend-value cs-num">
                {formatBytes(unaccountedBytes)}
              </span>
            </li>
          ) : null}
          {inconsistent ? (
            <li className="cs-viz-proportion__legend--warning">
              The segments add up to more than the reported total.
            </li>
          ) : null}
        </ul>
      ) : null}
    </div>
  );
}
