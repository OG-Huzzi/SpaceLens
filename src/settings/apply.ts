/**
 * Applies preferences to the document root as data attributes.
 *
 * CSS owns the visual result (`:root[data-theme="dark"]`, `[data-density]`,
 * `[data-reduced-motion]`); this module only states the intent. When a
 * preference is "system" the attribute is *removed*, so the OS media query is
 * the only thing deciding — that is what "follow the system" has to mean.
 */

import type { Settings } from "./schema";

export function applySettingsToDocument(
  settings: Settings,
  root: HTMLElement = document.documentElement
): void {
  if (settings.theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", settings.theme);
  }

  root.setAttribute("data-density", settings.density);

  if (settings.reducedMotion === "system") {
    root.removeAttribute("data-reduced-motion");
  } else {
    root.setAttribute(
      "data-reduced-motion",
      settings.reducedMotion === "reduce" ? "reduce" : "allow"
    );
  }

  root.setAttribute("data-advanced-details", settings.showAdvancedDetails ? "on" : "off");
  root.setAttribute("lang", settings.locale);
}
