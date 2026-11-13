/**
 * Performance measurement helpers.
 *
 * The performance suite has to answer two questions without a real browser:
 *
 *  1. "How long did this render take?" — `performance.now()` around the work.
 *  2. "Did this interaction force the layout engine to re-measure the tree
 *     over and over?" — in a browser this is a `longtask`; in jsdom the
 *     layout engine is a no-op, so the honest proxy is to count the *layout
 *     reads* the interaction performs.
 *
 * A layout read is a call that forces the engine to compute geometry
 * (`offsetHeight`, `getBoundingClientRect()`, …). A screen that reads geometry
 * once per row interleaved with a write is what a "reflow storm" looks like,
 * and the read count shows it even when jsdom cannot measure the time.
 */

export interface InteractionMeasurement {
  /** Wall-clock duration of the measured work, in milliseconds. */
  durationMs: number;
  /** How many times the measured work read layout geometry. */
  layoutReads: number;
  /** How many long tasks (> 50 ms) were observed, when the API exists. */
  longTasks: number;
  /** Whether a `longtask` PerformanceObserver was available. */
  longTaskObserverAvailable: boolean;
}

/** True when the runtime can observe `longtask` entries. */
export function supportsLongTaskObserver(): boolean {
  if (typeof PerformanceObserver === "undefined") return false;
  const supported = PerformanceObserver.supportedEntryTypes;
  return Array.isArray(supported) && supported.includes("longtask");
}

/**
 * Patch every layout-reading DOM API so calls can be counted.
 * Returns a function that restores the originals.
 */
function countLayoutReads(counter: { value: number }): () => void {
  const restores: Array<() => void> = [];

  const rectDescriptor = Object.getOwnPropertyDescriptor(
    Element.prototype,
    "getBoundingClientRect"
  );
  if (rectDescriptor && typeof rectDescriptor.value === "function") {
    const original = rectDescriptor.value;
    Object.defineProperty(Element.prototype, "getBoundingClientRect", {
      configurable: true,
      enumerable: rectDescriptor.enumerable,
      writable: rectDescriptor.writable,
      value: function patchedGetBoundingClientRect(this: Element): DOMRect {
        counter.value += 1;
        return original.call(this) as DOMRect;
      }
    });
    restores.push(() => {
      Object.defineProperty(
        Element.prototype,
        "getBoundingClientRect",
        rectDescriptor
      );
    });
  }

  const measured: Array<"offsetHeight" | "offsetWidth" | "clientHeight" | "clientWidth" | "scrollHeight" | "scrollWidth"> = [
    "offsetHeight",
    "offsetWidth",
    "clientHeight",
    "clientWidth",
    "scrollHeight",
    "scrollWidth"
  ];
  for (const key of measured) {
    const descriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, key);
    if (!descriptor || typeof descriptor.get !== "function") continue;
    const original = descriptor.get;
    Object.defineProperty(HTMLElement.prototype, key, {
      configurable: true,
      enumerable: descriptor.enumerable,
      get: function patchedMeasured(this: HTMLElement): number {
        counter.value += 1;
        return original.call(this) as number;
      }
    });
    restores.push(() => {
      Object.defineProperty(HTMLElement.prototype, key, descriptor);
    });
  }

  return () => {
    for (const restore of restores) restore();
  };
}

/**
 * Measure one interaction: how long it took, how many layout reads it made,
 * and whether it produced a long task.
 */
export function measureInteraction(run: () => void): InteractionMeasurement {
  const counter = { value: 0 };
  const restore = countLayoutReads(counter);

  let longTasks = 0;
  let observer: PerformanceObserver | null = null;
  const canObserve = supportsLongTaskObserver();
  if (canObserve) {
    observer = new PerformanceObserver((list) => {
      longTasks += list.getEntries().length;
    });
    try {
      observer.observe({ entryTypes: ["longtask"] });
    } catch {
      observer = null;
    }
  }

  const start = performance.now();
  let failure: unknown = null;
  try {
    run();
  } catch (error: unknown) {
    failure = error;
  }
  const elapsed = performance.now() - start;
  restore();
  observer?.disconnect();
  if (failure !== null) throw failure;
  return {
    durationMs: elapsed,
    layoutReads: counter.value,
    longTasks,
    longTaskObserverAvailable: canObserve && observer !== null
  };
}
