// One canonical chord form, used by every site that compares chords (task #135; NFR-USE-01, architecture §9.3):
// the registry's collision check and keymap build, the global-chord matcher, and the editor's passthrough
// skip-set. A binding spelled Mod-k, Cmd-k, Meta-k or Ctrl-k is the same chord per platform at all four.

import { describe, expect, it } from "vitest";

import { createEditorCommandRegistry } from "@/editor/codemirrorSetup";
import { chordMatches, CommandRegistry, normaliseChord, type EditorPlatform } from "@/editor/commands";

interface Group {
  platform: EditorPlatform;
  /** Spellings of one chord on that platform. */
  spellings: string[];
  canonical: string;
  /** The keydown that presses it. */
  event: { key: string; ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean };
  /** CodeMirror's own defaultKeymap binds it, so the passthrough skip-set is exercised. */
  inCodeMirror?: boolean;
}

const TABLE: Group[] = [
  { platform: "linux", spellings: ["Mod-k", "Ctrl-k", "Control-k", "c-k"], canonical: "ctrl-k", event: { key: "k", ctrlKey: true } },
  { platform: "win", spellings: ["Mod-k", "Ctrl-k", "Control-k"], canonical: "ctrl-k", event: { key: "k", ctrlKey: true } },
  { platform: "mac", spellings: ["Mod-k", "Cmd-k", "Meta-k", "m-k"], canonical: "meta-k", event: { key: "k", metaKey: true } },
  { platform: "linux", spellings: ["Mod-Shift-z", "Shift-Mod-z", "Ctrl-Shift-z", "Mod-Z"], canonical: "ctrl-shift-z", event: { key: "z", ctrlKey: true, shiftKey: true } },
  { platform: "mac", spellings: ["Mod-Shift-z", "Shift-Mod-z", "Cmd-Shift-z", "Mod-Z"], canonical: "meta-shift-z", event: { key: "z", metaKey: true, shiftKey: true } },
  { platform: "linux", spellings: ["Mod-[", "Ctrl-["], canonical: "ctrl-[", event: { key: "[", ctrlKey: true }, inCodeMirror: true },
  { platform: "mac", spellings: ["Mod-[", "Cmd-[", "Meta-["], canonical: "meta-[", event: { key: "[", metaKey: true }, inCodeMirror: true },
];

const noop = () => undefined;
const handlers = { save: noop, toggleLivePreview: noop, followLink: noop } as never;
const keyEvent = (init: Group["event"]) => ({ ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...init }) as unknown as KeyboardEvent;

describe.each(TABLE.map((g) => [`${g.platform}: ${g.spellings[0]}`, g] as const))("%s", (_name, group) => {
  const { platform, spellings } = group;

  it("normaliseChord: every spelling has the one canonical form", () => {
    for (const spelling of spellings) expect(normaliseChord(spelling, platform), spelling).toBe(group.canonical);
  });

  it("chordMatches: every spelling matches the keydown, and not a near miss", () => {
    for (const spelling of spellings) {
      expect(chordMatches(spelling, keyEvent(group.event), platform === "mac"), spelling).toBe(true);
      expect(chordMatches(spelling, keyEvent({ ...group.event, altKey: true }), platform === "mac"), `${spelling} + Alt`).toBe(false);
      expect(chordMatches(spelling, keyEvent({ key: "q" }), platform === "mac"), `${spelling} on q`).toBe(false);
    }
  });

  it("collidesWith: an override spelled any other way is refused", () => {
    const registry = new CommandRegistry(platform);
    registry.register({ id: "a", name: "A", defaultBinding: spellings[0], exec: noop });
    registry.register({ id: "b", name: "B", defaultBinding: "Alt-q", exec: noop });
    for (const spelling of spellings.slice(1)) expect(registry.setOverride("b", spelling), spelling).toBe(false);
    expect(registry.setOverrides([{ command_id: "b", key: spellings[1] }])).toEqual(["b"]);
  });

  it("buildKeymap: two editor commands defaulting to different spellings of it collide", () => {
    const registry = new CommandRegistry(platform);
    registry.register({ id: "a", name: "A", defaultBinding: spellings[0], when: "editor", run: () => true });
    registry.register({ id: "b", name: "B", defaultBinding: spellings[1], when: "editor", run: () => true });
    expect(() => registry.buildKeymap()).toThrow(/keybinding collision/);
  });

  it("matchGlobal: a rebinding in another spelling still matches the keydown", () => {
    const registry = new CommandRegistry(platform);
    registry.register({ id: "a", name: "A", defaultBinding: "Alt-q", exec: noop });
    expect(registry.setOverride("a", spellings[spellings.length - 1])).toBe(true);
    expect(registry.matchGlobal(keyEvent(group.event), platform === "mac")?.id).toBe("a");
  });

  if (group.inCodeMirror) {
    it("passthrough skip-set: a named command on any spelling suppresses CodeMirror's own binding of that chord", () => {
      for (const spelling of spellings) {
        const registry = createEditorCommandRegistry(handlers, platform, undefined, { "editor.save": spelling });
        const holders = registry.list().filter((c) => normaliseChord(c.defaultBinding ?? "", platform) === group.canonical);
        expect(holders.map((c) => c.id), spelling).toEqual(["editor.save"]);
      }
    });
  }
});

describe("tree-reserved chords use the same canonical form", () => {
  it("Ctrl-Enter on linux (= Mod-Enter) and Cmd-f on mac (= Mod-f) are refused for a tree command", () => {
    const linux = new CommandRegistry("linux");
    linux.register({ id: "t", name: "T", when: "tree", exec: noop, defaultBinding: "Alt-q" });
    expect(linux.setOverride("t", "Ctrl-Enter")).toBe(false);
    const mac = new CommandRegistry("mac");
    mac.register({ id: "t", name: "T", when: "tree", exec: noop, defaultBinding: "Alt-q" });
    expect(mac.setOverride("t", "Cmd-f")).toBe(false);
    expect(mac.setOverride("t", "Ctrl-f")).toBe(true); // Ctrl is not Mod on mac
  });
});

describe("CodeMirror's own entries for one folded chord", () => {
  it("the upstream mac pair Ctrl-A and Shift-Ctrl-a both stay registered, in order, and the keymap builds", () => {
    const registry = createEditorCommandRegistry(handlers, "mac");
    const pair = registry.list().filter((c) => c.fallthrough && normaliseChord(c.defaultBinding ?? "", "mac") === "ctrl-shift-a");
    expect(pair.map((c) => c.defaultBinding).sort()).toEqual(["Ctrl-A", "Shift-Ctrl-a"].sort());
    const keys = registry.buildKeymap().map((b) => b.key);
    expect(keys).toContain("Ctrl-A");
    expect(keys).toContain("Shift-Ctrl-a");
  });
});
