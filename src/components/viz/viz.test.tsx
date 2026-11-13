/**
 * Tests for the visualisation components.
 *
 * The assertions that matter here are the honest ones: an unknown size must
 * never take zero area, a gap must never be drawn as a zero, and the areas
 * must really be proportional to the bytes.
 */

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { formatBytes } from "../../lib/format";
import { Treemap } from "./Treemap";
import { ProportionBar } from "./ProportionBar";
import { DeltaSpark } from "./DeltaSpark";
import { meanAspectRatio, squarify, type Rect } from "./squarify";
import type { TreemapNode } from "./treemapLayout";

interface Measured {
  x: number;
  y: number;
  width: number;
  height: number;
  area: number;
  aspect: number;
}

function measureRects(container: HTMLElement): Map<string, Measured> {
  const measured = new Map<string, Measured>();
  container.querySelectorAll("rect[data-node-id]").forEach((element) => {
    const id = element.getAttribute("data-node-id") ?? "";
    const width = Number(element.getAttribute("width"));
    const height = Number(element.getAttribute("height"));
    measured.set(id, {
      x: Number(element.getAttribute("x")),
      y: Number(element.getAttribute("y")),
      width,
      height,
      area: width * height,
      aspect: Math.max(width / height, height / width)
    });
  });
  return measured;
}

/** The layout a naive slice-and-dice treemap would have produced. */
function sliceAndDice(
  values: readonly number[],
  rect: Rect
): { id: string; rect: Rect }[] {
  const total = values.reduce((sum, value) => sum + value, 0);
  const out: { id: string; rect: Rect }[] = [];
  let x = rect.x;
  values.forEach((value, index) => {
    const width = (value / total) * rect.width;
    out.push({
      id: `item-${index}`,
      rect: { x, y: rect.y, width, height: rect.height }
    });
    x += width;
  });
  return out;
}

const FLAT_NODES: TreemapNode[] = [
  { id: "a", label: "Applications", bytes: 60, tier: "SAFE" },
  { id: "b", label: "Documents", bytes: 30 },
  { id: "c", label: "Media", bytes: 10, tier: "REVIEW" }
];

describe("Treemap", () => {
  it("lays out rectangles whose area is proportional to the bytes", () => {
    const { container } = render(
      <Treemap nodes={FLAT_NODES} width={800} height={400} />
    );
    const rects = measureRects(container);
    const total = 800 * 400;

    expect(rects.size).toBe(3);
    expect(rects.get("a")?.area ?? 0).toBeCloseTo(total * 0.6, -3);
    expect(rects.get("b")?.area ?? 0).toBeCloseTo(total * 0.3, -3);
    expect(rects.get("c")?.area ?? 0).toBeCloseTo(total * 0.1, -3);

    // Relative areas, not just absolute ones.
    const shareOfA = (rects.get("a")?.area ?? 0) / (rects.get("b")?.area ?? 1);
    expect(shareOfA).toBeCloseTo(2, 2);
  });

  it("lays rectangles out squarer than slice-and-dice", () => {
    const values = [6, 6, 4, 4, 3, 3, 2, 2];
    const rect: Rect = { x: 0, y: 0, width: 800, height: 400 };

    const squarified = squarify(
      values.map((value, index) => ({ id: `item-${index}`, value })),
      rect
    );
    const diced = sliceAndDice(values, rect);

    const squarifiedMean = meanAspectRatio(squarified.map((entry) => entry.rect));
    const dicedMean = meanAspectRatio(diced.map((entry) => entry.rect));

    expect(squarifiedMean).toBeLessThan(dicedMean);
    expect(squarifiedMean).toBeLessThan(1.6);
  });

  it("gives an unknown node a real area and marks it as unknown", () => {
    const { container } = render(
      <Treemap
        nodes={[
          { id: "known", label: "Readable", bytes: 100 },
          { id: "unknown", label: "Protected areas", bytes: null }
        ]}
        width={800}
        height={400}
      />
    );

    const unknownRect = container.querySelector('rect[data-unknown="true"]');
    expect(unknownRect).not.toBeNull();
    const measured = measureRects(container);
    const unknown = measured.get("unknown");
    expect(unknown).toBeDefined();
    // A null size must never become zero area.
    expect(unknown?.area ?? 0).toBeGreaterThan(0);

    const chart = screen.getByRole("img");
    const label = chart.getAttribute("aria-label") ?? "";
    expect(label.toLowerCase()).toContain("unknown");
  });

  it("is deterministic across input permutations", () => {
    const first = render(
      <Treemap nodes={FLAT_NODES} width={800} height={400} />
    );
    const firstRects = measureRects(first.container);

    const permuted = render(
      <Treemap
        nodes={[FLAT_NODES[2], FLAT_NODES[0], FLAT_NODES[1]]}
        width={800}
        height={400}
      />
    );
    const permutedRects = measureRects(permuted.container);

    expect(permutedRects.size).toBe(firstRects.size);
    for (const [id, measured] of firstRects) {
      const other = permutedRects.get(id);
      expect(other).toBeDefined();
      expect(other?.x).toBeCloseTo(measured.x, 6);
      expect(other?.y).toBeCloseTo(measured.y, 6);
      expect(other?.width).toBeCloseTo(measured.width, 6);
      expect(other?.height).toBeCloseTo(measured.height, 6);
    }
  });

  it("exposes a text alternative naming the largest item and the total", () => {
    render(<Treemap nodes={FLAT_NODES} width={800} height={400} />);
    const label = screen.getByRole("img").getAttribute("aria-label") ?? "";
    expect(label).toContain("Applications");
    expect(label).toContain(formatBytes(100));
  });

  it("renders without crashing on empty input", () => {
    expect(() =>
      render(<Treemap nodes={[]} width={400} height={200} />)
    ).not.toThrow();
    expect(screen.getByRole("img")).toBeInTheDocument();
  });

  it("reports a selection to the caller", async () => {
    const onSelect = vi.fn();
    const { container } = render(
      <Treemap nodes={FLAT_NODES} width={800} height={400} onSelect={onSelect} />
    );
    const target = container.querySelector('rect[data-node-id="b"]');
    expect(target).not.toBeNull();
    target?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect.mock.calls[0][0].id).toBe("b");
  });
});

