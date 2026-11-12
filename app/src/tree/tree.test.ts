// Task #24, file tree: model (ADR-0015 §1/§4), virtual window, keyboard map,
// link text, and the component (progressive population, marks, virtualisation).

import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import FileTree from "@/tree/FileTree.vue";
import { insertLinkText, isNameUnique, navigate, visibleWindow } from "@/tree/treeLogic";
import { TreeModel } from "@/tree/treeModel";
import type { PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { TreeEntry } from "@/ipc/generated/TreeEntry";
import type { TreeMessage } from "@/ipc/generated/TreeMessage";

const note = (name: string, sidecar = false): TreeEntry => ({ name, kind: "note", sidecar });
const asset = (name: string, sidecar = false): TreeEntry => ({ name, kind: "asset", sidecar });
const dir = (name: string): TreeEntry => ({ name, kind: "dir", sidecar: false });
const listing = (d: string, entries: TreeEntry[]): TreeMessage => ({ kind: "listing", dir: d, entries });

describe("TreeModel", () => {
  it("renders listed directories and keeps un-listed ones pending, distinct from confirmed-empty", () => {
    const m = new TreeModel();
    m.apply(listing("", [dir("A"), dir("B"), dir("C"), note("n.md")]));
    m.apply(listing("B", []));
    m.apply(listing("C", [note("x.md")]));
    const byName = Object.fromEntries(m.rows().map((r) => [r.name, r.state]));
    expect(byName).toMatchObject({ A: "pending", B: "empty", C: "collapsed", "n.md": "leaf" });
    m.expand("C");
    expect(m.rows().map((r) => r.path)).toEqual(["A", "B", "C", "C/x.md", "n.md"]);
    m.expand("B"); // nothing to expand into: not offered
    expect(m.isExpanded("B")).toBe(false);
  });

  it("a folder resolves independently of its siblings the moment its own listing lands", () => {
    const m = new TreeModel();
    m.apply(listing("", [dir("A"), dir("B")]));
    m.apply(listing("B", [note("b.md")]));
    expect(m.rows().find((r) => r.name === "A")?.state).toBe("pending");
    expect(m.rows().find((r) => r.name === "B")?.state).toBe("collapsed");
  });

  it("counts file entries (what TreeComplete.entries counts), through listings, upserts and removes", () => {
    const m = new TreeModel();
    m.apply(listing("", [dir("D"), note("a.md"), asset("p.pdf", true)]));
    m.apply(listing("D", [note("b.md"), note("c.md")]));
    expect(m.fileCount).toBe(4);
    m.apply({ kind: "upsert", dir: "D", entry: note("d.md"), after: "c.md" });
    expect(m.fileCount).toBe(5);
    m.apply({ kind: "upsert", dir: "D", entry: note("d.md"), after: "c.md" }); // replaces, not doubles
    expect(m.fileCount).toBe(5);
    m.apply({ kind: "remove", dir: "", name: "D" }); // drops the whole subtree
    expect(m.fileCount).toBe(2);
    expect(m.isListed("D")).toBe(false);
  });

  it("a replacing listing drops the subtrees of directories that vanished", () => {
    const m = new TreeModel();
    m.apply(listing("", [dir("X"), dir("Y")]));
    m.apply(listing("X", [dir("deep")]));
    m.apply(listing("X/deep", [note("n.md")]));
    m.apply(listing("", [dir("Y")]));
    expect(m.isListed("X")).toBe(false);
    expect(m.isListed("X/deep")).toBe(false);
    expect(m.fileCount).toBe(0);
  });

  it("folds a sidecar into its asset's row (the entry carries the flag; there is no second row)", () => {
    const m = new TreeModel();
    m.apply(listing("", [asset("contract.pdf", true), note("plain.md")]));
    expect(m.rows().map((r) => [r.name, r.sidecar])).toEqual([
      ["contract.pdf", true],
      ["plain.md", false],
    ]);
  });

  it("applies a burst of 1,000 upserts in order, none dropped (ADR-0015 lossless rule)", () => {
    const m = new TreeModel();
    m.apply(listing("", []));
    // Each upsert follows the previous one: only strict arrival order yields this sequence.
    for (let i = 0; i < 1000; i += 1) {
      m.apply({
        kind: "upsert",
        dir: "",
        entry: note(`n${String(i).padStart(4, "0")}.md`),
        after: i === 0 ? null : `n${String(i - 1).padStart(4, "0")}.md`,
      });
    }
    const names = (m.listing("") ?? []).map((e) => e.name);
    expect(names).toHaveLength(1000);
    expect(names).toEqual(Array.from({ length: 1000 }, (_, i) => `n${String(i).padStart(4, "0")}.md`));
    expect(m.applied).toBe(1001);
    expect(m.misplacedUpserts).toBe(0);
  });

  it("replaying a random create/delete sequence equals a fresh listing (ADR-0015, task #24)", () => {
    // Reference: Rust's order, directories first then case-folded name.
    const order = (a: TreeEntry, b: TreeEntry): number =>
      Number(b.kind === "dir") - Number(a.kind === "dir") ||
      (a.name.toLowerCase() < b.name.toLowerCase() ? -1 : a.name.toLowerCase() > b.name.toLowerCase() ? 1 : 0);
    let seed = 12345;
    const rand = (n: number): number => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed % n;
    };
    const reference: TreeEntry[] = [];
    const m = new TreeModel();
    m.apply(listing("", []));
    for (let step = 0; step < 600; step += 1) {
      if (reference.length > 0 && rand(3) === 0) {
        const victim = reference.splice(rand(reference.length), 1)[0];
        m.apply({ kind: "remove", dir: "", name: victim.name });
      } else {
        const kind = rand(4) === 0 ? "dir" : rand(2) === 0 ? "note" : "asset";
        const name = `${"abcdefghij"[rand(10)]}${rand(50)}${kind === "note" ? ".md" : kind === "asset" ? ".png" : ""}`;
        const entry: TreeEntry = { name, kind, sidecar: false };
        const existing = reference.findIndex((e) => e.name === name);
        if (existing >= 0) reference.splice(existing, 1);
        reference.push(entry);
        reference.sort(order);
        const at = reference.findIndex((e) => e.name === name);
        m.apply({ kind: "upsert", dir: "", entry, after: at === 0 ? null : reference[at - 1].name });
      }
    }
    expect(m.listing("")).toEqual(reference);
    expect(m.misplacedUpserts).toBe(0);
    const fresh = new TreeModel();
    fresh.apply(listing("", reference));
    expect(fresh.fileCount).toBe(m.fileCount);
  });

  it("complete and walk_settled are recorded, not treated as entries", () => {
    const m = new TreeModel();
    m.apply({ kind: "complete", entries: 7, source: "index" });
    m.apply({ kind: "walk_settled", empty_folders: 3 });
    expect(m.complete).toEqual({ entries: 7, source: "index" });
    expect(m.emptyFolders).toBe(3);
    expect(m.fileCount).toBe(0);
  });
});

