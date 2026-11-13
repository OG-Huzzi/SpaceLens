import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { createMockApi } from "../../api";
import { defaultSettings } from "../../settings/schema";
import { CommandPalette } from "./CommandPalette";
import { ACTIONS_LIVE_IN_CLEANUP, PALETTE_COPY } from "./copy";
import { buildCommandSources, commandTarget } from "./coresightCommands";
import { filterCommands, scoreCommand } from "./fuzzy";
import type { Command, CommandSource } from "./types";

/**
 * The palette is tested through its public surface only: the host owns the
 * open state and hands the palette a set of sources. Nothing in this file
 * reaches into the component's internals.
 */
function Harness({
  sources,
  onRun
}: {
  sources: CommandSource[];
  onRun: (command: Command) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open commands
      </button>
      <CommandPalette
        open={open}
        onOpenChange={setOpen}
        sources={sources}
        onRun={onRun}
      />
    </>
  );
}

function staticSource(commands: Command[], label = "Screens"): CommandSource {
  return { id: "static", label, load: async () => commands };
}

async function openWithKeyboard(): Promise<HTMLElement> {
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  return screen.findByRole("dialog");
}

async function openWithButton(): Promise<HTMLElement> {
  const trigger = screen.getByRole("button", { name: "Open commands" });
  trigger.focus();
  fireEvent.click(trigger);
  await screen.findByRole("dialog");
  return trigger;
}

const SAMPLE: Command[] = [
  { id: "nav.home", label: "Home", group: "Screens" },
  { id: "nav.cleanup", label: "Cleanup", group: "Screens" },
  { id: "nav.storage", label: "Storage", group: "Screens" }
];

describe("command palette keyboard map", () => {
  it("opens with Ctrl+K and closes with Esc", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);

    const dialog = await openWithKeyboard();
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveAccessibleName(PALETTE_COPY.searchLabel);

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("returns the focus to the trigger when it closes", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);

    const trigger = await openWithButton();
    expect(screen.getByRole("combobox")).toHaveFocus();

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });

  it("moves the selection with the arrow keys and runs it with Enter", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);
    await openWithKeyboard();

    const options = screen.getAllByRole("option");
    expect(options[0]).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });
    expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowUp" });
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    expect(onRun).toHaveBeenCalledTimes(1);
    expect(onRun).toHaveBeenCalledWith(expect.objectContaining({ id: "nav.home" }));
  });

  it("cycles the groups with Tab", async () => {
    const onRun = vi.fn();
    render(
      <Harness
        sources={[
          staticSource([
            { id: "nav.home", label: "Home", group: "Screens" },
            { id: "nav.cleanup", label: "Cleanup", group: "Screens" },
            { id: "panel.dns", label: "DNS resolver ranking", group: "Reports" }
          ])
        ]}
        onRun={onRun}
      />
    );
    await openWithKeyboard();

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Tab" });
    expect(screen.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Tab", shiftKey: true });
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
  });

  it("runs a command with the mouse as well", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);
    await openWithKeyboard();

    fireEvent.click(screen.getByRole("option", { name: "Cleanup" }));
    expect(onRun).toHaveBeenCalledWith(expect.objectContaining({ id: "nav.cleanup" }));
  });
});

describe("command palette fuzzy matching", () => {
  it("ranks a scattered subsequence match for \"clnup\" above nothing else", () => {
    const ranked = filterCommands("clnup", SAMPLE);
    expect(ranked[0].command.label).toBe("Cleanup");
  });

  it("scores a prefix match above a scattered match", () => {
    const prefix = scoreCommand("cle", { id: "a", label: "Cleanup", group: "g" });
    const scattered = scoreCommand("clnup", { id: "b", label: "Cleanup", group: "g" });
    expect(prefix).not.toBeNull();
    expect(scattered).not.toBeNull();
    expect(prefix ?? 0).toBeGreaterThan(scattered ?? 0);
  });

  it("filters the list as characters are typed", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);
    await openWithKeyboard();

    fireEvent.change(screen.getByRole("combobox"), { target: { value: "clnup" } });

    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent("Cleanup");
  });
});

