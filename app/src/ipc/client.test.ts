import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * The IPC surface against the ACL that admits it (#125, F-467, ADR-0012).
 *
 * `crates/serat-app/capabilities/default.json` grants the webview five commands and
 * withholds `open_vault`, and its own justification is that the front end invokes
 * exactly those five. That justification was written about the hand-authored static
 * shell; since #98 the same directory is Vite output, and #102 could only verify it
 * by reading the *shipped bundle*. Until this file, the property held by Rollup
 * dropping an uncalled export — a bundler's optimisation standing in for an
 * assertion — and the first caller of a withheld command would have learned about
 * the ACL from a runtime denial (F-467).
 *
 * So this compares the two lists directly, at authoring time, in both directions:
 *
 *   * a wrapper for a command that is not granted is a runtime denial waiting for
 *     its first caller;
 *   * a grant with no wrapper is surface the ADR says should not be open — the
 *     capability's description claims the set is what the webview *actually*
 *     invokes, so an unused grant makes that description false.
 *
 * It reads the capability file rather than restating its contents, because a copy of
 * a list in a second place is a thing that goes stale silently. If the ACL and this
 * front end are meant to diverge, that is an ADR-0012 decision and this test is where
 * it gets recorded.
 *
 * Reading both files off disk is deliberate, and worth a clause because a reader
 * arriving from F-470 will pause at it. F-470's hazard is a check that reads a *file*
 * while the behaviour it judges lives in a compiled copy — there, the file is the
 * stale thing. Here the source *is* the subject: the claim is about what this front
 * end is written to invoke, so reading anything downstream of the bundler would be
 * reading the wrong artefact. F-470's own discriminator — does the observer read the
 * file that was mutated, or a compiled copy of it? — comes out on the safe side.
 */
describe("the IPC client stays inside ADR-0012's grant", () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const clientSource = readFileSync(join(here, "client.ts"), "utf8");
  const capabilityPath = join(here, "..", "..", "..", "crates", "serat-app", "capabilities", "default.json");

  /** Command names as this file's wrappers actually pass them to `invoke`. */
  function invokedCommands(): string[] {
    return [...clientSource.matchAll(/invoke\(\s*"([a-z_]+)"/g)].map((match) => match[1]).sort();
  }

  /**
   * Tauri names a command permission `allow-<command-with-dashes>`; everything else
   * in the list (`core:default`) is not a command grant.
   */
  function grantedCommands(): string[] {
    const capability = JSON.parse(readFileSync(capabilityPath, "utf8")) as { permissions: string[] };
    return capability.permissions
      .filter((permission) => permission.startsWith("allow-"))
      .map((permission) => permission.slice("allow-".length).replaceAll("-", "_"))
      .sort();
  }

  it("reads a capability file that actually grants something", () => {
    // Without this, a moved or renamed capability file would make the comparison
    // below "no wrappers, no grants" and pass over nothing.
    expect(grantedCommands().length).toBeGreaterThan(0);
    expect(invokedCommands().length).toBeGreaterThan(0);
  });

  it("invokes no command the capability withholds", () => {
    const ungranted = invokedCommands().filter((command) => !grantedCommands().includes(command));
    expect(
      ungranted,
      `app/src/ipc/client.ts wraps ${ungranted.join(", ")}, which ADR-0012's capability ` +
        `does not grant. The first caller gets a runtime ACL denial, not a compile error — ` +
        `that is F-467, and it is why the openVault wrapper was deleted rather than kept ` +
        `with a comment. Widening the grant is a threat-model decision, not a convenience.`,
    ).toEqual([]);
  });

  it("leaves no granted command without a caller", () => {
    const unused = grantedCommands().filter((command) => !invokedCommands().includes(command));
    expect(
      unused,
      `the capability grants ${unused.join(", ")} and nothing in app/src/ipc/client.ts ` +
        `invokes it. Add the wrapper or drop the grant — an unused grant is a widening ` +
        `that has already happened with nobody having decided it, and it makes the ` +
        `capability's own description ("the five commands the front end actually invokes") ` +
        `false. If the front end really does reach this command another way, that route is ` +
        `outside the declared IPC surface and is the thing to fix (ADR-0012, F-467).`,
    ).toEqual([]);
  });

  it("names open_vault nowhere but the note explaining its absence", () => {
    // The specific instance F-467 filed, pinned so it cannot come back quietly as a
    // wrapper: the note above the wrappers may discuss it, an invoke may not.
    expect(clientSource).not.toMatch(/invoke\(\s*"open_vault"/);
  });
});
