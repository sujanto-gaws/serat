# Screen: File tree — progressive population, attachments, sidecar folding, insert-link

Serves: FR-VLT-01, FR-VLT-08, FR-BIN-03 · US-01, US-18
Design basis: ADR-0015 (complete directory listings, progressive), architecture.md §3.5, §7.4

## Layout

Left pane, `layout.tree-pane-width-default` (260px), resizable within `tree-pane-width-min/max`. A single virtualised list (ADR-0015 §4 — DOM rows proportional to viewport, not vault size), one row per visible entry at `layout.row-height-tree` (26px).

```
┌ tree pane ──────────────────┐
│ [vault name]            [+] │  toolbar: new note, new folder, filter
│ ▾ Projects                  │
│   ▸ 2026                    │  pending: dashed folder glyph, no chevron yet
│   📄 roadmap                │  note (lontar mark)
│   📎 contract  ●             │  asset + sidecar fold (brass dot)
│   🖼 diagram.png             │  asset, no sidecar
│ ▾ Inbox                     │
│   📄 2026-09-24              brass dot = unsaved
└──────────────────────────────┘
```

## Progressive population (FR-VLT-01, ADR-0015)

The tree is never a blank pane waiting for a number to finish counting. It renders in two moments, both observable and both named in the ADR:

1. **First screen (`tree_first_screen`).** The root directory listing arrives and paints. Every row in the viewport is interactive immediately — clickable, focusable, keyboard-navigable — even though most of the vault has not been listed yet. Folders not yet listed render in the **pending state**: folder glyph with a 1px dashed `border-hairline` outline, no chevron (nothing to expand yet), name in `text-secondary`. They are not disabled — clicking one still selects it and shows "Listing…" in the context pane — they simply have no children to show.
2. **Complete (`tree_complete`).** Directory listings arrive breadth-first and replace the pending state directory by directory. A directory's pending glyph resolves to a normal folder (chevron, solid glyph, full-strength text) the instant its own listing lands, independent of its siblings — the user never waits for the *whole* tree to see any *one* folder resolve.

Because a listing is the complete, final-order contents of a directory (never partial), there is no "half-populated folder" state to design for — a folder is either pending (nothing yet) or fully listed.

**A truly empty folder is not the same thing as a pending one, and the tree must not conflate them.** Per the PRD's FR-VLT-01 wording, an empty directory arrives as an ordinary `TreeUpsert`-shaped listing once the verifying walk actually reaches it — after `tree_complete`, on the same live-update channel a create/delete uses, not as part of the two initial-population moments above. Until that listing arrives, a folder with genuinely nothing in it is indistinguishable from one whose contents just haven't been walked yet, and the UI does not pretend otherwise: both render in the **pending state** (dashed glyph, no chevron) until the folder's own listing — empty or not — lands. The distinction only becomes visible the moment it becomes knowable: a folder that resolves to an empty listing gets a solid glyph with **no chevron and no expand affordance** (there's nothing to expand into), versus a folder that resolves to a non-empty listing, which gets the normal chevron. Both are equally "resolved," and the UI never shows a folder as conclusively empty before the walk has actually confirmed it.

No progress bar, no percentage, no spinner over the pane — index/build state belongs on the status strip only, per architecture.md §9.1 ("extraction/index state is shown on the file, not a global progress bar"), and the tree's own progressive-paint behavior *is* its progress indicator.

### Live updates (after initial population)
- **Create**: new row inserted at the exact sorted position the backend names (`after` sibling) — no client-side re-sort, no flash of the whole folder.
- **Delete**: row removed in place.
- **Rename**: remove + insert, both animated as a single 120ms cross-fade rather than two separate flashes.
- **Content change (a save)**: **nothing happens to the tree.** No highlight, no re-sort, no flicker — this is a deliberate design guarantee (ADR-0015: "a save in a 5,000-entry folder costs the tree zero messages"), and the spec calls it out so an implementer doesn't add a "recently modified" highlight that would reintroduce the cost the ADR removed.

## Attachments (FR-VLT-08)

Non-Markdown files are listed exactly like notes, with a format-specific glyph (PDF, image, generic-file for anything else) and no special section — "attachments" are not a separate area of the tree, because segregating them would make the tree lie about where a file actually lives. An asset row shows no unsaved dot (it has no open-buffer concept) but does show the sidecar fold dot when paired (below).

