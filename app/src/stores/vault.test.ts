import { describe, expect, it } from "vitest";

import { describeIndexState } from "./vault";

// Exhaustive over IndexState's four variants (contracts/ipc.rs). §3.4: every one of
// them renders as a status line, never a modal and never a blocking spinner — this
// asserts each has a defined, non-empty rendering rather than assuming the switch
// covers them all.
describe("describeIndexState", () => {
  it("renders opening", () => {
    expect(describeIndexState({ kind: "opening" })).toBe("index: opening");
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
