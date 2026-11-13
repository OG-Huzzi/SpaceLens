import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { defaultSettings, type Settings } from "./settings/schema";
import { saveSettings } from "./settings/storage";

function seed(settings: Settings) {
  expect(saveSettings(settings)).toBe(true);
}

function resetDocument() {
  const root = document.documentElement;
  for (const attribute of [
    "data-theme",
    "data-density",
    "data-reduced-motion",
    "data-advanced-details"
  ]) {
    root.removeAttribute(attribute);
  }
}

beforeEach(() => {
  window.localStorage.clear();
  window.history.replaceState(null, "", "/");
  resetDocument();
});

afterEach(() => {
  window.localStorage.clear();
  resetDocument();
});

describe("app shell", () => {
  it("reaches Settings from the navigation and returns to a pillar screen", async () => {
    render(<App />);

    fireEvent.click(await screen.findByRole("link", { name: "Settings" }));
    expect(await screen.findByRole("heading", { name: "Preferences" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("link", { name: "Home" }));
    expect(
      await screen.findByRole("heading", { name: "Largest categories" })
    ).toBeInTheDocument();
  });

  it("keeps the ambient scan strip on every screen", async () => {
    render(<App />);

    const strip = await screen.findByRole("status", { name: "Scan status" });
    expect(strip).toHaveAttribute("aria-live", "polite");
    expect(strip.querySelectorAll("button, a, input")).toHaveLength(0);

    fireEvent.click(await screen.findByRole("link", { name: "Settings" }));
    await screen.findByRole("heading", { name: "Preferences" });
    expect(screen.getByRole("status", { name: "Scan status" })).toBeInTheDocument();
  });

  it("omits a hidden screen from the navigation and its route", async () => {
    seed({
      ...defaultSettings(),
      visibleScreens: {
        home: false,
        storage: true,
        cleanup: true,
        history: true,
        drives: true
      }
    });

    render(<App />);

    expect(await screen.findByRole("link", { name: "Storage" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Home" })).not.toBeInTheDocument();
    // The default route (#home) is hidden, so the first visible screen answers.
    expect(await screen.findByRole("heading", { name: "Categories" })).toBeInTheDocument();
  });

  it("applies the persisted theme before the first screen paints", async () => {
    seed({ ...defaultSettings(), theme: "dark" });

    render(<App />);

    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    await screen.findByRole("link", { name: "Home" });
  });
});
