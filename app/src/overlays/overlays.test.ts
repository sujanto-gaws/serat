// Task #25: quick switcher and command palette (FR-NAV-01, FR-NAV-06,
// NFR-PRF-06, NFR-USE-01, US-04; architecture.md §9.2, §9.3).

import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import { chordGlyphs, chordSpoken, CommandRegistry } from "@/editor/commands";
import { headingDocOffset } from "@/overlays/headingTarget";
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

  it("no match says so in a polite status region and offers no dead-end Create row (until #43)", async () => {
    const q = vi.fn(async ({ seq, query }: SwitcherQueryArgs) => answer(seq, query === "" ? [item("a.md")] : []));
    const w = mountSwitcher(q);
    await flushPromises();
    await w.find("input").setValue("xyz");
    await flushPromises();
    const status = w.find('[role="status"]');
    expect(status.text()).toBe("No note matches “xyz”");
    expect(status.attributes("aria-live")).toBe("polite");
    expect(w.text()).not.toContain("Create note");
    expect(rowsOf(w)).toHaveLength(0);
  });

  it("does not query mid-composition; sends the composed text on compositionend", async () => {
    const q = vi.fn(async ({ seq }: SwitcherQueryArgs) => answer(seq, []));
    const w = mountSwitcher(q);
    await flushPromises();
    q.mockClear();
    const input = w.find("input");
    await input.trigger("compositionstart");
    (input.element as HTMLInputElement).value = "にほ";
    await input.trigger("input");
    expect(q).not.toHaveBeenCalled();
    await input.trigger("compositionend");
    expect(q).toHaveBeenCalledWith({ seq: 2, query: "にほ", limit: 50 });
  });

  it("focus leaving to an outside element closes WITHOUT stealing focus back", async () => {
    const opener = document.createElement("button");
    const other = document.createElement("button");
    document.body.append(opener, other);
    opener.focus();
    const w = mountSwitcher(async ({ seq }) => answer(seq, []));
    await flushPromises();
    other.focus();
    expect(w.emitted("close")).toHaveLength(1);
    w.unmount();
    expect(document.activeElement).toBe(other);
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
      return answer(seq, [...items]); // a fresh array per answer, as IPC delivers
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
    // A loose O(n^2) guard on the median only, not a budget: jsdom is not a browser
    // (no layout, no paint) and CI hosts are loaded. NFR-PRF-06's keystroke-to-paint is a
    // browser bench (follow-up, with bench/editor-latency).
    expect(times[Math.floor(times.length / 2)]).toBeLessThan(150);
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
  it("renders chords per platform, in CodeMirror notation", () => {
    expect(chordGlyphs("Mod-Shift-m", true)).toEqual(["⌘", "⇧", "M"]);
    expect(chordGlyphs("Mod-Shift-m", false)).toEqual(["Ctrl", "Shift", "M"]);
    expect(chordSpoken("Mod-Shift-m", true)).toEqual(["Command", "Shift", "M"]);
    expect(chordSpoken("Mod-Shift-m", false)).toEqual(["Control", "Shift", "M"]);
  });
});

describe("heading target (CRLF-safe)", () => {
  const lf = "# Top\n\n## Open risks\nbody\n";
  const crlf = lf.replace(/\n/g, "\r\n");
  const byteOf = (raw: string, needle: string) => new TextEncoder().encode(raw.slice(0, raw.indexOf(needle))).length;
  it("LF note: exact offset", () => {
    expect(headingDocOffset(lf, byteOf(lf, "## Open"), "Open risks")).toBe(lf.indexOf("## Open"));
  });
  it("CRLF note: byte_start counts \\r, the editor document does not", () => {
    const doc = lf; // what CodeMirror holds
    const offset = headingDocOffset(crlf, byteOf(crlf, "## Open"), "Open risks");
    expect(offset).toBe(doc.indexOf("## Open"));
  });
  it("a stale byte_start falls back to searching for the heading, then to the note start", () => {
    expect(headingDocOffset(lf, 2, "Open risks")).toBe(lf.indexOf("## Open"));
    expect(headingDocOffset(lf, 2, "Gone")).toBe(0);
  });
});

describe("CommandPalette (FR-NAV-06)", () => {
  function setup() {
    const registry = new CommandRegistry();
    const ran: string[] = [];
    registry.register({ id: "editor.toggle-live-preview", name: "Toggle live preview", defaultBinding: "Mod-Shift-m", exec: () => void ran.push("preview") });
    registry.register({ id: "search.open", name: "Search vault", defaultBinding: "Mod-Shift-f", exec: () => void ran.push("search") });
    registry.register({ id: "tree.insert", name: "Insert link", unavailable: () => "Select a file in the tree first", exec: () => void ran.push("insert") });
    const w = mount(CommandPalette, { props: { registry, mac: true }, attachTo: document.body });
    mounted.push(w);
    return { w, registry, ran };
  }
  const palette = (w: ReturnType<typeof setup>["w"]) => w;

  it("lists every registered command, including unavailable ones, with accessible chord names", () => {
    const { w } = setup();
    const rows = w.findAll('[role="option"]');
    expect(rows).toHaveLength(3);
    const preview = rows.find((r) => r.attributes("aria-label")!.startsWith("Toggle live preview"))!;
    expect(preview.attributes("aria-label")).toBe("Toggle live preview, Command, Shift, M");
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
    expect(palette(w).emitted("run")).toEqual([["editor.toggle-live-preview"]]);
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
    registry.register({ id: "late", name: "Late addition", exec: () => {} });
    await nextTick();
    expect(w.findAll('[role="option"]')).toHaveLength(4);
    await w.find("input").trigger("keydown", { key: "ArrowDown" });
    expect(w.find("input").attributes("aria-activedescendant")).toBe("cp-opt-1");
    await w.find("input").trigger("keydown", { key: "Escape" });
    expect(palette(w).emitted("close")).toHaveLength(1);
    w.unmount();
    expect(document.activeElement).toBe(opener);
  });

  it("a rebinding shows in the chip; a shrinking registry keeps the selection valid", async () => {
    const { w, registry } = setup();
    registry.setOverride("search.open", "Mod-p");
    await nextTick();
    const row = w.findAll('[role="option"]').find((r) => r.attributes("aria-label")!.startsWith("Search vault"))!;
    expect(row.attributes("aria-label")).toBe("Search vault, Command, P");
    for (let i = 0; i < 2; i++) await w.find("input").trigger("keydown", { key: "ArrowDown" });
    registry.unregister("tree.insert");
    registry.unregister("search.open");
    await nextTick();
    expect(w.find("input").attributes("aria-activedescendant")).toBe("cp-opt-0");
  });
});
