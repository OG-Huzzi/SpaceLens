/**
 * The keyboard shortcut list.
 *
 * A small modal that answers "what can I press?". It is deliberately plain:
 * a table of keys and their meaning, a close button, Esc to dismiss, and a
 * focus trap that keeps the keyboard inside while it is open.
 *
 * The list is the reason the shell can be keyboard-first without hiding its
 * own behaviour: the palette shortcut is shown in the header, and the full map
 * is one keypress away.
 */

import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { PALETTE_COPY } from "./copy";
import "./command.css";

export interface ShortcutHelpProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** "⌘" on Apple platforms, "Ctrl" everywhere else. */
export function modifierKeyLabel(): string {
  if (typeof navigator === "undefined") return "Ctrl";
  const agent = `${navigator.userAgent ?? ""} ${navigator.platform ?? ""}`.toLowerCase();
  return agent.includes("mac") ? "⌘" : "Ctrl";
}

interface ShortcutRow {
  keys: string[];
  description: string;
}

export function shortcutRows(): ShortcutRow[] {
  const modifier = modifierKeyLabel();
  return [
    { keys: [modifier, "K"], description: PALETTE_COPY.shortcutDescriptions.open },
    { keys: ["↓", "↑", "J", "K"], description: PALETTE_COPY.shortcutDescriptions.move },
    { keys: ["Enter"], description: PALETTE_COPY.shortcutDescriptions.run },
    { keys: ["Esc"], description: PALETTE_COPY.shortcutDescriptions.close },
    { keys: ["Tab", "→"], description: PALETTE_COPY.shortcutDescriptions.cycleGroup },
    { keys: ["?"], description: PALETTE_COPY.shortcutDescriptions.list }
  ];
}

export function ShortcutHelpDialog({ open, onOpenChange }: ShortcutHelpProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const active = document.activeElement;
    restoreFocusRef.current = active instanceof HTMLElement ? active : null;
    dialogRef.current?.focus();
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

  useEffect(() => {
    if (!open) return;

    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (dialogRef.current === null) return;
      if (dialogRef.current.contains(target)) return;
      dialogRef.current.focus();
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onOpenChange(false);
    };

    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onOpenChange]);

  if (!open) return null;

  const rows = shortcutRows();

  return createPortal(
    <div
      className="cs-cmd-backdrop"
      onMouseDown={(event) => {
        if (event.target !== event.currentTarget) return;
        onOpenChange(false);
      }}
    >
      <div
        className="cs-cmd-shortcuts"
        role="dialog"
        aria-modal="true"
        aria-label={PALETTE_COPY.shortcutsTitle}
        tabIndex={-1}
        ref={dialogRef}
      >
        <h2 className="cs-cmd-shortcuts__title">{PALETTE_COPY.shortcutsTitle}</h2>
        <ul className="cs-cmd-shortcuts__list">
          {rows.map((row) => (
            <li className="cs-cmd-shortcuts__row" key={row.description}>
              <span className="cs-cmd-shortcuts__keys">
                {row.keys.map((key) => (
                  <kbd className="cs-cmd-shortcuts__key" key={key}>
                    {key}
                  </kbd>
                ))}
              </span>
              <span>{row.description}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>,
    document.body
  );
}
