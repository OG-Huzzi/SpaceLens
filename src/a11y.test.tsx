/**
 * Cross-cutting accessibility suite.
 *
 * This file is the mechanical enforcement of two documents:
 *
 *  - `docs/DESIGN_SYSTEM.md` — its anti-pattern list, especially
 *    "an icon-only button with no `aria-label`" and "any colour that is not
 *    from this token set";
 *  - `docs/UX_ARCHITECTURE.md` — "Actions live in Cleanup only", "scanning is
 *    ambient, never blocking", and the one-question-per-screen structure.
 *
 * RESILIENCE
 * ----------
 * `src/screens/` is being rebuilt in parallel with this suite. The suite
 * therefore never asserts on a class name, a CSS selector or the wording of a
 * heading. It asserts on:
 *
 *  - roles (`heading`, `button`, `status`, `banner`, …);
 *  - accessible names (computed the way a screen reader computes them);
 *  - ARIA state (`aria-live`, `aria-modal`, `aria-expanded`, `data-theme`);
 *  - the *absence* of controls, which no rewrite of a screen can satisfy by
 *    accident.
 *
 * The one place where a test needs a name is the Cleanup confirmation, and
 * there the rule itself is a name rule ("confirm"), so the assertion is
 * stable by definition.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { ScanStatusStrip } from "./components/ScanStatusStrip";
import { CleanupScreen } from "./screens/CleanupScreen";
import {
  accessibleName,
  assertNoFocusTrap,
  axeViolations,
  createCountingApi,
  describeElement,
  formatViolations,
  hasAccessibleName,
  PILLAR_SCREENS,
  renderScreen,
  SETTINGS_SCREEN,
  tabbableElements,
  waitForScreenReady
} from "./testing";

/** The five pillar screens, minus Cleanup (the only screen with actions). */
const INFORMATIONAL_SCREENS = PILLAR_SCREENS.filter((testCase) => testCase.id !== "cleanup");

/** `it.each` rows: a printable label plus the case. */
const CASES = PILLAR_SCREENS.map((testCase) => [testCase.label, testCase] as const);

/**
 * Controls that can change state.
 *
 * A text-like `<input>` is deliberately excluded: a filter over data that is
 * already loaded is a view control, not an action. The distinction is proved
 * separately — the filter must not trigger a single API call.
 */
function isViewFilter(element: Element): boolean {
  if (!(element instanceof HTMLInputElement)) return false;
  return ["search", "text"].includes(element.type);
}

function stateChangingControls(container: HTMLElement): Element[] {
  return Array.from(
    container.querySelectorAll(
      "button, input, select, textarea, form, [role='button'], [role='checkbox'], [role='switch'], [role='radio']"
    )
  ).filter((element) => !isViewFilter(element));
}

function resetDocument() {
  const root = document.documentElement;
  for (const attribute of [
    "data-theme",
    "data-density",
    "data-reduced-motion",
    "data-advanced-details"
  ]) {
    root.removeAttribute(attribute);
  }
}

beforeEach(() => {
  window.localStorage.clear();
  resetDocument();
});

afterEach(() => {
  window.localStorage.clear();
  resetDocument();
});

describe("screen structure", () => {
  it.each(CASES)("%s has exactly one h1 and a heading outline that never skips", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(testCase, api);

    const headings = Array.from(container.querySelectorAll("h1, h2, h3, h4, h5, h6"));
    const levelOne = headings.filter((heading) => heading.tagName === "H1");

    expect(levelOne).toHaveLength(1);

    let previous = 0;
    for (const heading of headings) {
      const level = Number.parseInt(heading.tagName.slice(1), 10);
      expect(
        level - previous,
        `${describeElement(heading)} skips a heading level after h${previous}`
      ).toBeLessThanOrEqual(1);
      previous = level;
    }
  });

  it("exposes exactly one banner, main, nav and contentinfo landmark", async () => {
    render(<App />);
    await waitFor(() => {
      expect(document.querySelector("main")).not.toBeNull();
    });

    // The banner/contentinfo roles are scoped: a `<header>` inside `<main>`
    // is a generic header, not a second banner. Testing Library's role map
    // does not implement that scoping, so the two landmarks are counted with
    // the HTML-AAM rule applied by hand.
    const outsideMain = "main, article, aside, section, nav";
    const banners = Array.from(document.querySelectorAll("header")).filter(
      (element) => element.closest(outsideMain) === null
    );
    const contentinfos = Array.from(document.querySelectorAll("footer")).filter(
      (element) => element.closest(outsideMain) === null
    );

    expect(banners).toHaveLength(1);
    expect(contentinfos).toHaveLength(1);
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(screen.getAllByRole("navigation").length).toBeGreaterThanOrEqual(1);
  });
});

