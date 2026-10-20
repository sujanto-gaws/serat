//! The Tauri command surface: `contracts/ipc.rs`, and nothing else.
//!
//! Every function here is three lines and contains no decision. The rules live in
//! [`crate::state::AppState`], where they can be asserted without a running
//! application — which is the only way the boundary rules get tests that can fail.
//!
//! Two commands are not in the contract, and both are instrumentation rather than
//! product: [`mark_first_paint`] and [`mark_vault_usable`] are the two marks
//! NFR-PRF-01 is measured on (see [`crate::startup`]). They are commands because the
//! only thing that knows a frame has been composited is the webview, and the only way
//! it can say so is to call one.

use std::sync::Arc;

use tauri::State;

use crate::ipc::{
    Backlink, CreateFolderArgs, FileContents, FileWritten, FolderCreated, IndexRead, IndexStatus,
    IpcError, NotePathArgs, OpenVaultArgs, ReadFileArgs, RefusedName, SearchArgs, SearchOutcome,
    SetKeybindingArgs, SetLivePreviewArgs, UiSettings, VaultOpened, WriteFileArgs,
};
use crate::startup;
use crate::state::AppState;

/// `open_vault(args) -> Result<VaultOpened, IpcError>`
///
/// # Errors
///
/// As [`AppState::open_vault`].
#[tauri::command]
pub fn open_vault(
    state: State<'_, Arc<AppState>>,
    args: OpenVaultArgs,
) -> Result<VaultOpened, IpcError> {
    state.open_vault(args)
}

/// `read_file(args) -> Result<FileContents, IpcError>`
///
/// # Errors
///
/// As [`AppState::read_file`].
#[tauri::command]
pub fn read_file(
    state: State<'_, Arc<AppState>>,
    args: ReadFileArgs,
) -> Result<FileContents, IpcError> {
    state.read_file(args)
}

/// `write_file(args) -> Result<FileWritten, IpcError>`
///
/// # Errors
///
/// As [`AppState::write_file`].
#[tauri::command]
pub fn write_file(
    state: State<'_, Arc<AppState>>,
    args: WriteFileArgs,
) -> Result<FileWritten, IpcError> {
    state.write_file(args)
}

/// `index_status() -> IndexStatus`
///
/// Never blocks and never touches SQLite.
#[tauri::command]
#[must_use]
pub fn index_status(state: State<'_, Arc<AppState>>) -> IndexStatus {
    state.index_status()
}

/// `get_ui_settings() -> UiSettings`
#[tauri::command(async)]
#[must_use]
pub fn get_ui_settings(state: State<'_, Arc<AppState>>) -> UiSettings {
    state.get_ui_settings()
}

/// `set_live_preview(args) -> Result<(), IpcError>`
///
/// # Errors
///
/// As [`AppState::set_live_preview`].
#[tauri::command(async)]
pub fn set_live_preview(
    state: State<'_, Arc<AppState>>,
    args: SetLivePreviewArgs,
) -> Result<(), IpcError> {
    state.set_live_preview(args)
}

/// `set_keybinding(args) -> Result<(), IpcError>`
///
/// # Errors
///
/// As [`AppState::set_keybinding`].
#[tauri::command(async)]
pub fn set_keybinding(
    state: State<'_, Arc<AppState>>,
    args: SetKeybindingArgs,
) -> Result<(), IpcError> {
    state.set_keybinding(args)
}

/// `search(args) -> Result<IndexRead<SearchOutcome>, IpcError>`
///
/// Never waits for the index. The read itself runs in `spawn_blocking`: serat-index's
/// search connection has one lock (a newer search interrupts the older, which returns
/// `SearchOutcome::Superseded`, passed through unchanged), and waiting for it blocks a
/// thread, which must never be a Tauri async worker or the webview main thread.
///
/// # Errors
///
/// As [`AppState::search`]; a panicked read thread is reported as [`IpcError::Io`].
#[tauri::command]
pub async fn search(
    state: State<'_, Arc<AppState>>,
    args: SearchArgs,
) -> Result<IndexRead<SearchOutcome>, IpcError> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || state.search(args))
        .await
        .map_err(|err| IpcError::Io {
            path: String::new(),
            message: format!("the search thread failed: {err}"),
        })?
}

/// `backlinks(args) -> Result<IndexRead<Vec<Backlink>>, IpcError>`
///
/// # Errors
///
/// As [`AppState::backlinks`].
#[tauri::command(async)]
pub fn backlinks(
    state: State<'_, Arc<AppState>>,
    args: NotePathArgs,
) -> Result<IndexRead<Vec<Backlink>>, IpcError> {
    state.backlinks(args)
}

