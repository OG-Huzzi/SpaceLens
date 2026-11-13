import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject
} from "react";
import {
  CONTRACT_VERSION,
  isContractCompatible,
  type CoreSightApi,
  type MockScenario
} from "./api";
import { createApi, isTauriRuntime } from "./api/tauriApi";
import { useApiData } from "./hooks/useApiData";
import { t } from "./i18n";
import {
  SCREEN_IDS,
  SUPPORTED_LOCALES,
  THEME_PREFERENCES,
  isLocaleCode,
  type ScreenId,
  type ThemePreference
} from "./settings/schema";
import { SettingsProvider, useSettings } from "./settings/SettingsProvider";
import { ScanStatusStrip } from "./components/ScanStatusStrip";
import {
  CommandPalette,
  PALETTE_COPY,
  PALETTE_DIALOG_ID,
  PANEL_HEADING_ID,
  PANEL_HOST_SCREEN,
  SCREEN_LABELS,
  ShortcutHelpDialog,
  buildCommandSources,
  commandTarget,
  modifierKeyLabel,
  panelTitle,
  type Command,
  type PanelTarget
} from "./components/command";
import { KeyHint } from "./design";
import { HomeScreen } from "./screens/HomeScreen";
import { StorageScreen } from "./screens/StorageScreen";
import { CleanupScreen } from "./screens/CleanupScreen";
import { HistoryScreen } from "./screens/HistoryScreen";
import { DrivesScreen } from "./screens/DrivesScreen";
import { SettingsScreen } from "./screens/SettingsScreen";

/**
 * Settings is a route, not a sixth pillar: the five pillar screens keep their
 * one-question-per-screen contract, and Settings lives beside them in the shell
 * as a drawer destination.
 */
type RouteId = ScreenId | "settings";

function isScreenId(value: string): value is ScreenId {
  return (SCREEN_IDS as readonly string[]).includes(value);
}

function isThemePreference(value: string): value is ThemePreference {
  return (THEME_PREFERENCES as readonly string[]).includes(value);
}

