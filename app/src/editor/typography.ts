// Task #104 (FR-EDT-01, FR-EDT-02, FR-EDT-10, NFR-PRF-02; ADR-0006; docs/03-design/ui/components.md
// section 3, screens/editor.md): the editor's type and colour. Everything here is a style rule over
// nodes and lines the parser and `reveal.ts` already produce; nothing runs on the input path beyond
// what CodeMirror's own time-sliced highlighter does, and no rule changes a line's height between
// "rendered" and "revealed" except the documented font switch itself.
//
// Three layers, all reading the `--serat-*` tokens from `src/tokens.css` (custom properties inherit
// into the editor's shadow root, so the same block styles shell and editor):
//   1. `seratHighlightStyle` maps highlight tags to classes (`serat-tok-*`), so a style rule can be
//      overridden by context instead of fighting an inline style.
//   2. `typographyTheme` is the CSS for those classes plus the surface: serif prose and Bricolage
//      headings in live mode, `mono-source` everywhere in split-source mode and on revealed lines.
//   3. `modeClass` marks `.cm-content` as `serat-live` or `serat-source`, the one switch the rules
//      above key on (FR-EDT-10: no hide/reveal and `mono-source` throughout in source mode).
// A line is `mono-source` when `reveal.ts` tags it `serat-mono-source` (a revealed line that has syntax
// to show, or a code line) or when the whole editor is in source mode.

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

import { seratTags } from "@/editor/mdExtensions";

export type EditorMode = "live" | "source";

/** The class names this module and `reveal.ts` share. */
export const MONO_SOURCE_LINE = "serat-mono-source";
export const CODE_BLOCK_LINE = "serat-code-line";

export const seratHighlightStyle = HighlightStyle.define([
  { tag: tags.heading1, class: "serat-tok-h1" },
  { tag: tags.heading2, class: "serat-tok-h2" },
  { tag: tags.heading3, class: "serat-tok-h3" },
  { tag: [tags.heading4, tags.heading5, tags.heading6], class: "serat-tok-h4" },
  { tag: tags.strong, class: "serat-tok-strong" },
  { tag: tags.emphasis, class: "serat-tok-em" },
  { tag: tags.strikethrough, class: "serat-tok-strike" },
  { tag: tags.monospace, class: "serat-tok-code" },
  { tag: tags.quote, class: "serat-tok-quote" },
  { tag: tags.link, class: "serat-tok-link" },
  { tag: tags.url, class: "serat-tok-url" },
  { tag: seratTags.embed, class: "serat-tok-embed" },
  { tag: seratTags.footnoteRef, class: "serat-tok-footnote" },
  { tag: seratTags.highlight, class: "serat-tok-mark" },
  { tag: [seratTags.blockRef, seratTags.field], class: "serat-tok-machine" },
  { tag: tags.processingInstruction, class: "serat-tok-marker" },
  { tag: tags.labelName, class: "serat-tok-label" },
]);

const HEADING_LIVE = ".serat-live .cm-line:not(.serat-mono-source)";

const heading = (size: string, line: string, extra: Record<string, string> = {}) => ({
  fontFamily: "var(--serat-font-ui)",
  fontSize: `var(--serat-size-${size})`,
  lineHeight: `var(--serat-line-${size}, ${line})`,
  fontWeight: "700",
  ...extra,
});

