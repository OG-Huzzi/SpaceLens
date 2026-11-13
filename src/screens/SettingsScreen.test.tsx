import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { SettingsProvider } from "../settings/SettingsProvider";
import { SETTINGS_STORAGE_KEY } from "../settings/schema";
import { loadSettings } from "../settings/storage";
import { SettingsScreen } from "./SettingsScreen";

function renderSettings() {
  return render(
    <SettingsProvider>
      <SettingsScreen />
    </SettingsProvider>
  );
}

function resetDocumentPreferences() {
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
  resetDocumentPreferences();
});

afterEach(() => {
  window.localStorage.clear();
  resetDocumentPreferences();
});

describe("Settings screen", () => {
  it("renders every preference group", () => {
    renderSettings();

    for (const heading of ["Language", "Appearance", "Screens", "Details", "About these preferences"]) {
      expect(screen.getByRole("heading", { name: heading })).toBeInTheDocument();
    }

    for (const name of ["Home", "Storage", "Cleanup", "History", "Drives"]) {
      expect(screen.getByRole("checkbox", { name })).toBeChecked();
    }
    expect(screen.getByRole("checkbox", { name: "Show advanced details" })).not.toBeChecked();
    expect(screen.getByRole("radio", { name: "System" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Comfortable" })).toBeChecked();
    expect(screen.getByLabelText("Interface language")).toHaveValue("en");
  });

  it("contains no data-destroying action", () => {
    renderSettings();

    const controls = [
      ...screen.queryAllByRole("button"),
      ...screen.queryAllByRole("link")
    ];
    expect(controls.length).toBeGreaterThan(0);

    const destructive =
      /delete|del\b|remove|erase|wipe|purge|shred|format|clean|empty trash|scan now|start scan|execute/i;
    for (const control of controls) {
      const name = control.getAttribute("aria-label") ?? control.textContent ?? "";
      expect(name, `control "${name}" must not be destructive`).not.toMatch(destructive);
    }

    // No form, no submit, no cleanup affordance can live here.
    expect(document.querySelectorAll("form")).toHaveLength(0);
    expect(document.querySelectorAll("input[type='submit']")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: /scan|cleanup/i })).not.toBeInTheDocument();
  });

  it("applies the theme preference to the document root", () => {
    renderSettings();
    const root = document.documentElement;
    expect(root.hasAttribute("data-theme")).toBe(false);

    fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    expect(root.getAttribute("data-theme")).toBe("dark");

    fireEvent.click(screen.getByRole("radio", { name: "Light" }));
    expect(root.getAttribute("data-theme")).toBe("light");

    fireEvent.click(screen.getByRole("radio", { name: "System" }));
    expect(root.hasAttribute("data-theme")).toBe(false);
  });

  it("applies density and reduced motion to the document root", () => {
    renderSettings();
    const root = document.documentElement;
    expect(root.getAttribute("data-density")).toBe("comfortable");
    expect(root.hasAttribute("data-reduced-motion")).toBe(false);

    fireEvent.click(screen.getByRole("radio", { name: "Compact" }));
    fireEvent.click(screen.getByRole("radio", { name: "Reduce motion" }));

    expect(root.getAttribute("data-density")).toBe("compact");
    expect(root.getAttribute("data-reduced-motion")).toBe("reduce");

    fireEvent.click(screen.getByRole("radio", { name: "Allow motion" }));
    expect(root.getAttribute("data-reduced-motion")).toBe("allow");
  });

  it("persists changes through the versioned storage key and restores them", () => {
    const { unmount } = renderSettings();

    fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
    fireEvent.click(screen.getByRole("radio", { name: "Compact" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Show advanced details" }));

    expect(window.localStorage.getItem(SETTINGS_STORAGE_KEY)).not.toBeNull();
    expect(loadSettings().theme).toBe("dark");

    unmount();
    renderSettings();

    expect(screen.getByRole("radio", { name: "Dark" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Compact" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Show advanced details" })).toBeChecked();
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.getAttribute("data-density")).toBe("compact");
  });

  it("switches the interface language live and persists the locale", () => {
    renderSettings();

    fireEvent.change(screen.getByLabelText("Interface language"), {
      target: { value: "de" }
    });

    expect(loadSettings().locale).toBe("de");
    expect(screen.getByRole("heading", { name: "Sprache" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Darstellung" })).toBeInTheDocument();
    expect(document.documentElement.getAttribute("lang")).toBe("de");
  });

  it("never lets the last visible screen be hidden", () => {
    renderSettings();

    for (const name of ["Home", "Storage", "Cleanup", "History"]) {
      fireEvent.click(screen.getByRole("checkbox", { name }));
    }

    const drives = screen.getByRole("checkbox", { name: "Drives" });
    expect(drives).toBeChecked();
    expect(drives).toBeDisabled();

    const stored = loadSettings();
    expect(stored.visibleScreens.drives).toBe(true);
    expect(Object.values(stored.visibleScreens).filter(Boolean)).toHaveLength(1);
  });
});
