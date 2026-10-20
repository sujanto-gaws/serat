// The editor's link-related IPC surface (contracts/ipc.rs: `resolve_links`,
// `note_anchors`, `switcher_query`, event `index://changed`), behind the same
// kind of mockable seam as `editorAdapter.ts`. `serat-app` does not implement
// these yet (task #18); nothing in `src/editor` calls `invoke`/`listen`
// directly, so landing the real commands changes this file alone.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { IndexChanged } from "./generated/IndexChanged";
import type { IndexRead } from "./generated/IndexRead";
import type { LinkResolution } from "./generated/LinkResolution";
import type { NoteAnchors } from "./generated/NoteAnchors";
import type { NotePathArgs } from "./generated/NotePathArgs";
import type { ResolveLinksArgs } from "./generated/ResolveLinksArgs";
import type { SwitcherQueryArgs } from "./generated/SwitcherQueryArgs";
import type { SwitcherResults } from "./generated/SwitcherResults";

export interface LinkIpcAdapter {
  /** `resolve_links(args) -> Vec<LinkResolution>` — never fails, never waits. */
  resolveLinks(args: ResolveLinksArgs): Promise<LinkResolution[]>;
  /** `note_anchors(args) -> Result<IndexRead<NoteAnchors>, IpcError>`. */
  noteAnchors(args: NotePathArgs): Promise<IndexRead<NoteAnchors>>;
  /** `switcher_query(args) -> SwitcherResults` — the link-completion corpus and ranking. */
  switcherQuery(args: SwitcherQueryArgs): Promise<SwitcherResults>;
  /** Subscribes to `index://changed` (invalidation only). */
  onIndexChanged(handler: (event: IndexChanged) => void): Promise<UnlistenFn>;
}

export const INDEX_CHANGED_EVENT = "index://changed";

export const defaultLinkIpc: LinkIpcAdapter = {
  resolveLinks(args) {
    return invoke("resolve_links", { args });
  },
  noteAnchors(args) {
    return invoke("note_anchors", { args });
  },
  switcherQuery(args) {
    return invoke("switcher_query", { args });
  },
  onIndexChanged(handler) {
    return listen<IndexChanged>(INDEX_CHANGED_EVENT, (event) => handler(event.payload));
  },
};
