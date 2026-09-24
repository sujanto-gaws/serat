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
  // `data-wikilink`/`data-resolved`/`data-footnote-ref` are this renderer's
  // own markers (`preview.ts`), read by the mounting component's click
  // handler and by task #22's resolver — never attacker-controlled markup,
  // since `preview.ts` is the only place that ever emits them and it always
  // HTML-escapes the value first.
  A: ["href", "title", "data-external", "data-wikilink", "data-resolved", "data-footnote-ref"],
  IMG: ["src", "alt", "title", "width", "height"],
  INPUT: ["type", "checked", "disabled"],
  TD: ["colspan", "rowspan", "align"],
  TH: ["colspan", "rowspan", "align"],
  OL: ["start"],
  LI: ["value", "class"],
  DIV: ["data-callout"],
  SPAN: ["data-embed-placeholder", "data-serat-image-placeholder"],
};

const SERAT_ASSET_ORIGINS = ["serat-asset:", "http://serat-asset.localhost"];

function isSeratAssetUrl(value: string): boolean {
  return SERAT_ASSET_ORIGINS.some((origin) => value.startsWith(origin));
}

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
  const tag = el.tagName;

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
  if (!ALLOWED_TAGS.has(tag)) {
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
    if (!allowedForTag.includes(name)) continue;

    if (tag === "A" && name === "href") {
      if (isDangerousUrlScheme(attr.value)) continue; // javascript:/data:/vbscript:
      // Anything that is not recognisably http(s)/mailto or an in-app
      // wikilink target is dropped rather than guessed at — an <a> with no
      // href renders as plain text, which is the safe default.
      if (!isHttpLikeUrl(attr.value) && !attr.value.startsWith("#")) continue;
      out.setAttribute("data-external", isHttpLikeUrl(attr.value) ? "true" : "false");
    }
    out.setAttribute(attr.name, attr.value);
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

/**
 * Sanitises a `markdown-it`-rendered HTML fragment (or any untrusted HTML —
 * a note is untrusted input: git, sync tools and agents all write into the
 * vault, per §9.3) down to the allowlist above. Returns a `DocumentFragment`
 * ready to append into a real DOM node — deliberately not a string, so the
 * caller never has to run untrusted HTML through `innerHTML`/`v-html` a
 * second time after this function already parsed and rebuilt it once.
 */
export function sanitizeToFragment(html: string, doc: Document = document): DocumentFragment {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  const fragment = doc.createDocumentFragment();
  for (const child of Array.from(parsed.body.childNodes)) {
    const sanitized = sanitizeNode(doc, child);
    if (sanitized) fragment.appendChild(sanitized);
  }
  return fragment;
}

/** Convenience wrapper returning the sanitised markup as a string, for
 * callers that only need to assert on structure (tests) or that render
 * through a template binding rather than direct DOM mounting. Serialising a
 * `DocumentFragment` we just built ourselves, from an allowlist, is not the
 * same risk as `v-html`ing untrusted markup — nothing here is untrusted by
 * this point. */
export function sanitizeToHtml(html: string, doc: Document = document): string {
  const fragment = sanitizeToFragment(html, doc);
  const container = doc.createElement("div");
  container.appendChild(fragment);
  return container.innerHTML;
}