describe("visibleWindow", () => {
  it("renders viewport rows plus overscan, whatever the total", () => {
    const w = visibleWindow(2600, 600, 26, 20_000);
    expect(w.end - w.start).toBeLessThanOrEqual(40);
    expect(w.start).toBe(100 - 8);
    expect(w.totalHeight).toBe(20_000 * 26);
    expect(w.offsetTop).toBe(w.start * 26);
  });
  it("clamps at both ends and for an empty list", () => {
    expect(visibleWindow(0, 600, 26, 5)).toMatchObject({ start: 0, end: 5 });
    expect(visibleWindow(99999, 600, 26, 5)).toMatchObject({ end: 5 });
    expect(visibleWindow(0, 600, 26, 0)).toMatchObject({ start: 0, end: 0 });
  });
});

describe("keyboard navigation", () => {
  const m = new TreeModel();
  m.apply(listing("", [dir("Open"), dir("Closed"), dir("Pending"), dir("Empty"), note("n.md")]));
  m.apply(listing("Open", [note("a.md")]));
  m.apply(listing("Closed", [note("b.md")]));
  m.apply(listing("Empty", []));
  m.expand("Open");
  const rows = m.rows();
  const key = (k: string, extra: Partial<{ ctrlOrMeta: boolean; shift: boolean }> = {}) => ({
    key: k,
    ctrlOrMeta: false,
    shift: false,
    ...extra,
  });

  it("moves with the arrows and clamps at the ends", () => {
    expect(navigate(rows, "Open", key("ArrowDown"))).toEqual({ kind: "focus", path: "Open/a.md" });
    expect(navigate(rows, "n.md", key("ArrowDown"))).toEqual({ kind: "focus", path: "n.md" });
    expect(navigate(rows, "Open", key("ArrowUp"))).toEqual({ kind: "focus", path: "Open" });
    expect(navigate(rows, null, key("ArrowDown"))).toEqual({ kind: "focus", path: "Open" });
    expect(navigate(rows, "Open", key("End"))).toEqual({ kind: "focus", path: "n.md" });
  });

  it("expands/collapses with the arrows; pending and empty folders offer nothing", () => {
    expect(navigate(rows, "Closed", key("ArrowRight"))).toEqual({ kind: "expand", path: "Closed" });
    expect(navigate(rows, "Open", key("ArrowRight"))).toEqual({ kind: "focus", path: "Open/a.md" });
    expect(navigate(rows, "Open", key("ArrowLeft"))).toEqual({ kind: "collapse", path: "Open" });
    expect(navigate(rows, "Open/a.md", key("ArrowLeft"))).toEqual({ kind: "focus", path: "Open" });
    expect(navigate(rows, "Pending", key("ArrowRight"))).toEqual({ kind: "none" });
    expect(navigate(rows, "Empty", key("Enter"))).toEqual({ kind: "none" });
  });

  it("→ on a note opens it without moving focus; Enter opens; Mod-Enter opens in a new tab", () => {
    expect(navigate(rows, "n.md", key("ArrowRight"))).toEqual({ kind: "open", path: "n.md", newTab: false, keepFocus: true });
    expect(navigate(rows, "n.md", key("Enter"))).toEqual({ kind: "open", path: "n.md", newTab: false, keepFocus: false });
    expect(navigate(rows, "n.md", key("Enter", { ctrlOrMeta: true }))).toMatchObject({ newTab: true });
  });

  it("F2 renames, Shift-F10 / Menu opens the context menu, Mod-Shift-L inserts a link (never for a folder)", () => {
    expect(navigate(rows, "n.md", key("F2"))).toEqual({ kind: "rename", path: "n.md" });
    expect(navigate(rows, "n.md", key("F10", { shift: true }))).toEqual({ kind: "context-menu", path: "n.md" });
    expect(navigate(rows, "n.md", key("F10"))).toEqual({ kind: "none" });
    expect(navigate(rows, "n.md", key("ContextMenu"))).toEqual({ kind: "context-menu", path: "n.md" });
    expect(navigate(rows, "n.md", key("L", { ctrlOrMeta: true, shift: true }))).toEqual({ kind: "none" }); // tree.insertLink is a registry command, not a navigate key
  });
});

