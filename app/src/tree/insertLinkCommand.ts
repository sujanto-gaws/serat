// `tree.insertLink` (FR-NAV-06, NFR-USE-01, architecture.md §9.3): Insert link is a real, rebindable
// registry command, active only while the tree has focus and a file row is selected. The tree's own
// keydown and the context menu's accelerator both read its effective binding; Rename stays a fixed F2.

import type { CommandContextInfo, CommandRegistry } from "@/editor/commands";

/** The slice of FileTree this command needs (its `defineExpose`d methods). */
export interface InsertLinkTarget {
  insertLinkUnavailable(ctx: CommandContextInfo): string | null;
  insertLinkSelected(): void;
}

export const TREE_INSERT_LINK_ID = "tree.insertLink";

export function registerTreeInsertLink(commands: CommandRegistry, tree: () => InsertLinkTarget | null | undefined): () => void {
  return commands.register({
    id: TREE_INSERT_LINK_ID,
    name: "Insert link to selected file",
    defaultBinding: "Mod-Shift-l",
    when: "tree", // global chords skip `when`-scoped commands; the tree's keydown calls registry.matchContext("tree")
    // Listed in the palette always (command-palette.md: unavailable commands stay, greyed, with a reason).
    unavailable: (ctx) => {
      const target = tree();
      return target ? target.insertLinkUnavailable(ctx) : "Focus the file tree first"; // null means active
    },
    exec: () => tree()?.insertLinkSelected(),
  });
}
