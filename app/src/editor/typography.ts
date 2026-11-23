// Task #104 (FR-EDT-01, FR-EDT-02, FR-EDT-10, NFR-PRF-02; ADR-0006; docs/03-design/ui/screens/editor.md,
// Live-preview typography; components.md section 3): which classes the editor's syntax carries. The values
// behind those classes live in the design CSS (livePreview.css, adopted by editorStyles.ts) and read the
// `--serat-*` tokens; nothing here chooses a size or a colour.
//
//   - `seratHighlightStyle` maps highlight tags to classes (`serat-strong`, `serat-marker`, ...). The
//     highlighter runs inside CodeMirror's own time-sliced pass; nothing is added to the input path.
//   - `reveal.ts` adds the line classes (`serat-h1`, `serat-quote`, `serat-code-line`, ...) and the visible
//     markers, from the same viewport-bound tree walk that hides the markers.
//   - `modeClass` puts `serat-live` or `serat-source` on `.cm-editor`, the one switch the CSS keys on
//     (FR-EDT-10: source mode hides nothing and resizes nothing).

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { tags } from "@lezer/highlight";

import { seratTags } from "@/editor/mdExtensions";

export type EditorMode = "live" | "source";

export const seratHighlightStyle = HighlightStyle.define([
  { tag: tags.strong, class: "serat-strong" },
  { tag: tags.emphasis, class: "serat-em" },
  { tag: tags.strikethrough, class: "serat-strike" },
  { tag: tags.monospace, class: "serat-code" },
  { tag: tags.url, class: "serat-url" },
  // Wikilinks and embeds are styled by links.ts (resolved / unresolved, once the index has answered), so
  // they take no generic link class of their own; these two come before `tags.link` so they win over it.
  { tag: [seratTags.wikiLink, seratTags.embed], class: "serat-wikilink" },
  { tag: seratTags.footnoteRef, class: "serat-footnote" },
  { tag: tags.link, class: "serat-md-link" },
  { tag: seratTags.highlight, class: "serat-highlight" },
  { tag: [seratTags.blockRef, seratTags.field], class: "serat-machine" },
  { tag: tags.processingInstruction, class: "serat-marker" },
  { tag: tags.labelName, class: "serat-code-info" },
]);

/** `serat-live` or `serat-source` on `.cm-editor`. Reconfigured (not rebuilt) when `editor.live_preview`
 * toggles. */
export function modeClass(mode: EditorMode): Extension {
  return [
    EditorView.editorAttributes.of({ class: mode === "live" ? "serat-live" : "serat-source" }),
    // No gutter in live mode: the reveal rule is the only line-level signal (editor.md, column and rhythm).
    // Source mode is the faithful view of the bytes, so it keeps line numbers.
    mode === "source" ? lineNumbers() : [],
  ];
}

/** The highlighter: installed in both modes (source mode is "plain syntax-highlighted Markdown"). */
export function typographyExtensions(): Extension[] {
  return [syntaxHighlighting(seratHighlightStyle)];
}
