/**
 * The i18n runtime.
 *
 * KEY SAFETY IS A TYPE CONSTRAINT
 * -------------------------------
 * `t()` accepts only `MessageKey`, which is `keyof typeof en`. There is no
 * `string` overload and no `any`: a typo like `t("nav.hom")` fails `tsc`.
 *
 * RUNTIME MISSES ARE LOUD
 * -----------------------
 * Typed keys cannot miss at compile time, but data can reach a key through a
 * cast, an older bundle, or a future locale file. When that happens `t()`
 * returns the key itself and logs a development warning — it never returns an
 * empty string, and it never pretends the key existed.
 *
 * ACTIVATION
 * ----------
 * `t()` reads a module-level active locale. `SettingsProvider` sets it before
 * rendering its children and re-renders the whole shell when the locale
 * changes, so every `t()` call is re-evaluated in the same commit. Components
 * that call `t()` must therefore not be memoised against locale changes.
 */

import { MESSAGES, type MessageKey } from "./catalogue";

export type { MessageKey } from "./catalogue";
export type LocaleCode = keyof typeof MESSAGES;
export type MessageParams = Readonly<Record<string, string | number>>;

let activeLocale: LocaleCode = "en";

export function getActiveLocale(): LocaleCode {
  return activeLocale;
}

export function setActiveLocale(locale: LocaleCode): void {
  activeLocale = locale;
}

const warnedKeys = new Set<string>();

function warnOnce(id: string, message: string): void {
  if (warnedKeys.has(id)) return;
  warnedKeys.add(id);
  if (import.meta.env.DEV && typeof console !== "undefined") {
    console.warn(`[i18n] ${message}`);
  }
}

function interpolate(template: string, params: MessageParams, key: MessageKey): string {
  return template.replace(/\{(\w+)\}/g, (_match: string, name: string) => {
    const value = params[name];
    if (value === undefined) {
      // Leave the placeholder visible: a silent empty string would hide the bug.
      warnOnce(`param:${key}:${name}`, `missing param "${name}" for key "${key}"`);
      return `{${name}}`;
    }
    return String(value);
  });
}

/**
 * Resolves a message in the active locale.
 *
 * @param key a literal key from the catalogue; unknown keys do not typecheck
 * @param params values for `{placeholder}` tokens in the message
 */
export function t(key: MessageKey, params?: MessageParams): string {
  const catalogue: Record<string, string> = MESSAGES[activeLocale];
  const template: string | undefined = catalogue[key];

  if (template === undefined) {
    warnOnce(
      `key:${activeLocale}:${key}`,
      `missing message "${key}" in locale "${activeLocale}" — rendering the key`
    );
    return key;
  }

  return params === undefined ? template : interpolate(template, params, key);
}

const numberFormatters = new Map<LocaleCode, Intl.NumberFormat>();

function numberFormatter(locale: LocaleCode): Intl.NumberFormat {
  const existing = numberFormatters.get(locale);
  if (existing !== undefined) return existing;
  const created = new Intl.NumberFormat(locale);
  numberFormatters.set(locale, created);
  return created;
}

/**
 * Plural-aware count message: `tCount(n, "unit.entry.one", "unit.entry.other")`
 * formats `n` for the active locale and interpolates it as `{count}`.
 * The two keys are passed explicitly so the singular/plural pair stays visible
 * at the call site instead of hiding behind an English plural rule.
 */
export function tCount(count: number, one: MessageKey, other: MessageKey): string {
  return t(count === 1 ? one : other, { count: numberFormatter(activeLocale).format(count) });
}
