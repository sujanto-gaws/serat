// architecture.md §9.3, "Decorations": "Custom inline extensions for what
// Lezer lacks: footnotes, [[wikilink]], ![[embed]], ^block-id, key:: value,
// and ==highlight==." `@codemirror/lang-markdown` + `@lezer/markdown`'s GFM
// bundle cover tables, task lists, strikethrough and autolinks; this module
// is the rest.
//
// Each construct is one flat node — `SeratWikiLink`, `SeratEmbed`,
// `SeratHighlight`, `SeratBlockRef`, `SeratField`, `SeratFootnoteRef` —
// spanning its markers and content together, rather than a tree of marker
// + content child nodes. `reveal.ts`'s decorator derives which characters
// within a node's span are "marker" characters from the construct's own
// fixed shape (`[[`/`]]`, `==`/`==`, a leading `^`, a leading `key::`) at
// decoration time, which keeps this grammar small: one node type is enough
// information for the reveal rule to hide exactly the marker runs, and
// nesting more structure than the reveal rule needs would only be overhead
// here.

import { Tag, tags } from "@lezer/highlight";
import type { InlineContext, InlineParser, MarkdownConfig } from "@lezer/markdown";

const CH_BANG = 33; // '!'
const CH_LBRACKET = 91; // '['
const CH_CARET = 94; // '^'
const CH_EQUALS = 61; // '='

/** `[[target]]` and `![[target]]` (FR-EDT-03: links and embeds). Runs before
 * the standard `Image`/`Link` parsers so `![[` is never mistaken for the
 * start of a Markdown image. */
const wikiLinkAndEmbed: InlineParser = {
  name: "SeratWikiLinkEmbed",
  // Ahead of every standard parser, not only "Image": the standard "Link"
  // parser also triggers on a bare `[` and — task #40's investigation found
  // this the hard way — happily claims the *inner* `[Note Name]` of a
  // `[[Note Name]]` wikilink as an ordinary link before this parser ever
  // gets a look, if this only ran "before Image". Running first means
  // nothing else ever sees the leading `[`/`![[` of one of these
  // constructs.
  before: "Escape",
  parse(cx: InlineContext, next: number, pos: number): number {
    const isEmbed = next === CH_BANG;
    const bracketStart = isEmbed ? pos + 1 : pos;
    if (cx.char(bracketStart) !== CH_LBRACKET || cx.char(bracketStart + 1) !== CH_LBRACKET) {
      return -1;
    }
    const contentStart = bracketStart + 2;
    const closeRel = cx.text.indexOf("]]", contentStart - cx.offset);
    if (closeRel < 0) {
      return -1;
    }
    const end = cx.offset + closeRel + 2;
    // A bare `[[]]`/`![[]]` (no target) is not a link worth rendering
    // specially — leave it as plain text for the standard parsers.
    if (end - 2 === contentStart) {
      return -1;
    }
    return cx.addElement(cx.elt(isEmbed ? "SeratEmbed" : "SeratWikiLink", pos, end));
  },
};

/** `==highlighted text==`. */
const highlight: InlineParser = {
  name: "SeratHighlight",
  parse(cx: InlineContext, next: number, pos: number): number {
    if (next !== CH_EQUALS || cx.char(pos + 1) !== CH_EQUALS) {
      return -1;
    }
    const contentStart = pos + 2;
    const closeRel = cx.text.indexOf("==", contentStart - cx.offset);
    if (closeRel < 0) {
      return -1;
    }
    const end = cx.offset + closeRel + 2;
    if (end - 2 === contentStart) {
      return -1; // "====" — nothing between the delimiters
    }
    return cx.addElement(cx.elt("SeratHighlight", pos, end));
  },
};

/** `^block-id`, only when it is the rest of the block (a block reference
 * always sits at the end of the paragraph/line it marks, per Obsidian's own
 * convention) — components.md §3: "always visible (never hidden by the
 * reveal rule — they are structurally load-bearing, not typographic
 * decoration)", so `reveal.ts` must be able to tell this node apart from
 * the ones that *do* hide, which its distinct node name does. */
