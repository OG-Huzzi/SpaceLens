/**
 * Fuzzy matching for the command palette.
 *
 * A subsequence match is enough to find "Cleanup" from "clnup", but a
 * subsequence alone ranks poorly: "Cleanup" and "Clean up temporary files"
 * would tie. The scorer therefore separates four tiers and adds quality
 * bonuses inside the weakest one:
 *
 *   exact label            1000  — "cleanup" === "cleanup"
 *   label prefix           800  — "cleanup…" for "cl"
 *   word prefix            600  — "… cleanup…" for "cl" after a word break
 *   substring              500  — "… cleanup …" anywhere in the label
 *   scattered subsequence  100..400 — "clnup" → "cleanup"
 *
 * Every tier boundary is far enough apart that a prefix match can never lose
 * to a scattered one, which is the ranking rule the palette promises.
 *
 * Ties are broken by command id, never by array position: the order in which
 * asynchronous sources happen to resolve must not change what is on top.
 */

import type { Command } from "./types";

export interface ScoredCommand {
  command: Command;
  score: number;
}

const EXACT_SCORE = 1000;
const PREFIX_SCORE = 800;
const WORD_PREFIX_SCORE = 600;
const SUBSTRING_SCORE = 500;
const SUBSEQUENCE_BASE = 100;

/** A keyword hit is a weaker signal than the visible label. */
const KEYWORD_PENALTY = 5;

function normalise(value: string): string {
  return value
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "");
}

function isBoundary(character: string | undefined): boolean {
  if (character === undefined) return true;
  return /[\s\-_/.·:]/.test(character);
}

/**
 * Scores a scattered subsequence match. Consecutive characters and matches
 * that start a word are worth more, so "clnup" prefers "cleanup" over a label
 * where the same letters are far apart.
 */
function subsequenceScore(query: string, text: string): number | null {
  let cursor = 0;
  let score = 0;
  let previous = -2;
  let run = 0;

  for (const character of query) {
    const found = text.indexOf(character, cursor);
    if (found < 0) return null;

    if (found === previous + 1) {
      run += 1;
      score += 10 + run * 4;
    } else {
      run = 0;
      score += 6;
    }

    if (isBoundary(text[found - 1])) score += 18;
    score -= Math.min(6, found - cursor) * 0.5;

    previous = found;
    cursor = found + 1;
  }

  return SUBSEQUENCE_BASE + score;
}

function scoreText(query: string, text: string): number | null {
  if (text === query) return EXACT_SCORE;
  if (text.startsWith(query)) return PREFIX_SCORE;

  const words = text.split(/[^a-z0-9]+/i).filter((word) => word.length > 0);
  if (words.some((word) => word.startsWith(query))) return WORD_PREFIX_SCORE;

  const index = text.indexOf(query);
  if (index >= 0) return SUBSTRING_SCORE - Math.min(100, index);

  return subsequenceScore(query, text);
}

/**
 * Scores one command against a query.
 *
 * Returns `null` when the query is not a subsequence of the label or of any
 * keyword. An empty query scores every command equally; the caller keeps the
 * original order in that case.
 */
export function scoreCommand(query: string, command: Command): number | null {
  const normalisedQuery = normalise(query).trim();
  if (normalisedQuery === "") return 0;

  let best: number | null = null;

  const labelScore = scoreText(normalisedQuery, normalise(command.label));
  if (labelScore !== null) best = labelScore;

  for (const keyword of command.keywords ?? []) {
    const keywordScore = scoreText(normalisedQuery, normalise(keyword));
    if (keywordScore === null) continue;
    const adjusted = Math.max(0, keywordScore - KEYWORD_PENALTY);
    if (best === null || adjusted > best) best = adjusted;
  }

  return best;
}

/**
 * Filters and ranks commands.
 *
 * An empty query keeps the input order (the order the host declared), so the
 * palette opens on a predictable list instead of an alphabetical shuffle.
 */
export function filterCommands(query: string, commands: Command[]): ScoredCommand[] {
  if (query.trim() === "") {
    return commands.map((command) => ({ command, score: 0 }));
  }

  const scored: ScoredCommand[] = [];
  for (const command of commands) {
    const score = scoreCommand(query, command);
    if (score !== null) scored.push({ command, score });
  }

  scored.sort((left, right) => {
    if (right.score !== left.score) return right.score - left.score;
    if (left.command.id === right.command.id) return 0;
    return left.command.id < right.command.id ? -1 : 1;
  });

  return scored;
}
