import { describe, expect, it } from "vitest";

import { createPinia, setActivePinia } from "pinia";

import type { IndexState } from "@/ipc/generated/IndexState";

import { describeIndexDetail, describeIndexState, useVaultStore } from "./vault";

// Exhaustive over IndexState's four variants (contracts/ipc.rs). §3.4: every one of
// them renders as a status line, never a modal and never a blocking spinner — this
// asserts each has a defined, non-empty rendering rather than assuming the switch
// covers them all.
describe("describeIndexState", () => {
  it("renders opening", () => {
    expect(describeIndexState({ kind: "opening" })).toBe("Opening index…");
  });

  it("renders ready with the note count", () => {
    expect(describeIndexState({ kind: "ready", note_count: 20_200 })).toBe(
      "index: ready · 20200 notes",
    );
  });

  it("renders rebuilding before the walk completes as a running count", () => {
    const text = describeIndexState({
      kind: "rebuilding",
      reason: "no_index",
      done: 120,
      total: 4_000,
      walk_complete: false,
    });
    expect(text).toContain("Rebuilding index (no_index)");
    expect(text).toContain("120");
    expect(text).not.toContain("120/4000");
  });

  it("renders rebuilding after the walk completes as a fraction", () => {
    const text = describeIndexState({
      kind: "rebuilding",
      reason: "vault_moved",
      done: 4_000,
      total: 4_000,
      walk_complete: true,
    });
    expect(text).toContain("4000/4000");
  });

  it("renders unavailable with the operator-readable message", () => {
    const text = describeIndexState({
      kind: "unavailable",
      message: "task I6 not implemented",
      path: "/vault/.serat/index.sqlite3",
    });
    expect(text).toBe("index: unavailable — task I6 not implemented");
  });
});

describe("opening reason (P1 #117)", () => {
  const waiting = { kind: "opening", reason: "previous_index_still_closing" } as const;

  it("renders the reason copy, with no internal term or number", () => {
    const text = describeIndexState(waiting);
    expect(text).toBe("Finishing the previous session's index — search will start when it's done");
    expect(text).not.toMatch(/actor|writer|detached|index\.db|lock|\d/i);
    expect(describeIndexDetail(waiting)).toContain("You can keep editing.");
  });

  it("renders absent, null and unknown reasons as the plain text without throwing", () => {
    for (const s of [
      { kind: "opening" },
      { kind: "opening", reason: null },
      { kind: "opening", reason: "held_by_another_process" },
    ]) {
      expect(describeIndexState(s as unknown as IndexState)).toBe("Opening index…");
      expect(describeIndexDetail(s as unknown as IndexState)).toBeNull();
    }
  });

  it("clears the reason when the state moves to Ready or Rebuilding", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.setIndexState(waiting);
    expect(store.indexStatusDetail).not.toBeNull();
    store.setIndexState({ kind: "ready", note_count: 3 });
    expect(store.indexStatusText).toBe("index: ready · 3 notes");
    expect(store.indexStatusDetail).toBeNull();
  });
});
