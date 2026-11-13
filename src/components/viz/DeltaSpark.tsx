/**
 * DeltaSpark — how a category's size moved across scans.
 *
 * A compact line-and-area chart with no axes: a clear baseline, the value of
 * the latest scan, and nothing else competing for attention.
 *
 * UNKNOWN IS A GAP, NOT A ZERO
 * ----------------------------
 * A scan that could not measure the category breaks the line. Drawing a line
 * across the gap would assert a value nobody measured, and drawing the point
 * at the baseline would claim the category was empty. The gap is marked with a
 * dashed tick and counted in the text alternative.
 */

import { useId, useMemo, useRef, type CSSProperties } from "react";
import { formatBytes } from "../../lib/format";
import { t } from "../../i18n";
import { useMeasuredWidth } from "./useMeasuredWidth";
import "./viz.css";

export interface DeltaPoint {
  label: string;
  bytes: number | null;
}

export interface DeltaSparkProps {
  points: DeltaPoint[];
  height?: number;
  /**
   * Optional explicit width. Without it the chart measures its container and
   * falls back to a sensible default when the container cannot be measured.
   */
  width?: number;
  className?: string;
}

/** A style object that may carry custom properties. */
type CssVars = CSSProperties & Record<`--${string}`, string | number>;

const DEFAULT_WIDTH = 640;
const DEFAULT_HEIGHT = 96;
const PAD_X = 10;
const PAD_TOP = 20;
const PAD_BOTTOM = 12;

interface Plotted {
  index: number;
  label: string;
  bytes: number;
  x: number;
  y: number;
}

interface Run {
  /** Indices into the original point list. */
  indices: number[];
  points: Plotted[];
}

