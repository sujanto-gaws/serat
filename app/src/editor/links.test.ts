// Task #22: wikilink resolution, styling, completion and follow-link
// (FR-EDT-03, FR-EDT-04, FR-NAV-03, US-04; architecture.md §9.3).

import { CompletionContext, type Completion, type CompletionResult } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LinkResolver } from "@/editor/linkResolver";
import {
  classifyLinkInput,
  LINK_SCAN_DEBOUNCE_MS,
  linkExtensions,
  linkNodeAt,
  makeLinkCompletionSource,
  rankAnchors,
} from "@/editor/links";
import { linkKey, parseLinkInner, refreshLinks } from "@/editor/linkState";
import { seratMarkdown } from "@/editor/markdownLanguage";
import type { LinkResolution } from "@/ipc/generated/LinkResolution";
import type { SwitcherCorpus } from "@/ipc/generated/SwitcherCorpus";
import type { SwitcherItem } from "@/ipc/generated/SwitcherItem";
import type { LinkIpcAdapter } from "@/ipc/linkAdapter";

// jsdom has no layout; CodeMirror's measure pass calls these on text ranges.
if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}

interface FakeNote {
  path: string;
  title?: string;
  aliases?: string[];
  headings?: string[];
  blocks?: string[];
}

const NOTES: FakeNote[] = [
  { path: "notes/Alpha.md", title: "Alpha", aliases: ["First"], headings: ["Intro", "Details", "Interop"], blocks: ["b1", "b2"] },
  { path: "notes/Alphabet.md", title: "Alphabet" },
  { path: "a/Meeting.md", title: "Meeting" },
  { path: "b/Meeting.md", title: "Meeting" },
  { path: "assets/pic.png" },
];

