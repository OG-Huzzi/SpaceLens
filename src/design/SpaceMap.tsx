/**
 * SpaceMap — the proportional view of what occupies storage.
 *
 * AREA IS THE ENCODING
 * --------------------
 * A category's rectangle area is exactly its share of the observed total —
 * not approximately, and not after a visual fudge. The layout is a binary
 * slice-and-dice: each step splits a rectangle along one axis in the same
 * ratio as the values it separates, so the area of every child is its value
 * fraction of the parent's area, exactly, at every depth.
 *
 * The rectangles are laid out in a normalised 0…1 box and positioned
 * absolutely, so the geometry does not depend on the rendered pixel size.
 * The split *direction* uses the container's aspect ratio so the shapes stay
 * sensible in a wide box, but the direction never changes the areas.
 *
 * THE DRAWING IS NOT THE DATA
 * ---------------------------
 * The map is a visual explanation; the legend beside it (a real list) carries
 * the same values for keyboard and screen-reader users. That is why the
 * drawing itself is hidden from assistive technology: announcing "rectangle"
 * twice would be noise, not information.
 */

import type { CSSProperties } from "react";
import { formatBytes, formatPercent } from "../lib/format";

export interface SpaceMapItem {
  id: string;
  label: string;
  bytes: number | null;
  share: number | null;
  delta?: number | null;
  unknown?: boolean;
}

interface SizedItem extends SpaceMapItem {
  value: number;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Placed {
  item: SizedItem;
  rect: Rect;
  /** The region's area as a fraction of the whole map. */
  area: number;
}

/**
 * The aspect ratio of the full map.
 *
 * The layout is aspect-agnostic in terms of area, but the shape quality is
 * not: this value decides which axis a rectangle is split along. It must stay
 * in step with `aspect-ratio` on `.cs-spacemap--full` in `src/styles.css`.
 */
const FULL_ASPECT = 16 / 10;

/**
 * The strip is a single horizontal bar, so it is always split along its long
 * axis. Any wide aspect produces that; the exact number does not matter.
 */
const STRIP_ASPECT = 24;

/**
 * Binary slice-and-dice.
 *
 * `rect` is expressed in fractions of the map box (0…1 on both axes).
 * `aspect` is the container's width-to-height ratio, used only to choose the
 * split axis; the area of every child remains its value fraction of the
 * parent's area whatever the axis.
 */
function layout(
  items: SizedItem[],
  rect: Rect,
  aspect: number,
  out: Placed[]
): void {
  if (items.length === 0) return;
  if (items.length === 1) {
    out.push({ item: items[0], rect, area: rect.w * rect.h });
    return;
  }

  const total = items.reduce((sum, item) => sum + item.value, 0);

  // Split where the running total is closest to half: the most balanced
  // division produces the most legible map.
  let running = 0;
  let splitIndex = 1;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (let index = 1; index < items.length; index += 1) {
    running += items[index - 1].value;
    const distance = Math.abs(running - total / 2);
    if (distance < bestDistance) {
      bestDistance = distance;
      splitIndex = index;
    }
  }

  const first = items.slice(0, splitIndex);
  const second = items.slice(splitIndex);
  const firstValue = first.reduce((sum, item) => sum + item.value, 0);
  const ratio = total === 0 ? 0 : firstValue / total;

  if (rect.w * aspect >= rect.h) {
    layout(
      first,
      { x: rect.x, y: rect.y, w: rect.w * ratio, h: rect.h },
      aspect,
      out
    );
    layout(
      second,
      { x: rect.x + rect.w * ratio, y: rect.y, w: rect.w * (1 - ratio), h: rect.h },
      aspect,
      out
    );
  } else {
    layout(
      first,
      { x: rect.x, y: rect.y, w: rect.w, h: rect.h * ratio },
      aspect,
      out
    );
    layout(
      second,
      { x: rect.x, y: rect.y + rect.h * ratio, w: rect.w, h: rect.h * (1 - ratio) },
      aspect,
      out
    );
  }
}

export function SpaceMap({
  items,
  label,
  variant = "full",
  className
}: {
  items: SpaceMapItem[];
  label: string;
  variant?: "full" | "strip";
  className?: string;
}) {
  const sized: SizedItem[] = items
    .filter((item) => item.share !== null && item.share > 0.0005)
    .map((item) => ({ ...item, value: item.share ?? 0 }))
    .sort((left, right) => right.value - left.value);

  const placed: Placed[] = [];
  layout(
    sized,
    { x: 0, y: 0, w: 1, h: 1 },
    variant === "full" ? FULL_ASPECT : STRIP_ASPECT,
    placed
  );

  return (
    <div
      className={`cs-spacemap cs-spacemap--${variant}${className ? ` ${className}` : ""}`}
      title={label}
      aria-hidden="true"
      data-variant={variant}
    >
      {placed.map(({ item, rect, area }) => {
        const regionStyle: CSSProperties = {
          left: `${rect.x * 100}%`,
          top: `${rect.y * 100}%`,
          width: `${rect.w * 100}%`,
          height: `${rect.h * 100}%`
        };
        // A region only earns a label when the label fits inside it. The
        // thresholds are fractions of the whole map, so a region that is 3%
        // of the area does not get three lines of text in it.
        const showName = variant === "full" && area >= 0.018;
        const showBytes = variant === "full" && area >= 0.055 && item.bytes !== null;
        const showShare = variant === "full" && area >= 0.032 && item.share !== null;
        return (
          <span
            className={`cs-spacemap__region${
              item.unknown ? " cs-spacemap__region--unknown" : ""
            }`}
            style={regionStyle}
            key={item.id}
          >
            {showName ? <span className="cs-spacemap__name">{item.label}</span> : null}
            {showBytes ? (
              <span className="cs-spacemap__bytes cs-num">{formatBytes(item.bytes)}</span>
            ) : null}
            {showShare ? (
              <span className="cs-spacemap__share cs-num">{formatPercent(item.share)}</span>
            ) : null}
          </span>
        );
      })}
    </div>
  );
}
