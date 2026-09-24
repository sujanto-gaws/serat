# Screen: Backlinks pane — block-precise excerpts

Serves: FR-NAV-04 · US-03
Design basis: architecture.md §4.5 (R2 backlinks, P1-04 branch)

## Layout

Lives in the context pane (right), as a section beneath the note outline, or as its own tab if the context pane is showing something else. Sections are plain headings (`ui-medium`, `small`), not cards — a backlinks list is a list, not a grid of tiles.

```
┌ context pane ───────────────────────┐
│ Outline                              │
│  …                                   │
│ Backlinks (3)                        │
│  ┌ Weekly Standup ──────────────┐    │
│  │ …decided to ship the tree    │    │
│  │ progressively rather than…   │    │
│  └───────────────────────────────┘    │
│  ┌ Weekly Standup ──────────────┐    │
│  │ …and Priya flagged the       │    │
│  │ sidecar fold as the one…     │    │
│  └───────────────────────────────┘    │
│  ┌ Roadmap ──────────────────────┐    │
│  │ frontmatter: owner            │    │
│  └───────────────────────────────┘    │
└───────────────────────────────────────┘
```

## Content model

One entry per **distinct (source note, source block)** — exactly the R2 read's shape and exactly FR-NAV-04's acceptance case: three paragraphs across two notes yields three entries, and two links inside one paragraph collapse to one entry (the read already de-duplicates; the UI does not re-count).

Each entry:
- **Source note title** (`ui-medium`, `small`), acts as a link — click or `↵` opens that note, scrolled to the linking block.
- **Excerpt**: the block's text, up to 300 characters, in `body-serif` at `small`-adjacent size (the pane is narrower than the editor, so excerpts run at 13–14px serif rather than the editor's 17px, still serif because it is quoted prose from a note, not UI chrome).
- **Frontmatter-origin links** (e.g. `owner: [[Person]]`) show the field key in `mono`, `micro`, `text-tertiary` instead of a prose excerpt, labelled exactly as the read returns it: *frontmatter*.
- No relevance score, no "3 links from this note" collapsing — every entry is its own block-precise row, because collapsing by source note is exactly what FR-NAV-04 explicitly does *not* want (the acceptance criterion is block count, not note count).

## Grouping

Entries are grouped by source note (a light `text-tertiary` note-title header once per source, not repeated per entry) but each block still renders as its own row underneath — grouping is a visual convenience, not a merge of the data shown above.

## States

| State | Presentation |
|---|---|
| Loading (query in flight, R2 ≤ 20ms expectation but shown anyway for consistency) | Two skeleton rows at reduced opacity for ~100ms minimum before showing anything, so a near-instant response doesn't flash. |
| Populated | As above. |
| Empty (no inbound links) | One line: "Nothing links here yet." Not an error — most new notes have no backlinks, and the tone should not apologise for that. |
| Note has unresolved inbound links pointing at it under a different casing/path (edge case) | Not shown here — unresolved links never appear in any backlinks pane, per architecture.md §4.5 ("an unresolved link names no note"); this is a deliberate absence, not a bug, and this spec states it so it isn't "fixed" later. |
| Note deleted mid-session while its backlinks pane is open | Pane shows "This note no longer exists" and stops updating; the tree/editor-close flow is what actually removes the pane. |
| Ambiguous target resolution (two `Meeting.md` files) | Only the note the resolver actually points at (shorter path, §9.3's rule) shows the backlink — the other `Meeting.md`'s pane legitimately shows nothing for that link, which matches Obsidian's own behaviour and is not a defect to flag to the user. |

## Interaction

- Clicking an excerpt opens the source note with the cursor placed at the linking block's start, and the block briefly (400ms, then fades) gets a `bg-selected-weak` highlight so the destination is visually confirmed — the one flash-on-navigate this system permits, because it answers a user action (goal: "motion that answers a person's action is welcome").
- Keyboard: `↓`/`↑` moves through entries when the pane has focus; `↵` opens the focused entry the same way a click does; `⌘` `↵` opens it in a new tab, matching the tree's convention.

## Keyboard map

| Action | Binding |
|---|---|
| Focus backlinks section | `⌘` `3` then `Tab` into the list, or the palette command `Focus backlinks` |
| Open focused entry | `↵` |
| Open focused entry in new tab | `⌘` `↵` |
| Refresh (rarely needed; the pane already updates on index change) | Not bound — there is no manual refresh action, because a stale-and-refreshable backlinks pane would be a second source of truth to reason about; it always reflects the current index. |

## Responsive rules

- In the collapsed/flyover context pane (< `breakpoint-narrow`), the backlinks section is reachable via the same flyover and keeps the same layout, just narrower — excerpts wrap rather than truncate, since a truncated quoted sentence is worse than a two-line one.

## Accessibility

- The list is `role="list"`; each entry is a `listitem` with an accessible name combining source title and excerpt, so a screen reader announces "Weekly Standup: decided to ship the tree progressively rather than…" as one navigable unit.
- The post-navigation highlight flash respects `prefers-reduced-motion` (becomes an instant, non-animated background with the same 400ms hold, no fade transition).
