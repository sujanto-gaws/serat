// The preview renderer (architecture.md §9.3): "Split preview and embeds
// share one renderer: markdown-it with GFM, footnote and wikilink plugins,
// plus the resolver above. ... Rendered HTML passes a strict allowlist
// sanitiser before it reaches the DOM" (`sanitize.ts`).
//
// `markdown-it` is dynamically imported — NFR-PRF-01's "don't bloat the
// startup bundle" — so a session that never opens split preview or an embed
// never pays for it. GFM's table/task-list/strikethrough extensions ship
// with `markdown-it`'s own defaults plus `html: false` (never trust raw
// HTML from the note without sanitising it regardless, but disabling it in
// the parser first is one fewer thing the sanitiser has to catch) and
// `linkify: true` (bare URLs, GFM autolinks).
//
// Wikilink resolution (§9.3: "the editor resolves through one batched
// command per viewport change... before the index is Ready, links resolve
// by file name against the ADR-0015 tree listings") is task #22's IPC
// surface; this renderer draws every `[[target]]` as an unresolved-styled
// span for now so the preview never blocks on it landing, and is the one
// obvious place #22 plugs a resolver in (`WikiLinkResolver` below).

import type { MarkdownIt, StateInline, Token } from "markdown-it";

import { sanitizeToFragment } from "@/editor/sanitize";

export interface WikiLinkResolver {
  /** Returns the resolved vault-relative path, or `null` if unresolved —
   * task #22's batched resolution. Absent (the P1-default renderer) means
   * every wikilink renders unresolved, which is a legible, honest state
   * (§9.3's "Link unresolved" row), never a broken one. */
  resolve(target: string): string | null;
}

/** `md.render`'s `env`: carries this render's marker nonce to the rules. */
interface RenderEnv {
  nonce?: string;
  [key: string]: unknown;
}

function markerPrefix(env?: RenderEnv): string {
  return `serat-${env?.nonce ?? ""}:`;
}

