// architecture.md §9.3, "Decorations": "Parse. Lezer markdown through
// @codemirror/lang-markdown with the GFM extensions (tables, task lists,
// strikethrough, autolinks). Custom inline extensions for what Lezer
// lacks: footnotes, [[wikilink]], ![[embed]], ^block-id, key:: value, and
// ==highlight==. The parse is incremental, and it is time-sliced by
// CodeMirror, so a long note never parses on one keystroke."

import { markdown } from "@codemirror/lang-markdown";
import type { LanguageSupport } from "@codemirror/language";
import { GFM } from "@lezer/markdown";

import { seratInlineSyntax } from "@/editor/mdExtensions";

/** The one Markdown language configuration used everywhere a Lezer parse of
 * a note happens: the live editor and the reveal-rule decorator. (The
 * preview renderer — split mode and embeds — uses `markdown-it` instead,
 * per §9.3's "three Markdown parsers" note; this module is not that one.) */
export function seratMarkdown(): LanguageSupport {
  return markdown({
    extensions: [GFM, seratInlineSyntax],
    // Obsidian syntax Serat does not render is left as plain source text
    // (C-01, NFR-CMP-01) — HTML-tag handling is CommonMark's own, and left
    // at its default here since Obsidian vaults sometimes carry raw HTML;
    // nothing about it invents new syntax on save.
  });
}
