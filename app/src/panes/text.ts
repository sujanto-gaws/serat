// Text helpers shared by the backlinks pane and the search panel (T-09:
// note text is rendered as text nodes only, never parsed as markup).

export interface Segment {
  text: string;
  match: boolean;
}

const MATCH_OPEN = "\u0002";
const MATCH_CLOSE = "\u0003";

/**
 * Splits a snippet on the backend's match delimiters (U+0002 ... U+0003) into
 * plain-text segments, each rendered as its own text node. A split, not a
 * parse: there is no HTML anywhere in this path, and the delimiters can also
 * occur in note text, where an unmatched one is harmless (it just toggles).
 * One function for the search panel's snippets and the backlinks excerpts.
 */
export function splitSnippet(snippet: string): Segment[] {
  const segments: Segment[] = [];
  let match = false;
  let current = "";
  const flush = (): void => {
    if (current !== "") segments.push({ text: current, match });
    current = "";
  };
  for (const part of snippet.split(/([\u0002\u0003])/)) {
    if (part === MATCH_OPEN) {
      flush();
      match = true;
    } else if (part === MATCH_CLOSE) {
      flush();
      match = false;
    } else {
      current += part;
    }
  }
  flush();
  return segments;
}

/** The snippet without its delimiters, for accessible names. */
export function plainSnippet(snippet: string): string {
  return splitSnippet(snippet)
    .map((s) => s.text)
    .join("");
}

/** The folder part of a vault-relative path (`""` for the root). */
export function folderOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

/** A display title: the given title, else the file name without `.md`. */
export function titleOf(path: string, title: string | null): string {
  if (title && title.trim()) return title;
  const name = path.slice(path.lastIndexOf("/") + 1);
  return name.replace(/\.md$/i, "");
}
