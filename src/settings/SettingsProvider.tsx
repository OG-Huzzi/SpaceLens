/**
 * React binding for the settings module.
 *
 * The provider owns exactly three side effects: persist on change, mirror the
 * preferences onto the document root, and publish the active locale to the
 * i18n runtime. Everything else stays in pure modules under `src/settings/`.
 */

import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useState,
  type ReactNode
} from "react";
import { setActiveLocale } from "../i18n";
import { applySettingsToDocument } from "./apply";
import {
  defaultSettings,
  normaliseSettings,
  withVisibleScreen,
  type ScreenId,
  type Settings
} from "./schema";
import { loadSettings, saveSettings } from "./storage";

/** Anything a screen may change. `schemaVersion` is owned by the schema. */
export type SettingsPatch = Partial<Omit<Settings, "schemaVersion">>;

export interface SettingsContextValue {
  settings: Settings;
  updateSettings: (patch: SettingsPatch) => void;
  setScreenVisible: (id: ScreenId, visible: boolean) => void;
  resetSettings: () => void;
  /** False when the browser refused to persist; changes then last one session. */
  canPersist: boolean;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [boot] = useState(() => {
    const loaded = loadSettings();
    return { settings: loaded, canPersist: saveSettings(loaded) };
  });
  const [settings, setSettings] = useState<Settings>(boot.settings);
  const [canPersist, setCanPersist] = useState<boolean>(boot.canPersist);

  // `t()` reads module state, so the active locale has to be current *before*
  // children render. This is an idempotent assignment derived from state, not a
  // subscription: this component re-rendering is what refreshes every string.
  setActiveLocale(settings.locale);

  useLayoutEffect(() => {
    applySettingsToDocument(settings);
  }, [settings]);

  const commit = useCallback((next: Settings) => {
    const normalised = normaliseSettings(next);
    setSettings(normalised);
    setCanPersist(saveSettings(normalised));
  }, []);

  const updateSettings = useCallback(
    (patch: SettingsPatch) => {
      commit({ ...settings, ...patch });
    },
    [commit, settings]
  );

  const setScreenVisible = useCallback(
    (id: ScreenId, visible: boolean) => {
      commit({
        ...settings,
        visibleScreens: withVisibleScreen(settings.visibleScreens, id, visible)
      });
    },
    [commit, settings]
  );

  const resetSettings = useCallback(() => {
    commit(defaultSettings());
  }, [commit]);

  const value: SettingsContextValue = {
    settings,
    updateSettings,
    setScreenVisible,
    resetSettings,
    canPersist
  };

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}

export function useSettings(): SettingsContextValue {
  const value = useContext(SettingsContext);
  if (value === null) {
    throw new Error("useSettings must be used inside a SettingsProvider");
  }
  return value;
}
