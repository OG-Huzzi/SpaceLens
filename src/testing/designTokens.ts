/**
 * Design-token conformance scanner.
 *
 * `docs/DESIGN_SYSTEM.md` states the rule in one sentence:
 *
 *   "All colours are CSS custom properties on `:root` … Components must not
 *    reference raw hex values."
 *
 * This module is the mechanical form of that sentence: it reads the CSS the
 * application actually ships and reports every raw hex colour that is not the
 * token vocabulary itself.
 *
 * The token file (`src/design/tokens.css`) is the single allowed home for raw
 * hex values. Everything else must reference `var(--cs-*)`.
 */

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";

export interface HexOffender {
  /** Path relative to the scanned root, with `/` separators. */
  file: string;
  /** 1-based line number in the source file. */
  line: number;
  /** The raw colour, e.g. `#1d5c4f`. */
  value: string;
  /** The source line, trimmed, for context. */
  context: string;
}

/** Matches `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`. */
const HEX_PATTERN = /#[0-9a-fA-F]{3,8}\b/g;

/** Strip CSS comments so a colour mentioned in prose is not an offender. */
export function stripCssComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/[^\n]*/g, "$1");
}

/**
 * Every raw hex colour in one CSS source, with the line it appears on.
 */
export function findHexColours(source: string): Array<{
  value: string;
  line: number;
  context: string;
}> {
  const withoutComments = stripCssComments(source);
  const lines = withoutComments.split(/\r?\n/);
  const found: Array<{ value: string; line: number; context: string }> = [];
  lines.forEach((text, index) => {
    HEX_PATTERN.lastIndex = 0;
    let match = HEX_PATTERN.exec(text);
    while (match !== null) {
      found.push({
        value: match[0].toLowerCase(),
        line: index + 1,
        context: text.trim().slice(0, 160)
      });
      match = HEX_PATTERN.exec(text);
    }
  });
  return found;
}

/** Every `.css` file under `root`, recursively, as relative paths. */
export function listCssFiles(root: string): string[] {
  if (!existsSync(root)) return [];
  const files: string[] = [];
  const walk = (directory: string, prefix: string): void => {
    for (const entry of readdirSync(directory)) {
      const full = `${directory}/${entry}`;
      const relative = prefix === "" ? entry : `${prefix}/${entry}`;
      if (statSync(full).isDirectory()) {
        walk(full, relative);
      } else if (entry.endsWith(".css")) {
        files.push(relative);
      }
    }
  };
  walk(root, "");
  return files.sort();
}

/**
 * Scan a directory of CSS sources and return every raw hex colour found,
 * tagged with the file it came from.
 */
export function scanCssFiles(root: string): HexOffender[] {
  const offenders: HexOffender[] = [];
  for (const file of listCssFiles(root)) {
    const source = readFileSync(`${root}/${file}`, "utf8");
    for (const found of findHexColours(source)) {
      offenders.push({ file, ...found });
    }
  }
  return offenders;
}

/** Group offenders by file, preserving the file order of the scan. */
export function groupByFile(offenders: HexOffender[]): Array<{
  file: string;
  offenders: HexOffender[];
}> {
  const groups = new Map<string, HexOffender[]>();
  for (const offender of offenders) {
    const existing = groups.get(offender.file);
    if (existing) {
      existing.push(offender);
    } else {
      groups.set(offender.file, [offender]);
    }
  }
  return Array.from(groups.entries()).map(([file, entries]) => ({
    file,
    offenders: entries
  }));
}

/** A report that names every offender, one per line. */
export function formatOffenders(offenders: HexOffender[]): string {
  if (offenders.length === 0) return "no raw hex colours found";
  return offenders
    .map(
      (offender) =>
        `${offender.file}:${offender.line} ${offender.value}  →  ${offender.context}`
    )
    .join("\n");
}

/** The distinct colour values of one file, lower-cased and sorted. */
export function distinctValues(offenders: HexOffender[]): string[] {
  return Array.from(new Set(offenders.map((offender) => offender.value))).sort();
}
