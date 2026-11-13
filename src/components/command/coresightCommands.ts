/**
 * The CoreSight command set.
 *
 * This module is the application's side of the palette: it knows that there
 * are five pillar screens, a settings route, four read-only report panels,
 * and one rule that decides what may never appear as a runnable command.
 *
 * THE RULE: NO STATE-CHANGING COMMAND IS EVER RUNNABLE HERE
 * -------------------------------------------------------
 * `docs/UX_ARCHITECTURE.md` says "Actions live in Cleanup only". The palette is
 * global, so it must never become a second place where a destructive action
 * can be started. Two mechanisms enforce that:
 *
 *  1. Every command id is namespaced: `nav.*` navigates, `panel.*` jumps to a
 *     read-only report, `action.*` is the only namespace that could ever carry
 *     a state change — and every `action.*` command is disabled.
 *  2. A disabled command is shown with its reason ("Actions live in Cleanup")
 *     rather than hidden, so the boundary stays visible instead of becoming a
 *     mystery.
 *
 * `commandTarget()` is the only way the shell turns a command into an effect,
 * and it returns `null` for anything that is not a navigation or a report jump.
 */

import type { CoreSightApi } from "../../api";
import { t, type MessageKey } from "../../i18n";
import { SCREEN_IDS, type ScreenId, type VisibleScreens } from "../../settings/schema";
import { ACTIONS_LIVE_IN_CLEANUP, PALETTE_COPY } from "./copy";
import type { Command, CommandSource } from "./types";

export type PaletteRoute = ScreenId | "settings";

export const SCREEN_LABELS: Record<ScreenId, MessageKey> = {
  home: "app.nav.home",
  storage: "app.nav.storage",
  cleanup: "app.nav.cleanup",
  history: "app.nav.history",
  drives: "app.nav.drives"
};

/** The route a `nav.*` command leads to. */
export const NAVIGATION_TARGETS: Readonly<Record<string, PaletteRoute>> = {
  "nav.home": "home",
  "nav.storage": "storage",
  "nav.cleanup": "cleanup",
  "nav.history": "history",
  "nav.drives": "drives",
  "nav.settings": "settings"
};

export const PANEL_TARGETS = ["startup", "dns", "blocklist", "privacy"] as const;
export type PanelTarget = (typeof PANEL_TARGETS)[number];

/** The screen that hosts each read-only report panel. */
export const PANEL_HOST_SCREEN: Record<PanelTarget, ScreenId> = {
  startup: "history",
  dns: "history",
  blocklist: "history",
  privacy: "home"
};

/** The heading id the shell focuses after jumping to a panel. */
export const PANEL_HEADING_ID: Record<PanelTarget, string> = {
  startup: "startup-panel-heading",
  dns: "dns-panel-heading",
  blocklist: "blocklist-panel-heading",
  privacy: "privacy-panel-heading"
};

const PANEL_TITLE_KEY: Record<PanelTarget, MessageKey> = {
  startup: "startup.title",
  dns: "dns.title",
  blocklist: "blocklist.title",
  privacy: "privacy.title"
};

/** The visible title of a read-only report panel, in the active locale. */
export function panelTitle(panel: PanelTarget): string {
  return t(PANEL_TITLE_KEY[panel]);
}

function isPanelTarget(value: string): value is PanelTarget {
  return (PANEL_TARGETS as readonly string[]).includes(value);
}

export type CommandTarget =
  | { kind: "navigate"; route: PaletteRoute }
  | { kind: "panel"; panel: PanelTarget };

/**
 * Resolves a command into the only two effects the shell will perform:
 * navigation and a jump to a read-only report. Anything else — including
 * every `action.*` command — resolves to `null` and does nothing.
 */
export function commandTarget(command: Command): CommandTarget | null {
  if (command.id.startsWith("nav.")) {
    const route = NAVIGATION_TARGETS[command.id];
    if (route === undefined) return null;
    return { kind: "navigate", route };
  }

  if (command.id.startsWith("panel.")) {
    const target = command.id.slice("panel.".length);
    if (!isPanelTarget(target)) return null;
    return { kind: "panel", panel: target };
  }

  return null;
}

export interface CommandContext {
  api: CoreSightApi;
  /** A hidden screen is not reachable through the palette either. */
  visibleScreens: VisibleScreens;
}

function countKeyword(value: number | null): string[] {
  return value === null ? [] : [String(value)];
}

/**
 * Builds the command sources the shell hands to the palette.
 *
 * The screens source answers immediately. The reports source is the honest
 * asynchronous one: it asks the engine for the four read-only reports, so the
 * palette can offer them (and search their counts) while the engine is still
 * reading. It never blocks the dialog.
 */
export function buildCommandSources(context: CommandContext): CommandSource[] {
  const screens: CommandSource = {
    id: "coresight.screens",
    label: PALETTE_COPY.screensGroup,
    load: async () => {
      const commands: Command[] = [];

      for (const id of SCREEN_IDS) {
        if (!context.visibleScreens[id]) continue;
        commands.push({
          id: `nav.${id}`,
          label: t(SCREEN_LABELS[id]),
          group: PALETTE_COPY.screensGroup,
          keywords: [id]
        });
      }

      commands.push({
        id: "nav.settings",
        label: t("app.nav.settings"),
        group: PALETTE_COPY.screensGroup,
        keywords: [t("settings.title")]
      });

      return commands;
    }
  };

  const reports: CommandSource = {
    id: "coresight.reports",
    label: PALETTE_COPY.reportsGroup,
    load: async () => {
      const [startup, dns, blocklist, privacy] = await Promise.allSettled([
        context.api.getStartupReport(),
        context.api.getDnsRanking(),
        context.api.getBlockListPlan(),
        context.api.getPrivacySummary()
      ]);

      const keywords: Record<PanelTarget, string[]> = {
        startup:
          startup.status === "fulfilled" ? countKeyword(startup.value.totalEnabled) : [],
        dns: dns.status === "fulfilled" ? [String(dns.value.ranked.length)] : [],
        blocklist:
          blocklist.status === "fulfilled" ? [String(blocklist.value.entries.length)] : [],
        privacy:
          privacy.status === "fulfilled" ? countKeyword(privacy.value.totalCount) : []
      };

      return PANEL_TARGETS.filter((target) =>
        context.visibleScreens[PANEL_HOST_SCREEN[target]]
      ).map((target) => ({
        id: `panel.${target}`,
        label: panelTitle(target),
        group: PALETTE_COPY.reportsGroup,
        keywords: keywords[target]
      }));
    }
  };

  const actions: CommandSource = {
    id: "coresight.actions",
    label: t("app.nav.cleanup"),
    load: async () => [
      {
        id: "action.cleanup.confirm",
        label: t("cleanup.confirm.button"),
        group: t("app.nav.cleanup"),
        disabled: true,
        disabledReason: ACTIONS_LIVE_IN_CLEANUP
      }
    ]
  };

  return [screens, reports, actions];
}
