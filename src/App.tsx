import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  CONTRACT_VERSION,
  isContractCompatible,
  type CoreSightApi,
  type MockScenario,
  type ScanStatus
} from "./api";
import { createApi, isTauriRuntime } from "./api/tauriApi";
import { useApiData } from "./hooks/useApiData";
import { formatCount, formatDateTime } from "./lib/format";
import { HomeScreen } from "./screens/HomeScreen";
import { StorageScreen } from "./screens/StorageScreen";
import { CleanupScreen } from "./screens/CleanupScreen";
import { HistoryScreen } from "./screens/HistoryScreen";
import { DrivesScreen } from "./screens/DrivesScreen";

type ScreenId = "home" | "storage" | "cleanup" | "history" | "drives";

const SCREENS: ReadonlyArray<{ id: ScreenId; label: string }> = [
  { id: "home", label: "Home" },
  { id: "storage", label: "Storage" },
  { id: "cleanup", label: "Cleanup" },
  { id: "history", label: "History" },
  { id: "drives", label: "Drives" }
];

function readScreenFromHash(): ScreenId {
  const value = window.location.hash.replace(/^#\/?/, "");
  const match = SCREENS.find((screen) => screen.id === value);
  return match?.id ?? "home";
}

function readMockScenario(): MockScenario {
  const params = new URLSearchParams(window.location.search);
  const value = params.get("state");
  if (value === "empty" || value === "partial" || value === "error" || value === "ready") {
    return value;
  }
  return "ready";
}

function scanSummary(scan: ScanStatus): string {
  if (scan.state === "idle") return "No completed scan yet";
  if (scan.state === "scanning") {
    return `Scanning · ${formatCount(scan.filesSeen, "entry", "entries")} seen${
      scan.currentArea ? ` · ${scan.currentArea}` : ""
    }`;
  }
  if (scan.state === "failed") return "Last scan failed · figures may be stale";
  if (scan.state === "partial") {
    return `Last scan completed with limits · ${formatCount(
      scan.coverage.unreadableEntries,
      "entry",
      "entries"
    )} could not be read`;
  }
  return `Scan complete · ${formatDateTime(scan.completedAt)}`;
}

function AppHeader({
  api,
  screen,
  liveEngine,
  onNavigate
}: {
  api: CoreSightApi;
  screen: ScreenId;
  liveEngine: boolean;
  onNavigate: () => void;
}) {
  const status = useApiData(() => api.getGlobalStatus(), [api]);

  let scanText = "Reading scan status…";
  let tone = "loading";
  if (status.status === "error") {
    scanText = "Scan status unavailable";
    tone = "attention";
  } else if (status.status === "ready") {
    scanText = scanSummary(status.data.scan);
    tone = status.data.scan.state;
  }

  return (
    <header className="app-header">
      <div className="app-header__topline">
        <a className="brand" href="#home" onClick={onNavigate} aria-label="CoreSight home">
          <span className="brand__mark" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
          <span className="brand__wordmark">CoreSight</span>
        </a>

        <div className={`scan-indicator scan-indicator--${tone}`}>
          <span className="scan-indicator__dot" aria-hidden="true" />
          <span className="scan-indicator__label" aria-live="polite" aria-atomic="true">
            {scanText}
          </span>
        </div>

        <span
          className="simulated-badge"
          title={
            liveEngine
              ? "Reading the local CoreSight engine through the coresight.v1 contract."
              : "Rendering deterministic local fixture data; no engine is attached."
          }
        >
          {liveEngine ? "Local engine" : "Local preview"}
        </span>
      </div>

      <nav className="primary-nav" aria-label="Primary">
        <ul>
          {SCREENS.map((item) => (
            <li key={item.id}>
              <a
                href={`#${item.id}`}
                aria-current={screen === item.id ? "page" : undefined}
                onClick={onNavigate}
              >
                {item.label}
              </a>
            </li>
          ))}
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
          <p className="eyebrow">Contract check</p>
          <h1 tabIndex={-1}>This UI cannot read the installed data contract</h1>
          <p className="page-header__intro">
            The locally returned contract is <code>{received}</code>, while this shell supports{" "}
            <code>{expected}</code>. CoreSight refuses to guess at incompatible fields.
          </p>
        </div>
      </header>
      <section className="state-panel state-panel--error" role="alert">
        <h2>No screens were populated</h2>
        <p>
          Update the app so the frontend and engine share a compatible contract version. No data
          was changed and no cleanup path was opened.
        </p>
      </section>
    </div>
  );
}

export default function App() {
  const scenario = useMemo(readMockScenario, []);
  const liveEngine = useMemo(isTauriRuntime, []);
  const api = useMemo(() => createApi({ mock: { scenario } }), [scenario]);
  const [screen, setScreen] = useState<ScreenId>(readScreenFromHash);
  const [focusCategoryId, setFocusCategoryId] = useState<string | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const mounted = useRef(false);

  useEffect(() => {
    const onHashChange = () => {
      setScreen(readScreenFromHash());
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
  }, [screen]);

  const globalStatus = useApiData(() => api.getGlobalStatus(), [api]);
  const contractCompatible =
    globalStatus.status === "ready"
      ? isContractCompatible(globalStatus.data.contractVersion)
      : true;

  function handleNavigate() {
    setFocusCategoryId(null);
  }

  function openCategoryInStorage(categoryId: string) {
    setFocusCategoryId(categoryId);
    window.location.hash = "storage";
  }

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
        Skip to content
      </a>
      <AppHeader
        api={api}
        screen={screen}
        liveEngine={liveEngine}
        onNavigate={handleNavigate}
      />
      <main id="content" ref={mainRef} tabIndex={-1}>
        {content}
      </main>
      <footer className="app-footer">
        <p>
          CoreSight analyses storage locally. No paths, names, or hashes leave this device.
        </p>
        <p>
          Phase 8 UI shell · contract {CONTRACT_VERSION} ·{" "}
          {liveEngine ? "live local engine" : "deterministic mock data"}
        </p>
      </footer>
    </>
  );
}
