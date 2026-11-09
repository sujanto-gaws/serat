// architecture.md §9.3, "Decorations": "Parse. Lezer markdown through
// @codemirror/lang-markdown with the GFM extensions (tables, task lists,
// strikethrough, autolinks). Custom inline extensions for what Lezer
// lacks: footnotes, [[wikilink]], ![[embed]], ^block-id, key:: value, and
// ==highlight==. The parse is incremental, and it is time-sliced by
// CodeMirror, so a long note never parses on one keystroke."
//
// Why this does not call `markdown()` (NFR-PRF-01, §9.3 command registry):
//
//   - `markdown()` imports `@codemirror/lang-html` unconditionally (its
//     default `htmlTagLanguage`), which drags the JS and CSS grammars into
//     the startup bundle: boot.js grew 347 -> 592 KB (+78% gzip) with it.
//     The Markdown language itself needs none of that, so this module builds
//     the `Language` from `commonmarkLanguage.parser` directly (the same
//     thing `markdown()` does internally, minus HTML nested parsing). HTML
//     blocks stay raw source text, which is also what C-01 wants. Nested
//     code-fence languages are not loaded at startup; when they are wanted
//     they belong behind a `LanguageDescription` whose `load()` is a dynamic
//     `import()`, i.e. a lazy chunk.
//   - `markdown()` defaults `addKeymap: true`, which installs its own
//     high-precedence keymap (Enter, Backspace) that bypasses the command
//     registry. Nothing here adds a keymap; the two Markdown commands are
//     exported for `codemirrorSetup.ts` to register under Serat ids, so
//     `keymap.of` still appears in exactly one place (`CodeMirrorEditor.test.ts`
//     asserts the built state carries exactly one keymap entry).
//   - The paste-URL-as-link handler and the HTML-tag completion that
//     `markdown()` also bundles are not part of any P1 requirement and are
//     omitted with the HTML grammar they depend on.

import { commonmarkLanguage, deleteMarkupBackward, insertNewlineContinueMarkup } from "@codemirror/lang-markdown";
import { Language, LanguageSupport } from "@codemirror/language";
import { GFM, type MarkdownParser } from "@lezer/markdown";

import { seratInlineSyntax } from "@/editor/mdExtensions";

export { deleteMarkupBackward, insertNewlineContinueMarkup };

/** The one Markdown language configuration used everywhere a Lezer parse of
 * a note happens: the live editor and the reveal-rule decorator. (The
 * preview renderer — split mode and embeds — uses `markdown-it` instead,
 * per §9.3's "three Markdown parsers" note; this module is not that one.) */
export function seratMarkdown(): LanguageSupport {
  const parser = (commonmarkLanguage.parser as MarkdownParser).configure([GFM, seratInlineSyntax]);
  return new LanguageSupport(new Language(commonmarkLanguage.data, parser, [], "markdown"));
}
