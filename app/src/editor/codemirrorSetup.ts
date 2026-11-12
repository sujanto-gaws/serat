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
import { acceptCompletion, closeCompletion, moveCompletionSelection, startCompletion } from "@codemirror/autocomplete";
import type { Extension } from "@codemirror/state";
import { EditorView, keymap, scrollPastEnd, type KeyBinding } from "@codemirror/view";

import { chordParts, CommandRegistry, type CommandId } from "@/editor/commands";
import { deleteMarkupBackward, insertNewlineContinueMarkup } from "@/editor/markdownLanguage";

export interface EditorCommandHandlers {
  save: (view: EditorView) => boolean;
  toggleLivePreview: () => boolean;
  followLink: (view: EditorView) => boolean;
}

/** Readable names for CodeMirror's own commands, keyed by the key they resolve to on a platform.
 * Static on purpose: the commands' function names are minified in a production build. */
const PASSTHROUGH_NAMES: Record<string, string> = {
  "Mod-z": "Undo",
  "Mod-y": "Redo",
  "Mod-Shift-z": "Redo",
  "Ctrl-Shift-z": "Redo",
  "Mod-u": "Undo selection",
  "Alt-u": "Redo selection",
  "Mod-Shift-u": "Redo selection",
  "Alt-ArrowLeft": "Move by syntax left",
  "Ctrl-ArrowLeft": "Move by syntax left",
  "Alt-ArrowRight": "Move by syntax right",
  "Ctrl-ArrowRight": "Move by syntax right",
  "Alt-ArrowUp": "Move line up",
  "Shift-Alt-ArrowUp": "Copy line up",
  "Alt-ArrowDown": "Move line down",
  "Shift-Alt-ArrowDown": "Copy line down",
  "Mod-Alt-ArrowUp": "Add cursor above",
  "Mod-Alt-ArrowDown": "Add cursor below",
  "Alt-l": "Select line",
  "Ctrl-l": "Select line",
  "Mod-i": "Select parent syntax",
  "Mod-[": "Outdent line",
  "Mod-]": "Indent line",
  "Mod-Alt-\\": "Indent selection",
  "Shift-Mod-k": "Delete line",
  "Shift-Mod-\\": "Go to matching bracket",
  "Mod-/": "Toggle line comment",
  "Alt-A": "Toggle block comment",
  "Ctrl-A": "Toggle block comment",
  "Ctrl-m": "Toggle tab focus mode",
  "Shift-Alt-m": "Toggle tab focus mode",
  ArrowLeft: "Move cursor left",
  ArrowRight: "Move cursor right",
  ArrowUp: "Move cursor up",
  ArrowDown: "Move cursor down",
  "Mod-ArrowLeft": "Move by word left",
  "Mod-ArrowRight": "Move by word right",
  "Cmd-ArrowLeft": "Move to line start",
  "Cmd-ArrowRight": "Move to line end",
  "Cmd-ArrowUp": "Move to document start",
  "Cmd-ArrowDown": "Move to document end",
  "Ctrl-ArrowUp": "Move page up",
  "Ctrl-ArrowDown": "Move page down",
  PageUp: "Move page up",
  PageDown: "Move page down",
  Home: "Move to line start",
  End: "Move to line end",
  "Mod-Home": "Move to document start",
  "Mod-End": "Move to document end",
  Enter: "Insert newline",
  "Mod-a": "Select all",
  Backspace: "Delete character backward",
  Delete: "Delete character forward",
  "Mod-Backspace": "Delete word backward",
  "Mod-Delete": "Delete word forward",
  "Alt-Backspace": "Delete word backward",
  "Alt-Delete": "Delete word forward",
  "Ctrl-b": "Move cursor left",
  "Ctrl-f": "Move cursor right",
  "Ctrl-p": "Move cursor up",
  "Ctrl-n": "Move cursor down",
  "Ctrl-a": "Move to line start",
  "Ctrl-e": "Move to line end",
  "Ctrl-d": "Delete character forward",
  "Ctrl-h": "Delete character backward",
  "Ctrl-k": "Delete to line end",
  "Ctrl-Alt-h": "Delete word backward",
  "Ctrl-o": "Split line",
  "Ctrl-t": "Transpose characters",
  "Ctrl-v": "Move page down",
};

