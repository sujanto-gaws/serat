// The typed IPC boundary. Every function here is a thin wrapper over
// `@tauri-apps/api`'s `invoke`/`listen`, typed against the generated bindings in
// `./generated` (ADR-0007). Nothing here is hand-written wire shape — only the
// command name strings, which must match `crates/serat-app/src/commands.rs`
// (`#[tauri::command]` uses the function name) and `contracts/ipc.rs`
// (`INDEX_STATUS_EVENT`) exactly. If either drifts, the round trip fails at
// runtime, not at compile time — there is no generated command list yet, only
// generated argument/return types.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { assetUrl as buildAssetUrl } from "../editor/assetUrl";
import type { AssetArgs } from "./generated/AssetArgs";
import type { AssetInfo } from "./generated/AssetInfo";
import type { ReadAssetRangeArgs } from "./generated/ReadAssetRangeArgs";
import type { Backlink } from "./generated/Backlink";
import type { ExternalChange } from "./generated/ExternalChange";
import type { AppClosing } from "./generated/AppClosing";
import type { FlushAckArgs } from "./generated/FlushAckArgs";
import type { IndexChanged } from "./generated/IndexChanged";
import type { VaultOpened } from "./generated/VaultOpened";
import type { MarkTreeArgs } from "./generated/MarkTreeArgs";
import type { NoteBufferArgs } from "./generated/NoteBufferArgs";
import type { Patch } from "./generated/Patch";
import type { RebaseBufferArgs } from "./generated/RebaseBufferArgs";
import type { SavePatchArgs } from "./generated/SavePatchArgs";
import type { SwitcherQueryArgs } from "./generated/SwitcherQueryArgs";
import type { SwitcherResults } from "./generated/SwitcherResults";
import type { TreeMessage } from "./generated/TreeMessage";
import type { CreateFolderArgs } from "./generated/CreateFolderArgs";
import type { FolderCreated } from "./generated/FolderCreated";
import type { GitignoreAppended } from "./generated/GitignoreAppended";
import type { GitignoreOffer } from "./generated/GitignoreOffer";
import type { IndexRead } from "./generated/IndexRead";
import type { LinkResolution } from "./generated/LinkResolution";
import type { NoteAnchors } from "./generated/NoteAnchors";
import type { NotePathArgs } from "./generated/NotePathArgs";
import type { ResolveLinksArgs } from "./generated/ResolveLinksArgs";
import type { RefusedName } from "./generated/RefusedName";
import type { SearchArgs } from "./generated/SearchArgs";
import type { SearchOutcome } from "./generated/SearchOutcome";
import type { SetKeybindingArgs } from "./generated/SetKeybindingArgs";
import type { SetLivePreviewArgs } from "./generated/SetLivePreviewArgs";
import type { UiSettings } from "./generated/UiSettings";
import type { FileContents } from "./generated/FileContents";
import type { FileWritten } from "./generated/FileWritten";
import type { IndexStatus } from "./generated/IndexStatus";
import type { IpcError } from "./generated/IpcError";
import type { ReadFileArgs } from "./generated/ReadFileArgs";
import type { WriteFileArgs } from "./generated/WriteFileArgs";

export type { IpcError };

/** `contracts/ipc.rs::INDEX_STATUS_EVENT`. See that file for why it is not "/". */
export const INDEX_STATUS_EVENT = "index://status";

/** `contracts/ipc.rs::INDEX_CHANGED_EVENT`. */
export const INDEX_CHANGED_EVENT = "index://changed";

/** `contracts/ipc.rs::EXTERNAL_CHANGE_EVENT`. */
export const EXTERNAL_CHANGE_EVENT = "note://external-change";
/** `contracts/ipc.rs::APP_CLOSING_EVENT`. */
export const APP_CLOSING_EVENT = "app://closing";

// There is deliberately no `openVault` wrapper, and its absence is the point rather
// than an omission. `open_vault` exists as a command, but
// `crates/serat-app/capabilities/default.json` withholds it from the webview
// (ADR-0012): the vault is opened from Rust inside `setup`, and granting it here
// would reopen the surface that removing it was meant to close. A wrapper stood here
// until task #125 with nothing calling it — so the "webview never invokes
// `open_vault`" property that the capability's own justification rests on held by
// Rollup's tree-shaking rather than by anything anyone asserted, and the first caller
// would have got a runtime ACL denial where "there is no such function" was available
// (F-467). Deleted rather than annotated: a deprecation note still compiles, and this
// file's job is to make the boundary visible at authoring time.
//
// The generated `OpenVaultArgs`/`VaultOpened` types stay under ./generated — ts-rs
// emits every type in the contract, and a type existing is not a grant. What keeps
// the two sides equal is `client.test.ts`, which compares the command names invoked
// below against the capability file's own grant.

