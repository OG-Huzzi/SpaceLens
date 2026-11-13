/**
 * The single persistence boundary for preferences.
 *
 * Nothing outside this module reads or writes `localStorage`. Every call is
 * wrapped: a browser may expose `localStorage` and still throw on access
 * (private-mode quotas, disabled site data, embedded webviews). A failed read
 * degrades to defaults; a failed write leaves the in-memory preference applied
 * for the session and reports `false` so the UI can say so instead of lying.
 */

import {
  SETTINGS_STORAGE_KEY,
  defaultSettings,
  normaliseSettings,
  parseSettings,
  serialiseSettings,
  type Settings
} from "./schema";

/** The slice of the Storage API this module needs; trivially fakeable in tests. */
export interface SettingsStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/**
 * Returns the real `localStorage` only after a successful read probe, or `null`
 * when storage is missing or blocked. The probe is read-only: loading settings
 * must not write anything.
 */
export function browserSettingsStorage(): SettingsStorage | null {
  if (typeof window === "undefined") return null;
  try {
    const storage = window.localStorage;
    if (!storage) return null;
    storage.getItem(SETTINGS_STORAGE_KEY);
    return storage;
  } catch {
    return null;
  }
}

export function loadSettings(
  storage: SettingsStorage | null = browserSettingsStorage()
): Settings {
  if (storage === null) return defaultSettings();
  try {
    return parseSettings(storage.getItem(SETTINGS_STORAGE_KEY));
  } catch {
    return defaultSettings();
  }
}

/** Returns whether the value reached storage. Never throws. */
export function saveSettings(
  settings: Settings,
  storage: SettingsStorage | null = browserSettingsStorage()
): boolean {
  if (storage === null) return false;
  try {
    // Normalise on the way out as well: what is written is always a complete,
    // current-version envelope, never a partially patched object.
    storage.setItem(SETTINGS_STORAGE_KEY, serialiseSettings(normaliseSettings(settings)));
    return true;
  } catch {
    return false;
  }
}

export function clearStoredSettings(
  storage: SettingsStorage | null = browserSettingsStorage()
): boolean {
  if (storage === null) return false;
  try {
    storage.removeItem(SETTINGS_STORAGE_KEY);
    return true;
  } catch {
    return false;
  }
}
