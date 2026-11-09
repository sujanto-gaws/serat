// Fuzzy subsequence match for the command palette (FR-NAV-05). Commands are a
// few dozen strings, so this is a plain in-memory scan; nothing here is on the
// switcher's path (that ranking is the core's, ADR-0015 / architecture §9.2).

import type { Span } from "@/ipc/generated/Span";

export interface FuzzyHit {
  score: number;
  /** Matched ranges in `text` (UTF-16 code units), merged. */
  spans: Span[];
}

/** Case-insensitive subsequence match; `null` when `query` is not a subsequence of `text`. */
export function fuzzyMatch(query: string, text: string): FuzzyHit | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const spans: Span[] = [];
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const ch of q) {
    if (ch === " ") continue;
    const at = t.indexOf(ch, from);
    if (at < 0) return null;
    // Consecutive and word-start matches rank higher; early matches beat late ones.
    const wordStart = at === 0 || /[\s\-_/]/.test(t[at - 1]);
    score += (at === prev + 1 ? 3 : 1) + (wordStart ? 2 : 0) - at * 0.01;
    const last = spans[spans.length - 1];
    if (last && last.end === at) last.end = at + 1;
    else spans.push({ start: at, end: at + 1 });
    prev = at;
    from = at + 1;
  }
  return { score, spans };
}

export interface Piece {
  text: string;
  match: boolean;
}

/** Splits `text` into plain/matched pieces for rendering as separate text nodes. */
export function splitBySpans(text: string, spans: Span[]): Piece[] {
  const pieces: Piece[] = [];
  let at = 0;
  for (const span of spans) {
    const start = Math.max(at, Math.min(span.start, text.length));
    const end = Math.max(start, Math.min(span.end, text.length));
    if (start > at) pieces.push({ text: text.slice(at, start), match: false });
    if (end > start) pieces.push({ text: text.slice(start, end), match: true });
    at = end;
  }
  if (at < text.length) pieces.push({ text: text.slice(at), match: false });
  return pieces;
}