export function readFile(args: ReadFileArgs): Promise<FileContents> {
  return invoke("read_file", { args });
}

export function writeFile(args: WriteFileArgs): Promise<FileWritten> {
  return invoke("write_file", { args });
}

export function indexStatus(): Promise<IndexStatus> {
  return invoke("index_status");
}

// ---- P1 (task #18). Each wrapper is the command's name and its generated types and
// nothing else; the rules are in contracts/ipc.rs and crates/serat-app.

/** `refused_names`: the list behind `IndexStatus.refused_names` (FR-VLT-01). */
export function refusedNames(): Promise<RefusedName[]> {
  return invoke("refused_names");
}

/** `create_folder`: one directory, inside the vault (FR-VLT-11). */
export function createFolder(args: CreateFolderArgs): Promise<FolderCreated> {
  return invoke("create_folder", { args });
}

/** `search`: free text, phrases and at most one `[key:value]` (FR-NAV-02, FR-STR-03). */
export function search(args: SearchArgs): Promise<IndexRead<SearchOutcome>> {
  return invoke("search", { args });
}

/** `backlinks`: one entry per distinct (source note, source block) (FR-NAV-04). */
export function backlinks(args: NotePathArgs): Promise<IndexRead<Backlink[]>> {
  return invoke("backlinks", { args });
}

/** `get_ui_settings`: the front end's slice of the settings TOML (FR-PLT-05). */
export function getUiSettings(): Promise<UiSettings> {
  return invoke("get_ui_settings");
}

/** `set_live_preview`: `false` selects split preview (FR-EDT-10). */
export function setLivePreview(args: SetLivePreviewArgs): Promise<void> {
  return invoke("set_live_preview", { args });
}

/** `set_keybinding`: set, unbind or reset one command's binding (FR-PLT-05). */
export function setKeybinding(args: SetKeybindingArgs): Promise<void> {
  return invoke("set_keybinding", { args });
}

/** `gitignore_offer_status`: whether to show the `.serat/` ignore offer (NFR-CMP-04). */
export function gitignoreOfferStatus(): Promise<GitignoreOffer> {
  return invoke("gitignore_offer_status");
}

/** `append_gitignore_entry`: only from the offer's explicit confirmation. Takes no path. */
export function appendGitignoreEntry(): Promise<GitignoreAppended> {
  return invoke("append_gitignore_entry");
}

/** `decline_gitignore_offer`: "Don't ask again", kept in `.serat/prefs.json`. */
export function declineGitignoreOffer(): Promise<void> {
  return invoke("decline_gitignore_offer");
}

/**
 * `choose_vault`: shows the native folder dialog and opens what the user picks (US-01). The
 * webview sends no path. `null` is a cancelled dialog and changes nothing; a chosen folder
 * answers with the same `VaultOpened` that `vault://opened` carries.
 */
export function chooseVault(): Promise<VaultOpened | null> {
  return invoke("choose_vault");
}

/** `asset_info`: MIME, size and sidecar of a vault file (FR-BIN-01; ADR-0017). */
export function assetInfo(args: AssetArgs): Promise<AssetInfo> {
  return invoke("asset_info", { args });
}

/**
 * `read_asset_range`: ADR-0017 fallback only, for a webview that will not deliver a
 * custom-scheme 206. Raw bytes (an `ArrayBuffer`), at most 1,000 KiB per call.
 */
export function readAssetRange(args: ReadAssetRangeArgs): Promise<ArrayBuffer> {
  return invoke("read_asset_range", { args });
}

/**
 * URL of the `serat-asset` scheme for a vault path; the bytes path is not a command.
 *
 * `.`, `..` and empty segments are refused rather than encoded: `encodeURIComponent`
 * leaves dots alone, and a URL parser collapses `a/../b` (and `%2e%2e`) before the
 * handler ever sees the request, so such a path could never name the file asked for.
 * The native handler decodes exactly once and re-validates (ADR-0017 steps 3-4);
 * this is the front end refusing to build a URL it knows will be rewritten.
 */
