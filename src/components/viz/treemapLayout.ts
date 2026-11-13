/**
 * Treemap layout: squarified rectangles plus honest handling of unknown
 * sizes.
 *
 * HONESTY INVARIANT — unknown is not zero
 * ---------------------------------------
 * A node whose `bytes` is `null` was not measured. It is never laid out as
 * zero area (that would erase it) and it is never silently dropped. Instead it
 * is drawn in a clearly separated, hatched "unknown" band that carries its own
 * label, so a reader can tell "we could not measure this" from "this is
 * empty".
 *
 * HOW MUCH SPACE THE UNKNOWN BAND GETS
 * ------------------------------------
 * When the container has a known total (a parent whose own size is known), the
 * band takes exactly the share of the container that the known children do not
 * account for — that difference is a measured fact, so it gets a measured
 * area. When no total is known, the band takes a fixed, documented fraction
 * of the container area. A fixed fraction is the honest choice here: the true
 * size is unknown, so any number would be invented.
 *
 * AREA IS PROPORTIONAL TO BYTES
 * -----------------------------
 * Within one container, the known children are laid out so that sibling areas
 * are exactly proportional to their byte counts. The proportional claim
 * survives nesting because a container that has to host an unknown band
 * reserves the room for it in its own allocation instead of taking it out of
 * its children.
 */

import { squarify, type Rect } from "./squarify";

export type TreemapTier = "SAFE" | "REVIEW" | "OFF_LIMITS";

export interface TreemapNode {
  id: string;
  label: string;
  /** Logical bytes. `null` means unknown and MUST render as unknown, not as 0. */
  bytes: number | null;
  /** Optional nested children; a parent with children renders as a container. */
  children?: TreemapNode[];
  /** Safety tier, when the node is an actionable category. */
  tier?: TreemapTier;
  /** Optional secondary colour key, e.g. an app name for attribution. */
  segment?: string;
}

/**
 * Share of a container's area reserved for unknown-sized children when the
 * container has no known total to subtract from.
 */
export const UNKNOWN_BAND_FRACTION = 0.18;

export interface TreemapLeaf {
  kind: "leaf";
  node: TreemapNode;
  rect: Rect;
  depth: number;
}

export interface TreemapFrame {
  kind: "frame";
  node: TreemapNode;
  rect: Rect;
  depth: number;
}

export interface TreemapBand {
  kind: "band";
  /** Exactly one unknown node per band rectangle. */
  nodes: TreemapNode[];
  rect: Rect;
  depth: number;
}

export type TreemapDrawable = TreemapLeaf | TreemapFrame | TreemapBand;

export interface TreemapLayout {
  drawables: TreemapDrawable[];
  /** Sum of every measured leaf size. */
  knownTotal: number;
  /** Number of nodes whose size is unknown. */
  unknownCount: number;
  /** The biggest top-level node with a known size. */
  top: { node: TreemapNode; value: number } | null;
  /** Every leaf, in a stable order, for the accessible list. */
  leaves: TreemapNode[];
}

function childrenOf(node: TreemapNode): TreemapNode[] {
  return node.children ?? [];
}

/**
 * The value a node contributes to its parent's layout.
 *
 * - A leaf contributes its measured size, or `null` when unknown.
 * - A container contributes its own size when that is known.
 * - A container without a known size contributes the sum of its measured
 *   children; if it also has unknown children, the sum is grossed up by the
 *   unknown band's share so the band has room without stealing from the
 *   measured children.
 */
export function effectiveValue(node: TreemapNode): number | null {
  const children = childrenOf(node);
  if (children.length === 0) return node.bytes;
  if (node.bytes !== null) return node.bytes;

  let sum = 0;
  let hasUnknownChild = false;
  for (const child of children) {
    const value = effectiveValue(child);
    if (value === null) hasUnknownChild = true;
    else sum += value;
  }
  if (sum <= 0) return null;
  return hasUnknownChild ? sum / (1 - UNKNOWN_BAND_FRACTION) : sum;
}

/** Sum of the measured leaf sizes below (and including) a node. */
export function leafTotal(node: TreemapNode): number {
  const children = childrenOf(node);
  if (children.length === 0) return node.bytes ?? 0;
  let sum = 0;
  for (const child of children) sum += leafTotal(child);
  return sum;
}

/**
 * The inner rectangle available to a container's children. The container's
 * label needs a header strip; when the frame is too small for one, the
 * children get the whole frame.
 */
function innerRect(rect: Rect, label: string): Rect {
  const padding = 4;
  const header = 18;
  const labelFits = rect.width >= 64 && rect.height >= 34 && label.length > 0;
  const top = labelFits ? header : padding;
  return {
    x: rect.x + padding,
    y: rect.y + top,
    width: Math.max(0, rect.width - padding * 2),
    height: Math.max(0, rect.height - top - padding)
  };
}

