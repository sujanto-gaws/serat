// Where a switcher heading result lands in the editor (FR-NAV-01, heading mode).
// `HeadingRef.byte_start` is a byte offset into the file as it was indexed, and
// "the note may have changed" since (contracts/ipc.rs). So the offset is only
// trusted when the line it points at is a heading with the expected text;
// otherwise the note is searched for that heading, else it opens at the start.
// The offset is raw-file based (CRLF counts two) and the editor document is
// LF-normalised, so it goes through `rawIndexToDocOffset` like `locateBlock`.

import { byteToUtf16Index, rawIndexToDocOffset } from "@/panes/locate";

const HEADING_LINE = /^ {0,3}#{1,6}[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$/;

function headingTextOf(line: string): string | null {
  const m = HEADING_LINE.exec(line.replace(/\r$/, ""));
  return m ? m[1].trim() : null;
}

/** Editor-document offset (UTF-16, LF) of the heading `text`, near `byteStart` in `raw`. */
export function headingDocOffset(raw: string, byteStart: number, text: string): number {
  const want = text.trim();
  const index = byteToUtf16Index(raw, byteStart);
  const lineStart = raw.lastIndexOf("\n", index - 1) + 1;
  const lineEndAt = raw.indexOf("\n", index);
  const lineEnd = lineEndAt < 0 ? raw.length : lineEndAt;
  if (headingTextOf(raw.slice(lineStart, lineEnd)) === want) return rawIndexToDocOffset(raw, lineStart);
  // Moved: first heading line with this text.
  let at = 0;
  while (at <= raw.length) {
    const end = raw.indexOf("\n", at);
    const stop = end < 0 ? raw.length : end;
    if (headingTextOf(raw.slice(at, stop)) === want) return rawIndexToDocOffset(raw, at);
    if (end < 0) break;
    at = end + 1;
  }
  return 0;
}
