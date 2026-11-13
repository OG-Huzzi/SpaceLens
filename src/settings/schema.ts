/**
 * CoreSight preferences — typed, versioned settings schema.
 *
 * PURE CLIENT STATE. This module never touches IPC, the filesystem, or the
 * network. Persistence is `localStorage` only, behind `./storage.ts`, under the
 * single key `coresight.settings.v1`.
 *
 * HIDING A SCREEN IS A VIEW PREFERENCE, NEVER A CAPABILITY CHANGE
 * ---------------------------------------------------------------
 * `visibleScreens` only decides which destinations the app shell offers. It
 * does not grant, revoke, or re-scope any engine capability: every
 * state-changing path still lives in the Cleanup screen behind a mandatory
 * preview and an explicit confirmation, and the engine's authorization gates
 * never read this object. Hiding Cleanup hides a navigation entry — it does not
 * "disable cleanup", and no preference here can enable anything.
 *
 * FALLBACK POLICY
 * ---------------
 * Stored data is untrusted input. Unparseable JSON, a non-object payload, an
 * unknown `schemaVersion`, or a screen visibility set with nothing visible all
 * fall back to `defaultSettings()` instead of throwing. Individual fields with
 * the wrong type fall back one field at a time so one bad value cannot erase
 * every other preference.
 */

export const SETTINGS_SCHEMA_VERSION = 1 as const;
export const SETTINGS_STORAGE_KEY = "coresight.settings.v1";

export const THEME_PREFERENCES = ["system", "light", "dark"] as const;
export type ThemePreference = (typeof THEME_PREFERENCES)[number];

export const DENSITY_PREFERENCES = ["comfortable", "compact"] as const;
export type DensityPreference = (typeof DENSITY_PREFERENCES)[number];

/**
 * "system" follows the operating system. "reduce" forces reduced motion and
 * "allow" overrides an operating-system reduce-motion setting — the OS default
 * is respected, not imposed.
 */
export const REDUCED_MOTION_PREFERENCES = ["system", "reduce", "allow"] as const;
export type ReducedMotionPreference = (typeof REDUCED_MOTION_PREFERENCES)[number];

/** The five pillar screens. Settings itself is a route, never one of them. */
export const SCREEN_IDS = ["home", "storage", "cleanup", "history", "drives"] as const;
export type ScreenId = (typeof SCREEN_IDS)[number];

export type VisibleScreens = Readonly<Record<ScreenId, boolean>>;

/**
 * "en" is the base locale and the only one that is guaranteed complete. "de" is
 * a complete second catalogue used to prove the machinery; see `src/i18n`.
 */
export const SUPPORTED_LOCALES = ["en", "de"] as const;
export type LocaleCode = (typeof SUPPORTED_LOCALES)[number];

export interface Settings {
  schemaVersion: typeof SETTINGS_SCHEMA_VERSION;
  theme: ThemePreference;
  reducedMotion: ReducedMotionPreference;
  density: DensityPreference;
  showAdvancedDetails: boolean;
  visibleScreens: VisibleScreens;
  locale: LocaleCode;
}

/** Every screen visible: hiding anything is an explicit user act. */
export const DEFAULT_VISIBLE_SCREENS: VisibleScreens = Object.freeze({
  home: true,
  storage: true,
  cleanup: true,
  history: true,
  drives: true
});

export function isLocaleCode(value: string): value is LocaleCode {
  return (SUPPORTED_LOCALES as readonly string[]).includes(value);
}

/**
 * Default locale is the browser locale when it matches a supported locale,
 * otherwise English. `navigator` is absent in non-DOM test/worker contexts, in
 * which case English is the honest fallback.
 */
export function detectBrowserLocale(): LocaleCode {
  if (typeof navigator === "undefined") return "en";
  const requested: readonly unknown[] = [navigator.language, ...(navigator.languages ?? [])];
  for (const tag of requested) {
    if (typeof tag !== "string") continue;
    const base = tag.toLowerCase().split("-")[0];
    if (isLocaleCode(base)) return base;
  }
  return "en";
}

export function defaultSettings(): Settings {
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    theme: "system",
    reducedMotion: "system",
    density: "comfortable",
    showAdvancedDetails: false,
    visibleScreens: { ...DEFAULT_VISIBLE_SCREENS },
    locale: detectBrowserLocale()
  };
}

type SettingsRecord = Record<string, unknown>;
type SettingsMigration = (previous: SettingsRecord) => SettingsRecord | null;

