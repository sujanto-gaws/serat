# Theming study

Owner: **frontend-designer** · Status: study · Date: 2026-11-27 · Task: #227
Serves: NFR-USE-03, NFR-USE-02 · US-38

Owner request (2026-11-27): study themes, dark/light mode and related UI improvements. This is a study, not a spec: nothing here changes a token, a screen or `app/`. Section 7 proposes the P2 tasks; section 5 is a product decision that belongs to the owner.

Constraints that bound every answer: C-01 and C-02 (nothing in the vault requires new syntax, nothing lives only in the index), C-07 (Tauri v2, Vue 3), design goal 2 (nothing blocks the keystroke), and the app CSP in `crates/serat-app/tauri.conf.json` (`default-src 'self'; script-src 'self'; style-src 'self'; … connect-src 'self' ipc: …`, no remote origin, no `'unsafe-inline'`; ADR-0017 and its amendments; threat-model T-02, T-09).

Sources read: `docs/03-design/ui/{serat-identity.md, tokens.json, tokens.css, components.md, live-preview.css}`, `docs/03-design/ui/screens/*.md`, `app/src/**` (colour grep of `.vue`, `.css`, `.ts`, tests excluded), `app/src/styles/{tokens.css, contrast.test.ts, tokens.test.ts}`, `app/src/editor/{editorStyles.ts, typography.ts, livePreview.css, livePreviewSupplement.css, blockWidgets.ts, links.ts}`, `app/index.html`, `crates/serat-app/webview/index.html`, `crates/serat-app/src/settings.rs`, PRD NFR-USE-02/03 and US-38, ADR-0017, threat-model T-02 and T-09.

## 1. Where colour lives today

### 1.1 The shape of it

`docs/03-design/ui/tokens.json` has three layers: four brand colours, five **ramps** (indigo 50 to 700, brass 100 to 600, rust 100 to 600, neutral 50 to 900) and about 33 **roles** (`bg-*`, `border-*`, `text-*`, `accent-*`, and named single-purpose roles such as `unsaved-dot`, `unresolved-link`, `conflict-*`). `tokens.css` mirrors it as `--serat-*` custom properties on `:root`, roles referencing ramps through `var()`. `app/src/styles/tokens.css` is byte-identical to the docs copy today, and `app/src/styles/tokens.test.ts` keeps it in step with `tokens.json`. Light only: the docs say so in three places (`tokens.json` meta, `tokens.css` header, `components.md` §5).

That architecture is already the right one for theming: a dark theme is a second assignment of the **roles**, and no component needs to learn about ramps. The problem is that the app code mostly does not stop at the roles.

### 1.2 Inventory

"Tokenised" means every colour the surface paints comes from a `--serat-*` role, with no literal that would survive a theme change. A `var(--serat-x, #fallback)` counts as tokenised for the value but is listed as "fallback" because the literal is dead code once the token exists and is a trap when it is not (see finding C).