describe("insert link text", () => {
  it("embeds images and PDFs, links other assets, links notes by stem", () => {
    expect(insertLinkText({ path: "a/pic.PNG", name: "pic.PNG", kind: "asset" }, true)).toBe("![[pic.PNG]]");
    expect(insertLinkText({ path: "doc.pdf", name: "doc.pdf", kind: "asset" }, true)).toBe("![[doc.pdf]]");
    expect(insertLinkText({ path: "data.csv", name: "data.csv", kind: "asset" }, true)).toBe("[[data.csv]]");
    expect(insertLinkText({ path: "notes/Plan.md", name: "Plan.md", kind: "note" }, true)).toBe("[[Plan]]");
  });
  it("uses the vault path when the bare name is ambiguous", () => {
    expect(insertLinkText({ path: "b/Plan.md", name: "Plan.md", kind: "note" }, false)).toBe("[[b/Plan]]");
    expect(insertLinkText({ path: "b/pic.png", name: "pic.png", kind: "asset" }, false)).toBe("![[b/pic.png]]");
  });
  it("detects name ambiguity across listed directories", () => {
    const m = new TreeModel();
    m.apply(listing("", [dir("a"), dir("b"), note("solo.md")]));
    m.apply(listing("a", [note("Plan.md")]));
    m.apply(listing("b", [note("Plan.md")]));
    expect(isNameUnique(m, "solo.md")).toBe(true);
    expect(isNameUnique(m, "Plan.md")).toBe(false);
  });
});

