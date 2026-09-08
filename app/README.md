# `app/` — Serat's Vue 3 frontend

Task #98 (P0/impl). Vue 3 + Pinia + CodeMirror 6, TypeScript, Vite. Mounted as a
**plain text box only** — no live preview (that is P1, ADR-0006), no wikilink UI,
no lens switcher, no file tree (`list_notes` is P1). This replaces the placeholder
shell that used to live in `crates/serat-app/webview/` (task S1); that directory is
now this project's *build output*, not hand-written source.

## Commands

```sh
npm install
npm run dev       # Vite dev server, for iterating on components in a browser
npm run build     # type-checks, then builds into ../crates/serat-app/webview
npm test          # vitest run
npm run typecheck # vue-tsc --noEmit, without a build
```

`npm run dev` is a browser preview only: `window.__TAURI__` and
`window.__SERAT_BOOT__` do not exist outside the Tauri webview, so the IPC-dependent
paths (reading a file, the startup marks) do not run against a real vault this way.
Exercising them needs `cargo run -p serat-app -- --vault <dir>`, which serves the
built `crates/serat-app/webview/` — run `npm run build` first.

## Where the build goes, and why it is committed

`vite.config.ts` builds directly into `../crates/serat-app/webview/`, which is where
`tauri.conf.json`'s `frontendDist: "webview"` already pointed — so mounting Vue did
not need to touch that file. Filenames are **fixed, not content-hashed**
(`boot.js`, `index.html`, `shell.css`): `crates/serat-app/src/lib.rs` has a test,
`the_placeholder_bundle_exists_and_carries_the_two_marks`, that opens those two
files by name and asserts `boot.js` still calls both NFR-PRF-01 marks from inside a
`requestAnimationFrame`. A hashed entry filename would make that test unable to find
the file at all.

The built output is **committed**, the same way the placeholder shell's
`index.html`/`boot.js`/`shell.css` were. This is a real gap, not a preference: there
is no CI step yet that runs `npm run build` before `cargo test --workspace` (see
`.github/workflows/ci.yml`), and that test reads `crates/serat-app/webview/` from
disk — an uncommitted, gitignored build output would make a fresh checkout's
`cargo test --workspace` fail on every machine that has not separately run
`npm run build`. Until CI grows a frontend build step (a decision for test-engineer
and tauri-app-dev, not this task), **run `npm run build` and commit the result under
`crates/serat-app/webview/` as part of every change here that could affect it.**

## What mounting a framework costs (the honest number this task was asked for)

The placeholder shell's `<textarea>` sat in static HTML: it existed and was
focusable before a single line of `boot.js` ran, because the browser builds that DOM
from markup alone. Vue's `#app` div is empty until `main.ts` executes,
`createApp(App).mount("#app")` runs, and `CodeMirrorEditor`'s `onMounted` constructs
the `EditorView` — the editor's existence now depends on script parse, module
evaluation and a synchronous Vue mount, where before it depended on nothing.

That dependency is real but the ordering property architecture §3.4 cares about is
preserved: `CodeMirrorEditor` is mounted synchronously (no `await` runs before the
`EditorView` is constructed — see its own doc comment), so the editor exists and
takes keystrokes in the *same script execution* that produces the page's first
paint, not after an IPC round trip. `src/App.test.ts` asserts this directly: it
mounts `App` with every IPC call stubbed to a promise that never resolves, and
still finds a focusable `.cm-content` in the DOM. The two NFR-PRF-01 marks
(`src/startup.ts`) are unchanged in shape from the placeholder's `boot.js`: the
paint mark is still taken from inside a double `requestAnimationFrame`, and the
usable mark still waits for it.

What is not yet known — deliberately left for test-engineer's task #100
re-measurement rather than guessed at here — is the size of the added constant:
script download (none; it is bundled and served locally, not fetched), parse and
Vue+CodeMirror module evaluation, and the mount itself. Task #98 was asked to report
that cost rather than tune around it, and this is the honest version of that report:
the *ordering* guarantee holds, the *zero-JS-dependency* property of the placeholder
does not, and only #100's measurement says whether the difference is visible against
NFR-PRF-01's budget.

## Layout

```
src/
  main.ts                  createApp().mount() — no product logic
  App.vue                  titlebar / editor / statusbar, wires startup + save
  startup.ts                the two NFR-PRF-01 marks — see its own doc comment
  editor/CodeMirrorEditor.vue   ADR-0006's seam: everything else talks to this,
                                 never to CodeMirror's API directly
  stores/vault.ts           Pinia store: vault root, index status text, open
                             buffer's path/hash, diagnostic marks text
  ipc/client.ts             typed wrappers over @tauri-apps/api, against the
                             generated bindings (ADR-0007)
  ipc/generated/            committed ts-rs output — do not hand-edit (ADR-0007)
```
