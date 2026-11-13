import { act, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  CONTRACT_VERSION,
  createMockApi,
  type CoreSightApi,
  type ScanStatus
} from "../api";
import { ScanStatusStrip } from "./ScanStatusStrip";

function scanStatus(overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    scanId: "scan-2026-11-10-1518",
    state: "idle",
    startedAt: null,
    completedAt: null,
    filesSeen: null,
    bytesSeen: null,
    currentArea: null,
    etaSeconds: null,
    coverage: {
      completeness: "complete",
      unreadableEntries: 0,
      unknownSizeEntries: 0,
      excludedAreas: [],
      note: null
    },
    ...overrides
  };
}

function apiFor(status: ScanStatus): CoreSightApi {
  const api = createMockApi({ latencyMs: 0 });
  api.getGlobalStatus = async () => ({
    contractVersion: CONTRACT_VERSION,
    simulatedData: true,
    scan: status
  });
  return api;
}

function apiThatFails(): CoreSightApi {
  const api = createMockApi({ latencyMs: 0 });
  api.getGlobalStatus = async () => {
    throw new Error("status unavailable");
  };
  return api;
}

async function renderStrip(api: CoreSightApi, showAdvancedDetails = false) {
  let view!: ReturnType<typeof render>;
  // Flush the resolved status promise inside act so the strip settles before
  // the assertions instead of updating the DOM after the test body ends.
  await act(async () => {
    view = render(
      <ScanStatusStrip api={api} showAdvancedDetails={showAdvancedDetails} />
    );
  });
  const strip = screen.getByRole("status");
  return { ...view, strip };
}

describe("ambient scan strip", () => {
  it("shows the idle state", async () => {
    const { strip } = await renderStrip(apiFor(scanStatus({ state: "idle" })));

    expect(await within(strip).findByText("No completed scan yet")).toBeInTheDocument();
    await waitFor(() => expect(strip).toHaveAttribute("data-scan-state", "idle"));
  });

  it("shows scanning progress the contract actually provides", async () => {
    const { strip } = await renderStrip(
      apiFor(
        scanStatus({
          state: "scanning",
          startedAt: "2026-11-10T15:12:42.000Z",
          filesSeen: 1204,
          bytesSeen: 4_294_967_296,
          currentArea: "Graphics"
        })
      )
    );

    expect(
      await within(strip).findByText("Scanning · 1,204 entries seen · Graphics")
    ).toBeInTheDocument();
    await waitFor(() => expect(strip).toHaveAttribute("data-scan-state", "scanning"));
    // Indeterminate meter: no invented percentage, but motion while scanning.
    expect(strip.querySelector(".scan-strip__meter")).not.toBeNull();
  });

  it("shows the complete state with the completion time", async () => {
    const { strip } = await renderStrip(
      apiFor(
        scanStatus({
          state: "complete",
          completedAt: "2026-11-10T15:18:00.000Z",
          filesSeen: 1204822
        })
      )
    );

    expect(await within(strip).findByText(/^Scan complete · /)).toBeInTheDocument();
    await waitFor(() => expect(strip).toHaveAttribute("data-scan-state", "complete"));
  });

  it("shows the failed state", async () => {
    const { strip } = await renderStrip(apiFor(scanStatus({ state: "failed" })));

    expect(
      await within(strip).findByText("Last scan failed · figures may be stale")
    ).toBeInTheDocument();
    await waitFor(() => expect(strip).toHaveAttribute("data-scan-state", "failed"));
  });

  it("shows the partial state with the unreadable count", async () => {
    const { strip } = await renderStrip(
      apiFor(
        scanStatus({
          state: "partial",
          completedAt: "2026-11-10T15:18:00.000Z",
          coverage: {
            completeness: "partial",
            unreadableEntries: 184,
            unknownSizeEntries: 37,
            excludedAreas: ["Protected system locations"],
            note: "Lower bound."
          }
        })
      )
    );

    expect(
      await within(strip).findByText(
        "Last scan completed with limits · 184 entries could not be read"
      )
    ).toBeInTheDocument();
    await waitFor(() => expect(strip).toHaveAttribute("data-scan-state", "partial"));
  });

  it("says so when the status cannot be read, instead of inventing a state", async () => {
    const { strip } = await renderStrip(apiThatFails());

    expect(await within(strip).findByText("Scan status unavailable")).toBeInTheDocument();
    await waitFor(() => expect(strip).toHaveAttribute("data-scan-state", "unavailable"));
  });
});

describe("ambient scan strip safety and accessibility", () => {
  it("is a polite live region that never takes focus and holds no control", async () => {
    const { strip } = await renderStrip(
      apiFor(scanStatus({ state: "complete", completedAt: "2026-11-10T15:18:00.000Z" }))
    );

    expect(strip).toHaveAttribute("role", "status");
    expect(strip).toHaveAttribute("aria-live", "polite");
    expect(strip).toHaveAttribute("aria-atomic", "true");
    expect(strip).toHaveAttribute("aria-label", "Scan status");
    expect(strip).not.toHaveAttribute("tabindex");

    await within(strip).findByText(/^Scan complete · /);

    // "Actions live in Cleanup only": the strip can never start or cancel a scan.
    expect(
      strip.querySelectorAll("button, a, input, select, textarea, [role='button']")
    ).toHaveLength(0);
    expect(within(strip).queryAllByRole("button")).toHaveLength(0);
    expect(within(strip).queryAllByRole("link")).toHaveLength(0);
    expect(strip.contains(document.activeElement)).toBe(false);
    expect(strip.querySelectorAll("[tabindex]")).toHaveLength(0);
  });

  it("only shows diagnostic detail when advanced details are enabled", async () => {
    const api = apiFor(
      scanStatus({
        state: "complete",
        completedAt: "2026-11-10T15:18:00.000Z",
        bytesSeen: 4_294_967_296
      })
    );

    const { strip, rerender } = await renderStrip(api);
    await within(strip).findByText(/^Scan complete · /);
    expect(strip.textContent).not.toContain("scan-2026-11-10-1518");

    rerender(<ScanStatusStrip api={api} showAdvancedDetails />);
    expect(await within(strip).findByText(/scan-2026-11-10-1518/)).toBeInTheDocument();
    expect(strip.textContent).toContain("4 GB");
  });
});