function newNonce(): string {
  const bytes = new Uint8Array(12);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

let markdownItPromise: Promise<MarkdownIt> | null = null;

async function getMarkdownIt(): Promise<MarkdownIt> {
  if (!markdownItPromise) {
    markdownItPromise = import("markdown-it").then(({ default: MarkdownItCtor }) => {
      const md = new MarkdownItCtor({
        html: false,
        linkify: true,
        breaks: false,
      });
      md.use(wikiLinkPlugin);
      md.use(taskListPlugin);
      md.use(footnoteRefPlugin);
      return md;
    });
  }
  return markdownItPromise;
}

/** A minimal `markdown-it` inline rule for `[[target]]` / `[[target|text]]`
 * / `![[target]]`, ahead of task #22's real resolver landing. Renders an
 * `<a data-wikilink="…">` (resolved styling/interaction is `Preview.vue`'s
 * job) or, for an embed, a `<span data-embed="…">` placeholder — turning it
 * into a real `<img>` happens through the same `serat-asset:` rule
 * `sanitize.ts` already enforces once #22 supplies a resolved path.
 */
function wikiLinkPlugin(md: MarkdownIt): void {
  md.inline.ruler.before("link", "serat_wikilink", (state: StateInline, silent: boolean) => {
    const src = state.src;
    const start = state.pos;
    const isEmbed = src[start] === "!" && src[start + 1] === "[" && src[start + 2] === "[";
    const bracketStart = isEmbed ? start + 1 : start;
    if (src[bracketStart] !== "[" || src[bracketStart + 1] !== "[") {
      return false;
    }
    const close = src.indexOf("]]", bracketStart + 2);
    if (close < 0) {
      return false;
    }
    const inner = src.slice(bracketStart + 2, close);
    if (!inner) {
      return false;
    }
    if (silent) {
      return true;
    }
    const pipeIndex = inner.indexOf("|");
    const target = pipeIndex >= 0 ? inner.slice(0, pipeIndex) : inner;
    const display = pipeIndex >= 0 ? inner.slice(pipeIndex + 1) : inner;

    const token = state.push(isEmbed ? "serat_embed" : "serat_wikilink", "", 0);
    token.meta = { target, display };
    state.pos = close + 2;
    return true;
  });

  md.renderer.rules.serat_wikilink = (tokens: Token[], idx: number, _o: unknown, env?: RenderEnv) => {
    const { target, display } = tokens[idx].meta as { target: string; display: string };
    const escapedDisplay = md.utils.escapeHtml(display);
    // Unresolved by default (see the module note above); `data-target`
    // carries the raw target text so a future resolver pass, or the
    // click handler, can look it up without re-parsing markup.
    // The marker is requested via a nonce-prefixed href, which the
    // sanitiser rewrites into `data-wikilink` after sanitising.
    return `<a href="#${markerPrefix(env)}wikilink:${encodeURIComponent(target)}">${escapedDisplay}</a>`;
  };

  md.renderer.rules.serat_embed = (tokens: Token[], idx: number, _o: unknown, env?: RenderEnv) => {
    const { target } = tokens[idx].meta as { target: string; display: string };
    const escapedTarget = md.utils.escapeHtml(target);
    // Not an <img> here — the module note's "no resolver yet" case. Once
    // #22 supplies a `serat-asset:` path this becomes a real <img> through
    // the ordinary sanitiser rule (never a second, embed-specific one).
    return `<span title="${markerPrefix(env)}embed:${encodeURIComponent(target)}">[[embed: ${escapedTarget}]]</span>`;
  };
}

/** GFM task lists (`- [ ] x`, `- [x] y`): markdown-it has no native concept
 * of these — this rewrites a list item's leading `[ ]`/`[x]` inline token
 * into a (disabled, read-only — this is a render, not a form) checkbox,
 * post-parse, the same technique `markdown-it-task-lists` uses. */
function taskListPlugin(md: MarkdownIt): void {
  md.core.ruler.push("serat_task_list", (state: import("markdown-it").StateCore) => {
    const { tokens } = state;
    for (let i = 0; i < tokens.length; i++) {
      if (tokens[i].type !== "list_item_open") continue;
      const inline = tokens[i + 2];
      if (!inline || inline.type !== "inline" || !inline.children || inline.children.length === 0) continue;
      const first = inline.children[0];
      if (first.type !== "text") continue;
      const match = /^\[( |x|X)\]\s+/.exec(first.content);
      if (!match) continue;
      const checked = match[1].toLowerCase() === "x";
      first.content = first.content.slice(match[0].length);
      const checkbox = new state.Token("html_inline", "", 0);
      checkbox.content = `<input type="checkbox" disabled${checked ? " checked" : ""}> `;
      inline.children.unshift(checkbox);
      tokens[i].attrJoin("class", "serat-task-list-item");
    }
    return true;
  });
}

/** `[^label]` footnote references (architecture.md §9.3's custom-inline
 * list). Renders a superscript anchor; the definition line
 * (`[^label]: text`) is left as ordinary paragraph text in P1's preview —
 * source-accurate (C-01: nothing is rewritten) but not yet collected into a
 * numbered footnotes section, which is a known scope gap for a follow-up
 * task, not a silent omission. */
function footnoteRefPlugin(md: MarkdownIt): void {
  md.inline.ruler.before("link", "serat_footnote_ref", (state: StateInline, silent: boolean) => {
    const src = state.src;
    const start = state.pos;
    if (src[start] !== "[" || src[start + 1] !== "^") {
      return false;
    }
    const close = src.indexOf("]", start + 2);
    if (close < 0) {
      return false;
    }
    const label = src.slice(start + 2, close);
    if (!label || /\s/.test(label)) {
      return false;
    }
    if (silent) {
      return true;
    }
    const token = state.push("serat_footnote_ref", "", 0);
    token.meta = { label };
    state.pos = close + 1;
    return true;
  });

  md.renderer.rules.serat_footnote_ref = (tokens: Token[], idx: number, _o: unknown, env?: RenderEnv) => {
    const { label } = tokens[idx].meta as { label: string };
    const escaped = md.utils.escapeHtml(label);
    return `<sup><a href="#${markerPrefix(env)}fnref:${encodeURIComponent(label)}">${escaped}</a></sup>`;
  };
}

/**
 * Renders `markdownSource` to a sanitised `DocumentFragment`, ready to
 * mount. Chunked rendering for large notes (FR-EDT-10: "a 5,000-line note
 * never renders in one frame") is the caller's job — `renderChunked` below
 * — because chunking needs to interleave with the DOM, which a pure render
 * function should not assume how to do.
 */
export async function renderMarkdownToFragment(markdownSource: string, doc: Document = document): Promise<DocumentFragment> {
  const md = await getMarkdownIt();
  const nonce = newNonce();
  const html = md.render(markdownSource, { nonce } satisfies RenderEnv);
  return sanitizeToFragment(html, doc, { markerNonce: nonce });
}

/**
 * Splits a note into block-level chunks by blank-line-separated groups of
 * lines, for progressive, idle-scheduled rendering (§9.3's split-preview
 * section: "The render is scheduled in idle time and chunked, so a
 * 5,000-line note never renders in one frame"). A pure function — the
 * scheduling itself is `Preview.vue`'s job, using
 * `requestIdleCallback`/`setTimeout` as the environment allows.
 */
export function chunkMarkdownSource(markdownSource: string, targetLinesPerChunk = 200): string[] {
  const lines = markdownSource.split("\n");
  if (lines.length <= targetLinesPerChunk) {
    return [markdownSource];
  }
  const chunks: string[] = [];
  let current: string[] = [];
  for (const line of lines) {
    current.push(line);
    if (current.length >= targetLinesPerChunk && line.trim() === "") {
      chunks.push(current.join("\n"));
      current = [];
    }
  }
  if (current.length > 0) {
    chunks.push(current.join("\n"));
  }
  return chunks;
}
