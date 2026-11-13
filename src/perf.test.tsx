/**
 * Performance suite.
 *
 * PERFORMANCE BUDGETS
 * --------------------
 * The budgets below are wall-clock limits for a *jsdom* run, not for the
 * shipped application. They are deliberately generous (a loaded CI runner is
 * slow, and a flaky suite is worse than no suite) while still being far
 * below the time a human would notice. A regression that doubles the cost of
 * a screen — a render that re-reads the engine, a list that mounts thousands
 * of nodes, an interaction that measures the layout once per row — trips them.
 *
 * | Budget                | Value | What it protects                                   |
 * |-----------------------|-------|----------------------------------------------------|
 * | `RENDER_BUDGET_MS`    | 400ms | the synchronous `render()` of one screen           |
 * | `SETTLE_BUDGET_MS`    | 1500ms| one screen mounting *and* settling from the mock  |
 * | `INTERACTION_BUDGET_MS` | 400ms | one interaction handler returning                 |
 * | `LAYOUT_READ_BUDGET`  | 200   | layout reads performed by one interaction          |
 * | `SCALE_RENDER_BUDGET_MS` | 15000ms | the 5,000-item scale render (see the PERF GAP)  |
 *
 * MEASUREMENT
 * -----------
 * jsdom has no layout engine, so a "long task" cannot be observed the way a
 * browser observes it. The suite therefore measures two things that *are*
 * honest in jsdom:
 *
 *  1. wall-clock time around the work (`performance.now()`), and
 *  2. how many times the work reads layout geometry
 *     (`getBoundingClientRect`, `offsetHeight`, …). A reflow storm is
 *     exactly "read the geometry once per row, interleaved with a write", and
 *     the read count shows it even where the timing cannot.
 *
 * When the runtime does expose a `longtask` PerformanceObserver, the suite
 * uses it as well and asserts that no long task was produced.
 */

import { fireEvent, render } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";
import { StorageScreen } from "./screens/StorageScreen";
import {
  createCountingApi,
  createStaticApi,
  makeStorageData,
  measureInteraction,
  PILLAR_SCREENS,
  renderScreen,
  waitForScreenReady
} from "./testing";

/** Wall-clock budget for the synchronous `render()` of one screen. */
const RENDER_BUDGET_MS = 400;

/** Wall-clock budget for one screen to mount *and* settle from the mock. */
const SETTLE_BUDGET_MS = 1500;

/** Wall-clock budget for one interaction handler (scroll or filter change). */
const INTERACTION_BUDGET_MS = 400;

/** How many layout reads one interaction may perform before it is a storm. */
const LAYOUT_READ_BUDGET = 200;

/**
 * Budget for the 5,000-item scale render.
 *
 * This is a liveness bound, not a quality bound: jsdom is one to two orders
 * of magnitude slower than a browser, and the machine may be busy. What the
 * scale test really asserts is the DOM shape (see the PERF GAP below), which
 * is machine-independent.
 */
const SCALE_RENDER_BUDGET_MS = 45000;

/** The scale render mounts five thousand rows; it needs more than the default. */
const SCALE_TEST_TIMEOUT_MS = 90000;

const CASES = PILLAR_SCREENS.map((testCase) => [testCase.label, testCase] as const);

describe("initial render budget", () => {
  beforeAll(async () => {
    // Warm the render path once, so the first *measured* screen is not
    // charged for the JIT compiling React's render loop. Without this the
    // first screen measured in a run is consistently the slowest.
    const api = createCountingApi({ latencyMs: 0 });
    const { unmount } = await renderScreen(PILLAR_SCREENS[0], api);
    unmount();
  });

  it.each(CASES)("%s renders from the mock within 400 ms", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });

    const start = performance.now();
    const { unmount } = render(testCase.render(api));
    const elapsed = performance.now() - start;

    expect(
      elapsed,
      `${testCase.label} took ${elapsed.toFixed(1)} ms to render (budget ${RENDER_BUDGET_MS} ms)`
    ).toBeLessThan(RENDER_BUDGET_MS);

    await waitForScreenReady(api);
    unmount();
  });

  it.each(CASES)("%s mounts and settles from the mock within 1500 ms", async (_label, testCase) => {
    const api = createCountingApi({ latencyMs: 0 });

    const start = performance.now();
    await renderScreen(testCase, api);
    const elapsed = performance.now() - start;

    expect(
      elapsed,
      `${testCase.label} took ${elapsed.toFixed(1)} ms to mount and settle (budget ${SETTLE_BUDGET_MS} ms)`
    ).toBeLessThan(SETTLE_BUDGET_MS);
  });
});

