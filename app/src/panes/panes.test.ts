// Task #24: backlinks pane and search panel (FR-NAV-02, FR-NAV-04, US-03,
// US-05; architecture.md §4.5), plus their shared text/locate/query helpers.

import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import BacklinksPane from "@/panes/BacklinksPane.vue";
import SearchPanel from "@/panes/SearchPanel.vue";
import { byteToUtf16Index, locateBlock, rawIndexToDocOffset } from "@/panes/locate";
import { checkQuery } from "@/panes/searchQuery";
import { plainSnippet, splitSnippet } from "@/panes/text";
import type { PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { Backlink } from "@/ipc/generated/Backlink";
import type { BlockLocation } from "@/ipc/generated/BlockLocation";
import type { SearchHit } from "@/ipc/generated/SearchHit";

const block = (id: number, ord = 0, start = 0, hash = "h"): BlockLocation => ({
  block_id: id,
  ord,
  heading_path: null,
  byte_start: start,
  byte_end: start + 10,
  note_hash: hash,
});

describe("splitSnippet", () => {
  it("splits on U+0002/U+0003 into text segments; markup stays text", () => {
    const segs = splitSnippet("a \u0002<b>hit</b>\u0003 z");
    expect(segs).toEqual([
      { text: "a ", match: false },
      { text: "<b>hit</b>", match: true },
      { text: " z", match: false },
    ]);
    expect(plainSnippet("a \u0002x\u0003 z")).toBe("a x z");
  });
  it("tolerates stray delimiters from note text", () => {
    expect(splitSnippet("one\u0003two")).toEqual([
      { text: "one", match: false },
      { text: "two", match: false },
    ]);
    expect(splitSnippet("")).toEqual([]);
  });
});

describe("checkQuery", () => {
  it("accepts text, quoted phrases and one filter", () => {
    expect(checkQuery('budget "q3 review" [type:project]')).toEqual({ kind: "ok", text: 'budget "q3 review"', filter: "[type:project]" });
    expect(checkQuery("   ")).toEqual({ kind: "empty" });
  });
  it("refuses an unbalanced quote before sending", () => {
    expect(checkQuery('"budget')).toEqual({ kind: "unbalanced_quote" });
  });
  it("refuses a second complete filter, but not an opening bracket or a bracket inside quotes", () => {
    expect(checkQuery("a [type:x] [owner:y]")).toEqual({ kind: "second_filter", first: "[type:x]", second: "[owner:y]" });
    expect(checkQuery("a [type:x] [owner:")).toMatchObject({ kind: "ok" });
    expect(checkQuery('a [type:x] "[owner:y]"')).toMatchObject({ kind: "ok", filter: "[type:x]" });
  });
});

describe("locateBlock", () => {
  it("maps UTF-8 byte offsets to UTF-16 indices", () => {
    expect(byteToUtf16Index("aéb", 3)).toBe(2); // a (1) + e-acute (2) = 3 bytes
    expect(byteToUtf16Index("\u{1F600}x", 4)).toBe(2);
  });
  it("exact when the hash is unchanged, in the editor's CRLF-as-one model", () => {
    const text = "a\r\nb\r\nc";
    expect(rawIndexToDocOffset(text, 6)).toBe(4);
    const pos = locateBlock(text, "h", block(1, 0, 6));
    expect(pos).toEqual({ offset: 4, approximate: false });
  });
  it("approximate, by ord, when the note changed since it was indexed", () => {
    const text = "first\n\nsecond\n\nthird";
    expect(locateBlock(text, "other", block(1, 1, 999))).toEqual({ offset: 7, approximate: true });
    expect(locateBlock(text, null, block(1, 99, 0))).toMatchObject({ approximate: true });
  });
});

// ---------------------------------------------------------------------------

function ipcWith(overrides: Partial<PaneIpcAdapter> = {}) {
  let changed: () => void = () => undefined;
  const ipc: PaneIpcAdapter = {
    openTree: vi.fn(),
    markTree: vi.fn(),
    search: vi.fn(),
    backlinks: vi.fn(),
    onIndexChanged: vi.fn(async (handler: () => void) => {
      changed = handler;
      return () => undefined;
    }),
    ...overrides,
  };
  return { ipc, fireChanged: () => changed() };
}

const link = (source: string, id: number | null, excerpt: string, title: string | null = null): Backlink => ({
  source_path: source,
  source_title: title,
  block: id === null ? null : block(id),
  excerpt,
});

describe("BacklinksPane", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("one block-precise entry per (note, block): three paragraphs over two notes are three entries", async () => {
    const backlinks = vi.fn().mockResolvedValue({
      kind: "ready",
      value: [link("Standup.md", 1, "decided to ship"), link("Standup.md", 2, "Priya flagged"), link("Roadmap.md", 3, "owner here")],
    });
    const { ipc } = ipcWith({ backlinks });
    const wrapper = mount(BacklinksPane, { props: { path: "Target.md", ipc } });
    await vi.advanceTimersByTimeAsync(150);
    expect(backlinks).toHaveBeenCalledWith({ path: "Target.md" });
    expect(wrapper.findAll(".serat-backlink")).toHaveLength(3);
    expect(wrapper.findAll(".serat-backlinks-source").map((h) => h.text())).toEqual(["Standup", "Roadmap"]); // grouped, header once per note
    expect(wrapper.find("h4").exists()).toBe(false); // no heading as a bare child of a list (ARIA)
    expect(wrapper.text()).toContain("Backlinks (3)");
  });

  it("labels a frontmatter-origin link 'frontmatter' and renders excerpts as text, not markup", async () => {
    const backlinks = vi.fn().mockResolvedValue({
      kind: "ready",
      value: [link("A.md", null, ""), link("B.md", 1, "<img src=x onerror=alert(1)>")],
    });
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc: ipcWith({ backlinks }).ipc } });
    await vi.advanceTimersByTimeAsync(150);
    expect(wrapper.find(".serat-backlink-frontmatter").text()).toBe("frontmatter");
    expect(wrapper.find("img").exists()).toBe(false);
    expect(wrapper.text()).toContain("<img src=x onerror=alert(1)>");
  });

  it("holds the skeleton for the minimum time, then shows the answer", async () => {
    const backlinks = vi.fn().mockResolvedValue({ kind: "ready", value: [link("A.md", 1, "x")] });
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc: ipcWith({ backlinks }).ipc } });
    await vi.advanceTimersByTimeAsync(10);
    expect(wrapper.find('[data-testid="backlinks-loading"]').exists()).toBe(true);
    expect(wrapper.findAll(".serat-backlink")).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(150);
    expect(wrapper.find('[data-testid="backlinks-loading"]').exists()).toBe(false);
    expect(wrapper.findAll(".serat-backlink")).toHaveLength(1);
  });

  it("states: empty, not ready, missing note", async () => {
    const empty = mount(BacklinksPane, {
      props: { path: "T.md", ipc: ipcWith({ backlinks: vi.fn().mockResolvedValue({ kind: "ready", value: [] }) }).ipc },
    });
    const notReady = mount(BacklinksPane, {
      props: {
        path: "T.md",
        ipc: ipcWith({ backlinks: vi.fn().mockResolvedValue({ kind: "not_ready", state: { kind: "opening" } }) }).ipc,
      },
    });
    const missing = mount(BacklinksPane, { props: { path: "T.md", missing: true, ipc: ipcWith().ipc } });
    await vi.advanceTimersByTimeAsync(150);
    expect(empty.text()).toContain("Nothing links here yet.");
    expect(notReady.find('[data-testid="backlinks-not-ready"]').exists()).toBe(true);
    expect(missing.text()).toContain("This note no longer exists");
  });

  it("re-queries on index://changed, not otherwise; Enter opens the focused entry at its block, Mod-Enter in a new tab", async () => {
    const backlinks = vi.fn().mockResolvedValue({ kind: "ready", value: [link("A.md", 1, "x"), link("B.md", 2, "y")] });
    const { ipc, fireChanged } = ipcWith({ backlinks });
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc } });
    await vi.advanceTimersByTimeAsync(150);
    expect(backlinks).toHaveBeenCalledTimes(1);
    fireChanged();
    await vi.advanceTimersByTimeAsync(150);
    expect(backlinks).toHaveBeenCalledTimes(2);

    const section = wrapper.get("section");
    await section.trigger("keydown", { key: "ArrowDown" });
    await section.trigger("keydown", { key: "Enter" });
    expect(wrapper.emitted("open")?.[0][0]).toMatchObject({ path: "B.md", newTab: false, block: { block_id: 2 } });
    await section.trigger("keydown", { key: "Enter", ctrlKey: true });
    expect(wrapper.emitted("open")?.[1][0]).toMatchObject({ newTab: true });
  });

  it("changing the open note re-queries; a superseded answer is ignored", async () => {
    let resolveFirst!: (v: unknown) => void;
    const backlinks = vi
      .fn()
      .mockReturnValueOnce(new Promise((r) => (resolveFirst = r)))
      .mockResolvedValueOnce({ kind: "ready", value: [link("Second.md", 1, "second")] });
    const wrapper = mount(BacklinksPane, { props: { path: "One.md", ipc: ipcWith({ backlinks }).ipc } });
    await vi.advanceTimersByTimeAsync(10);
    await wrapper.setProps({ path: "Two.md" });
    await vi.advanceTimersByTimeAsync(150);
    resolveFirst({ kind: "ready", value: [link("First.md", 1, "first")] });
    await vi.advanceTimersByTimeAsync(150);
    expect(wrapper.text()).toContain("second");
    expect(wrapper.text()).not.toContain("first");
  });
});