describe("accessible names", () => {
  it.each(CASES)("%s names every focusable control", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(testCase, api);

    const unnamed = tabbableElements(container).filter((element) => !hasAccessibleName(element));
    expect(unnamed.map((element) => describeElement(element))).toEqual([]);
  });

  it.each(CASES)("%s passes the native accessibility scan", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(testCase, api);

    const violations = await axeViolations(container);
    expect(formatViolations(violations)).toBe("no violations");
  });

  it("labels every control on the Settings screen", async () => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(SETTINGS_SCREEN, api);

    const controls = Array.from(
      container.querySelectorAll<HTMLElement>("input, select, textarea")
    );
    expect(controls.length).toBeGreaterThan(0);

    const unnamed = controls.filter((element) => !hasAccessibleName(element));
    expect(unnamed.map((element) => describeElement(element))).toEqual([]);

    const unlabelled = controls.filter((element) => {
      const labelled = element as HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;
      return (labelled.labels?.length ?? 0) === 0;
    });
    expect(unlabelled.map((element) => describeElement(element))).toEqual([]);
  });
});

describe("keyboard order", () => {
  it.each(CASES)("%s does not trap the keyboard", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(testCase, api);

    // A screen must never trap the keyboard: no modal nested inside it, no
    // focus sentinel, no duplicated tab stop, no positive tabindex.
    expect(() => assertNoFocusTrap(container)).not.toThrow();

    // Nothing re-orders the tab sequence: the stops are in document order.
    const stops = tabbableElements(container);
    const inDocumentOrder = [...stops].sort((left, right) =>
      left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1
    );
    expect(stops).toEqual(inDocumentOrder);
  });

  it.each(CASES)("%s uses no positive tabindex", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(testCase, api);

    const positive = Array.from(container.querySelectorAll("[tabindex]")).filter(
      (element) => Number.parseInt(element.getAttribute("tabindex") ?? "0", 10) > 0
    );
    expect(positive.map((element) => describeElement(element))).toEqual([]);
  });

  it("keeps the app shell free of positive tabindex", async () => {
    render(<App />);
    await waitFor(() => {
      expect(document.querySelector("main")).not.toBeNull();
    });

    const positive = Array.from(document.querySelectorAll("[tabindex]")).filter(
      (element) => Number.parseInt(element.getAttribute("tabindex") ?? "0", 10) > 0
    );
    expect(positive.map((element) => describeElement(element))).toEqual([]);
  });
});

describe("the ambient scan strip", () => {
  it("is a polite status region that never takes focus", async () => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = render(<ScanStatusStrip api={api} />);

    await waitFor(() => {
      expect(container.querySelector("[data-scan-state]")).not.toBeNull();
    });

    const strip = container.querySelector('[role="status"]');
    expect(strip).not.toBeNull();
    if (strip === null) return;

    expect(strip.getAttribute("aria-live")).toBe("polite");
    expect(strip.getAttribute("aria-atomic")).toBe("true");
    expect(strip.hasAttribute("tabindex")).toBe(false);
    expect(
      strip.querySelectorAll("a[href], button, input, select, textarea, [tabindex]")
    ).toHaveLength(0);

    // The strip must not steal focus when it appears or when it updates.
    expect(strip.contains(document.activeElement)).toBe(false);
    expect(document.activeElement).toBe(document.body);
  });
});