### Insert link to attachment
- **From the tree**: right-click, or `⇧F10`/Menu key for the keyboard-reachable context menu (see Keyboard map), or the palette/`⌘⇧L` command → `Insert link` — writes the shortest resolving form at the editor's cursor: `![[name.png]]` for an image or PDF, `[[name.ext]]` otherwise (architecture.md §7.4). Disabled (not hidden — greyed with a tooltip) when no note is focused in the editor.
- **Drag**: dragging a tree row onto the editor inserts the same link form at the drop point. A drop target line (2px `accent-interactive`) shows the insertion point in the prose while dragging.
- Feedback: a brief inline confirmation is unnecessary — the inserted link appearing in the editor *is* the confirmation (writing guidance: don't narrate what the user can already see happen).

## Paired presentation — sidecar folding (FR-BIN-03)

`contract.pdf` and `contract.pdf.md` fold into **one row**: `contract.pdf`'s asset glyph, with a small brass dot at its lower-right corner meaning "this entry carries a metadata note." The sidecar itself never appears as its own row (ADR §7.4: "and no entry for `x.pdf.md`"). It remains reachable — through the switcher, search, and backlinks — because it is still an ordinary note everywhere except this one list.

- Opening the folded row opens the binary viewer (`binary-viewers.md`) with the sidecar's frontmatter available as the viewer's metadata panel (type, tags, custom fields) — not opened as a second buffer.
- A small affordance in that metadata panel, `Edit metadata note`, opens the sidecar itself in the editor when the user actually wants to touch its Markdown directly (e.g. to add a field the metadata panel doesn't surface yet) — an escape hatch, not the default path.
- Deleting the sidecar turns the row back into a plain asset (dot disappears); deleting the asset turns the sidecar back into a plain note row. Both are single-entry tree updates (ADR §7.4), so this transition is instant and not a full-folder refresh.

## States

| State | Presentation |
|---|---|
| Pending directory | Dashed folder glyph, no chevron, `text-secondary` name. |
| Listed, collapsed | Solid folder glyph, chevron pointing right. |
| Listed, expanded | Chevron pointing down, children indented 16px. |
| Selected (has focus ring or is the open note) | 2px `accent-interactive` left rail; row background unchanged (keeps text legible — see components.md). |
| Note open in editor, unsaved | Brass dot replaces the note glyph. |
| Asset + sidecar | Asset glyph + brass corner dot. |
| Dragging | Row 60% opacity; drop-target line shown at valid targets. |
| Filtered (tree filter box) | Non-matching rows hidden; matched substrings bolded in `accent-interactive`, same convention as the switcher/palette (components.md §5). |
| Confirmed empty folder (its own listing has arrived and is empty) | Solid folder glyph, **no chevron** (nothing to expand), name at full `text-primary` strength — visually distinct from the pending state's dashed glyph and secondary-weight name, even though the row shows nothing underneath in both cases. Expanding it (there's nothing to expand, so this is a no-op) is simply not offered rather than offered and empty. |
| Empty vault (root listing itself is empty) | One line, `text-secondary`: "Nothing here yet — press ⌘N to start a note." (an invitation, per the skill's guidance on empty states, not a bare "No files.") This is the root's own confirmed-empty state, shown only once the root listing has actually arrived — never shown while the root is still pending. |

## Keyboard map

| Action | Binding |
|---|---|
| Focus tree pane | `⌘` `1` |
| Move selection | `↑` `↓` |
| Expand / collapse | `→` `←` (also toggles on a note row: `→` opens it into the editor without moving focus) |
| Open selected | `↵` |
| Open in new tab | `⌘` `↵` |
| New note in current folder | `⌘` `N` |
| New folder | `⌘` `⇧` `N` |
| Rename | `F2`, on all platforms — **not** `↵`, which is already Open above; a tree row has exactly one meaning per key, and giving Enter a second, state-dependent meaning ("open, unless it was already selected, in which case rename") is exactly the kind of implicit mode-switch this system avoids elsewhere (e.g. the editor's reveal rule follows the selection instead of a toggle for the same reason). |
| Insert link into active editor tab | `⌘` `⇧` `L` — a real, registered command-palette entry (`Insert link to selected file`), rebindable; also reachable via the row's context menu and by dragging the row into the editor (see *Insert link to attachment* above). |
| Context menu | `⇧` `F10` / Menu key, or right-click |
| Filter tree | `⌘` `F`, **local to the tree pane** (active only when the tree has focus) — focuses the tree's own filter box. This is deliberately a different chord from the search panel's `⌘⇧F` (a global binding, always available, opens vault-wide search) so the two never compete for the same keypress; see the master table in `components.md` §7. |

This table is a subset of the single authoritative binding set in `components.md` §7.

## Responsive rules

- Below `breakpoint-compact` (720px), the tree becomes a flyover panel (toggled by a tree icon in the title bar) rather than a docked pane, and closes automatically after a selection is opened, returning focus to the editor.
- The tree pane never scrolls horizontally: long names truncate with an ellipsis and reveal the full name in a tooltip and in the row's `title` attribute.

## Accessibility

- The tree is a native `role="tree"` structure: `aria-expanded`, `aria-level`, `aria-selected` on every row, so assistive tech gets real hierarchy regardless of the visual indentation scheme.
- Pending directories announce as "Projects, folder, contents loading" rather than being silently non-interactive; a confirmed-empty directory announces as "Archive, folder, empty" once its own listing has actually landed — the two are never conflated for assistive tech any more than they are visually.
- The sidecar fold is announced as "contract, PDF, has metadata note" — the folding is a presentation choice, not a loss of information for screen-reader users, who otherwise couldn't see the brass dot at all.