/** The palette name of a passthrough binding; `shift` names its Shift (extend-selection) variant. */
export function passthroughName(key: string, platform: EditorPlatform, shift: boolean): string {
  // On macOS Mod-Backspace/Delete are the line-boundary deletes; elsewhere they delete by word.
  let name = PASSTHROUGH_NAMES[key];
  if (platform === "mac" && key === "Mod-Backspace") name = "Delete to line start";
  if (platform === "mac" && key === "Mod-Delete") name = "Delete to line end";
  if (!name) return `Editor command (${key})`;
  if (!shift) return name;
  if (name.startsWith("Move cursor ")) return `Extend selection ${name.slice("Move cursor ".length)}`;
  if (name.startsWith("Move to ")) return `Extend selection to ${name.slice("Move to ".length)}`;
  if (name.startsWith("Move by ")) return `Extend selection by ${name.slice("Move by ".length)}`;
  if (name.startsWith("Move page ")) return `Extend selection ${name.slice("Move ".length)}`;
  return `${name} (extending selection)`;
}

/** A chord in the one form two spellings of it share: modifiers lowercased and sorted (`Shift-Mod-z` is
 * `Mod-Shift-z`). The key keeps its case: CodeMirror tells `Ctrl-a` from `Ctrl-A`, and the registered set
 * must stay what it was. */
