import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createMockApi } from "../api";
import { DrivesScreen } from "./DrivesScreen";
import { HistoryScreen } from "./HistoryScreen";
import { HomeScreen } from "./HomeScreen";
import { StorageScreen } from "./StorageScreen";

function makeApi() {
  return createMockApi({ latencyMs: 0 });
}

/**
 * The informational screens must not expose any control that can change state.
 * Asserting the absence of the role itself is deliberate: if a button, submit
 * control, or checkbox is added to these screens for any future feature, this
 * test fails and forces a review of the "Actions live in Cleanup only" rule.
 */
function expectNoStateChangingControls(screenName: string) {
  const buttons = screen.queryAllByRole("button");
  expect(
    buttons.map((button) => button.getAttribute("aria-label") ?? button.textContent),
    `${screenName} must not render action buttons`
  ).toEqual([]);

  expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
  expect(screen.queryAllByRole("switch")).toHaveLength(0);
  expect(
    document.querySelectorAll("[role='button'], input[type='submit'], form")
  ).toHaveLength(0);

  // Any link on these screens must be in-app navigation, never an action.
  for (const link of screen.queryAllByRole("link")) {
    expect(link.getAttribute("href") ?? "").toMatch(/^#/);
  }
}

describe("actions live in Cleanup only", () => {
  it("renders Home without a state-changing control", async () => {
    render(<HomeScreen api={makeApi()} />);
    await screen.findByRole("heading", { name: "Largest categories" });
    expectNoStateChangingControls("Home");
  });

  it("renders Storage without a state-changing control", async () => {
    render(<StorageScreen api={makeApi()} focusCategoryId={null} />);
    await screen.findByRole("heading", { name: "Categories" });
    expectNoStateChangingControls("Storage");
  });

  it("renders History without a state-changing control", async () => {
    render(<HistoryScreen api={makeApi()} onOpenCategory={() => undefined} />);
    await screen.findByRole("heading", { name: "Snapshot timeline" });
    expectNoStateChangingControls("History");
  });

  it("renders Drives without a state-changing control", async () => {
    render(<DrivesScreen api={makeApi()} />);
    await screen.findByRole("heading", { name: "How drive memory works" });
    expectNoStateChangingControls("Drives");
  });
});
