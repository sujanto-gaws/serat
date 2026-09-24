# Screen: Editor — hybrid live preview and split-preview fallback

Serves: FR-EDT-01, FR-EDT-02, FR-EDT-03, FR-EDT-10, FR-NAV-06, NFR-PRF-02, NFR-CMP-01 · US-02, US-03, US-08
Design basis: architecture.md §9.3, ADR-0006 (CodeMirror 6 decorations), ADR-0016 (patch saves, coordinates), §3.3 (reconciliation)

## Layout

```
┌ title bar (vault name — lens switcher and proposal count are P2/P3, absent in P1) ┐
├ tree │ ─────────────── editor pane ─────────────────── │ context pane ┤
│      │  [tab strip: open notes, unsaved = brass dot]    │  outline /   │
│      │  ┌──────────────────────────────────────────┐    │  backlinks / │
│      │  │        72ch measure, centred              │    │  (P2: props) │
│      │  │        Source Serif 4 body                 │    │              │
│      │  │        headings in Bricolage Grotesque      │    │              │
│      │  └──────────────────────────────────────────┘    │              │
├──────┴──────────────────────────────────────────────────┴──────────────┤
│ status strip: line:col (mono) · words · index state · save state        │
└───────────────────────────────────────────────────────────────────────┘
```

Two editor modes, one document, one command registry (architecture.md §9.3: "editing capability is identical in both modes, because it is the same editor").

## Mode 1 — Hybrid live preview (default, `editor.live_preview = true`)

- Formatting renders inline: `**bold**` shows as **bold** text with the `**` markers hidden, `# Heading` shows as a styled heading with the `#` hidden, wikilinks render as styled link text, tables render as a `StateField` block widget.
- **Reveal rule** (FR-EDT-02): a line is shown "raw" — markers visible and editable, `mono-source` styling — only while the selection intersects it. Leaving the line hides the markers again **within one frame** (the acceptance criterion, literally: architecture.md §9.3's frame-budget discipline applies here too — no debounce on reveal/hide).
- Cursor entering a bold span: `**` fades in over `motion.duration.micro` (120ms) — a state change the user caused, so a short transition is earned (frontend-design skill: "motion that answers a person's action is welcome").
- Block widgets (tables, embeds, callouts) are inert until clicked/selected; clicking one line inside a table reveals that line's raw `|` syntax the same way, the rest of the table stays rendered.
- **Image embeds** (`![[image.png]]`, `![[image.png|300]]`) render as a plain `<img>` element whose `src` is the `serat-asset://` URL for that path (ADR-0017), and nothing else: never `<object>`, `<iframe>`, inline SVG markup, or the sanitised HTML string inserted via `v-html`/`innerHTML`. This applies identically to the split-preview pane and to any embed nested inside another note's render — see components.md §3/§4 for the rendering rule stated once, generally, so it doesn't need re-deriving per surface.
- Obsidian syntax Serat doesn't render (Dataview blocks, plugin fences) shows as plain `mono-source` text with a thin left rule in `text-tertiary` and a small inline label "not rendered by Serat" the first time it's seen per session (dismissible), never rewritten (C-01).
- **External `http(s)://` links** render the same as a resolved wikilink visually (`text-link`, underline on hover/focus) but never navigate the app window on click, hover or hover-adjacent movement. Hovering shows the full URL in a tooltip (so the destination is known before committing to it — the same reasoning as a normal browser's status-bar preview). Only an explicit click, or `↵`/`Space` when the link has keyboard focus, opens it — and it opens in the **system default browser**, via the shell-open command, never inside the webview. Nothing about an external link is prefetched, resolved or fetched by Serat at render time (NFR-SEC-01: the app makes no network request the user didn't directly cause) — this holds identically in live preview, split preview and inside embeds.

### States
| State | Presentation |
|---|---|
| Clean, focused | As above. Status strip save state: `Saved`. |
| Dirty (unsaved change) | Tab dot turns brass. Status strip: `Unsaved` in `text-secondary`, no colour alarm — this is the normal state of writing, not a warning. |
| Saving (debounced, async, off the keystroke path per §9.3) | Status strip: `Saving…` for the ~200ms round trip; no spinner over the buffer, no blocking. |
| Save refused (`StaleHash`) | See the dedicated subsection below — this row is intentionally short because the sequencing has enough rules (ADR-0016 §2) to need its own explanation, not a one-line status-table cell. |
| Link unresolved | Rust-coloured text with dotted underline (components.md §3); hovering shows a tooltip "No note named “X” — click to create" (Enter on it, when focused via keyboard nav of links, offers the same). |
| Embed loading / embed depth-2 cycle | A cycle renders as a plain link, per architecture.md §9.3, with a small tooltip "embedded here would repeat itself." |
| Sidecar note open | A sidecar (`x.pdf.md`) is an **ordinary, fully editable note** — nothing distinguishes its editor state from any other note's (US-18/FR-BIN-02 treat it as real frontmatter someone edits directly, and the file-tree's "Edit metadata note" escape hatch would be pointless if this view refused input). There is no "read-only" state modelled here for it. |
| Genuinely non-writable content (e.g. a future read-only view, out of scope for these nine screens) | Where a view truly cannot accept edits, it **refuses input at the point of typing** — keystrokes simply don't insert, with a one-time tooltip explaining why on the first attempt — rather than accepting text into a buffer whose Save button is then disabled. Accepting typing that can never be saved is a worse lie than not accepting it. |