// ---------------------------------------------------------------------------

function fakeIpc() {
  let push: (m: TreeMessage) => void = () => undefined;
  const ipc: PaneIpcAdapter = {
    openTree: vi.fn(async (onMessage) => {
      push = onMessage;
    }),
    markTree: vi.fn().mockResolvedValue(undefined),
    search: vi.fn(),
    backlinks: vi.fn(),
    onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
  };
  return { ipc, push: (m: TreeMessage) => push(m) };
}

describe("FileTree component", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0) as unknown as number);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("20,000 entries produce a bounded number of DOM rows (virtualised)", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", Array.from({ length: 20_000 }, (_, i) => note(`note-${String(i).padStart(5, "0")}.md`))));
    await wrapper.vm.$nextTick();
    const rendered = wrapper.findAll('[role="treeitem"]').length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(80);
    const spacer = wrapper.find(".serat-tree-spacer").attributes("style");
    expect(spacer).toContain(`height: ${20_000 * 26}px`);
    wrapper.unmount();
  });

  it("progressive population: root rows are interactive at once; pending folders resolve one by one", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [dir("Projects"), note("a.md")]));
    await wrapper.vm.$nextTick();
    const rowsBefore = wrapper.findAll('[role="treeitem"]');
    expect(rowsBefore[0].attributes("aria-label")).toBe("Projects, folder, contents loading");
    expect(rowsBefore[0].classes()).toContain("serat-tree-state-pending");
    await rowsBefore[1].trigger("click");
    expect(wrapper.emitted("open")?.[0][0]).toEqual({ path: "a.md", newTab: false, keepFocus: false });

    push(listing("Projects", [note("p.md")]));
    await wrapper.vm.$nextTick();
    const projects = wrapper.findAll('[role="treeitem"]')[0];
    expect(projects.classes()).toContain("serat-tree-state-collapsed");
    expect(projects.attributes("aria-expanded")).toBe("false");
    await projects.trigger("click");
    expect(wrapper.findAll('[role="treeitem"]').map((r) => r.attributes("data-path"))).toEqual(["Projects", "Projects/p.md", "a.md"]);
    wrapper.unmount();
  });

  it("mark_tree: first_screen once the root has rendered and painted, complete after Complete is applied, in that order", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    expect(ipc.markTree).not.toHaveBeenCalled();

    push(listing("", [dir("D"), note("a.md"), note("b.md")]));
    await wrapper.vm.$nextTick();
    expect(ipc.markTree).not.toHaveBeenCalled(); // not before the double rAF
    await vi.advanceTimersByTimeAsync(5);
    expect(ipc.markTree).toHaveBeenCalledTimes(1);
    expect(ipc.markTree).toHaveBeenLastCalledWith({ mark: "first_screen", entries_in_model: 2 });

    push(listing("D", [note("c.md")]));
    push({ kind: "complete", entries: 3, source: "index" });
    await wrapper.vm.$nextTick();
    await vi.advanceTimersByTimeAsync(5);
    expect(ipc.markTree).toHaveBeenCalledTimes(2);
    expect(ipc.markTree).toHaveBeenLastCalledWith({ mark: "complete", entries_in_model: 3 });

    // Later live updates never re-mark.
    push({ kind: "upsert", dir: "", entry: note("z.md"), after: "b.md" });
    await vi.advanceTimersByTimeAsync(20);
    expect(ipc.markTree).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });

  it("an empty root, once listed, shows the invitation (never while pending)", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.text()).not.toContain("Nothing here yet");
    push(listing("", []));
    await wrapper.vm.$nextTick();
    expect(wrapper.text()).toContain("Nothing here yet");
    wrapper.unmount();
  });

  it("keyboard: arrows move focus (aria-selected), → expands, F2/Menu emit", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [dir("D"), asset("pic.png", true), note("a.md")]));
    push(listing("D", [note("x.md")]));
    await wrapper.vm.$nextTick();
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" });
    await tree.trigger("keydown", { key: "ArrowRight" });
    expect(wrapper.findAll('[role="treeitem"]').map((r) => r.attributes("data-path"))).toEqual(["D", "D/x.md", "pic.png", "a.md"]);
    await tree.trigger("keydown", { key: "ArrowDown" });
    await tree.trigger("keydown", { key: "ArrowDown" });
    expect(wrapper.get('[aria-selected="true"]').attributes("data-path")).toBe("pic.png");
    await tree.trigger("keydown", { key: "F2" });
    expect(wrapper.emitted("rename")?.[0][0]).toEqual({ path: "pic.png" });
    wrapper.unmount();
  });

  it("announces the sidecar fold and shows the brass dot on the asset row only", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [asset("contract.pdf", true), asset("diagram.png"), note("n.md")]));
    await wrapper.vm.$nextTick();
    const items = wrapper.findAll('[role="treeitem"]');
    expect(items[0].attributes("aria-label")).toBe("contract, PDF, has metadata note");
    expect(items[0].find(".serat-tree-dot").exists()).toBe(true);
    expect(items[1].find(".serat-tree-dot").exists()).toBe(false);
    wrapper.unmount();
  });

  it("an unsaved open note shows the dot", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc, openPath: "n.md", openDirty: true }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [note("n.md")]));
    await wrapper.vm.$nextTick();
    expect(wrapper.find(".serat-tree-dot").exists()).toBe(true);
    wrapper.unmount();
  });

  it("applies every message of a 1,000-upsert burst in order through the component", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", []));
    for (let i = 0; i < 1000; i += 1) {
      push({
        kind: "upsert",
        dir: "",
        entry: note(`n${String(i).padStart(4, "0")}.md`),
        after: i === 0 ? null : `n${String(i - 1).padStart(4, "0")}.md`,
      });
    }
    await wrapper.vm.$nextTick();
    const model = (wrapper.vm as unknown as { model: TreeModel }).model;
    expect(model.listing("")?.map((e) => e.name)).toEqual(Array.from({ length: 1000 }, (_, i) => `n${String(i).padStart(4, "0")}.md`));
    wrapper.unmount();
  });

  it("tree://status stopped fills a persistent live region outside the tree (ADR-0015)", async () => {
    const { ipc } = fakeIpc();
    let fire: (s: { vault_id: string; stopped: boolean }) => void = () => undefined;
    ipc.onTreeStatus = vi.fn(async (h) => {
      fire = h;
      return () => undefined;
    });
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    const live = wrapper.get('[data-testid="tree-status"]');
    expect(live.attributes("role")).toBe("status");
    expect(live.text()).toBe(""); // present before it has anything to say
    expect(wrapper.get('[role="tree"]').element.contains(live.element)).toBe(false);
    fire({ vault_id: "v", stopped: false });
    await wrapper.vm.$nextTick();
    expect(live.text()).toBe("");
    fire({ vault_id: "v", stopped: true });
    await wrapper.vm.$nextTick();
    expect(live.text()).toBe("file tree stopped updating; reopen the vault");
    wrapper.unmount();
  });

  it("tree://status for another vault is discarded once the vault id is known; any is accepted before", async () => {
    const { ipc } = fakeIpc();
    let fire: (s: { vault_id: string; stopped: boolean }) => void = () => undefined;
    ipc.onTreeStatus = vi.fn(async (h) => {
      fire = h;
      return () => undefined;
    });
    const known = mount(FileTree, { props: { ipc, vaultId: "mine" }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    fire({ vault_id: "other", stopped: true });
    await known.vm.$nextTick();
    expect(known.get('[data-testid="tree-status"]').text()).toBe("");
    fire({ vault_id: "mine", stopped: true });
    await known.vm.$nextTick();
    expect(known.get('[data-testid="tree-status"]').text()).toContain("stopped updating");
    known.unmount();

    const unknown = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    fire({ vault_id: "anything", stopped: true });
    await unknown.vm.$nextTick();
    expect(unknown.get('[data-testid="tree-status"]').text()).toContain("stopped updating");
    unknown.unmount();
  });

  it("no vault open (after a failed open): tree://status is dropped even for the announced id; a new open is accepted again (#103)", async () => {
    const { ipc } = fakeIpc();
    let fire: (s: { vault_id: string; stopped: boolean }) => void = () => undefined;
    ipc.onTreeStatus = vi.fn(async (h) => {
      fire = h;
      return () => undefined;
    });
    const w = mount(FileTree, { props: { ipc, vaultId: "B", vaultOpen: false }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    fire({ vault_id: "B", stopped: true });
    await w.vm.$nextTick();
    expect(w.get('[data-testid="tree-status"]').text()).toBe("");
    await w.setProps({ vaultId: "C", vaultOpen: true });
    fire({ vault_id: "C", stopped: true });
    await w.vm.$nextTick();
    expect(w.get('[data-testid="tree-status"]').text()).toContain("stopped updating");
    w.unmount();
  });

  it("open_tree is on the wire before the status listener is registered; a late listener resolving after unmount is unlistened", async () => {
    const { ipc } = fakeIpc();
    const order: string[] = [];
    ipc.openTree = vi.fn(async () => void order.push("open_tree"));
    const off = vi.fn();
    let resolve!: (f: () => void) => void;
    ipc.onTreeStatus = vi.fn(() => {
      order.push("listen");
      return new Promise<() => void>((r) => (resolve = r));
    });
    const wrapper = mount(FileTree, { props: { ipc } });
    expect(order).toEqual(["open_tree", "listen"]);
    wrapper.unmount();
    resolve(off);
    await vi.advanceTimersByTimeAsync(0);
    expect(off).toHaveBeenCalledTimes(1);
  });

  it("20,000 notes in one expanded listing: bounded DOM rows and the apply+render time (jsdom, real clock)", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    vi.useRealTimers();
    vi.stubGlobal("requestAnimationFrame", () => 0);
    const t0 = performance.now();
    push(listing("", [{ name: "big", kind: "dir", sidecar: false }, ...Array.from({ length: 9_999 }, (_, i) => note(`r${i}.md`))]));
    push(listing("big", Array.from({ length: 10_001 }, (_, j) => note(`n${j}.md`))));
    (wrapper.vm as unknown as { model: { expand(p: string): void } }).model.expand("big");
    push({ kind: "complete", entries: 20_000, source: "walk" } as TreeMessage);
    await wrapper.vm.$nextTick();
    const ms = performance.now() - t0;
    console.log(`20k-note tree (all expanded) applied and rendered in ${ms.toFixed(1)} ms`);
    expect((wrapper.vm as unknown as { model: { rows(): unknown[] } }).model.rows().length).toBe(20_001);
    expect(wrapper.findAll('[role="treeitem"]').length).toBeLessThan(80);
    expect(ms).toBeLessThan(2000);
    wrapper.unmount();
  });

  it("filter: hides non-matches, keeps ancestors expanded, bolds the match; Esc clears and returns focus to the tree", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [{ name: "Projects", kind: "dir", sidecar: false }, note("Alpha.md")]));
    push(listing("Projects", [note("Roadmap.md"), note("Budget.md")]));
    await wrapper.vm.$nextTick();
    const input = wrapper.get('[data-testid="tree-filter"]');
    await input.setValue("road");
    const paths = wrapper.findAll('[role="treeitem"]').map((r) => r.attributes("data-path"));
    expect(paths).toEqual(["Projects", "Projects/Roadmap.md"]);
    expect(wrapper.get(".serat-tree-match").text()).toBe("Road");
    await input.setValue("zzz");
    expect(wrapper.get('[data-testid="tree-no-matches"]').text()).toContain("zzz");
    await input.trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect((input.element as HTMLInputElement).value).toBe("");
    expect(document.activeElement).toBe(wrapper.get('[role="tree"]').element);
    wrapper.unmount();
  });

  it("Mod-F in the tree pane focuses the filter; Mod-Shift-F is left alone for global search", async () => {
    const { ipc } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "F", ctrlKey: true, shiftKey: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).not.toBe(wrapper.get('[data-testid="tree-filter"]').element);
    await tree.trigger("keydown", { key: "f", ctrlKey: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).toBe(wrapper.get('[data-testid="tree-filter"]').element);
    wrapper.unmount();
  });

  it("New note / New folder are rendered disabled with a reason, not hidden, when the host gives no create command (file-tree.md)", async () => {
    const { ipc } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc } });
    for (const id of ["tree-new-note", "tree-new-folder"]) {
      const b = wrapper.get(`[data-testid="${id}"]`);
      expect(b.attributes("disabled")).toBeDefined();
      expect(b.attributes("title")).toContain("not available");
    }
    wrapper.unmount();
  });

  it("an upsert emits added with the full path", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc } });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [{ name: "d", kind: "dir", sidecar: false }]));
    push(listing("d", []));
    push({ kind: "upsert", dir: "d", entry: note("a.md"), after: null } as TreeMessage);
    expect(wrapper.emitted("added")?.[0]).toEqual(["d/a.md"]);
    wrapper.unmount();
  });

  it("format glyphs, ids and aria-activedescendant; a clicked pending folder emits pending-listing, cleared when it lists", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    push(
      listing("", [
        { name: "Later", kind: "dir", sidecar: false },
        { name: "doc.pdf", kind: "asset", sidecar: false },
        { name: "pic.png", kind: "asset", sidecar: false },
        { name: "x.bin", kind: "asset", sidecar: false },
        note("n.md"),
      ]),
    );
    await wrapper.vm.$nextTick();
    const glyphs = wrapper.findAll(".serat-tree-glyph").map((g) => g.classes().find((c) => c.startsWith("serat-glyph-")));
    expect(glyphs).toEqual(["serat-glyph-folder", "serat-glyph-pdf", "serat-glyph-image", "serat-glyph-file", "serat-glyph-note"]);
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" });
    const id = tree.attributes("aria-activedescendant");
    expect(document.getElementById(id as string)?.getAttribute("data-path")).toBe("Later");
    await wrapper.get('[data-path="Later"]').trigger("click");
    expect(wrapper.emitted("pending-listing")?.at(-1)).toEqual(["Later"]);
    push(listing("Later", [note("a.md")]));
    await wrapper.vm.$nextTick();
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.emitted("pending-listing")?.at(-1)).toEqual([null]);
    wrapper.unmount();
  });

  it("a remove message emits removed with the full path", async () => {
    const { ipc, push } = fakeIpc();
    const wrapper = mount(FileTree, { props: { ipc } });
    await vi.advanceTimersByTimeAsync(0);
    push(listing("", [{ name: "d", kind: "dir", sidecar: false }]));
    push(listing("d", [note("a.md")]));
    push({ kind: "remove", dir: "d", name: "a.md" } as TreeMessage);
    expect(wrapper.emitted("removed")?.[0]).toEqual(["d/a.md"]);
    wrapper.unmount();
  });
});
