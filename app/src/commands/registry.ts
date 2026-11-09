// The front-end command registry (architecture.md §9.3; FR-NAV-05, FR-NAV-06,
// NFR-USE-01). Every action the command palette lists is registered here, and
// other code adds to it with `register` (which returns the disposer). Bindings
// are strings such as "Mod+Shift+M" (`Mod` is ⌘ on macOS, Ctrl elsewhere).
//
// Not yet: persisting rebindings (`set_keybinding` exists in the contract; the
// rebind editor of command-palette.md is a later task) and per-command
// frecency persistence (the contract has no command history). Both are held in
// memory here so the palette's ranking shape is in place.

import { shallowRef, type ShallowRef } from "vue";

export interface Command {
  /** Stable internal id, e.g. `editor.togglePreview`. Never shown. */
  id: string;
  /** What the user reads, e.g. "Toggle live preview". */
  name: string;
  /** Default chord, e.g. "Mod+K"; omitted for palette-only commands. */
  binding?: string;
  /**
   * The `when` clause: returns `null` when the command can run now, else a
   * one-line reason (shown instead of running — command-palette.md).
   */
  unavailable?: () => string | null;
  run: () => void | Promise<void>;
}

export interface CommandRegistry {
  /** Adds (or replaces) a command; returns a function that removes it again. */
  register(command: Command): () => void;
  get(id: string): Command | undefined;
  /** All commands in default palette order: most-run first, then by name. */
  list(): Command[];
  /** The user's rebinding when one exists (`null` = explicitly unbound), else the default. */
  effectiveBinding(id: string): string | null;
  /** `undefined` removes the override (back to the default), `null` unbinds. */
  setOverride(id: string, binding: string | null | undefined): void;
  /** Runs a command unless its `when` clause refuses; returns the reason if refused. */
  run(id: string): Promise<string | null>;
  /** Pairs of command ids sharing one effective binding (the §9.3 invariant; should be empty). */
  conflicts(): Array<[string, string]>;
  /** Bumps on every change; read inside a computed/render to subscribe. */
  readonly version: ShallowRef<number>;
}

export function createCommandRegistry(): CommandRegistry {
  const commands = new Map<string, Command>();
  const overrides = new Map<string, string | null>();
  const runs = new Map<string, number>();
  const version = shallowRef(0);
  const bump = (): void => {
    version.value++;
  };

  const effectiveBinding = (id: string): string | null => {
    if (overrides.has(id)) return overrides.get(id) ?? null;
    return commands.get(id)?.binding ?? null;
  };

  return {
    version,
    register(command) {
      commands.set(command.id, command);
      bump();
      return () => {
        if (commands.get(command.id) === command) {
          commands.delete(command.id);
          bump();
        }
      };
    },
    get: (id) => commands.get(id),
    list() {
      return [...commands.values()].sort(
        (a, b) => (runs.get(b.id) ?? 0) - (runs.get(a.id) ?? 0) || a.name.localeCompare(b.name),
      );
    },
    effectiveBinding,
    setOverride(id, binding) {
      if (binding === undefined) overrides.delete(id);
      else overrides.set(id, binding);
      bump();
    },
    async run(id) {
      const command = commands.get(id);
      if (!command) return "unknown command";
      const reason = command.unavailable?.() ?? null;
      if (reason !== null) return reason;
      runs.set(id, (runs.get(id) ?? 0) + 1);
      await command.run();
      return null;
    },
    conflicts() {
      const byBinding = new Map<string, string>();
      const found: Array<[string, string]> = [];
      for (const id of commands.keys()) {
        const binding = effectiveBinding(id);
        if (binding === null) continue;
        const key = binding.toLowerCase();
        const other = byBinding.get(key);
        if (other !== undefined) found.push([other, id]);
        else byBinding.set(key, id);
      }
      return found;
    },
  };
}

/** The app-wide registry. Tests create their own with `createCommandRegistry`. */
export const commandRegistry: CommandRegistry = createCommandRegistry();

/** Chord parts for display chips: `Mod+Shift+M` -> ["⌘","⇧","M"] (macOS) or ["Ctrl","Shift","M"]. */
export function chordParts(binding: string, mac: boolean): string[] {
  const glyph: Record<string, [string, string]> = {
    mod: ["⌘", "Ctrl"],
    shift: ["⇧", "Shift"],
    alt: ["⌥", "Alt"],
    enter: ["↵", "Enter"],
    esc: ["⎋", "Esc"],
  };
  return binding.split("+").map((part) => {
    const g = glyph[part.toLowerCase()];
    return g ? g[mac ? 0 : 1] : part.length === 1 ? part.toUpperCase() : part;
  });
}

export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform ?? "");
}
