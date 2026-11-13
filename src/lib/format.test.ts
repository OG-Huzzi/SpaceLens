import { afterEach, describe, expect, it } from "vitest";
import {
  formatAge,
  formatBytes,
  formatCount,
  formatDelta,
  formatNumber,
  formatPercent
} from "./format";
import { setActiveLocale, t } from "../i18n";

afterEach(() => {
  setActiveLocale("en");
});

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

describe("locale-aware formatting", () => {
  it("uses each locale's unknown word for a null byte count, never a digit", () => {
    expect(formatBytes(null)).toBe("Unknown");
    expect(formatBytes(null)).not.toMatch(/\d/);

    setActiveLocale("de");
    expect(formatBytes(null)).toBe("Unbekannt");
    expect(formatBytes(null)).not.toMatch(/\d/);
  });

  it("renders a real zero in both locales", () => {
    expect(formatBytes(0)).toBe("0 B");

    setActiveLocale("de");
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(null)).not.toBe(formatBytes(0));
  });

  it("uses the locale decimal separator and grouping", () => {
    const twelvePointFourGiB = Math.round(12.4 * 1024 ** 3);

    expect(formatBytes(twelvePointFourGiB)).toBe("12.4 GB");
    expect(formatNumber(1204)).toBe("1,204");

    setActiveLocale("de");
    expect(formatBytes(twelvePointFourGiB)).toBe("12,4 GB");
    expect(formatNumber(1204)).toBe("1.204");
  });

  it("renders a relative-time phrase fully in the active locale", () => {
    expect(formatAge(21)).toBe("21 days");
    expect(t("cleanup.card.oldestEvidence", { age: formatAge(21) })).toBe(
      "Oldest evidence: 21 days"
    );

    setActiveLocale("de");
    expect(formatAge(21)).toBe("21 Tage");
    // Regression for “Ältester Beleg: 21 days”: the unit is localised too, so
    // the whole composed phrase is German.
    expect(t("cleanup.card.oldestEvidence", { age: formatAge(21) })).toBe(
      "Ältester Beleg: 21 Tage"
    );
    expect(t("cleanup.card.oldestEvidence", { age: formatAge(21) })).not.toContain("days");
  });
});