export function DeltaSpark({
  points,
  height = DEFAULT_HEIGHT,
  width,
  className
}: DeltaSparkProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const measured = useMeasuredWidth(containerRef, 0);
  const reactId = useId();
  const hatchId = `cs-viz-hatch-${reactId.replace(/[^a-zA-Z0-9_-]/g, "")}`;

  const chartWidth = Math.max(
    1,
    width === undefined
      ? measured > 0
        ? measured
        : DEFAULT_WIDTH
      : measured > 0
        ? Math.min(width, measured)
        : width
  );
  const chartHeight = Math.max(40, height);

  const model = useMemo(() => {
    const known = points
      .map((point, index) => ({ point, index }))
      .filter((entry) => entry.point.bytes !== null);
    const max = known.reduce(
      (highest, entry) => Math.max(highest, entry.point.bytes ?? 0),
      0
    );
    const plotHeight = Math.max(1, chartHeight - PAD_TOP - PAD_BOTTOM);
    const baselineY = chartHeight - PAD_BOTTOM;
    const span = Math.max(1, chartWidth - PAD_X * 2);

    const xFor = (index: number): number =>
      points.length <= 1
        ? chartWidth / 2
        : PAD_X + (index / (points.length - 1)) * span;
    const yFor = (bytes: number): number =>
      max > 0 ? baselineY - (bytes / max) * plotHeight : baselineY;

    const runs: Run[] = [];
    let current: Run = { indices: [], points: [] };
    points.forEach((point, index) => {
      if (point.bytes === null) {
        if (current.indices.length > 0) runs.push(current);
        current = { indices: [], points: [] };
        return;
      }
      const plotted: Plotted = {
        index,
        label: point.label,
        bytes: point.bytes,
        x: xFor(index),
        y: yFor(point.bytes)
      };
      current.indices.push(index);
      current.points.push(plotted);
    });
    if (current.indices.length > 0) runs.push(current);

    const gaps = points
      .map((point, index) => ({ point, index }))
      .filter((entry) => entry.point.bytes === null)
      .map((entry) => ({ index: entry.index, label: entry.point.label, x: xFor(entry.index) }));

    const last = points.length > 0 ? points[points.length - 1] : null;
    const plotted = runs.flatMap((run) => run.points);
    const lastPlotted = plotted.length > 0 ? plotted[plotted.length - 1] : null;
    const largest = known.reduce<{ point: DeltaPoint; bytes: number } | null>(
      (best, entry) => {
        const bytes = entry.point.bytes ?? 0;
        if (best === null || bytes > best.bytes) {
          return { point: entry.point, bytes };
        }
        return best;
      },
      null
    );

    return { runs, gaps, last, lastPlotted, largest, max, baselineY, xFor };
  }, [points, chartWidth, chartHeight]);

  const summaryParts: string[] = [
    `Change over ${points.length} scan${points.length === 1 ? "" : "s"}.`
  ];
  if (model.last !== null) {
    summaryParts.push(
      `Latest ${model.last.label}: ${
        model.last.bytes === null ? t("state.unknown") : formatBytes(model.last.bytes)
      }.`
    );
  }
  if (model.largest !== null) {
    summaryParts.push(
      `Largest observed ${model.largest.point.label}: ${formatBytes(model.largest.bytes)}.`
    );
  }
  if (model.gaps.length > 0) {
    summaryParts.push(
      `${model.gaps.length} scan${model.gaps.length === 1 ? "" : "s"} of unknown size, shown as a gap.`
    );
  }
  const summary = summaryParts.join(" ");

  const hatchStyle: CssVars = { "--cs-viz-hatch": `url(#${hatchId})` };

  return (
    <div
      className={`cs-viz-spark${className === undefined ? "" : ` ${className}`}`}
      ref={containerRef}
    >
      <svg
        className="cs-viz-spark__svg"
        role="img"
        aria-label={summary}
        width={chartWidth}
        height={chartHeight}
        viewBox={`0 0 ${chartWidth} ${chartHeight}`}
        preserveAspectRatio="xMinYMin meet"
      >
        <defs>
          <pattern
            id={hatchId}
            patternUnits="userSpaceOnUse"
            width={8}
            height={8}
            patternTransform="rotate(45)"
          >
            <line className="cs-viz-hatch__line" x1={0} y1={0} x2={0} y2={8} />
          </pattern>
        </defs>

        <line
          className="cs-viz-spark__baseline"
          x1={PAD_X / 2}
          y1={model.baselineY}
          x2={chartWidth - PAD_X / 2}
          y2={model.baselineY}
        />

        {model.runs.map((run) =>
          run.points.length > 1 ? (
            <path
              key={`area-${run.indices.join("-")}`}
              className="cs-viz-spark__area"
              d={`M ${run.points[0].x} ${model.baselineY} ${run.points
                .map((point) => `L ${point.x} ${point.y}`)
                .join(" ")} L ${run.points[run.points.length - 1].x} ${
                model.baselineY
              } Z`}
            />
          ) : null
        )}

        {model.runs.map((run) =>
          run.points.length > 1 ? (
            <polyline
              key={`line-${run.indices.join("-")}`}
              className="cs-viz-spark__line"
              data-run={run.indices.join("-")}
              points={run.points.map((point) => `${point.x},${point.y}`).join(" ")}
            />
          ) : null
        )}

        {model.runs.flatMap((run) =>
          run.points.map((point) => (
            <circle
              key={`dot-${point.index}`}
              className="cs-viz-spark__dot"
              cx={point.x}
              cy={point.y}
              r={2.5}
              data-point="true"
              data-label={point.label}
            />
          ))
        )}

        {model.gaps.map((gap) => (
          <line
            key={`gap-${gap.index}`}
            className="cs-viz-spark__gap"
            data-gap="true"
            data-label={gap.label}
            data-x={gap.x}
            x1={gap.x}
            y1={model.baselineY}
            x2={gap.x}
            y2={model.baselineY - 10}
          />
        ))}

        {model.runs.length === 0 ? (
          <rect
            className="cs-viz-spark__empty"
            style={hatchStyle}
            x={PAD_X / 2}
            y={PAD_TOP / 2}
            width={Math.max(0, chartWidth - PAD_X)}
            height={Math.max(0, chartHeight - PAD_TOP - PAD_BOTTOM / 2)}
          />
        ) : null}

        {model.last !== null ? (
          <text
            className="cs-viz-spark__value"
            x={chartWidth - PAD_X / 2}
            y={
              model.last.bytes === null
                ? model.baselineY - 14
                : Math.max(PAD_TOP - 4, (model.lastPlotted?.y ?? model.baselineY) - 8)
            }
            textAnchor="end"
          >
            {model.last.bytes === null
              ? t("state.unknown")
              : formatBytes(model.last.bytes)}
          </text>
        ) : null}
      </svg>
    </div>
  );
}
