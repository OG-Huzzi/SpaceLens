import { describe, expect, it } from "vitest";
import {
  DEFAULT_VISIBLE_SCREENS,
  SETTINGS_SCHEMA_VERSION,
  SETTINGS_STORAGE_KEY,
  defaultSettings,
  hasVisibleScreen,
  normaliseSettings,
  parseSettings,
  withVisibleScreen,
  type Settings
} from "./schema";
import {
  clearStoredSettings,
  loadSettings,
  saveSettings,
  type SettingsStorage
} from "./storage";

/** In-memory stand-in for `localStorage`; keeps the tests off real browser state. */
class MemoryStorage implements SettingsStorage {
  private readonly entries = new Map<string, string>();

  public getItem(key: string): string | null {
    return this.entries.get(key) ?? null;
  }

  public setItem(key: string, value: string): void {
    this.entries.set(key, value);
  }

  public removeItem(key: string): void {
    this.entries.delete(key);
  }

  public raw(key: string): string | null {
    return this.entries.get(key) ?? null;
  }
}

/** A browser that exposes storage but refuses every operation. */
class BrokenStorage implements SettingsStorage {
  public getItem(): string | null {
    throw new Error("storage blocked");
  }

  public setItem(): void {
    throw new Error("storage blocked");
  }

  public removeItem(): void {
    throw new Error("storage blocked");
  }
}

function customSettings(): Settings {
  return {
    ...defaultSettings(),
    theme: "dark",
    reducedMotion: "reduce",
    density: "compact",
    showAdvancedDetails: true,
    visibleScreens: {
      home: true,
      storage: false,
      cleanup: true,
      history: false,
      drives: true
    },
    locale: "de"
  };
}

describe("settings round-trip", () => {
  it("persists and restores every preference through the versioned key", () => {
    const storage = new MemoryStorage();
    const settings = customSettings();

    expect(saveSettings(settings, storage)).toBe(true);

    const raw = storage.raw(SETTINGS_STORAGE_KEY);
    expect(raw).not.toBeNull();
    const envelope = JSON.parse(raw ?? "null") as { schemaVersion?: unknown };
    expect(envelope.schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);

    expect(loadSettings(storage)).toEqual(settings);
  });

  it("writes a complete current-version envelope even from a partial object", () => {
    const storage = new MemoryStorage();
    const partial: Settings = { ...customSettings() };

    saveSettings(partial, storage);

    const stored = loadSettings(storage);
    expect(stored.schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
    expect(stored.theme).toBe("dark");
    expect(stored.locale).toBe("de");
  });

  it("clears the stored value", () => {
    const storage = new MemoryStorage();
    saveSettings(customSettings(), storage);

    expect(clearStoredSettings(storage)).toBe(true);
    expect(storage.raw(SETTINGS_STORAGE_KEY)).toBeNull();
    expect(loadSettings(storage)).toEqual(defaultSettings());
  });
});

describe("corrupt or unknown stored data falls back to defaults", () => {
  it.each([
    ["truncated JSON", "{not json"],
    ["an array instead of an object", "[1,2,3]"],
    ["a JSON string", "\"coresight\""],
    ["JSON null", "null"],
    ["an empty string", "   "]
  ])("falls back for %s", (_label, payload) => {
    const storage = new MemoryStorage();
    storage.setItem(SETTINGS_STORAGE_KEY, payload);

    expect(() => loadSettings(storage)).not.toThrow();
    expect(loadSettings(storage)).toEqual(defaultSettings());
  });

  it("falls back for an unknown schema version instead of guessing", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({ ...customSettings(), schemaVersion: SETTINGS_SCHEMA_VERSION + 98 })
    );

    expect(loadSettings(storage)).toEqual(defaultSettings());
  });

  it("falls back for an older, unmigratable version", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({ ...customSettings(), schemaVersion: 0 })
    );

    expect(loadSettings(storage)).toEqual(defaultSettings());
  });

  it("falls back for a payload with no version at all", () => {
    expect(parseSettings(JSON.stringify({ theme: "dark" }))).toEqual(defaultSettings());
  });

  it("keeps valid fields when a single field is invalid", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        theme: "neon",
        density: "compact",
        showAdvancedDetails: "yes",
        visibleScreens: { ...DEFAULT_VISIBLE_SCREENS, history: false }
      })
    );

    const loaded = loadSettings(storage);
    expect(loaded.theme).toBe(defaultSettings().theme);
    expect(loaded.showAdvancedDetails).toBe(defaultSettings().showAdvancedDetails);
    expect(loaded.density).toBe("compact");
    expect(loaded.visibleScreens.history).toBe(false);
  });

  it("repairs a stored visibility set with nothing visible", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        visibleScreens: { home: false, storage: false, cleanup: false, history: false, drives: false }
      })
    );

    const loaded = loadSettings(storage);
    expect(hasVisibleScreen(loaded.visibleScreens)).toBe(true);
    expect(loaded.visibleScreens).toEqual(DEFAULT_VISIBLE_SCREENS);
  });
});

describe("storage failures never crash the app", () => {
  it("degrades to defaults when the browser blocks reads", () => {
    expect(loadSettings(new BrokenStorage())).toEqual(defaultSettings());
  });

  it("reports a failed write instead of throwing", () => {
    expect(saveSettings(customSettings(), new BrokenStorage())).toBe(false);
    expect(clearStoredSettings(new BrokenStorage())).toBe(false);
  });

  it("works without any storage at all", () => {
    expect(loadSettings(null)).toEqual(defaultSettings());
    expect(saveSettings(customSettings(), null)).toBe(false);
  });
});

describe("screen visibility is a view preference", () => {
  it("refuses to hide the final visible screen", () => {
    const onlyDrives: Settings["visibleScreens"] = {
      home: false,
      storage: false,
      cleanup: false,
      history: false,
      drives: true
    };

    const next = withVisibleScreen(onlyDrives, "drives", false);
    expect(next).toBe(onlyDrives);
    expect(hasVisibleScreen(next)).toBe(true);
  });

  it("hides one screen without touching any other preference", () => {
    const start = customSettings();
    const next = normaliseSettings({
      ...start,
      visibleScreens: withVisibleScreen(start.visibleScreens, "drives", false)
    });

    expect(next.visibleScreens.drives).toBe(false);
    expect(next.visibleScreens.home).toBe(true);
    expect(next.theme).toBe(start.theme);
    expect(next.locale).toBe(start.locale);
  });
});
