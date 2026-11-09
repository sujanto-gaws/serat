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
import type { InjectionKey } from "vue";

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
  /** Whether the key press is cancelled even when `run` returns false. Defaults
   * to true; passthrough bindings carry CodeMirror's own value so native keys
   * (paste, caret movement the editor does not handle) are not swallowed. */
  preventDefault?: boolean;
  /** Editor-scoped action: runs against the editor's view (CodeMirror keymap). */
  run?: (view: EditorView) => boolean;
  /** View-less action (switcher, palette, panes): runs from the global keydown
   * handler built from this registry, or from the palette. Needs no editor. */
  exec?: () => void | Promise<void>;
  /** Context check (command-palette.md "unavailable in context"): `null` when
   * the command can run now, else a one-line reason shown instead of running. */
  unavailable?: () => string | null;
}

/**
 * The registry NFR-USE-01 requires exactly one of, kept as a class rather
 * than a bare module singleton so tests can construct an isolated instance
 * (the shared default instance below is what the running app uses).
 */
export class CommandRegistry {
  private readonly commands = new Map<CommandId, Command>();
  private readonly overrides = new Map<CommandId, string | null>();
  private readonly runCounts = new Map<CommandId, number>();
  private readonly listeners = new Set<() => void>();

  /** Calls `listener` after any register/unregister/override. Returns the unsubscribe. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }

  /** Throws on a duplicate id — the registry's own invariant, not something
   * left to a test to notice after the fact. */
  register(command: Command): () => void {
    if (this.commands.has(command.id)) {
      throw new Error(`CommandRegistry: duplicate command id "${command.id}"`);
    }
    if (!command.name.trim()) {
      throw new Error(`CommandRegistry: command "${command.id}" has no name`);
    }
    if (!command.run && !command.exec) {
      throw new Error(`CommandRegistry: command "${command.id}" has neither run nor exec`);
    }
    this.commands.set(command.id, command);
    this.changed();
    return () => this.unregister(command.id);
  }

  /** Removes a command (an editor going away takes its commands with it). */
  unregister(id: CommandId): void {
    if (this.commands.delete(id)) this.changed();
  }

  /**
   * NFR-USE-01: the user's rebinding, `[keybindings]` in the settings TOML.
   * `null` unbinds; `undefined` restores the default. Rebind UI, conflict
   * handling and `set_keybinding` are task #93; this is the read side.
   */
  setOverride(id: CommandId, key: string | null | undefined): boolean {
    if (key === undefined) this.overrides.delete(id);
    else if (this.collidesWith(id, key) === null) this.overrides.set(id, key);
    else return false; // refused: it would share a chord in the same `when` context
    this.changed();
    return true;
  }

  /**
   * Replaces all overrides at once (`UiSettings.keybindings` at startup).
   * An override that would share a chord with another command in the same
   * `when` context is dropped (not applied) and its command id returned, so
   * `buildKeymap` can never throw on user data: a throw inside the editor's
   * state replacement would strand the old text against a new note's base.
   * Overrides naming no registered command are kept (the command may register later).
   */
  setOverrides(overrides: ReadonlyArray<{ command_id: string; key: string | null }>): CommandId[] {
    this.overrides.clear();
    const refused: CommandId[] = [];
    for (const o of overrides) {
      if (o.key !== null && this.collidesWith(o.command_id, o.key) !== null) refused.push(o.command_id);
      else this.overrides.set(o.command_id, o.key);
    }
    this.changed();
    return refused;
  }

  /** The other command that already holds `key` in `id`'s `when` context, if any. */
  private collidesWith(id: CommandId, key: string | null): CommandId | null {
    if (key === null) return null;
    const mine = this.commands.get(id);
    const bucket = mine?.when ?? "*";
    for (const other of this.commands.values()) {
      if (other.id === id || (other.when ?? "*") !== bucket) continue;
      if (this.effectiveBinding(other.id) === key) return other.id;
    }
    return null;
  }

  /** The binding in force: the override (`null` = unbound) else the default. */
  effectiveBinding(id: CommandId): string | null {
    if (this.overrides.has(id)) return this.overrides.get(id) ?? null;
    return this.commands.get(id)?.defaultBinding ?? null;
  }

