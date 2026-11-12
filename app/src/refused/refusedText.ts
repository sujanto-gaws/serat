// Wording and untrusted-text handling for the refused-names surface
// (docs/03-design/ui/screens/file-tree.md "Names not shown"; FR-VLT-01, NFR-SEC-06).

/** `REFUSED_NAMES_CAP` in contracts/ipc.rs: the log, the status count and the list all stop here. */
export const REFUSED_NAMES_CAP = 1000;

/** Trailing debounce of the strip's live region (a walk raising the count name by name announces once). */
export const ANNOUNCE_DEBOUNCE_MS = 1000;
/** Trailing debounce of the panel's re-fetch when the count changes while it is open. */
export const REFETCH_DEBOUNCE_MS = 500;

/** First open only: "Loading the list" appears if the call is still pending after this long. */
export const LOADING_DELAY_MS = 150;

/** The strip button's visible text. `null` = render nothing (absent or zero is never "0 names not shown"). */
export function refusedCountText(count: number | null): string | null {
  if (count === null || count <= 0) return null;
  if (count >= REFUSED_NAMES_CAP) return `${REFUSED_NAMES_CAP}+ names not shown`;
  return count === 1 ? "1 name not shown" : `${count} names not shown`;
}

// Control (Cc), format incl. bidi, zero-width, tag characters and the soft hyphen (Cf), and line/paragraph
// separators (Zl, Zp), plus the escape's own opening bracket U+27E8: a literal "<U+202E>" typed into a name
// must not render identically to a real U+202E, so it is itself escaped.
const ESCAPE_OPEN = String.fromCodePoint(0x27e8);
const ESCAPE_CLOSE = String.fromCodePoint(0x27e9);
const UNSAFE = new RegExp("[\\p{Cc}\\p{Cf}\\p{Zl}\\p{Zp}" + ESCAPE_OPEN + "]", "u");

/**
 * Shows control and bidi characters as visible escapes like `U+202E` in angle brackets, so a hostile name
 * cannot reorder its own display or hide its extension. Only ever rendered as text, never as HTML.
 */
export function escapeUntrusted(text: string): string {
  let out = "";
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    out += UNSAFE.test(ch) ? `${ESCAPE_OPEN}U+${code.toString(16).toUpperCase().padStart(4, "0")}${ESCAPE_CLOSE}` : ch;
  }
  return out;
}
