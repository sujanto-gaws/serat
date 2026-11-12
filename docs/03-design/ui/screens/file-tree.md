# Screen: File tree — progressive population, attachments, sidecar folding, insert-link

Serves: FR-VLT-01, FR-VLT-08, FR-BIN-03, NFR-SEC-06, NFR-USE-01 · US-01, US-18
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

## Names not shown — refused-names count and list (FR-VLT-01, NFR-SEC-06)

A file the vault cannot turn into a vault path (a NUL byte, a `..` segment, a literal `\` in a name, a non-UTF-8 name on non-Windows platforms, and the Windows name rules: `:`, device names, a trailing dot or space) is never silently absent from the tree (design goal 5). The tree cannot show it, so the *report* lives beside the tree rather than in it. `IndexStatus.refused_names` carries the count; the `refused_names` command returns the list (`contracts/ipc.rs`).

### Decision: a status-strip count that opens a panel, not a tree footer

| Option | Verdict |
|---|---|
| **Status-strip count + anchored panel** | **Chosen.** |
| Tree footer | Rejected. The tree pane is collapsible (`⌘B`) and becomes a flyover below `breakpoint-compact`, so a fact kept in its footer would vanish exactly when the pane is hidden. A footer also takes a permanent row from a virtualised list for a condition most vaults never have, and it would put a status fact inside the tree, which *File tree* above keeps free of progress and index state. The strip is always visible, already carries the other vault facts (index state, `Updated externally`), and costs nothing when the count is absent. |

### The count (status strip)

- **Absent, not zero.** `refused_names` of `None` or `0` renders nothing: no "0 names not shown", no empty slot. The strip does not reserve space for it.
- **Wording.** `1 name not shown`, `12 names not shown`, and at the cap `1000+ names not shown`. The old trailing "(invalid on this platform)" is dropped: the reasons vary per name and are in the list, and the clause was wrong for a name refused on any other ground.
- **Control.** A text button, updated immediately when the count changes, `typography.small`, `ui` family, `text-secondary`, led by a 6px `accent-error` status dot (components.md §6: rust = refused). Hover raises the text to `text-primary`; no underline until hover or focus. The dot is a reinforcement: the text carries the meaning, so it is never colour alone.
- **Semantics.** `<button aria-haspopup="dialog" aria-expanded>`; its name is its visible text. At the cap the visible `1000+` is `aria-hidden` and a visually-hidden `1000 or more` stands in, so a screen reader never reads "plus". The button is **not** inside the live region below.
- **Live region.** A separate, always-mounted, visually hidden `role="status"` node (polite, atomic), a sibling of the button, carries the sentence ("3 names not shown") and is empty when the count is absent, so a count that appears, changes or clears is announced. The button text updates at once; only the status node's sentence is coalesced with a 1s trailing debounce, so a walk that raises the count name by name announces once, not per file. This is a deliberate exception to "don't narrate what the user can already see": focus is not on the strip, and the user cannot see a count they are not looking at.
- **Reach.** The button is in the strip's Tab order. Also a palette command, `Show names not shown` (`vault.refused-names`, no default binding, greyed with the reason "Every name can be shown" when the count is zero and "Open a vault first" when it is `None`). It opens the same panel and returns focus the same way. It is not a primary action for NFR-USE-01's purposes (it is a report, not something the user does to the vault), so it does not join that primary-action list; it is still listed and bindable in the palette like every command.

### The list (anchored, non-modal panel)

Opens above the strip at its left edge, width 440px (max 90vw), `max-height` 50vh, `bg-overlay`, 1px `border-hairline`, `radius.md`, `elevation.overlay`, anchored to the button as a non-modal notice (components.md §5). Below `breakpoint-compact` it is a full-width sheet above the strip, `max-height` 60vh. Open/close 160ms (`motion.duration.panel`): opacity and a 4px rise; opacity only under `prefers-reduced-motion`.

```
┌ Names not shown ────────────────────── [×] ┐
│ 12 files in your vault folder can't be      │
│ listed on this computer. Serat leaves them  │
│ as they are.                                │
│ ─────────────────────────────────────────── │
│ notes/meeting: friday.md          (mono)    │
│ Contains a : (mono), which is a drive       │
│ prefix or an NTFS alternate data stream on  │
│ Windows                                     │
│ ─────────────────────────────────────────── │
│ ...                                         │
│ A name stays listed until Serat next scans  │
│ its folder, even if you have since renamed  │
│ or deleted the file.                        │
└─────────────────────────────────────────────┘
```

- **Header.** Title `Names not shown` (`ui-medium`), close button (24x24 hit target). Under it, one sentence, `text-secondary`: `N files in your vault folder can't be listed on this computer. Serat leaves them as they are.` (`1 file ... can't`). N comes from the list length, not the status count.
- **Rows.** Path first, in `mono` `small`, `text-primary`, wrapping at any character (never truncated: the exact name is the point). Reason second, in `ui` `small`, `text-secondary`. The backend writes reasons as lowercase predicates with Markdown backticks (`contains a `:`, which is a drive prefix ...`); the panel presents them by capitalising the first letter and rendering each backticked span as inline `mono` without the backticks (`Contains a :, which is a drive prefix ...`), and changes nothing else. One row per distinct path, in the backend's byte order, separated by a hairline. No icons, no per-row actions: a path in this list failed to become a vault path, so nothing can open or act on it, and it is never passed back to a command (contract: display only).
- **Untrusted text (NFR-SEC-06).** Paths and reasons are rendered as text, never HTML. Control characters and bidi overrides in a path **or in a reason** (reasons interpolate the name) are shown as visible escapes (`⟨U+202E⟩`) so a hostile name cannot reorder its own display or hide its extension. Text is selectable, and what is copied is the display form: escaped, and with U+FFFD where a non-UTF-8 name was decoded lossily. The panel does not promise a byte-exact name to paste into a file manager; the user finds the file by its visible shape.
- **Footer.** One line, `text-secondary`: `A name stays listed until Serat next scans its folder, even if you have since renamed or deleted the file.` (the contract's accepted staleness, said once instead of left to surprise; the log drops entries per scanned subtree).
- **Overflow at the cap (1000).** The strip reads `1000+ names not shown`; the header sentence becomes `1000 or more files in your vault folder can't be listed on this computer. This list shows 1000 of them. Serat leaves them as they are.` The list never claims a total it does not know. At 1000 rows the list is a plain scroll container (no virtualisation; the cap is its bound), with `content-visibility: auto` on rows.
- **Empty state** (the panel is open and the list is empty, whether it opened that way or the count fell to zero while open): one line, `text-secondary`: `Every name in this vault can be shown.` The panel is not closed for the user; it waits for `Esc`.
- **Load failure** (the `refused_names` call errors): `Couldn't load the list. The count in the status strip is still correct.` with a `Retry` secondary button. No retry loop.

### Keyboard and focus

| Action | Behaviour |
|---|---|
| Open | `↵` or `Space` on the strip button, or the palette command. Click toggles. |
| Focus on open | Moves to the panel (`tabindex="-1"` on the dialog) so the title and description are read. `Tab` then reaches `×` (it precedes the list in reading order), then the list container. |
| Scroll the list | The list container is `tabindex="0"` with `role="list"` and an `aria-label` of `Names not shown`; `↑ ↓ PageUp PageDown Home End` scroll natively. Rows are not individually focusable (they have no action). |
| Close | `Esc`, the `×` button, activating the strip button again, opening any overlay (palette, switcher), or focus moving to a non-overlay element (components.md §5 rule). A pointer press outside also closes it, except on the anchor button (which toggles). Window blur does not close it. |
| Focus return | To the element that was focused when the panel opened: the strip button if opened from it, otherwise the palette's opener (the palette already restores it). If that element is gone (the count fell to none and the button unmounted), the chain is the tree's selected row, then the editor; the editor also when the tree is collapsed, not only below `breakpoint-compact`. Focus is never dropped to `<body>`. If focus left the panel on its own (the user pressed `⌘2`), it is not pulled back. |

### Accessibility

- `role="dialog"` with `aria-modal="false"`, `aria-labelledby` the title, `aria-describedby` the header sentence. It is non-modal: no focus trap, no scrim, the rest of the app stays operable, so a user can read a name here and fix it in a file manager without dismissing anything first.
- The panel's own count sentence is not a live region; only the strip's is, so a change is announced once.
- Contrast. Light: `text-primary` and `text-secondary` on `bg-overlay` meet 4.5:1 (components.md baseline); the `accent-error` dot is about 5.4:1 on paper. Dark (P2, NFR-USE-03; no new hue): roles map to an ink ground and paper text. The `accent-error` dot is about 2.9:1 on ink, under the 3:1 non-text minimum, so the dark ramp must supply a lighter `accent-error` before dark ships. Until then nothing depends on the dot, because the text states the fact.

### When the count changes while the list is open

- Each status event whose `refused_names` differs triggers one re-fetch of the list, 500ms trailing debounce. The old list stays on screen until the new one arrives: no spinner, no blank flash.
- The scroll position and, if the user has focus inside the list, that focus are kept. Rows inserted above the viewport do not move what the user is reading (`overflow-anchor: auto`).
- The header sentence updates in place. The strip's live region announces the new count once; the panel stays quiet.
- A count that returns to zero shows the empty state (above). A count that goes absent (`None`, the vault closed) closes the panel with the usual focus return, because the thing it described no longer exists.
- Entries leave the log when a walk covering their subtree replaces it (contract), so the count can change in steps. The 500ms re-fetch debounce still applies; the panel keeps the old list until the new one arrives.
- If the list and the count disagree after a fetch (the log moved between the two reads), the panel trusts its own list for its sentence, the strip keeps the status value, and the next event reconciles them. Neither is shown as an error.

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
| First / last row | `Home` `End` |
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