describe("ProportionBar", () => {
  it("draws a hatched region for segments of unknown size", () => {
    const { container } = render(
      <ProportionBar
        segments={[
          { id: "a", label: "Applications", bytes: 100, tier: "SAFE" },
          { id: "b", label: "Protected", bytes: null }
        ]}
      />
    );

    const unknown = container.querySelector('[data-region="unknown"]');
    expect(unknown).not.toBeNull();
    const width = Number.parseFloat((unknown as HTMLElement).style.width);
    expect(width).toBeGreaterThan(0);
    expect(unknown?.className).toContain("unknown");
  });

  it("shows an unaccounted difference instead of folding it in", () => {
    const { container } = render(
      <ProportionBar
        segments={[{ id: "a", label: "Applications", bytes: 100 }]}
        totalBytes={150}
      />
    );

    const unaccounted = container.querySelector('[data-region="unaccounted"]');
    expect(unaccounted).not.toBeNull();
    const width = Number.parseFloat((unaccounted as HTMLElement).style.width);
    expect(width).toBeGreaterThan(0);
    expect(container.textContent ?? "").toContain("Unaccounted for");

    const label = screen.getByRole("img").getAttribute("aria-label") ?? "";
    expect(label).toContain("unaccounted");
  });

  it("exposes a text alternative naming the largest segment and the total", () => {
    render(
      <ProportionBar
        segments={[
          { id: "a", label: "Applications", bytes: 1000 },
          { id: "b", label: "Documents", bytes: 500 }
        ]}
      />
    );
    const label = screen.getByRole("img").getAttribute("aria-label") ?? "";
    expect(label).toContain("Applications");
    expect(label).toContain(formatBytes(1500));
  });

  it("renders without crashing on empty input", () => {
    expect(() => render(<ProportionBar segments={[]} />)).not.toThrow();
    expect(screen.getByRole("img")).toBeInTheDocument();
  });
});

describe("DeltaSpark", () => {
  it("breaks the line at an unknown point instead of drawing a zero", () => {
    const { container } = render(
      <DeltaSpark
        points={[
          { label: "scan-1", bytes: 100 },
          { label: "scan-2", bytes: null },
          { label: "scan-3", bytes: 200 },
          { label: "scan-4", bytes: 300 }
        ]}
        width={600}
        height={120}
      />
    );

    const gap = container.querySelector('[data-gap="true"]');
    expect(gap).not.toBeNull();
    const gapX = Number(gap?.getAttribute("data-x"));

    // No data point is drawn for the unknown scan.
    const unknownDot = container.querySelector('[data-point="true"][data-label="scan-2"]');
    expect(unknownDot).toBeNull();

    // The line never crosses the gap.
    const lines = Array.from(container.querySelectorAll("polyline[data-run]"));
    expect(lines.length).toBe(1);
    for (const line of lines) {
      const coordinates = (line.getAttribute("points") ?? "")
        .split(" ")
        .map((pair) => Number(pair.split(",")[0]));
      for (const x of coordinates) {
        expect(Math.abs(x - gapX)).toBeGreaterThan(0.001);
      }
    }
  });

  it("labels the last point", () => {
    const { container } = render(
      <DeltaSpark
        points={[
          { label: "scan-1", bytes: 100 },
          { label: "scan-2", bytes: 300 }
        ]}
        width={600}
        height={120}
      />
    );
    const value = container.querySelector(".cs-viz-spark__value");
    expect(value?.textContent).toContain(formatBytes(300));
  });

  it("exposes a text alternative naming the largest point and the latest value", () => {
    render(
      <DeltaSpark
        points={[
          { label: "scan-1", bytes: 100 },
          { label: "scan-2", bytes: 300 }
        ]}
        width={600}
        height={120}
      />
    );
    const label = screen.getByRole("img").getAttribute("aria-label") ?? "";
    expect(label).toContain("scan-2");
    expect(label).toContain(formatBytes(300));
  });

  it("renders without crashing on empty input", () => {
    expect(() => render(<DeltaSpark points={[]} />)).not.toThrow();
    expect(screen.getByRole("img")).toBeInTheDocument();
  });
});
