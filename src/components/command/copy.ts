/**
 * Palette copy that the message catalogue does not cover yet.
 *
 * `src/i18n/catalogue.ts` is closed for this task: it enforces en/de parity at
 * compile time and adding a key would break the build for every other worker
 * on the tree. The handful of strings below are therefore plain English
 * literals, collected in one file so that the catalogue owner can move them
 * across in a single pass (each entry needs one `en` and one `de` key).
 *
 * Everything else the palette shows — screen names, panel titles, "Loading",
 * "Read-only", "Unknown", "Settings" — comes from the existing catalogue.
 */

/** The wording the UX architecture requires for a state-changing command. */
export const ACTIONS_LIVE_IN_CLEANUP = "Actions live in Cleanup";

export const PALETTE_COPY = {
  /** Label of the header trigger and of the dialog itself. */
  searchLabel: "Search commands",
  /** Placeholder inside the search field. */
  placeholder: "Search screens, reports and settings",
  /** Group heading for navigation commands. */
  screensGroup: "Screens",
  /** Group heading for the read-only report commands. */
  reportsGroup: "Reports",
  /** Shown when nothing matches the current query. */
  noResultsTitle: "No matching commands",
  /** What to try when nothing matches. */
  noResultsBody:
    "Try a screen name such as Cleanup, a report name, or clear the search.",
  /** Shown when a source could not be loaded. */
  sourceFailed: "This group could not be loaded.",
  /** Label of the shortcut list dialog and of the footer trigger. */
  shortcutsTitle: "Keyboard shortcuts",
  /** Descriptions of the keyboard map, one row per shortcut. */
  shortcutDescriptions: {
    open: "Open the command palette",
    move: "Move the selection",
    run: "Run the highlighted command",
    close: "Close the palette",
    cycleGroup: "Cycle through the groups",
    list: "Show this list"
  }
} as const;
