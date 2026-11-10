// Which surfaces keep the webview's NATIVE context menu, and which never show it (NFR-USE-01, NFR-CMP-01).
//
// Spec position (components.md, file-tree.md, editor.md): the only Serat context menu is the file tree's.
// editor.md, the backlinks pane and the search panel specify none, so none is invented here.
//
// WHY this exists, and why it is not cosmetic: the webview's native menu offers "Open Link in New Window",
// "View Frame Source" and similar items that reach a window by a path outside every navigation guard
// (T-07; crates/serat-app/src/lib.rs, task #37). The Rust side disables the native menu ONLY on Linux/BSD
// release builds (lib.rs:496-517: WebKitGTK; WebView2's `AreDefaultContextMenusEnabled` and WKWebView's
// WKUIDelegate are separate platform work, #37's follow-ups). Until those land, THIS policy is the only
// suppression on Windows and macOS, in release and debug alike.
//
//   - a plain text field (an `<input>` that takes text, a `<textarea>`) or editable text (the editor's
//     contenteditable, the naming prompt) keeps the native menu: cut / copy / paste are what it is for;
//   - everywhere else the native menu is suppressed. The tree's own menu is opened by its handlers, which
//     run first (bubbling) and prevent the default.
//
// Deliberate P1 gaps: read-only text (the preview pane, backlinks snippets, the Compare view) has no native
// Copy item; Cmd/Ctrl-C still works. And the policy installs in debug builds too (lib.rs carves debug out
// on Linux so a developer can right-click "Inspect Element"): one rule in every build beats a dev build that
// behaves differently from the shipped one, and DevTools stays reachable by keyboard (F12 / Ctrl-Shift-I).
//
// WINDOWS MANUAL CHECK (#77): in WebView2, Menu key and Shift-F10 fire a native `contextmenu` on KEY-UP
// (macOS / WebKit and Chromium on Linux differ). The tree container handles that event (FileTree.vue
// `onTreeContextMenu`); verify on a real Windows window that (1) pressing Menu opens exactly one Serat menu
// for the focused row, (2) no native menu flashes first, (3) the same keys inside the naming prompt or the
// editor still show the NATIVE menu, (4) right-clicking a link in the preview pane shows NO native menu
// (no "Open Link in New Window"), and (5) any NEW Serat menu surface added later gets the same key-up
// handling and a case in `nativeMenu.test.ts`.

const TEXT_INPUT_TYPES = new Set(["text", "search", "url", "tel", "email", "password", "number", ""]);

/** True when `target` is somewhere the native context menu is wanted. */
export function allowsNativeMenu(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const field = target.closest("input, textarea, [contenteditable]");
  if (!field) return false;
  if (field instanceof HTMLInputElement) return TEXT_INPUT_TYPES.has((field.getAttribute("type") ?? "").toLowerCase());
  if (field instanceof HTMLTextAreaElement) return true;
  // `contenteditable="false"` (the read-only editor before a note loads) is not editable text.
  return (field as HTMLElement).getAttribute("contenteditable") !== "false";
}

/**
 * Installs the policy on `root` (bubbling phase, so a Serat menu's own handler has already prevented the
 * default and its `defaultPrevented` checks see the untouched event). Returns the uninstall function.
 */
export function installNativeMenuPolicy(root: Document | HTMLElement = document): () => void {
  const onContextMenu = (event: Event): void => {
    // `composedPath()[0]` is the real target even inside the editor's shadow root (the document sees
    // only the host element there).
    const target = event.composedPath()[0] ?? event.target;
    if (!allowsNativeMenu(target)) event.preventDefault();
  };
  root.addEventListener("contextmenu", onContextMenu);
  return () => root.removeEventListener("contextmenu", onContextMenu);
}
