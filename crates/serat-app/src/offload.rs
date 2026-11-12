//! Vault-byte commands, off the webview's thread (task #49; NFR-PRF-02, NFR-PRF-03,
//! NFR-REL-01, design goal 2).
//!
//! A sync `#[tauri::command]` runs on the main thread, and a sync `#[tauri::command(async)]`
//! runs on an async-runtime worker that the rest of IPC dispatch shares. Neither is a place
//! to read or write a note, hash it, wait on a path lock or sit in the 2 s flush wait of a
//! vault switch. Every command that does goes through [`blocking`], which hands the work to
//! the runtime's blocking pool and awaits it, so the caller's thread is free the moment the
//! work is queued.
//!
//! # What this does not change
//!
//! The work is the same `AppState` method the command called before, so every ordering rule
//! lives where it did and is not restated here:
//!
//! * **Saves for one note stay ordered by the hash chain, not by a thread.** Each save names
//!   the hash it is based on; `serat-vault` applies it under the per-path lock and refuses a
//!   stale base (ADR-0016 §2). Two saves racing on one base cannot both land, and the editor
//!   keeps at most one in flight per note. Moving the work between threads cannot reorder a
//!   chain that never relied on thread identity.
//! * **`FileWritten` pairs the hash with the mtime of the same write** (ADR-0015 §2): both come
//!   from `write_atomic_if` / `write_patch` under the path lock, inside one blocking job.
//! * **`flush_ack` stays a sync command** (a mutex and a condvar notify, never I/O) and is the
//!   only thing that ends the flush wait; the switch waits on the blocking pool now, so a
//!   thread that must deliver `flush_ack` is never the one waiting for it.
//! * **The asset validator and the read use one handle**: `asset_info` and `read_asset_range`
//!   clone the `Arc<Vault>` once and run both steps inside one blocking job.
//!
//! Nothing here is domain logic: each method is `blocking(|| self.x(args))`.

use std::sync::Arc;

use crate::ipc::{
    AssetArgs, AssetInfo, ExternalChange, FileContents, FileWritten, IpcError, NoteBufferArgs,
    OpenVaultArgs, Patch, ReadAssetRangeArgs, ReadFileArgs, RebaseBufferArgs, SavePatchArgs,
    VaultOpened, WriteFileArgs,
};
use crate::state::AppState;

/// Runs `work` on the blocking pool and awaits it. A panic in `work` is an
/// [`IpcError::Io`], not a crashed command.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, IpcError> + Send + 'static,
) -> Result<T, IpcError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| IpcError::Io {
            path: String::new(),
            message: err.to_string(),
        })?
}

impl AppState {
    /// [`AppState::open_vault`] on the blocking pool: it closes the previous vault, which
    /// waits up to 2 s for the editor's `flush_ack` (NFR-REL-01).
    ///
    /// # Errors
    ///
    /// As [`AppState::open_vault`].
    pub async fn open_vault_off_thread(
        self: &Arc<Self>,
        args: OpenVaultArgs,
    ) -> Result<VaultOpened, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.open_vault(args)).await
    }

    /// [`AppState::read_file`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::read_file`].
    pub async fn read_file_off_thread(
        self: &Arc<Self>,
        args: ReadFileArgs,
    ) -> Result<FileContents, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.read_file(args)).await
    }

    /// [`AppState::write_file`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::write_file`].
    pub async fn write_file_off_thread(
        self: &Arc<Self>,
        args: WriteFileArgs,
    ) -> Result<FileWritten, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.write_file(args)).await
    }

    /// [`AppState::note_opened`] on the blocking pool (it reads the note).
    ///
    /// # Errors
    ///
    /// As [`AppState::note_opened`].
    pub async fn note_opened_off_thread(
        self: &Arc<Self>,
        args: NoteBufferArgs,
    ) -> Result<(), IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.note_opened(&args)).await
    }

    /// [`AppState::save_patch`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::save_patch`].
    pub async fn save_patch_off_thread(
        self: &Arc<Self>,
        args: SavePatchArgs,
    ) -> Result<FileWritten, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.save_patch(&args)).await
    }

    /// [`AppState::request_external_diff`] on the blocking pool (it reads the note).
    ///
    /// # Errors
    ///
    /// As [`AppState::request_external_diff`].
    pub async fn request_external_diff_off_thread(
        self: &Arc<Self>,
        args: NoteBufferArgs,
    ) -> Result<Option<ExternalChange>, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.request_external_diff(&args)).await
    }

    /// [`AppState::rebase_buffer`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::rebase_buffer`].
    pub async fn rebase_buffer_off_thread(
        self: &Arc<Self>,
        args: RebaseBufferArgs,
    ) -> Result<Vec<Patch>, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.rebase_buffer(&args)).await
    }

    /// [`AppState::asset_info`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::asset_info`].
    pub async fn asset_info_off_thread(
        self: &Arc<Self>,
        args: AssetArgs,
    ) -> Result<AssetInfo, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.asset_info(args)).await
    }

    /// [`AppState::read_asset_range`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::read_asset_range`].
    pub async fn read_asset_range_off_thread(
        self: &Arc<Self>,
        args: ReadAssetRangeArgs,
    ) -> Result<Vec<u8>, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.read_asset_range(args)).await
    }
}

#[cfg(test)]
mod tests;
