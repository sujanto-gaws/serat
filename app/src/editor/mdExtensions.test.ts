// Regression coverage for task #40's finding: the wikilink/embed inline
// parser must run ahead of every standard parser ("before: Escape"), not
// only "before: Image" — the standard `Link` parser happily claims the
// *inner* `[Note Name]` of `[[Note Name]]` as an ordinary link if it gets a
// look first, silently turning every wikilink into an unresolved plain
// link with two stray brackets either side.

import { EditorState } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import { describe, expect, it } from "vitest";

import { seratMarkdown } from "@/editor/markdownLanguage";

function nodeNamesAt(doc: string): Array<{ name: string; from: number; to: number }> {
  const state = EditorState.create({ doc, extensions: [seratMarkdown()] });
  const found: Array<{ name: string; from: number; to: number }> = [];
  syntaxTree(state).iterate({
    enter: (node) => {
      found.push({ name: node.type.name, from: node.from, to: node.to });
    },
  });
  return found;
}

describe("mdExtensions node priority", () => {
  it("parses [[Note Name]] as a single SeratWikiLink node, not a standard Link", () => {
    const names = nodeNamesAt("[[Note Name]]");
    expect(names.some((n) => n.name === "SeratWikiLink" && n.from === 0 && n.to === 13)).toBe(true);
    expect(names.some((n) => n.name === "Link")).toBe(false);
  });

  it("parses ![[image.png]] as a single SeratEmbed node, not a standard Image", () => {
    const names = nodeNamesAt("![[image.png]]");
    expect(names.some((n) => n.name === "SeratEmbed" && n.from === 0 && n.to === 14)).toBe(true);
    expect(names.some((n) => n.name === "Image")).toBe(false);
  });

  it("still parses an ordinary Markdown link normally", () => {
    const names = nodeNamesAt("[text](https://example.com)");
    expect(names.some((n) => n.name === "Link")).toBe(true);
    expect(names.some((n) => n.name === "SeratWikiLink")).toBe(false);
  });

  it("a wikilink followed immediately by more text still resolves correctly", () => {
    const names = nodeNamesAt("see [[Target]] here");
    const wikilink = names.find((n) => n.name === "SeratWikiLink");
    expect(wikilink).toBeDefined();
    expect(wikilink?.from).toBe(4);
    expect(wikilink?.to).toBe(14);
  });
});
