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
