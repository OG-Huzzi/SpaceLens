import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createMockApi } from "../api";
import { CleanupScreen } from "./CleanupScreen";

function renderCleanup(scenario: "ready" | "partial" = "ready") {
  const api = createMockApi({ latencyMs: 0, scenario });
  render(<CleanupScreen api={api} />);
  return api;
}

async function waitForOpportunities() {
  await screen.findByRole("heading", { name: "Opportunities" });
}

describe("Cleanup safety surface", () => {
  it("shows every off-limits category with an explicit reason", async () => {
    renderCleanup();
    await waitForOpportunities();

    const panel = screen.getByRole("region", { name: "Visibly excluded" });
    const entries = within(panel).getAllByRole("listitem");

    expect(entries).toHaveLength(3);
    for (const entry of entries) {
      expect(entry.textContent).toMatch(/Why it is excluded:\s*\S+/);
      expect(within(entry).queryAllByRole("button")).toHaveLength(0);
    }

    expect(within(panel).getByText(/Protected system area/)).toBeInTheDocument();
    expect(within(panel).getByText(/Irreplaceable originals/)).toBeInTheDocument();
    expect(
      within(panel).getByText(/Separate, explicitly authorized capability/)
    ).toBeInTheDocument();
  });

  it("announces a partial scan and states the unreadable count", async () => {
    renderCleanup("partial");
    await waitForOpportunities();

    expect(screen.getByText("This scan is incomplete")).toBeInTheDocument();
    expect(screen.getByLabelText("Unreadable entries: 184 entries")).toBeInTheDocument();
    expect(screen.getByLabelText("Unknown sizes: 37 entries")).toBeInTheDocument();
    expect(
      screen.getByText(/Actions are disabled while this scan is incomplete/)
    ).toBeInTheDocument();
  });

  it("has no execute affordance until a preview has been produced", async () => {
    renderCleanup();
    await waitForOpportunities();

    expect(
      screen.queryByRole("button", { name: /Confirm move to Trash/i })
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /execute|delete|clean all|remove all/i })
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Exact preview · nothing has moved")).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Read details for Rebuildable build caches"
      })
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Preview what would move" })
    );

    expect(await screen.findByText("Exact preview · nothing has moved")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Exactly what would move" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Confirm move to Trash" })
    ).toBeInTheDocument();
  });

  it("requires an explicit acknowledgement before confirmation is enabled", async () => {
    renderCleanup();
    await waitForOpportunities();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Read details for Rebuildable build caches"
      })
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Preview what would move" })
    );

    const acknowledge = await screen.findByRole("checkbox", {
      name: /I have read exactly what would move/
    });
    expect(acknowledge).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Confirm move to Trash" })).toBeDisabled();

    fireEvent.click(acknowledge);

    expect(acknowledge).toBeChecked();
    expect(screen.getByRole("button", { name: "Confirm move to Trash" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Confirm move to Trash" }));
    expect(
      await screen.findByRole("heading", { name: "Confirmation recorded" })
    ).toBeInTheDocument();
  });

  it("offers no bulk cleanup control across all opportunities", async () => {
    const api = renderCleanup();
    await waitForOpportunities();

    const cleanup = await api.getCleanup();
    const listButtons = screen.queryAllByRole("button");
    expect(listButtons).toHaveLength(cleanup.items.length);
    for (const button of listButtons) {
      expect(button.getAttribute("aria-label")).toMatch(/^Read details for /);
    }

    const controls = [
      ...screen.queryAllByRole("button"),
      ...screen.queryAllByRole("link")
    ];
    const bulkName =
      /clean everything|clean all|remove all|delete all|select all|all opportunities/i;
    const bulkControls = controls.filter((control) =>
      bulkName.test(control.getAttribute("aria-label") ?? control.textContent ?? "")
    );

    expect(bulkControls).toHaveLength(0);
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    expect(screen.getByText("No clean-everything path exists.")).toBeInTheDocument();

    await expect(api.previewCleanup([])).rejects.toMatchObject({ code: "NOTHING_TO_DO" });
  });
});