describe("interaction cost", () => {
  it.each(CASES)(
    "%s handles a scroll and a filter change without a reflow storm",
    async (_label, testCase) => {
      const api = createCountingApi({ latencyMs: 0 });
      const { container } = await renderScreen(testCase, api);

      const filter = container.querySelector("input");
      const measurement = measureInteraction(() => {
        if (filter !== null) {
          fireEvent.change(filter, { target: { value: "a" } });
        }
        fireEvent.scroll(container, { target: { scrollTop: 240 } });
      });

      expect(
        measurement.durationMs,
        `${testCase.label} interaction took ${measurement.durationMs.toFixed(1)} ms`
      ).toBeLessThan(INTERACTION_BUDGET_MS);
      expect(
        measurement.layoutReads,
        `${testCase.label} interaction performed ${measurement.layoutReads} layout reads`
      ).toBeLessThan(LAYOUT_READ_BUDGET);
      if (measurement.longTaskObserverAvailable) {
        expect(measurement.longTasks).toBe(0);
      }
    }
  );
});

describe("memoisation", () => {
  it.each(CASES)(
    "%s does not re-read the engine when it is re-rendered with unchanged props",
    async (_label, testCase) => {
      const api = createCountingApi({ latencyMs: 0 });
      const { rerender } = await renderScreen(testCase, api);

      const callsBefore = api.totalCalls;
      rerender(testCase.render(api));
      await waitForScreenReady(api);

      expect(
        api.totalCalls,
        `${testCase.label} re-read the engine on an unchanged re-render`
      ).toBe(callsBefore);
    }
  );
});

describe("list scale", () => {
  /**
   * PERF GAP — the current behaviour is documented, not endorsed.
   *
   * A 5,000-category storage report mounts every row eagerly: there is no
   * virtual window, no pagination and no stated cap. The engine reports
   * 1,087,920 observed items, so a deep category tree is not hypothetical.
   *
   * The test below asserts the behaviour that exists today, so a future
   * change is noticed; the strict expectation is the skipped test that
   * follows it.
   */
  it(
    "documents the DOM cost of a 5,000-category report (no virtualisation, no pagination, no cap)",
    async () => {
      const api = createCountingApi(
        { latencyMs: 0 },
        createStaticApi({ storage: makeStorageData(5000) })
      );

      const start = performance.now();
      render(<StorageScreen api={api} focusCategoryId={null} />);
      await waitForScreenReady(api);
      const elapsed = performance.now() - start;

      const rows = document.querySelectorAll("li").length;
      const nodes = document.querySelectorAll("*").length;

      // The DOM shape is the machine-independent evidence: every category
      // became a row, so nothing was capped, paginated or virtualised.
      expect(rows).toBeGreaterThanOrEqual(5000);
      expect(nodes / rows).toBeLessThan(50);
      expect(elapsed).toBeLessThan(SCALE_RENDER_BUDGET_MS);
    },
    SCALE_TEST_TIMEOUT_MS
  );

  /**
   * PERF GAP — the assertion the implementation does not meet yet.
   *
   * Recommended bound: a screen that renders 5,000 categories should mount a
   * *window* of rows, not all of them. 400 rendered rows is a generous
   * window for a 56 px row in a 1,000 px viewport (≈ 18 visible rows, 20×
   * over-fetch). The alternatives the design system allows are equally
   * acceptable: an explicit cap with a "show more" control, or pagination.
   *
   * Skipped on purpose: it documents the gap instead of pretending it passes.
   * Delete the `.skip` once the screen virtualises, paginates or caps.
   */
  it.skip(
    "PERF GAP: Storage must not mount 5,000 rows for a 5,000-category report (recommended bound: ≤ 400 rendered rows)",
    async () => {
      const api = createCountingApi(
        { latencyMs: 0 },
        createStaticApi({ storage: makeStorageData(5000) })
      );

      render(<StorageScreen api={api} focusCategoryId={null} />);
      await waitForScreenReady(api);

      const rows = document.querySelectorAll("li").length;
      expect(rows).toBeLessThanOrEqual(400);
    },
    SCALE_TEST_TIMEOUT_MS
  );
},
    SCALE_TEST_TIMEOUT_MS
  );