### Save refused (`StaleHash`) — the sequencing, restated from ADR-0016 §2, and why it can't hang

A `StaleHash` refusal is never followed by a blind retry or an indefinite "waiting" state. ADR-0016 §2 gives the front end exactly four rules for ordering a save against an `ExternalChange`, and the UI is built to make all four observable rather than silent:

1. **An `ExternalChange` is applied only when its `base_hash` equals the buffer's current base hash.** One that doesn't match — because something else has since happened to the buffer or the file — is not applied.
2. **While a save is in flight, an arriving `ExternalChange` is held, not applied.** When the save resolves, the held change is applied if its `base_hash` now matches the (possibly just-updated) base, and dropped otherwise. A save is therefore never overtaken by a reconciliation mid-flight.
3. **Any other mismatch drops the change and calls `request_external_diff(path, base_hash)`** — the front end asks explicitly for a fresh diff against its current base rather than waiting for a watcher event that may not arrive in the shape it needs.
4. **After a `StaleHash` refusal specifically, the front end requests the diff (rule 3) unless the explaining `ExternalChange` was already applied** (i.e. rule 1 already resolved it via the watcher's own event, which can arrive first). Either way, something is always actively pursued — there is no path where the front end is simply idle, waiting on nothing, after a refusal.

**What this looks like on screen**, because "the front end requests a diff" is not itself a user-visible state:

| Moment | Status strip / editor presentation |
|---|---|
| The instant a save is refused | Save state reads **`Unsaved`** — the same token as ordinary dirty-buffer text (`text-secondary`, no alarm colour). It never reads `Saving…` (the save has already resolved, as a refusal) and never `Saved` (nothing was written). This is deliberately the *least* alarming truthful state: a refused save is not yet known to be a conflict, only known to be not-yet-saved. |
| The diff arrives and resolves to a clean-buffer or dirty-disjoint outcome | Same as an ordinary external change: **`Updated externally`** for 3s in the status strip, then back to the normal save state (`external-change-notice.md`'s outcome table). This is the common case and resolves within one round trip, typically well under a second. |
| The diff arrives and resolves to an overlap | The full conflict notice appears (`external-change-notice.md`) — this *is* the visible resolution of the refusal; nothing about the refusal itself needed a separate error state, because the notice explains it. |
| **`request_external_diff` returns nothing (disk already equals the current base) *and* the save still can't be assumed safe to blindly retry, or the request fails outright** (IPC error, note closed mid-flight, etc.) | A **persistent** status-strip line, `text-secondary`, replacing the ordinary save state until acknowledged: **"Not saved — file changed on disk"**, with a **Compare** action (opens the same read-only diff view `external-change-notice.md`'s Compare uses) so the user can see what's actually different before deciding what to do next. This is the one case that doesn't resolve itself in a round trip, and it is the state that keeps the flow from ever going quiet: if nothing else explains the refusal, this line says so instead of the UI simply sitting at `Unsaved` forever with no indication anything is wrong. |

No state in this sequence is a spinner or a toast that disappears before it's read — the `Unsaved` state persists until something resolves it, and the fallback line persists until the user acts on it (Compare, or a subsequent successful save).

- Source pane (left of the split, or full width in narrow layout): plain syntax-highlighted Markdown, `mono-source` throughout, no reveal/hide — nothing is ever hidden in this mode, which is the point of the fallback.
- Preview pane (right): the same `markdown-it`-rendered, sanitised output as embeds use (architecture.md §9.3, "the preview renderer, and what it may not do"), `body-serif`, read-only, scroll-synced via `data-line`.
- The two panes are separated by a draggable hairline divider (min 30% / max 70% split), remembered per window, not per note.
- Preview re-renders 150ms after the last keystroke, scheduled in idle time (§9.3) — this is deliberately *not* instant, and the spec calls that out so a reviewer doesn't read the 150ms lag as a bug.
- Switching between modes is instant (no reflow delay) and is itself a palette command (`Toggle live preview`) with a default binding — see `command-palette.md`. It is also exposed as a toolbar toggle icon at the top-right of the editor pane, because US-08's whole premise is "available when hybrid preview misbehaves," which argues for a control that doesn't depend on the palette working.

### States (split mode)
| State | Presentation |
|---|---|
| Normal | As above, both panes live. |
| Preview render fails (malformed sanitiser input, degenerate document) | Preview pane shows a plain-language message ("This note couldn't be rendered — the source is unaffected") in place of the broken render; source pane is completely unaffected and remains fully editable, which is the whole reason FR-EDT-10 exists. |
| Very large note (5,000+ lines) | Preview chunks its render (§9.3); a thin brass progress rule at the top of the preview pane only, never blocking the source pane. |

## Keyboard map (both modes)

| Action | Default binding | Notes |
|---|---|---|
| Toggle live preview / split | `⌘` `⇧` `M` (Win/Linux: `Ctrl+Shift+M`) | Registered command, rebindable (FR-NAV-06). |
| Save now | `⌘` `S` | No-op if nothing dirty; otherwise triggers the same patch-save path the idle debounce uses (ADR-0016) — the command doesn't skip the hash check. |
| Insert link | `[` `[` then type | Opens switcher in link mode inline (§9.2/9.3); `Esc` cancels and leaves the typed `[[` as literal text. |
| Follow link under cursor | `⌘` `↵` (Enter) | For a wikilink: opens the resolved target, or offers to create it if unresolved. For an external `http(s)://` link: opens it in the system browser (see Mode 1's external-link bullet above) — never inside the app window. |
| Reveal/hide is not a keybinding | — | It follows the selection, by design (FR-EDT-02); there is no manual toggle, because a manual toggle would fight the automatic rule on the same line. |
| Move focus tree → editor → context pane | `⌘` `1` / `⌘` `2` / `⌘` `3` | Consistent with the palette-registered pane-focus commands. |
| Command palette | `⌘` `K` | See `command-palette.md`. |
| Quick switcher | `⌘` `O` | See `quick-switcher.md`. |

This table is a subset of the single authoritative binding set in `components.md` §7, which resolves every chord used across all nine P1 screens against every other.

## Responsive rules

- ≥ `breakpoint-narrow` (960px): three-pane layout as drawn.
- `breakpoint-compact`–`breakpoint-narrow` (720–960px): context pane collapses to a flyover (icon in the editor toolbar reopens it over the editor, not beside it); tree stays docked.
- < `breakpoint-compact` (720px): tree also collapses to a flyover; editor is the only persistent pane; split-preview mode stacks the two panes vertically instead of side-by-side (dragging a horizontal divider instead of vertical) because a sub-720px window has no width to spare for two side-by-side prose columns.
- Prose measure (72ch) never exceeds the available width; below it the measure shrinks with the pane rather than the type shrinking, down to `measure.prose-min-comfortable` (52ch), below which the app relies on the flyover behaviour above rather than compressing further.

## Accessibility

- The reveal rule is also keyboard-driven: moving the text cursor (not just the mouse) into a span reveals it, so a keyboard-only user gets the same editing surface a mouse user does — this is not a "hover to reveal" pattern, which would exclude keyboard use entirely.
- Rendered headings carry real heading semantics (`role="heading"` + `aria-level`) mirrored from the Markdown level, so the note's structure is available to a screen reader even though visual styling comes from the `ui` family rather than semantic HTML heading tags (CodeMirror's DOM is not semantic HTML by default).
- Split-preview's preview pane is `aria-live="off"` by default (a 150ms-debounced re-render firing a live region on every keystroke would be constant noise) with a manual "read preview" action for screen-reader users, registered in the palette.
- Focus never silently leaves the editor: opening the switcher, palette or a notice is an explicit focus move with a visible return path (`Esc`).
