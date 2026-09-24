# Screen: Search panel — snippet, jump-to-location, one [key:value] filter

Serves: FR-NAV-02 (P1 half: notes only — PDF-content search is P2/out of scope here) · US-05
Design basis: architecture.md §4.5 (R1 search, R3 property lookup, R1+R3)

## Layout

A docked panel (not an overlay like the switcher/palette — search results are a working set someone reads down, not a single pick-and-dismiss action), opened as a tab in the context pane or as a full-height left-of-editor panel when explicitly opened via `⌘` `⇧` `F` — a **global** binding, available regardless of which pane has focus. Distinct from the quick switcher (which finds a note by name) and the tree filter (which filters what's expanded, bound to the unrelated, tree-local `⌘F` — see the master table in `components.md` §7 for why the two don't collide) — this is the one surface that reads inside note bodies.

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
- **A second `[key:value]` term is refused, not silently dropped.** The refusal triggers on a **complete** second `[key:value]` token — its closing `]` typed, or the whole token arriving at once via paste or drag — never on the opening `[` alone (typing a literal `[` for prose, or starting to type a second filter before deciding to delete it, must not trigger a message on every keystroke). The moment a second complete filter token is detected while one chip already exists, it is left as **plain text, not turned into a chip**, and an inline message appears below the input — "One property filter at a time — remove `type:project` first." The characters themselves are never swallowed (typed or pasted), so nothing the user entered disappears without explanation. This matches architecture.md §4.5's own rule precisely: "refused with a message and never silently dropped."
- Removing the chip (click its `×`, or `Backspace` when the cursor is immediately after it) returns to free-text-only search.
- Search runs on `Enter` and after 250ms of idle typing (§4.5) — **not** search-as-you-type at every keystroke like the switcher, and the panel does not pretend otherwise: there's a deliberate, small (~250ms) settle before results update, which avoids a result list that reflows on every character for a query type that's typically longer than a switcher query.

## Results (R1 search)

- Each row: source title (`ui-medium`) + folder (`text-tertiary`, `micro`) on one line; snippet below in `body-serif` at reduced size (prose, because it is quoted note text — same reasoning as the backlinks pane), with matched terms bolded in `accent-interactive`. **Rendering the snippet is a split, not a parse.** The backend delimits matches with U+0002/U+0003; the front end splits the snippet string on those two control characters (`String.split`, never a regex-as-HTML substitution) into an array of plain-text segments, and renders each segment as its own text node — either a plain span or the bold-match span — through the framework's normal text interpolation (`{{ segment }}`). No segment is ever passed through `innerHTML`, `v-html`, or any HTML-parsing step; there is no HTML to parse, because the raw note text was never HTML to begin with (T-09; architecture.md §4.5 "results never carry HTML"). The same splitting rule is used for the backlinks pane's excerpts (`backlinks-pane.md`) — one function, not two.
- Results paginate at 50 (R1's `LIMIT`); a "Load more" row at the bottom fetches the next page with the same query — no infinite auto-scroll, so the result count stays a stable, readable number rather than growing under the cursor.
- **R1+R3** (free text plus one property filter): identical row layout; the filter chip stays visible above the results the whole time so it's never ambiguous what's constraining the list.
- **Filter-only search** (a `[key:value]` term with no free text — pure R3): rows show the matching notes/assets with no snippet (there's no text match to excerpt) — instead a small `mono`/`micro` line showing the matched field and value, e.g. `type: project`.
- **A sidecar hit is returned as its asset** (architecture.md §4.5): the row shows the asset's name and format glyph, not the `.md` sidecar's filename — consistent with the tree's folding rule, so the two surfaces don't disagree about what a paired asset "is."

## Jump-to-location

- Clicking a result opens the source note (or, for a sidecar/asset hit, the binary viewer) with the cursor/viewport placed at the matched block, using the same byte-range → `{line, col}` mapping the editor's save path uses (ADR-0016's function), so this is never a second, divergent notion of "where."
- **Approximate location**: when the note's hash has changed since it was indexed (edited since, not yet reindexed), the block is found by its `ord` instead of its byte range, and the jump lands at the block's current position with a small `text-secondary` note beneath the result row (required information about the reliability of what's about to happen on click, so it gets the higher-contrast token, not `text-tertiary`): "approximate — this note has changed since it was indexed." This is stated on the result itself, before the click, not as a surprise after navigating — the design decision in §4.5 is "never a wrong line presented as exact," and the UI honours that by never hiding the distinction.

## States

| State | Presentation |
|---|---|
| Empty input | Panel shows recent searches (up to 5, from session memory only — not persisted to `.serat/`, unlike the switcher's frecency file, since search history is more sensitive to show accidentally and P1 draws no requirement asking for it to persist) or, on first use, one line of guidance: "Search note text, or narrow with `[key:value]`." |
| Typing, before the 250ms settle | No loading indicator yet — nothing has been asked of the backend. |
| Waiting on results (rare given the ≤100ms expectation, but designed for) | Two skeleton rows, same convention as the backlinks pane, shown only if the wait exceeds ~150ms (avoids a flash for the common instant case). |
| Results | As above. |
| No results | "No matches for “budget” — try removing the `[type:project]` filter" when a filter is active (naming the exact next action), or plainly "No matches for “budget”" with no filter active. |
| Second filter attempted | Inline refusal message, described above, auto-dismisses after the offending characters are edited. |
| Punctuation-only query, or an unbalanced `"` (e.g. `"budget` with no closing quote) | **Never sent to FTS5 as-is and never shown as an error.** Because free text is always quoted into an FTS5 string in Rust (§4.5: "user text can therefore never be FTS5 syntax"), a punctuation-only query simply quotes to an FTS5 string that matches nothing meaningful and returns the ordinary "No results" state above — nothing breaks, there's just nothing useful to find. An **unbalanced quote is caught before it's sent**, with an inline message in the same style as the second-filter refusal: "Close the quote to search a phrase" — the input is not submitted while unbalanced, so the panel never round-trips a malformed phrase through the backend to find out it failed. |
| Index not yet `Ready` (first open, or a rebuild in progress) | **No partial results are ever shown.** Per architecture.md §4.5/`contracts/ipc.rs` rule 1, a read against a not-`Ready` index returns the index state, not rows — this is a backend guarantee, not a UI choice to interpret loosely. The panel reflects that state directly and does not attempt to query in the meantime: "Building the index — search available shortly," replacing the results area entirely (no stale or partial list left showing underneath it), matching the "index state as a fact, not a fault" tone used elsewhere. The panel automatically re-runs the pending query once `Ready` fires — no manual retry needed. |

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

This table is a subset of the single authoritative binding set in `components.md` §7.

## Responsive rules

- In the docked-tab placement, the panel shares the context pane's collapse behaviour (`file-tree.md`/`editor.md` responsive rules); in the full-height placement it collapses to a flyover below `breakpoint-compact`, same as the tree.
- Snippets wrap rather than truncate at narrow widths (same reasoning as backlinks: a broken quoted sentence is worse than a wrapped one).

## Accessibility

- Results list is `role="list"`; the filter chip is a real focusable, labelled control (`aria-label="Filter: type is project. Press Backspace or activate to remove."`), not a decorative pill.
- The refusal message for a second filter is announced via a polite live region, not only shown visually.
- Matched-term bolding in snippets is reinforced by the accessible name including the plain snippet text (no reliance on visual bold alone), consistent with the switcher/palette convention.
