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
 * Evidence for `mark_first_paint` — task #108/#112, closing F-377/F-388/F-396
 * (`crates/serat-app`'s guard is what actually rejects implausible evidence; this
 * type only has to match its parameter names).
 *
 * **Field names are camelCase, not this file's usual snake_case.** Every other wire
 * shape here is a nested struct field (`WriteFileArgs.base_hash` and friends), which
 * ts-rs preserves verbatim from Rust and Tauri deserialises with no renaming. These
 * are **top-level command parameters**, and Tauri v2 renames a command's own
 * parameters to camelCase by default before matching them against the JS call —
 * confirmed against `crates/serat-app/src/commands.rs`'s actual Rust signature,
 * which takes `first_contentful_paint_ms: Option<f64>`, `raf1_ms: f64`, `raf2_ms: f64`
 * and is only reachable by sending `firstContentfulPaintMs`/`raf1Ms`/`raf2Ms`.
 * Sending the snake_case form silently fails to match the parameter — a bug this
 * file shipped for one revision before catching it against the real branch.
 */
export interface PaintEvidence {
  /**
   * `PerformancePaintTiming`'s `first-contentful-paint` entry, if the webview
   * recorded one — the browser's own record of when it painted actual content
   * (text, image, non-white canvas/SVG), which this app's `#app` div cannot produce
   * before Vue mounts. `null` when the entry is genuinely absent (no Paint Timing
   * support on this platform, or the call ran before the browser recorded one);
   * `crates/serat-app`'s guard takes this as `Option<f64>` precisely so that case is
   * a typed, recorded rejection rather than a raw IPC deserialisation failure that
   * produces no trace and no explanation (F-396(c)).
   */
  firstContentfulPaintMs: number | null;
  /**
   * The **first** `requestAnimationFrame` callback's own `DOMHighResTimeStamp`
   * argument. Checked against `firstContentfulPaintMs` on the Rust side
   * (`crates/serat-app`'s `validate_paint_evidence`): F-396(b) found that the paint
   * entry alone is sound only as long as `app/index.html` stays contentless, a
   * property nothing asserts and that lives in a different file from the guard that
   * depends on it — the frame check makes soundness independent of what
   * `index.html` happens to contain today.
   */
  raf1Ms: number;
  /** The second callback's timestamp — taken after the frame reached the compositor. */
  raf2Ms: number;
  /**
   * `PerformanceNavigationTiming.domContentLoadedEventEnd`, if the webview recorded
   * one. Fires on HTML parse completion, before any script (including Vue's mount)
   * has run — it is not paint evidence and plays no part in any guard. Kept because
   * it is the inner mark ADR-0014's open objection to vue-frontend-dev asked for,
   * splitting `S_shell` into WebView2's navigation cost and this bundle's mount
   * cost. `null` when the entry is absent. Diagnostic only — `crates/serat-app`
   * ignores this field entirely; it is not a declared parameter of `mark_first_paint`.
   */
  navDomContentLoadedMs: number | null;
}

/**
 * The paint mark (NFR-PRF-01). Call this from inside a double
 * `requestAnimationFrame` and nowhere else — see `src/startup.ts`. Returns
 * milliseconds since t0, which `serat-app` computed and is not re-derived here.
 *
 * Can reject: `crates/serat-app`'s guard refuses evidence that is absent, that is not
 * two distinct composited frames, or where the frames precede the paint entry by more
 * than clock-quantisation noise can explain.
 */
export function markFirstPaint(evidence: PaintEvidence): Promise<number> {
  return invoke("mark_first_paint", { ...evidence });
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
