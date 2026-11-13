import { useEffect, useState, type RefObject } from "react";

/**
 * Measures the width of a container so a chart can render fluidly.
 *
 * The hook never invents a size: when the container cannot be measured (no
 * ResizeObserver, or a zero-width container as in a test environment) it
 * reports `fallback` and the caller decides what that means.
 */
export function useMeasuredWidth(
  ref: RefObject<HTMLElement | null>,
  fallback: number
): number {
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const element = ref.current;
    if (element === null) return;

    const measure = (): void => {
      const next = element.getBoundingClientRect().width;
      setWidth((current) =>
        Math.abs(current - next) < 0.5 ? current : next
      );
    };

    measure();

    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => measure());
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);

  return width > 0 ? width : fallback;
}
