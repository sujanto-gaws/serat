// The passthrough skip-set is derived from the named editor commands actually registered, not a hand-typed
// list (architecture §9.3; NFR-USE-01, NFR-MNT-01).

import { describe, expect, it } from "vitest";

import { createEditorCommandRegistry, normaliseChord, type EditorPlatform } from "@/editor/codemirrorSetup";
import expected from "@/editor/__fixtures__/editorCommandIds.json";

const handlers = { save: () => undefined, toggleLivePreview: () => undefined, followLink: () => undefined } as never;

const snapshot = (platform: EditorPlatform, named?: Record<string, string>) =>
  createEditorCommandRegistry(handlers, platform, undefined, named)
    .list()
    .map((c) => `${c.id}=${c.defaultBinding ?? ""}`);

describe("derived seenKeys", () => {
  it.each(["mac", "win", "linux"] as const)(
    "%s: the registered ids and bindings are exactly what the hand-typed list produced",
    (platform) => {
      // fixture captured from the literal-list implementation, before it was deleted
      expect(snapshot(platform)).toEqual(expected[platform]);
    },
  );

  it("changing a named command's default binding leaves no duplicate id and no passthrough for the old key", () => {
    const registry = createEditorCommandRegistry(handlers, "linux", undefined, { "editor.undo": "Mod-Alt-u" });
    const ids = registry.list().map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
    // the old key (Mod-z) is now free, so CodeMirror's own binding for it is registered, once
    const onModZ = registry.list().filter((c) => c.defaultBinding === "Mod-z");
    expect(onModZ).toHaveLength(1);
    expect(onModZ[0].id).toMatch(/^editor\.cm-history\./);
    // the new key is held only by the named command
    expect(registry.list().filter((c) => c.defaultBinding === "Mod-Alt-u").map((c) => c.id)).toEqual(["editor.undo"]);
    expect(() => registry.buildKeymap()).not.toThrow();
  });

  it("a named command moved onto a key CodeMirror also binds is not registered twice", () => {
    const registry = createEditorCommandRegistry(handlers, "linux", undefined, { "editor.save": "Mod-a" });
    const onModA = registry.list().filter((c) => c.defaultBinding === "Mod-a");
    expect(onModA.map((c) => c.id)).toEqual(["editor.save"]);
    expect(() => registry.buildKeymap()).not.toThrow();
  });

  it("modifier spelling and order do not defeat the skip", () => {
    expect(normaliseChord("Shift-Mod-z", "linux")).toBe(normaliseChord("Mod-Shift-z", "linux"));
    expect(normaliseChord("Ctrl-a", "linux")).not.toBe(normaliseChord("Ctrl-A", "linux"));
    const registry = createEditorCommandRegistry(handlers, "linux", undefined, { "editor.undo": "Shift-Mod-z" });
    const onChord = registry.list().filter((c) => normaliseChord(c.defaultBinding ?? "", "linux") === normaliseChord("Mod-Shift-z", "linux"));
    expect(onChord.map((c) => c.id)).toEqual(["editor.undo"]);
  });

  it("Mod is the platform modifier: a named Ctrl-Shift-z on linux (Cmd-Shift-z on mac) suppresses the Mod-Shift-z passthrough", () => {
    expect(normaliseChord("Mod-Shift-z", "linux")).toBe(normaliseChord("Ctrl-Shift-z", "linux"));
    expect(normaliseChord("Mod-Shift-z", "mac")).toBe(normaliseChord("Cmd-Shift-z", "mac"));
    expect(normaliseChord("Mod-Shift-z", "mac")).toBe(normaliseChord("Meta-Shift-z", "mac"));
    expect(normaliseChord("Mod-s", "mac")).not.toBe(normaliseChord("Ctrl-s", "mac"));
    const linux = createEditorCommandRegistry(handlers, "linux", undefined, { "editor.save": "Ctrl-Shift-z" });
    expect(linux.list().filter((c) => c.defaultBinding === "Mod-Shift-z")).toHaveLength(0);
    const mac = createEditorCommandRegistry(handlers, "mac", undefined, { "editor.save": "Cmd-Shift-z" });
    expect(mac.list().filter((c) => c.defaultBinding === "Mod-Shift-z")).toHaveLength(0);
  });

  it("an uppercase letter means Shift: a named Mod-Z suppresses the Mod-Shift-z passthrough, and Ctrl-a stays distinct from Ctrl-A", () => {
    expect(normaliseChord("Mod-Z", "linux")).toBe(normaliseChord("Mod-Shift-z", "linux"));
    expect(normaliseChord("Ctrl-a", "mac")).toBe("ctrl-a");
    expect(normaliseChord("Ctrl-A", "mac")).toBe("ctrl-shift-a");
    const mac = createEditorCommandRegistry(handlers, "mac", undefined, { "editor.save": "Mod-Z" });
    expect(mac.list().filter((c) => c.defaultBinding === "Mod-Shift-z")).toHaveLength(0);
    // the upstream mac pair (Ctrl-A and Shift-Ctrl-a) fold together but are both registered: the fold is
    // applied against the named set only
    const ids = createEditorCommandRegistry(handlers, "mac").list().map((c) => c.defaultBinding);
    expect(ids).toContain("Ctrl-a");
    expect(ids).toContain("Ctrl-A");
    expect(ids).toContain("Shift-Ctrl-a");
  });

  it.each(["mac", "win", "linux"] as const)("%s: the named set is exactly the 13 named defaults, and no passthrough shares a chord with one", (platform) => {
    const list = createEditorCommandRegistry(handlers, platform).list();
    const named = list.filter((c) => !c.id.startsWith("editor.cm"));
    expect(named.map((c) => c.defaultBinding).sort()).toEqual(
      ["Mod-s", "Mod-Shift-m", "Mod-Enter", "Mod-z", "Mod-y", "Enter", "Backspace", "ArrowDown", "ArrowUp", "PageDown", "PageUp", "Escape", "Ctrl-Space"].sort(),
    );
    const folded = new Set(named.map((c) => normaliseChord(c.defaultBinding ?? "", platform)));
    for (const c of list.filter((x) => x.id.startsWith("editor.cm"))) {
      expect(folded.has(normaliseChord(c.defaultBinding ?? "", platform))).toBe(false);
    }
  });
});
