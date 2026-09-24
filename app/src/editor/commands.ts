// The command registry (architecture.md §9.3, FR-NAV-06, NFR-USE-01):
// "One front-end module owns every user-invocable action:
// register({ id, name, defaultBinding?, when?, run }). The palette lists
// the registry with each command's effective binding. The CodeMirror keymap
// is built from the registry, including the default editing and history
// commands, which are re-registered under Serat ids. `keymap.of` appears in
// exactly one place, and a test asserts that."
//
// This module is the registry. `buildEditorKeymap` (in `codemirrorSetup.ts`)
// is the one and only place `keymap.of` is called for the editor surface —
// other panes (tree, palette, switcher) register their own commands into
// the same registry as those screens land, but do not get their own
// `keymap.of`; a registry-wide keymap is out of scope for #21 and left for
// whichever task wires the tree/palette/switcher panes, so as not to design
// a second keymap-construction site into existence.

import type { EditorView, KeyBinding } from "@codemirror/view";

/** Matches `KeybindingOverride.command_id` shape 1:1 (`contracts/ipc.rs`) —
 * this registry's ids are exactly what the settings TOML keys rebindings by. */
export type CommandId = string;

/** The `when` contexts a P1 command can be scoped to — a subset of
 * components.md §7's section headings, "editor" being the one this task
 * populates. Kept open-ended (not a closed union) because later tasks
 * (#22 switcher-in-editor, tree, palette) add their own contexts and this
 * registry must not need a code change here to accept them. */
export type CommandContext = string;

export interface Command {
  id: CommandId;
  name: string;
  /** CodeMirror key notation (`Mod-s`), matching `KeybindingOverride.key`'s
   * documented shape. `undefined` means "no default binding" — a command
   * reachable only from the palette or a toolbar control. */
  defaultBinding?: string;
  /** `undefined` means global (architecture.md §9.1's "every primary action
   * has a keyboard binding and appears in the palette" applies regardless
   * of focus). */
  when?: CommandContext;
  run: (view: EditorView) => boolean;
}

/**
 * The registry NFR-USE-01 requires exactly one of, kept as a class rather
 * than a bare module singleton so tests can construct an isolated instance
 * (the shared default instance below is what the running app uses).
 */
export class CommandRegistry {
  private readonly commands = new Map<CommandId, Command>();

  /** Throws on a duplicate id — the registry's own invariant, not something
   * left to a test to notice after the fact. */
  register(command: Command): void {
    if (this.commands.has(command.id)) {
      throw new Error(`CommandRegistry: duplicate command id "${command.id}"`);
    }
    if (!command.name.trim()) {
      throw new Error(`CommandRegistry: command "${command.id}" has no name`);
    }
    this.commands.set(command.id, command);
  }

  get(id: CommandId): Command | undefined {
    return this.commands.get(id);
  }

  has(id: CommandId): boolean {
    return this.commands.has(id);
  }

  list(): Command[] {
    return [...this.commands.values()];
  }

  /**
   * Builds the CodeMirror keybinding array the sole `keymap.of` call
   * consumes. `overrides` is the user's `[keybindings]` TOML, as
   * `UiSettings.keybindings` (`KeybindingOverride[]`) — `key: null` means
   * "deliberately unbound" and drops the command from the keymap entirely,
   * per that type's own doc comment.
   *
   * Throws on any two commands sharing a binding in the same `when`
   * context — global and a specific `when` never collide with each other
   * (a global binding and a same-key `when`-scoped one are simultaneously
   * active only if CodeMirror's own `when`-less-first order picks one, and
   * that is a real ambiguity this registry refuses to ship rather than
   * silently resolve).
   */
  buildKeymap(overrides: ReadonlyMap<CommandId, string | null> = new Map()): KeyBinding[] {
    const seen = new Map<string, CommandId>();
    const bindings: KeyBinding[] = [];
    for (const command of this.commands.values()) {
      const override = overrides.get(command.id);
      const key = overrides.has(command.id) ? override : command.defaultBinding;
      if (!key) {
        continue;
      }
      const bucket = `${command.when ?? "*"}::${key}`;
      const existing = seen.get(bucket);
      if (existing) {
        throw new Error(
          `CommandRegistry: keybinding collision — "${key}" in context "${command.when ?? "*"}" is bound to both "${existing}" and "${command.id}"`,
        );
      }
      seen.set(bucket, command.id);
      bindings.push({
        key,
        preventDefault: true,
        run: (view) => command.run(view),
      });
    }
    return bindings;
  }
}

/** NFR-USE-01's "primary action" list for the editor surface, per
 * `editor.md`'s Keyboard map and `components.md` §7's Global/editor-local
 * rows this task is responsible for registering. Used by `commands.test.ts`
 * to assert every one of them actually landed in the registry — "the
 * 'Inspect' the requirement names, made repeatable" (§9.3). */
export const EDITOR_PRIMARY_ACTION_IDS: readonly CommandId[] = [
  "editor.save",
  "editor.toggle-live-preview",
  "editor.follow-link",
  "editor.undo",
  "editor.redo",
];
