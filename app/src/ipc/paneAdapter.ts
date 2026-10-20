// IPC for the three panes (task #24): the streaming file tree (ADR-0015), the
// backlinks read (§4.5 R2) and search (§4.5 R1/R3). Mockable like the other
// adapters: components take a `PaneIpcAdapter` prop, tests substitute their own,
// and nothing outside this file calls `invoke`/`Channel` for these commands.
// `serat-app` does not implement them yet (task #18); landing the real commands
// changes `defaultPaneIpc` alone.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { defaultLinkIpc } from "./linkAdapter";
import type { Backlink } from "./generated/Backlink";
import type { IndexRead } from "./generated/IndexRead";
import type { MarkTreeArgs } from "./generated/MarkTreeArgs";
import type { NotePathArgs } from "./generated/NotePathArgs";
import type { SearchArgs } from "./generated/SearchArgs";
import type { SearchOutcome } from "./generated/SearchOutcome";
import type { TreeMessage } from "./generated/TreeMessage";

export interface PaneIpcAdapter {
  /**
   * `open_tree(on_message: Channel<TreeMessage>)`: ordered, lossless, root
   * listing first. `onMessage` is called once per message, in arrival order.
   */
  openTree(onMessage: (message: TreeMessage) => void): Promise<void>;
  /** `mark_tree(args)`: stamps `tree_first_screen` / `tree_complete` on the Rust clock. */
  markTree(args: MarkTreeArgs): Promise<void>;
  /** `search(args) -> Result<IndexRead<SearchOutcome>, IpcError>`. */
  search(args: SearchArgs): Promise<IndexRead<SearchOutcome>>;
  /** `backlinks(args) -> Result<IndexRead<Vec<Backlink>>, IpcError>`. */
  backlinks(args: NotePathArgs): Promise<IndexRead<Backlink[]>>;
  /** `index://changed`: invalidation only. */
  onIndexChanged(handler: () => void): Promise<UnlistenFn>;
}

export const defaultPaneIpc: PaneIpcAdapter = {
  async openTree(onMessage) {
    const channel = new Channel<TreeMessage>();
    channel.onmessage = onMessage;
    await invoke("open_tree", { onMessage: channel });
  },
  async markTree(args) {
    await invoke("mark_tree", { args });
  },
  search(args) {
    return invoke("search", { args });
  },
  backlinks(args) {
    return invoke("backlinks", { args });
  },
  onIndexChanged(handler) {
    return defaultLinkIpc.onIndexChanged(() => handler());
  },
};
