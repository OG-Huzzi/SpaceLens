/**
 * Treemap — proportional disk usage.
 *
 * Area is the encoding: a node's rectangle area is its share of the measured
 * total, so the map answers "where did the space go" at a glance. The layout
 * is squarified (see `./squarify`), because a slice-and-dice map of a 2 %
 * category is a sliver you cannot compare with a 20 % square.
 *
 * UNKNOWN IS A FIRST-CLASS STATE
 * ------------------------------
 * A node whose size was never measured gets its own hatched band with its own
 * label. It is never given zero area (that would erase it) and never merged
 * into a neighbour (that would invent a number). The band says exactly what
 * it knows: this item's size is unknown.
 *
 * KEYBOARD AND SCREEN READERS
 * ---------------------------
 * The drawing is exposed as a single `role="img"` chart with a written
 * summary, and *next to it* a real list of buttons, one per node, carrying
 * the same values in text. The list was chosen over focusable SVG regions
 * because a screen reader announces a native button inside a list item far
 * more reliably than an SVG `<rect>` with a `tabindex`; the buttons also give
 * keyboard users the same selection behaviour as a mouse click. The list is
 * visually hidden until it receives focus, so sighted keyboard users see
 * where they are.
 */

import { useId, useMemo, useRef, type CSSProperties } from "react";
import { formatBytes, formatPercent } from "../../lib/format";
import { t } from "../../i18n";
import { useMeasuredWidth } from "./useMeasuredWidth";
import {
  layoutTreemap,
  type TreemapNode,
  type TreemapTier
} from "./treemapLayout";
import "./viz.css";

export type { TreemapNode, TreemapTier };

export interface TreemapProps {
  nodes: TreemapNode[];
  width?: number;
  height?: number;
  onSelect?: (node: TreemapNode) => void;
  selectedId?: string;
}

/** A style object that may carry custom properties. */
type CssVars = CSSProperties & Record<`--${string}`, string | number>;

const DEFAULT_WIDTH = 720;
const MIN_HEIGHT = 200;
const MAX_HEIGHT = 520;

/**
 * Estimated advance width of the 12 px sans label, in pixels (≈0.55 em).
 * Text is hidden rather than clipped when it does not fit; the tier colour is
 * never hidden, so a small red region still reads as red.
 */
const LABEL_CHAR_WIDTH = 6.4;
const LABEL_LINE_HEIGHT = 15;
const VALUE_LINE_HEIGHT = 14;

interface Size {
  width: number;
  height: number;
}

function fitText(
  text: string,
  size: Size,
  charWidth: number,
  lineHeight: number
): string | null {
  const available = size.width - 8;
  if (available < 16 || size.height < lineHeight) return null;
  const maxChars = Math.floor(available / charWidth);
  if (maxChars < 3) return null;
  if (text.length <= maxChars) return text;
  return `${text.slice(0, Math.max(1, maxChars - 1))}…`;
}

function tierClass(tier: TreemapTier | undefined): string {
  if (tier === "SAFE") return "cs-viz-treemap__rect--safe";
  if (tier === "REVIEW") return "cs-viz-treemap__rect--review";
  if (tier === "OFF_LIMITS") return "cs-viz-treemap__rect--offlimits";
  return "cs-viz-treemap__rect--neutral";
}

