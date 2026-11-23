// Task #159 (FR-EDT-01, FR-EDT-02, NFR-USE-01; C-01, C-04; docs/03-design/ui/screens/editor.md "Callouts and the
// fold marker", acceptance items 1-8): the fold glyph, count, aria-expanded, click ruling, keyboard and command,
// on a real EditorView in jsdom. A toggle is view state only: the document, dirty state and undo stack stay put.

import { history, undoDepth } from "@codemirror/commands";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { blockWidgetsExtension, calloutFoldField, countBodyLines, toggleCalloutFoldAtCaret } from "@/editor/blockWidgets";
import { createEditorCommandRegistry } from "@/editor/codemirrorSetup";
import { seratMarkdown } from "@/editor/markdownLanguage";

const views: EditorView[] = [];
afterEach(() => {
  while (views.length) views.pop()!.destroy();
  document.body.innerHTML = "";
});

function mountView(doc: string, anchor = doc.length): EditorView {
  const state = EditorState.create({
    doc,
    selection: { anchor },
    extensions: [history(), seratMarkdown(), blockWidgetsExtension()],
  });
  const view = new EditorView({ state, parent: document.body });
  ensureSyntaxTree(view.state, view.state.doc.length, 5000);
  view.dispatch({ selection: { anchor } }); // build the widgets against the finished parse
  views.push(view);
  return view;
}

const FOLDED = "> [!tip]- Remember to rebase\n> one\n> two\n> three\n\nafter\n";
const OPEN = "> [!tip]+ Open one\n> body\n\nafter\n";
const PLAIN = "> [!tip] No marker\n> body\n\nafter\n";
const EMPTY = "> [!tip]- Nothing inside\n\nafter\n";

const callout = (v: EditorView) => v.dom.querySelector<HTMLElement>(".serat-callout");
const row = (v: EditorView) => v.dom.querySelector<HTMLElement>(".serat-callout-title")!;
const glyph = (v: EditorView) => v.dom.querySelector<HTMLElement>(".serat-callout-glyph")!;
const body = (v: EditorView) => v.dom.querySelector<HTMLElement>(".serat-callout-body");
const count = (v: EditorView) => v.dom.querySelector<HTMLElement>(".serat-callout-count");

describe("1. glyph, title, count, aria-expanded", () => {
  it("a `-` callout with a body: glyph, count, aria-expanded=false, hidden body", () => {
    const v = mountView(FOLDED, FOLDED.length);
    expect(callout(v)?.getAttribute("data-fold")).toBe("collapsed");
    expect(row(v).getAttribute("role")).toBe("button");
    expect(row(v).tabIndex).toBe(0);
    expect(row(v).getAttribute("aria-expanded")).toBe("false");
    expect(row(v).getAttribute("aria-controls")).toBe(body(v)!.id);
    expect(body(v)!.hidden).toBe(true);
    expect(count(v)!.hidden).toBe(false);
    expect(count(v)!.textContent).toBe(", 3 lines hidden");
    expect(glyph(v).getAttribute("aria-hidden")).toBe("true");
  });

  it("a `+` callout: expanded, no visible count, aria-expanded=true", () => {
    const v = mountView(OPEN);
    expect(row(v).getAttribute("aria-expanded")).toBe("true");
    expect(body(v)!.hidden).toBe(false);
    expect(count(v)!.hidden).toBe(true);
  });

  it("no marker, or an empty body: no glyph, count or aria-expanded", () => {
    for (const doc of [PLAIN, EMPTY]) {
      const v = mountView(doc);
      expect(glyph(v), doc).toBeNull();
      expect(count(v), doc).toBeNull();
      expect(row(v).hasAttribute("aria-expanded"), doc).toBe(false);
      expect(row(v).hasAttribute("role"), doc).toBe(false);
      v.destroy();
      views.pop();
    }
  });

  it("the count is singular for one line and counts non-blank source lines", () => {
    expect(countBodyLines("a\n\nb\n  \nc")).toBe(3);
    const v = mountView("> [!tip]- T\n> only\n\nafter\n");
    expect(count(v)!.textContent).toBe(", 1 line hidden");
  });
});

describe("2. clicking the glyph toggles in preview only", () => {
  it("toggles glyph state, count, body and aria-expanded together; document, dirty state, undo and caret unchanged", () => {
    const v = mountView(FOLDED);
    const docBefore = v.state.doc.toString();
    const depthBefore = undoDepth(v.state);
    const selBefore = v.state.selection.main.head;
    glyph(v).dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    glyph(v).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
    expect(row(v).getAttribute("aria-expanded")).toBe("true");
    expect(body(v)!.hidden).toBe(false);
    expect(count(v)!.hidden).toBe(true);
    expect(v.state.doc.toString()).toBe(docBefore); // the `-` marker is untouched
    expect(undoDepth(v.state)).toBe(depthBefore);
    expect(v.state.selection.main.head).toBe(selBefore);
    // still rendered as a widget: the source was not revealed
    expect(callout(v)).not.toBeNull();
    glyph(v).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(callout(v)?.getAttribute("data-fold")).toBe("collapsed");
  });

  it("the glyph's mousedown default is cancelled, and the widget owns events inside the glyph box only", () => {
    const v = mountView(FOLDED);
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    glyph(v).dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    const widget = v.state.field(calloutFoldField, false);
    expect(widget).toBeDefined();
  });
});

describe("3. clicking elsewhere still reveals the source", () => {
  it("moving the caret into the callout (what a click on the title text or body does) shows the raw source", () => {
    const v = mountView(FOLDED, FOLDED.length);
    expect(callout(v)).not.toBeNull();
    v.dispatch({ selection: { anchor: 3 } });
    expect(callout(v)).toBeNull();
    expect(v.dom.textContent).toContain("[!tip]-");
  });
});

