/**
 * The command palette.
 *
 * KEYBOARD-FIRST, AND COMPLETE
 * ----------------------------
 * Cmd/Ctrl+K opens it from anywhere, Esc closes it, the selection moves with
 * the arrow keys (or j/k while the search field is empty), Enter runs the
 * highlighted command, Tab or the sideways arrow keys cycle through the
 * groups, and every printable character filters. The palette is a dialog with
 * `aria-modal`, a label, and a focus trap: focus cannot leave it while it is
 * open, and it returns to whatever opened it when it closes.
 *
 * IT IS NOT THE ONLY WAY TO REACH ANYTHING
 * ---------------------------------------
 * The navigation still works, the screen links still work, and the palette
 * only adds a faster route. It is an accelerator, never a gate.
 *
 * ASYNCHRONOUS SOURCES DO NOT BLOCK THE DIALOG
 * -------------------------------------------
 * Every source is loaded when the palette opens, but the dialog is rendered
 * immediately. A source that has not answered yet shows a pending row under
 * its own group heading; a source that fails shows a failed row. Neither takes
 * the palette down, and neither stops the commands that already arrived.
 *
 * THE PALETTE IS APP-AGNOSTIC
 * ---------------------------
 * It knows nothing about screens, storage or cleanup. It receives commands
 * through `CommandSource`s and reports a run through `onRun`. The CoreSight
 * specific command set lives in `coresightCommands.ts`.
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent
} from "react";
import { createPortal } from "react-dom";
import { t } from "../../i18n";
import { ACTIONS_LIVE_IN_CLEANUP, PALETTE_COPY } from "./copy";
import { filterCommands } from "./fuzzy";
import {
  PALETTE_DIALOG_ID,
  PALETTE_LIST_ID,
  type Command,
  type CommandPaletteProps
} from "./types";
import "./command.css";

type SourceStatus = "loading" | "ready" | "error";

interface PaletteSection {
  key: string;
  label: string;
  commands: { command: Command; score: number }[];
  pending: boolean;
  failed: boolean;
}

export function CommandPalette({
  open,
  onOpenChange,
  sources,
  onRun
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [loaded, setLoaded] = useState<Record<string, Command[]>>({});
  const [status, setStatus] = useState<Record<string, SourceStatus>>({});
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [announcement, setAnnouncement] = useState("");

  const inputRef = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const generationRef = useRef(0);
  const openRef = useRef(open);
  openRef.current = open;

  // The load effect must not re-run because the host handed us a new array
  // identity, so the sources are read through a ref and reloaded only when the
  // palette opens.
  const sourcesRef = useRef(sources);
  sourcesRef.current = sources;

  /* ----------------------------------------------------------------------- */
  /* Opening                                                                  */
  /* ----------------------------------------------------------------------- */

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (event.key.toLowerCase() !== "k") return;
      if (!event.metaKey && !event.ctrlKey) return;
      if (openRef.current) return;
      event.preventDefault();
      onOpenChange(true);
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onOpenChange]);

  /* ----------------------------------------------------------------------- */
  /* Loading the sources                                                      */
  /* ----------------------------------------------------------------------- */

  useEffect(() => {
    if (!open) return;

    const generation = generationRef.current + 1;
    generationRef.current = generation;

    setQuery("");
    setSelectedIndex(0);
    setAnnouncement("");

    const currentSources = sourcesRef.current;
    const initialStatus: Record<string, SourceStatus> = {};
    for (const source of currentSources) initialStatus[source.id] = "loading";
    setStatus(initialStatus);
    setLoaded({});

    let cancelled = false;

    for (const source of currentSources) {
      // `Promise.resolve().then(...)` turns a synchronously throwing loader
      // into a rejection instead of an exception during render.
      void Promise.resolve()
        .then(() => source.load())
        .then(
          (commands) => {
            if (cancelled || generationRef.current !== generation) return;
            setLoaded((previous) => ({ ...previous, [source.id]: commands }));
            setStatus((previous) => ({ ...previous, [source.id]: "ready" }));
          },
          () => {
            if (cancelled || generationRef.current !== generation) return;
            setStatus((previous) => ({ ...previous, [source.id]: "error" }));
          }
        );
    }

    return () => {
      cancelled = true;
    };
  }, [open]);

  /* ----------------------------------------------------------------------- */
  /* Focus: in on open, back to the trigger on close                          */
  /* ----------------------------------------------------------------------- */

  useEffect(() => {
    if (!open) return;

    const active = document.activeElement;
    restoreFocusRef.current = active instanceof HTMLElement ? active : null;

    inputRef.current?.focus();
  }, [open]);

  useEffect(() => {
    if (open) return;
    const target = restoreFocusRef.current;
    restoreFocusRef.current = null;
    if (target === null) return;
    if (!target.isConnected) return;
    if (target === document.body) return;
    target.focus();
  }, [open]);

  // The dialog is modal, so focus is pulled back whenever something outside
  // tries to take it.
  useEffect(() => {
    if (!open) return;

    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (dialogRef.current === null) return;
      if (dialogRef.current.contains(target)) return;
      inputRef.current?.focus();
    };

    document.addEventListener("focusin", onFocusIn);
    return () => document.removeEventListener("focusin", onFocusIn);
  }, [open]);

  /* ----------------------------------------------------------------------- */
  /* Results                                                                  */
  /* ----------------------------------------------------------------------- */

  const allCommands = useMemo(() => Object.values(loaded).flat(), [loaded]);
  const scored = useMemo(() => filterCommands(query, allCommands), [query, allCommands]);

  const sections = useMemo<PaletteSection[]>(() => {
    const scoreById = new Map<string, number>();
    const rankById = new Map<string, number>();
    scored.forEach((entry, index) => {
      scoreById.set(entry.command.id, entry.score);
      rankById.set(entry.command.id, index);
    });

    const result: PaletteSection[] = [];
    const byGroup = new Map<string, PaletteSection>();

    for (const source of sources) {
      const state = status[source.id] ?? "loading";

      if (state === "loading") {
        result.push({
          key: `pending:${source.id}`,
          label: source.label,
          commands: [],
          pending: true,
          failed: false
        });
        continue;
      }

      if (state === "error") {
        result.push({
          key: `error:${source.id}`,
          label: source.label,
          commands: [],
          pending: false,
          failed: true
        });
        continue;
      }

      const commands = loaded[source.id] ?? [];
      const matched = commands
        .map((command) => ({ command, score: scoreById.get(command.id) }))
        .filter(
          (entry): entry is { command: Command; score: number } => entry.score !== undefined
        )
        .sort((left, right) => {
          const leftRank = rankById.get(left.command.id) ?? 0;
          const rightRank = rankById.get(right.command.id) ?? 0;
          return leftRank - rightRank;
        });

      for (const entry of matched) {
        const groupKey = entry.command.group;
        let section = byGroup.get(groupKey);
        if (section === undefined) {
          section = {
            key: `group:${groupKey}`,
            label: groupKey,
            commands: [],
            pending: false,
            failed: false
          };
          byGroup.set(groupKey, section);
          result.push(section);
        }
        section.commands.push(entry);
      }
    }

    return result;
  }, [sources, status, loaded, scored]);

  const flat = useMemo(
    () => sections.flatMap((section) => section.commands.map((entry) => entry.command)),
    [sections]
  );

  const indexById = useMemo(() => {
    const map = new Map<string, number>();
    flat.forEach((command, index) => map.set(command.id, index));
    return map;
  }, [flat]);

  const activeIndex =
    flat.length === 0 ? -1 : Math.min(Math.max(selectedIndex, 0), flat.length - 1);

  const hasPending = sections.some((section) => section.pending);

  /* ----------------------------------------------------------------------- */
  /* Movement                                                                 */
  /* ----------------------------------------------------------------------- */

  const move = useCallback(
    (delta: number) => {
      if (flat.length === 0) return;
      setSelectedIndex((current) => {
        const base = Math.min(Math.max(current, 0), flat.length - 1);
        return (base + delta + flat.length) % flat.length;
      });
    },
    [flat.length]
  );

  const cycleGroup = useCallback(
    (direction: number) => {
      const starts: number[] = [];
      let cursor = 0;
      for (const section of sections) {
        if (section.commands.length > 0) starts.push(cursor);
        cursor += section.commands.length;
      }
      if (starts.length === 0) return;

      const current = activeIndex < 0 ? starts[0] : activeIndex;
      let groupIndex = 0;
      for (let index = 0; index < starts.length; index += 1) {
        if (starts[index] <= current) groupIndex = index;
      }

      const next =
        direction > 0
          ? (groupIndex + 1) % starts.length
          : (groupIndex - 1 + starts.length) % starts.length;
      setSelectedIndex(starts[next]);
    },
    [sections, activeIndex]
  );

  const runCommand = useCallback(
    (command: Command) => {
      if (command.disabled === true) {
        setAnnouncement(command.disabledReason ?? ACTIONS_LIVE_IN_CLEANUP);
        return;
      }
      setAnnouncement("");
      onRun(command);
      onOpenChange(false);
    },
    [onRun, onOpenChange]
  );

  const runSelected = useCallback(() => {
    if (activeIndex < 0) return;
    const command = flat[activeIndex];
    if (command === undefined) return;
    runCommand(command);
  }, [activeIndex, flat, runCommand]);

  /* ----------------------------------------------------------------------- */
  /* Keyboard                                                                 */
  /* ----------------------------------------------------------------------- */

  const handleKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLInputElement>) => {
      switch (event.key) {
        case "Escape":
          event.preventDefault();
          onOpenChange(false);
          return;
        case "ArrowDown":
          event.preventDefault();
          move(1);
          return;
        case "ArrowUp":
          event.preventDefault();
          move(-1);
          return;
        case "Enter":
          event.preventDefault();
          runSelected();
          return;
        case "Tab":
          // Tab cycles the groups and never leaves the dialog: the search
          // field is the only focusable element inside it.
          event.preventDefault();
          cycleGroup(event.shiftKey ? -1 : 1);
          return;
        case "ArrowRight":
          event.preventDefault();
          cycleGroup(1);
          return;
        case "ArrowLeft":
          event.preventDefault();
          cycleGroup(-1);
          return;
        case "j":
        case "k":
          // j/k are the vim-style movement keys, but only while the search
          // field is empty — otherwise they are ordinary search characters.
          if (query !== "") return;
          event.preventDefault();
          move(event.key === "j" ? 1 : -1);
          return;
        default:
          break;
      }
    },
    [cycleGroup, move, onOpenChange, query, runSelected]
  );

  /* ----------------------------------------------------------------------- */
  /* Render                                                                   */
  /* ----------------------------------------------------------------------- */

  if (!open) return null;

  const backdropMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget) return;
    onOpenChange(false);
  };

  const activeCommandId =
    activeIndex >= 0 ? `${PALETTE_DIALOG_ID}-option-${activeIndex}` : undefined;

  return createPortal(
    <div className="cs-cmd-backdrop" onMouseDown={backdropMouseDown}>
      <div
        className="cs-cmd-palette"
        role="dialog"
        aria-modal="true"
        aria-label={PALETTE_COPY.searchLabel}
        id={PALETTE_DIALOG_ID}
        ref={dialogRef}
      >
        <div className="cs-cmd-palette__search">
          <label className="cs-cmd-visually-hidden" htmlFor="coresight-command-input">
            {PALETTE_COPY.searchLabel}
          </label>
          <input
            id="coresight-command-input"
            ref={inputRef}
            className="cs-cmd-palette__input"
            type="text"
            role="combobox"
            aria-expanded="true"
            aria-controls={PALETTE_LIST_ID}
            aria-autocomplete="list"
            aria-activedescendant={activeCommandId}
            autoComplete="off"
            spellCheck={false}
            placeholder={PALETTE_COPY.placeholder}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={handleKeyDown}
          />
        </div>

        <div
          className="cs-cmd-palette__list"
          role="listbox"
          aria-label={PALETTE_COPY.searchLabel}
          id={PALETTE_LIST_ID}
        >
          {sections.map((section) => (
            <div
              className="cs-cmd-palette__group"
              role="group"
              aria-label={section.label}
              key={section.key}
            >
              <div className="cs-cmd-palette__group-label" aria-hidden="true">
                {section.label}
              </div>

              {section.pending ? (
                <p className="cs-cmd-palette__pending" role="status">
                  {t("state.loading.label")}…
                </p>
              ) : null}

              {section.failed ? (
                <p className="cs-cmd-palette__failed" role="status">
                  {PALETTE_COPY.sourceFailed}
                </p>
              ) : null}

              {section.commands.length > 0 ? (
                <ul className="cs-cmd-palette__options" role="presentation">
                  {section.commands.map((entry) => {
                    const index = indexById.get(entry.command.id) ?? -1;
                    const disabled = entry.command.disabled === true;
                    return (
                      <li
                        className="cs-cmd-palette__option"
                        id={`${PALETTE_DIALOG_ID}-option-${index}`}
                        key={entry.command.id}
                        role="option"
                        aria-selected={index === activeIndex}
                        aria-disabled={disabled ? true : undefined}
                        data-disabled={disabled ? "true" : undefined}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => runCommand(entry.command)}
                      >
                        <span className="cs-cmd-palette__option-label">
                          {entry.command.label}
                        </span>
                        {disabled && entry.command.disabledReason ? (
                          <span className="cs-cmd-palette__option-reason">
                            {entry.command.disabledReason}
                          </span>
                        ) : null}
                        {entry.command.shortcut ? (
                          <kbd className="cs-cmd-palette__option-shortcut">
                            {entry.command.shortcut}
                          </kbd>
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              ) : null}
            </div>
          ))}
        </div>

        {flat.length === 0 && !hasPending ? (
          <div className="cs-cmd-palette__empty" role="status">
            <p className="cs-cmd-palette__empty-title">{PALETTE_COPY.noResultsTitle}</p>
            <p className="cs-cmd-palette__empty-body">{PALETTE_COPY.noResultsBody}</p>
          </div>
        ) : null}

        <p className="cs-cmd-visually-hidden" role="status" aria-live="polite">
          {announcement}
        </p>
      </div>
    </div>,
    document.body
  );
}
