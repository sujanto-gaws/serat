# Screen: Command palette — every action, its binding, rebindable

Serves: FR-NAV-06, NFR-USE-01
Design basis: architecture.md §9.3 "The command registry"

## Layout

Same overlay chrome as the quick switcher (components.md §5); opened with `⌘` `K`. The palette is a view onto the single command registry every keybinding in the app is built from — this spec is also, functionally, the UI for NFR-USE-01's "Inspect" acceptance method: what a reviewer looks at to confirm every primary action is listed and bindable.

```
┌──────────────────────────────────────────┐
│  toggle liv█                              │
├──────────────────────────────────────────┤
│  Toggle live preview          ⌘ ⇧ M        │
│  Toggle context pane          ⌘ .          │
├──────────────────────────────────────────┤
│  ↑↓ navigate   ↵ run   esc                │
└──────────────────────────────────────────┘
```

## Content

- Fuzzy-matched over every registered command's name (not id — ids are internal, e.g. `editor.togglePreview`; names are what's shown, e.g. "Toggle live preview," per the writing guidance: name things by what users understand).
- Each row: command name (`ui`, matched characters bolded in `accent-interactive`) left-aligned; its **effective** keybinding chip-pair right-aligned (components.md §5's keybinding chip) — effective meaning the user's rebinding if one exists, the registry default otherwise. A command with no binding at all (some exist only for palette/API access) shows no chip, not an empty chip.
- Commands unavailable in the current context (`when` clause false — e.g. "Insert link to selected file" with no tree selection) still appear, greyed (`text-tertiary`), so the palette stays a complete map of the app rather than a moving target; selecting one shows why it's unavailable instead of silently failing.
- Grouping: none by category in the default view — a single ranked list keeps the fuzzy-match behaviour predictable. Typing `>` is reserved for a future category filter (not built in P1; noted so the affordance isn't accidentally claimed by another feature).

## Rebinding shortcuts (NFR-USE-01)

Rebinding does not happen inside the palette itself (the palette is for *running* commands quickly, not configuring them — conflating the two would slow down the common case for the rare one). Instead:
- Each palette row has a secondary affordance, a small "Rebind" icon button that appears on row hover/focus, opening a **compact inline editor** in place of that row: "Press new shortcut…" with a live-captured chip preview, `Enter` to confirm, `Esc` to cancel, `Backspace` to clear the binding entirely (falls back to none, not to the default — an explicit "no binding" is a state distinct from "I never touched this").
- The same rebinding surface is also reachable as a full **Settings → Keybindings** table (not specified further here — it is the same registry rendered as a two-column table, `command` / `binding`, sortable, filterable, out of scope for a modal-vs-page decision this doc doesn't need to make since it's not one of the nine listed screens) for someone who wants to review or export the whole map.
- **Conflict handling**: attempting to bind a chord already in use *in the same `when` context* shows an inline warning naming the colliding command, with two choices — `Reassign` (the new command takes it, the old command loses its binding, both are recorded) or `Cancel`. This mirrors the registry's own unit-test invariant (architecture.md §9.3: "no two commands share a binding in the same `when` context") back to the user instead of silently allowing a conflict the registry would later fail on.
- Rebindings persist to the settings TOML (`[keybindings]`, keyed by command id, per FR-PLT-05) — nothing here is index-derived or vault-derived, so it survives a rebuild and does not sync with the vault's own files.

## States

| State | Presentation |
|---|---|
| Default (opened, empty query) | All commands, ranked by frecency of use (same signal family as the switcher, stored per-command rather than per-note) then alphabetically for ties. |
| Typing | Fuzzy filter as above. |
| No match | "No command matches “xyz”" — no secondary action here (unlike the switcher, there's nothing to "create"). |
| Rebinding in progress | Inline capture row, described above; rest of the list dims slightly (`opacity: 0.5`) to focus attention without a modal. |
| Conflict on rebind | Inline warning row beneath the capture row, `accent-error-bg` background, `Reassign` / `Cancel` buttons. |
| Command unavailable in context | Greyed row, still selectable, shows a one-line reason instead of running ("Select a file in the tree first"). |

## Keyboard map

| Action | Binding |
|---|---|
| Open palette | `⌘` `K` |
| Move selection | `↑` `↓` |
| Run selected command | `↵` |
| Start rebinding the selected command | `⌘` `R` while a row is focused (mouse users get the hover icon) |
| Confirm captured shortcut | `↵` (auto-confirms on any valid non-modifier-only chord after a short settle, so the user doesn't have to remember to press Enter after pressing the shortcut itself — pressing the intended chord *is* the input) |
| Cancel rebinding | `Esc` |
| Dismiss palette | `Esc` |

This table is a subset of the single authoritative binding set in `components.md` §7.

## Responsive rules

Same as the quick switcher (components.md §5, `quick-switcher.md`): 560px card, `min(560px, 90vw)`, ≥ 6 visible rows above 480px window height.

## Accessibility

- Same listbox pattern as the switcher. The keybinding chip-pair for each command is included in the row's accessible name as spoken text ("Toggle live preview, Command, Shift, M") rather than relying on visual glyph chips alone.
- The inline rebinding capture announces via a live region what chord was captured before commit, so a screen-reader user gets the same confirmation a sighted user gets from the chip preview.
- This screen *is* the mechanism NFR-USE-01 is inspected against: every command registered via the front-end registry (architecture.md §9.3) must appear here with no exceptions, which is the acceptance criterion made literal rather than a separate audit spreadsheet.