describe("command palette states", () => {
  it("shows a disabled command with its reason and refuses to run it", async () => {
    const onRun = vi.fn();
    render(
      <Harness
        sources={[
          staticSource([
            {
              id: "action.cleanup.confirm",
              label: "Confirm move to Trash",
              group: "Cleanup",
              disabled: true,
              disabledReason: ACTIONS_LIVE_IN_CLEANUP
            }
          ])
        ]}
        onRun={onRun}
      />
    );
    await openWithKeyboard();

    const option = screen.getByRole("option", { name: /Confirm move to Trash/ });
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(option).toHaveTextContent(ACTIONS_LIVE_IN_CLEANUP);

    fireEvent.click(option);
    expect(onRun).not.toHaveBeenCalled();

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    expect(onRun).not.toHaveBeenCalled();
  });

  it("opens before an async source resolves and shows a pending state", async () => {
    const onRun = vi.fn();
    let resolve: (commands: Command[]) => void = () => undefined;
    const source: CommandSource = {
      id: "async",
      label: "Reports",
      load: () =>
        new Promise<Command[]>((settle) => {
          resolve = settle;
        })
    };

    render(<Harness sources={[source]} onRun={onRun} />);

    const dialog = await openWithKeyboard();
    expect(dialog).toBeInTheDocument();
    expect(screen.getByText("Reports")).toBeInTheDocument();
    expect(screen.getByText(/^Loading/)).toBeInTheDocument();

    await act(async () => {
      resolve([{ id: "panel.dns", label: "DNS resolver ranking", group: "Reports" }]);
    });

    await waitFor(() =>
      expect(screen.getByRole("option", { name: "DNS resolver ranking" })).toBeInTheDocument()
    );
    expect(screen.queryByText(/^Loading/)).not.toBeInTheDocument();
  });

  it("says so when nothing matches", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);
    await openWithKeyboard();

    fireEvent.change(screen.getByRole("combobox"), { target: { value: "zzzzzz" } });

    expect(screen.getByText(PALETTE_COPY.noResultsTitle)).toBeInTheDocument();
    expect(screen.getByText(PALETTE_COPY.noResultsBody)).toBeInTheDocument();
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  it("keeps the focus inside the dialog while it is open", async () => {
    const onRun = vi.fn();
    render(<Harness sources={[staticSource(SAMPLE)]} onRun={onRun} />);
    const dialog = await openWithKeyboard();

    expect(dialog.contains(document.activeElement)).toBe(true);

    // Tab cycles the groups instead of leaving the dialog.
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Tab" });
    expect(dialog.contains(document.activeElement)).toBe(true);

    // Focus that is forced outside is pulled back in.
    screen.getByRole("button", { name: "Open commands" }).focus();
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
  });
});

describe("the CoreSight command set", () => {
  function sourcesFor(visibleScreens = defaultSettings().visibleScreens) {
    return buildCommandSources({
      api: createMockApi({ latencyMs: 0 }),
      visibleScreens
    });
  }

  async function loadAll(sources: CommandSource[]): Promise<Command[]> {
    const groups = await Promise.all(sources.map((source) => source.load()));
    return groups.flat();
  }

  it("never offers a runnable state-changing command", async () => {
    const commands = await loadAll(sourcesFor());

    const actions = commands.filter((command) => command.id.startsWith("action."));
    expect(actions.length).toBeGreaterThan(0);
    for (const action of actions) {
      expect(action.disabled).toBe(true);
      expect(action.disabledReason).toBe(ACTIONS_LIVE_IN_CLEANUP);
      // A state-changing command has no effect at all, not even a disabled one.
      expect(commandTarget(action)).toBeNull();
    }

    for (const command of commands) {
      if (command.id.startsWith("action.")) continue;
      expect(commandTarget(command)).not.toBeNull();
    }
  });

  it("reaches every visible screen and the settings route", async () => {
    const commands = await loadAll(sourcesFor());
    const ids = commands.map((command) => command.id);

    expect(ids).toContain("nav.home");
    expect(ids).toContain("nav.storage");
    expect(ids).toContain("nav.cleanup");
    expect(ids).toContain("nav.history");
    expect(ids).toContain("nav.drives");
    expect(ids).toContain("nav.settings");
  });

  it("does not reach a screen that Settings hid", async () => {
    const commands = await loadAll(
      sourcesFor({ ...defaultSettings().visibleScreens, home: false })
    );
    const ids = commands.map((command) => command.id);

    expect(ids).not.toContain("nav.home");
    // The privacy report lives on Home, so a hidden Home hides it too.
    expect(ids).not.toContain("panel.privacy");
    expect(ids).toContain("panel.startup");
  });

  it("resolves only navigation and read-only report targets", async () => {
    const commands = await loadAll(sourcesFor());
    for (const command of commands) {
      const target = commandTarget(command);
      if (target === null) continue;
      expect(["navigate", "panel"]).toContain(target.kind);
    }
  });
});
