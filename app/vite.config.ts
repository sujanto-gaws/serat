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

export default defineConfig(({ mode }) => ({
  base: "./",
  plugins: [vue()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
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
  },
}));
