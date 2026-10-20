// The preview renderer's sanitiser (architecture.md §9.3, "The preview
// renderer, and what it may not do", T-09). Split preview and every embed
// share this: a strict allowlist over parsed DOM, never a regex over the
// HTML string (a regex sanitiser is the classic way to miss an obfuscated
// vector — this walks the parsed tree instead, which cannot be tricked by
// how the bytes were spelled).
//
// What it must hold, verbatim from §9.3:
//   "It keeps no <script>, no event-handler attributes, no style
//   attributes, no iframe, object or embed, no download attribute, and no
//   javascript: or data: URL of any kind. An inline data:image/* renders as
//   a labelled placeholder, like a remote image. ... It loads no remote
//   resource: a remote image renders as a labelled placeholder with its
//   URL, and is never fetched."
//
// Images specifically follow components.md §4's rule, restated once there
// and enforced here: "a plain <img src="serat-asset://…">, and nothing
// else — never <object>, <iframe>, inline SVG markup, or a sanitised HTML
// string inserted via v-html/innerHTML." This module produces the `<img>`
// case only for a `serat-asset:`/`http://serat-asset.localhost` source;
// every other image source becomes the labelled placeholder, never an
// `<img>` pointed at a remote or `data:` URL.
//
import { isSeratAssetUrl } from "@/editor/assetUrl";

// Incoming `data-*` attributes are never copied: they are the renderer's own
// channel to the mounting component (`data-external`, `data-wikilink`, ...),
// so letting a document supply them would let it forge that channel. The
// sanitiser sets `data-external` itself, and `preview.ts` asks for the
// wikilink/footnote/embed markers by emitting an unguessable per-render
// nonce-prefixed href/title that is rewritten into the real markers only
// *after* sanitising (`applyTrustedMarkers`).
//
// `class` is dropped everywhere except one exact value on `<li>`
// (`serat-task-list-item`), because styling hooks are not content.
//
// External-link click interception (never navigate the webview; open only
// on an explicit click, via the system opener) is a DOM event-listener
// concern, not a sanitisation one, and lives in the component that mounts
// this output (`Preview.vue`) — see that file.

/** Tags that pass through untouched (after their attributes are filtered). */
const ALLOWED_TAGS = new Set([
  "P",
  "BR",
  "HR",
  "STRONG",
  "EM",
  "DEL",
  "S",
  "CODE",
  "PRE",
  "BLOCKQUOTE",
  "UL",
  "OL",
  "LI",
  "TABLE",
  "THEAD",
  "TBODY",
  "TFOOT",
  "TR",
  "TH",
  "TD",
  "A",
  "IMG",
  "H1",
  "H2",
  "H3",
  "H4",
  "H5",
  "H6",
  "SPAN",
  "MARK",
  "SUP",
  "SUB",
  "INPUT", // task-list checkboxes only — attribute filter below restricts the rest
  "DIV", // callouts
  "B",
  "I",
  "HGROUP",
]);

/** Attributes allowed per tag, beyond the universal rules below (never
 * `on*`, never `style`, never `download`). */
const ALLOWED_ATTRS: Record<string, readonly string[]> = {
  // No `data-*` anywhere: see the module note.
  A: ["href", "title"],
  IMG: ["src", "alt", "title", "width", "height"],
  INPUT: ["type", "checked", "disabled"],
  TD: ["colspan", "rowspan", "align"],
  TH: ["colspan", "rowspan", "align"],
  OL: ["start"],
  LI: ["value", "title"], // title only carries the nonce-gated footnote request
  DIV: ["title"], // callout request, nonce-gated like the embed one below
  SPAN: ["title"], // carries only the nonce-prefixed embed request, rewritten after sanitising
};

/** The single `class` value kept (on `<li>` only) — markdown-it task lists. */
const ALLOWED_LI_CLASS = "serat-task-list-item";

const XHTML_NS = "http://www.w3.org/1999/xhtml";

// ADR-0017: a `serat-asset:` URL is accepted here by prefix only. Path
// containment (no `..`, absolute paths, `:` segments, device names, symlink
// escapes) and the viewable-types check are the scheme handler's job in
// serat-app, which resolves every request through the vault's containment
// functions; this sanitiser deliberately neither normalises nor second-guesses
// the path, so there is exactly one containment rule to get right.

/** `javascript:`, `vbscript:` and any `data:` URL of any kind are refused
 * outright — never rendered as a link either, not only as an image. */
function isDangerousUrlScheme(value: string): boolean {
  const trimmed = value.trim().toLowerCase();
  return trimmed.startsWith("javascript:") || trimmed.startsWith("vbscript:") || trimmed.startsWith("data:");
}

