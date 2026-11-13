// Task #98 (P0/impl): mount Vue 3 + CodeMirror 6. See app/README.md.
//
// The build lands directly in crates/serat-app/webview/, which tauri.conf.json's
// `frontendDist` already points at (unchanged) — so this replaces the placeholder
// shell in place rather than asking tauri-app-dev to repoint the config. Filenames
// are fixed (no content hash) so the output is small, reviewable and stable to diff:
// crates/serat-app/src/lib.rs has a test that opens `webview/index.html` and
// `webview/boot.js` by name and asserts boot.js still calls both NFR-PRF-01 marks
// from inside a `requestAnimationFrame`. Hashing the entry filename would silently
// break that test's ability to find the file at all, which is a worse failure mode
// than a name collision on rebuild.
//
// `base: "./"` matches Tauri's asset protocol, which does not guarantee the app is
// served from a root-relative "/".
import { fileURLToPath, URL } from "node:url";

import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

import { pdfjsDataPlugin } from "./src/build/pdfjsData";

export default defineConfig(({ mode }) => ({
  base: "./",
  plugins: [vue(), pdfjsDataPlugin()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  // The pdf.js worker (a `new Worker(new URL(...))` in viewers/pdfEngine.ts) keeps a fixed name
  // like everything else here, not a content hash.
  worker: {
    rollupOptions: { output: { entryFileNames: "assets/[name].js" } },
  },
  build: {
    // A bench build (NFR-PRF-02 harness only; installs `window.__seratBenchOpenNote`)
    // goes to app/dist-bench so it can never overwrite the shipped webview/.
    outDir: fileURLToPath(new URL(mode === "bench" ? "./dist-bench" : "../crates/serat-app/webview", import.meta.url)),
    emptyOutDir: true,
    // No inline script and no CDN, so `tauri.conf.json`'s "script-src 'self'" is
    // satisfiable with no changes — but that claim used to be made for "style-src
    // 'self'" too, and it was wrong (F-417/task #116, measured over the DevTools
    // Protocol against a real WebView2 window): this build's *own* CSS is a static
    // file served from 'self' and is fine, but CodeMirror styles itself at *runtime*
    // through style-mod, which — for a mount parented directly into the document —
    // creates a `<style>` element and sets `textContent`. That is an inline
    // stylesheet, and the CSP has no `'unsafe-inline'`, nonce or hash for it, so the
    // console showed "Applying inline style violates ... style-src 'self'" and the
    // editor rendered with none of its own styling. Nothing in this file could have
    // caught it: the violation is a runtime DOM write, not present in the build
    // output vite.config.ts's comment was reasoning about. The fix lives in
    // src/editor/CodeMirrorEditor.vue: CodeMirror is mounted into a shadow root,
    // where style-mod uses `adoptedStyleSheets` instead — constructed
    // programmatically, not "inline style" under CSP at all, so no CSP change was
    // needed or made.
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        // Fixed names, not content-hashed. See the module note above.
        entryFileNames: "boot.js",
        chunkFileNames: "boot-chunk-[name].js",
        assetFileNames: (asset) => (asset.names?.[0]?.endsWith(".css") ? "shell.css" : "assets/[name][extname]"),
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: false,
    include: ["src/**/*.test.ts"],
    // Task #131 (NFR-MNT-01, NFR-PRF-08): on a 4-core host under load, parallel workers starve
    // the vitest main process. Under vitest 3 the main process's reply to a worker's RPC then
    // missed a fixed 60 s window and the run exited 1 with `[vitest-worker]: Timeout calling
    // "onTaskUpdate"` although every test passed (6 of 10 runs under 4 busy loops). vitest 5
    // sets that window to none (`timeout: -1` in the worker RPC), so the failure's cause is
    // gone; the cap stays because it costs nothing (60.7 s with one worker against 63.1 s
    // with three, 561 tests) and keeps the workers off the main process's core. Note that
    // vitest 4 removed `test.poolOptions`: `poolOptions.forks.maxForks` is now ignored with
    // only a deprecation line, and the option is the top-level `maxWorkers`.
    // See docs/05-testing/test-plan.md, "Flake log".
    maxWorkers: 1,
    setupFiles: ["src/test/jsdom-shims.ts"],
  },
}));
