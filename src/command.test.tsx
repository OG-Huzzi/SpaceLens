import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { ACTIONS_LIVE_IN_CLEANUP, PALETTE_COPY } from "./components/command";
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

/**
 * Shell-level tests for the command palette.
 *
 * The palette is an accelerator, never a gate: the navigation has to keep
 * working while it exists, and a screen that Settings hid must not become
 * reachable through the palette.
 *
 * Every query is scoped to the palette dialog. The shell also owns two
 * `<select>` elements, and a native select exposes its choices as options
 * too — an unscoped "option" query would read the theme control instead of
 * the command list.
 */
function palette() {
  return within(screen.getByRole("dialog", { name: PALETTE_COPY.searchLabel }));
}

describe("the command palette in the app shell", () => {
  it("opens with Ctrl+K without taking the navigation away", async () => {
    render(<App />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });

    const dialog = await screen.findByRole("dialog", { name: PALETTE_COPY.searchLabel });
    expect(dialog).toHaveAttribute("aria-modal", "true");

    // The navigation is still there: the palette is an extra route, not the
    // only one.
    expect(screen.getByRole("navigation", { name: "Primary" })).toBeInTheDocument();
  });

  it("opens from the visible header button and gives the focus back", async () => {
    render(<App />);

    const trigger = await screen.findByRole("button", { name: /Search commands/ });
    trigger.focus();
    fireEvent.click(trigger);

    await screen.findByRole("dialog", { name: PALETTE_COPY.searchLabel });

    fireEvent.keyDown(palette().getByRole("combobox"), { key: "Escape" });
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });

  it("navigates when a screen command runs", async () => {
    render(<App />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    await screen.findByRole("dialog", { name: PALETTE_COPY.searchLabel });

    const option = await palette().findByRole("option", { name: "Storage" });
    fireEvent.click(option);

    await waitFor(() =>
      expect(screen.getByRole("link", { name: "Storage" })).toHaveAttribute(
        "aria-current",
        "page"
      )
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("does not offer a screen that Settings hid", async () => {
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

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    await screen.findByRole("dialog", { name: PALETTE_COPY.searchLabel });

    await palette().findByRole("option", { name: "Storage" });
    expect(palette().queryByRole("option", { name: "Home" })).not.toBeInTheDocument();
    // The privacy report lives on Home, so a hidden Home hides it too.
    expect(
      palette().queryByRole("option", { name: "Privacy findings" })
    ).not.toBeInTheDocument();
  });

  it("offers no runnable state-changing command", async () => {
    render(<App />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    await screen.findByRole("dialog", { name: PALETTE_COPY.searchLabel });

    const disabled = await palette().findByRole("option", { name: /Confirm move to Trash/ });
    expect(disabled).toHaveAttribute("aria-disabled", "true");
    expect(disabled).toHaveTextContent(ACTIONS_LIVE_IN_CLEANUP);

    // The report group is loaded from the engine, so it arrives last.
    await palette().findByRole("option", { name: "Startup inventory" });

    const enabled = palette()
      .getAllByRole("option")
      .filter((option) => option.getAttribute("aria-disabled") !== "true")
      .map((option) => (option.textContent ?? "").trim())
      .sort();

    expect(enabled).toEqual(
      [
        "Block-list plan",
        "Cleanup",
        "DNS resolver ranking",
        "Drives",
        "History",
        "Home",
        "Privacy findings",
        "Settings",
        "Startup inventory",
        "Storage"
      ].sort()
    );
  });

  it("jumps to a read-only report and puts the focus on its heading", async () => {
    render(<App />);

    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    await screen.findByRole("dialog", { name: PALETTE_COPY.searchLabel });

    const option = await palette().findByRole("option", { name: "Startup inventory" });
    fireEvent.click(option);

    // The report lives on History, so History becomes the current screen…
    await waitFor(() =>
      expect(screen.getByRole("link", { name: "History" })).toHaveAttribute(
        "aria-current",
        "page"
      )
    );

    // …and the keyboard continues from the report heading, not from the top
    // of the page.
    await waitFor(() =>
      expect(document.activeElement?.textContent ?? "").toContain("Startup inventory")
    );
  });

  it("opens the shortcut list with ?", async () => {
    render(<App />);

    fireEvent.keyDown(window, { key: "?" });

    const dialog = await screen.findByRole("dialog", { name: PALETTE_COPY.shortcutsTitle });
    expect(dialog).toBeInTheDocument();
  });
});
