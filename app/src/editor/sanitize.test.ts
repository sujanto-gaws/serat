// Hostile-input tests for the preview sanitiser (architecture.md §9.3, T-09).
// task #21's brief names these vectors explicitly: script, on*, javascript:,
// data:, iframe, object, download.

import { describe, expect, it } from "vitest";

import { sanitizeToHtml } from "@/editor/sanitize";

describe("sanitizeToHtml — hostile inputs", () => {
  it("drops <script> and its content entirely", () => {
    const out = sanitizeToHtml('<p>hi</p><script>alert(1)</script>');
    expect(out).not.toContain("<script");
    expect(out).not.toContain("alert(1)");
    expect(out).toContain("hi");
  });

  it("strips every on* event-handler attribute", () => {
    const out = sanitizeToHtml('<p onclick="alert(1)" onmouseover="x()">hi</p>');
    expect(out).not.toMatch(/on\w+\s*=/i);
    expect(out).toContain("hi");
  });

  it("drops a javascript: href", () => {
    const out = sanitizeToHtml('<a href="javascript:alert(1)">click</a>');
    expect(out).not.toContain("javascript:");
    expect(out).not.toMatch(/href\s*=/i);
  });

  it("never renders a data: image as <img>, whatever it contains", () => {
    const out = sanitizeToHtml('<img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=" alt="evil">');
    expect(out).not.toContain("<img");
    expect(out).not.toContain("data:");
    expect(out).toContain("not loaded");
  });

  it("drops a data: href on a link the same as an image src", () => {
    const out = sanitizeToHtml('<a href="data:text/html,<script>alert(1)</script>">click</a>');
    expect(out).not.toContain("data:");
    expect(out).not.toContain("<script");
  });

  it("drops <iframe> and its content, wherever it is nested", () => {
    const out = sanitizeToHtml('<div><p>before</p><iframe src="https://evil.example"></iframe><p>after</p></div>');
    expect(out).not.toContain("<iframe");
    expect(out).not.toContain("evil.example");
    expect(out).toContain("before");
    expect(out).toContain("after");
  });

  it("drops <object> and <embed>", () => {
    const out = sanitizeToHtml('<object data="evil.swf"></object><embed src="evil.swf">');
    expect(out).not.toContain("<object");
    expect(out).not.toContain("<embed");
  });

  it("strips a download attribute from a link", () => {
    const out = sanitizeToHtml('<a href="https://example.com/x" download="x">get</a>');
    expect(out).not.toContain("download");
    expect(out).toContain('href="https://example.com/x"');
  });

  it("strips a style attribute from any element", () => {
    const out = sanitizeToHtml('<p style="background:url(javascript:alert(1))">hi</p>');
    expect(out).not.toContain("style=");
  });

  it("never fetches a remote image; it becomes a labelled placeholder with the URL", () => {
    const out = sanitizeToHtml('<img src="https://example.com/cat.png" alt="a cat">');
    expect(out).not.toContain("<img");
    expect(out).toContain("a cat");
  });

  it("renders a serat-asset image as a plain <img>, nothing else", () => {
    const out = sanitizeToHtml('<img src="serat-asset://localhost/photo.png" alt="photo" width="300">');
    expect(out).toContain("<img");
    expect(out).toContain('src="serat-asset://localhost/photo.png"');
    expect(out).toContain('width="300"');
  });

  it("renders the Windows serat-asset origin the same way", () => {
    const out = sanitizeToHtml('<img src="http://serat-asset.localhost/photo.png" alt="photo">');
    expect(out).toContain("<img");
  });

  it("never renders inline SVG markup, even as a direct child", () => {
    const out = sanitizeToHtml('<svg onload="alert(1)"><circle r="5"/></svg>');
    expect(out).not.toContain("<svg");
    expect(out).not.toContain("onload");
  });

  it("keeps ordinary GFM markup (tables, task lists, links) intact", () => {
    const out = sanitizeToHtml(
      '<table><tr><th>a</th></tr><tr><td>b</td></tr></table>' +
        '<ul><li><input type="checkbox" checked>done</li></ul>' +
        '<a href="https://example.com">ok</a>',
    );
    expect(out).toContain("<table>");
    expect(out).toContain("<th>a</th>");
    expect(out).toContain('type="checkbox"');
    expect(out).toContain("checked");
    expect(out).toContain("disabled"); // task-list checkboxes are always read-only
    expect(out).toContain('href="https://example.com"');
  });

  it("marks an external link so the mounting component can intercept its click", () => {
    const out = sanitizeToHtml('<a href="https://example.com">ok</a>');
    expect(out).toContain('data-external="true"');
  });

  it("does not mark an in-note anchor as external", () => {
    const out = sanitizeToHtml('<a href="#heading">jump</a>');
    expect(out).toContain('data-external="false"');
  });

  it("unwraps an unrecognised tag but keeps its safe text content", () => {
    const out = sanitizeToHtml("<form><p>keep me</p></form>");
    expect(out).not.toContain("<form");
    expect(out).toContain("keep me");
  });

  it("survives a deeply nested hostile payload without throwing", () => {
    const payload =
      '<div><p><a href="javascript:alert(1)"><img src="data:image/png;base64,AAAA" onerror="x()"></a>' +
      '<script>document.cookie</script><iframe srcdoc="<script>1</script>"></iframe></p></div>';
    expect(() => sanitizeToHtml(payload)).not.toThrow();
    const out = sanitizeToHtml(payload);
    expect(out).not.toContain("<script");
    expect(out).not.toContain("<iframe");
    expect(out).not.toContain("javascript:");
    expect(out).not.toContain("onerror");
    expect(out).not.toContain("<img");
  });

  it("drops class everywhere except the exact task-list class on <li>", () => {
    const out = sanitizeToHtml('<p class="x">a</p><ul><li class="serat-task-list-item">t</li><li class="evil">u</li></ul>');
    expect(out).not.toContain('class="x"');
    expect(out).not.toContain("evil");
    expect(out).toContain('<li class="serat-task-list-item">');
  });

  it("never copies an incoming data-* attribute (no forging the renderer's markers)", () => {
    const out = sanitizeToHtml(
      '<a href="#x" data-wikilink="forged" data-external="true" data-resolved="true">a</a><span data-embed-placeholder="y">s</span>',
    );
    expect(out).not.toContain("data-wikilink");
    expect(out).not.toContain("data-resolved");
    expect(out).not.toContain("data-embed-placeholder");
    expect(out).toContain('data-external="false"'); // the sanitiser's own, from the href
  });

  it("adds wikilink/embed markers only for the matching nonce, after sanitising", () => {
    const html =
      '<a href="#serat-n0nce:wikilink:Foo%20Bar">Foo</a>' +
      '<a href="#serat-other:wikilink:Evil">E</a>' +
      '<span title="serat-n0nce:embed:pic.png">e</span>';
    const out = sanitizeToHtml(html, document, { markerNonce: "n0nce" });
    expect(out).toContain('data-wikilink="Foo Bar"');
    expect(out).toContain('data-resolved="false"');
    expect(out).toContain('data-embed-placeholder="pic.png"');
    expect(out.match(/data-wikilink/g)).toHaveLength(1);
    expect(sanitizeToHtml(html)).not.toContain("data-wikilink");
  });

  it("drops script and style in a foreign namespace along with their text", () => {
    const out = sanitizeToHtml(
      "<p>a</p><svg><script>alert(1)</script><style>p{}</style></svg><math><style>x</style></math>",
    );
    expect(out).not.toContain("alert");
    expect(out).not.toContain("p{}");
    expect(out).not.toMatch(/<script|<style|<svg|<math/i);
    expect(out).toContain("<p>a</p>");
  });
});
