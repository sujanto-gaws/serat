// Shims every test file gets (vite.config.ts `test.setupFiles`). jsdom has no `document.execCommand`, and
// CodeMirror's Safari path (jsdom reports navigator.vendor as Apple) calls it when focusing a selection,
// including from a jsdom selectionchange timer that can fire after a test file's last test: without the
// shim that surfaced as an intermittent "Errors 1 error" on an otherwise green run.
// Some files run in the node environment (`// @vitest-environment node`), where there is no document.
if (typeof document !== "undefined") (document as unknown as { execCommand: () => boolean }).execCommand = () => false;
