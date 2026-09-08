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

export default defineConfig({
  base: "./",
  plugins: [vue()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  build: {
    outDir: fileURLToPath(new URL("../crates/serat-app/webview", import.meta.url)),
    emptyOutDir: true,
    // No inline script and no CDN, so `tauri.conf.json`'s
    // "script-src 'self'; style-src 'self'" is satisfiable with no changes.
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
});