export const typographyTheme = EditorView.baseTheme({
  "&": { backgroundColor: "var(--serat-surface)", color: "var(--serat-text-primary)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--serat-font-prose)" },
  ".cm-content": { caretColor: "var(--serat-accent-interactive)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--serat-accent-interactive)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground": {
    background: "var(--serat-surface-selected)",
  },
  ".cm-gutters": {
    backgroundColor: "var(--serat-surface)",
    color: "var(--serat-text-tertiary)",
    border: "none",
    fontFamily: "var(--serat-font-mono)",
    fontSize: "var(--serat-size-micro)",
  },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "transparent" },
  ".cm-placeholder": { color: "var(--serat-text-tertiary)" },

  // Rendered text (live mode): body-serif, held to the prose measure and centred (components.md section 3).
  ".cm-content.serat-live": {
    fontFamily: "var(--serat-font-prose)",
    fontSize: "var(--serat-size-body-serif)",
    lineHeight: "var(--serat-line-body-serif)",
    maxWidth: "var(--serat-measure-prose)",
    margin: "0 auto",
  },
  // Source mode (the split fallback's left pane): mono-source throughout.
  ".cm-content.serat-source": {
    fontFamily: "var(--serat-font-mono)",
    fontSize: "var(--serat-size-mono-source)",
    lineHeight: "var(--serat-line-mono-source)",
  },
  // A revealed line, or a code line: mono-source, content in text-primary.
  ".cm-line.serat-mono-source": {
    fontFamily: "var(--serat-font-mono)",
    fontSize: "var(--serat-size-mono-source)",
    lineHeight: "var(--serat-line-mono-source)",
  },
  ".cm-line.serat-code-line": { backgroundColor: "var(--serat-surface-sunken)" },

  // Headings: Bricolage at heading-lg/md/sm, only on rendered lines. On a revealed line, or in source mode,
  // the size and family stay mono-source and only the weight and colour say "heading".
  ".serat-tok-h1, .serat-tok-h2, .serat-tok-h3, .serat-tok-h4": { fontWeight: "700" },
  [`${HEADING_LIVE} .serat-tok-h1`]: heading("heading-lg", "34px", { letterSpacing: "-0.02em" }),
  [`${HEADING_LIVE} .serat-tok-h2`]: heading("heading-md", "29px", { letterSpacing: "-0.015em" }),
  [`${HEADING_LIVE} .serat-tok-h3, ${HEADING_LIVE} .serat-tok-h4`]: heading("heading-sm", "26px", {
    letterSpacing: "-0.01em",
  }),

  ".serat-tok-strong": { fontWeight: "700" },
  ".serat-tok-em": { fontStyle: "italic" },
  ".serat-tok-strike": { textDecoration: "line-through" },
  ".serat-tok-quote": { color: "var(--serat-text-secondary)" },
  ".serat-tok-link, .serat-tok-url": { color: "var(--serat-link)" },
  ".serat-tok-mark": { backgroundColor: "var(--serat-accent-secondary-bg)", borderRadius: "var(--serat-radius-sm)" },
  ".serat-tok-footnote": { fontSize: "0.8em", verticalAlign: "super", lineHeight: "0" },

  // Inline code and fenced code: mono-source on a sunken fill, no border.
  ".serat-tok-code": {
    fontFamily: "var(--serat-font-mono)",
    fontSize: "var(--serat-size-mono-source)",
    backgroundColor: "var(--serat-surface-sunken)",
    borderRadius: "var(--serat-radius-sm)",
  },
  ".serat-code-line .serat-tok-code": { backgroundColor: "transparent", borderRadius: "0" },

  // Always-visible machine syntax (`^id`, `key::`): mono-source, text-tertiary, never hidden.
  ".serat-tok-machine": {
    fontFamily: "var(--serat-font-mono)",
    fontSize: "var(--serat-size-mono-source)",
    color: "var(--serat-text-tertiary)",
  },
  // Revealed marker characters (`**`, `#`, `[[`): structure in text-secondary, content stays text-primary.
  ".serat-tok-marker, .serat-tok-label": { color: "var(--serat-text-secondary)" },
});

/** Marks `.cm-content` with the mode the rules above key on. Reconfigured (not rebuilt) when
 * `editor.live_preview` toggles. */
export function modeClass(mode: EditorMode): Extension {
  return EditorView.contentAttributes.of({ class: mode === "live" ? "serat-live" : "serat-source" });
}

/** The highlighter and the theme: installed in both modes (source mode is "plain syntax-highlighted
 * Markdown", screens/editor.md). */
export function typographyExtensions(): Extension[] {
  return [syntaxHighlighting(seratHighlightStyle), typographyTheme];
}
