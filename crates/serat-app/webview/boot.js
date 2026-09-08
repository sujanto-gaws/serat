// The P0 shell's boot script, and the front-end half of the NFR-PRF-01 measurement.
//
// Two marks, and where they are taken is the whole point (see crates/serat-app/src/startup.rs):
//
//   mark_first_paint   from inside a double requestAnimationFrame. A single rAF
//                      callback runs *before* the frame is painted; the second one
//                      runs after it has been handed to the compositor, which is the
//                      earliest moment a browser can honestly say something was drawn.
//                      Taking this at script start, or in Rust at window creation,
//                      would pass 400 ms over a blank rectangle — which is exactly the
//                      failure test-engineer asked us to make impossible.
//
//   mark_vault_usable  once the last-open buffer is in the editor. The editor accepts
//                      keystrokes before this, because it is in the static markup.
//
// Nothing here is a product feature. Task S2 replaces this file; it must keep both
// marks, and crates/serat-app/src/lib.rs has a test that fails if it does not.

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
// A single rAF callback runs *before* the frame is painted; the second runs after it
// has gone to the compositor. Nothing else may call this.
const painted = new Promise((resolve) => {
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      invoke("mark_first_paint").then(
        (ms) => {
          marksLabel.textContent = `painted ${ms.toFixed(1)} ms`;
          resolve(ms);
        },
        (err) => {
          // Never silently swallowed: an unhandled rejection here would look
          // identical to a shell that painted nothing.
          marksLabel.textContent = `paint mark failed: ${JSON.stringify(err)}`;
          resolve(null);
        },
      );
    });
  });
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
