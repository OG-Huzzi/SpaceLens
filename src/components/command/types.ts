/**
 * The command palette's data vocabulary.
 *
 * The palette component is deliberately app-agnostic: it knows how to search,
 * rank, group and run commands, but nothing about CoreSight. The application
 * supplies the commands through `CommandSource`s, which keeps the component
 * testable without a router, an API, or a settings store.
 */

/**
 * One runnable (or deliberately unrunnable) destination.
 *
 * `id` is the stable identity used by the host to decide what a command does.
 * The palette itself never interprets it: it shows the label, runs the
 * callback, and stays out of the application's business.
 */
export interface Command {
  id: string;
  label: string;
  /** Heading the command is listed under. */
  group: string;
  /** Extra text a search should look at, without showing it in the list. */
  keywords?: string[];
  /** A key hint shown on the right of the row, e.g. "Ctrl K". */
  shortcut?: string;
  /**
   * A disabled command is shown, never hidden: the reason travels with it so
   * the boundary stays visible. A disabled command cannot be run.
   */
  disabled?: boolean;
  /**
   * Why the command cannot be run. Required in practice whenever a command is
   * disabled — a disabled row without a reason is a dead end.
   */
  disabledReason?: string;
}

/**
 * A group of commands that is loaded lazily.
 *
 * `load` may resolve at any time; the palette opens without waiting for it and
 * shows the group as pending until it arrives. A source that rejects marks its
 * group as failed rather than taking the whole palette down.
 */
export interface CommandSource {
  id: string;
  /** Group heading shown while the source is still loading. */
  label: string;
  load: () => Promise<Command[]>;
}

export interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Command sources, injected so the palette stays testable and app-agnostic. */
  sources: CommandSource[];
  onRun: (command: Command) => void;
}

/** Stable DOM id of the palette dialog, used by the header trigger. */
export const PALETTE_DIALOG_ID = "coresight-command-palette";

/** Stable DOM id of the palette's listbox. */
export const PALETTE_LIST_ID = "coresight-command-list";
