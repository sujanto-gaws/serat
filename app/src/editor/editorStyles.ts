// Task #104: the editor's stylesheet reaches its shadow root, not the document. CodeMirror lives in a shadow
// root (CodeMirrorEditor.vue: its runtime CSS needs `adoptedStyleSheets` to satisfy the CSP's
// `style-src 'self'`), so a stylesheet imported by main.ts cannot see `.cm-editor`. The design CSS
// (docs/03-design/ui/live-preview.css, plus the supplement for what that file leaves out) is therefore
// adopted into the same root, the same way CodeMirror adopts its own: a constructable stylesheet, which the
// CSP permits and an inline <style> element would not. The custom properties it reads (`--serat-*`, from
// styles/tokens.css) inherit into the root from :root.

import liveCss from "@/editor/livePreview.css?raw";
import supplementCss from "@/editor/livePreviewSupplement.css?raw";

export const EDITOR_CSS = `${liveCss}\n${supplementCss}`;

let sheet: CSSStyleSheet | null = null;

/** Adopts the editor stylesheet into `root`. Returns false where the platform has no constructable
 * stylesheets (jsdom): nothing is injected as a <style> element instead, because that is exactly what the
 * CSP forbids; unstyled is the honest fallback. */
export function adoptEditorStyles(root: ShadowRoot): boolean {
  if (typeof CSSStyleSheet === "undefined" || !("adoptedStyleSheets" in root)) return false;
  try {
    if (!sheet) {
      sheet = new CSSStyleSheet();
      sheet.replaceSync(EDITOR_CSS);
    }
    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
    return true;
  } catch {
    return false;
  }
}
