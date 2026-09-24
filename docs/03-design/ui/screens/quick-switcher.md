# Screen: Quick switcher

Serves: FR-NAV-01, NFR-PRF-06 · US-04
Design basis: architecture.md §9.2

## Layout

Overlay chrome per components.md §5: card centred horizontally, upper-third vertically, `elevation.overlay`, opened with `⌘` `O`.

```
┌──────────────────────────────────────────┐
│  Roadm█                                   │  input, typography.input
├──────────────────────────────────────────┤
│  📄 Roadmap 2026                 Projects  │  title match, path metadata
│  📄 Q3 Roadmap Review            Inbox     │
│  📄 roadmap-notes                Archive   │  path match (lower confidence)
│  📄 Team Roadmap                 Alias: RM │  alias match, labelled
├──────────────────────────────────────────┤
│  ↑↓ navigate   ↵ open   ⌘↵ new tab   esc  │  footer hint row
└──────────────────────────────────────────┘
```

With `#` present in the query (`Roadmap#risk`), results switch to heading mode for the matched note(s):

```
┌──────────────────────────────────────────┐
│  Roadmap#risk█                            │
├──────────────────────────────────────────┤
│  Roadmap 2026 › Open risks                │  note title, then heading
│  Roadmap 2026 › Risk log                  │
└──────────────────────────────────────────┘
```

## Content and ranking (as designed in architecture.md §9.2 — this spec renders it, not re-derives it)

- Candidates: title, vault-relative path, aliases (default mode); note headings after `#` (heading mode). Note prefix before `#` filters which notes' headings are searched; empty prefix searches every heading.
- Each result row shows: kind glyph, matched candidate text with matched characters bolded in `accent-interactive` (never background-highlighted — components.md §5), and right-aligned metadata in `text-tertiary`/`micro`: the containing folder for a title/path match, `Alias: <name>` for an alias match, nothing extra for a heading match (the note title already carries that context via the `›` separator).
- **An empty query** shows the 20 most frecent notes, unlabelled as "recent" — the ranking already communicates that, and a literal "Recent" eyebrow would be exactly the kind of unnecessary label the design guidance warns against.
- No match: "No note matches “xyz”" plus a secondary action row, `Create note "xyz"`, `↵`-able directly — turning a dead end into the obvious next step (writing guidance: an empty state is an invitation to act).

## Keyboard map

| Action | Binding |
|---|---|
| Open switcher | `⌘` `O` |
| Open switcher in link-insert mode (from inside the editor, typing `[[`) | Automatic after `[[` + 2 characters — same component, inline instead of overlaid, per architecture.md §9.3 |
| Move selection | `↑` `↓` (wraps at ends) |
| Jump to heading mode | Type `#` — no separate keybinding; it's a query-syntax switch, consistent with the search panel's `[key:value]` convention |
| Open selected in current tab | `↵` |
| Open selected in new tab | `⌘` `↵` |
| Dismiss, return focus to origin | `Esc` |

This table is a subset of the single authoritative binding set in `components.md` §7.

## States

| State | Presentation |
|---|---|
| Typing (< 3 chars) | Results still update (the 50ms budget is measured from the 3rd character; 1–2 character queries return early and are simply less selective — no artificial "type more" gate). |
| Results returned | List as above, first row pre-selected. |
| Stale response arriving late (sequence-numbered, per §9.2) | Discarded silently by the front end — never briefly flashes an outdated list before the fresher one. Nothing to design here beyond "the visible list only ever reflects the newest keystroke," which is the point of the guarantee. |
| No `.serat/history.json` yet (fresh vault) or corrupted | Ranking degrades to match-class + scorer only, silently — no error shown, because this is capability degrading, not failing (design goal 5). |
| Pre-`Ready` index (first open / rebuild) | Candidates are paths and file names only, from the ADR-0015 tree listings already streaming in; the switcher works from the very first directory listing rather than waiting for the index. No visible "limited mode" banner — results are simply narrower until titles/aliases/headings join in, which is consistent with never surfacing index machinery in a content-facing surface (architecture.md §9.1). |

## Responsive rules

- Card width: 560px on wide windows, down to `min(560px, 90vw)` — never edge-to-edge, even in the compact breakpoint, because an edge-to-edge overlay stops reading as "floating above the page."
- Result count visible before scrolling: at least 6 rows at any window height ≥ 480px.

## Accessibility

- `role="listbox"` / `role="option"` pattern with `aria-activedescendant` tracking the selected row, so arrow-key navigation is announced correctly without moving DOM focus off the input.
- The footer hint row is `aria-hidden` (decorative reinforcement of keys already usable) but the same actions are separately exposed as real labelled buttons for screen-reader and switch-access users who cannot chord `⌘` `↵`.
- Match-highlighting (bold + colour) is never the only way a match is conveyed to assistive tech: the accessible name for each option includes the plain matched text, not markup.
