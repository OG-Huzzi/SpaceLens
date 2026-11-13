import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  CONTRACT_VERSION,
  isContractCompatible,
  type CoreSightApi,
  type MockScenario
} from "./api";
import { createApi, isTauriRuntime } from "./api/tauriApi";
import { useApiData } from "./hooks/useApiData";
import { t, type MessageKey } from "./i18n";
import { SCREEN_IDS, type ScreenId } from "./settings/schema";
import { SettingsProvider, useSettings } from "./settings/SettingsProvider";
import { ScanStatusStrip } from "./components/ScanStatusStrip";
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

const SCREEN_LABELS: Record<ScreenId, MessageKey> = {
  home: "app.nav.home",
  storage: "app.nav.storage",
  cleanup: "app.nav.cleanup",
  history: "app.nav.history",
  drives: "app.nav.drives"
};

function isScreenId(value: string): value is ScreenId {
  return (SCREEN_IDS as readonly string[]).includes(value);
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
  onNavigate
}: {
  api: CoreSightApi;
  screen: RouteId;
  liveEngine: boolean;
  onNavigate: (next: RouteId) => void;
}) {
  const { settings } = useSettings();
  const visibleScreens = SCREEN_IDS.filter((id) => settings.visibleScreens[id]);
  const homeTarget: ScreenId = visibleScreens[0] ?? "home";

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
  const mainRef = useRef<HTMLElement>(null);
  const mounted = useRef(false);

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
      <AppHeader api={api} screen={screen} liveEngine={liveEngine} onNavigate={handleNavigate} />
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
      </footer>
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