export function normaliseChord(binding: string): string {
  const parts = chordParts(binding);
  const key = parts.pop() ?? "";
  return [...parts.map((part) => part.toLowerCase()).sort(), key].join("-");
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
    if (!seenKeys.has(normaliseChord(key))) {
      seenKeys.add(normaliseChord(key));
      const { run } = binding;
      registry.register({
        id: base,
        name: passthroughName(key, platform, false),
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
      if (!seenKeys.has(normaliseChord(shiftKey))) {
        seenKeys.add(normaliseChord(shiftKey));
        const shiftRun = binding.shift;
        registry.register({
          id: `${base}-shift`,
          name: passthroughName(key, platform, true),
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
  /** The app-wide registry to add to (the palette lists it); a fresh one when omitted. */
  registry: CommandRegistry = new CommandRegistry(),
  /** Test seam: replaces a named command's default binding, so the passthrough skip-set can be shown to follow it. */
  namedBindings: Readonly<Record<string, string>> = {},
): CommandRegistry {
  const bind = (id: string, fallback: string): string => namedBindings[id] ?? fallback;
  const preexisting = new Set(registry.list().map((c) => c.id));

  registry.register({
    id: "editor.save",
    name: "Save now",
    defaultBinding: bind("editor.save", "Mod-s"),
    when: "editor",
    run: handlers.save,
  });
  registry.register({
    id: "editor.toggle-live-preview",
    name: "Toggle live preview / split",
    defaultBinding: bind("editor.toggle-live-preview", "Mod-Shift-m"),
    run: () => handlers.toggleLivePreview(),
    // Global per components.md §7: works without editor focus and from the palette.
    exec: () => void handlers.toggleLivePreview(),
  });
  registry.register({
    id: "editor.follow-link",
    name: "Follow link/URL under cursor",
    defaultBinding: bind("editor.follow-link", "Mod-Enter"),
    when: "editor",
    run: handlers.followLink,
  });
  registry.register({
    id: "editor.undo",
    name: "Undo",
    defaultBinding: bind("editor.undo", "Mod-z"),
    when: "editor",
    run: (view) => undo(view),
  });
  registry.register({
    id: "editor.redo",
    name: "Redo",
    defaultBinding: bind("editor.redo", "Mod-y"),
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
    defaultBinding: bind("editor.markdown-newline", "Enter"),
    when: "editor",
    // An open completion popup takes Enter first (accept), exactly as the
    // autocompletion keymap would — but through this registry.
    run: (view) => acceptCompletion(view) || insertNewlineContinueMarkup(view) || insertNewlineAndIndent(view),
  });

  // Completion popup navigation: each key first drives the popup when one is
  // open (the command returns false otherwise) and then falls through to the
  // ordinary CodeMirror command for that key.
  const defaultRuns = new Map<string, (view: EditorView) => boolean>();
  for (const binding of defaultKeymap) {
    if (binding.key && binding.run && !defaultRuns.has(binding.key)) defaultRuns.set(binding.key, binding.run);
  }
  const popupKeys: Array<[string, string, string, (view: EditorView) => boolean]> = [
    ["ArrowDown", "editor.completion-next", "Next completion", moveCompletionSelection(true)],
    ["ArrowUp", "editor.completion-previous", "Previous completion", moveCompletionSelection(false)],
    ["PageDown", "editor.completion-page-next", "Next completion page", moveCompletionSelection(true, "page")],
    ["PageUp", "editor.completion-page-previous", "Previous completion page", moveCompletionSelection(false, "page")],
    ["Escape", "editor.completion-close", "Close completion", closeCompletion],
  ];
  for (const [key, id, name, popupRun] of popupKeys) {
    const fallback = defaultRuns.get(key);
    registry.register({
      id,
      name,
      defaultBinding: bind(id, key),
      when: "editor",
      run: (view) => popupRun(view) || (fallback ? fallback(view) : false),
    });
  }
  registry.register({
    id: "editor.start-completion",
    name: "Show completions",
    defaultBinding: bind("editor.start-completion", "Ctrl-Space"),
    when: "editor",
    run: startCompletion,
  });
  registry.register({
    id: "editor.markdown-delete-backward",
    name: "Delete backward (outdent Markdown marker)",
    defaultBinding: bind("editor.markdown-delete-backward", "Backspace"),
    when: "editor",
    run: (view) => deleteMarkupBackward(view) || deleteCharBackward(view),
  });

  // Keys already given an explicit, named command above are never double-registered under a generic
  // passthrough id (the same physical key under two ids). The set is DERIVED from what this function
  // registered (not a literal list that could drift from the bindings above, architecture §9.3), in the
  // normalised form `sameChord` compares. Shared across both passthrough calls and mutated by them, so a
  // key repeated between `historyKeymap` and `defaultKeymap`, or within either, is registered once.
  const seenKeys = new Set<string>();
  for (const command of registry.list()) {
    if (!preexisting.has(command.id) && command.defaultBinding) seenKeys.add(normaliseChord(command.defaultBinding));
  }
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
  overrides?: ReadonlyMap<CommandId, string | null>,
): Extension[] {
  // No argument: the registry's own (validated) overrides, so a TOML rebinding reaches the keymap.
  return [history(), keymap.of(registry.buildKeymap(overrides))];
}

/** NFR-PRF-02 / FR-EDT-01: CodeMirror must own the scroll. The editor fills its (bounded) column,
 * `.cm-scroller` scrolls, the document can scroll past its end, and the caret keeps a margin from
 * the scroller's edges. Margins exist because of the end/strict stall: with the caret flush against
 * the bottom edge Chromium reported 2-9 s presentation delays with an idle main thread. The height
 * bound alone fixes the ~124k px page (unscrollable editor); see style.css (.editor-column). */
export const EDITOR_SCROLL_MARGINS = { top: 24, bottom: 48 } as const;

export function editorScrollExtensions(): Extension[] {
  return [
    scrollPastEnd(),
    EditorView.scrollMargins.of(() => ({ ...EDITOR_SCROLL_MARGINS })),
    EditorView.theme({ "&": { height: "100%" }, ".cm-scroller": { overflow: "auto" } }),
  ];
}
