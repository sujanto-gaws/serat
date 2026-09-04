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
    FileContents, FileWritten, IndexStatus, IpcError, OpenVaultArgs, ReadFileArgs, VaultOpened,
    WriteFileArgs,
};
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

/// The webview has composited its first frame.
///
/// Called from inside a double `requestAnimationFrame`, which is the earliest point at
/// which the browser can honestly say a frame went to the compositor. Returns the mark
/// in milliseconds since t0 so the shell can show its own number.
///
/// **This is the only way `t0_to_window_visible_ms` is ever set.** Nothing in Rust
/// stamps it; see [`crate::startup`] for why that matters.
#[tauri::command]
#[must_use]
pub fn mark_first_paint(state: State<'_, Arc<AppState>>) -> f64 {
    state.startup.mark_first_paint();
    state.startup.elapsed_ms()
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
