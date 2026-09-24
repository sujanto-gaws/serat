# Screen: Search panel — snippet, jump-to-location, one [key:value] filter

Serves: FR-NAV-02 (P1 half: notes only — PDF-content search is P2/out of scope here) · US-05
Design basis: architecture.md §4.5 (R1 search, R3 property lookup, R1+R3; P1-04 branch)

## Layout

A docked panel (not an overlay like the switcher/palette — search results are a working set someone reads down, not a single pick-and-dismiss action), opened as a tab in the context pane or as a full-height left-of-editor panel when explicitly opened via `⌘` `⇧` `F`. Distinct from the quick switcher (which finds a note by name) and the tree filter (which filters what's expanded) — this is the one surface that reads inside note bodies.

```
┌ search ──────────────────────────────────────┐
│  budget [type:project]              ×          │  input + filter chip
├────────────────────────────────────────────────┤
│  23 results                                     │  count, not a spinner
│  Roadmap 2026                          Projects │
│   …the Q3 **budget** review moved the whole…    │
│  Vendor contract                       Archive  │
│   …renegotiated **budget** line before the…     │
└──────────────────────────────────────────────────┘
```

## Input model (FR-NAV-02 P1, notes only)

- Free text, `"quoted phrases"`, and **at most one** `[key:value]` term — exactly the closed set R1/R3/R1+R3 compile to (architecture.md §4.5). This is stated here because it constrains the UI directly: there is no way to add a second filter chip.
- Typing a `[key:value]` term renders it as a **filter chip** (components.md §6: mono, brass) inline in the input the moment its closing `]` is typed, visually separating "what you typed as a filter" from "what you typed as text" — the same way a mail client turns a recipient into a chip.
- **A second `[key:value]` term is refused, not silently dropped**: typing the opening `[` of a second filter, when one chip already exists, shows an inline message below the input — "One property filter at a time — remove `type:project` first" — and the characters are still typed as literal text (not swallowed), so nothing the user typed disappears without explanation. This matches architecture.md §4.5's own rule precisely: "refused with a message and never silently dropped."
- Removing the chip (click its `×`, or `Backspace` when the cursor is immediately after it) returns to free-text-only search.
- Search runs on `Enter` and after 250ms of idle typing (§4.5) — **not** search-as-you-type at every keystroke like the switcher, and the panel does not pretend otherwise: there's a deliberate, small (~250ms) settle before results update, which avoids a result list that reflows on every character for a query type that's typically longer than a switcher query.

## Results (R1 search)

- Each row: source title (`ui-medium`) + folder (`text-tertiary`, `micro`) on one line; snippet below in `body-serif` at reduced size (prose, because it is quoted note text — same reasoning as the backlinks pane), with matched terms bolded in `accent-interactive`, delimited server-side by control characters and converted to spans client-side — **never raw HTML from a note** (T-09; architecture.md §4.5 "results never carry HTML").
- Results paginate at 50 (R1's `LIMIT`); a "Load more" row at the bottom fetches the next page with the same query — no infinite auto-scroll, so the result count stays a stable, readable number rather than growing under the cursor.
- **R1+R3** (free text plus one property filter): identical row layout; the filter chip stays visible above the results the whole time so it's never ambiguous what's constraining the list.
- **Filter-only search** (a `[key:value]` term with no free text — pure R3): rows show the matching notes/assets with no snippet (there's no text match to excerpt) — instead a small `mono`/`micro` line showing the matched field and value, e.g. `type: project`.
- **A sidecar hit is returned as its asset** (architecture.md §4.5): the row shows the asset's name and format glyph, not the `.md` sidecar's filename — consistent with the tree's folding rule, so the two surfaces don't disagree about what a paired asset "is."

## Jump-to-location

- Clicking a result opens the source note (or, for a sidecar/asset hit, the binary viewer) with the cursor/viewport placed at the matched block, using the same byte-range → `{line, col}` mapping the editor's save path uses (ADR-0016's function), so this is never a second, divergent notion of "where."
- **Approximate location**: when the note's hash has changed since it was indexed (edited since, not yet reindexed), the block is found by its `ord` instead of its byte range, and the jump lands at the block's current position with a small `text-tertiary` note beneath the result row: "approximate — this note has changed since it was indexed." This is stated on the result itself, before the click, not as a surprise after navigating — the design decision in §4.5 is "never a wrong line presented as exact," and the UI honours that by never hiding the distinction.

## States

| State | Presentation |
|---|---|
| Empty input | Panel shows recent searches (up to 5, from session memory only — not persisted to `.serat/`, unlike the switcher's frecency file, since search history is more sensitive to show accidentally and P1 draws no requirement asking for it to persist) or, on first use, one line of guidance: "Search note text, or narrow with `[key:value]`." |
| Typing, before the 250ms settle | No loading indicator yet — nothing has been asked of the backend. |
| Waiting on results (rare given the ≤100ms expectation, but designed for) | Two skeleton rows, same convention as the backlinks pane, shown only if the wait exceeds ~150ms (avoids a flash for the common instant case). |
| Results | As above. |
| No results | "No matches for “budget” — try removing the `[type:project]` filter" when a filter is active (naming the exact next action), or plainly "No matches for “budget”" with no filter active. |
| Second filter attempted | Inline refusal message, described above, auto-dismisses after the offending characters are edited. |
| Vault index not `Ready` | Search still runs (R1/R3 are index reads); if the index has no rows yet (very first open, before any table exists) the panel says "Still building the search index — try again in a moment," matching the "index state as a fact, not a fault" tone used elsewhere. |

## Keyboard map

| Action | Binding |
|---|---|
| Open/focus search panel | `⌘` `⇧` `F` |
| Run search | `↵` (also auto-runs after 250ms idle) |
| Remove active filter chip | `Backspace` immediately after it, or click its `×` |
| Move through results | `↓` `↑` |
| Open focused result | `↵` |
| Open focused result in new tab | `⌘` `↵` |
| Load next page | `⌘` `↵` on the "Load more" row, or click |
| Close panel, return focus | `Esc` |

## Responsive rules

- In the docked-tab placement, the panel shares the context pane's collapse behaviour (`file-tree.md`/`editor.md` responsive rules); in the full-height placement it collapses to a flyover below `breakpoint-compact`, same as the tree.
- Snippets wrap rather than truncate at narrow widths (same reasoning as backlinks: a broken quoted sentence is worse than a wrapped one).

## Accessibility

- Results list is `role="list"`; the filter chip is a real focusable, labelled control (`aria-label="Filter: type is project. Press Backspace or activate to remove."`), not a decorative pill.
- The refusal message for a second filter is announced via a polite live region, not only shown visually.
- Matched-term bolding in snippets is reinforced by the accessible name including the plain snippet text (no reliance on visual bold alone), consistent with the switcher/palette convention.
