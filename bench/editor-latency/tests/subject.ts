// The seam — the only module in this harness that needs to know how to
// reach a running Serat app. Mirrors the convention in
// `bench/budget/src/subject.rs`: everything measured enters through one
// function here, so when its blockers land, only this file's body changes
// and no measurement code in the spec moves.
//
// **Deliberately not wired today.** Two things this harness cannot decide
// for itself, both pending the architect's editor-integration design
// (P1 task #5) and its implementation (task #21):
//
//   1. How the app under test is launched and reached — a Tauri webview has
//      no plain HTTP origin Playwright can navigate `page.goto()` to by
//      default; whether the harness drives a `vite dev` server standing in
//      for the webview, or attaches to a real Tauri window over CDP, is a
//      call the design makes, not this harness.
//   2. How a specific note is opened deterministically — a URL, an IPC
//      command from the test driver, or a fixture vault path passed at
//      launch. FR-NAV-01/06 give candidates (quick switcher, command
//      palette) but which one is stable enough to drive from a test is a
//      design decision, not a test-engineering one.
//
// Until then, `openNoteForLatencyTest` throws with the task numbers that
// unblock it, the same way `bench/budget`'s subjects failed loudly before
// I3–I6 landed, rather than silently measuring nothing.
import type { Page } from "@playwright/test";

/** Serves NFR-PRF-02. Opens a 5,000-line note and returns a locator for the
 * editable surface the harness will type into. Not implemented — see the
 * module doc comment above. */
export async function openNoteForLatencyTest(_page: Page): Promise<never> {
  throw new Error(
    "editor-latency subject not wired: no app entry point and no " +
      "5,000-line fixture note exist yet. Blocked on P1 task #5 " +
      "(architect: editor integration design) and task #21 " +
      "(vue-frontend-dev: hybrid live-preview editor). " +
      "See bench/editor-latency/README.md.",
  );
}

/** The fixture this harness needs once #5/#21 land: exactly 5,000 lines,
 * per NFR-PRF-02's acceptance criterion ("Benchmark harness reports p95
 * under 16ms" in a 5,000-line note) — not approximately 5,000, because the
 * budget is stated against that exact size and a fixture that drifts makes
 * the number incomparable across runs. Generation belongs beside
 * `bench/vaultgen`'s existing deterministic generator (NFR-MNT-02) rather
 * than being hand-authored here. */
export const FIXTURE_NOTE_LINE_COUNT = 5000;