function isHttpLikeUrl(value: string): boolean {
  const trimmed = value.trim().toLowerCase();
  return trimmed.startsWith("http://") || trimmed.startsWith("https://") || trimmed.startsWith("mailto:");
}

/** Builds the "not rendered here" placeholder §9.3 specifies for a remote or
 * `data:` image, labelled with the URL so the user can see what was refused
 * without Serat ever fetching it. */
function buildImagePlaceholder(doc: Document, src: string, alt: string | null): HTMLElement {
  const span = doc.createElement("span");
  span.setAttribute("data-serat-image-placeholder", "true");
  span.textContent = alt ? `[image not loaded: ${alt}]` : `[image not loaded: ${truncateForLabel(src)}]`;
  return span;
}

function truncateForLabel(value: string): string {
  return value.length > 120 ? `${value.slice(0, 117)}...` : value;
}

function sanitizeElement(doc: Document, el: Element): Node | null {
  // `localName`, not `tagName`: in a foreign namespace (`<svg><script>`,
  // `<math><style>`) `tagName` keeps the source case, so a case-sensitive
  // comparison would miss it and its text content would survive.
  const tag = el.localName.toUpperCase();

  // Hard-refused tags: never kept, never their children either, for the
  // ones capable of carrying an active surface. Script and style content
  // must not survive as inert text nodes either — dropping the element
  // drops its text content too.
  if (tag === "SCRIPT" || tag === "STYLE" || tag === "IFRAME" || tag === "OBJECT" || tag === "EMBED") {
    return null;
  }
  // A disallowed tag that isn't actively dangerous (e.g. <form>, <meta>,
  // <link>, <base>, <svg>, <video>) is unwrapped: its children are kept
  // (recursively sanitised) but the element itself is not, so legitimate
  // text nested inside an unrecognised wrapper is not lost.
  if (!ALLOWED_TAGS.has(tag) || el.namespaceURI !== XHTML_NS) {
    const fragment = doc.createDocumentFragment();
    for (const child of Array.from(el.childNodes)) {
      const sanitized = sanitizeNode(doc, child);
      if (sanitized) fragment.appendChild(sanitized);
    }
    return fragment;
  }

  if (tag === "IMG") {
    return sanitizeImage(doc, el);
  }

  const out = doc.createElement(tag.toLowerCase());
  const allowedForTag = ALLOWED_ATTRS[tag] ?? [];
  for (const attr of Array.from(el.attributes)) {
    const name = attr.name.toLowerCase();
    if (name.startsWith("on")) continue; // every event-handler attribute
    if (name === "style") continue;
    if (name === "download") continue;
    if (name.startsWith("data-")) continue; // never copy an incoming marker
    if (name === "class") {
      if (!(tag === "LI" && attr.value === ALLOWED_LI_CLASS)) continue;
    } else if (!allowedForTag.includes(name)) {
      continue;
    }

    if (tag === "A" && name === "href") {
      if (isDangerousUrlScheme(attr.value)) continue; // javascript:/data:/vbscript:
      // Anything that is not recognisably http(s)/mailto or an in-app
      // wikilink target is dropped rather than guessed at — an <a> with no
      // href renders as plain text, which is the safe default.
      if (!isHttpLikeUrl(attr.value) && !attr.value.startsWith("#")) continue;
      out.setAttribute("data-external", isHttpLikeUrl(attr.value) ? "true" : "false");
    }
    out.setAttribute(name, attr.value);
  }

  if (tag === "INPUT") {
    // Task-list checkboxes only: force type=checkbox and always disabled —
    // this is a read-only render, never a form.
    out.setAttribute("type", "checkbox");
    out.setAttribute("disabled", "");
    if (el.hasAttribute("checked")) out.setAttribute("checked", "");
    return out;
  }

  for (const child of Array.from(el.childNodes)) {
    const sanitized = sanitizeNode(doc, child);
    if (sanitized) out.appendChild(sanitized);
  }
  return out;
}

function sanitizeImage(doc: Document, el: Element): Node {
  const src = el.getAttribute("src") ?? "";
  const alt = el.getAttribute("alt");
  if (!isSeratAssetUrl(src)) {
    // Remote http(s) and any data: URL alike: never fetched, never
    // rendered as <img> — a labelled placeholder only.
    return buildImagePlaceholder(doc, src, alt);
  }
  const img = doc.createElement("img");
  img.setAttribute("src", src);
  if (alt) img.setAttribute("alt", alt);
  const width = el.getAttribute("width");
  const height = el.getAttribute("height");
  if (width && /^\d+$/.test(width)) img.setAttribute("width", width);
  if (height && /^\d+$/.test(height)) img.setAttribute("height", height);
  return img;
}