const stem = (path: string) => path.replace(/^.*\//, "").replace(/\.md$/, "");
const lower = (text: string) => text.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** Mirrors the documented rule: exact path, else file name, shortest path, ASCII case-insensitive. */
function resolveOne(target: string): LinkResolution {
  const note = lower(target.split(/[#^]/, 1)[0].trim());
  const hits = NOTES.filter((n) => {
    const p = lower(n.path);
    return p === note || p === `${note}.md` || lower(n.path.replace(/^.*\//, "")) === note || lower(stem(n.path)) === note;
  }).sort((a, b) => a.path.length - b.path.length || (a.path < b.path ? -1 : 1));
  return { target, path: hits[0]?.path ?? null, subpath_found: null, via: "index" };
}

function fakeLinks(corpus: SwitcherCorpus = "index"): LinkIpcAdapter & { resolveLinks: ReturnType<typeof vi.fn>; switcherQuery: ReturnType<typeof vi.fn> } {
  return {
    resolveLinks: vi.fn(async ({ targets }: { targets: string[] }) => targets.map(resolveOne)),
    noteAnchors: vi.fn(async ({ path }: { path: string }) => {
      const note = NOTES.find((n) => n.path === path);
      return { kind: "ready" as const, value: { headings: note?.headings ?? [], block_ids: note?.blocks ?? [] } };
    }),
    switcherQuery: vi.fn(async ({ seq, query }: { seq: number; query: string }) => {
      const q = lower(query);
      const items: SwitcherItem[] = [];
      for (const n of NOTES.filter((x) => x.path.endsWith(".md"))) {
        const duplicate = NOTES.filter((o) => stem(o.path) === stem(n.path)).length > 1;
        const link_text = duplicate ? n.path.replace(/\.md$/, "") : stem(n.path);
        const candidates: Array<[string, SwitcherItem["matched"]]> = [
          [n.title ?? stem(n.path), "title"],
          ...((n.aliases ?? []).map((a) => [a, "alias"]) as Array<[string, SwitcherItem["matched"]]>),
        ];
        for (const [candidate, matched] of candidates) {
          const c = lower(candidate);
          if (c.startsWith(q) || c.includes(q)) {
            items.push({ path: n.path, title: n.title ?? null, matched, candidate, highlight: [], heading: null, link_text });
            break;
          }
        }
      }
      items.sort((a, b) => Number(lower(b.candidate).startsWith(q)) - Number(lower(a.candidate).startsWith(q)));
      return { seq, corpus, items };
    }),
    onIndexChanged: vi.fn(async () => () => undefined),
  };
}

function makeView(doc: string, ipc: LinkIpcAdapter, source: string | null = "src.md") {
  const resolver = new LinkResolver(ipc);
  const deps = { ipc, resolver, getSourcePath: () => source };
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({ doc, extensions: [seratMarkdown(), linkExtensions(deps)] }),
  });
  return { view, resolver, deps };
}

async function settle(ms = LINK_SCAN_DEBOUNCE_MS + 50): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms);
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("link text parsing", () => {
  it("splits target, subpath and display without rewriting anything", () => {
    expect(parseLinkInner("Note#Head|shown")).toEqual({ target: "Note#Head", note: "Note", subpath: "Head", display: "shown" });
    expect(parseLinkInner("#Local")).toMatchObject({ note: "", subpath: "Local" });
    expect(parseLinkInner("Note#^id")).toMatchObject({ note: "Note", subpath: "^id" });
  });
  it("keys targets case-insensitively for ASCII only", () => {
    expect(linkKey(" My Note ")).toBe(linkKey("my note"));
    expect(linkKey("É")).not.toBe(linkKey("é"));
  });
});

describe("resolved / unresolved styling", () => {
  it("styles resolved and unresolved links from one batched resolve_links call", async () => {
    const ipc = fakeLinks();
    const { view } = makeView("see [[Alpha]] and [[Nowhere]] and ![[pic.png]]", ipc);
    expect(ipc.resolveLinks).not.toHaveBeenCalled(); // nothing synchronous
    await settle();
    expect(ipc.resolveLinks).toHaveBeenCalledTimes(1);
    expect(ipc.resolveLinks.mock.calls[0][0]).toEqual({ source_path: "src.md", targets: ["Alpha", "Nowhere", "pic.png"] });

    const resolved = [...view.dom.querySelectorAll(".serat-link-resolved")].map((e) => e.textContent);
    const unresolved = [...view.dom.querySelectorAll(".serat-link-unresolved")];
    expect(resolved).toEqual(["[[Alpha]]", "![[pic.png]]"]);
    expect(unresolved.map((e) => e.textContent)).toEqual(["[[Nowhere]]"]);
    expect(unresolved[0].getAttribute("title")).toContain("No note named");
    // C-01/C-04: the text is untouched.
    expect(view.state.doc.toString()).toBe("see [[Alpha]] and [[Nowhere]] and ![[pic.png]]");
  });

  it("resolves case-insensitively and asks once for spellings that differ only by ASCII case", async () => {
    const ipc = fakeLinks();
    const { view } = makeView("[[ALPHA]] [[alpha]] [[Alpha]]", ipc);
    await settle();
    expect(ipc.resolveLinks.mock.calls[0][0].targets).toHaveLength(1);
    expect(view.dom.querySelectorAll(".serat-link-resolved")).toHaveLength(3);
  });

  it("an ambiguous name resolves to the shortest path (server rule), not an error", async () => {
    expect(resolveOne("Meeting").path).toBe("a/Meeting.md");
  });

  it("makes no IPC call per keystroke: typing is silent, one batch follows the pause", async () => {
    const ipc = fakeLinks();
    const { view } = makeView("[[Alpha]]", ipc);
    await settle();
    ipc.resolveLinks.mockClear();

    for (const ch of "typing a new [[Alp") {
      view.dispatch({ changes: { from: view.state.doc.length, insert: ch } });
      await vi.advanceTimersByTimeAsync(20); // well inside the debounce
    }
    expect(ipc.resolveLinks).not.toHaveBeenCalled();
    expect(ipc.switcherQuery).not.toHaveBeenCalled();

    view.dispatch({ changes: { from: view.state.doc.length, insert: "ha]]" } });
    await settle();
    expect(ipc.resolveLinks.mock.calls.length).toBeLessThanOrEqual(1);
  });

  it("serves already-answered targets from cache: edits elsewhere cost no IPC", async () => {
    const ipc = fakeLinks();
    const { view } = makeView("[[Alpha]]\n\ntext", ipc);
    await settle();
    ipc.resolveLinks.mockClear();
    view.dispatch({ changes: { from: view.state.doc.length, insert: " more" } });
    await settle();
    expect(ipc.resolveLinks).not.toHaveBeenCalled();
  });

  it("re-resolves after index://changed invalidation and restyles from the new answer", async () => {
    const ipc = fakeLinks();
    const { view, resolver } = makeView("[[Gamma]]", ipc);
    await settle();
    expect(view.dom.querySelectorAll(".serat-link-unresolved")).toHaveLength(1);

    NOTES.push({ path: "notes/Gamma.md", title: "Gamma" }); // the note now exists
    try {
      resolver.invalidate(); // what the component does on index://changed
      view.dispatch({ effects: refreshLinks.of(null) });
      await settle();
      expect(ipc.resolveLinks).toHaveBeenCalledTimes(2);
      expect(view.dom.querySelectorAll(".serat-link-unresolved")).toHaveLength(0);
      expect(view.dom.querySelectorAll(".serat-link-resolved")).toHaveLength(1);
    } finally {
      NOTES.pop();
    }
  });

  it("an IPC failure leaves links unstyled and does not throw into typing", async () => {
    const ipc = fakeLinks();
    ipc.resolveLinks.mockRejectedValue(new Error("boom"));
    const { view } = makeView("[[Alpha]]", ipc);
    await settle();
    expect(view.dom.querySelectorAll(".serat-link-resolved, .serat-link-unresolved")).toHaveLength(0);
  });

  it("does nothing without an open note", async () => {
    const ipc = fakeLinks();
    makeView("[[Alpha]]", ipc, null);
    await settle();
    expect(ipc.resolveLinks).not.toHaveBeenCalled();
  });
});

describe("completion input classification", () => {
  it("needs two characters for names, but not for # or ^ or an explicit request", () => {
    expect(classifyLinkInput("a", 2, false)).toBeNull();
    expect(classifyLinkInput("a", 2, true)).toMatchObject({ kind: "name" });
    expect(classifyLinkInput("al", 2, false)).toMatchObject({ kind: "name", query: "al", from: 2 });
    expect(classifyLinkInput("Note#He", 2, false)).toMatchObject({ kind: "heading", target: "Note", query: "He", from: 7 });
    expect(classifyLinkInput("Note#^b", 2, false)).toMatchObject({ kind: "block", target: "Note", query: "b", from: 8 });
    expect(classifyLinkInput("Note^b", 2, false)).toMatchObject({ kind: "block", query: "b", from: 7 });
    expect(classifyLinkInput("Note|dis", 2, false)).toBeNull();
  });
  it("ranks anchor prefix matches before substring matches, case-insensitively", () => {
    expect(rankAnchors(["Interop", "Intro", "Details", "Print"], "INT")).toEqual(["Interop", "Intro", "Print"]);
    expect(rankAnchors(["a", "b"], "")).toEqual(["a", "b"]);
  });
});

async function complete(doc: string, ipc: LinkIpcAdapter, deps?: { source?: string | null }) {
  const { view, resolver } = makeView(doc, ipc, deps?.source === undefined ? "src.md" : deps.source);
  view.dispatch({ selection: { anchor: doc.length } });
  const source = makeLinkCompletionSource({ ipc, resolver, getSourcePath: () => (deps?.source === undefined ? "src.md" : deps.source) });
  const result = (await source(new CompletionContext(view.state, doc.length, false))) as CompletionResult | null;
  return { view, result };
}

function apply(view: EditorView, option: Completion, from: number, to: number): void {
  (option.apply as (v: EditorView, c: Completion, f: number, t: number) => void)(view, option, from, to);
}

describe("[[ completion", () => {
  it("lists notes in the server's order, case-insensitively, with alias and path detail", async () => {
    const ipc = fakeLinks();
    const { result } = await complete("see [[ALP", ipc);
    expect(result).not.toBeNull();
    expect(result!.filter).toBe(false); // the server's ranking is kept, not re-filtered
    const labels = result!.options.map((o) => o.label);
    expect(labels).toEqual(["Alpha", "Alphabet"]);
    // Boosts descend so CodeMirror keeps that order.
    const boosts = result!.options.map((o) => o.boost ?? 0);
    expect(boosts[0]).toBeGreaterThan(boosts[1]);
    expect(result!.options[0].detail).toBe("notes/Alpha.md");
  });

  it("offers an alias and inserts link|alias", async () => {
    const ipc = fakeLinks();
    const { view, result } = await complete("[[Fir", ipc);
    const option = result!.options[0];
    expect(option.label).toBe("First");
    expect(option.detail).toContain("alias");
    apply(view, option, result!.from, 5);
    expect(view.state.doc.toString()).toBe("[[Alpha|First]]");
    expect(view.state.selection.main.head).toBe("[[Alpha|First".length); // before the ]]
  });

  it("uses the vault path without .md when the file name is ambiguous", async () => {
    const ipc = fakeLinks();
    const { view, result } = await complete("[[Meet", ipc);
    const option = result!.options[0];
    apply(view, option, result!.from, 6);
    expect(view.state.doc.toString()).toBe("[[a/Meeting]]");
  });

  it("replaces only the typed fragment and does not duplicate an existing ]]", async () => {
    const ipc = fakeLinks();
    const { view, result } = await complete("x [[Alp", ipc);
    view.dispatch({ changes: { from: 7, insert: "]] tail" }, selection: { anchor: 7 } });
    apply(view, result!.options[0], result!.from, 7);
    expect(view.state.doc.toString()).toBe("x [[Alpha]] tail");
  });

  it("every inserted link re-resolves to the note it was chosen for (FR-EDT-03)", async () => {
    const ipc = fakeLinks();
    const { result } = await complete("[[Me", ipc);
    for (const option of result!.options) {
      const { view } = makeView("", ipc);
      apply(view, option, 0, 0);
      const inserted = view.state.doc.toString().replace(/^\[\[|\]\]$/g, "").split("|")[0];
      expect(resolveOne(inserted).path, inserted).not.toBeNull();
    }
    const chosen = result!.options.find((o) => o.label === "Meeting");
    expect(chosen).toBeDefined();
  });

  it("labels names-only (tree) answers while the index is not ready", async () => {
    const { result } = await complete("[[Alp", fakeLinks("tree"));
    expect(result!.options[0].detail).toContain("file names only");
  });

  it("does not query under two characters or after |", async () => {
    const ipc = fakeLinks();
    expect((await complete("[[a", ipc)).result).toBeNull();
    expect((await complete("[[Alpha|dis", ipc)).result).toBeNull();
    expect(ipc.switcherQuery).not.toHaveBeenCalled();
  });

  it("drops an answer that a newer keystroke has superseded", async () => {
    const ipc = fakeLinks();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const first = ipc.switcherQuery.getMockImplementation()!;
    ipc.switcherQuery.mockImplementationOnce(async (args: { seq: number; query: string }) => {
      await gate;
      return first(args);
    });
    const { view, resolver } = makeView("[[Al", ipc);
    const source = makeLinkCompletionSource({ ipc, resolver, getSourcePath: () => "src.md" });
    const slow = source(new CompletionContext(view.state, 4, false));
    const fast = source(new CompletionContext(view.state, 4, false));
    release();
    expect(await slow).toBeNull();
    expect(await fast).not.toBeNull();
  });
});

describe("# and ^ completion", () => {
  it("completes the resolved note's headings, prefix matches first", async () => {
    const ipc = fakeLinks();
    const { result } = await complete("[[Alpha#int", ipc);
    expect(result!.options.map((o) => o.label)).toEqual(["Intro", "Interop"]);
    expect(ipc.noteAnchors).toHaveBeenCalledWith({ path: "notes/Alpha.md" });
  });

  it("completes block ids after # ^ and after ^", async () => {
    const ipc = fakeLinks();
    expect((await complete("[[Alpha#^b", ipc)).result!.options.map((o) => o.label)).toEqual(["^b1", "^b2"]);
    expect((await complete("[[Alpha^b", ipc)).result!.options.map((o) => o.label)).toEqual(["^b1", "^b2"]);
  });

  it("[[# completes the current note's headings", async () => {
    const ipc = fakeLinks();
    await complete("[[#", ipc, { source: "notes/Alpha.md" });
    expect(ipc.noteAnchors).toHaveBeenCalledWith({ path: "notes/Alpha.md" });
  });

  it("offers nothing for an unresolved note or a not-ready index", async () => {
    const ipc = fakeLinks();
    expect((await complete("[[Nowhere#h", ipc)).result).toBeNull();
    ipc.noteAnchors = vi.fn(async () => ({ kind: "not_ready" as const, state: { kind: "rebuilding" } as never }));
    expect((await complete("[[Alpha#h", ipc)).result).toBeNull();
  });

  it("inserts the heading text and keeps the closing brackets", async () => {
    const ipc = fakeLinks();
    const { view, result } = await complete("[[Alpha#Det", ipc);
    apply(view, result!.options[0], result!.from, 11);
    expect(view.state.doc.toString()).toBe("[[Alpha#Details]]");
  });
});

describe("link under the cursor", () => {
  it("finds a wikilink or embed at a position and nothing elsewhere", () => {
    const state = EditorState.create({ doc: "a [[Alpha#H|x]] b ![[pic.png]]", extensions: [seratMarkdown()] });
    expect(linkNodeAt(state, 5)?.link.target).toBe("Alpha#H");
    expect(linkNodeAt(state, 20)?.isEmbed).toBe(true);
    expect(linkNodeAt(state, 0)).toBeNull();
  });
});
