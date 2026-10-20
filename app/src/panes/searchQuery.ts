// Client-side checks on a search query before anything is sent (search-panel.md,
// architecture.md §4.5). The backend parses the query into one of three fixed
// statements; these checks only stop input that is known to be refused from
// round-tripping.

export type QueryCheck =
  | { kind: "empty" }
  | { kind: "ok"; text: string; filter: string | null }
  /** An odd number of `"`: "Close the quote to search a phrase". */
  | { kind: "unbalanced_quote" }
  /** A second complete `[key:value]` term: left as text, never silently dropped. */
  | { kind: "second_filter"; first: string; second: string };

const FILTER = /\[([^[\]\s:]+):([^[\]]*)\]/g;

/** Splits `query` into text outside quotes and the `[key:value]` terms found there. */
export function checkQuery(query: string): QueryCheck {
  const trimmed = query.trim();
  if (trimmed === "") return { kind: "empty" };
  const quotes = (trimmed.match(/"/g) ?? []).length;
  if (quotes % 2 === 1) return { kind: "unbalanced_quote" };

  // Terms inside "quoted phrases" are text, not filters.
  let outside = "";
  let inQuote = false;
  for (const ch of trimmed) {
    if (ch === '"') {
      inQuote = !inQuote;
      outside += " ";
    } else {
      outside += inQuote ? " " : ch;
    }
  }
  const filters = [...outside.matchAll(FILTER)].map((m) => m[0]);
  if (filters.length > 1) return { kind: "second_filter", first: filters[0], second: filters[1] };
  const text = trimmed.replace(filters[0] ?? "", "").replace(/\s+/g, " ").trim();
  return { kind: "ok", text, filter: filters[0] ?? null };
}

export const SECOND_FILTER_MESSAGE = (first: string): string =>
  `One property filter at a time — remove ${first.slice(1, -1)} first.`;

export const UNBALANCED_QUOTE_MESSAGE = "Close the quote to search a phrase";

export const CASE_SENSITIVE_HINT =
  "Property filters are case-sensitive: [type:project] does not match a note with type: Project.";
