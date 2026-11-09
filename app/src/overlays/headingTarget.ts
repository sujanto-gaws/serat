// Where a switcher heading result lands in the editor (FR-NAV-01, heading mode).
// `HeadingRef.byte_start` is a byte offset into the file as it was indexed, and
// "the note may have changed" since (contracts/ipc.rs). The index stores heading
// text as plain text (`## **Open** risks` is "Open risks"), so the offset is
// trusted when the line there is ANY heading (ATX, or a setext title whose next
// line is an underline); only when it is not is the note searched for a heading
// whose plain text matches, else it opens at the start.
// The offset is raw-file based (CRLF counts two) and the editor document is
// LF-normalised, so it goes through `rawIndexToDocOffset` like `locateBlock`.

import { byteToUtf16Index, rawIndexToDocOffset } from "@/panes/locate";

const ATX = /^ {0,3}#{1,6}(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const SETEXT_UNDERLINE = /^ {0,3}(?:=+|-+)[ \t]*$/;

/** Heading text with inline markup reduced to plain text, as the index stores it. */
export function plainHeadingText(text: string): string {
  return text
    .replace(/\[\[(?:[^\]|]*\|)?([^\]]*)\]\]/g, "$1")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__|\*|~~|`)/g, "") // emphasis/code delimiters
    .replace(/(^|[^\w])_+|_+(?=[^\w]|$)/g, "$1") // single `_` emphasis at word edges; snake_case stays
    .replace(/\s+/g, " ")
    .trim();
}

/** The heading text of `line`, given the line after it (for setext); null when it is no heading. */
function headingOf(line: string, next: string | undefined): string | null {
  const l = line.replace(/\r$/, "");
  const atx = ATX.exec(l);
  if (atx) return plainHeadingText(atx[1] ?? "");
  if (l.trim() !== "" && next !== undefined && SETEXT_UNDERLINE.test(next.replace(/\r$/, ""))) return plainHeadingText(l);
  return null;
}

interface Line {
  start: number;
  text: string;
}

/** Index of the first line after a leading `---` frontmatter block (0 when there is none). */
function bodyStart(lines: Line[]): number {
  if (lines[0]?.text.replace(/\r$/, "") !== "---") return 0;
  for (let i = 1; i < lines.length; i++) {
    if (/^(---|\.\.\.)[ \t]*\r?$/.test(lines[i].text)) return i + 1;
  }
  return 0;
}

function linesOf(raw: string): Line[] {
  const lines: Line[] = [];
  let at = 0;
  for (;;) {
    const end = raw.indexOf("\n", at);
    lines.push({ start: at, text: raw.slice(at, end < 0 ? raw.length : end) });
    if (end < 0) return lines;
    at = end + 1;
  }
}

/** Editor-document offset (UTF-16, LF) of the heading `text`, near `byteStart` in `raw`. */
export function headingDocOffset(raw: string, byteStart: number, text: string): number {
  const lines = linesOf(raw);
  const index = byteToUtf16Index(raw, byteStart);
  const at = lines.findIndex((l, i) => index >= l.start && (i === lines.length - 1 || index < lines[i + 1].start));
  const first = bodyStart(lines);
  if (at >= first && headingOf(lines[at].text, lines[at + 1]?.text) !== null) return rawIndexToDocOffset(raw, lines[at].start);
  const want = plainHeadingText(text);
  for (let i = first; i < lines.length; i++) {
    if (headingOf(lines[i].text, lines[i + 1]?.text) === want) return rawIndexToDocOffset(raw, lines[i].start);
  }
  return 0;
}
