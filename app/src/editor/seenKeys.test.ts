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
    expect(normaliseChord("Shift-Mod-z")).toBe(normaliseChord("Mod-Shift-z"));
    expect(normaliseChord("Ctrl-a")).not.toBe(normaliseChord("Ctrl-A"));
    const registry = createEditorCommandRegistry(handlers, "linux", undefined, { "editor.undo": "Shift-Mod-z" });
    const onChord = registry.list().filter((c) => normaliseChord(c.defaultBinding ?? "") === normaliseChord("Mod-Shift-z"));
    expect(onChord.map((c) => c.id)).toEqual(["editor.undo"]);
  });
});
