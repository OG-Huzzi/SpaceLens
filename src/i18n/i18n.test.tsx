import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { de, en, type MessageKey } from "./catalogue";
import { getActiveLocale, setActiveLocale, t, tCount } from "./index";

afterEach(() => {
  setActiveLocale("en");
  vi.restoreAllMocks();
});

describe("message catalogue", () => {
  it("ships the same keys in every supported locale, all non-empty", () => {
    expect(Object.keys(de).sort()).toEqual(Object.keys(en).sort());
    for (const [key, value] of Object.entries(de)) {
      expect(value.trim(), `German message ${key}`).not.toBe("");
    }
    for (const [key, value] of Object.entries(en)) {
      expect(value.trim(), `English message ${key}`).not.toBe("");
    }
  });

  it("returns the expected string for a known key", () => {
    expect(getActiveLocale()).toBe("en");
    expect(t("app.nav.home")).toBe("Home");
    expect(t("tier.safe")).toBe("Safe");
  });

  it("interpolates parameters", () => {
    expect(t("scan.scanning", { seen: "1,204 entries" })).toBe(
      "Scanning · 1,204 entries seen"
    );
    expect(t("scan.scanningInArea", { seen: "12 entries", area: "Graphics" })).toBe(
      "Scanning · 12 entries seen · Graphics"
    );
    expect(t("cleanup.preview.title", { title: "Build caches" })).toBe(
      "Review: Build caches"
    );
  });

  it("switches the whole catalogue with the active locale", () => {
    setActiveLocale("de");
    expect(t("app.nav.home")).toBe("Start");
    expect(t("tier.safe")).toBe("Sicher");
    expect(t("cleanup.confirm.button")).toBe("Verschieben in den Papierkorb bestätigen");
  });

  it("leaves an unknown parameter placeholder visible instead of emptying it", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    expect(t("scan.scanning", {})).toBe("Scanning · {seen} seen");
    expect(warn).toHaveBeenCalled();
  });

  it("formats counted units for the active locale", () => {
    expect(tCount(1, "unit.entry.one", "unit.entry.other")).toBe("1 entry");
    expect(tCount(1204, "unit.entry.one", "unit.entry.other")).toBe("1,204 entries");

    setActiveLocale("de");
    expect(tCount(1, "unit.entry.one", "unit.entry.other")).toBe("1 Eintrag");
    expect(tCount(1204, "unit.entry.one", "unit.entry.other")).toBe("1.204 Einträge");
  });

  it("rejects an unknown key at compile time", () => {
    // @ts-expect-error — keys are a closed union; this line must never typecheck.
    const build = () => t("not.a.real.key");
    expect(typeof build).toBe("function");
  });
});

describe("runtime missing keys are explicit", () => {
  it("returns the key itself and warns in development", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    // The cast is the only way to reach the runtime guard: TypeScript rejects
    // this key at the call site, which is exactly the primary defence tested
    // above. Untyped data (an older bundle, a hand-written call) can still get
    // here, so the guard has to be honest.
    const unknownKey = "runtime.missing.key" as unknown as MessageKey;

    const value = t(unknownKey);
    expect(value).toBe("runtime.missing.key");
    expect(value).not.toBe("");
    expect(warn).toHaveBeenCalledTimes(1);
    expect(String(warn.mock.calls[0]?.[0])).toContain("runtime.missing.key");
  });

  it("renders the key rather than an empty string", () => {
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const unknownKey = "rendered.missing.key" as unknown as MessageKey;

    function Probe() {
      return <p>{t(unknownKey)}</p>;
    }

    const { container } = render(<Probe />);
    expect(container.textContent).toBe("rendered.missing.key");
  });
});
