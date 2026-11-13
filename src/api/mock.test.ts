import { describe, expect, it } from "vitest";
import { createMockApi } from "../api";

describe("deterministic mock fixture", () => {
  it("returns identical data for identical options", async () => {
    const first = createMockApi({ latencyMs: 0 });
    const second = createMockApi({ latencyMs: 0 });

    await expect(first.getCleanup()).resolves.toEqual(await second.getCleanup());
    await expect(first.getStorage()).resolves.toEqual(await second.getStorage());
    await expect(first.getHomeSummary()).resolves.toEqual(await second.getHomeSummary());
  });

  it("does not leak mutations between calls", async () => {
    const api = createMockApi({ latencyMs: 0 });
    const first = await api.getCleanup();
    first.items.pop();

    const second = await api.getCleanup();
    expect(second.items).toHaveLength(6);
  });
});
