/**
 * Design-token conformance.
 *
 * `docs/DESIGN_SYSTEM.md`:
 *
 *   "All colours are CSS custom properties on `:root` … Components must not
 *    reference raw hex values."
 *   "Any colour that is not from this token set." — anti-pattern.
 *
 * This test is that sentence, executed. It reads every stylesheet the
 * application ships and fails, listing the offenders, when a raw hex colour
 * appears anywhere outside `src/design/tokens.css` — the single file that is
 * allowed to *define* the vocabulary.
 *
 * The second check is the mirror image: a custom property that is used but
 * never defined is a silent fallback to nothing, which is how a colour
 * disappears from a screen.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  distinctValues,
  findHexColours,
  formatOffenders,
  groupByFile,
  listCssFiles,
  scanCssFiles
} from "./designTokens";

/**
 * The `src` directory, resolved from this module's own location.
 *
 * `import.meta.url` is a `file:` URL under Vitest, but the jsdom
 * environment supplies its own `URL` implementation, so the path is derived
 * with string handling instead of `new URL(...)`.
 */
const SRC_ROOT = fileURLToPath(import.meta.url)
  .replace(/[\\/]testing[\\/][^\\/]+$/, "")
  .replace(/\\/g, "/");

/** The one file allowed to hold raw colour values. */
const TOKEN_FILE = "design/tokens.css";

/** Every custom property the token vocabulary defines. */
function definedTokens(source: string): Set<string> {
  const names = new Set<string>();
  const pattern = /(--cs-[a-z0-9-]+)\s*:/g;
  let match = pattern.exec(source);
  while (match !== null) {
    names.add(match[1]);
    match = pattern.exec(source);
  }
  return names;
}

/** Every custom property a stylesheet refers to. */
function referencedTokens(source: string): Set<string> {
  const names = new Set<string>();
  const pattern = /var\(\s*(--cs-[a-z0-9-]+)/g;
  let match = pattern.exec(source);
  while (match !== null) {
    names.add(match[1]);
    match = pattern.exec(source);
  }
  return names;
}

describe("the scanner itself", () => {
  it("finds raw hex colours and ignores the ones inside comments", () => {
    const source = `
      /* the old palette used #ff0000 here */
      .a { color: #1d5c4f; background: #abc; }
    `;
    const found = findHexColours(source);
    expect(found.map((entry) => entry.value)).toEqual(["#1d5c4f", "#abc"]);
    expect(found[0].line).toBe(3);
  });

  it("lists every stylesheet under src", () => {
    const files = listCssFiles(SRC_ROOT);
    expect(files).toContain("styles.css");
    expect(files).toContain(TOKEN_FILE);
  });
});

describe("design token conformance", () => {
  it("keeps every raw hex colour inside the token vocabulary", () => {
    const offenders = scanCssFiles(SRC_ROOT).filter(
      (offender) => offender.file !== TOKEN_FILE
    );

    expect(
      formatOffenders(offenders),
      "raw hex colours outside src/design/tokens.css — use var(--cs-*) instead"
    ).toBe("no raw hex colours found");
  });

  it("defines the token vocabulary in exactly one file", () => {
    const filesWithColours = groupByFile(scanCssFiles(SRC_ROOT))
      .filter((group) => group.offenders.length > 0)
      .map((group) => group.file);

    expect(filesWithColours).toEqual([TOKEN_FILE]);
  });

  it("never references a design token that does not exist", () => {
    const tokens = definedTokens(readFileSync(`${SRC_ROOT}/${TOKEN_FILE}`, "utf8"));
    const undefinedTokens: string[] = [];

    for (const file of listCssFiles(SRC_ROOT)) {
      if (file === TOKEN_FILE) continue;
      const source = readFileSync(`${SRC_ROOT}/${file}`, "utf8");
      for (const token of referencedTokens(source)) {
        if (!tokens.has(token)) {
          undefinedTokens.push(`${file}: ${token}`);
        }
      }
    }

    expect(
      undefinedTokens,
      "these custom properties are used but never defined — the browser falls back to nothing"
    ).toEqual([]);
  });

  it("reports the token vocabulary it found", () => {
    const tokens = definedTokens(readFileSync(`${SRC_ROOT}/${TOKEN_FILE}`, "utf8"));
    // The vocabulary from docs/DESIGN_SYSTEM.md, sampled.
    for (const required of [
      "--cs-bg",
      "--cs-surface",
      "--cs-border",
      "--cs-text",
      "--cs-accent",
      "--cs-safe",
      "--cs-review",
      "--cs-offlimits",
      "--cs-focus",
      "--cs-space-4",
      "--cs-radius-md",
      "--cs-duration-base"
    ]) {
      expect(tokens.has(required), `${required} must be defined in ${TOKEN_FILE}`).toBe(true);
    }
  });
});

describe("the inventory of colours", () => {
  it("keeps a countable record of every colour the product ships", () => {
    const byFile = groupByFile(scanCssFiles(SRC_ROOT)).map((group) => ({
      file: group.file,
      count: group.offenders.length,
      distinct: distinctValues(group.offenders).length
    }));

    // The record is what the conformance report quotes; it must not be empty
    // for the token file, and it must be empty for everything else.
    const tokenFile = byFile.find((entry) => entry.file === TOKEN_FILE);
    expect(tokenFile?.count ?? 0).toBeGreaterThan(0);

    const outside = byFile.filter((entry) => entry.file !== TOKEN_FILE);
    expect(outside).toEqual([]);
  });
});