  /** Palette order: most-run first, then by name (command-palette.md "Default"). */
  ranked(): Command[] {
    return this.list().sort(
      (a, b) => (this.runCounts.get(b.id) ?? 0) - (this.runCounts.get(a.id) ?? 0) || a.name.localeCompare(b.name),
    );
  }

  /**
   * Runs a command from the palette or a global chord. Returns the reason it
   * did not run (unavailable, or an editor command with no editor), else null.
   */
  async execute(id: CommandId, view: EditorView | null): Promise<string | null> {
    const command = this.commands.get(id);
    if (!command) return "Unknown command";
    const reason = command.unavailable?.() ?? null;
    if (reason !== null) return reason;
    if (command.exec) {
      this.runCounts.set(id, (this.runCounts.get(id) ?? 0) + 1);
      await command.exec();
      return null;
    }
    if (!view) return "Open a note first";
    this.runCounts.set(id, (this.runCounts.get(id) ?? 0) + 1);
    command.run?.(view);
    return null;
  }

  /** The view-less global command whose effective binding `event` presses, if any. */
  matchGlobal(event: KeyboardEvent, mac: boolean): Command | undefined {
    for (const command of this.commands.values()) {
      if (!command.exec || command.when) continue;
      const binding = this.effectiveBinding(command.id);
      if (binding && chordMatches(binding, event, mac)) return command;
    }
    return undefined;
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
  buildKeymap(overrides: ReadonlyMap<CommandId, string | null> = this.overrides): KeyBinding[] {
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
      if (!command.run) continue; // view-less: handled by the global keydown handler
      const run = command.run;
      bindings.push({
        key,
        preventDefault: command.preventDefault ?? true,
        run: (view) => run(view),
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

/** Splits CodeMirror key notation: `Mod-Shift-m` -> ["Mod","Shift","m"]; `Mod--` -> ["Mod","-"]. */
export function chordParts(binding: string): string[] {
  return binding.split(/-(?!$)/);
}

/** Does `event` press `binding` (CodeMirror notation, `Mod` = Cmd on mac, Ctrl elsewhere)? */
export function chordMatches(binding: string, event: KeyboardEvent, mac: boolean): boolean {
  const parts = chordParts(binding);
  const key = (parts.pop() ?? "").toLowerCase();
  const wants = new Set(parts.map((p) => p.toLowerCase()));
  const mod = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (wants.has("mod") !== mod) return false;
  if (wants.has("shift") !== event.shiftKey) return false;
  if (wants.has("alt") !== event.altKey) return false;
  return event.key.toLowerCase() === key;
}

const GLYPHS: Record<string, [string, string]> = {
  mod: ["⌘", "Ctrl"],
  shift: ["⇧", "Shift"],
  alt: ["⌥", "Alt"],
  enter: ["↵", "Enter"],
  escape: ["⎋", "Esc"],
};
const SPOKEN: Record<string, [string, string]> = {
  mod: ["Command", "Control"],
  shift: ["Shift", "Shift"],
  alt: ["Option", "Alt"],
  enter: ["Enter", "Enter"],
  escape: ["Escape", "Escape"],
};

function mapParts(binding: string, table: Record<string, [string, string]>, mac: boolean): string[] {
  return chordParts(binding).map((part) => {
    const hit = table[part.toLowerCase()];
    return hit ? hit[mac ? 0 : 1] : part.length === 1 ? part.toUpperCase() : part;
  });
}

/** Display chips ("⌘", "⇧", "M") for a binding. */
export function chordGlyphs(binding: string, mac: boolean): string[] {
  return mapParts(binding, GLYPHS, mac);
}

/** Spoken names for the accessible label: "Command", "Shift", "M". */
export function chordSpoken(binding: string, mac: boolean): string[] {
  return mapParts(binding, SPOKEN, mac);
}

/** `provide`/`inject` key for the app-wide registry (`App.vue` provides it). */
export const COMMANDS_KEY: InjectionKey<CommandRegistry> = Symbol("serat.commands");