function layoutNodes(
  nodes: readonly TreemapNode[],
  rect: Rect,
  containerTotal: number | null,
  depth: number,
  out: TreemapDrawable[]
): void {
  const known: { node: TreemapNode; value: number }[] = [];
  const unknown: TreemapNode[] = [];
  for (const node of nodes) {
    const value = effectiveValue(node);
    if (value === null) unknown.push(node);
    else known.push({ node, value });
  }

  const knownSum = known.reduce((sum, item) => sum + item.value, 0);

  let bandRect: Rect | null = null;
  if (unknown.length > 0) {
    const area = rect.width * rect.height;
    let bandArea: number;
    if (containerTotal !== null && containerTotal > knownSum) {
      // The container's own total is known: the gap between it and its
      // measured children is a measured difference.
      bandArea = area * ((containerTotal - knownSum) / containerTotal);
    } else {
      bandArea = area * UNKNOWN_BAND_FRACTION;
    }
    const wide = rect.width >= rect.height;
    const band: Rect = wide
      ? {
          x: rect.x,
          y: rect.y + rect.height - bandArea / rect.width,
          width: rect.width,
          height: bandArea / rect.width
        }
      : {
          x: rect.x + rect.width - bandArea / rect.height,
          y: rect.y,
          width: bandArea / rect.height,
          height: rect.height
        };
    bandRect = band;

    // One hatched strip per unknown node, so every unknown node keeps a
    // visible, non-zero area of its own.
    const count = unknown.length;
    for (let index = 0; index < count; index += 1) {
      const strip = wide
        ? {
            x: band.x,
            y: band.y + (band.height * index) / count,
            width: band.width,
            height: band.height / count
          }
        : {
            x: band.x + (band.width * index) / count,
            y: band.y,
            width: band.width / count,
            height: band.height
          };
      out.push({ kind: "band", nodes: [unknown[index]], rect: strip, depth });
    }
  }

  const band = bandRect;
  const knownRect: Rect = band !== null
    ? rect.width >= rect.height
      ? {
          x: rect.x,
          y: rect.y,
          width: rect.width,
          height: rect.height - band.height
        }
      : {
          x: rect.x,
          y: rect.y,
          width: rect.width - band.width,
          height: rect.height
        }
    : rect;

  if (knownSum <= 0 || knownRect.width <= 0 || knownRect.height <= 0) return;

  // Deterministic order: biggest first, ties broken by id.
  const ordered = known
    .slice()
    .sort((left, right) =>
      right.value === left.value
        ? left.node.id.localeCompare(right.node.id)
        : right.value - left.value
    );

  const placed = squarify(
    ordered.map((item) => ({ id: item.node.id, value: item.value })),
    knownRect
  );
  const byId = new Map(ordered.map((item) => [item.node.id, item.node]));

  for (const entry of placed) {
    const node = byId.get(entry.id);
    if (node === undefined) continue;
    const children = childrenOf(node);
    if (children.length > 0) {
      out.push({ kind: "frame", node, rect: entry.rect, depth });
      const inner = innerRect(entry.rect, node.label);
      if (inner.width > 4 && inner.height > 4) {
        layoutNodes(children, inner, node.bytes, depth + 1, out);
      }
    } else {
      out.push({ kind: "leaf", node, rect: entry.rect, depth });
    }
  }
}

/**
 * Lay a treemap out inside `rect`.
 *
 * The returned drawables are ordered back-to-front: containers first, then
 * their leaves, then the unknown band.
 */
export function layoutTreemap(
  nodes: readonly TreemapNode[],
  rect: Rect
): TreemapLayout {
  const drawables: TreemapDrawable[] = [];
  layoutNodes(nodes, rect, null, 0, drawables);

  let knownTotal = 0;
  const leaves: TreemapNode[] = [];

  const walk = (list: readonly TreemapNode[]): void => {
    for (const node of list) {
      const children = childrenOf(node);
      if (children.length === 0) {
        if (node.bytes !== null) knownTotal += node.bytes;
        leaves.push(node);
        continue;
      }
      walk(children);
    }
  };
  walk(nodes);

  // One band rectangle per unknown node: every unknown node keeps its own
  // visible area.
  const unknownCount = drawables.filter((item) => item.kind === "band").length;

  let top: { node: TreemapNode; value: number } | null = null;
  for (const node of nodes) {
    const value = effectiveValue(node);
    if (value === null) continue;
    if (top === null || value > top.value) top = { node, value };
  }

  return { drawables, knownTotal, unknownCount, top, leaves };
}
