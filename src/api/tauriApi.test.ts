import { afterEach, describe, expect, it } from "vitest";
import type { ApiError, CleanupPreview, DataCoverage } from "../api";
import {
  CoreSightApiError,
  TauriCoreSightApi,
  createApi,
  createTauriApi
} from "./tauriApi";

const completeCoverage: DataCoverage = {
  completeness: "complete",
  unreadableEntries: 0,
  unknownSizeEntries: 0,
  excludedAreas: [],
  note: null
};

const validPreview: CleanupPreview = {
  planId: "plan-opp-build-caches-1",
  opportunityIds: ["opp-build-caches"],
  items: [
    {
      title: "Rebuildable build caches",
      bytes: 4096,
      itemCount: 12,
      destination: "System Trash (CoreSight cleanup batch)",
      undo: "Restore from Trash within 30 days.",
      reason: "Build caches are regenerable."
    }
  ],
  totalBytes: 4096,
  whatStaysUntouched: ["Source code"],
  destination: "System Trash (CoreSight cleanup batch)",
  howToUndo: "Open Trash and choose Put Back within 30 days.",
  consequence: "The next build recreates the cache.",
  complete: true,
  warnings: [],
  coverage: completeCoverage
};

type TauriTestWindow = Window & { __TAURI_INTERNALS__?: unknown };
const tauriWindow = window as TauriTestWindow;

afterEach(() => {
  delete tauriWindow.__TAURI_INTERNALS__;
});

