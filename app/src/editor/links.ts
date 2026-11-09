// Wikilinks in the editor (FR-EDT-03, FR-EDT-04, FR-NAV-03, US-04; ADR-0016,
// ADR-0017; architecture.md §9.3 "Links and embeds").
//
//   - Styling: a debounced viewport scan sends ONE `resolve_links` batch for
//     targets not yet answered; resolved / unresolved links get a mark
//     decoration. Nothing on the keystroke path calls IPC: edits only map the
//     existing decorations, and the scan runs after the user pauses.
//   - Completion: `[[` + two characters queries `switcher_query` (same corpus
//     and ranking as the switcher; the server's order is kept), then `#` and
//     `^` complete a resolved note's headings and block ids via `note_anchors`.
//   - Follow link: `followLinkAt` finds the link under the cursor for the
//     host to navigate; this module never navigates.
//
// C-01/C-04: link text is never rewritten. Completion replaces only the
// fragment the user typed after `[[`; decorations are display-only.

import { autocompletion, type Completion, type CompletionContext, type CompletionSource } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import { type EditorState, type Extension, RangeSetBuilder } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";

import type { LinkResolver } from "@/editor/linkResolver";
import {
  hasResolutionEffect,
  innerOfLinkNode,
  linkKey,
  parseLinkInner,
  refreshLinks,
  resolutionsField,
  setResolutions,
  type ParsedLink,
} from "@/editor/linkState";
import type { LinkIpcAdapter } from "@/ipc/linkAdapter";
import { createSwitcherQuerier } from "@/ipc/switcherQuery";

/** Idle time after the last doc/viewport change before the scan runs. */
export const LINK_SCAN_DEBOUNCE_MS = 200;

export interface LinkExtensionDeps {
  ipc: LinkIpcAdapter;
  resolver: LinkResolver;
  /** Vault-relative path of the open note, or null when none is open. */
  getSourcePath(): string | null;
}

// ---------------------------------------------------------------------------
// Link nodes
// ---------------------------------------------------------------------------

export interface LinkNode {
  from: number;
  to: number;
  isEmbed: boolean;
  link: ParsedLink;
}

function linkNodesIn(state: EditorState, from: number, to: number): LinkNode[] {
  const found: LinkNode[] = [];
  syntaxTree(state).iterate({
    from,
    to,
    enter: (node) => {
      if (node.name !== "SeratWikiLink" && node.name !== "SeratEmbed") return;
      const inner = innerOfLinkNode(state.sliceDoc(node.from, node.to));
      if (inner !== null) {
        found.push({ from: node.from, to: node.to, isEmbed: node.name === "SeratEmbed", link: parseLinkInner(inner) });
      }
      return false;
    },
  });
  return found;
}

/** The link whose source range contains `pos` (inclusive), for follow-link. */
export function linkNodeAt(state: EditorState, pos: number): LinkNode | null {
  const lineStart = state.doc.lineAt(pos);
  return linkNodesIn(state, lineStart.from, lineStart.to).find((n) => n.from <= pos && pos <= n.to) ?? null;
}

// ---------------------------------------------------------------------------
// Decorations + debounced scan
// ---------------------------------------------------------------------------

function buildDecorations(view: EditorView): DecorationSet {
  const resolutions = view.state.field(resolutionsField);
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    for (const node of linkNodesIn(view.state, from, to)) {
      // `[[#Heading]]` and `[[^id]]` target the current note: always resolved
      // as far as the note goes; only the subpath can be missing.
      const resolution = resolutions.get(linkKey(node.link.target));
      if (!resolution) continue; // not answered yet: no styling, never a guess
      const resolved = resolution.path !== null;
      const subpathMissing = resolved && resolution.subpath_found === false;
      const classes = [resolved ? "serat-link-resolved" : "serat-link-unresolved"];
      if (subpathMissing) classes.push("serat-link-subpath-missing");
      const attributes: Record<string, string> = { class: classes.join(" ") };
      if (!resolved) attributes.title = `No note named “${node.link.note || node.link.target}” — click to create`;
      builder.add(node.from, node.to, Decoration.mark({ attributes }));
    }
  }
  return builder.finish();
}