describe("actions live in Cleanup only", () => {
  it.each(INFORMATIONAL_SCREENS.map((testCase) => [testCase.label, testCase] as const))(
    "%s exposes no state-changing control",
    async (_label, testCase) => {
      const api = createCountingApi({ latencyMs: 0 });
      const { container } = await renderScreen(testCase, api);

      expect(stateChangingControls(container).map((element) => describeElement(element))).toEqual(
        []
      );

      // The one carve-out, stated exactly: a text-like input on a pillar
      // screen is a *filter over data that is already loaded*, never an
      // action. `docs/UX_ARCHITECTURE.md` asks Storage for "Search within",
      // so the rule is "a filter, and only a filter" rather than "no input
      // at all". The next test proves the filter changes nothing.
      const inputs = Array.from(container.querySelectorAll("input, select, textarea"));
      for (const input of inputs) {
        expect(
          isViewFilter(input),
          `${describeElement(input)} is not a read-only filter`
        ).toBe(true);
      }
    }
  );

  it.each(INFORMATIONAL_SCREENS.map((testCase) => [testCase.label, testCase] as const))(
    "%s never calls the engine when a control is used",
    async (_label, testCase) => {
      const api = createCountingApi({ latencyMs: 0 });
      const { container } = await renderScreen(testCase, api);

      const callsBefore = api.totalCalls;
      const filter = container.querySelector("input");
      if (filter !== null) {
        fireEvent.change(filter, { target: { value: "a" } });
      }
      fireEvent.scroll(container, { target: { scrollTop: 100 } });

      await waitFor(() => {
        expect(api.totalCalls).toBe(callsBefore);
      });
    }
  );

  it("gives the Storage search filter an accessible name and no engine access", async () => {
    const api = createCountingApi({ latencyMs: 0 });
    const { container } = await renderScreen(
      PILLAR_SCREENS.find((testCase) => testCase.id === "storage") ??
        PILLAR_SCREENS[0],
      api
    );

    const filter = container.querySelector("input");
    expect(filter, "Storage must offer a search filter").not.toBeNull();
    if (filter === null) return;
    expect(hasAccessibleName(filter)).toBe(true);

    const callsBefore = api.totalCalls;
    fireEvent.change(filter, { target: { value: "Dev" } });
    await waitFor(() => {
      expect(api.totalCalls).toBe(callsBefore);
    });
  });

  it("confirms a cleanup through a real button whose accessible name says confirm", async () => {
    const api = createCountingApi({ latencyMs: 0 });
    render(<CleanupScreen api={api} />);
    await waitForScreenReady(api);

    // Open the first opportunity: the card is a disclosure, not a dialog.
    const toggle = screen
      .getAllByRole("button")
      .find((button) => button.getAttribute("aria-expanded") === "false");
    expect(toggle, "the cleanup list must offer a disclosure per opportunity").toBeDefined();
    if (toggle === undefined) return;
    fireEvent.click(toggle);

    const preview = await screen.findByRole("button", { name: /preview/i });
    fireEvent.click(preview);

    const confirm = await screen.findByRole("button", { name: /confirm/i });
    expect(confirm.tagName).toBe("BUTTON");
    expect(confirm.getAttribute("type")).toBe("button");
    expect(accessibleName(confirm).toLowerCase()).toContain("confirm");
  });
});

describe("themes", () => {
  it("applies both themes through the data-theme attribute on the document element", async () => {
    const api = createCountingApi({ latencyMs: 0 });
    render(SETTINGS_SCREEN.render(api));

    const dark = await screen.findByRole("radio", { name: "Dark" });
    fireEvent.click(dark);
    await waitFor(() => {
      expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    });

    const light = screen.getByRole("radio", { name: "Light" });
    fireEvent.click(light);
    await waitFor(() => {
      expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    });
  });
});