/**
 * Migration registry, keyed by the *stored* version that needs upgrading.
 *
 * There is no released predecessor to v1 in this build, so the registry is
 * empty on purpose: inventing a migration for data nobody ever wrote would be
 * fiction. When `SETTINGS_SCHEMA_VERSION` becomes 2, add
 * `1: (previous) => ({ ...previous, schemaVersion: 2, ... })` here, keep the
 * v1 fixture in the tests, and prove the upgrade path with real v1 data.
 */
const MIGRATIONS: Readonly<Partial<Record<number, SettingsMigration>>> = {};

function readVersion(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) return null;
  return value;
}

function isRecord(value: unknown): value is SettingsRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Walks the migration chain one version at a time. Returns `null` for any
 * version this build cannot reach — the caller then uses defaults. A migration
 * that does not advance the version is rejected so this can never loop.
 */
function upgradeToCurrent(raw: SettingsRecord): SettingsRecord | null {
  let current = raw;
  let version = readVersion(current.schemaVersion);
  while (version !== null && version < SETTINGS_SCHEMA_VERSION) {
    const migration = MIGRATIONS[version];
    if (migration === undefined) return null;
    const next = migration(current);
    if (next === null) return null;
    const nextVersion = readVersion(next.schemaVersion);
    if (nextVersion === null || nextVersion <= version) return null;
    current = next;
    version = nextVersion;
  }
  return version === SETTINGS_SCHEMA_VERSION ? current : null;
}

function readEnum<T extends string>(
  value: unknown,
  allowed: readonly T[],
  fallback: T
): T {
  // The `includes` check is the runtime proof; the cast only restates it.
  if (typeof value === "string" && (allowed as readonly string[]).includes(value)) {
    return value as T;
  }
  return fallback;
}

function readBoolean(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

export function hasVisibleScreen(screens: VisibleScreens): boolean {
  return SCREEN_IDS.some((id) => screens[id]);
}

function readVisibleScreens(value: unknown, fallback: VisibleScreens): VisibleScreens {
  if (!isRecord(value)) return fallback;
  const next: Record<ScreenId, boolean> = { ...fallback };
  for (const id of SCREEN_IDS) {
    const flag = value[id];
    if (typeof flag === "boolean") next[id] = flag;
  }
  // An empty navigation would leave the app with no destination at all, so a
  // "nothing visible" value is repaired back to the default rather than obeyed.
  return hasVisibleScreen(next) ? next : fallback;
}

/**
 * Applies one visibility change, refusing to hide the final visible screen.
 * Returning the previous object makes the refusal observable by identity, which
 * the Settings screen uses to keep the last checkbox checked.
 */
export function withVisibleScreen(
  screens: VisibleScreens,
  id: ScreenId,
  visible: boolean
): VisibleScreens {
  const next: Record<ScreenId, boolean> = { ...screens, [id]: visible };
  return hasVisibleScreen(next) ? next : screens;
}

/** Coerces any stored value into a complete, valid `Settings`. Never throws. */
export function normaliseSettings(raw: unknown): Settings {
  const fallback = defaultSettings();
  if (!isRecord(raw)) return fallback;
  const upgraded = upgradeToCurrent(raw);
  if (upgraded === null) return fallback;

  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    theme: readEnum(upgraded.theme, THEME_PREFERENCES, fallback.theme),
    reducedMotion: readEnum(
      upgraded.reducedMotion,
      REDUCED_MOTION_PREFERENCES,
      fallback.reducedMotion
    ),
    density: readEnum(upgraded.density, DENSITY_PREFERENCES, fallback.density),
    showAdvancedDetails: readBoolean(
      upgraded.showAdvancedDetails,
      fallback.showAdvancedDetails
    ),
    visibleScreens: readVisibleScreens(upgraded.visibleScreens, fallback.visibleScreens),
    locale: readEnum(upgraded.locale, SUPPORTED_LOCALES, fallback.locale)
  };
}

/** Reads a raw `localStorage` string. `null`/empty means "no stored settings". */
export function parseSettings(raw: string | null): Settings {
  if (raw === null || raw.trim() === "") return defaultSettings();
  try {
    return normaliseSettings(JSON.parse(raw));
  } catch {
    // Unparseable payload (truncated write, manual edit, other app): defaults.
    return defaultSettings();
  }
}

export function serialiseSettings(settings: Settings): string {
  return JSON.stringify(settings);
}
