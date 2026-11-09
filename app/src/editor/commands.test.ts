// architecture.md §9.3: "A unit test asserts that ids are unique, that
// every command has a name, that no two commands share a binding in the
// same `when` context, and that every entry in the NFR-USE-01 primary-action
// list is registered. That test is the 'Inspect' the requirement names,
// made repeatable." Also: "`keymap.of` appears in exactly one place, and a
// test asserts that."

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import { CommandRegistry, EDITOR_PRIMARY_ACTION_IDS } from "@/editor/commands";
import { createEditorCommandRegistry } from "@/editor/codemirrorSetup";

function noopHandler() {
  return true;
}

function buildDefaultRegistry(): CommandRegistry {
  return createEditorCommandRegistry({
    save: noopHandler,
    toggleLivePreview: noopHandler,
    followLink: noopHandler,
  });
}

describe("CommandRegistry", () => {
  it("refuses a duplicate id", () => {
    const registry = new CommandRegistry();
    registry.register({ id: "a", name: "A", run: () => true });
    expect(() => registry.register({ id: "a", name: "A again", run: () => true })).toThrow(/duplicate command id/);
  });

  it("refuses a command with no name", () => {
    const registry = new CommandRegistry();
    expect(() => registry.register({ id: "a", name: "  ", run: () => true })).toThrow(/has no name/);
  });

  it("throws when two commands in the same `when` context share a binding", () => {
    const registry = new CommandRegistry();
    registry.register({ id: "a", name: "A", defaultBinding: "Mod-b", when: "editor", run: () => true });
    registry.register({ id: "b", name: "B", defaultBinding: "Mod-b", when: "editor", run: () => true });
    expect(() => registry.buildKeymap()).toThrow(/keybinding collision/);
  });

  it("allows the same key in two different `when` contexts", () => {
    const registry = new CommandRegistry();
    registry.register({ id: "a", name: "A", defaultBinding: "Mod-f", when: "tree", run: () => true });
    registry.register({ id: "b", name: "B", defaultBinding: "Mod-f", when: "search-panel", run: () => true });
    expect(() => registry.buildKeymap()).not.toThrow();
  });

  it("a user override replaces the default binding", () => {
    const registry = new CommandRegistry();
    registry.register({ id: "a", name: "A", defaultBinding: "Mod-b", when: "editor", run: () => true });
    const bindings = registry.buildKeymap(new Map([["a", "Mod-Shift-b"]]));
    expect(bindings).toHaveLength(1);
    expect(bindings[0].key).toBe("Mod-Shift-b");
  });

  it("an override of `null` unbinds a command entirely", () => {
    const registry = new CommandRegistry();
    registry.register({ id: "a", name: "A", defaultBinding: "Mod-b", when: "editor", run: () => true });
    const bindings = registry.buildKeymap(new Map([["a", null]]));
    expect(bindings).toHaveLength(0);
  });

  it("a command with no default binding and no override is simply absent from the keymap", () => {
    const registry = new CommandRegistry();
    registry.register({ id: "a", name: "Palette-only", run: () => true });
    expect(registry.buildKeymap()).toHaveLength(0);
    expect(registry.list()).toHaveLength(1);
  });
});