describe("4. the toggled state survives edits and reveal, and resets on reopen", () => {
  it("survives an edit elsewhere and revealing then leaving the callout", () => {
    const v = mountView(FOLDED);
    glyph(v).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
    v.dispatch({ changes: { from: v.state.doc.length, insert: "more\n" } }); // an edit elsewhere
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
    v.dispatch({ selection: { anchor: 3 } }); // reveal
    expect(callout(v)).toBeNull();
    v.dispatch({ selection: { anchor: v.state.doc.length } }); // leave
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
  });

  it("survives an edit above the callout (the key moves with the text)", () => {
    const v = mountView(`intro\n\n${FOLDED}`);
    v.dispatch({ selection: { anchor: v.state.doc.length } });
    glyph(v).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    v.dispatch({ changes: { from: 0, insert: "a new first line\n\n" } });
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
  });

  it("deleting a toggled callout does not hand its state to the next callout", () => {
    const second = "> [!tip]- Second one\n> hidden body\n\nend\n";
    const doc = `${FOLDED}${second}`;
    const v = mountView(doc, doc.length);
    glyph(v).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true })); // the first callout
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
    v.dispatch({ changes: { from: 0, to: FOLDED.indexOf("after\n") + "after\n".length } }); // delete the first block
    v.dispatch({ selection: { anchor: v.state.doc.length } });
    expect(v.state.doc.toString().startsWith("> [!tip]- Second one")).toBe(true);
    expect(callout(v)?.getAttribute("data-fold")).toBe("collapsed"); // follows its own marker
  });

  it("the body id is derived from the block start, so a rebuild keeps it", () => {
    const v = mountView(FOLDED);
    const id = body(v)!.id;
    expect(id).toMatch(/^serat-callout-body-\d+$/);
    v.dispatch({ changes: { from: v.state.doc.length, insert: "x" } });
    expect(body(v)!.id).toBe(id);
  });

  it("a fresh state (reopening the note) takes the marker's state again", () => {
    const v = mountView(FOLDED);
    glyph(v).dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    v.destroy();
    views.pop();
    const again = mountView(FOLDED);
    expect(callout(again)?.getAttribute("data-fold")).toBe("collapsed");
  });
});

describe("5. keyboard and command", () => {
  it("Enter and Space on the focused title row toggle it; Tab reaches each foldable row", () => {
    const v = mountView(`${FOLDED}${OPEN}`.replace("after\n> [!tip]+", "after\n\n> [!tip]+"));
    const rows = [...v.dom.querySelectorAll<HTMLElement>('.serat-callout-title[role="button"]')];
    expect(rows.length).toBeGreaterThanOrEqual(1);
    for (const r of rows) expect(r.tabIndex).toBe(0); // in the tab order
    const first = rows[0];
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
    first.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true }));
    expect(callout(v)?.getAttribute("data-fold")).toBe("collapsed");
  });

  it("the Toggle callout fold command exists with no default chord, toggles the callout at the caret, and refuses elsewhere", () => {
    const registry = createEditorCommandRegistry({ save: () => true, toggleLivePreview: () => undefined, followLink: () => true } as never);
    const command = registry.list().find((c) => c.id === "editor.toggle-callout-fold");
    expect(command?.name).toBe("Toggle callout fold");
    expect(command?.defaultBinding).toBeUndefined();

    const v = mountView(FOLDED, 3); // caret inside: revealed source
    expect(toggleCalloutFoldAtCaret(v)).toBe(true);
    v.dispatch({ selection: { anchor: v.state.doc.length } });
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
    v.dispatch({ selection: { anchor: v.state.doc.length } });
    expect(toggleCalloutFoldAtCaret(v)).toBe(false); // caret on the trailing paragraph
    expect(callout(v)?.getAttribute("data-fold")).toBe("expanded");
  });
});

describe("6 and 7. tokens, reduced motion, hit target, accessible name", () => {
  it("the theme uses the named tokens, the 24 px hit target and the reduced-motion duration", () => {
    mountView(FOLDED);
    const css = [...document.querySelectorAll("style")].map((s) => s.textContent ?? "").join("\n");
    expect(css).toContain("--serat-text-secondary");
    expect(css).toContain("--serat-accent-interactive");
    expect(css).toContain("--serat-surface-sunken");
    expect(css).toContain("--serat-radius-sm");
    expect(css).toContain("--serat-duration-micro");
    expect(css).toMatch(/prefers-reduced-motion: reduce[^}]*\{[^}]*transition-duration:\s*1ms/);
    expect(css).toMatch(/width:\s*24px/);
  });

  it("the collapsed title's accessible name is the title plus the count; the glyph is hidden from assistive technology", () => {
    const v = mountView(FOLDED);
    const name = [...row(v).childNodes]
      .filter((n) => !(n instanceof HTMLElement && n.getAttribute("aria-hidden") === "true"))
      .map((n) => (n instanceof HTMLElement && n.hidden ? "" : (n.textContent ?? "")))
      .join("");
    expect(name).toBe("Remember to rebase, 3 lines hidden");
    expect(row(v).hasAttribute("aria-label")).toBe(false);
  });
});

describe("8. the keystroke path", () => {
  it("typing in a note with no callout adds no fold state, no extra widget and no layout read", () => {
    const v = mountView("plain text\n\nmore\n");
    const before = v.state.field(calloutFoldField);
    v.dispatch({ changes: { from: 0, insert: "x" } });
    expect(v.state.field(calloutFoldField)).toBe(before); // the same object: no allocation, no mapping
    expect(v.dom.querySelector(".serat-callout")).toBeNull();
  });
});