const blockRef: InlineParser = {
  name: "SeratBlockRef",
  parse(cx: InlineContext, next: number, pos: number): number {
    if (next !== CH_CARET) {
      return -1;
    }
    const localStart = pos - cx.offset;
    const rest = cx.text.slice(localStart + 1);
    const match = /^[a-zA-Z0-9-]+/.exec(rest);
    if (!match) {
      return -1;
    }
    const idEndLocal = localStart + 1 + match[0].length;
    if (cx.text.slice(idEndLocal).trim().length > 0) {
      return -1; // not at the end of the block — not a block reference
    }
    return cx.addElement(cx.elt("SeratBlockRef", pos, cx.offset + idEndLocal));
  },
};

/** `key:: value` inline fields (Obsidian/Dataview convention), only when
 * `key::` opens the block's inline content — components.md §3's "always
 * visible" rule applies to this marker too. */
const inlineField: InlineParser = {
  name: "SeratField",
  parse(cx: InlineContext, _next: number, pos: number): number {
    if (pos !== cx.offset) {
      return -1;
    }
    const match = /^[A-Za-z_][\w-]*::(?=\s|$)/.exec(cx.text);
    if (!match) {
      return -1;
    }
    return cx.addElement(cx.elt("SeratField", pos, cx.offset + match[0].length));
  },
};

/** `[^label]` footnote references. Footnote *definitions* (`[^label]:
 * text`, block-level, always at column 0) are left as plain paragraph text
 * for P1 — they still round-trip byte-for-byte (C-01), they are simply not
 * specially decorated yet; the reference form is the one that appears
 * inline in running prose and is what the reveal rule most needs to hide. */
const footnoteRef: InlineParser = {
  name: "SeratFootnoteRef",
  before: "Link",
  parse(cx: InlineContext, next: number, pos: number): number {
    if (next !== CH_LBRACKET || cx.char(pos + 1) !== CH_CARET) {
      return -1;
    }
    const localStart = pos - cx.offset;
    const rest = cx.text.slice(localStart + 2);
    const match = /^[^\s[\]]+\]/.exec(rest);
    if (!match) {
      return -1;
    }
    const end = cx.offset + localStart + 2 + match[0].length;
    return cx.addElement(cx.elt("SeratFootnoteRef", pos, end));
  },
};

/** Highlight tags for the Serat nodes, so `typography.ts`'s `HighlightStyle` can style them. The link-like
 * ones are sub-tags of `tags.link`: a style written for plain links applies to them too, and a more
 * specific rule can still tell them apart. `blockRef` and `field` are plain tags with no parent (they
 * are always-visible machine syntax, components.md section 3), so no generic style leaks onto them. */
export const seratTags = {
  wikiLink: Tag.define("wikiLink", tags.link),
  embed: Tag.define("embed", tags.link),
  highlight: Tag.define("highlight"),
  blockRef: Tag.define("blockRef"),
  field: Tag.define("field"),
  footnoteRef: Tag.define("footnoteRef", tags.link),
};

/** The one `MarkdownExtension` this module exports — pass it to
 * `markdown({ extensions: [gfmAndSeratInline] })` alongside `@lezer/markdown`'s
 * `GFM` bundle. */
export const seratInlineSyntax: MarkdownConfig = {
  defineNodes: [
    { name: "SeratWikiLink", style: seratTags.wikiLink },
    { name: "SeratEmbed", style: seratTags.embed },
    { name: "SeratHighlight", style: seratTags.highlight },
    { name: "SeratBlockRef", style: seratTags.blockRef },
    { name: "SeratField", style: seratTags.field },
    { name: "SeratFootnoteRef", style: seratTags.footnoteRef },
  ],
  parseInline: [wikiLinkAndEmbed, highlight, blockRef, inlineField, footnoteRef],
};

/** Node type names this module adds — used by `reveal.ts` to know which
 * nodes to look at, and by `reveal.ts`/`preview.ts` to know which of them
 * are exempt from the hide rule (block refs and inline fields). */
export const SERAT_REVEALABLE_NODE_TYPES = new Set([
  "SeratWikiLink",
  "SeratEmbed",
  "SeratHighlight",
  "SeratFootnoteRef",
]);

/** components.md §3: never hidden by the reveal rule. */
export const SERAT_ALWAYS_VISIBLE_NODE_TYPES = new Set(["SeratBlockRef", "SeratField"]);
