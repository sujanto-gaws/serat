// A thin, mockable adapter in front of the P1 editor-buffer commands
// (`contracts/ipc.rs`: `note_opened`, `note_closed`, `save_patch`,
// `request_external_diff`, `rebase_buffer`). `serat-app` does not implement
// these yet (task #18) — this module is the seam task #21's brief asks for:
// everything in `src/editor` talks to the vault through this interface, and
// nothing calls `@tauri-apps/api/core`'s `invoke` directly. Swapping the
// real implementation in, once #18 lands, is a change to
// `defaultEditorIpc` alone.
//
// Deliberately not exported as a singleton the rest of the module tree
// imports directly: `CodeMirrorEditor.vue` and `EditorBuffer` both take an
// `EditorIpcAdapter` as a constructor/prop argument, so tests (and the
// NFR-PRF-02 harness's stub, per architecture.md §9.3's "the stub answers
// each with latencies recorded from the real binary") can substitute their
// own without any module-level mocking.

import { invoke } from "@tauri-apps/api/core";

import type { ExternalChange } from "./generated/ExternalChange";
import type { FileWritten } from "./generated/FileWritten";
import type { NoteBufferArgs } from "./generated/NoteBufferArgs";
import type { Patch } from "./generated/Patch";
import type { RebaseBufferArgs } from "./generated/RebaseBufferArgs";
import type { SavePatchArgs } from "./generated/SavePatchArgs";

export interface EditorIpcAdapter {
  /** `note_opened(args) -> Result<(), IpcError>` (ADR-0016 §3). */
  noteOpened(args: NoteBufferArgs): Promise<void>;
  /** `note_closed(args) -> ()`. */
  noteClosed(args: NoteBufferArgs): Promise<void>;
  /** `save_patch(args) -> Result<FileWritten, IpcError>` (ADR-0016 §2). */
  savePatch(args: SavePatchArgs): Promise<FileWritten>;
  /** `request_external_diff(args) -> Result<ExternalChange | null, IpcError>` (ADR-0016 §2 rule 3/4). */
  requestExternalDiff(args: NoteBufferArgs): Promise<ExternalChange | null>;
  /** `rebase_buffer(args) -> Result<Patch[], IpcError>` (ADR-0016 §4, *Keep mine*). */
  rebaseBuffer(args: RebaseBufferArgs): Promise<Patch[]>;
}

/** The real adapter: one `invoke` per method, command names matching
 * `crates/serat-app/src/commands.rs` (see `src/ipc/client.ts`'s header note
 * on why that match is load-bearing and not compiler-checked). */
export const defaultEditorIpc: EditorIpcAdapter = {
  noteOpened(args) {
    return invoke("note_opened", { args });
  },
  noteClosed(args) {
    return invoke("note_closed", { args });
  },
  savePatch(args) {
    return invoke("save_patch", { args });
  },
  requestExternalDiff(args) {
    return invoke("request_external_diff", { args });
  },
  rebaseBuffer(args) {
    return invoke("rebase_buffer", { args });
  },
};