export function assetUrl(path: string, windows: boolean): string {
  return buildAssetUrl(path, windows ? "windows" : "other"); // the one builder, also used by embeds and viewers
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

/** `contracts/ipc.rs::VAULT_OPENED_EVENT`. */
export const VAULT_OPENED_EVENT = "vault://opened";

/**
 * Subscribes to `vault://opened` (architecture 3.4, "Which vault an event belongs to"): every
 * open AFTER the launch vault announces its new `vault_id` here, before any event carrying it.
 * The emit can precede a failed open (`choose_vault` then answers `Err`).
 */
export function onVaultOpened(handler: (opened: VaultOpened) => void): Promise<UnlistenFn> {
  return listen<VaultOpened>(VAULT_OPENED_EVENT, (event) => handler(event.payload));
}

/** True when `err` is the shape `IpcError` promises — Tauri rejects with `unknown`. */
export function isIpcError(err: unknown): err is IpcError {
  return typeof err === "object" && err !== null && "kind" in err;
}

// ---- P1 (task #18c): the tree stream and the editor buffers (ADR-0015, ADR-0016).

/**
 * `open_tree(on_message: Channel<TreeMessage>)`: ordered and lossless, root listing
 * first. `onMessage` is called once per message, in arrival order. Calling it again
 * replaces the stream and re-sends the tree from the root.
 */
export async function openTree(onMessage: (message: TreeMessage) => void): Promise<void> {
  const channel = new Channel<TreeMessage>();
  channel.onmessage = onMessage;
  await invoke("open_tree", { onMessage: channel });
}

/** `mark_tree`: `tree_first_screen` / `tree_complete`, stamped on the Rust clock. */
export function markTree(args: MarkTreeArgs): Promise<void> {
  return invoke("mark_tree", { args });
}

/** `note_opened`: an editor pane holds this note at this base (never an embed or preview). */
export function noteOpened(args: NoteBufferArgs): Promise<void> {
  return invoke("note_opened", { args });
}

/** `note_closed`: release the base. */
export function noteClosed(args: NoteBufferArgs): Promise<void> {
  return invoke("note_closed", { args });
}

/** `save_patch`: the editor's only save, patches in base coordinates (ADR-0016 §2). */
export function savePatch(args: SavePatchArgs): Promise<FileWritten> {
  return invoke("save_patch", { args });
}

/** `request_external_diff`: the recovery path; `null` means the disk equals the base. */
export function requestExternalDiff(args: NoteBufferArgs): Promise<ExternalChange | null> {
  return invoke("request_external_diff", { args });
}

/** `rebase_buffer`: *Keep mine* — patches from the disk text to the buffer, in disk coordinates. */
export function rebaseBuffer(args: RebaseBufferArgs): Promise<Patch[]> {
  return invoke("rebase_buffer", { args });
}

/**
 * `flush_ack`: tell the shell everything pending has been submitted (or nothing was),
 * ending its bounded wait before the vault closes. Echo the `vault_id` of `app://closing`.
 */
export function flushAck(args: FlushAckArgs): Promise<void> {
  return invoke("flush_ack", { args });
}

/**
 * Subscribes to `app://closing` (NFR-REL-01): the window or vault is closing. Flush the
 * pending patch set with `savePatch` (still accepted until `deadline_ms`), then `flushAck`.
 */
export function onClosing(handler: (closing: AppClosing) => void): Promise<UnlistenFn> {
  return listen<AppClosing>(APP_CLOSING_EVENT, (event) => handler(event.payload));
}

/** Subscribes to `note://external-change` (unthrottled, not ordered against responses). */
export function onExternalChange(handler: (change: ExternalChange) => void): Promise<UnlistenFn> {
  return listen<ExternalChange>(EXTERNAL_CHANGE_EVENT, (event) => handler(event.payload));
}

// ---- P1 (task #18c): the quick switcher and index invalidation (architecture 9.2, 3.1).

/**
 * `switcher_query`: one call per keystroke (FR-NAV-01, NFR-PRF-06). Never rejects for
 * a reason of its own and never waits for the index: before it is `Ready` the answer
 * is built from the tree's paths (`corpus: "tree"`). A `#` switches to heading mode.
 * Drop any response whose `seq` is not the latest sent, and any with `discarded: true`
 * (the query was superseded while it waited and was not computed: its empty `items`
 * is not "no matches").
 */
export function switcherQuery(args: SwitcherQueryArgs): Promise<SwitcherResults> {
  return invoke("switcher_query", { args });
}

/**
 * `resolve_links`: one batched call per viewport change (FR-EDT-03, NFR-CMP-01), the
 * link adapter's own wrapper (`linkAdapter.ts`) is what the editor calls. Never waits
 * for the index (before it is `Ready` answers carry `via: "tree"`); rejects only with
 * `vault_not_found`.
 */
export function resolveLinks(args: ResolveLinksArgs): Promise<LinkResolution[]> {
  return invoke("resolve_links", { args });
}

/** `note_anchors`: the headings and block ids `[[note#` and `[[note^` complete against. */
export function noteAnchors(args: NotePathArgs): Promise<IndexRead<NoteAnchors>> {
  return invoke("note_anchors", { args });
}

/**
 * Subscribes to `index://changed`: throttled to ten per second, invalidation only.
 * Re-query what is open; never treat it as proof that a particular write was indexed.
 * Discard an event whose `vault_id` is not the current vault's.
 */
export function onIndexChanged(handler: (changed: IndexChanged) => void): Promise<UnlistenFn> {
  return listen<IndexChanged>(INDEX_CHANGED_EVENT, (event) => handler(event.payload));
}