export function Treemap({
  nodes,
  width,
  height,
  onSelect,
  selectedId
}: TreemapProps) {
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
  const chartHeight = Math.max(
    1,
    height ??
      Math.round(Math.min(Math.max(chartWidth * 0.56, MIN_HEIGHT), MAX_HEIGHT))
  );

  const layout = useMemo(
    () => layoutTreemap(nodes, { x: 0, y: 0, width: chartWidth, height: chartHeight }),
    [nodes, chartWidth, chartHeight]
  );

  const total = layout.knownTotal;
  const topShare =
    layout.top !== null && total > 0 ? layout.top.value / total : null;

  const summaryParts: string[] = [`Treemap of ${layout.leaves.length} items.`];
  if (layout.top !== null) {
    summaryParts.push(
      `Largest ${layout.top.node.label}: ${formatBytes(layout.top.value)}` +
        (topShare === null ? "" : ` (${formatPercent(topShare)})`)
    );
    summaryParts.push(`Measured total ${formatBytes(total)}.`);
  } else {
    summaryParts.push("No measured sizes.");
  }
  if (layout.unknownCount > 0) {
    summaryParts.push(
      `${layout.unknownCount} item${layout.unknownCount === 1 ? "" : "s"} of unknown size, shown hatched.`
    );
  }
  const summary = summaryParts.join(" ");

  const listItems = layout.leaves.map((node) => {
    const share = node.bytes !== null && total > 0 ? node.bytes / total : null;
    const tierText = node.tier === undefined ? "" : `, ${node.tier}`;
    const segmentText = node.segment === undefined ? "" : `, ${node.segment}`;
    const text =
      node.bytes === null
        ? `${node.label}: ${t("state.unknown")}`
        : `${node.label}: ${formatBytes(node.bytes)}${
            share === null ? "" : `, ${formatPercent(share)}`
          }${tierText}${segmentText}`;
    return { node, text };
  });

  return (
    <div className="cs-viz-treemap" ref={containerRef}>
      <svg
        className="cs-viz-treemap__svg"
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

        {layout.drawables.map((drawable) => {
          if (drawable.kind === "frame") {
            const { node, rect } = drawable;
            const label = fitText(
              node.label,
              { width: rect.width, height: rect.height },
              LABEL_CHAR_WIDTH,
              LABEL_LINE_HEIGHT
            );
            return (
              <g key={`frame-${node.id}`} className="cs-viz-treemap__frame-group">
                <rect
                  className="cs-viz-treemap__frame"
                  x={rect.x}
                  y={rect.y}
                  width={rect.width}
                  height={rect.height}
                  rx={4}
                />
                {label !== null ? (
                  <text
                    className="cs-viz-treemap__frame-label"
                    x={rect.x + 6}
                    y={rect.y + 13}
                  >
                    {label}
                  </text>
                ) : null}
              </g>
            );
          }

          if (drawable.kind === "band") {
            const node = drawable.nodes[0];
            const { rect } = drawable;
            const label = fitText(
              `${node.label} · ${t("state.unknown")}`,
              { width: rect.width, height: rect.height },
              LABEL_CHAR_WIDTH,
              LABEL_LINE_HEIGHT
            );
            const hatchStyle: CssVars = {
              "--cs-viz-hatch": `url(#${hatchId})`
            };
            return (
              <g key={`band-${node.id}`} className="cs-viz-treemap__band-group">
                <rect
                  className="cs-viz-treemap__band"
                  style={hatchStyle}
                  x={rect.x}
                  y={rect.y}
                  width={rect.width}
                  height={rect.height}
                  data-node-id={node.id}
                  data-unknown="true"
                />
                {label !== null ? (
                  <text
                    className="cs-viz-treemap__band-label"
                    x={rect.x + 6}
                    y={rect.y + 13}
                  >
                    {label}
                  </text>
                ) : null}
              </g>
            );
          }

          const { node, rect } = drawable;
          const selected = selectedId !== undefined && selectedId === node.id;
          const onFill = node.tier !== undefined;
          const label = fitText(
            node.label,
            { width: rect.width, height: rect.height },
            LABEL_CHAR_WIDTH,
            LABEL_LINE_HEIGHT
          );
          const valueText =
            node.bytes === null ? t("state.unknown") : formatBytes(node.bytes);
          const value = fitText(
            valueText,
            { width: rect.width, height: rect.height - LABEL_LINE_HEIGHT },
            LABEL_CHAR_WIDTH * 0.85,
            VALUE_LINE_HEIGHT
          );
          const segmentText = fitText(
            node.segment ?? "",
            {
              width: rect.width,
              height: rect.height - LABEL_LINE_HEIGHT * 2
            },
            LABEL_CHAR_WIDTH * 0.85,
            VALUE_LINE_HEIGHT
          );

          return (
            <g key={`leaf-${node.id}`} className="cs-viz-treemap__leaf">
              <rect
                className={`cs-viz-treemap__rect ${tierClass(node.tier)}${
                  selected ? " cs-viz-treemap__rect--selected" : ""
                }`}
                x={rect.x}
                y={rect.y}
                width={rect.width}
                height={rect.height}
                rx={2}
                data-node-id={node.id}
                data-tier={node.tier ?? "none"}
                onClick={
                  onSelect === undefined ? undefined : () => onSelect(node)
                }
              />
              {label !== null ? (
                <text
                  className={`cs-viz-treemap__label${
                    onFill ? " cs-viz-treemap__label--on-fill" : ""
                  }`}
                  x={rect.x + 5}
                  y={rect.y + 12}
                >
                  {label}
                </text>
              ) : null}
              {value !== null ? (
                <text
                  className={`cs-viz-treemap__value${
                    onFill ? " cs-viz-treemap__value--on-fill" : ""
                  }`}
                  x={rect.x + 5}
                  y={rect.y + 12 + VALUE_LINE_HEIGHT}
                >
                  {value}
                </text>
              ) : null}
              {segmentText !== null && node.segment !== undefined ? (
                <text
                  className={`cs-viz-treemap__segment${
                    onFill ? " cs-viz-treemap__value--on-fill" : ""
                  }`}
                  x={rect.x + 5}
                  y={rect.y + 12 + VALUE_LINE_HEIGHT * 2}
                >
                  {segmentText}
                </text>
              ) : null}
            </g>
          );
        })}
      </svg>

      <ul className="cs-viz-sr-list">
        {listItems.map((item) => (
          <li key={item.node.id}>
            {onSelect === undefined ? (
              <span className="cs-viz-sr-list__item">{item.text}</span>
            ) : (
              <button
                type="button"
                className="cs-viz-sr-list__button"
                aria-pressed={
                  selectedId !== undefined && selectedId === item.node.id
                }
                onClick={() => onSelect(item.node)}
              >
                {item.text}
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
