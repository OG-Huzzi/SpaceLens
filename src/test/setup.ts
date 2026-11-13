import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// jsdom implements layout as a no-op and does not provide scrollTo. Cleanup
// calls it only to reset the viewport when the preview screen opens.
Object.defineProperty(window, "scrollTo", {
  configurable: true,
  writable: true,
  value: vi.fn()
});

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  cleanup();
});
