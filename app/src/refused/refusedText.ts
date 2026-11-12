// Wording and untrusted-text handling for the refused-names surface
// (docs/03-design/ui/screens/file-tree.md "Names not shown"; FR-VLT-01, NFR-SEC-06).

/** `REFUSED_NAMES_CAP` in contracts/ipc.rs: the log, the status count and the list all stop here. */
export const REFUSED_NAMES_CAP = 1000;

/** Trailing debounce of the strip's live region (a walk raising the count name by name announces once). */
export const ANNOUNCE_DEBOUNCE_MS = 1000;
/** Trailing debounce of the panel's re-fetch when the count changes while it is open. */
export const REFETCH_DEBOUNCE_MS = 500;

/** The strip button's visible text. `null` = render nothing (absent or zero is never "0 names not shown"). */
export function refusedCountText(count: number | null): string | null {
  if (count === null || count <= 0) return null;
  if (count >= REFUSED_NAMES_CAP) return `${REFUSED_NAMES_CAP}+ names not shown`;
  return count === 1 ? "1 name not shown" : `${count} names not shown`;
}

// Control characters (C0, DEL, C1), line/paragraph separators, zero-width and bidi formatting characters.
const UNSAFE_RANGES: ReadonlyArray<readonly [number, number]> = [
  [0x0000, 0x001f],
  [0x007f, 0x009f],
  [0x061c, 0x061c],
  [0x200b, 0x200f],
  [0x2028, 0x202e],
  [0x2060, 0x2069],
  [0xfeff, 0xfeff],
];

function isUnsafe(code: number): boolean {
  return UNSAFE_RANGES.some(([lo, hi]) => code >= lo && code <= hi);
}

/**
 * Shows control and bidi characters as visible `⟨U+202E⟩` escapes so a hostile name cannot reorder its
 * own display or hide its extension. The result is only ever rendered as text, never as HTML.
 */
export function escapeUntrusted(text: string): string {
  let out = "";
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    out += isUnsafe(code) ? `⟨U+${code.toString(16).toUpperCase().padStart(4, "0")}⟩` : ch;
  }
  return out;
}