describe("Tauri transport validation", () => {
  it("maps a valid preview through the parser and calls previewPlan", async () => {
    const calls: Array<{ command: string; args: unknown }> = [];
    const api = createTauriApi({
      invoke: async (command, args) => {
        calls.push({ command, args });
        return validPreview;
      }
    });

    await expect(api.previewCleanup(["opp-build-caches"])).resolves.toEqual(validPreview);
    expect(calls).toEqual([
      {
        command: "preview_plan",
        args: { opportunityIds: ["opp-build-caches"] }
      }
    ]);
  });

  it("unwraps a valid desktop-shell envelope", async () => {
    const api = createTauriApi({
      invoke: async () => ({
        ok: { kind: "coresight.v1.previewPlan", payload: validPreview }
      })
    });

    await expect(api.previewCleanup(["opp-build-caches"])).resolves.toEqual(validPreview);
  });

  it("rejects an envelope that answers a different command", async () => {
    // A shell that routes previewPlan to another handler must not have its
    // payload parsed: that would attribute another command's data to the
    // Cleanup screen.
    const api = createTauriApi({
      invoke: async () => ({
        ok: { kind: "coresight.v1.getCategories", payload: validPreview }
      })
    });

    await expect(api.previewCleanup(["opp-build-caches"])).rejects.toMatchObject({
      code: "INVALID_RESPONSE"
    });
  });

  it("reports an unsupported command instead of invoking a missing handler", async () => {
    let invoked = 0;
    const api = createTauriApi({
      invoke: async () => {
        invoked += 1;
        return { ok: { kind: "coresight.v1.getStorage", payload: {} } };
      }
    });

    await expect(api.getStorage()).rejects.toMatchObject({
      code: "UNSUPPORTED_IN_THIS_BUILD"
    });
    expect(invoked).toBe(0);
  });

  it("normalises an ApiError carried inside a resolved envelope", async () => {
    const api = createTauriApi({
      invoke: async () => ({
        error: {
          code: "PLAN_REJECTED_UNSAFE",
          message: "The plan was rejected by CoreSight's safety policy.",
          detail: '["unsafe"]'
        }
      })
    });

    await expect(api.previewCleanup(["opp-build-caches"])).rejects.toMatchObject({
      code: "PLAN_REJECTED_UNSAFE",
      message: "The plan was rejected by CoreSight's safety policy."
    });
  });

  it("rejects a response with undefined where a number is required, rather than using 0", async () => {
    const malformed: unknown = {
      ...validPreview,
      items: [{ ...validPreview.items[0], bytes: undefined }]
    };
    const api = createTauriApi({ invoke: async () => malformed });

    let caught: unknown;
    try {
      await api.previewCleanup(["opp-build-caches"]);
    } catch (error: unknown) {
      caught = error;
    }

    expect(caught).toBeInstanceOf(CoreSightApiError);
    const apiError = caught as ApiError;
    expect(apiError.code).toBe("INVALID_RESPONSE");
    expect(apiError.detail).toContain("items[0].bytes");
    expect(apiError.detail).not.toContain("4096");
  });

  it("rejects a response with a missing required field", async () => {
    const malformed: Record<string, unknown> = { ...validPreview };
    delete malformed.planId;
    const api = createTauriApi({ invoke: async () => malformed });

    await expect(api.previewCleanup(["opp-build-caches"])).rejects.toMatchObject({
      code: "INVALID_RESPONSE"
    });
  });

  it("preserves a known ApiError code and message verbatim", async () => {
    const api = createTauriApi({
      invoke: async () => {
        throw {
          code: "PERMISSION_GAP",
          message: "Some locations could not be read.",
          detail: "4 protected entries were skipped."
        };
      }
    });

    await expect(api.listDrives()).rejects.toMatchObject({
      code: "PERMISSION_GAP",
      message: "Some locations could not be read.",
      detail: "4 protected entries were skipped."
    });
  });

  it("replaces an unknown code with a generic message and never leaks the raw error", async () => {
    const api = createTauriApi({
      invoke: async () => {
        throw {
          code: "INTERNAL_PANIC",
          message: "panicked at src-tauri/src/main.rs:42\nstack trace follows",
          detail: '{"secret":"abc"}'
        };
      }
    });

    let caught: unknown;
    try {
      await api.listDrives();
    } catch (error: unknown) {
      caught = error;
    }

    expect(caught).toBeInstanceOf(CoreSightApiError);
    const apiError = caught as ApiError;
    expect(apiError.code).toBe("UNKNOWN_API_ERROR");
    expect(apiError.message).not.toMatch(/panicked|stack|\{/);
    expect(apiError.detail ?? "").not.toMatch(/secret|\{/);
  });

  it("does not fall back to mock data when a live call fails", async () => {
    const api = createTauriApi({
      invoke: async () => {
        throw {
          code: "SNAPSHOT_UNAVAILABLE",
          message: "No local snapshot is available."
        };
      }
    });

    await expect(api.listDrives()).rejects.toMatchObject({
      code: "SNAPSHOT_UNAVAILABLE",
      message: "No local snapshot is available."
    });
  });

  it("confirms a plan before it calls executePlan", async () => {
    const calls: string[] = [];
    const api = createTauriApi({
      invoke: async (command) => {
        calls.push(command);
        if (command === "confirm_plan") {
          return { accepted: true, rejected: [] };
        }
        if (command === "execute_plan") {
          return {
            accepted: true,
            simulated: false,
            movedItems: 12,
            freedBytes: 4096,
            receipts: ["receipt-opp-build-caches"],
            leftovers: [],
            verification: "verified",
            note: "Cleanup completed and verified."
          };
        }
        throw new Error(`unexpected command ${command}`);
      }
    });

    const result = await api.executeCleanup({
      planId: "plan-opp-build-caches-1",
      confirmed: true,
      acknowledgedWarnings: []
    });

    expect(calls).toEqual([
      "confirm_plan",
        "execute_plan"
    ]);
    expect(result.accepted).toBe(true);
    expect(result.freedBytes).toBe(4096);
  });
});

describe("createApi transport selection", () => {
  it("returns the deterministic mock outside Tauri", async () => {
    const api = createApi({ mock: { latencyMs: 0 } });

    expect(api).not.toBeInstanceOf(TauriCoreSightApi);
    await expect(api.getGlobalStatus()).resolves.toMatchObject({ simulatedData: true });
  });

  it("returns the Tauri transport when the Tauri runtime marker is present", () => {
    tauriWindow.__TAURI_INTERNALS__ = {};
    expect(createApi()).toBeInstanceOf(TauriCoreSightApi);
  });
});
