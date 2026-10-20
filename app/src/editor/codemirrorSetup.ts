// Builds the editor's default command registry and the one CodeMirror
// keymap extension derived from it (architecture.md §9.3: "`keymap.of`
// appears in exactly one place, and a test asserts that" — this file is
// that one place for the editor surface; see `commands.ts`'s header note).

import {
  defaultKeymap,
  deleteCharBackward,
  history,
  historyKeymap,
  insertNewlineAndIndent,
  redo,
  undo,
} from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { keymap, type EditorView, type KeyBinding } from "@codemirror/view";

import { CommandRegistry, type CommandId } from "@/editor/commands";
import { deleteMarkupBackward, insertNewlineContinueMarkup } from "@/editor/markdownLanguage";

export interface EditorCommandHandlers {
  save: (view: EditorView) => boolean;
  toggleLivePreview: (view: EditorView) => boolean;
  followLink: (view: EditorView) => boolean;
}

function slugForKey(key: string): string {
  return key
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

/** Re-registers every binding of a `@codemirror/commands` keymap array under
 * a Serat id, per architecture.md §9.3's "the default editing and history
 * commands, which are re-registered under Serat ids". `excludeKeys` skips
 * bindings already given an explicit, named id above (undo/redo) so the
 * same physical key is not registered twice under two ids. */
function registerPassthrough(
  registry: CommandRegistry,
  bindings: readonly KeyBinding[],
  idPrefix: string,
  seenKeys: Set<string>,
): void {
  bindings.forEach((binding, index) => {
    const key = binding.key ?? binding.mac;
    // `@codemirror/commands`' own keymap arrays occasionally bind the same
    // literal key twice (once generically, once for a more specific
    // context CodeMirror itself disambiguates at dispatch time via `run`
    // returning `false` to fall through). This registry has no such
    // fallthrough concept — one id, one binding — so the first array entry
    // for a given key wins and later duplicates of the same key are
    // skipped, never registered under a second id (which is what would
    // make `buildKeymap` see a real collision).
    if (!key || seenKeys.has(key) || !binding.run) {
      return;
    }
    seenKeys.add(key);
    const { run } = binding;
    const id = `${idPrefix}.${slugForKey(key)}-${index}`;
    registry.register({
      id,
      name: `Editor: ${key}`,
      defaultBinding: key,
      when: "editor",
      run: (view) => run(view),
    });
  });
}

/**
 * Builds the registry the running editor uses. Handlers cover the
 * component-specific actions (save, mode toggle, follow-link); every other
 * editing/history command comes from CodeMirror's own `defaultKeymap`/
 * `historyKeymap`, wrapped so the registry — not `@codemirror/commands` — is
 * the single source every keybinding in the editor comes from.
 */
export function createEditorCommandRegistry(handlers: EditorCommandHandlers): CommandRegistry {
  const registry = new CommandRegistry();

  registry.register({
    id: "editor.save",
    name: "Save now",
    defaultBinding: "Mod-s",
    when: "editor",
    run: handlers.save,
  });
  registry.register({
    id: "editor.toggle-live-preview",
    name: "Toggle live preview / split",
    defaultBinding: "Mod-Shift-m",
    run: handlers.toggleLivePreview,
  });
  registry.register({
    id: "editor.follow-link",
    name: "Follow link/URL under cursor",
    defaultBinding: "Mod-Enter",
    when: "editor",
    run: handlers.followLink,
  });
  registry.register({
    id: "editor.undo",
    name: "Undo",
    defaultBinding: "Mod-z",
    when: "editor",
    run: (view) => undo(view),
  });
  registry.register({
    id: "editor.redo",
    name: "Redo",
    defaultBinding: "Mod-y",
    when: "editor",
    run: (view) => redo(view),
  });

  // The two Markdown list/quote commands that `@codemirror/lang-markdown`
  // would otherwise install through its own high-precedence keymap
  // (`addKeymap`), re-registered under Serat ids (§9.3). Each falls back to
  // the plain CodeMirror command exactly as that keymap's fallthrough did,
  // so Enter/Backspace outside a list behave as before.
  registry.register({
    id: "editor.markdown-newline",
    name: "Insert newline (continue Markdown list/quote)",
    defaultBinding: "Enter",
    when: "editor",
    run: (view) => insertNewlineContinueMarkup(view) || insertNewlineAndIndent(view),
  });
  registry.register({
    id: "editor.markdown-delete-backward",
    name: "Delete backward (outdent Markdown marker)",
    defaultBinding: "Backspace",
    when: "editor",
    run: (view) => deleteMarkupBackward(view) || deleteCharBackward(view),
  });

  // Keys already given an explicit, named command above — never
  // double-registered under a generic passthrough id, which is what would
  // let the same physical key collide with itself under two different ids.
  // Shared across both passthrough calls (and mutated by them) so a key
  // repeated between `historyKeymap` and `defaultKeymap`, or within either
  // array, is registered exactly once.
  const seenKeys = new Set(["Mod-s", "Mod-Shift-m", "Mod-Enter", "Mod-z", "Mod-y", "Enter", "Backspace"]);
  registerPassthrough(registry, historyKeymap, "editor.cm-history", seenKeys);
  registerPassthrough(registry, defaultKeymap, "editor.cm", seenKeys);

  return registry;
}

/**
 * The extensions that give the registry effect inside CodeMirror:
 * `history()` (state) plus exactly one `keymap.of(...)` built from the
 * registry. `overrides` is `UiSettings.keybindings` reshaped into a lookup
 * map — see `CodeMirrorEditor.vue`.
 */
export function buildEditorExtensions(
  registry: CommandRegistry,
  overrides: ReadonlyMap<CommandId, string | null> = new Map(),
): Extension[] {
  return [history(), keymap.of(registry.buildKeymap(overrides))];
}
