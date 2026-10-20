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
  platform: EditorPlatform,
): void {
  bindings.forEach((binding, index) => {
    // Per-platform key choice exactly as CodeMirror makes it: the platform's
    // override (`mac` / `win` / `linux`) wins, else `key`. A binding with no
    // key for this platform is skipped: the mac-only emacs bindings
    // (Ctrl-v, Ctrl-k, Ctrl-f ...) must not exist on Windows/Linux, where
    // they would cancel native paste and friends.
    const key = binding[platform] ?? binding.key;
    // `@codemirror/commands`' own keymap arrays occasionally bind the same
    // literal key twice (once generically, once for a more specific
    // context CodeMirror itself disambiguates at dispatch time via `run`
    // returning `false` to fall through). This registry has no such
    // fallthrough concept: one id, one binding. So the first array entry
    // for a given key wins and later duplicates of the same key are
    // skipped, never registered under a second id (which is what would
    // make `buildKeymap` see a real collision).
    if (!key || !binding.run) {
      return;
    }
    const base = `${idPrefix}.${slugForKey(key)}-${index}`;
    if (!seenKeys.has(key)) {
      seenKeys.add(key);
      const { run } = binding;
      registry.register({
        id: base,
        name: `Editor: ${key}`,
        defaultBinding: key,
        when: "editor",
        // CodeMirror's own default (false): a key the command declines
        // stays native.
        preventDefault: binding.preventDefault ?? false,
        run: (view) => run(view),
      });
    }
    // `shift: run` is CodeMirror's shorthand for the same key with Shift held
    // (selection-extending variants: Shift-ArrowLeft, Shift-Home ...).
    if (binding.shift) {
      const shiftKey = `Shift-${key}`;
      if (!seenKeys.has(shiftKey)) {
        seenKeys.add(shiftKey);
        const shiftRun = binding.shift;
        registry.register({
          id: `${base}-shift`,
          name: `Editor: ${shiftKey}`,
          defaultBinding: shiftKey,
          when: "editor",
          preventDefault: binding.preventDefault ?? false,
          run: (view) => shiftRun(view),
        });
      }
    }
  });
}

export type EditorPlatform = "mac" | "win" | "linux";

/** The same sniff CodeMirror itself uses for its platform key choice. */
export function detectPlatform(): EditorPlatform {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform ?? "";
  if (/Mac|iPhone|iPad/.test(platform)) return "mac";
  if (/Win/.test(platform)) return "win";
  return "linux";
}

/**
 * Builds the registry the running editor uses. Handlers cover the
 * component-specific actions (save, mode toggle, follow-link); every other
 * editing/history command comes from CodeMirror's own `defaultKeymap`/
 * `historyKeymap`, wrapped so the registry — not `@codemirror/commands` — is
 * the single source every keybinding in the editor comes from.
 */
export function createEditorCommandRegistry(
  handlers: EditorCommandHandlers,
  platform: EditorPlatform = detectPlatform(),
): CommandRegistry {
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
  registerPassthrough(registry, historyKeymap, "editor.cm-history", seenKeys, platform);
  registerPassthrough(registry, defaultKeymap, "editor.cm", seenKeys, platform);

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
