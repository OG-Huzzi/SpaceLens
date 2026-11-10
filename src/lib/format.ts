/**
 * Display-only formatting. None of these helpers infer missing values.
 * Every nullish input renders an explicit "Unknown" phrase.
 */

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

const decimal = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 1,
  minimumFractionDigits: 0
});

const integer = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 0
});

const dateTime = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  year: "numeric",
  hour: "numeric",
  minute: "2-digit"
});

export function formatBytes(bytes: number | null): string {
  if (bytes === null) return "Unknown";
  if (bytes === 0) return "0 B";
  const sign = bytes < 0 ? "-" : "";
  let value = Math.abs(bytes);
  let index = 0;
  while (value >= 1024 && index < BYTE_UNITS.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${sign}${decimal.format(value)} ${BYTE_UNITS[index]}`;
}

export function formatCount(value: number | null, singular: string, plural: string): string {
  if (value === null) return `Unknown ${plural}`;
  return `${integer.format(value)} ${value === 1 ? singular : plural}`;
}

export function formatNumber(value: number | null): string {
  if (value === null) return "Unknown";
  return integer.format(value);
}

export function formatPercent(share: number | null): string {
  if (share === null) return "Unknown";
  return `${decimal.format(share * 100)}%`;
}

export function formatDelta(bytes: number | null): string {
  if (bytes === null) return "Unknown change";
  if (bytes === 0) return "No change";
  if (bytes > 0) return `+${formatBytes(bytes)}`;
  return `-${formatBytes(Math.abs(bytes))}`;
}

export function formatSignedCount(value: number | null): string {
  if (value === null) return "Unknown";
  if (value === 0) return "No change";
  return value > 0 ? `+${integer.format(value)}` : `-${integer.format(Math.abs(value))}`;
}

export function formatAge(days: number | null): string {
  if (days === null) return "Age unknown";
  if (days < 1) return "Less than a day";
  return `${integer.format(days)} ${days === 1 ? "day" : "days"}`;
}

export function formatDateTime(iso: string | null): string {
  if (iso === null) return "Unknown";
  const value = new Date(iso);
  if (Number.isNaN(value.getTime())) return "Unknown";
  return dateTime.format(value);
}

export function formatEta(seconds: number | null): string {
  if (seconds === null) return "Time remaining unknown";
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} sec remaining`;
  const minutes = Math.round(seconds / 60);
  return `${minutes} min remaining`;
}

export function formatShare(share: number | null): string {
  if (share === null) return "Share unknown";
  return `${formatPercent(share)} of parent`;
}

export function pluralise(value: number | null, singular: string, plural: string): string {
  if (value === null) return plural;
  return value === 1 ? singular : plural;
}

export function knownOrUnknown(value: number | null, formatter: (value: number) => string): string {
  return value === null ? "Unknown" : formatter(value);
}
