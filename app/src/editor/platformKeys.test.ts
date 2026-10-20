// Passthrough keymaps are chosen per platform as CodeMirror itself does, so
// Windows (the reference platform) never loses native paste/caret keys to
// mac-only emacs bindings.

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import { createEditorCommandRegistry } from "@/editor/codemirrorSetup";

const yes = () => true;

const keysFor = (platform: "mac" | "win" | "linux") =>
  new Map(
    createEditorCommandRegistry({ save: yes, toggleLivePreview: yes, followLink: yes }, platform)
      .buildKeymap()
      .map((b) => [b.key, b] as const),
  );

describe("per-platform passthrough keys", () => {
  it("does not bind the mac-only emacs keys on Windows or Linux (native paste etc. survive)", () => {
    for (const platform of ["win", "linux"] as const) {
      const keys = keysFor(platform);
      for (const emacs of ["Ctrl-v", "Ctrl-k", "Ctrl-f", "Ctrl-d", "Ctrl-h", "Ctrl-t", "Ctrl-b", "Ctrl-n", "Ctrl-p"]) {
        expect(keys.has(emacs), `${platform} ${emacs}`).toBe(false);
      }
    }
    expect(keysFor("mac").has("Ctrl-v")).toBe(true);
  });

  it("binds the Shift variants that extend the selection", () => {
    const keys = keysFor("win");
    expect(keys.has("Shift-ArrowLeft")).toBe(true);
    expect(keys.has("Shift-Home")).toBe(true);
  });

  it("Shift-ArrowLeft really extends the selection through the registry", () => {
    const view = new EditorView({ state: EditorState.create({ doc: "abc", selection: { anchor: 2 } }) });
    keysFor("win").get("Shift-ArrowLeft")!.run!(view);
    expect(view.state.selection.main.anchor).toBe(2);
    expect(view.state.selection.main.head).toBe(1);
    view.destroy();
  });

  it("redoes with Ctrl-Shift-z on Linux, Mod-Shift-z on mac, Mod-y on Windows", () => {
    expect(keysFor("linux").has("Ctrl-Shift-z")).toBe(true);
    expect(keysFor("win").has("Mod-y")).toBe(true);
    expect(keysFor("mac").has("Mod-Shift-z")).toBe(true);
  });

  it("passthrough bindings do not cancel keys the command declines", () => {
    expect(keysFor("win").get("Mod-a")?.preventDefault).toBe(false);
  });
});
