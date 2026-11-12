# Component inventory — P1

Owner: frontend-designer. Tokens referenced below are from `tokens.json`. Every component that carries text states which type family it uses, because that choice is not incidental in this system (see tokens.json `meta.principles`).

Cited requirement IDs are the ones a component exists to satisfy; screen specs in `screens/` carry the full FR/US list per screen.

---

## 1. App shell

**Title bar** (`layout.titlebar-height`, `typography.ui`)
- **P1 scope: vault name only.** architecture.md §9 places a lens switcher and a proposal-count badge in the title bar, but lenses beyond `Notes` are P3 and the AI review queue is P2 — both are **absent from every P1 screen in this set**, including this title bar. Left: window controls (platform-native). Centre/right: vault name, and the `.serat/` gitignore offer when eligible (`gitignore-offer.md`, docked to the tree, not the title bar itself). No default keybinding in the `⌘1..9` range is claimed for a lens switcher in P1 — that range is fully available and is spent instead on pane focus, below.
- When a lens switcher ships (P3), it will need its own keybinding scheme designed against whatever P1/P2 have claimed by then; this file does not reserve one in advance.

**Three-pane layout**: file tree (`layout.tree-pane-width-*`) | editor | context pane (`layout.context-pane-width-*`). Panes are separated by a 1px hairline (`color.role.border-hairline`), never a shadow. Each pane is independently collapsible (`Cmd/Ctrl+B` tree, `Cmd/Ctrl+.` context pane). Below `layout.breakpoint-narrow` the context pane collapses to a flyover triggered by its own toolbar icon; below `layout.breakpoint-compact` the tree does the same and the editor becomes the only persistent pane (see each screen's Responsive section).

**Status strip** (bottom of window, `typography.small`, mono for the hash/position fields): cursor `line:col` (mono), word count, index state (`Opening…`, with its reason when it has one / `Ready` / `Rebuilding…` / `Unavailable`; copy for `Opening` in `index-state-copy.md`, task #115 — never a blocking spinner over content, per architecture.md §9.1 "extraction/index state is shown on the file, not a global bar"), save state.

---

## 2. Navigation

**Tree row** (`layout.row-height-tree`, `typography.ui`)
- States: default, hover (`bg-surface-sunken`), selected (`bg-selected`, left 2px indigo rail instead of a full highlight box — keeps the row's text legible and avoids the "everything is a rounded card" tell), open-in-editor (bold weight, not colour, so colour stays reserved for interaction), pending (row painted with a 1px dashed `border-hairline` folder icon and no chevron until its listing lands — ADR-0015 §4), dragging (opacity 0.6, drop-target line in `accent-interactive`).
- Kind glyphs: folder (chevron + closed/open folder), note (the two-strip lontar mark reduction, 16px), asset (format-specific glyph: PDF, image, generic), **paired asset+sidecar** (asset glyph with a small brass dot at 4 o'clock — the one place a non-error, non-unsaved use of brass appears, meaning "this entry carries a metadata note folded into it").
- Unsaved indicator: a brass dot to the left of the note name, replacing the kind glyph's position only while dirty (goal: one signal, not a glyph plus a dot).
- Depth: 16px indentation per level, no vertical guide rules (rules read as a spreadsheet; indentation alone is legible at ≤ 6 levels, which covers the benchmark vault's shape).

**Quick switcher** and **Command palette**: see their own screen specs. Both are the same overlay chrome — see §5 below.

**Backlinks pane row**: see `backlinks-pane.md`.

---

## 3. Editor

**Editor surface**: CodeMirror 6 host. Prose measure capped at `typography.measure.prose-max` (72ch), centred in the pane with generous side margins reclaimed as reading margin, not decoration — this is the single place in the shell where whitespace is the point.
- Rendered (live-preview) text: `typography.body-serif`, in `color.role.text-primary`, `prose` family.
- Revealed raw syntax (the line under the cursor/selection): `typography.mono-source`, in `color.role.text-secondary` for the marker characters (`**`, `#`, `[[`, `]]`) and `text-primary` for the content between them — so the eye can tell "structure" from "content" without colour-coding every token.
- Headings render at `heading-sm` / `heading-md` / `heading-lg` in the `ui` family (Bricolage), deliberately breaking from the serif body: a heading is the app's structure imposed on the prose, not more prose.
- Inline code / fenced code blocks: `mono-source`, on `bg-surface-sunken`, no border — a filled band reads as "this is data" without adding a rule.
- Wikilinks (resolved): `color.role.text-link`, no underline at rest, underline on hover/focus. Unresolved: `color.role.unresolved-link` (rust) with a dotted underline — colour alone is not the only signal (colour-blindness; NFR-USE-02 is P5 but the habit starts now).
- Block reference / inline field markers (`^id`, `key:: value`): `mono-source`, `text-tertiary`, always visible (never hidden by the reveal rule — they are structurally load-bearing, not typographic decoration).

**Split-preview pane**: read-only render of the same document, `body-serif`, in a sibling pane separated by a hairline. See `editor.md`.

**Conflict notice**: see `external-change-notice.md`. Uses the "overlay" elevation token but is docked, not centred — it never blocks the buffer underneath (architecture.md §9.1: no modal on the write path).

---

## 4. Binary viewers

**PDF viewer chrome**: toolbar (`ui`, `small` icons) with page field (mono, editable, `Enter` jumps), page count, zoom, fit-width/fit-page toggle. Canvas area on `bg-surface-sunken`. See `binary-viewers.md`.

**Image viewer chrome**: toolbar with zoom percentage (mono), fit/100%/zoom-to-selection. Checkerboard backdrop (`neutral.200`/`neutral.100` 8px check) behind transparent images — the one place a textured background is allowed, because it is information (this image has alpha), not decoration.

**Rendering rule for every image, everywhere it appears** (the image viewer, an editor embed, a split-preview render, a folded tree-row thumbnail if one ever exists): a plain `<img src="serat-asset://…">`, and nothing else — never `<object>`, `<iframe>`, inline SVG markup, or a sanitised HTML string inserted via `v-html`/`innerHTML`. This is one rule stated once here rather than re-derived per screen; `editor.md` and `binary-viewers.md` both point back to it. It holds for every raster and vector format alike, including SVG, which is otherwise the one image format capable of carrying script — `<img>` never executes an SVG's scripts, `<object>`/`<iframe>` can.

---

## 5. Overlays (palette, switcher, dialogs, tooltips)

Shared chrome for `command-palette.md` and `quick-switcher.md`:
- Card: `bg-overlay`, `radius.md`, `elevation.overlay`, 1px `border-hairline`. Centred horizontally, upper-third vertically (never true centre — true centre reads as a modal that owns the whole window; upper-third reads as a tool you summon and dismiss).
- Input row: `typography.input`, `ui` family, no visible border once the card itself has the hairline — the input is the card, not a box inside it.
- Result rows: `typography.ui`, icon + label + right-aligned metadata (mono, `small`/`micro`) + keybinding chip where relevant.
- Matched characters: `color.role.accent-interactive`, `weight 600` — never a background highlight (a highlighted substring inside a highlighted row is the generic-AI tell; bolding the matched letters in the accent colour is legible and quieter).
- Empty/no-match state: one line, `text-secondary`, states what to do next (see writing guidance — never "No results found." alone).
- Keyboard: `↑↓` move, `Enter` accept, `Esc` dismiss, all overlays close on focus loss to a non-overlay element.

**Keybinding chip**: `mono`, `micro`/`small`, `bg-surface-sunken`, `radius.chip`, 1px `border-hairline`, one glyph per physical key (`⌘` `⇧` `⌥` `⌃` `↵` `⎋` `⇥` on macOS; `Ctrl` `Shift` `Alt` `Enter` `Esc` `Tab` spelled out on Windows/Linux, since those platforms have no single-glyph convention) joined with no separator character (not `Cmd+K`, but two adjacent chips `⌘` `K` — a chip pair reads as "these two keys", a plus sign reads as arithmetic).

**Non-modal notice** (conflict notice, gitignore offer): docked card, `elevation.overlay`, anchored to the pane it concerns (editor for conflicts, tree for the gitignore offer) rather than to window centre. Dismissible by `Esc` **when the card itself has focus** — but `Esc` on a notice with unresolved state (the conflict notice) parks it rather than discarding the choice, and does not compete with `Esc` bindings already owned by the content the notice sits over (CodeMirror's own `Esc` handling keeps working while the editor has focus and the notice is simply showing) — see `external-change-notice.md` and §7's conflict-notice keybinding scope note.

---

## 6. Buttons, inputs, chips

**Button**: `ui-medium`, `radius.sm`, 1px `border-hairline` by default (secondary), filled `accent-interactive` / `text-on-indigo` for primary, filled `accent-error` for destructive. No pill shape, no drop shadow. Hover darkens the fill/border by one ramp step (`indigo.700`, `rust.600`); no scale or lift transform (motion is reserved for state that actually changed, per the frontend-design skill's motion guidance).

**Text input / search field**: `typography.input`, `bg-surface`, 1px `border-hairline`, `radius.sm`; focus replaces the hairline with `border.focus-ring` (2px indigo) rather than adding a glow — a ring, not a halo.

**Filter chip** (the search panel's single `[key:value]` term): `mono`, `small`, `accent-secondary-bg` fill (brass tint, because a filter is an earned, secondary annotation on a query, not a primary action), but **`text-primary` (ink) for the text itself, not `accent-secondary`** — brass-on-brass-tint measures 2.45:1, well under the 4.5:1 body-text minimum this file's own accessibility baseline sets; ink on the same tint clears it comfortably while the fill still carries the brass association. Brass stays reserved for the chip's border and its close (×) glyph, where it's decorative reinforcement rather than the only way to read the label. A close (×) target ends the chip.

**Status dot**: 6px circle. Brass = unsaved / attention (pending tree entry, sidecar fold). Rust = error/unresolved/refused. Never both meanings on the same token — see tokens.json's `unsaved-dot` vs `unresolved-link` vs `conflict-*` roles, which are named separately even though two of them render brass, so a future change to one cannot silently change the other.

---

---

## 7. Master keybinding table — P1 defaults, conflict-free

Every P1 default binding, in one place, so it can be checked once for the registry's own invariant (architecture.md §9.3: "no two commands share a binding in the same `when` context") rather than re-derived per screen. Each screen's own Keyboard map is a subset of this table; where a screen's table and this one would ever disagree, this one is authoritative. `⌘` is `Ctrl` on Windows/Linux throughout, per the keybinding-chip convention in §5. All bindings are rebindable (FR-NAV-06, `command-palette.md`) — "default" describes what ships, not what's fixed.

### Global (available regardless of which pane has focus)

| Binding | Command | Screen |
|---|---|---|
| `⌘K` | Command palette | `command-palette.md` |
| `⌘O` | Quick switcher | `quick-switcher.md` |
| `⌘S` | Save now | `editor.md` |
| `⌘⇧F` | Open/focus search panel (vault-wide) | `search-panel.md` |
| `⌘B` | Toggle tree pane | §1 above |
| `⌘.` | Toggle context pane | §1 above |
| `⌘1` | Focus tree pane | `editor.md`, `file-tree.md` |
| `⌘2` | Focus editor pane | `editor.md` |
| `⌘3` | Focus context pane (outline/backlinks) | `editor.md`, `backlinks-pane.md` |
| `⌘N` | New note (in the currently-selected tree folder, or vault root) | `file-tree.md` |
| `⌘⇧N` | New folder | `file-tree.md` |
| `⌘⇧M` | Toggle live preview / split mode | `editor.md` |
| `⌘⇧L` | Insert link to selected file (tree-selection-scoped; greyed in the palette when nothing's selected, per the "unavailable in context" convention in §5/`command-palette.md`) | `file-tree.md` |
| `⌘⇧T` | *Reserved*: reopen closed tab (standard editor convention; not one of the nine screens specified here, named so nothing else claims the chord) | — |

### Local to the tree pane (`when` tree-focused)

| Binding | Command |
|---|---|
| `↑` `↓` | Move selection |
| `→` `←` | Expand / collapse (or open a note without moving focus) |
| `↵` | Open selected |
| `⌘↵` | Open in new tab |
| `F2` | Rename (all platforms — see `file-tree.md` for why `↵` is not overloaded to also mean rename) |
| `⇧F10` / Menu key | Context menu |
| `⌘F` | Filter tree (**local** — distinct from the global `⌘⇧F` vault search above; the two can never fire for the same keypress because only one `when` context is active at a time) |

### Local to the backlinks list (`when` backlinks-focused)

| Binding | Command |
|---|---|
| `↑` `↓` | Move through entries |
| `↵` | Open focused entry |
| `⌘↵` | Open focused entry in new tab |

### Local to an overlay — quick switcher / command palette (`when` that overlay is open)

| Binding | Command |
|---|---|
| `↑` `↓` | Move selection |
| `↵` | Open / run selected |
| `⌘↵` | Open in new tab (switcher only — the palette has no "new tab" concept) |
| `Esc` | Dismiss |
| `⌘R` | Start rebinding the selected row (palette only) |

### Local to the editor (`when` editor-focused)

| Binding | Command |
|---|---|
| `⌘↵` | Follow link/URL under cursor — wikilink resolves in-app; `http(s)://` opens in the system browser (`editor.md`) |
| `[` `[` + 2 chars | Opens the switcher inline, in link mode |

### Local to the search panel (`when` search-panel-focused)

| Binding | Command |
|---|---|
| `↵` | Run search (also auto-runs 250ms after the last keystroke) |
| `↓` `↑` | Move through results |
| `↵` (on a result) | Open focused result |
| `⌘↵` (on a result) | Open in new tab |
| `⌘↵` (on the "Load more" row) | Load next page |
| `Backspace` (cursor right after the filter chip) | Remove the active `[key:value]` chip |
| `Esc` | Close panel |

### Local to the PDF viewer (`when` PDF-viewer-focused)

| Binding | Command |
|---|---|
| `→` `←` (also `PageDown`/`PageUp`) | Next / previous page |
| `⌘G` | Jump to page field |
| `⌘+` `⌘-` | Zoom in / out |
| `⌘0` | Toggle fit width / fit page |
| `⌘F` | pdf.js's own built-in find bar — inherited, not registered in Serat's command registry, and scoped to this `when` context so it never collides with the tree's `⌘F` above |

### Local to the image viewer (`when` image-viewer-focused)

| Binding | Command |
|---|---|
| `⌘+` `⌘-` | Zoom in / out |
| `⌘0` | Toggle fit / actual size |
| `⌘⇧0` | Reset pan to centre |

### Local to a pending external-change conflict

`⌘⇧K`/`⌘⇧O`/`⌘⇧D` (below) are scoped `when` that note's conflict notice is active, same as any other contextual binding. **`Esc` is scoped more narrowly still — `when` the notice card itself holds focus, not merely "a conflict is pending for this note".** The editor buffer under the notice stays fully focusable and editable while a conflict is pending (per the notice's own Layout section), and CodeMirror binds `Esc` for its own purposes there (closing autocomplete, dropping a multi-cursor selection, exiting search-in-note, etc.) — those must keep working normally while the user is typing. So: `Esc` while the **editor/buffer** has focus runs CodeMirror's own `Esc` handler, untouched; `Esc` only minimises the conflict notice when focus is **on the notice itself** (its buttons, or the notice landed focus when it first appeared and hasn't been moved away — see `external-change-notice.md`'s "focus **not** stolen" note, which means in practice the user has to `Tab` to the notice, or click it, before `Esc` there means "minimise" rather than "whatever CodeMirror does"). The minimised tab indicator is a third, separate focus target with its own `↵`-to-reopen binding (`external-change-notice.md`), not reachable via a global `Esc`.

| Binding | Command |
|---|---|
| `⌘⇧K` | Keep mine |
| `⌘⇧O` | Take theirs (**not** `⌘⇧T`, reserved above for reopen-closed-tab) |
| `⌘⇧D` | Open compare |
| `Esc` (**only** when the notice card has focus) | Minimise the notice (does not resolve the conflict) |

### Local to the `.serat/` gitignore offer (`when` that card has focus)

| Binding | Command |
|---|---|
| `↵` | Add to .gitignore (the card's default button) |
| `Esc` | Not now |

---

## Accessibility baseline (applies to every component above)

- Every interactive element reachable by `Tab`/`Shift+Tab` in visual order; overlays trap focus while open and return it to the invoking control on close.
- Visible focus ring (`border.focus-ring`) on every focusable element — never `outline: none` without a replacement.
- Colour is never the only carrier of a state: unresolved links get a dotted underline, conflicts get an icon and a label, errors get text, not just rust fill.
- Hit targets ≥ 24×24px even where the visual glyph is 16px (tree row kind icons, chip close buttons).
- `prefers-reduced-motion` collapses overlay open/close to opacity-only per `tokens.json` `motion.reduced-motion`.
- All body/prose text meets 4.5:1 contrast against `bg-app`/`bg-surface`; `text-tertiary` is reserved for metadata never required for comprehension (word counts, timestamps).