| Surface | Source file | Tokenised | What is hard-coded or wrong |
|---|---|---|---|
| Page ground, body text | `app/src/style.css` | Partly | Uses the P0 aliases `--paper` / `--ink` (brand constants), not `--serat-surface-app` / `--serat-text-primary`. Aliasing `--paper` to a role would be wrong: `--paper` is also used as a *brand* colour (title-bar text). |
| Title bar | `app/src/style.css` | Partly | `--indigo` ground, `--paper` text: brand constants, fine in both themes, but the dark title bar should step down to indigo-700 so it does not outshine the page. |
| Pane dividers, status strip border | `app/src/style.css` | No | `rgba(22, 25, 27, 0.12)` ink-on-light literals (lines 100, 104, 128). Invisible on a dark ground. |
| Status strip text | `app/src/style.css` | No | `rgba(22, 25, 27, 0.7)` (lines 130, 156). |
| Status strip refused-names mark | `app/src/style.css` | Partly | `.marks { color: var(--brass) }` is brass **as text**: 2.73:1 on paper. Fails AA today (section 4). |
| Refused-names dot | `app/src/refused/RefusedNames.vue` | No | `var(--rust, #a8452f)`: `--rust` is **not defined** anywhere (the token is `--serat-rust`), so the literal is what paints. |
| File tree rows, rail, kind glyphs | `app/src/tree/FileTree.vue` | Partly | `--serat-*` with literal fallbacks, plus `var(--indigo, #24457e)` and `var(--brass, #c08a47)` for the selected rail and the sidecar dot (brand constants, not roles). `rgba(22,25,27,0.12)` fallbacks on two borders. |
| Tree context menu | `app/src/tree/TreeContextMenu.vue` | Yes (fallbacks) | Roles with literal fallbacks; selected row falls back to `rgba(36, 69, 126, 0.12)`. |
| Quick switcher, command palette | `app/src/overlays/overlay.css` | **No** | Card ground is `var(--paper)` (#F3F4F1), not `--serat-surface-overlay` as `components.md` §5 specifies. Every hairline, shadow, muted and secondary text and the selected-row wash is a local `--ov-*` variable built with `color-mix(in srgb, var(--ink) N%, transparent)`. Flipping `--ink` for dark would turn these into near-white-on-near-white; `--ov-selected` mixes `--indigo`, which is 1.87:1 on a dark ground. The hit highlight is `var(--indigo)`. The overlay needs rewriting onto roles, not patching. |
| Backlinks pane | `app/src/panes/BacklinksPane.vue` | Partly | Roles with fallbacks; focused-row wash is a literal `rgba(36, 69, 126, 0.12)` (indigo at 12%); `text-tertiary` is used for **essential** text (source path, front-matter note): 3.25:1 on surface, fails AA. |
| Search panel | `app/src/panes/SearchPanel.vue` | Partly | Same pattern: indigo-12% literal for the focused hit, `var(--indigo)` and `var(--brass)` brand constants, `text-tertiary` for folder and kind labels (11px, fails AA). |
| Callouts, fold glyph, focus outline | `app/src/editor/blockWidgets.ts` (inline CM theme object, lines 850 to 901) | Yes | Roles with literal fallbacks, including `rgba(0,0,0,0.04)` for the raised ground. All map cleanly. The callout rail uses `--serat-accent` (alias of `accent-interactive`), which must be the lightened indigo in dark. |
| Wikilinks (resolved, unresolved) | `app/src/editor/links.ts` | Yes (fallbacks) | `--serat-link` and `--serat-rust` aliases with literal fallbacks. |
| Editor prose, headings, quote, code, highlight, caret, selection | `app/src/editor/livePreview.css`, `livePreviewSupplement.css` | **Yes** | The best surface in the app: no colour literal at all, only `var(--serat-*)` and one `color-mix()` for the link underline. This is what "tokenised" should look like. |
| CodeMirror highlighter | `app/src/editor/typography.ts` (`HighlightStyle.define`) | Yes | Maps tags to **classes** (`serat-strong`, `serat-marker`, …) and chooses no colour; `editorStyles.ts` adopts the design CSS into the shadow root, where it reads `--serat-*` inherited from `:root`. So the HighlightStyle needs **no per-theme build**. |
| CodeMirror chrome the app did not style | `.cm-tooltip`, `.cm-panels`, `.cm-gutters` (source mode; `livePreviewSupplement.css:64` sets it), fold placeholder, bracket-match | Unknown | CodeMirror's `baseTheme` picks `&light` or `&dark` rules from the `EditorView.darkTheme` facet, which the app never sets. Today unreachable chrome (no tooltips or panels are mounted), but a dark theme must set the facet. |
| Split preview prose | `app/src/styles/prose.css` | Yes | Roles only. |
| External-change compare | `app/src/editor/ExternalCompare.vue` | Yes (fallbacks) | Roles with `#fff`, `#ddd`, `#f3e3b8` fallbacks. |
| Editor floating bars, dividers | `app/src/editor/CodeMirrorEditor.vue` (`<style>`) | Yes (fallbacks) | `#ddd`, `#fff`, `#b08d2f` fallbacks. |
| Viewer toolbars, borders, info text | `app/src/viewers/{ViewerHost,PdfViewer,ImageViewer}.vue` | **No** | `#0002`, `#0003` borders and `#0009` text are black-alpha literals: invisible or wrong on dark. Text `#0009` is 5.74:1 on white today. Zero `--serat-*` use. |
| PDF stage and page sheet | `PdfViewer.vue` | **No** | Stage `#0000000d`, sheet `background: #fff` with `box-shadow: 0 0 0 1px #0002`, flash outline `#a3472f`. The canvas itself is pdf.js output (section 3.4). |
| Image viewer checkerboard | `ImageViewer.vue` | **No** | `repeating-conic-gradient(#0000000d 0% 25%, transparent 0% 50%)`. A transparency checkerboard is a functional gradient (shows alpha), not decoration; it stays, retoned per theme. |
| Selection (`::selection`) | none | n/a | Not styled outside the editor; the OS default applies in the tree, panes and overlays. |
| Scrollbars | none | n/a | Not styled: native. They stay light on a dark page unless `color-scheme` is set. |
| Form controls, `accent-color` | none | n/a | No `color-scheme` or `accent-color` anywhere in `app/src` (grep found neither, nor `prefers-color-scheme`, nor `data-theme`). Native inputs, checkboxes and scrollbars will render light on dark. |
| Focus rings | `overlay.css` (`.overlay-input:focus-visible`), `RefusedNames.vue`, `prose.css`, `blockWidgets.ts` | Partly | Role-based (`border-focus`, `--serat-focus-ring`) except the overlay input, which draws an `inset` underline in `--indigo`. No global `:focus-visible` rule yet; each surface does its own. |
| Shadows | `overlay.css` (`--ov-shadow`), `tokens.css` (`--serat-elevation-overlay`) | Partly | Both are ink-alpha on light. On a dark ground a shadow is nearly invisible: dark elevation must come from a lighter surface plus the hairline (section 3.3). |
| Fonts | `app/src/styles/fonts.css` | Yes | Not colour; unaffected by theme. |

### 1.3 The three biggest findings

**A. Overlays and viewers bypass the roles, and the overlay's `--ink`/`--paper` mixing is a dark-mode trap.** `overlay.css` builds its whole palette from `color-mix(--ink …)` on `--paper`; the three viewers use black-alpha literals; the status strip and pane dividers use `rgba(22,25,27,…)`. None of these can follow a role swap. A large share of the app's colour surface (overlays, viewers, status strip, pane chrome) must be moved onto roles before a dark block can be added, which makes that migration the true critical path of NFR-USE-03, not the dark values.

**B. There is no `color-scheme` declaration and no theme-aware native chrome.** Scrollbars, form controls, the OS selection colour in the tree and panes, the default context menu and the title-bar frame will all stay light in a dark theme. One `color-scheme` line per theme fixes most of it; the native frame needs the Tauri window theme (section 2.2).

**C. The fallback literals hide a dead token name.** `RefusedNames.vue` reads `--rust`, which no stylesheet defines, so the `#a8452f` fallback paints; `FileTree.vue` and `SearchPanel.vue` read `--indigo`/`--brass` (brand constants) where a role is meant. A fallback makes a missing token invisible. Either drop the fallbacks once `tokens.css` is guaranteed loaded first (it is: `main.ts` imports it first), or keep them and add a test that rejects a `var(--x, …)` whose `--x` is not defined. `tokens.test.ts` already checks every `--serat-*` read is defined; it does not cover `--rust`, `--indigo`, `--brass`, `--paper`, `--ink`, `--ov-*`.

Related and worth stating: the editor itself (the surface users look at most) is already fully tokenised, and its HighlightStyle already reads tokens by class. The risk in this project is the periphery, not the core.

## 2. Modes

Three user-visible choices: **System** (default, NFR-USE-03), **Light**, **Dark**.

### 2.1 What the webview exposes

`prefers-color-scheme` is a webview media query, so its source is the engine, not Tauri:

| Engine (platform) | Source of the media query | Follows the OS live? |
|---|---|---|
| WebView2 (Windows) | The WebView2 environment's preferred colour scheme, which defaults to the Windows "app mode" setting; Tauri also sets it from the window theme when one is forced. | Yes, as a `change` event on the `MediaQueryList`. |
| WKWebView (macOS) | The effective appearance of the hosting window (`NSAppearance`). | Yes. A forced window theme overrides it. |
| WebKitGTK (Linux) | The GTK settings (`gtk-application-prefer-dark-theme`, and on newer WebKitGTK the desktop portal's colour-scheme). | Version- and desktop-dependent. This is the platform to verify; do not assume it. |

Claims in this table come from the platform documentation and Tauri's behaviour as I know it; **tauri-app-dev should confirm each row against tauri 2.11.5 / wry on the CI images before the design depends on it** (the Linux row especially). The design therefore must not rely on the media query alone for Linux.

### 2.2 Tauri v2's window theme API

**Verified by tauri-app-dev against tauri 2.11.5, wry 0.55.1 and tao 0.35.3 (corrections to the first draft of this section).** `set_theme` and the config `theme` are **app-wide on Linux and macOS, not per window** (tauri `window/mod.rs:1895-97`; tao sets `gtk_application_prefer_dark_theme` on Linux), and the config documentation says "Only implemented on Windows and macOS 10.14+" (tauri-utils `config.rs:2056`), so on Linux treat the forced theme as best effort. WebView2 follows the forced theme through the profile's `PreferredColorScheme` (wry `webview2/mod.rs:433`, `:1835`); WKWebView follows `NSAppearance` (inferred, not read in source); **WebKitGTK's media-query response is unverified and stays a probe** (task 8). The event name `tauri://theme-changed` is confirmed (tauri `manager/window.rs:35`).

Tauri v2 exposes the window theme in Rust and JS: `Window::theme()` (the current OS or forced theme), `Window::set_theme(Option<Theme>)` (`None` returns to the system setting; desktop only), a `theme` key on window config, `WebviewWindowBuilder::theme(Option<Theme>)`, and a theme-changed window event (Rust `WindowEvent::ThemeChanged`; JS `onThemeChanged` on the window, delivered on the `tauri://theme-changed` event channel; the owner's request wrote it as `window.theme-changed`, which is the same concept, the wire name should be confirmed in the 2.11.5 source). What it does:

- `set_theme` styles the **native frame** (title bar buttons, the GTK header, WKWebView appearance) and, as a consequence, the webview's `prefers-color-scheme`.
- The theme-changed event is the Linux fallback: if the media query does not fire, the event still tells the page the OS flipped (when GTK reports it).

Two facts about Serat make this simple. First, **the main window is built in code** (`windows: []` in `tauri.conf.json`, ADR-0017), so `.theme(...)` is set on the builder from the loaded settings, before the window is shown, which fixes the native frame's boot flash. Second, if the webview should call `setTheme` itself it would need a new ACL permission (ADR-0012, `core:window:allow-set-theme`). **Prefer to do it in Rust**: the settings command that stores the preference also calls `set_theme`, and the webview gains no new permission.

Without the API, the webview media query follows the OS on WebView2 and WKWebView by itself. So System mode on those two needs no Tauri call at all; the API earns its place for (a) forced Light or Dark, (b) the native frame, (c) the Linux fallback.

### 2.3 The flash of the wrong theme at boot

The shipped page (`crates/serat-app/webview/index.html`) is a head with `<script type="module" src="./boot.js">` and `<link rel="stylesheet" href="./shell.css">`, and a body that is exactly `<div id="app"></div>` (task #137, TC-131). A module script is deferred, so the browser may paint **before** `boot.js` runs; #137 pins that the stylesheet cannot paint contentful content, but it can paint a **background colour**, and today that is paper. In a dark system a user would see one paper frame, then the app. The CSP forbids the usual fix (an inline `<script>` that sets `data-theme`: `script-src 'self'`, no hash, and the CSP string is pinned byte for byte by `security_config.rs`).

The mechanism, in layers, each sufficient for its case:

1. **System mode: CSS only, no script.** The render-blocking stylesheet declares `:root { color-scheme: light dark; }` and carries the dark token block under `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { … } }`. The first frame already has the right ground and the stylesheet needs no script. This is the default path, so the default case has **no** boot cost and no CSP change.
2. **Forced Light or Dark: the window theme.** serat-app loads the settings TOML into `state` before it builds the window (architecture §3.4, `main → state_ready`), so it passes `.theme(Some(Light|Dark))` to the builder. **The theme must be read from the loaded settings in `state`, not from `settings_geometry()`**, which deliberately reads `Settings::default()` (`lib.rs:1022`). On Linux this is app-wide and best effort (section 2.2). The webview's media query then reports the forced scheme from the first frame and layer 1 produces the right ground. No script runs.
3. **Belt for engines that ignore layer 2 (WebKitGTK, if the probe says so): a Tauri initialization script.** `WebviewWindowBuilder::initialization_script` runs at document creation, before any page script, and is not subject to the page's `script-src` (the dialog plugin already ships one; T-09 records it as "script the bundle scan cannot see"). Serat already ships one initialization script (`window.__SERAT_BOOT__`, `lib.rs:867`), so this extends an existing channel rather than adding a new kind of script. Initialization scripts are CSP-exempt because they go through the platform user-script API, not inline HTML (`security_config.rs:72-90`), which T-09 already notes. It would be a **constant string chosen from an enum**, for example `document.documentElement.dataset.theme="dark"`, never interpolated from a file value. It sets `data-theme` before first paint and removes the question. Cost: one more script the bundle scan cannot see, so architect and code-reviewer must agree before it is used; it is only needed if the Linux probe fails.
4. **After boot:** `boot.js` / the mode store sets `document.documentElement.dataset.theme` to `light` or `dark` (or removes it for System) from the stored preference. From then on the attribute, not the media query, is authoritative.

Rejected: a blocking external classic script `<script src="theme.js">` in the head. It is CSP-legal (`script-src 'self'`) and would read a `localStorage` mirror, but it adds a fetch before first paint on the NFR-PRF-01 path and a second copy of the preference outside the settings file. The layers above reach the same result with no script in the HTML. TC-131's stylesheet allowlist is untouched: a `@media` block of custom-property assignments contains no `content`, `url(` or other paint source.

`shell.css` and `app/src/style.css` must set `background` from a role (`--serat-surface-app`) rather than `--paper`, or the first frame is still paper. Note `shell.css` is a **committed build artefact** (`crates/serat-app/webview/`, which TC-131 already flags as stale): the token-migration task must rebuild it, or #136's bundle-drift job fails.

### 2.4 Persistence

The preference is a UI setting in the settings TOML in the platform config directory (FR-PLT-05; `crates/serat-app/src/settings.rs`, which today has `last_vault`, `last_open`, `window`, `editor`, `keybindings`). A natural shape is `[ui] theme = "system" | "light" | "dark"`. It is plain text (NFR-CMP-02), lives outside the vault, outside `.serat/` (C-02 holds trivially: it is not index state), and defaults to `system` when absent, so an old settings file keeps working (`#[serde(default)]`; `Settings` at `settings.rs:34` has no `ui` or `theme` field yet, so `[ui] theme` is an addition). The read path is `get_ui_settings` / `UiSettings` (`commands.rs:154`); a setter needs a `ts-rs` type change and an ACL entry, with `set_live_preview` as the precedent. Reading it is already on the boot path before the window exists, so the layer-2 mechanism costs nothing extra. Whether it is per machine or per vault: **per machine** (the OS is per machine, and a vault is shared between machines, so a UI preference in it would travel to a screen it does not suit; it would also be state NFR-CMP-01 asks Serat to keep out of the vault).

### 2.5 Live switching, and not stalling the editor

US-38: "Given the OS theme changes, when the app is in system mode, then it follows without restart."

- In System mode, no code runs on an OS change: the engine re-evaluates the `@media` block. Optionally the page listens to `matchMedia("(prefers-color-scheme: dark)")` `change` only to update things that are not CSS (the `darkTheme` facet below, the PDF filter).
- In Light/Dark, the store writes `dataset.theme` and the Rust side calls `set_theme`. The switch is **one attribute write on `:root`**; the cascade recomputes custom properties. The editor is **not re-mounted**: its adopted stylesheet reads inherited `--serat-*`, so it recolours with the document. The one CodeMirror-owned piece, the `EditorView.darkTheme` facet, is flipped by reconfiguring one `Compartment` in a single transaction: it does not rebuild the view or touch the document, history or selection.
- **Not measured, a question for test-engineer:** what a full-page recolour costs on a 5,000-line note (the NFR-PRF-02 shape). The live-preview editor renders only the viewport, so the expected cost is the repaint of a screenful, but it should be a measured case in the performance harness: switch theme on the 5,000-line fixture, record the longest main-thread task and the time to the next frame, and assert the typing-latency budget is unaffected. Switching is rare; the goal is that it is never a stall, and that no theme code ever sits on the input path.
- The three viewers and the status strip must not hold a JS copy of a colour (read `getComputedStyle` once and cache it): a cached colour is a stale colour after a switch.

## 3. Token architecture

### 3.1 Keep: semantic roles over ramps

Do not restructure. Three layers: brand constants (never themed: indigo, brass, ink, paper), ramps (lightness steps), **roles** (the only thing app code may read). Theming is "assign roles per theme". The rule to add: **app code and component CSS read roles only**; brand constants appear only in the mark, the title bar and the icon. That rule fixes findings A and C.

Additions the dark theme forces, none a new hue:

- `--serat-border-input`: a role for form-control boundaries at 3:1 (today the input boundary is `border-hairline`, 1.48:1: WCAG 1.4.11 asks 3:1 for the boundary that identifies a control).
- `--serat-scrim`: the overlay backdrop colour (`overlay.css` paints a scrim today).
- `--serat-shadow-*`: the elevation colour as a role, because shadow colour does not invert.
- `--serat-text-on-brass` is not needed: brass is used as a dot and as a fill under ink text.

### 3.2 The two token blocks

```css
:root {                         /* light: today's values, unchanged */
  color-scheme: light;
  /* ramps and roles as in tokens.css */
}

/* Dark, explicit. */
:root[data-theme="dark"] {
  color-scheme: dark;
  /* dark role assignments */
}

/* Dark, from the OS, unless the user forced Light. Same declarations. */
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
    /* dark role assignments */
  }
}
```

The duplicate declaration is the cost of having no script in the System path. `tokens.json` should therefore gain a `theme` axis (`color.role` keeps the light values; `color.role-dark` carries the overrides), and `tokens.css` is generated into both blocks, so there is one source of truth. `tokens.test.ts` should assert the two dark blocks are equal.

Two rules: `color-scheme` is part of each block, so native scrollbars, inputs and the default selection follow; and `accent-color` is set to `--serat-accent-interactive` on `:root`.

### 3.3 The dark ramp, derived from the identity palette

Identity rules are kept: indigo is the primary, brass is the accent only, no gradients, no new hue. Dark is **ink as the ground, paper as the text**, which is the mark's own inversion (`serat-identity.md`: the mark "inverts cleanly", no second version needed).

Neutral ramp, dark steps (ink to paper, interpolated the same way as the light neutrals):

| Step | Value | Use |
|---|---|---|
| ink-950 | `#0E1011` | surface-sunken |
| ink-900 | `#111314` | surface-app |
| ink-800 | `#16191B` | surface (the brand ink, `serat-identity.md`) |
| ink-750 | `#1B1F22` | surface-raised |
| ink-700 | `#20252A` | surface-overlay |
| ink-600 | `#2C3135` | border-hairline |
| neutral-600 | `#4C4F49` | border-strong (existing step) |
| neutral-500 | `#6B6F67` | border-input, rule-quote (existing step) |
| neutral-400 | `#8C9088` | text-tertiary (existing step) |
| neutral-300 | `#B4B7AE` | text-secondary (existing step) |
| neutral-100 | `#E7E8E4` | text-primary (existing step) |

Accent steps for dark (lighter indigo and rust because dark indigo `#24457E` on ink is 1.87:1 and rust `#9C4A3A` on ink is under 3:1, as `screens/file-tree.md` already records):

| Step | Value | Use |
|---|---|---|
| indigo-300 | `#9DB8EA` | accent-interactive, text-link, border-focus |
| indigo-200d | `#BCD0F2` | accent-interactive-hover, text-link hover |
| indigo-500 | `#2F569B` (existing) | primary button fill (paper text on it: 6.50:1) |
| indigo-800 | `#223457` | surface-selected |
| indigo-700d | `#2C4577` | surface-selection |
| indigo-700 | `#1B3660` (existing) | title bar |
| brass-500 | `#C08A47` (existing, unchanged) | accent-secondary, unsaved-dot, conflict-border |
| brass-900d | `#4A3B22` | surface-highlight, conflict-bg, accent-secondary-bg |
| rust-300 | `#E39A89` | accent-error, unresolved-link |
| rust-900d | `#3E211B` | accent-error-bg |

Role map, light to dark (light values are today's):

| Role | Light | Dark |
|---|---|---|
| surface-app | `#F3F4F1` | `#111314` |
| surface | `#FFFFFE` | `#16191B` |
| surface-sunken | `#E7E8E4` | `#0E1011` |
| surface-overlay | `#FFFFFE` | `#20252A` |
| surface-raised | `#F3F4F1` | `#1B1F22` |
| surface-selected | `#E4E9F2` | `#223457` |
| surface-selected-weak | `#EFF2F8` | `#1B2742` (text-primary 12.05:1, text-secondary 7.29:1, text-link 7.40:1) |
| surface-selection | `#CBD6EA` | `#2C4577` |
| surface-highlight / conflict-bg / accent-secondary-bg | `#F3E6D2` | `#4A3B22` |
| border-hairline | `#D3D5CE` | `#2C3135` |
| border-strong | `#B4B7AE` | `#4C4F49` |
| border-input (new) | `#8C9088` | `#6B6F67` |
| border-focus | `#24457E` | `#9DB8EA` |
| text-primary | `#16191B` | `#E7E8E4` |
| text-secondary / text-marker | `#4C4F49` | `#B4B7AE` |
| text-tertiary | `#8C9088` | `#8C9088` |
| rule-quote | `#8C9088` | `#6B6F67` |
| text-link / accent-interactive | `#24457E` | `#9DB8EA` |
| accent-interactive-hover | `#1B3660` | `#BCD0F2` |
| text-on-indigo | `#F3F4F1` | `#F3F4F1` (on the indigo-500 fill) |
| accent-secondary / unsaved-dot | `#C08A47` | `#C08A47` |
| accent-error / unresolved-link | `#9C4A3A` | `#E39A89` |
| accent-error-bg | `#F1DAD3` | `#3E211B` |
| elevation-overlay | ink shadow, 16% | none; the layer is `surface-overlay` + hairline |

Design notes: (1) dark surfaces get **lighter as they rise** (sunken, app, surface, raised, overlay), the dark-mode equivalent of a shadow, so the "floating layers take the overlay recipe" rule in `components.md` §5 survives with the shadow replaced by a step. (2) The dark ground is the brand ink, not true black and not a blue-grey: it keeps the warm-neutral cast of the light neutrals. (3) Brass is unchanged: at 5.86:1 on the dark surface it is *better* in dark than in light, where it is 3.01:1. (4) Link underline at 60% alpha measures 3.96:1 in dark (3.27:1 in light), so the existing `--serat-lp-link-underline-alpha` works in both. (5) The primary button uses the indigo-500 fill in dark with paper text so the button keeps the identity colour as a fill and reads as a filled shape at 2.46:1 against `surface` (acceptable: the button's label carries the information; a 1px `border-hairline` is not needed).

### 3.4 How roles reach CodeMirror, the PDF viewer and images

- **CodeMirror.** No per-theme build is needed: `seratHighlightStyle` is class-based and the design CSS reads tokens inside the shadow root. The only per-theme piece is the `EditorView.darkTheme` facet (so CodeMirror's own base theme picks its `&dark` rules for chrome the app has not restyled), held in a `Compartment` and reconfigured on switch. A `HighlightStyle` built per theme with literal colours is the alternative; it is rejected because it would duplicate the token values in TypeScript and re-run on every switch.
- **PDF viewer (pdf.js canvas).** Two options: **leave the page as printed**, or **invert the canvas** with a CSS `filter: invert(1) hue-rotate(180deg)`. Recommendation: **leave as printed by default**. A PDF is a fixed-layout document: inversion changes the colour of figures, photographs and anything a reader may be checking, and corrupts a PDF that already has a dark ground. The cost of "as printed" is a white page in a dark window; the sheet's existing `box-shadow: 0 0 0 1px` frame is retoned (dark: `border-hairline`) and the stage ground goes to `surface-sunken`, which frames the white page like a sheet on a desk. Offer inversion as an explicit per-viewer toggle ("Dark page") in the viewer toolbar later (P4), because it is a filter on a `<canvas>` and needs no pdf.js change. Constraints from ADR-0017 Amendment 1 are untouched: the filter is a CSS property on the app's own canvas, it adds no `blob:` or `data:` source and no script, and pdf.js's `enableScripting: false`, `useWasm: false` and the worker origin do not change. The pdf.js text layer is transparent text and is unaffected by either option; the text-selection highlight inside it follows `::selection`.
- **Images.** Leave images exactly as stored; never filter or invert an image (a user's diagram or photo is data). The checkerboard behind transparent images is retoned per theme (dark: a 6% paper wash). SVG is shown only through `<img>` (ADR-0017), so its colours are its own: a black-on-transparent SVG will be near invisible on a dark ground, a known limitation; the checkerboard or a "light backdrop" toggle on the image viewer is the mitigation, not rewriting the image.

## 4. Contrast

### 4.1 Method

WCAG 2.x relative luminance, per `app/src/styles/contrast.test.ts` (the same code, so the numbers match its output):

```
c' = c / 255
c_lin = c' / 12.92                       if c' <= 0.03928
      = ((c' + 0.055) / 1.055) ^ 2.4     otherwise
L = 0.2126 * R_lin + 0.7152 * G_lin + 0.0722 * B_lin
ratio = (L_lighter + 0.05) / (L_darker + 0.05)
```

AA thresholds: 4.5:1 for text under 24px (or 18.66px bold), 3:1 for large text and for non-text (control boundaries, focus indicators, icons that carry meaning). Values below were computed with a script over the hex values in `tokens.json` and the dark values in section 3.3, not estimated.

### 4.2 Light theme, today's tokens

| Foreground on background | Ratio | AA |
|---|---|---|
| text-primary on surface | 17.65 | pass |
| text-primary on surface-app | 16.00 | pass |
| text-primary on surface-sunken | 14.35 | pass |
| text-primary on surface-selected | 14.50 | pass |
| text-primary on surface-selection | 12.07 | pass |
| text-primary on surface-highlight | 14.35 | pass |
| text-secondary on surface | 8.32 | pass |
| text-secondary on surface-app | 7.54 | pass |
| text-secondary on surface-sunken | 6.77 | pass |
| text-secondary on surface-selected | 6.83 | pass |
| **text-tertiary on surface** | **3.25** | **fail (text)** |
| **text-tertiary on surface-app** | **2.94** | **fail (text)** |
| text-link on surface | 9.42 | pass |
| text-link on surface-selected | 7.74 | pass |
| text-link on surface-sunken | 7.66 | pass |
| text-on-indigo on indigo-600 | 8.54 | pass |
| text-on-indigo on indigo-700 | 10.91 | pass |
| accent-error (rust-500) on surface | 6.08 | pass |
| accent-error on accent-error-bg | 4.55 | pass (narrow) |
| rust-600 on accent-error-bg | 6.21 | pass |
| paper on accent-error fill (destructive button) | 5.51 | pass |
| text-primary on brass-500 fill | 5.86 | pass |
| **brass-500 on surface, as text** | **3.01** | **fail (text); pass as a 3:1 dot** |
| **brass-500 on surface-app, as text** | **2.73** | **fail (text)** (`.marks` in `style.css`) |
| brass-600 on surface | 4.00 | fail as body text, pass large |
| brass-500 on surface-highlight | 2.45 | fail (already known: `components.md` filter chip uses ink text) |

Non-text (3:1):

| Element | Ratio | Result |
|---|---|---|
| border-focus (indigo-600) on surface / app / selected | 9.42 / 8.54 / 7.74 | pass |
| rule-quote (neutral-400) on surface | 3.25 | pass |
| link underline at 60% | 3.27 | pass |
| border-hairline on surface | 1.48 | decorative divider, exempt |
| border-strong on surface | 2.03 | **fail if it is the only cue for an input boundary** |
| `#0003` viewer button/input border on white | 1.61 | **fail for input boundaries** |

Literal fallbacks that matter: `#0009` text on white 5.74 pass; `rgba(22,25,27,.7)` on paper 6.21 pass; `#8a8f94` (the fallback for `text-tertiary`) 3.26 fail; `#6b7075` 5.00 pass; indigo on ink 1.87 (the reason the overlay's `--ov-selected` mixing cannot be reused in dark).

**Failures in today's light tokens, summary:**

1. `text-tertiary` is used as **essential text** in `BacklinksPane.vue` (source path, front-matter note) and `SearchPanel.vue` (folder and kind labels at 11px): 3.25:1 on surface. `livePreviewSupplement.css` already avoids it for this reason ("text-tertiary is 3.25:1 on surface"); the panes do not. Fix: move those uses to `text-secondary`, and reserve `text-tertiary` for disabled or decorative marks, as `components.md` already says.
2. `style.css .marks { color: var(--brass) }` (the status strip's refused-names mark) is brass text at 2.73:1 on paper. Fix: text in `text-secondary`, with the brass or rust dot carrying the colour (which is what `components.md` §1 specifies).
3. Input boundaries use `border-hairline` (1.48:1) or `border-strong` (2.03:1); WCAG 1.4.11 wants 3:1 for the boundary of a text input (the proposed `border-input` role at neutral-400 is 3.25:1 light, neutral-500 3.44:1 dark).
4. The `accent-error` on `accent-error-bg` pair passes at 4.55:1 with almost no margin; any tint change breaks it. The test should pin it.

### 4.3 Dark theme, proposed values (section 3.3)

| Foreground on background | Ratio | AA |
|---|---|---|
| text-primary on surface | 14.35 | pass |
| text-primary on surface-app | 15.14 | pass |
| text-primary on surface-sunken | 15.50 | pass |
| text-primary on surface-raised | 13.48 | pass |
| text-primary on surface-overlay | 12.55 | pass |
| text-primary on surface-selected | 10.05 | pass |
| text-primary on surface-selection | 7.67 | pass |
| text-primary on surface-highlight | 8.79 | pass |
| text-secondary on surface | 8.68 | pass |
| text-secondary on surface-sunken | 9.38 | pass |
| text-secondary on surface-overlay | 7.60 | pass |
| text-secondary on surface-selected | 6.09 | pass |
| text-tertiary on surface | 5.43 | pass |
| text-tertiary on surface-overlay | 4.75 | pass (narrow) |
| **text-tertiary on surface-selected** | **3.81** | **fail (text)** |
| text-link on surface | 8.81 | pass |
| text-link on surface-overlay | 7.71 | pass |
| text-link on surface-selected | 6.17 | pass |
| text-link hover on surface | 11.31 | pass |
| paper on indigo-500 fill (primary button) | 6.50 | pass |
| accent-error (rust-300) on surface | 7.78 | pass |
| accent-error on surface-overlay | 6.80 | pass |
| accent-error on accent-error-bg | 6.43 | pass |
| brass-500 on surface (dot, 3:1) | 5.86 | pass |
| brass-500 on surface-overlay | 5.13 | pass |
| brass-500 on surface-highlight (non-text) | 3.59 | pass (3:1) |

Non-text (3:1): border-focus on surface / overlay / selected 8.81 / 7.71 / 6.17 pass; rule-quote (neutral-500) on surface 3.44 pass; border-input on surface 3.44 pass; link underline at 60% 3.96 pass; border-hairline 1.34 and border-strong 2.12 are decorative dividers, as in light. Dark text pairs pass 4.5:1 except `text-tertiary` on `surface-selected` (3.81:1). The margins are thinner than in light for the secondary tier: `text-secondary` on `surface-selected` is 6.09:1, a margin of 1.59 over the threshold, and `text-tertiary` on `surface-overlay` clears it by only 0.25. **Remedy, to settle in task 1:** (i) the rule that no `text-tertiary` text sits on a selected row or an overlay (tree rows, search and backlink hits all select), which is already the right rule in light; and/or (ii) a lighter dark `text-tertiary` of `#A3A79F` (5.06:1 on selected, higher elsewhere). Recommend both. The dark theme is otherwise more accessible than light today, because paper-on-ink removes the failing brass-as-text case and the light `text-tertiary` failure on surface.

### 4.4 Focus visibility

Today each surface draws its own ring (`--serat-focus-ring: 2px solid var(--serat-border-focus)`, offset 1px) and the overlay input uses an inset indigo underline. In both themes the ring is 9.42:1 (light) and 8.81:1 (dark) against the surface and at least 6.17:1 against selected rows, so contrast is not the risk; **coverage** is. Proposal: one global `:focus-visible { outline: var(--serat-focus-ring); outline-offset: var(--serat-focus-ring-offset); }` in `style.css` with per-surface overrides only where the ring would clip (tree rows, overlay input). A ring must never be `outline: none` without a replacement. NFR-USE-02's acceptance ("keyboard traversal shows focus at every stop") then becomes a test over the global rule plus a Playwright/vitest traversal, not an audit.

### 4.5 The automated check, and when

NFR-USE-02 is a P5 requirement, but its acceptance ("Automated contrast audit passes in both themes") describes a test that already half-exists: `app/src/styles/contrast.test.ts` parses `tokens.css` and checks nine text pairs and two non-text pairs. Recommendation: **land the theme-matrix version at P2, in the same change that adds the dark tokens**, not at P5. Reasons: a dark palette tuned by eye is how `text-tertiary` happened; the test is cheap (a table of pairs, run once per theme block); and a P5 audit of a P2 palette finds problems when changing a token touches every screen.

Shape: a list of `[fg role, bg role, threshold]` pairs (the 4.2 and 4.3 tables are the seed list); the test resolves roles in `:root` and in each dark block (both blocks, so an edit to only one fails) and asserts each pair. A second assertion walks the source files and rejects a colour literal outside `tokens.css` and an allowlist (brand constants and the checkerboard), which is the regression guard for findings A and C. Owner: test-engineer (the test), frontend-designer supplies the pair list. It sits in `app/` (vitest), as the existing one does, so no new bin is needed; a doclint-style bin would only be justified if the pair list moved into `tokens.json`.

## 5. Obsidian compatibility

Users of Obsidian keep themes in `.obsidian/themes/<name>/theme.css` and snippets in `.obsidian/snippets/*.css`, written against Obsidian's variable names (`--background-primary`, `--background-secondary`, `--text-normal`, `--text-muted`, `--text-faint`, `--interactive-accent`, `--text-accent`, `--text-selection`, `--background-modifier-border`, `--text-error`, `--text-highlight-bg`) and, for real themes, against Obsidian's DOM (`.markdown-source-view`, `.cm-s-obsidian`, `.workspace-leaf`). **This is a product decision for the owner**; the options and their consequences:

| | (a) Read nothing | (b) Map a subset of variables | (c) Inject theme CSS |
|---|---|---|---|
| What happens | Serat has its own themes only. | Rust reads `:root`, `.theme-dark`, `.theme-light` blocks of the user's chosen snippet or theme, extracts only `--name: <colour>` declarations from a fixed allowlist, validates each value as a colour, and hands a **map of validated colours** to the webview, which assigns Serat roles with `style.setProperty` (CSSOM). No stylesheet text reaches the page. | The user's CSS file is loaded as a stylesheet. |
| C-01 (nothing requires proprietary syntax) | Holds. | Holds: the `.obsidian/` files are only **read**, and nothing requires them; a vault without them works identically (NFR-CMP-01 already requires `.obsidian/` untouched). | Holds for the vault, but Serat becomes dependent on Obsidian's DOM contract to look right. |
| C-02 (no state only in the index) | Holds. | Holds: the choice of snippet is a UI setting (settings TOML, outside the vault); nothing is stored in `.serat/`. | Same as (b). |
| CSP (ADR-0017) | No change. | No change: no new `style-src` source, no inline style. `style-src 'self'` stays. CSSOM property writes are not blocked by `style-src`. | **Conflicts.** An external vault file is not `'self'`; an inline `<style>` is blocked outright; the only route is a constructable stylesheet (`adoptedStyleSheets`), which the CSP permits (the editor already uses it, `editorStyles.ts`) and which is therefore a **hole**: the whole point of `style-src 'self'` is that nothing outside the bundle styles the app. It would also need the CSP-pinning test (`security_config.rs`) to be rethought, and a code-reviewer-signed exception to T-09's "escapes its styling" row. |
| Arbitrary-CSS risk | None. | Bounded: only colour values from an allowlist of names are accepted; no selectors, no `url()`, no `@import`, no `content`, so no exfiltration and no UI spoofing beyond colour. | Full. `url(https://…)` in CSS is governed by `img-src` / `font-src` (both without a remote origin), and `connect-src 'self'` blocks fetch and XHR (no remote origin in any directive, T-02), so **exfiltration by `url()` does not succeed** and should be stated as such, but arbitrary selectors still reach every element: hide the review queue's diff, restyle a destructive confirmation to look benign, set `content:` text, or paint over the status strip. C-05 (no AI write lands without diff review) is a promise the UI makes visibly; a hostile cloned vault must not be able to restyle that surface. |
| Does it deliver what Obsidian users expect? | No. | **Partly, and honestly:** colours from a snippet carry over; fonts, spacing and layout do not. | No: Obsidian themes target Obsidian's DOM, which Serat does not have; most of the CSS would match nothing. A theme would mostly be dead weight carrying all of (c)'s risk. |
| Cost | none | M: a small CSS-variable extractor in Rust (a strict declaration parser, not a CSS engine), an allowlist map, an IPC type, a settings picker, and a **contrast guard** (a mapped colour pair that fails AA is rejected and the Serat role stays). | L: plus the CSP change, a sanitiser for the CSS, a hostile corpus. |
| Owner | n/a | rust-core-dev (parser, in `serat-vault` or `serat-parse`), tauri-app-dev (IPC), vue-frontend-dev (apply, picker) | architect (ADR), tauri-app-dev (CSP), code-reviewer (T-09 exception) |

If **(b)** is chosen: product-analyst adds a **new FR under CMP** (Should, P3 or later; "Serat shall map a documented subset of Obsidian CSS colour variables from a user-selected snippet onto its roles, read-only, with no stylesheet text loaded") and an acceptance criterion that a hostile snippet with `url()`, `@import` and selectors changes nothing; architect writes **an ADR** (the read path from `.obsidian/`, the allowlist, the CSSOM-only application and why that is not a `style-src` change) and updates threat-model T-09 (UI spoofing by vault content; the roles `accent-error`, the review-queue and diff colours are **not overridable**). If **(c)** is chosen: the same FR, an ADR that amends ADR-0017 and the CSP pinning, a new threat entry, and the owner accepting that the security model weakens. I do not recommend it.

**Recommendation: (a) for P2, with (b) kept open as a post-P2 owner decision.** Reasons: P2's job is two first-party themes that pass AA; (b) multiplies the contrast matrix by whatever a user supplies, and its guard (reject a failing pair) is more design than it looks. The Obsidian user's real need, "my accent and ground colours", is far better served after Serat's own themes exist, as a narrow, validated variable map (b) than by running their CSS. Do not do (c) under the current CSP.

## 6. Beyond two themes

| Item | What | Cost | Needs a requirement? |
|---|---|---|---|
| **Accent choice** | Let the user pick the interactive accent from a short set (indigo default, plus two or three that pass AA in both themes). | S for the mechanism (one role pair), M for the matrix (each accent doubles the contrast table). It also bends identity: the brief is "indigo reads as navy, not purple", and brass is the only secondary. | Yes: it is a new capability not in NFR-USE-03. Product-analyst. Recommendation: not now; revisit after dark ships. |
| **High-contrast theme** | A third token block: pure ink and paper, 7:1 text (AAA), 3px focus ring, 1px real borders on every container, no tint-only distinctions. Also honour `@media (prefers-contrast: more)` and Windows forced-colors (`@media (forced-colors: active)`: system colours via `CanvasText`, `Canvas`, `Highlight`). | M. The dark token machinery makes it one more block; forced-colors is a separate audit (borders become visible, background-only cues vanish: the tree rail and the selected row need a border or outline fallback). | NFR-USE-02 asks AA, not AAA, so a high-contrast theme is a new requirement; forced-colors support is arguably within NFR-USE-02's "all interactive elements shall have a visible focus state". Product-analyst to decide. Recommendation: ship `prefers-contrast: more` and forced-colors fallbacks in P5 with the audit, not a named theme. |
| **Reduced motion** | Already implemented: `tokens.css` collapses the three duration tokens to 1ms under `prefers-reduced-motion: reduce`. Gap: no test pins it, and any CSS animation or JS timer that does not read the duration tokens (the PDF page-flash outline, the callout fold) escapes it. | S: a vitest scan that animations read the duration tokens. | No: it is hygiene under NFR-USE-02's spirit. |
| **Font size scale** | A UI-size setting. The type scale is in **px** (`--serat-type-*-size`, `body-serif` 17px, `ui` 13px), so a root-font-size scale does nothing; the scale needs `rem` or a `--serat-ui-scale` multiplier applied through `calc()` in each size token. Panes use rem in places and px in others (`style.css` 12px, viewers 0.8rem), so the migration touches everything. | M to L. The 72ch measure and the row heights (`row-height-tree` 26px, 28px list) are px and must scale with it. | Yes, if it is user-visible: it is accessibility (WCAG 1.4.4 resize text) and not in NFR-USE-02 as written. Product-analyst. Recommendation: fold into a later NFR-USE-02 revision; do not start it before dark ships. |
| **Editor-only theme** (a distinct reading ground, such as a warmer paper) | A role override for `surface` inside the editor. | S | No; but it fights the identity brief's restraint. Not recommended. |

## 7. P2 task breakdown

Subject style follows CLAUDE.md (`[Pn/phase] imperative`). No new IDs are proposed; "Serves" lists existing IDs. Sizes: S under a day, M a few days, L about a week.

| # | Proposed subject | Serves | Owner | Depends on | Size |
|---|---|---|---|---|---|
| 1 | `[P2/design] Specify the dark token set and the theme-matrix contrast table` (extend `tokens.json` with a `role-dark` axis, regenerate `tokens.css`, add `border-input` and shadow roles, finalise section 3.3 values) | NFR-USE-03, NFR-USE-02 | frontend-designer | this study; owner answers Q1, Q3 | S |
| 2 | `[P2/impl] Move overlays, viewers, status strip and pane chrome onto roles` (remove every colour literal and `--ink`/`--paper`/`--indigo`/`--brass` use outside the brand surfaces, rewrite `overlay.css` on `--serat-surface-overlay`, fix the three AA failures in 4.2) | NFR-USE-03, NFR-USE-02 | vue-frontend-dev | none; can start now; **must precede 6** | M |
| 3 | `[P2/test] Add the theme-matrix contrast test and the no-literal-colour guard` (extend `contrast.test.ts` over every theme block; reject a colour literal outside `tokens.css` and the allowlist; reject a `var(--x, …)` whose `--x` is undefined) | NFR-USE-02 | test-engineer | 1 supplies the pair list; the no-literal half can start with 2 | S |
| 4 | `[P2/impl] Add the persisted theme setting and window theme` (`[ui] theme` in the settings TOML with `serde(default)`; `.theme()` on the window builder from settings; `set_theme` in the settings-update path, no new ACL permission; confirm the theme-changed event name and the WebKitGTK behaviour; initialization-script fallback only if the probe fails) | NFR-USE-03, NFR-CMP-02, US-38 | tauri-app-dev | architect review of the initialization script if used | M |
| 5 | `[P2/impl] Add the mode switch: store, command palette entry, status strip control` (System/Light/Dark; palette command, no default chord; sets `dataset.theme`; listens for `matchMedia` changes; flips the CodeMirror `darkTheme` Compartment) | NFR-USE-03, NFR-USE-01, US-38 | vue-frontend-dev | 4 (setting and command), 1 (tokens), 6 | M |
| 6 | `[P2/impl] Install the dark token blocks and set color-scheme and accent-color` (both blocks generated from `tokens.json`, shell and page ground on roles) | NFR-USE-03 | vue-frontend-dev | 1, **2** | S |
| 7 | `[P2/impl] Retone the viewers: PDF stage and sheet, image checkerboard, toolbars` (leave PDF canvas as printed; retone frame and ground; optional inversion deferred) | NFR-USE-03 | vue-frontend-dev | 2, 6 | S |
| 8 | `[P2/test] Measure theme-switch cost on the 5,000-line note and boot-time flash` (longest task and frame time on switch; first-frame ground colour with OS dark and forced Light; confirm NFR-PRF-01 and the typing budget are unaffected; Linux probe of `prefers-color-scheme` following the window theme) | NFR-USE-03, NFR-PRF-02, NFR-PRF-01 | test-engineer | 4, 5, 6 | M |
| 9 | `[P2/design] Update components.md, screens/*.md and tokens docs for both themes` (retire "light only" notes; dark rules per screen; keyboard-focus coverage; the global `:focus-visible` rule) | NFR-USE-03, NFR-USE-02 | frontend-designer | 1 | S |
| 10 | `[P2/decision] Decide Obsidian theme compatibility (a), (b) or (c)` | NFR-CMP-01 | owner; then product-analyst (new FR under CMP) and architect (ADR) if (b) or (c) | none; not on the dark critical path | S to decide; M for (b) after |
| 11 | `[P5/test] Keyboard-traversal and forced-colors audit in both themes` | NFR-USE-02 | test-engineer | 6 | M |

Order: **1 and 2 start immediately and in parallel**; 3 follows 1; 6 needs 1 and 2; 4 can run alongside 2; 5 needs 4 and 6; 7 needs 2 and 6; 8 closes the phase after 4 to 6; 9 follows 1; 10 is independent and gates nothing in P2 unless the owner picks (b) or (c). The critical path is 2, then 6, then 5, then 8.

## 8. Open questions for the owner

1. Is the default for a new install **System** (as NFR-USE-03 says), with a manual override in settings and the palette?
2. Should the theme preference be per machine (recommended) rather than per vault?
3. Is the dark ground the brand ink `#16191B` family (as proposed), or do you want a bluer, lighter dark?
4. Should PDFs stay white in dark mode (recommended), with a later per-viewer "dark page" toggle, or should they be inverted by default?
5. For Obsidian themes and snippets: (a) nothing, (b) colour variables only, or (c) full CSS? Recommended (a) now.
6. If (b), is reading `.obsidian/snippets` and `.obsidian/themes` (read-only, never written) acceptable alongside NFR-CMP-01's "`.obsidian/` untouched"?
7. Do you want a user-selectable accent colour, or does brand indigo stay the only interactive colour?
8. Do you want a named high-contrast theme, or is honouring the OS contrast and forced-colours settings enough?
9. Is a UI font-size setting wanted in P2, or can it wait for the accessibility pass in P5?
10. Is shipping the Linux theme fallback (a constant initialization script) acceptable if the WebKitGTK probe shows the window theme does not reach the media query?