function sanitizeNode(doc: Document, node: Node): Node | null {
  if (node.nodeType === Node.TEXT_NODE) {
    return doc.createTextNode(node.textContent ?? "");
  }
  if (node.nodeType === Node.ELEMENT_NODE) {
    return sanitizeElement(doc, node as Element);
  }
  // Comments, processing instructions, doctypes: dropped.
  return null;
}

export interface SanitizeOptions {
  /** Per-render secret that `preview.ts` embeds in the hrefs/titles it wants
   * rewritten into trusted markers. Absent means no marker is ever added. */
  markerNonce?: string;
}

function safeDecode(value: string): string | null {
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

/** Adds the renderer's markers after sanitising, from nonce-prefixed
 * requests only. A document cannot guess the nonce, so cannot forge them. */
function applyTrustedMarkers(root: ParentNode, nonce: string): void {
  const prefix = `serat-${nonce}:`;
  for (const a of Array.from(root.querySelectorAll("a[href]"))) {
    const href = a.getAttribute("href") ?? "";
    if (!href.startsWith(`#${prefix}`)) continue;
    const rest = href.slice(prefix.length + 1);
    const colon = rest.indexOf(":");
    if (colon < 0) continue;
    const kind = rest.slice(0, colon);
    let payload: string;
    try {
      payload = decodeURIComponent(rest.slice(colon + 1));
    } catch {
      continue;
    }
    a.removeAttribute("data-external");
    a.setAttribute("href", "#");
    if (kind === "wikilink") {
      a.setAttribute("data-wikilink", payload);
      a.setAttribute("data-resolved", "false");
    } else if (kind === "fnref") {
      a.setAttribute("href", `#fn-${payload}`);
      a.setAttribute("data-footnote-ref", payload);
    }
  }
  for (const div of Array.from(root.querySelectorAll("div[title]"))) {
    const title = div.getAttribute("title") ?? "";
    const callout = `${prefix}callout:`;
    // Always drop the title: a div never legitimately keeps one.
    div.removeAttribute("title");
    if (!title.startsWith(callout)) continue;
    const type = safeDecode(title.slice(callout.length));
    if (type !== null && /^[\w-]+$/.test(type)) div.setAttribute("data-callout", type);
  }
  for (const li of Array.from(root.querySelectorAll("li[title]"))) {
    const title = li.getAttribute("title") ?? "";
    const item = `${prefix}fnitem:`;
    li.removeAttribute("title"); // an <li> never legitimately keeps one
    if (!title.startsWith(item)) continue;
    const label = safeDecode(title.slice(item.length));
    // Only after sanitising and only for the nonce: a document cannot plant ids.
    if (label !== null) li.setAttribute("id", `fn-${label}`);
  }
  for (const span of Array.from(root.querySelectorAll("span[title]"))) {
    const title = span.getAttribute("title") ?? "";
    const embed = `${prefix}embed:`;
    if (!title.startsWith(embed)) continue;
    span.removeAttribute("title");
    try {
      span.setAttribute("data-embed-placeholder", decodeURIComponent(title.slice(embed.length)));
    } catch {
      /* leave unmarked */
    }
  }
}

/**
 * Sanitises a `markdown-it`-rendered HTML fragment (or any untrusted HTML —
 * a note is untrusted input: git, sync tools and agents all write into the
 * vault, per §9.3) down to the allowlist above. Returns a `DocumentFragment`
 * ready to append into a real DOM node — deliberately not a string, so the
 * caller never has to run untrusted HTML through `innerHTML`/`v-html` a
 * second time after this function already parsed and rebuilt it once.
 */
export function sanitizeToFragment(
  html: string,
  doc: Document = document,
  options: SanitizeOptions = {},
): DocumentFragment {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  const fragment = doc.createDocumentFragment();
  for (const child of Array.from(parsed.body.childNodes)) {
    const sanitized = sanitizeNode(doc, child);
    if (sanitized) fragment.appendChild(sanitized);
  }
  if (options.markerNonce) applyTrustedMarkers(fragment, options.markerNonce);
  return fragment;
}

/** Convenience wrapper returning the sanitised markup as a string, for
 * callers that only need to assert on structure (tests) or that render
 * through a template binding rather than direct DOM mounting. Serialising a
 * `DocumentFragment` we just built ourselves, from an allowlist, is not the
 * same risk as `v-html`ing untrusted markup — nothing here is untrusted by
 * this point. */
export function sanitizeToHtml(html: string, doc: Document = document, options: SanitizeOptions = {}): string {
  const fragment = sanitizeToFragment(html, doc, options);
  const container = doc.createElement("div");
  container.appendChild(fragment);
  return container.innerHTML;
}
