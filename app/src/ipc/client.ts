// The typed IPC boundary. Every function here is a thin wrapper over
// `@tauri-apps/api`'s `invoke`/`listen`, typed against the generated bindings in
// `./generated` (ADR-0007). Nothing here is hand-written wire shape — only the
// command name strings, which must match `crates/serat-app/src/commands.rs`
// (`#[tauri::command]` uses the function name) and `contracts/ipc.rs`
// (`INDEX_STATUS_EVENT`) exactly. If either drifts, the round trip fails at
// runtime, not at compile time — there is no generated command list yet, only
// generated argument/return types.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { FileContents } from "./generated/FileContents";
import type { FileWritten } from "./generated/FileWritten";
import type { IndexStatus } from "./generated/IndexStatus";
import type { IpcError } from "./generated/IpcError";
import type { OpenVaultArgs } from "./generated/OpenVaultArgs";
import type { ReadFileArgs } from "./generated/ReadFileArgs";
import type { VaultOpened } from "./generated/VaultOpened";
import type { WriteFileArgs } from "./generated/WriteFileArgs";

export type { IpcError };

/** `contracts/ipc.rs::INDEX_STATUS_EVENT`. See that file for why it is not "/". */
export const INDEX_STATUS_EVENT = "index://status";

export function openVault(args: OpenVaultArgs): Promise<VaultOpened> {
  return invoke("open_vault", { args });
}

export function readFile(args: ReadFileArgs): Promise<FileContents> {
  return invoke("read_file", { args });
}

export function writeFile(args: WriteFileArgs): Promise<FileWritten> {
  return invoke("write_file", { args });
}

export function indexStatus(): Promise<IndexStatus> {
  return invoke("index_status");
}

/**
 * The paint mark (NFR-PRF-01). Call this from inside a double
 * `requestAnimationFrame` and nowhere else — see `src/startup.ts`. Returns
 * milliseconds since t0, which `serat-app` computed and is not re-derived here.
 */
export function markFirstPaint(): Promise<number> {
  return invoke("mark_first_paint");
}

/**
 * The vault-usable mark (NFR-PRF-01). `note` is what was rendered, or `null` if
 * nothing was — recorded by the Rust side so a fast number over an empty shell is
 * legible as one. See `src/startup.ts` for the ordering this must respect.
 */
export function markVaultUsable(note: string | null): Promise<number> {
  return invoke("mark_vault_usable", { note });
}

/**
 * Subscribes to `index://status`. A rebuild is a status line, not a modal and not
 * an error (architecture §3.4) — the caller decides how to render `IndexStatus`,
 * this only delivers it.
 */
export function onIndexStatus(handler: (status: IndexStatus) => void): Promise<UnlistenFn> {
  return listen<IndexStatus>(INDEX_STATUS_EVENT, (event) => handler(event.payload));
}

/** True when `err` is the shape `IpcError` promises — Tauri rejects with `unknown`. */
export function isIpcError(err: unknown): err is IpcError {
  return typeof err === "object" && err !== null && "kind" in err;
}