function makeLinksPlugin(deps: LinkExtensionDeps) {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      private timer: ReturnType<typeof setTimeout> | null = null;
      private destroyed = false;

      constructor(private readonly view: EditorView) {
        this.decorations = buildDecorations(view);
        this.schedule();
      }

      update(update: ViewUpdate): void {
        const refreshed =
          update.transactions.some((tr) => hasResolutionEffect(tr.effects) || tr.effects.some((e) => e.is(refreshLinks)));
        if (refreshed || (update.viewportChanged && !update.docChanged)) {
          // Cheap: only the visible ranges, no IPC.
          this.decorations = buildDecorations(update.view);
        } else if (update.docChanged) {
          this.decorations = this.decorations.map(update.changes);
        }
        if (update.docChanged || update.viewportChanged || update.transactions.some((tr) => tr.effects.some((e) => e.is(refreshLinks)))) {
          this.schedule();
        }
      }

      /** Debounced: nothing runs until the user pauses (NFR-PRF-02). */
      private schedule(): void {
        if (this.timer) clearTimeout(this.timer);
        this.timer = setTimeout(() => {
          this.timer = null;
          void this.scan();
        }, LINK_SCAN_DEBOUNCE_MS);
      }

      private async scan(): Promise<void> {
        const source = deps.getSourcePath();
        if (!source || this.destroyed) return;
        const targets: string[] = [];
        for (const { from, to } of this.view.visibleRanges) {
          for (const node of linkNodesIn(this.view.state, from, to)) targets.push(node.link.target);
        }
        if (targets.length === 0) return;
        const answers = await deps.resolver.resolve(source, targets);
        if (this.destroyed || answers.length === 0) return;
        if (deps.getSourcePath() !== source) return; // note switched meanwhile
        this.view.dispatch({ effects: setResolutions.of(answers) });
      }

      destroy(): void {
        this.destroyed = true;
        if (this.timer) clearTimeout(this.timer);
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}

const linkTheme = EditorView.baseTheme({
  ".serat-link-resolved": { color: "var(--serat-link, #3b6ea5)" },
  ".serat-link-unresolved": {
    color: "var(--serat-rust, #b7410e)",
    textDecoration: "underline dotted",
  },
  ".serat-link-subpath-missing": { textDecoration: "underline dotted" },
});

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

export type LinkCompletionMode =
  | { kind: "name"; query: string; from: number }
  | { kind: "heading"; target: string; query: string; from: number }
  | { kind: "block"; target: string; query: string; from: number };

/**
 * Classifies the text between an open `[[` and the cursor. `inner` is that
 * text (after the brackets), `innerStart` its document offset. Returns null
 * after a `|` (display text is free text) or when a name query is under two
 * characters and completion was not requested explicitly.
 */
export function classifyLinkInput(inner: string, innerStart: number, explicit: boolean): LinkCompletionMode | null {
  if (inner.includes("|")) return null;
  const hash = inner.indexOf("#");
  if (hash >= 0) {
    const target = inner.slice(0, hash);
    const rest = inner.slice(hash + 1);
    if (rest.startsWith("^")) return { kind: "block", target, query: rest.slice(1), from: innerStart + hash + 2 };
    return { kind: "heading", target, query: rest, from: innerStart + hash + 1 };
  }
  const caret = inner.indexOf("^");
  if (caret >= 0) return { kind: "block", target: inner.slice(0, caret), query: inner.slice(caret + 1), from: innerStart + caret + 1 };
  if (inner.length < 2 && !explicit) return null;
  return { kind: "name", query: inner, from: innerStart };
}

/** Replaces `[from, to)` with `text`, adds the closing `]]` only if absent,
 * and leaves the cursor before it so `#`/`^` can follow. */
function insertLinkText(view: EditorView, from: number, to: number, text: string): void {
  const hasClose = view.state.sliceDoc(to, to + 2) === "]]";
  view.dispatch({
    changes: { from, to, insert: hasClose ? text : `${text}]]` },
    selection: { anchor: from + text.length },
    userEvent: "input.complete",
  });
}

/** Prefix matches first, then substring matches, each in document order; case-insensitive. */
export function rankAnchors(candidates: readonly string[], query: string): string[] {
  const q = query.toLowerCase();
  if (!q) return [...candidates];
  const prefix: string[] = [];
  const inner: string[] = [];
  for (const c of candidates) {
    const lower = c.toLowerCase();
    if (lower.startsWith(q)) prefix.push(c);
    else if (lower.includes(q)) inner.push(c);
  }
  return [...prefix, ...inner];
}

export function makeLinkCompletionSource(deps: LinkExtensionDeps): CompletionSource {
  const querier = createSwitcherQuerier((args) => deps.ipc.switcherQuery(args));
  return async (context: CompletionContext) => {
    const before = context.matchBefore(/\[\[[^\]\n]*/);
    if (!before) return null;
    const mode = classifyLinkInput(before.text.slice(2), before.from + 2, context.explicit);
    if (!mode) return null;
    // CodeMirror has already superseded this context: no IPC for an answer nobody will read.
    if (context.aborted) return null;
    const source = deps.getSourcePath();

    if (mode.kind === "name") {
      let results;
      try {
        results = await querier.run(mode.query, 50);
      } catch {
        return null; // transport failure: no completions, nothing thrown into typing
      }
      if (context.aborted || results === null) return null; // a newer keystroke wins; discarded = superseded, ignore
      const names = results.corpus === "tree" ? " · file names only" : "";
      const total = results.items.length;
      const options: Completion[] = results.items.map((item, index) => {
        const isAlias = item.matched === "alias";
        const text = isAlias ? `${item.link_text}|${item.candidate}` : item.link_text;
        return {
          label: item.candidate,
          detail: (isAlias ? `alias → ${item.title ?? item.path}` : item.path) + names,
          type: "text",
          boost: total - index, // keep the server's ranking
          apply: (view, _completion, from, to) => insertLinkText(view, from, to, text),
        };
      });
      return { from: mode.from, options, filter: false };
    }

    // `#` / `^`: complete against the target note's anchors.
    let path: string | null = null;
    if (mode.target.trim() === "") {
      path = source;
    } else if (source) {
      const [answer] = await deps.resolver.resolve(source, [mode.target.trim()]);
      path = answer?.path ?? null;
    }
    if (!path || context.aborted) return null;
    let anchors;
    try {
      anchors = await deps.ipc.noteAnchors({ path });
    } catch {
      return null;
    }
    if (context.aborted || anchors.kind !== "ready") return null;
    const pool = mode.kind === "heading" ? anchors.value.headings : anchors.value.block_ids;
    const ranked = rankAnchors(pool, mode.query).filter((a) => !/[\]|\n]/.test(a));
    const total = ranked.length;
    return {
      from: mode.from,
      filter: false,
      options: ranked.map((text, index) => ({
        label: mode.kind === "block" ? `^${text}` : text,
        type: mode.kind === "heading" ? "keyword" : "variable",
        boost: total - index,
        apply: (view, _c, from, to) => insertLinkText(view, from, to, text),
      })),
    };
  };
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/** Everything the editor needs for links, as one extension. The caller owns
 * `resolver` (it must be invalidated on `index://changed`). */
export function linkExtensions(deps: LinkExtensionDeps): Extension[] {
  return [
    resolutionsField,
    makeLinksPlugin(deps),
    linkTheme,
    autocompletion({
      override: [makeLinkCompletionSource(deps)],
      activateOnTyping: true,
      // Keys come from the command registry (architecture.md §9.3), never from
      // a keymap this extension installs itself.
      defaultKeymap: false,
      icons: false,
    }),
  ];
}
