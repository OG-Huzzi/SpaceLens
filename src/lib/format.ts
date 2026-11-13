/**
 * Display-only formatting. None of these helpers infer missing values.
 * Every nullish input renders an explicit unknown phrase.
 *
 * LOCALE WIRING
 * -------------
 * Number, byte, percent, and date formatters are built for the *active* locale
 * from the i18n runtime (`getActiveLocale()`), not for a hard-coded `en-US`.
 * They are cached per locale, so switching the locale in Settings picks up the
 * German decimal comma and date order without recreating a formatter on every
 * render. The unknown sentinels are catalogue messages, so `formatBytes(null)`
 * can never degrade into a numeric string: it is a word, and it is the word of
 * the active locale.
 */

import { getActiveLocale, t, tCount, type LocaleCode } from "../i18n";

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

interface Formatters {
  decimal: Intl.NumberFormat;
  integer: Intl.NumberFormat;
  percent: Intl.NumberFormat;
  dateTime: Intl.DateTimeFormat;
}

const formatterCache = new Map<LocaleCode, Formatters>();

function formatters(): Formatters {
  const locale = getActiveLocale();
  const existing = formatterCache.get(locale);
  if (existing !== undefined) return existing;

  const created: Formatters = {
    decimal: new Intl.NumberFormat(locale, {
      maximumFractionDigits: 1,
      minimumFractionDigits: 0
    }),
    integer: new Intl.NumberFormat(locale, {
      maximumFractionDigits: 0
    }),
    percent: new Intl.NumberFormat(locale, {
      style: "percent",
      maximumFractionDigits: 1
    }),
    dateTime: new Intl.DateTimeFormat(locale, {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "numeric",
      minute: "2-digit"
    })
  };

  formatterCache.set(locale, created);
  return created;
}

export function formatBytes(bytes: number | null): string {
  if (bytes === null) return t("state.unknown");
  if (bytes === 0) return "0 B";
  const sign = bytes < 0 ? "-" : "";
  let value = Math.abs(bytes);
  let index = 0;
  while (value >= 1024 && index < BYTE_UNITS.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${sign}${formatters().decimal.format(value)} ${BYTE_UNITS[index]}`;
}

/**
 * Low-level counted phrase for callers that already own their unit words.
 * Screens should prefer `tCount()` with catalogue plural pairs so the unit
 * itself is translated; this helper stays for API compatibility.
 */
export function formatCount(value: number | null, singular: string, plural: string): string {
  if (value === null) return `${t("state.unknown")} ${plural}`;
  return `${formatters().integer.format(value)} ${value === 1 ? singular : plural}`;
}

export function formatNumber(value: number | null): string {
  if (value === null) return t("state.unknown");
  return formatters().integer.format(value);
}

export function formatPercent(share: number | null): string {
  if (share === null) return t("state.unknown");
  return formatters().percent.format(share);
}

export function formatDelta(bytes: number | null): string {
  if (bytes === null) return t("format.unknownChange");
  if (bytes === 0) return t("format.noChange");
  if (bytes > 0) return `+${formatBytes(bytes)}`;
  return `-${formatBytes(Math.abs(bytes))}`;
}

export function formatSignedCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  if (value === 0) return t("format.noChange");
  return value > 0
    ? `+${formatters().integer.format(value)}`
    : `-${formatters().integer.format(Math.abs(value))}`;
}

export function formatAge(days: number | null): string {
  if (days === null) return t("format.ageUnknown");
  if (days < 1) return t("format.lessThanADay");
  return tCount(days, "unit.day.one", "unit.day.other");
}

export function formatDateTime(iso: string | null): string {
  if (iso === null) return t("state.unknown");
  const value = new Date(iso);
  if (Number.isNaN(value.getTime())) return t("state.unknown");
  return formatters().dateTime.format(value);
}

export function formatEta(seconds: number | null): string {
  if (seconds === null) return t("format.etaUnknown");
  if (seconds < 60) {
    return t("format.etaSeconds", {
      count: formatters().integer.format(Math.max(1, Math.round(seconds)))
    });
  }
  return t("format.etaMinutes", {
    count: formatters().integer.format(Math.round(seconds / 60))
  });
}

export function formatShare(share: number | null): string {
  if (share === null) return t("format.shareUnknown");
  return t("format.shareOfParent", { percent: formatPercent(share) });
}

export function pluralise(value: number | null, singular: string, plural: string): string {
  if (value === null) return plural;
  return value === 1 ? singular : plural;
}

export function knownOrUnknown(value: number | null, formatter: (value: number) => string): string {
  return value === null ? t("state.unknown") : formatter(value);
}
