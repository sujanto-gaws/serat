// The editor's link-related IPC surface (contracts/ipc.rs: `resolve_links`,
// `note_anchors`, `switcher_query`, event `index://changed`), behind the same
// kind of mockable seam as `editorAdapter.ts`. `serat-app` implements
// `resolve_links` and `note_anchors` since task #181 (a guard in `acl_manifest.rs`
// fails if a command name passed to `invoke` here is not registered); nothing in
// `src/editor` calls `invoke`/`listen` directly.

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
  /** `resolve_links(args) -> Result<Vec<LinkResolution>, IpcError>` — never waits; fails only with no vault open. */
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
