# Screen: Editor — hybrid live preview and split-preview fallback

Serves: FR-EDT-01, FR-EDT-02, FR-EDT-03, FR-EDT-10, FR-NAV-06, NFR-PRF-02, NFR-CMP-01 · US-02, US-03, US-08
Design basis: architecture.md §9.3 (P1-05 branch), ADR-0006 (CodeMirror 6 decorations), ADR-0016 (patch saves, coordinates), §3.3 (reconciliation)

## Layout

```
┌ title bar (lens switcher, proposal count) ─────────────────────────────┐
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
- Obsidian syntax Serat doesn't render (Dataview blocks, plugin fences) shows as plain `mono-source` text with a thin left rule in `text-tertiary` and a small inline label "not rendered by Serat" the first time it's seen per session (dismissible), never rewritten (C-01).

### States
| State | Presentation |
|---|---|
| Clean, focused | As above. Status strip save state: `Saved`. |
| Dirty (unsaved change) | Tab dot turns brass. Status strip: `Unsaved` in `text-secondary`, no colour alarm — this is the normal state of writing, not a warning. |
| Saving (debounced, async, off the keystroke path per §9.3) | Status strip: `Saving…` for the ~200ms round trip; no spinner over the buffer, no blocking. |
| Save refused (`StaleHash`) | Never happens silently: this is the reconciliation flow, see `external-change-notice.md`. The editor itself never shows a raw error toast for this — the notice *is* the UI for it. |
| Link unresolved | Rust-coloured text with dotted underline (components.md §3); hovering shows a tooltip "No note named “X” — click to create" (Enter on it, when focused via keyboard nav of links, offers the same). |
| Embed loading / embed depth-2 cycle | A cycle renders as a plain link, per architecture.md §9.3, with a small tooltip "embedded here would repeat itself." |
| Read-only (asset sidecar, or note outside vault write scope) | Editor chrome unchanged; a `text-tertiary` label "Read-only" replaces the save state in the status strip; typing is accepted into the buffer (so paste/format still works) but Save is disabled with a tooltip explaining why. |

## Mode 2 — Split preview (`editor.live_preview = false`, FR-EDT-10, US-08)

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
| Follow link under cursor | `⌘` `↵` (Enter) | Opens the resolved target; on an unresolved link, offers to create it. |
| Reveal/hide is not a keybinding | — | It follows the selection, by design (FR-EDT-02); there is no manual toggle, because a manual toggle would fight the automatic rule on the same line. |
| Move focus tree → editor → context pane | `⌘` `1` / `⌘` `2` / `⌘` `3` | Consistent with the palette-registered pane-focus commands. |
| Command palette | `⌘` `K` | See `command-palette.md`. |
| Quick switcher | `⌘` `O` | See `quick-switcher.md`. |

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
