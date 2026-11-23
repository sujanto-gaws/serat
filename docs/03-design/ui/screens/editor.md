# Screen: Editor — hybrid live preview and split-preview fallback

Serves: FR-EDT-01, FR-EDT-02, FR-EDT-03, FR-EDT-10, FR-NAV-06, NFR-PRF-02, NFR-CMP-01 · US-02, US-03, US-08, US-35
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
- **Reveal rule** (FR-EDT-02): a line is shown "raw" — markers visible and editable — only while the selection intersects it. Raw means the markers: they take `mono-source` styling in `text-secondary`, and the text between them keeps its rendered face, so the caret never sees a line change typeface under it (Live-preview typography, below). Leaving the line hides the markers again **within one frame** (the acceptance criterion, literally: architecture.md §9.3's frame-budget discipline applies here too — no debounce on reveal/hide).
- Cursor entering a bold span: `**` appears in the same frame, with no fade. This replaces an earlier 120ms fade: FR-EDT-02's criterion is one frame, and CodeMirror recreates the marker spans on edits, so an entry animation would replay on every keystroke typed on the line.
- Block widgets (tables, embeds, callouts) are inert until clicked/selected; clicking one line inside a table reveals that line's raw `|` syntax the same way, the rest of the table stays rendered.
- **Image embeds** (`![[image.png]]`, `![[image.png|300]]`) render as a plain `<img>` element whose `src` is the `serat-asset://` URL for that path (ADR-0017), and nothing else: never `<object>`, `<iframe>`, inline SVG markup, or the sanitised HTML string inserted via `v-html`/`innerHTML`. This applies identically to the split-preview pane and to any embed nested inside another note's render — see components.md §3/§4 for the rendering rule stated once, generally, so it doesn't need re-deriving per surface.
- Obsidian syntax Serat doesn't render (Dataview blocks, plugin fences) shows as plain `mono-source` text with a thin left rule in `text-tertiary` and a small inline label "not rendered by Serat" the first time it's seen per session (dismissible), never rewritten (C-01).
- **External `http(s)://` links** render the same as a resolved wikilink visually (`text-link`, a quiet underline at rest that goes solid on hover/focus, as Live-preview typography specifies) but never navigate the app window on click, hover or hover-adjacent movement. Hovering shows the full URL in a tooltip (so the destination is known before committing to it — the same reasoning as a normal browser's status-bar preview). Only an explicit click, or `↵`/`Space` when the link has keyboard focus, opens it — and it opens in the **system default browser**, via the shell-open command, never inside the webview. Nothing about an external link is prefetched, resolved or fetched by Serat at render time (NFR-SEC-01: the app makes no network request the user didn't directly cause) — this holds identically in live preview, split preview and inside embeds.

### Callouts and the fold marker (#152; FR-EDT-01, FR-EDT-02, NFR-USE-01)

A callout whose first line carries Obsidian's fold marker (`> [!tip]- Title` collapsed, `> [!tip]+ Title` expanded) renders as a title row and a body. Since #146 a collapsed one hides the body, and nothing said so: it read as an empty callout. The fix is a disclosure control on the title row. Callouts with no marker, or with an empty body, are not foldable and show no glyph, no count and no `aria-expanded`.

- **Glyph.** The app has no icon library yet, so the glyph is a token-based CSS chevron, not an imported icon: a 7 px square with a 1.5 px right and bottom border in `currentColor`, turned 45 degrees so it reads as a right-pointing chevron. It sits at the **leading edge** of the title, before the title text, where a file-tree row's chevron sits (`components.md` file tree). It is 16 px wide visually, in `color.role.text-secondary` at rest and `color.role.accent-interactive` on hover and focus. The hit target is 24 x 24 px (the `components.md` accessibility rule) and extends into the title row's leading padding, so the title text does not move.
- **Rotation.** Collapsed points right. Expanded points down (a further 90 degrees clockwise). The turn is a `transform` transition over `motion.duration.micro` (120 ms) with `motion.easing.enter`: it answers the person's click, so it is earned. **Reduced motion:** the duration collapses to 1 ms, so the glyph snaps. The body itself is never animated, because a height transition would run layout inside the frame budget (§9.3); it appears and disappears in the frame of the click.
- **Count.** Collapsed shows a trailing one-line count at the title's right end: `3 lines hidden`, `1 line hidden`, in `typography.small` and `color.role.text-secondary` (not `text-tertiary`, which fails 4.5:1 on `bg-surface`). The number is the body's non-blank source lines. Expanded shows nothing, because the body is the count. The count tells a reader that content exists and roughly how much, which the glyph alone does not.
- **Semantics.** The title row is `role="button"`, `tabindex="0"`, with `aria-expanded="false"` (collapsed) or `"true"` (expanded) and `aria-controls` naming the body's id. Its accessible name is the title text followed by the count when collapsed (for example "Remember to rebase, 3 lines hidden"), so no `aria-label` overrides it. The glyph is `aria-hidden="true"`. The body is `hidden` while collapsed, so a screen reader does not read it.
- **Hover and focus.** Hovering the glyph sets `cursor: pointer` on it and paints its 24 x 24 box in `bg-surface-sunken` with `radius.sm`. Keyboard focus on the title row shows `border.focus-ring` with `border.focus-ring-offset`. The callout's own colours are unchanged and no shadow is added.
- **Interaction ruling.** A click that lands on the glyph's 24 x 24 box **toggles the fold in the rendered preview and does nothing else**: the caret does not move, the source is not revealed, and the widget stays rendered. The widget returns `true` from `ignoreEvent` for events whose target is inside the glyph box (and calls `preventDefault` on its `mousedown`), so CodeMirror does not turn the click into a selection. A click anywhere else on the callout, the title text included, keeps today's behaviour: `ignoreEvent` returns `false`, the caret lands in the block and the raw `> [!tip]- Title` source is revealed (FR-EDT-02). Only the glyph is split off, so each action stays one click: the title text and body remain the pointer route to the source.
- **View state, never bytes.** Toggling changes only the view. It dispatches no document change, so nothing is patched or saved, the unsaved dot does not appear, undo history is untouched, and the `-` / `+` marker stays exactly as written (C-01, C-04). The initial state comes from the marker each time the note is opened. Within a session the toggled state survives re-rendering (the widget is rebuilt on edits) and survives the caret revealing and re-hiding the source; it is held in editor view state keyed by the callout's position in the note, carried through edits and dropped when the callout is deleted; a fresh editor state reads the marker again. It is not written to the file, the index or the vault.
- **Keyboard.** Tab and Shift+Tab move between the title rows of foldable callouts; `Enter` and `Space` on a focused title row toggle it; `Esc` returns focus to the text caret where it was. The palette command **Toggle callout fold** toggles the callout containing the caret (rendered or revealed) with no focus change. It needs no default chord, so the binding set in `components.md` §7 is unchanged; a user can bind one (FR-NAV-06). Reaching the source by keyboard is unchanged: arrowing the caret into the block reveals it.

**Acceptance (one vue-frontend-dev task):**
1. A `-` callout with a non-empty body renders the right-pointing glyph, the title, `N lines hidden`, `aria-expanded="false"` on the title row and a `hidden` body. A `+` callout renders the down-pointing glyph, no count and `aria-expanded="true"`. A callout with no marker, or an empty body, has none of these.
2. Clicking the glyph toggles glyph, count, body and `aria-expanded` together. The document string, dirty state and undo stack are identical before and after (unit-tested), and the caret does not move.
3. Clicking the title text or the body reveals the raw source as before.
4. The toggled state survives an edit elsewhere in the note and revealing then leaving the callout; reopening the note restores the state the marker gives.
5. Tab reaches each foldable title row; `Enter` and `Space` toggle; the focus ring is visible; the palette command exists and toggles.
6. Glyph and count use only the tokens named above; under `prefers-reduced-motion: reduce` the turn takes 1 ms; the hit target is 24 x 24 px.
7. A collapsed title's accessible name is the title plus the count, and the glyph is hidden from assistive technology.
8. The keystroke path is unchanged: no layout measurement is added to a transaction with no callout in range (NFR-PRF-02).

### Live-preview typography (#103; FR-EDT-01, FR-EDT-02, FR-EDT-10, NFR-USE-01, NFR-PRF-02)

Until now the reveal rule hid the Markdown markers and nothing styled what was left, so a note read as unstyled text. This section is the whole typographic contract for Mode 1, the Mode 2 source pane and the Mode 2 preview pane. It adds no new decision about behaviour; every size is a step on the `tokens.json` scale.

**Files.** `tokens.css` is every `--serat-*` custom property, generated from `tokens.json` (the app defines none today; the widgets in `app/src/editor` already reference eighteen of them, with fallbacks). `live-preview.css` is the reference stylesheet for the classes below. Both live in `docs/03-design/ui/` and are installed into `app/` by copying (`app/src/styles/tokens.css`, `app/src/editor/livePreview.css`), because the design folder is not importable from the app.

**The idea.** Two voices on one page. The note is Source Serif 4, the vault's voice, set at 17/28 on a measure tuned for reading. Structure is Bricolage Grotesque, the app's voice: headings are the app's structure laid over the prose, so they change typeface as well as size. Mono appears only where the machine speaks: revealed markers, code, properties. Nothing is decorated; hierarchy comes from size, weight, one rule on quotes and one fill on code.

#### Column and rhythm

| Property | Value | Why |
|---|---|---|
| Body | `body-serif`: Source Serif 4, 17px / 28px, `text-primary` | 28px is the baseline; every block is a whole number of 28px lines except heading padding. |
| Column | `max-width: 72ch` (`typography.measure.editor`, equal to `measure.prose-max`), centred, 24px side gutter (`space.7`) | Measured with the real Source Serif 4 at 17 / 28 on six full lines at 1400px: 64ch gave 61.8 characters on average, 68ch 67.8, 72ch 72.4 (71 to 74), 76ch 75.0 (73 to 80). 72ch is the smallest value inside the 66 to 80 target with margin on both sides. |
| Content padding | `.cm-content` padding-top 16px (`space.5`), bottom 24px (`space.7`) | Not 24px on top. With CodeMirror's 24px scroll margin and a 34 / 40 H1, a 24px top padding left Control+Home 6px short of scrollTop 0. |
| Paragraph gap | One blank line (28px). Blank lines are never shrunk | A shortened blank line would grow to 28px on the first character typed into it, a jump under the caret. |
| Heading space | Padding on the heading line: H1 16 above and 4 below, H2 12 above and 2 below, H3 8 above, H4 to H6 none; none for a heading that opens the note | CodeMirror measures lines, not margins. The blank line before a heading is still there; the padding adds to it. |
| Caret | 2px `accent-interactive` | The same indigo as every interactive thing; ink would be invisible against the text. |
| Selection | `bg-selection` (`indigo.200`), ink text 12:1 | `bg-selected` (`indigo.100`) is too faint on `bg-surface` to see a selection. |
| Active line | No wash | The reveal rule is the only line-level signal. |

#### Headings

| Level | Scale step | Size / line | Weight | Notes |
|---|---|---|---|---|
| H1 | `display` | 34 / 40, -0.025em | 700 | The note title when the author writes one. |
| H2 | `heading-lg` | 28 / 34, -0.02em | 700 | |
| H3 | `heading-md` | 23 / 29, -0.015em | 700 | |
| H4 | `heading-sm` | 19 / 26, -0.01em | 700 | |
| H5 | `heading-xs` | 17 / 28, -0.005em | 700 | Same size as the body, told apart by face and weight. |
| H6 | `heading-xs` | 17 / 28, -0.005em | 600 | `text-secondary` (8.3:1). |

All in the `ui` family, `text-primary` unless stated, `text-wrap: balance`. The ratio between steps is about 1.2, so five sizes stay distinguishable without a heading outweighing the page. This replaces the three-step mapping in `components.md` section 3. Each heading line carries `role="heading"` and `aria-level` (Accessibility, below).

#### Inline

| Markdown | Rendered | Class |
|---|---|---|
| `**bold**` | Source Serif 4 weight 700 | `serat-strong` |
| `*italic*` | Italic face (bundled; never synthesised oblique) | `serat-em` |
| `~~strike~~` | Line-through 1px, `text-secondary` | `serat-strike` |
| `==highlight==` | `bg-highlight` (`brass.100`) fill, ink text, `radius.sm`, 0.15em side padding. Ink on brass.100 is 14.4:1; brass is never a text colour (brass-600 on white is 4.0:1) | `serat-highlight` |
| `` `code` `` | Mono at 0.85em (14.5px in body), `bg-surface-sunken`, `radius.sm`, 0.3em side padding, no border | `serat-code` |
| `[[Note]]`, resolved | `text-link`, 1px underline at 60% alpha, solid on hover | `serat-link-resolved` |
| `[[Note#Missing]]` | As resolved, with a dashed brass underline | `serat-link-subpath-missing` |
| `[[Nothing]]`, unresolved | `unresolved-link` (rust), dotted underline | `serat-link-unresolved` |
| `[text](https://...)` | As a resolved wikilink; the `(url)` shows only when the line is revealed, mono, `text-secondary` | `serat-md-link`, `serat-url` |

The underline measures 3.27:1 on `bg-surface` at 60% alpha (35% measured 1.88:1 and failed the 3:1 rule). The quote rule is `neutral.400` (3.25:1); `neutral.300` measured 2.03:1. There is no dark ramp yet (NFR-USE-03, P2): re-pick both steps against the dark surface when it lands. Links keep a quiet underline at rest. This amends `components.md` section 3 and the external-link bullet above, which said "no underline at rest": indigo on ink body text measures 1.9:1, so colour alone cannot mark a link (WCAG 1.4.1; the accessibility baseline already says colour is never the only carrier).

#### Revealed markers

On a line the selection touches, the hidden characters come back as `serat-marker`: JetBrains Mono at 0.85em of the surrounding size, weight 400, `text-secondary` (`text-marker`, 8.3:1). Inside a heading the marker scales to the heading (H1 0.5em, H2 0.55, H3 0.65, H4 0.8, H5 and H6 0.85), so a revealed `#` is not 34px mono. Same frame, no transition. Standard markers get the class from the highlighter (`processingInstruction`); the custom ones (`[[`, `]]`, `![[`, `==`, `[^`) get it from the reveal plugin when it declines to hide them.

#### Block level

| Block | Treatment | Class |
|---|---|---|
| Blockquote | 2px `rule-quote` (`neutral.400`, 3.25:1 on `bg-surface`) left rule, 16px (`space.5`) indent, `text-secondary`; not italic, because italic is reserved for emphasis. `>` hidden unless revealed. Nesting beyond one level looks the same in P1 | `serat-quote` |
| Bullet list | Marker `-`, `*` or `+` renders as a bullet glyph in `text-secondary` in a 1.5em column; one 1.5em indent per level; wrapped lines hang under the item text. The raw leading indent is hidden when not revealed, otherwise it would double the padding. Today `ListMark` is hidden outright, which makes bullets vanish: the reveal plugin must replace it with the glyph instead | `serat-li`, `--serat-depth`, `serat-bullet` |
| Ordered list | The numeral stays visible (it is information; the reveal plugin must not hide it), `text-secondary`, tabular figures, same column and indent | `serat-li`, `serat-list-number` |
| Fenced code | `mono-source` (14.5 / 24) on `bg-surface-sunken`, full column width, 16px side padding, no border. The fence lines stay as empty bands and are the block's top and bottom padding; the language shows in `micro` mono, `text-secondary` | `serat-code-line`, `serat-code-info` |
| Properties (YAML) | Raw, `mono-source`, `text-secondary` on `bg-surface-sunken`; never rendered, never hidden. Detection stops at 200 lines: an opening `---` with no closing `---` within the first 200 lines is not properties, so a stray rule cannot style a whole note | `serat-frontmatter` |
| Thematic break | Stays raw in P1: `---` in mono, `text-secondary`, letter-spaced. A drawn rule is a P2 widget | `serat-hr` |
| Tables, callouts, embeds | Block widgets own their markup. Table text is `body-serif-sm` (15 / 24); header cells are `ui` 600 with a `border-strong` underline, body rows a `border-hairline` underline, no vertical rules, no zebra. Callout title `ui-medium`, body `body-serif`. Images: `max-width: 100%`, `height: auto`, no radius | (widget) |

Block refs and inline fields (`^id`, `key:: value`) are `mono-source` in `text-secondary`, not `text-tertiary` as `components.md` said: `text-tertiary` is 3.25:1 on `bg-surface`, and these are always visible because they are load-bearing.

#### Mode 2

Source pane: one monospace size (`mono-source`, 14.5 / 24), nothing hidden, nothing resized. Headings and `**bold**` weight 700, `*italic*` italic, markers and URLs `text-secondary`, links `text-link`, unresolved links `unresolved-link` dotted, code and highlights on their fills. No column limit; lines soft-wrap at the pane edge. The preview pane and embeds use `.serat-prose` in `live-preview.css`: the same scale, so a note reads the same in all three places.

#### Fonts

The three families are bundled with the app as self-hosted variable `woff2` (Source Serif 4 roman and italic, Bricolage Grotesque, JetBrains Mono), all under the SIL Open Font License. Nothing loads from a network (NFR-SEC-01). The editor mounts after the body and mono faces have loaded, because CodeMirror measures line heights once; when a face loads later it calls `view.requestMeasure()`. Fallbacks are the stacks in `tokens.json`.

**Acceptance (one vue-frontend-dev task, #104):**
1. `tokens.css` is installed and imported first; `grep -rn "var(--serat-" app/src` finds no name that `tokens.css` does not define; the P0 variables in `app/src/style.css` are replaced by `--serat-brand-*` or aliased to them. A unit test parses `tokens.json` and fails if a colour ramp or role value differs from `tokens.css`.
2. In the Mode 1 editor, H1 to H6 lines render at the sizes, family and weight in the heading table; body text is Source Serif 4 17 / 28; the average full line on the fixture note holds 66 to 80 characters, measured and recorded in the PR (adjust `measure.editor` if not).
3. Bold, italic, strike, highlight, inline code, resolved, subpath-missing and unresolved links, quotes, bullet and ordered lists, fenced code and properties each match their table row. A fixture note containing every one is committed and screenshotted in the PR.
4. Revealed markers use `serat-marker` and appear in the same frame as the selection change, with no transition and no change to the line's text typeface.
5. List lines hang correctly: a wrapped continuation aligns with the item text within 1px at depth 0 to 3, revealed and not; bullets are visible when not revealed and ordered numerals are never hidden.
6. Links keep the underline at rest; unresolved links are dotted; no link state is carried by colour alone.
7. **Met (task #162).** The three families are bundled from the SIL OFL packages `@fontsource-variable/source-serif-4` 5.3.1, `@fontsource-variable/bricolage-grotesque` 5.3.0 and `@fontsource-variable/jetbrains-mono` 5.3.0 (devDependencies; latin subset, weight axis only; each package's LICENSE beside its files in `node_modules`, OFL-1.1 allow-listed in `npm-deny.json`). `fonts.css` keeps `local()` first, then the bundled `url()` with `format("woff2")`; Vite emits the files under `webview/assets`, served from 'self' (the CSP is unchanged). Rendered with every non-local request aborted, the page requests no font off-origin and all four faces reach `loaded`; `requestMeasure` runs on late font load. Measured at a 1400px viewport on `live-preview-all.md` with the real faces: at 64ch the full wrapped lines of body text and list items run 59 to 69 characters, average 61.8 (6 lines), below the 66-80 target. The same fixture at 68ch gives 63-73 (average 67.8), at 72ch 71-74 (72.4), at 76ch 73-80 (75.0). Ratifying `measure.editor` is frontend-designer's, recorded separately; the token is unchanged by this task.
8. The Mode 2 source pane and `.serat-prose` preview follow the Mode 2 paragraph; the editor root carries `serat-live` or `serat-source`, never both.
9. The new decorations are built from `view.visibleRanges` and not from the selection, except what the reveal plugin already does; the end-of-document keystroke scenario (architecture.md section 9.3) and the NFR-PRF-02 budget job still pass.
10. Contrast: every text and fill pair above measures at least 4.5:1 for text and 3:1 for the quote rule and link underline, checked once with a script and recorded in the PR; `text-tertiary` appears on no text needed to understand the note.
11. The inline themes in `links.ts` and `blockWidgets.ts` that duplicate these colours are removed or yield to `livePreview.css`; their fallback hex values are not edited to match.

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

## Scrolling (both modes)

Ratified by architect from #42 (`task/p1-42-prf02-stalls` @ `be0724a`), which introduced both behaviours without a spec. Serves FR-EDT-01, NFR-PRF-02 · US-35. Design basis: architecture.md §9.3, *Headless observation: the end-of-document stall*.

- **The editor column owns the scroll. The page never scrolls.** `#app` is bounded to the window, so the title bar, the tab strip and the status strip stay in place at any note length. The editor scrolls inside CodeMirror's own scroller, and the tree and context panes each scroll themselves. A 5,000-line note never makes the window taller than the window. This is the fix for a real defect: the page used to grow to the note's height, the title bar and status strip scrolled away, and the file tree's virtual window rendered thousands of rows (architecture.md §9.3).
- **Scroll past end.** The last line can be scrolled up to the top of the editor, with empty space below it. Writing at the end of a note then happens at a comfortable eye height instead of against the bottom edge of the window. The empty space is not part of the document, and it is never saved.
- **Scroll margins around the caret: 24 px above, 48 px below.** When the caret moves or text is typed, the editor scrolls just enough to keep the caret line at least 24 px from the top edge and 48 px from the bottom edge. The caret line is therefore never flush with an edge, and the next line is always partly in view while writing. The bottom margin is the larger one because writing moves downward.
- **Why, in two parts.** The first is reading comfort, as above. The second is measured. On headless Chromium 153, typing with the caret line flush against the scroller's bottom edge stalled presentation for seconds, and these two behaviours together clear it. So they are **load-bearing for NFR-PRF-02's headless measurement, and not only a UX choice**. Removing either one, or changing a margin value, is a change to that measurement as well as to this screen. It needs the end-of-document scenario re-run and architecture.md §9.3 updated in the same change.
- **Ruling (#75): 48 px stays, as a named measured exception.** 24 px is `space.7`. 48 px is not on the `space` scale, which has 40 px (`space.9`) and 56 px (`space.10`), and it is deliberately not rounded to either. It is the figure that cleared the end-of-document stall (architecture.md §9.3, #42). Either neighbour would serve reading comfort equally well, so moving buys nothing and costs a harness run. It is recorded in `tokens.json` as `editor.scroll-margin-bottom` = 48 px, beside `editor.scroll-margin-top` = `space.7`. The constant that implements it is `EDITOR_SCROLL_MARGINS` in `app/src/editor/codemirrorSetup.ts`. **Changing the bottom margin to 40 or 56 is allowed only after a new end/strict harness run is archived and §9.3 is updated in the same change.**
- Scroll is never animated by these rules. They adjust the scroll position in the same frame as the edit that moved the caret (§9.3's frame budget), so a keystroke never waits on a scroll transition.

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
