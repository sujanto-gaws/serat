// Task #25: quick switcher and command palette (FR-NAV-01, FR-NAV-05,
// NFR-PRF-06, NFR-USE-01, US-04; architecture.md §9.2, §9.3).

import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import { chordParts, createCommandRegistry } from "@/commands/registry";
import type { SwitcherItem } from "@/ipc/generated/SwitcherItem";
import type { SwitcherQueryArgs } from "@/ipc/generated/SwitcherQueryArgs";
import type { SwitcherResults } from "@/ipc/generated/SwitcherResults";
import { createSwitcherQuerier } from "@/ipc/switcherQuery";
import CommandPalette from "@/overlays/CommandPalette.vue";
import { fuzzyMatch, splitBySpans } from "@/overlays/fuzzy";
import QuickSwitcher from "@/overlays/QuickSwitcher.vue";

const item = (path: string, over: Partial<SwitcherItem> = {}): SwitcherItem => ({
  path,
  title: path.replace(/^.*\//, "").replace(/\.md$/, ""),
  matched: "title",
  candidate: path.replace(/^.*\//, "").replace(/\.md$/, ""),
  highlight: [],
  heading: null,
  link_text: path,
  ...over,
});

const answer = (seq: number, items: SwitcherItem[], over: Partial<SwitcherResults> = {}): SwitcherResults => ({
  seq,
  corpus: "index",
  items,
  discarded: false,
  ...over,
});

interface Pending {
  args: SwitcherQueryArgs;
  resolve(r: SwitcherResults): void;
}

/** A switcher whose answers the test releases by hand, in any order. */
function manualQuery() {
  const pending: Pending[] = [];
  const fn = vi.fn(
    (args: SwitcherQueryArgs) =>
      new Promise<SwitcherResults>((resolve) => {
        pending.push({ args, resolve });
      }),
  );
  return { fn, pending };
}

const mounted: Array<{ unmount(): void }> = [];
function mountSwitcher(query: (a: SwitcherQueryArgs) => Promise<SwitcherResults>) {
  const w = mount(QuickSwitcher, { props: { query }, attachTo: document.body });
  mounted.push(w);
  return w;
}
afterEach(() => {
  for (const w of mounted.splice(0)) w.unmount();
  document.body.innerHTML = "";
});

const rowsOf = (w: ReturnType<typeof mountSwitcher>) => w.findAll('[role="option"]');

describe("createSwitcherQuerier (shared seq discipline)", () => {
  it("increments seq, and only the newest answer survives", async () => {
    const { fn, pending } = manualQuery();
    const q = createSwitcherQuerier(fn);
    const a = q.run("a");
    const b = q.run("ab");
    expect(pending.map((p) => p.args.seq)).toEqual([1, 2]);
    pending[1].resolve(answer(2, [item("x.md")]));
    pending[0].resolve(answer(1, [item("old.md")]));
    expect(await b).not.toBeNull();
    expect(await a).toBeNull();
  });
  it("treats discarded as superseded (null), not as an empty result", async () => {
    const q = createSwitcherQuerier(async ({ seq }) => answer(seq, [], { discarded: true }));
    expect(await q.run("x")).toBeNull();
  });
  it("treats an answer whose seq is not the latest as stale", async () => {
    const q = createSwitcherQuerier(async ({ seq }) => answer(seq - 1, []));
    expect(await q.run("x")).toBeNull();
  });
});

describe("QuickSwitcher (FR-NAV-01)", () => {
  it("empty query on open asks for the recent list (seq 1) and shows it, first row selected", async () => {
    const q = vi.fn(async ({ seq }: SwitcherQueryArgs) => answer(seq, [item("a.md"), item("b/c.md")]));
    const w = mountSwitcher(q);
    await flushPromises();
    expect(q).toHaveBeenCalledWith({ seq: 1, query: "", limit: 20 });
    const rows = rowsOf(w);
    expect(rows).toHaveLength(2);
    expect(rows[0].attributes("aria-selected")).toBe("true");
    expect(w.find("input").attributes("aria-activedescendant")).toBe("qs-opt-0");
    expect(w.text()).not.toMatch(/recent/i); // no "Recent" eyebrow (spec)
  });

  it("every keystroke sends the next seq immediately; the newest answer wins over a late stale one", async () => {
    const { fn, pending } = manualQuery();
    const w = mountSwitcher(fn);
    pending[0].resolve(answer(1, []));
    await flushPromises();
    await w.find("input").setValue("r");
    await w.find("input").setValue("ro");
    expect(pending.slice(1).map((p) => p.args.seq)).toEqual([2, 3]);
    pending[2].resolve(answer(3, [item("Roadmap.md")]));
    await flushPromises();
    pending[1].resolve(answer(2, [item("Stale.md")])); // arrives late
    await flushPromises();
    expect(rowsOf(w).map((r) => r.attributes("aria-label"))).toEqual(["Roadmap"]);
  });

  it("a discarded answer is ignored: it neither clears the list nor says 'no matches'", async () => {
    const { fn, pending } = manualQuery();
    const w = mountSwitcher(fn);
    pending[0].resolve(answer(1, [item("a.md")]));
    await flushPromises();
    await w.find("input").setValue("zz");
    pending[1].resolve(answer(2, [], { discarded: true }));
    await flushPromises();
    expect(rowsOf(w)).toHaveLength(1);
    expect(w.text()).not.toContain("No note matches");
  });

  it("no match offers 'Create note', and Enter on it emits create", async () => {
    const q = vi.fn(async ({ seq, query }: SwitcherQueryArgs) => answer(seq, query === "" ? [item("a.md")] : []));
    const w = mountSwitcher(q);
    await flushPromises();
    await w.find("input").setValue("xyz");
    await flushPromises();
    expect(w.text()).toContain("Create note “xyz”");
    await w.find("input").trigger("keydown", { key: "Enter" });
    expect(w.emitted("create")).toEqual([[{ name: "xyz" }]]);
    expect(w.emitted("open")).toBeUndefined();
  });

  it("arrow keys move the selection and wrap; Enter opens, Ctrl/Cmd+Enter asks for a new tab", async () => {
    const q = vi.fn(async ({ seq }: SwitcherQueryArgs) => answer(seq, [item("a.md"), item("b.md"), item("c.md")]));
    const w = mountSwitcher(q);
    await flushPromises();
    const input = w.find("input");
    await input.trigger("keydown", { key: "ArrowUp" });
    expect(input.attributes("aria-activedescendant")).toBe("qs-opt-2"); // wrapped
    await input.trigger("keydown", { key: "ArrowDown" });
    await input.trigger("keydown", { key: "ArrowDown" });
    expect(input.attributes("aria-activedescendant")).toBe("qs-opt-1");
    await input.trigger("keydown", { key: "Enter" });
    await input.trigger("keydown", { key: "Enter", ctrlKey: true });
    expect(w.emitted("open")).toEqual([
      [{ path: "b.md", newTab: false, heading: null }],
      [{ path: "b.md", newTab: true, heading: null }],
    ]);
  });

  it("heading mode renders 'Note › Heading' and opens at the heading's byte offset", async () => {
    const heading = item("Roadmap 2026.md", {
      title: "Roadmap 2026",
      matched: "heading",
      candidate: "Open risks",
      highlight: [{ start: 0, end: 4 }],
      heading: { text: "Open risks", byte_start: 42 },
    });
    const q = vi.fn(async ({ seq, query }: SwitcherQueryArgs) => answer(seq, query.includes("#") ? [heading] : []));
    const w = mountSwitcher(q);
    await flushPromises();
    await w.find("input").setValue("Roadmap#risk");
    await flushPromises();
    expect(q).toHaveBeenLastCalledWith({ seq: 2, query: "Roadmap#risk", limit: 50 });
    const row = rowsOf(w)[0];
    expect(row.text()).toContain("Roadmap 2026 › Open risks");
    expect(row.attributes("aria-label")).toBe("Roadmap 2026 › Open risks");
    expect(row.find(".hit").text()).toBe("Open");
    await w.find("input").trigger("keydown", { key: "Enter" });
    expect(w.emitted("open")![0][0]).toMatchObject({ path: "Roadmap 2026.md", heading: { text: "Open risks", byteStart: 42 } });
  });

  it("an alias match is labelled and the highlight is a bold span, never markup", async () => {
    const alias = item("Team Roadmap.md", { matched: "alias", candidate: "RM", highlight: [{ start: 0, end: 1 }], title: "Team Roadmap" });
    const w = mountSwitcher(async ({ seq }) => answer(seq, [alias]));
    await flushPromises();
    const row = rowsOf(w)[0];
    expect(row.find(".meta").text()).toBe("Alias: RM");
    expect(row.find(".meta .hit").text()).toBe("R");
    expect(row.attributes("aria-label")).toBe("Team Roadmap, alias RM");
  });

  it("a title match with a folder shows the folder as metadata", async () => {
    const w = mountSwitcher(async ({ seq }) => answer(seq, [item("Projects/Roadmap 2026.md")]));
    await flushPromises();
    expect(rowsOf(w)[0].find(".meta").text()).toBe("Projects");
  });

  it("Esc closes and focus returns to the opener", async () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const w = mountSwitcher(async ({ seq }) => answer(seq, []));
    await flushPromises();
    expect(document.activeElement).toBe(w.find("input").element);
    await w.find("input").trigger("keydown", { key: "Escape" });
    expect(w.emitted("close")).toHaveLength(1);
    w.unmount();
    expect(document.activeElement).toBe(opener);
  });

  it("a transport failure is swallowed (capability degrades, nothing thrown into typing)", async () => {
    const q = vi
      .fn<(a: SwitcherQueryArgs) => Promise<SwitcherResults>>()
      .mockResolvedValueOnce(answer(1, [item("a.md")]))
      .mockRejectedValueOnce(new Error("boom"));
    const w = mountSwitcher(q);
    await flushPromises();
    await w.find("input").setValue("q");
    await flushPromises();
    expect(rowsOf(w)).toHaveLength(1);
  });

  it("exposes labelled buttons for the chorded actions", async () => {
    const w = mountSwitcher(async ({ seq }) => answer(seq, [item("a.md")]));
    await flushPromises();
    const buttons = w.findAll("button").map((b) => b.text());
    expect(buttons).toContain("Open selected note in new tab");
  });
});

describe("render cost (NFR-PRF-06, informational)", () => {
  it("500 keystrokes: result arrival to DOM update", async () => {
    const items = Array.from({ length: 50 }, (_, i) =>
      item(`Folder ${i % 7}/Note number ${i}.md`, { highlight: [{ start: 0, end: 4 }] }),
    );
    let arrived = 0;
    const query = async ({ seq }: SwitcherQueryArgs): Promise<SwitcherResults> => {
      await Promise.resolve();
      arrived = performance.now();
      return answer(seq, items);
    };
    const w = mountSwitcher(query);
    await flushPromises();
    const times: number[] = [];
    const input = w.find("input");
    for (let i = 0; i < 500; i++) {
      // A different list each time so Vue has real work to do.
      items.reverse();
      await input.setValue(`note ${i}`);
      await flushPromises();
      await nextTick();
      times.push(performance.now() - arrived);
    }
    expect(rowsOf(w)).toHaveLength(50);
    times.sort((a, b) => a - b);
    const mean = times.reduce((s, t) => s + t, 0) / times.length;
    const p95 = times[Math.floor(times.length * 0.95)];
    console.info(`switcher render cost, 50 rows, 500 keystrokes (jsdom): mean ${mean.toFixed(2)} ms, p95 ${p95.toFixed(2)} ms`);
    // A loose tripwire against an accidental O(n^2) or sync-heavy regression,
    // not a budget: jsdom is not a browser (no layout, no paint).
    expect(p95).toBeLessThan(50);
  }, 60_000);
});

describe("fuzzy / chord helpers", () => {
  it("matches a subsequence, case-insensitively, with merged spans; rejects non-subsequences", () => {
    const hit = fuzzyMatch("tlp", "Toggle live preview")!;
    expect(hit.spans).toEqual([{ start: 0, end: 1 }, { start: 4, end: 5 }, { start: 12, end: 13 }]);
    expect(fuzzyMatch("zzz", "Toggle live preview")).toBeNull();
    expect(splitBySpans("abcd", [{ start: 1, end: 3 }])).toEqual([
      { text: "a", match: false },
      { text: "bc", match: true },
      { text: "d", match: false },
    ]);
  });
  it("renders chords per platform", () => {
    expect(chordParts("Mod+Shift+M", true)).toEqual(["⌘", "⇧", "M"]);
    expect(chordParts("Mod+Shift+M", false)).toEqual(["Ctrl", "Shift", "M"]);
  });
});

describe("command registry (FR-NAV-05, NFR-USE-01)", () => {
  it("register returns a disposer; list orders by use then name", async () => {
    const r = createCommandRegistry();
    const run = vi.fn();
    const off = r.register({ id: "b", name: "Beta", run });
    r.register({ id: "a", name: "Alpha", run });
    expect(r.list().map((c) => c.id)).toEqual(["a", "b"]);
    await r.run("b");
    expect(r.list().map((c) => c.id)).toEqual(["b", "a"]); // frecency first
    off();
    expect(r.get("b")).toBeUndefined();
    expect(run).toHaveBeenCalledTimes(1);
  });
  it("an unavailable command does not run and returns its reason", async () => {
    const r = createCommandRegistry();
    const run = vi.fn();
    r.register({ id: "x", name: "X", unavailable: () => "Select a file first", run });
    expect(await r.run("x")).toBe("Select a file first");
    expect(run).not.toHaveBeenCalled();
  });
  it("effective binding: override wins, null unbinds, undefined restores; conflicts are reported", () => {
    const r = createCommandRegistry();
    r.register({ id: "a", name: "A", binding: "Mod+K", run: () => {} });
    r.register({ id: "b", name: "B", run: () => {} });
    expect(r.effectiveBinding("a")).toBe("Mod+K");
    expect(r.effectiveBinding("b")).toBeNull();
    r.setOverride("a", null);
    expect(r.effectiveBinding("a")).toBeNull();
    r.setOverride("a", undefined);
    expect(r.effectiveBinding("a")).toBe("Mod+K");
    r.setOverride("b", "mod+k");
    expect(r.conflicts()).toEqual([["a", "b"]]);
  });
});

describe("CommandPalette (FR-NAV-05)", () => {
  function setup() {
    const registry = createCommandRegistry();
    const ran: string[] = [];
    registry.register({ id: "editor.togglePreview", name: "Toggle live preview", binding: "Mod+Shift+M", run: () => void ran.push("preview") });
    registry.register({ id: "search.open", name: "Search vault", binding: "Mod+Shift+F", run: () => void ran.push("search") });
    registry.register({ id: "tree.insert", name: "Insert link", unavailable: () => "Select a file in the tree first", run: () => void ran.push("insert") });
    const w = mount(QuickPaletteHost(registry), { attachTo: document.body });
    mounted.push(w);
    return { w, registry, ran };
  }
  function QuickPaletteHost(registry: ReturnType<typeof createCommandRegistry>) {
    return { components: { CommandPalette }, setup: () => ({ registry }), template: `<CommandPalette :registry="registry" />` };
  }
  const palette = (w: ReturnType<typeof setup>["w"]) => w.findComponent(CommandPalette);

  it("lists every registered command, including unavailable ones, with accessible chord names", () => {
    const { w } = setup();
    const rows = w.findAll('[role="option"]');
    expect(rows).toHaveLength(3);
    expect(w.find("#cp-opt-0").attributes("aria-label")).toContain("Command");
    const preview = rows.find((r) => r.attributes("aria-label")!.startsWith("Toggle live preview"))!;
    expect(preview.findAll("kbd")).toHaveLength(3);
    const insert = rows.find((r) => r.text().startsWith("Insert link"))!;
    expect(insert.classes()).toContain("unavailable");
    expect(insert.find("kbd").exists()).toBe(false); // no binding: no empty chip
  });

  it("fuzzy-filters by name and Enter emits run for the top match", async () => {
    const { w } = setup();
    await w.find("input").setValue("tgl prv");
    expect(w.findAll('[role="option"]')).toHaveLength(1);
    await w.find("input").trigger("keydown", { key: "Enter" });
    expect(palette(w).emitted("run")).toEqual([["editor.togglePreview"]]);
  });

  it("no match says so", async () => {
    const { w } = setup();
    await w.find("input").setValue("qqq");
    expect(w.text()).toContain("No command matches “qqq”");
  });

  it("selecting an unavailable command shows why and stays open", async () => {
    const { w } = setup();
    await w.find("input").setValue("insert");
    await w.find("input").trigger("keydown", { key: "Enter" });
    expect(w.find('[data-testid="palette-reason"]').text()).toBe("Select a file in the tree first");
    expect(palette(w).emitted("run")).toBeUndefined();
  });

  it("a command registered later appears; arrows move; Esc closes and restores focus", async () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const { w, registry } = setup();
    registry.register({ id: "late", name: "Late addition", run: () => {} });
    await nextTick();
    expect(w.findAll('[role="option"]')).toHaveLength(4);
    await w.find("input").trigger("keydown", { key: "ArrowDown" });
    expect(w.find("input").attributes("aria-activedescendant")).toBe("cp-opt-1");
    await w.find("input").trigger("keydown", { key: "Escape" });
    expect(palette(w).emitted("close")).toHaveLength(1);
    w.unmount();
    expect(document.activeElement).toBe(opener);
  });
});