describe("BacklinksPane (round 2)", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("index://changed re-reads in the background: no skeleton, old answer stays until the new one swaps in", async () => {
    let release!: (v: unknown) => void;
    const backlinks = vi
      .fn()
      .mockResolvedValueOnce({ kind: "ready", value: [link("A.md", 1, "old")] })
      .mockReturnValueOnce(new Promise((r) => (release = r)));
    const { ipc, fireChanged } = ipcWith({ backlinks });
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc } });
    await vi.advanceTimersByTimeAsync(150);
    fireChanged();
    await vi.advanceTimersByTimeAsync(150);
    expect(wrapper.find('[data-testid="backlinks-loading"]').exists()).toBe(false);
    expect(wrapper.text()).toContain("old");
    release({ kind: "ready", value: [link("B.md", 2, "new")] });
    await vi.advanceTimersByTimeAsync(10);
    expect(wrapper.text()).toContain("new");
    expect(wrapper.text()).not.toContain("old");
  });

  it("a path change does show the skeleton", async () => {
    const backlinks = vi.fn().mockResolvedValue({ kind: "ready", value: [link("A.md", 1, "x")] });
    const wrapper = mount(BacklinksPane, { props: { path: "One.md", ipc: ipcWith({ backlinks }).ipc } });
    await vi.advanceTimersByTimeAsync(150);
    await wrapper.setProps({ path: "Two.md" });
    expect(wrapper.find('[data-testid="backlinks-loading"]').exists()).toBe(true);
  });

  it("roving tabindex: the section is the tab stop until an entry is focused; arrows move DOM focus", async () => {
    const backlinks = vi.fn().mockResolvedValue({ kind: "ready", value: [link("A.md", 1, "x"), link("B.md", 2, "y")] });
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc: ipcWith({ backlinks }).ipc }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(150);
    const section = wrapper.get("section");
    expect(section.attributes("tabindex")).toBe("0");
    expect(section.attributes("aria-activedescendant")).toBeUndefined();
    (section.element as HTMLElement).focus(); // tabbing in lands on the current entry
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement?.textContent).toContain("x");
    expect(section.attributes("tabindex")).toBe("-1");
    await section.trigger("keydown", { key: "ArrowDown" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement?.textContent).toContain("y");
    expect(document.activeElement?.getAttribute("tabindex")).toBe("-1");
    await section.trigger("keydown", { key: "ArrowUp" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement?.textContent).toContain("x");
    wrapper.unmount();
  });

  it("activating an entry opens it; the pane itself does not flash (the destination does, #82)", async () => {
    const backlinks = vi.fn().mockResolvedValue({ kind: "ready", value: [link("A.md", 1, "x")] });
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc: ipcWith({ backlinks }).ipc } });
    await vi.advanceTimersByTimeAsync(150);
    await wrapper.get(".serat-backlink").trigger("click");
    expect(wrapper.emitted("open")).toHaveLength(1);
    expect(wrapper.find(".serat-backlink-confirmed").exists()).toBe(false);
  });

  it("a listener that resolves after unmount is unlistened at once", async () => {
    const off = vi.fn();
    let resolve!: (f: () => void) => void;
    const onIndexChanged = vi.fn(() => new Promise<() => void>((r) => (resolve = r)));
    const wrapper = mount(BacklinksPane, { props: { path: "T.md", ipc: ipcWith({ onIndexChanged, backlinks: vi.fn().mockResolvedValue({ kind: "ready", value: [] }) }).ipc } });
    wrapper.unmount();
    resolve(off);
    await vi.advanceTimersByTimeAsync(0);
    expect(off).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------

const hit = (path: string, snippet: string | null, id = 1): SearchHit => ({
  path,
  title: null,
  kind: "note",
  mime: null,
  sidecar_path: null,
  block: block(id),
  snippet,
});
const hits = (items: SearchHit[]) => ({ kind: "ready" as const, value: { kind: "hits" as const, items } });

describe("SearchPanel", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("sends nothing while typing; one call 250 ms after the last keystroke", async () => {
    const search = vi.fn().mockResolvedValue(hits([hit("a/Roadmap.md", "the \u0002budget\u0003 review")]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    const field = wrapper.get('[data-testid="search-input"]');
    for (const q of ["b", "bu", "bud", "budg"]) {
      await field.setValue(q);
      await vi.advanceTimersByTimeAsync(100);
    }
    expect(search).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(search).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledWith({ query: "budg", offset: 0, limit: 50 });
    expect(wrapper.text()).toContain("1 result");
    expect(wrapper.get(".serat-hit-snippet b").text()).toBe("budget");
    expect(wrapper.text()).toContain("a"); // folder
  });

  it("Enter runs immediately and only once", async () => {
    const search = vi.fn().mockResolvedValue(hits([]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    const field = wrapper.get('[data-testid="search-input"]');
    await field.setValue("budget");
    await field.trigger("keydown", { key: "Enter" });
    await vi.advanceTimersByTimeAsync(400);
    expect(search).toHaveBeenCalledTimes(1);
  });

  it("refuses an unbalanced quote before sending, with the message, and keeps the characters", async () => {
    const search = vi.fn();
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    const field = wrapper.get('[data-testid="search-input"]');
    await field.setValue('"budget');
    await field.trigger("keydown", { key: "Enter" });
    await vi.advanceTimersByTimeAsync(400);
    expect(search).not.toHaveBeenCalled();
    expect(wrapper.get('[data-testid="search-refusal"]').text()).toBe("Close the quote to search a phrase");
    expect((field.element as HTMLInputElement).value).toBe('"budget');
  });

  it("refuses a second complete [key:value] filter before sending; an opening bracket alone is fine", async () => {
    const search = vi.fn().mockResolvedValue(hits([]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    const field = wrapper.get('[data-testid="search-input"]');
    await field.setValue("a [type:project] [owner:");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.find('[data-testid="search-refusal"]').exists()).toBe(false);
    expect(search).toHaveBeenCalledTimes(1);

    search.mockClear();
    await field.setValue("a [type:project] [owner:me]");
    await vi.advanceTimersByTimeAsync(300);
    expect(search).not.toHaveBeenCalled();
    expect(wrapper.get('[data-testid="search-refusal"]').text()).toContain("One property filter at a time");
    expect(wrapper.get('[data-testid="search-refusal"]').text()).toContain("type:project");
  });

  it("shows the chip and the case-sensitivity hint when a filter is active, and removes the chip on click", async () => {
    const search = vi.fn().mockResolvedValue(hits([]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    const field = wrapper.get('[data-testid="search-input"]');
    expect(wrapper.find('[data-testid="search-hint"]').exists()).toBe(false);
    await field.setValue("budget [type:project]");
    expect(wrapper.get('[data-testid="search-chip"]').text()).toContain("[type:project]");
    expect(wrapper.get('[data-testid="search-hint"]').text()).toContain("case-sensitive");
    await wrapper.get('[data-testid="search-chip"]').trigger("click");
    expect((field.element as HTMLInputElement).value).toBe("budget");
    await vi.advanceTimersByTimeAsync(400);
  });

  it("no results names the filter to remove", async () => {
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search: vi.fn().mockResolvedValue(hits([])) }).ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("budget [type:project]");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.get('[data-testid="search-none"]').text()).toBe(
      "No matches for “budget [type:project]” — try removing the [type:project] filter",
    );
  });

  it("a not-ready index replaces the results entirely and the query re-runs on index change", async () => {
    const search = vi
      .fn()
      .mockResolvedValueOnce({ kind: "not_ready", state: { kind: "opening" } })
      .mockResolvedValueOnce(hits([hit("A.md", "x")]));
    const { ipc, fireChanged } = ipcWith({ search });
    const wrapper = mount(SearchPanel, { props: { ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("budget");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.get('[data-testid="search-not-ready"]').text()).toContain("Building the index");
    expect(wrapper.find('[data-testid="search-results"]').exists()).toBe(false);

    fireChanged();
    await vi.advanceTimersByTimeAsync(10);
    expect(search).toHaveBeenCalledTimes(2);
    expect(wrapper.find('[data-testid="search-not-ready"]').exists()).toBe(false);
    expect(wrapper.findAll('[role="listitem"]')).toHaveLength(1);
  });

  it("Load more fetches the next page with the same query and appends", async () => {
    const page1 = Array.from({ length: 50 }, (_, i) => hit(`n${i}.md`, null, i));
    const search = vi
      .fn()
      .mockResolvedValueOnce(hits(page1))
      .mockResolvedValueOnce(hits([hit("last.md", null, 99)]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("note");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.findAll('[role="listitem"]')).toHaveLength(50);
    await wrapper.get('[data-testid="search-more"]').trigger("click");
    await vi.advanceTimersByTimeAsync(10);
    expect(search).toHaveBeenLastCalledWith({ query: "note", offset: 50, limit: 50 });
    expect(wrapper.findAll('[role="listitem"]')).toHaveLength(51);
    expect(wrapper.find('[data-testid="search-more"]').exists()).toBe(false);
  });

  it("a snippet with markup renders as text; Enter on a result opens it at its block", async () => {
    const search = vi.fn().mockResolvedValue(hits([hit("A.md", "<script>alert(1)</script> \u0002hit\u0003")]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("hit");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.find("script").exists()).toBe(false);
    expect(wrapper.text()).toContain("<script>alert(1)</script>");
    await wrapper.get('[data-testid="search-results"]').trigger("keydown", { key: "Enter" });
    expect(wrapper.emitted("open")?.[0][0]).toMatchObject({ path: "A.md", block: { block_id: 1 }, newTab: false });
  });

  it("a newer query wins over a slower earlier one", async () => {
    let releaseFirst!: (v: unknown) => void;
    const search = vi
      .fn()
      .mockReturnValueOnce(new Promise((r) => (releaseFirst = r)))
      .mockResolvedValueOnce(hits([hit("Second.md", null)]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    const field = wrapper.get('[data-testid="search-input"]');
    await field.setValue("one");
    await vi.advanceTimersByTimeAsync(300);
    await field.setValue("two");
    await vi.advanceTimersByTimeAsync(300);
    releaseFirst(hits([hit("First.md", null)]));
    await vi.advanceTimersByTimeAsync(10);
    expect(wrapper.text()).toContain("Second");
    expect(wrapper.text()).not.toContain("First");
  });

  it("shows skeleton rows only when the wait exceeds the threshold", async () => {
    let release!: (v: unknown) => void;
    const search = vi.fn().mockReturnValue(new Promise((r) => (release = r)));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("slow");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.find('[data-testid="search-loading"]').exists()).toBe(false);
    await vi.advanceTimersByTimeAsync(200);
    expect(wrapper.find('[data-testid="search-loading"]').exists()).toBe(true);
    release(hits([]));
    await vi.advanceTimersByTimeAsync(10);
    expect(wrapper.find('[data-testid="search-loading"]').exists()).toBe(false);
  });

  it("empty input shows guidance, then recent searches (session memory only)", async () => {
    const search = vi.fn().mockResolvedValue(hits([hit("A.md", null)]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    expect(wrapper.get('[data-testid="search-idle"]').text()).toContain("Search note text");
    await wrapper.get('[data-testid="search-input"]').setValue("budget");
    await vi.advanceTimersByTimeAsync(300);
    await wrapper.get('[data-testid="search-input"]').setValue("");
    await vi.advanceTimersByTimeAsync(300);
    expect(wrapper.get('[data-testid="search-idle"]').text()).toContain("budget");
  });

  it("Load more continues the query that produced the results, not what was typed since", async () => {
    const page1 = Array.from({ length: 50 }, (_, i) => hit(`n${i}.md`, null, i));
    const search = vi
      .fn()
      .mockResolvedValueOnce(hits(page1))
      .mockResolvedValueOnce(hits([hit("last.md", null, 99)]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("note");
    await vi.advanceTimersByTimeAsync(300);
    await wrapper.get('[data-testid="search-input"]').setValue("something else");
    await wrapper.get('[data-testid="search-more"]').trigger("click");
    await vi.advanceTimersByTimeAsync(10);
    expect(search).toHaveBeenNthCalledWith(2, { query: "note", offset: 50, limit: 50 });
  });

  it("Esc closes from the input and from the results list", async () => {
    const search = vi.fn().mockResolvedValue(hits([hit("A.md", null)]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc } });
    await wrapper.get('[data-testid="search-input"]').setValue("a");
    await vi.advanceTimersByTimeAsync(300);
    await wrapper.get('[data-testid="search-input"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.emitted("close")).toHaveLength(1);
    await wrapper.get('[data-testid="search-results"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.emitted("close")).toHaveLength(2);
  });

  it("roving tabindex on the results: arrows move DOM focus; the list is the tab stop until then", async () => {
    const search = vi.fn().mockResolvedValue(hits([hit("A.md", null, 1), hit("B.md", null, 2)]));
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith({ search }).ipc }, attachTo: document.body });
    await wrapper.get('[data-testid="search-input"]').setValue("a");
    await vi.advanceTimersByTimeAsync(300);
    const list = wrapper.get('[data-testid="search-results"]');
    expect(list.attributes("tabindex")).toBe("0");
    expect(list.attributes("aria-activedescendant")).toBeUndefined();
    (list.element as HTMLElement).focus();
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement?.textContent).toContain("A");
    await list.trigger("keydown", { key: "ArrowDown" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement?.textContent).toContain("B");
    expect(list.attributes("tabindex")).toBe("-1");
    wrapper.unmount();
  });

  it("focus() focuses the input (the host's open-or-focus chord)", async () => {
    const wrapper = mount(SearchPanel, { props: { ipc: ipcWith().ipc }, attachTo: document.body });
    (wrapper.vm as unknown as { focus(): void }).focus();
    expect(document.activeElement).toBe(wrapper.get('[data-testid="search-input"]').element);
    wrapper.unmount();
  });
});
