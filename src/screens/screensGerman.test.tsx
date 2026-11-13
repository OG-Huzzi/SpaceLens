import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createMockApi } from "../api";
import { setActiveLocale } from "../i18n";
import { DrivesScreen } from "./DrivesScreen";
import { HistoryScreen } from "./HistoryScreen";
import { HomeScreen } from "./HomeScreen";
import { StorageScreen } from "./StorageScreen";

afterEach(() => {
  setActiveLocale("en");
});

function makeApi() {
  return createMockApi({ latencyMs: 0 });
}

/**
 * One heading assertion per informational screen is enough to prove the screen
 * is reading the active locale rather than a hard-coded English literal. The
 * full catalogue parity tests cover the rest of the strings.
 *
 * Each test first waits for a heading that only exists once the mock snapshot
 * has loaded. The loading state also renders a translated h1, but that node is
 * replaced when the data arrives, so asserting on it directly would race.
 */
describe("the four informational screens render in German", () => {
  it("renders the Home heading in German", async () => {
    setActiveLocale("de");
    render(<HomeScreen api={makeApi()} />);

    await screen.findByRole("heading", { name: "Größte Kategorien" });
    expect(
      screen.getByRole("heading", {
        level: 1,
        name: "Wie steht es um deinen Speicher?"
      })
    ).toBeInTheDocument();
  });

  it("renders the Storage heading in German", async () => {
    setActiveLocale("de");
    render(<StorageScreen api={makeApi()} focusCategoryId={null} />);

    await screen.findByRole("heading", { name: "Kategorien" });
    expect(
      screen.getByRole("heading", {
        level: 1,
        name: "Was belegt meinen Speicher?"
      })
    ).toBeInTheDocument();
  });

  it("renders the History heading in German", async () => {
    setActiveLocale("de");
    render(<HistoryScreen api={makeApi()} onOpenCategory={() => undefined} />);

    await screen.findByRole("heading", { name: "Zeitleiste der Momentaufnahmen" });
    expect(
      screen.getByRole("heading", {
        level: 1,
        name: "Was hat sich geändert?"
      })
    ).toBeInTheDocument();
  });

  it("renders the Drives heading in German", async () => {
    setActiveLocale("de");
    render(<DrivesScreen api={makeApi()} />);

    await screen.findByRole("heading", {
      name: "Wie das Laufwerksgedächtnis funktioniert"
    });
    expect(
      screen.getByRole("heading", {
        level: 1,
        name: "Welche Laufwerke habe ich?"
      })
    ).toBeInTheDocument();
  });
});