function readRouteFromHash(): RouteId {
  const value = window.location.hash.replace(/^#\/?/, "");
  if (value === "settings") return "settings";
  if (isScreenId(value)) return value;
  return "home";
}

function readMockScenario(): MockScenario {
  const params = new URLSearchParams(window.location.search);
  const value = params.get("state");
  if (value === "empty" || value === "partial" || value === "error" || value === "ready") {
    return value;
  }
  return "ready";
}

function AppHeader({
  api,
  screen,
  liveEngine,
  onNavigate,
  onOpenPalette,
  paletteOpen,
  commandTriggerRef
}: {
  api: CoreSightApi;
  screen: RouteId;
  liveEngine: boolean;
  onNavigate: (next: RouteId) => void;
  onOpenPalette: () => void;
  paletteOpen: boolean;
  commandTriggerRef: RefObject<HTMLButtonElement>;
}) {
  const { settings, updateSettings } = useSettings();
  const visibleScreens = SCREEN_IDS.filter((id) => settings.visibleScreens[id]);
  const homeTarget: ScreenId = visibleScreens[0] ?? "home";

  function handleThemeChange(value: string) {
    if (!isThemePreference(value)) return;
    updateSettings({ theme: value });
  }

  function handleLocaleChange(value: string) {
    if (!isLocaleCode(value)) return;
    updateSettings({ locale: value });
  }

  return (
    <header className="app-header">
      <div className="app-header__topline">
        <a
          className="brand"
          href={`#${homeTarget}`}
          onClick={() => onNavigate(homeTarget)}
          aria-label={t("app.brand.home")}
        >
          <span className="brand__mark" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
          <span className="brand__wordmark">CoreSight</span>
        </a>

        <span
          className="simulated-badge"
          title={
            liveEngine
              ? t("app.header.engineLiveTitle")
              : t("app.header.enginePreviewTitle")
          }
        >
          {liveEngine ? t("app.header.engineLive") : t("app.header.enginePreview")}
        </span>

        <div className="cs-cmd-header-controls">
          <button
            type="button"
            className="cs-cmd-trigger"
            ref={commandTriggerRef}
            onClick={onOpenPalette}
            aria-haspopup="dialog"
            aria-expanded={paletteOpen}
            aria-controls={paletteOpen ? PALETTE_DIALOG_ID : undefined}
          >
            <span>{PALETTE_COPY.searchLabel}</span>
            <KeyHint keys={[modifierKeyLabel(), "K"]} />
          </button>

          <label className="cs-cmd-select">
            <span className="cs-cmd-visually-hidden">{t("settings.theme.legend")}</span>
            <select
              value={settings.theme}
              onChange={(event) => handleThemeChange(event.target.value)}
              aria-label={t("settings.theme.legend")}
            >
              <option value="system">{t("settings.theme.system")}</option>
              <option value="light">{t("settings.theme.light")}</option>
              <option value="dark">{t("settings.theme.dark")}</option>
            </select>
          </label>

          <label className="cs-cmd-select">
            <span className="cs-cmd-visually-hidden">{t("settings.language.label")}</span>
            <select
              value={settings.locale}
              onChange={(event) => handleLocaleChange(event.target.value)}
              aria-label={t("settings.language.label")}
            >
              {SUPPORTED_LOCALES.map((locale) => (
                <option key={locale} value={locale}>
                  {t(locale === "en" ? "settings.language.en" : "settings.language.de")}
                </option>
              ))}
            </select>
          </label>
        </div>
      </div>

      <ScanStatusStrip api={api} showAdvancedDetails={settings.showAdvancedDetails} />

      <nav className="primary-nav" aria-label={t("app.nav.label")}>
        <ul>
          {visibleScreens.map((item) => (
            <li key={item}>
              <a
                href={`#${item}`}
                aria-current={screen === item ? "page" : undefined}
                onClick={() => onNavigate(item)}
              >
                {t(SCREEN_LABELS[item])}
              </a>
            </li>
          ))}
          <li className="primary-nav__utility">
            <a
              href="#settings"
              aria-current={screen === "settings" ? "page" : undefined}
              onClick={() => onNavigate("settings")}
            >
              {t("app.nav.settings")}
            </a>
          </li>
        </ul>
      </nav>
    </header>
  );
}

function ContractMismatch({ received, expected }: { received: string; expected: string }) {
  return (
    <div className="screen">
      <header className="page-header">
        <div className="page-header__copy">
          <p className="eyebrow">{t("contract.eyebrow")}</p>
          <h1 tabIndex={-1}>{t("contract.title")}</h1>
          <p className="page-header__intro">
            {t("contract.bodyBefore")}
            <code>{received}</code>
            {t("contract.bodyBetween")}
            <code>{expected}</code>
            {t("contract.bodyAfter")}
          </p>
        </div>
      </header>
      <section className="state-panel state-panel--error" role="alert">
        <h2>{t("contract.panelTitle")}</h2>
        <p>{t("contract.panelBody")}</p>
      </section>
    </div>
  );
}

function AppShell() {
  const { settings } = useSettings();
  const scenario = useMemo(readMockScenario, []);
  const liveEngine = useMemo(isTauriRuntime, []);
  const api = useMemo(() => createApi({ mock: { scenario } }), [scenario]);
  const [route, setRoute] = useState<RouteId>(readRouteFromHash);
  const [focusCategoryId, setFocusCategoryId] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [pendingPanel, setPendingPanel] = useState<PanelTarget | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const mounted = useRef(false);
  const commandTriggerRef = useRef<HTMLButtonElement>(null);
  const paletteOpenRef = useRef(paletteOpen);
  paletteOpenRef.current = paletteOpen;

  const visibleScreens = settings.visibleScreens;
  const sources = useMemo(
    () => buildCommandSources({ api, visibleScreens }),
    [api, visibleScreens]
  );

  useEffect(() => {
    const onHashChange = () => {
      setRoute(readRouteFromHash());
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    window.scrollTo({ top: 0, behavior: "auto" });
    const heading = mainRef.current?.querySelector("h1");
    if (heading instanceof HTMLElement) heading.focus({ preventScroll: true });
  }, [route]);

  const globalStatus = useApiData(() => api.getGlobalStatus(), [api]);
  const contractCompatible =
    globalStatus.status === "ready"
      ? isContractCompatible(globalStatus.data.contractVersion)
      : true;

  function handleNavigate(next: RouteId) {
    setFocusCategoryId(null);
    // Update immediately so navigation never waits on the browser's
    // `hashchange` event; the hash still changes for bookmarking and history.
    setRoute(next);
  }

  function openCategoryInStorage(categoryId: string) {
    setFocusCategoryId(categoryId);
    window.location.hash = "storage";
  }

  /**
   * Opening focuses the trigger first, so the palette can hand the focus back
   * to it when it closes — the keyboard path and the mouse path end in the
   * same place.
   */
  function handlePaletteOpenChange(next: boolean) {
    if (next) commandTriggerRef.current?.focus();
    setPaletteOpen(next);
  }

  /**
   * The only two effects a command may have: navigate, or jump to a read-only
   * report. `commandTarget()` returns null for anything else, so a command
   * that would change state can never reach the engine from here.
   */
  function handleRunCommand(command: Command) {
    if (command.disabled === true) return;
    const target = commandTarget(command);
    if (target === null) return;

    if (target.kind === "navigate") {
      handleNavigate(target.route);
      return;
    }

    setPendingPanel(target.panel);
    handleNavigate(PANEL_HOST_SCREEN[target.panel]);
  }

  // Focus the report heading after the jump. The panel may still be loading
  // when the route changes, so the lookup is retried for a short while. The
  // heading is found by its id first and by its title second, so a panel that
  // moves in the markup still receives the focus.
  useEffect(() => {
    const panel = pendingPanel;
    if (panel === null) return;

    const headingId = PANEL_HEADING_ID[panel];
    const title = panelTitle(panel);
    let cancelled = false;
    let attempts = 0;

    const findHeading = (): HTMLElement | null => {
      const byId = document.getElementById(headingId);
      if (byId instanceof HTMLElement) return byId;

      const headings = document.querySelectorAll<HTMLElement>("h2, h3");
      for (const heading of headings) {
        if ((heading.textContent ?? "").trim() === title) return heading;
      }
      return null;
    };

    const tryFocus = () => {
      if (cancelled) return;
      attempts += 1;
      const heading = findHeading();
      if (heading !== null) {
        heading.setAttribute("tabindex", "-1");
        heading.focus({ preventScroll: true });
        if (typeof heading.scrollIntoView === "function") {
          heading.scrollIntoView({ block: "start", behavior: "auto" });
        }
        setPendingPanel(null);
        return;
      }
      if (attempts >= 20) {
        setPendingPanel(null);
        return;
      }
      window.setTimeout(tryFocus, 100);
    };

    tryFocus();
    return () => {
      cancelled = true;
    };
  }, [route, pendingPanel]);

  // "?" opens the shortcut list, unless the user is typing somewhere.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "?") return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (paletteOpenRef.current) return;
      const target = event.target;
      if (target instanceof HTMLElement) {
        if (target.isContentEditable) return;
        const tag = target.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      }
      event.preventDefault();
      setShortcutsOpen(true);
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // A screen switched off in Settings is not reachable by route either: the
  // preference hides the destination, nothing about its capability changes.
  const firstVisible: ScreenId = SCREEN_IDS.find((id) => settings.visibleScreens[id]) ?? "home";
  const screen: RouteId =
    route === "settings"
      ? "settings"
      : settings.visibleScreens[route]
        ? route
        : firstVisible;

  let content: ReactNode;
  if (!contractCompatible && globalStatus.status === "ready") {
    content = (
      <ContractMismatch
        received={globalStatus.data.contractVersion}
        expected={CONTRACT_VERSION}
      />
    );
  } else {
    switch (screen) {
      case "settings":
        content = <SettingsScreen />;
        break;
      case "storage":
        content = <StorageScreen api={api} focusCategoryId={focusCategoryId} />;
        break;
      case "cleanup":
        content = <CleanupScreen api={api} />;
        break;
      case "history":
        content = <HistoryScreen api={api} onOpenCategory={openCategoryInStorage} />;
        break;
      case "drives":
        content = <DrivesScreen api={api} />;
        break;
      case "home":
      default:
        content = <HomeScreen api={api} />;
    }
  }

  return (
    <>
      <a className="skip-link" href="#content">
        {t("app.skipToContent")}
      </a>
      <AppHeader
        api={api}
        screen={screen}
        liveEngine={liveEngine}
        onNavigate={handleNavigate}
        onOpenPalette={() => handlePaletteOpenChange(true)}
        paletteOpen={paletteOpen}
        commandTriggerRef={commandTriggerRef}
      />
      <main id="content" ref={mainRef} tabIndex={-1}>
        {content}
      </main>
      <footer className="app-footer">
        <p>{t("app.footer.privacy")}</p>
        <p>
          {t("app.footer.build", {
            contract: CONTRACT_VERSION,
            mode: liveEngine ? t("app.footer.modeLive") : t("app.footer.modePreview")
          })}
        </p>
        <p>
          <button
            type="button"
            className="cs-cmd-footer-button"
            onClick={() => setShortcutsOpen(true)}
          >
            {PALETTE_COPY.shortcutsTitle}
          </button>
        </p>
      </footer>

      <CommandPalette
        open={paletteOpen}
        onOpenChange={handlePaletteOpenChange}
        sources={sources}
        onRun={handleRunCommand}
      />

      <ShortcutHelpDialog open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
    </>
  );
}

export default function App() {
  return (
    <SettingsProvider>
      <AppShell />
    </SettingsProvider>
  );
}
