// The P0 shell's boot script, and the front-end half of the NFR-PRF-01 measurement.
//
// Two marks, and where they are taken is the whole point (see crates/serat-app/src/startup.rs):
//
//   mark_first_paint   carries evidence, not just a call. F-377: a rAF-counting
//                      version of this mark passed a substring test under two
//                      mutations — collapsing a double requestAnimationFrame to one,
//                      and hoisting the invoke() call to script start with a no-op
//                      rAF left behind — because rAF depth is something a script
//                      controls regardless of whether anything was ever composited.
//                      This version instead waits for the browser's own
//                      `first-contentful-paint` PerformancePaintTiming entry, which
//                      the Paint Timing API cannot report before the frame has
//                      actually reached the compositor, and sends its `startTime` as
//                      the command's argument. `mark_first_paint` (Rust side) rejects
//                      anything that is not a positive, finite number — a call with
//                      no real entry to read has nothing to send, and nothing is
//                      accepted in its place. If the platform does not support the
//                      Paint Timing API at all, the same rule applies: no fabricated
//                      substitute, no mark, the trace is honestly incomplete rather
//                      than fast.
//
//   mark_vault_usable  once the last-open buffer is in the editor. The editor accepts
//                      keystrokes before this, because it is in the static markup.
//
// Nothing here is a product feature. Task S2 replaces this file; it must keep both
// marks — mark_first_paint carrying real Paint Timing evidence, not a rAF count — and
// crates/serat-app/src/lib.rs has a test that fails if it does not.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const boot = window.__SERAT_BOOT__ || { vault: null, open: null, exit_after_ready: false };

const editor = document.getElementById("editor");
const vaultLabel = document.getElementById("vault");
const indexLabel = document.getElementById("index");
const marksLabel = document.getElementById("marks");

/** The hash the buffer was read at — the optimistic lock of architecture §6.1. */
let baseHash = null;
/** The note the buffer holds, vault-relative. */
let openNote = null;

function describeIndex(state) {
  switch (state.kind) {
    case "opening":
      return "index: opening";
    case "ready":
      return `index: ready · ${state.note_count} notes`;
    case "rebuilding":
      return state.walk_complete
        ? `Rebuilding index (${state.reason}) — search and queries are incomplete until this finishes · ${state.done}/${state.total}`
        : `Rebuilding index (${state.reason}) — search and queries are incomplete until this finishes · ${state.done}`;
    case "unavailable":
      return `index: unavailable — ${state.message}`;
    default:
      return `index: ${state.kind}`;
  }
}

// A rebuild is a status line, not a modal and not an error (architecture §3.4).
listen("index://status", (event) => {
  indexLabel.textContent = describeIndex(event.payload.state);
});

if (boot.vault) {
  vaultLabel.textContent = boot.vault;
}

// The paint mark. Scheduled before the file read is awaited, so a slow disk cannot
// delay it — the two are independent, which is what §3.4's second row describes.
//
// Gated on the browser's own `first-contentful-paint` entry, not on how many
// requestAnimationFrame callbacks ran (F-377): rAF depth is under this script's
// control and proves nothing about whether anything was composited, but the Paint
// Timing API cannot report the entry before it was. Nothing else may call
// mark_first_paint, and nothing here may send it anything but that entry's
// `startTime`.
function firstContentfulPaintEntry() {
  return performance
    .getEntriesByType("paint")
    .find((entry) => entry.name === "first-contentful-paint");
}

const painted = new Promise((resolve) => {
  function reportFailure(detail) {
    // Never silently swallowed: an unhandled failure here would look identical to a
    // shell that painted nothing. The shell itself still works — the buffer already
    // read and rendered independently of this mark (design goal 5) — only the
    // cold-start measurement is unavailable, which is the honest outcome of nothing
    // real to send rather than a fabricated one.
    marksLabel.textContent = `paint mark unavailable: ${detail}`;
    resolve(null);
  }

  function sendEvidence(entry) {
    invoke("mark_first_paint", { firstContentfulPaintMs: entry.startTime }).then(
      (ms) => {
        marksLabel.textContent = `painted ${ms.toFixed(1)} ms`;
        resolve(ms);
      },
      (err) => reportFailure(JSON.stringify(err)),
    );
  }

  if (
    typeof PerformanceObserver === "undefined" ||
    !(PerformanceObserver.supportedEntryTypes || []).includes("paint")
  ) {
    // Missing evidence is a rejection, never a default: this platform has nothing
    // this script could honestly call evidence, so nothing is sent.
    reportFailure("PerformancePaintTiming is not supported on this platform");
    return;
  }

  // The entry may already be in the performance timeline by the time this script
  // runs `buffered: true` catches that; the observer catches it otherwise.
  const existing = firstContentfulPaintEntry();
  if (existing) {
    sendEvidence(existing);
    return;
  }

  const observer = new PerformanceObserver((list) => {
    const entry = list
      .getEntriesByType("paint")
      .find((candidate) => candidate.name === "first-contentful-paint");
    if (entry) {
      observer.disconnect();
      sendEvidence(entry);
    }
  });
  observer.observe({ type: "paint", buffered: true });
});

async function openBuffer() {
  if (!boot.open) {
    // Nothing to restore. Still usable: the editor takes keystrokes and a new note can
    // be written. The trace records last_open as null so the number is legible as one
    // taken over an empty buffer.
    await painted;
    await invoke("mark_vault_usable", { note: null });
    return;
  }

  try {
    const file = await invoke("read_file", { args: { path: boot.open } });
    editor.value = file.text;
    baseHash = file.hash;
    openNote = file.path;
  } catch (err) {
    // A refused read is an explanation, not a modal (§9.1).
    editor.value = `${boot.open} could not be read: ${JSON.stringify(err)}`;
  }

  // Wait for the paint mark to have landed before reporting usable. Both are round
  // trips and the read can finish first; the shell then exits with the usable mark
  // taken and the paint mark still in flight, and the run reports no trace at all.
  // Measured, not theorised — it is what the first smoke run of this shell did.
  // The Rust side enforces the ordering too (`Marks::vault_usable`); this keeps the
  // reported numbers honest rather than merely ordered.
  await painted;
  const ms = await invoke("mark_vault_usable", { note: openNote });
  marksLabel.textContent = `${marksLabel.textContent} · usable ${ms.toFixed(1)} ms`;
}

// Ctrl/Cmd-S writes the buffer back, optimistically locked on the hash it was read at.
// A stale hash resolves in place: no modal on the write path (§9.1).
editor.addEventListener("keydown", async (event) => {
  if (!((event.ctrlKey || event.metaKey) && event.key === "s")) {
    return;
  }
  event.preventDefault();
  if (!openNote) {
    return;
  }
  try {
    const written = await invoke("write_file", {
      args: { path: openNote, text: editor.value, base_hash: baseHash },
    });
    baseHash = written.hash;
    marksLabel.textContent = `saved ${written.bytes} bytes`;
  } catch (err) {
    if (err && err.kind === "stale_hash") {
      marksLabel.textContent = "changed on disk since it was read — reopen to merge";
    } else {
      marksLabel.textContent = `not saved: ${JSON.stringify(err)}`;
    }
  }
});

// Late subscribers catch up by polling; the contract has both channels for this reason.
invoke("index_status").then((status) => {
  indexLabel.textContent = describeIndex(status.state);
});

openBuffer();
