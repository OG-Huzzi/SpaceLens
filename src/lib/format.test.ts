import { describe, expect, it } from "vitest";
import { formatBytes, formatCount, formatDelta, formatPercent } from "./format";

describe("unknown is not zero", () => {
  it("renders an explicit unknown phrase for every null formatter", () => {
    expect(formatBytes(null)).toBe("Unknown");
    expect(formatCount(null, "entry", "entries")).toBe("Unknown entries");
    expect(formatPercent(null)).toBe("Unknown");
    expect(formatDelta(null)).toBe("Unknown change");

    const rendered = [
      formatBytes(null),
      formatCount(null, "entry", "entries"),
      formatPercent(null),
      formatDelta(null)
    ];

    for (const value of rendered) {
      expect(value.toLowerCase()).toContain("unknown");
      // A bare zero or "0 B" would be a false observation, not a placeholder.
      expect(value).not.toMatch(/(^|\s)0(\s|$)/);
      expect(value).not.toBe("0");
    }
  });

  it("renders a real zero as a real zero, distinct from unknown", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatCount(0, "entry", "entries")).toBe("0 entries");
    expect(formatPercent(0)).toBe("0%");
    expect(formatDelta(0)).toBe("No change");

    expect(formatBytes(null)).toBe("Unknown");
    expect(formatBytes(null)).not.toBe("0 B");
  });
});
