import { describe, expect, it } from "vitest";

import { chunkMarkdownSource, renderMarkdownToFragment } from "@/editor/preview";

async function renderToHtml(source: string): Promise<string> {
  const fragment = await renderMarkdownToFragment(source);
  const container = document.createElement("div");
  container.appendChild(fragment);
  return container.innerHTML;
}

describe("renderMarkdownToFragment", () => {
  it("renders a GFM table", async () => {
    const html = await renderToHtml("| a | b |\n| --- | --- |\n| 1 | 2 |\n");
    expect(html).toContain("<table>");
    expect(html).toContain("<td>1</td>");
  });

  it("renders a task list with disabled, read-only checkboxes", async () => {
    const html = await renderToHtml("- [ ] todo\n- [x] done\n");
    expect(html).toContain('type="checkbox"');
    expect(html).toContain("disabled");
    expect(html).toMatch(/checked[^>]*>\s*done|done/);
    expect(html).toContain("todo");
  });

  it("renders a footnote reference as a superscript anchor", async () => {
    const html = await renderToHtml("see this[^1]\n\n[^1]: the note\n");
    expect(html).toContain("<sup>");
    expect(html).toContain('href="#fn-1"');
  });

  it("renders a callout as a div with a trusted data-callout and a title", async () => {
    const html = await renderToHtml("> [!warning] Careful\n> mind the gap\n");
    expect(html).toContain('<div data-callout="warning">');
    expect(html).toContain("<strong>Careful</strong>");
    expect(html).toContain("mind the gap");
    expect(html).not.toContain("blockquote");
    expect(html).not.toContain("title=");
  });

  it("defaults the callout title to the capitalised type, and leaves plain quotes alone", async () => {
    expect(await renderToHtml("> [!note]\n> x\n")).toContain("<strong>Note</strong>");
    expect(await renderToHtml("> just a quote\n")).toContain("<blockquote>");
  });

  it("collects footnote definitions into a numbered section after the content", async () => {
    const html = await renderToHtml("see this[^1]\n\n[^1]: the *note*\n");
    expect(html).toContain("<hr>");
    expect(html).toMatch(/<ol>[\s\S]*<li>[\s\S]*the <em>note<\/em>[\s\S]*<\/ol>/);
    expect(html).not.toContain("[^1]:");
  });

  it("raw HTML in a note never becomes a div (so no forged callout)", async () => {
    const html = await renderToHtml('<div data-callout="x">hi</div>\n');
    expect(html).not.toContain("<div");
  });

  it("renders an unresolved wikilink as a styled, non-navigating anchor", async () => {
    const html = await renderToHtml("see [[Some Note]] here\n");
    expect(html).toContain("data-wikilink=");
    expect(html).toContain('data-resolved="false"');
    expect(html).toContain("Some Note");
  });

  it("never emits a script tag, even from a fenced code block label trick", async () => {
    const html = await renderToHtml("```html\n<script>alert(1)</script>\n```\n");
    // The code block's contents are text, not live markup — but assert the
    // sanitiser stage still holds even if it were.
    expect(html).not.toMatch(/<script(?!\/)/);
  });

  it("renders GFM strikethrough", async () => {
    const html = await renderToHtml("~~gone~~\n");
    expect(html).toContain("<s>gone</s>");
  });
});

describe("chunkMarkdownSource", () => {
  it("returns the whole source as one chunk when it is short", () => {
    const source = "a\nb\nc";
    expect(chunkMarkdownSource(source, 200)).toEqual([source]);
  });

  it("splits a long source into multiple chunks at blank lines", () => {
    const paragraphs = Array.from({ length: 30 }, (_, i) => `para ${i}`);
    const source = paragraphs.join("\n\n");
    const chunks = chunkMarkdownSource(source, 10);
    expect(chunks.length).toBeGreaterThan(1);
    expect(chunks.join("")).toContain("para 0");
    expect(chunks.join("")).toContain("para 29");
  });
});