describe("the default editor registry", () => {
  const registry = buildDefaultRegistry();

  it("has unique ids by construction (register() enforces it) and every command has a name", () => {
    for (const command of registry.list()) {
      expect(command.name.trim().length).toBeGreaterThan(0);
    }
    const ids = registry.list().map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("has no two commands sharing a binding in the same `when` context", () => {
    expect(() => registry.buildKeymap()).not.toThrow();
  });

  it("registers every NFR-USE-01 primary-action id named in editor.md / components.md §7", () => {
    for (const id of EDITOR_PRIMARY_ACTION_IDS) {
      expect(registry.has(id), `missing command "${id}"`).toBe(true);
    }
  });

  it("re-registers CodeMirror's own default editing and history commands under Serat ids", () => {
    const ids = registry.list().map((c) => c.id);
    expect(ids.some((id) => id.startsWith("editor.cm."))).toBe(true);
    expect(ids.some((id) => id.startsWith("editor.cm-history."))).toBe(true);
  });

  it("builds a non-empty keymap with the documented default bindings", () => {
    const bindings = registry.buildKeymap();
    const byKey = new Map(bindings.map((b) => [b.key, b]));
    expect(byKey.has("Mod-s")).toBe(true); // Save now — editor.md, components.md §7
    expect(byKey.has("Mod-Shift-m")).toBe(true); // Toggle live preview
    expect(byKey.has("Mod-Enter")).toBe(true); // Follow link
  });
});

describe("keymap.of is called in exactly one place", () => {
  it("scans src/ for literal keymap.of( calls outside tests", () => {
    const srcRoot = join(__dirname, "..");
    const hits: string[] = [];

    function walk(dir: string): void {
      for (const entry of readdirSync(dir)) {
        const full = join(dir, entry);
        const stat = statSync(full);
        if (stat.isDirectory()) {
          walk(full);
          continue;
        }
        if (!/\.(ts|vue)$/.test(entry) || /\.test\.ts$/.test(entry)) {
          continue;
        }
        const text = readFileSync(full, "utf-8");
        if (text.includes("keymap.of(")) {
          hits.push(full);
        }
      }
    }

    walk(srcRoot);
    expect(hits).toEqual([join(srcRoot, "editor", "codemirrorSetup.ts")]);
  });
});

describe("view-less (global) commands in the one registry (architecture.md 9.3, FR-NAV-06)", () => {
  const press = (init: KeyboardEventInit) => new KeyboardEvent("keydown", init);

  function withGlobals(): CommandRegistry {
    const registry = buildDefaultRegistry();
    for (const [id, key] of [
      ["switcher.open", "Mod-o"],
      ["palette.open", "Mod-k"],
      ["search.open", "Mod-Shift-f"],
      ["tree.focus", "Mod-1"],
      ["backlinks.focus", "Mod-3"],
    ] as const) {
      registry.register({ id, name: id, defaultBinding: key, exec: () => {} });
    }
    return registry;
  }

  it("global commands share the uniqueness/collision invariant with the editor's", () => {
    const registry = withGlobals();
    expect(() => registry.buildKeymap()).not.toThrow();
    const ids = registry.list().map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
    registry.register({ id: "x.dup", name: "Dup", defaultBinding: "Mod-o", exec: () => {} });
    expect(() => registry.buildKeymap()).toThrow(/keybinding collision/);
  });

  it("view-less commands stay out of the CodeMirror keymap", () => {
    const keys = withGlobals().buildKeymap().map((b) => b.key);
    expect(keys).not.toContain("Mod-o");
  });

  it("matchGlobal follows the effective binding: an override moves the chord", () => {
    const registry = withGlobals();
    expect(registry.matchGlobal(press({ key: "o", ctrlKey: true }), false)?.id).toBe("switcher.open");
    expect(registry.matchGlobal(press({ key: "o", metaKey: true }), true)?.id).toBe("switcher.open");
    expect(registry.matchGlobal(press({ key: "k", ctrlKey: true, metaKey: false }), true)).toBeUndefined(); // mac: Ctrl is not Mod
    expect(registry.matchGlobal(press({ key: "F", ctrlKey: true, shiftKey: true }), false)?.id).toBe("search.open");
    registry.setOverrides([{ command_id: "switcher.open", key: "Mod-p" }]);
    expect(registry.matchGlobal(press({ key: "o", ctrlKey: true }), false)).toBeUndefined();
    expect(registry.matchGlobal(press({ key: "p", ctrlKey: true }), false)?.id).toBe("switcher.open");
    registry.setOverride("switcher.open", null);
    expect(registry.effectiveBinding("switcher.open")).toBeNull();
  });

  it("execute: runs exec, refuses with the unavailable reason, needs a view for editor commands", async () => {
    const registry = new CommandRegistry();
    const exec = vi.fn();
    registry.register({ id: "g", name: "G", exec });
    registry.register({ id: "u", name: "U", unavailable: () => "Select a file first", exec });
    registry.register({ id: "e", name: "E", run: () => true });
    expect(await registry.execute("g", null)).toBeNull();
    expect(await registry.execute("u", null)).toBe("Select a file first");
    expect(await registry.execute("e", null)).toBe("Open a note first");
    expect(exec).toHaveBeenCalledTimes(1);
    expect(registry.ranked()[0].id).toBe("g"); // most-run first
  });

  it("register returns a disposer; subscribe hears changes", () => {
    const registry = new CommandRegistry();
    const heard = vi.fn();
    registry.subscribe(heard);
    const off = registry.register({ id: "a", name: "A", exec: () => {} });
    off();
    expect(registry.has("a")).toBe(false);
    expect(heard).toHaveBeenCalledTimes(2);
  });

  it("refuses a command with neither run nor exec", () => {
    expect(() => new CommandRegistry().register({ id: "n", name: "N" })).toThrow(/neither/);
  });
});
