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
//! * **A job finds the vault when it starts and carries no vault id.** A command queued
//!   before a switch may therefore run against the vault that is open when the job starts,
//!   not the one open when the command was sent. The front end filters *events* by
//!   `vault_id` (`CodeMirrorEditor.vue`, `App.vue`), but a command's answer carries no id
//!   and is not filtered. That residual is inherited: the `async` commands this replaces
//!   resolved the vault when they ran in just the same way.
//!
//! # Writers and the settings file
//!
//! Every command that writes (`create_folder`, `append_gitignore_entry`,
//! `decline_gitignore_offer`, `set_live_preview`, `set_keybinding`, the settings write inside
//! `mark_vault_usable`) goes through [`blocking`] too.
//!
//! # Reads routed here on evidence (task #133)
//!
//! `backlinks` opens a SQLite read connection and queries it (file I/O), and
//! `get_ui_settings` takes the settings lock, which `update_settings` holds across the
//! settings-file write, so it can wait on disk. Both go through [`blocking`].
//!
//! # Left on a Tokio worker, and why
//!
//! These stay sync `command(async)` functions because nothing on their path does file I/O
//! or waits on a lock that is held across any:
//!
//! * `note_closed`: one `Buffers` map operation under its leaf mutex (no I/O, no other lock).
//! * `refused_names`: clones at most 1,000 entries from the refusal log under its leaf mutex.
//! * `open_tree`: takes short leaf mutexes (feed state, marks) and spawns the `serat-tree`
//!   thread; the walk and every send happen on that thread, never in the command.
//!
//! The vault-open lock they all take is held exclusively only for a `replace` or `take`;
//! the close's flush and join run after it is released.
//!
//! Nothing here is domain logic: each method is `blocking(|| self.x(args))`.

use std::sync::Arc;

use crate::ipc::{
    AssetArgs, AssetInfo, Backlink, CreateFolderArgs, ExternalChange, FileContents, FileWritten,
    FolderCreated, GitignoreAppended, GitignoreOffer, IndexRead, IpcError, NoteBufferArgs,
    NotePathArgs, OpenVaultArgs, Patch, ReadAssetRangeArgs, ReadFileArgs, RebaseBufferArgs,
    SavePatchArgs, SetKeybindingArgs, SetLivePreviewArgs, UiSettings, VaultOpened, WriteFileArgs,
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

    /// [`AppState::create_folder`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::create_folder`].
    pub async fn create_folder_off_thread(
        self: &Arc<Self>,
        args: CreateFolderArgs,
    ) -> Result<FolderCreated, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.create_folder(args)).await
    }

    /// [`AppState::append_gitignore_entry`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::append_gitignore_entry`].
    pub async fn append_gitignore_entry_off_thread(
        self: &Arc<Self>,
    ) -> Result<GitignoreAppended, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.append_gitignore_entry()).await
    }

    /// [`AppState::decline_gitignore_offer`] on the blocking pool.
    ///
    /// # Errors
    ///
    /// As [`AppState::decline_gitignore_offer`].
    pub async fn decline_gitignore_offer_off_thread(self: &Arc<Self>) -> Result<(), IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.decline_gitignore_offer()).await
    }

    /// [`AppState::set_live_preview`] on the blocking pool (it writes the settings file).
    ///
    /// # Errors
    ///
    /// As [`AppState::set_live_preview`].
    pub async fn set_live_preview_off_thread(
        self: &Arc<Self>,
        args: SetLivePreviewArgs,
    ) -> Result<(), IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.set_live_preview(args)).await
    }

    /// [`AppState::set_keybinding`] on the blocking pool (it writes the settings file).
    ///
    /// # Errors
    ///
    /// As [`AppState::set_keybinding`].
    pub async fn set_keybinding_off_thread(
        self: &Arc<Self>,
        args: SetKeybindingArgs,
    ) -> Result<(), IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.set_keybinding(args)).await
    }

    /// [`AppState::backlinks`] on the blocking pool: it opens a SQLite read connection and
    /// queries it.
    ///
    /// # Errors
    ///
    /// As [`AppState::backlinks`].
    pub async fn backlinks_off_thread(
        self: &Arc<Self>,
        args: NotePathArgs,
    ) -> Result<IndexRead<Vec<Backlink>>, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.backlinks(args)).await
    }

    /// [`AppState::get_ui_settings`] on the blocking pool: the settings lock is held across
    /// the settings-file write by `update_settings`, so a read can wait on disk.
    ///
    /// # Errors
    ///
    /// Only a panicked job ([`IpcError::Io`]); the read itself cannot fail.
    pub async fn get_ui_settings_off_thread(self: &Arc<Self>) -> Result<UiSettings, IpcError> {
        let state = Arc::clone(self);
        blocking(move || Ok(state.get_ui_settings())).await
    }

    /// [`AppState::gitignore_offer_status`] on the blocking pool (it stats `.git` and reads
    /// `.gitignore` and the declined marker).
    ///
    /// # Errors
    ///
    /// As [`AppState::gitignore_offer_status`].
    pub async fn gitignore_offer_status_off_thread(
        self: &Arc<Self>,
    ) -> Result<GitignoreOffer, IpcError> {
        let state = Arc::clone(self);
        blocking(move || state.gitignore_offer_status()).await
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

#[cfg(test)]
mod reads_tests;