/// `create_folder(args) -> Result<FolderCreated, IpcError>`
///
/// # Errors
///
/// As [`AppState::create_folder`].
#[tauri::command(async)]
pub fn create_folder(
    state: State<'_, Arc<AppState>>,
    args: CreateFolderArgs,
) -> Result<FolderCreated, IpcError> {
    state.create_folder(args)
}

/// `refused_names() -> Vec<RefusedName>`
///
/// FR-VLT-01. A read of the refusal log, not a walk; empty with no vault open. Runs
/// off the main thread (`async`) like every command that touches the vault.
#[tauri::command(async)]
#[must_use]
pub fn refused_names(state: State<'_, Arc<AppState>>) -> Vec<RefusedName> {
    state.refused_names()
}

/// The webview has composited its first frame.
///
/// `first_contentful_paint_ms` must be the `startTime` of the browser's own
/// `first-contentful-paint` `PerformancePaintTiming` entry — the Paint Timing API does
/// not report one until content has actually been painted, which is what makes this
/// argument evidence rather than a timestamp we simply trust (F-377; see
/// [`crate::startup`] and `app/src/startup.ts`). Returns the mark in milliseconds since
/// t0, on this process's own clock, so the shell can show its own number.
///
/// **This is the only way `t0_to_window_visible_ms` is ever set.** Nothing in Rust
/// stamps it, and this command refuses to stamp it from anything but genuine evidence.
///
/// Task #112 (F-396) adds two things to what task #105 built. `first_contentful_paint_ms`
/// is now `Option` because a webview with no Paint Timing support genuinely has nothing
/// to send — that case must be a *typed, recorded* rejection a reader can tell apart
/// from "the guard was never reached", not a raw IPC deserialisation failure against a
/// mandatory `f64` that produces no trace and no explanation at all. And
/// `raf1_ms`/`raf2_ms` — the two `requestAnimationFrame` callbacks' own timestamps,
/// already sent by `app/src/startup.ts` and previously ignored here — are now checked
/// against the paint entry: F-396(b) found that `first-contentful-paint` alone is sound
/// only as long as `app/index.html` stays contentless, a property nothing in this crate
/// asserts and that lives in a different file from this guard. See
/// [`startup::validate_paint_evidence`] for what the check actually verifies.
///
/// A one-line delegate to [`AppState::mark_first_paint`], like every other command
/// here (module doc) — F-397, task #113: the earlier version reached into
/// `state.startup` and called `validate_paint_evidence` itself, which put the
/// forwarding logic nowhere a test could drive it (`tauri::State` has no public
/// constructor outside the framework), and a mutation discarding the caller's
/// arguments for a hardcoded constant left every existing test green.
/// `AppState::mark_first_paint` is what a test now drives.
///
/// # Errors
///
/// As [`AppState::mark_first_paint`].
#[tauri::command]
pub fn mark_first_paint(
    state: State<'_, Arc<AppState>>,
    first_contentful_paint_ms: Option<f64>,
    raf1_ms: f64,
    raf2_ms: f64,
) -> Result<f64, startup::PaintEvidenceRejected> {
    state.mark_first_paint(first_contentful_paint_ms, raf1_ms, raf2_ms)
}

/// The file tree has been delivered: in P0, the last-open buffer is rendered and the
/// editor takes keystrokes.
///
/// This is **one of four** terms in "vault usable", not the whole of it. §3.4's binding
/// definition is its list of preconditions rather than its summary table row, and the
/// trace reports the latest of them — the shell having painted, the index open check
/// having finished, this, and the watcher having settled. A vault reported usable
/// before it is watched has a window in which an edit is silently missed; a vault
/// reported usable before the open check has finished cannot say what its index is
/// doing. Neither failure errors, which is why both are gates rather than notes.
///
/// `note` is what was rendered, or `None` if nothing was — which is recorded in the
/// trace so a fast number over an empty shell is legible as one.
#[tauri::command]
#[must_use]
pub fn mark_vault_usable(state: State<'_, Arc<AppState>>, note: Option<String>) -> f64 {
    state.startup.set_last_open(note.clone());
    state.startup.mark_buffer_rendered();
    if let Some(note) = note
        && let Err(problem) = state.remember_open(&note)
    {
        eprintln!("serat: settings not saved: {problem}");
    }
    state.startup.elapsed_ms()
}
