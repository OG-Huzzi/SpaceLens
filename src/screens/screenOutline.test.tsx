/**
 * The heading outline of every pillar screen.
 *
 * `docs/DESIGN_SYSTEM.md` requires a real typographic hierarchy, and the
 * mechanical form of that is a heading outline that never skips a level: a
 * reader who navigates by heading must not meet an `h3` whose parent is the
 * page title.
 *
 * The assertion is deliberately independent of the shared test helpers: it
 * renders the screens with the repository's own mock API, so a failure here
 * is a defect in these screens and nothing else.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createMockApi, type CoreSightApi } from "../api";
import { CleanupScreen } from "./CleanupScreen";
import { DrivesScreen } from "./DrivesScreen";
import { HistoryScreen } from "./HistoryScreen";
import { HomeScreen } from "./HomeScreen";
import { StorageScreen } from "./StorageScreen";

function makeApi(): CoreSightApi {
  return createMockApi({ latencyMs: 0 });
}

interface HeadingStep {
  level: number;
  text: string;
}

function outline(container: HTMLElement): HeadingStep[] {
  return Array.from(container.querySelectorAll("h1, h2, h3, h4, h5, h6")).map(
    (heading) => ({
      level: Number.parseInt(heading.tagName.slice(1), 10),
      text: (heading.textContent ?? "").trim().slice(0, 48)
    })
  );
}

async function expectNoSkippedLevels(container: HTMLElement, screenName: string) {
  const headings = outline(container);

  expect(headings.length, `${screenName} must render headings`).toBeGreaterThan(0);
  expect(
    headings.filter((heading) => heading.level === 1),
    `${screenName} must have exactly one h1`
  ).toHaveLength(1);
  expect(headings[0]?.level, `${screenName} must open with its h1`).toBe(1);

  let previous = 0;
  for (const heading of headings) {
    expect(
      heading.level - previous,
      `${screenName}: "${heading.text}" (h${heading.level}) skips a level after h${previous}`
    ).toBeLessThanOrEqual(1);
    previous = heading.level;
  }
}

describe("the heading outline of every pillar screen", () => {
  it("Home never skips a heading level", async () => {
    const { container } = render(<HomeScreen api={makeApi()} />);
    await screen.findByRole("heading", { name: "Largest categories" });
    await expectNoSkippedLevels(container, "Home");
  });

  it("Storage never skips a heading level", async () => {
    const { container } = render(<StorageScreen api={makeApi()} focusCategoryId={null} />);
    await screen.findByRole("heading", { name: "Categories" });
    await expectNoSkippedLevels(container, "Storage");
  });

  it("Cleanup never skips a heading level", async () => {
    const { container } = render(<CleanupScreen api={makeApi()} />);
    await screen.findByRole("heading", { name: "Opportunities" });
    await expectNoSkippedLevels(container, "Cleanup");
  });

  it("History never skips a heading level", async () => {
    const { container } = render(<HistoryScreen api={makeApi()} onOpenCategory={() => undefined} />);
    await screen.findByRole("heading", { name: "Snapshot timeline" });
    await expectNoSkippedLevels(container, "History");
  });

  it("Drives never skips a heading level", async () => {
    const { container } = render(<DrivesScreen api={makeApi()} />);
    await screen.findByRole("heading", { name: "How drive memory works" });
    await expectNoSkippedLevels(container, "Drives");
  });
});
