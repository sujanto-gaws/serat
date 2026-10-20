// Shared link state (FR-EDT-03, architecture.md §9.3 "Links and embeds"):
// the editor-state half of link resolution. The async half (batching,
// caching, IPC) is `linkResolver.ts`; this module is deliberately free of
// both IPC and decorations so `blockWidgets.ts` (embed images) and `links.ts`
// (link styling, completion) can share it without importing each other.

import { StateEffect, StateField } from "@codemirror/state";

import type { LinkResolution } from "@/ipc/generated/LinkResolution";

/** Cache/lookup key for a link target. Resolution is case-insensitive for
 * ASCII only (§9.3: "NOCASE folds ASCII only"), so only A-Z are folded:
 * `[[Foo]]` and `[[foo]]` share an entry, `[[É]]` and `[[é]]` do not. */
export function linkKey(target: string): string {
  return target.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
}

export interface ParsedLink {
  /** As written, minus brackets and `|display` — what `resolve_links` takes. */
  target: string;
  /** Note part, before any `#`/`^` subpath. Empty for `[[#Heading]]`. */
  note: string;
  /** `Heading`, `A#B` or `^id`; null when there is no subpath. */
  subpath: string | null;
  display: string | null;
}

/** Parses the text between `[[` and `]]` (never rewrites it — C-01). */
export function parseLinkInner(inner: string): ParsedLink {
  const bar = inner.indexOf("|");
  const display = bar >= 0 ? inner.slice(bar + 1) : null;
  const target = (bar >= 0 ? inner.slice(0, bar) : inner).trim();
  const hash = target.indexOf("#");
  const caret = target.indexOf("^");
  const cut = hash >= 0 ? hash : caret;
  const note = cut >= 0 ? target.slice(0, cut).trim() : target;
  let subpath: string | null = null;
  if (cut >= 0) subpath = hash >= 0 ? target.slice(hash + 1) : target.slice(caret);
  return { target, note, subpath, display };
}

/** The inner text of a `[[..]]` / `![[..]]` node's source, or null. */
export function innerOfLinkNode(nodeText: string): string | null {
  const match = /^!?\[\[([\s\S]*)\]\]$/.exec(nodeText);
  return match ? match[1] : null;
}

/** Results arriving from `resolve_links`; merged into the state field. */
export const setResolutions = StateEffect.define<LinkResolution[]>();

/** Asks the links plugin to rebuild its decorations (cache invalidated, viewport rescan). */
export const refreshLinks = StateEffect.define<null>();

export const resolutionsField = StateField.define<ReadonlyMap<string, LinkResolution>>({
  create: () => new Map(),
  update(value, tr) {
    let next: Map<string, LinkResolution> | null = null;
    for (const effect of tr.effects) {
      if (effect.is(setResolutions)) {
        next ??= new Map(value);
        for (const resolution of effect.value) next.set(linkKey(resolution.target), resolution);
      }
    }
    return next ?? value;
  },
});

/** Whether any transaction in `tr` carried a resolution update. */
export function hasResolutionEffect(effects: readonly StateEffect<unknown>[]): boolean {
  return effects.some((e) => e.is(setResolutions));
}
