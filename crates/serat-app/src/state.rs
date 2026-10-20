//! Application state, and the four commands' logic.
//!
//! The logic lives here rather than in `commands.rs` so that every rule in
//! `contracts/ipc.rs` can be asserted without a running Tauri application. The
//! `#[tauri::command]` functions next door are three lines each and contain no
//! decisions — which is the shape §2.1 asks for from `serat-app`: wiring, not domain
//! logic. Every capability below comes from `serat-vault` or `serat-index`.
//!
//! The five boundary rules of `contracts/ipc.rs`, and where each is enforced:
//!
//! | Rule | Here |
//! |---|---|
//! | 1 · no command blocks on the index | [`AppState::index_status`] reads a published value; [`AppState::read_file`] and [`AppState::write_file`] never mention the index at all |
//! | 2 · every path is a vault-relative `VaultPath` | [`AppState::resolve`], before any filesystem call |
//! | 3 · only `.md` is writable | [`AppState::write_file`], first |
//! | 4 · a write is optimistically locked on the file hash | [`AppState::write_file`], §6.1 step 2 |
//! | 5 · errors are a closed set | [`crate::error::from_vault_error`], an exhaustive match |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;

use serat_core::{ContentHash, VaultError, VaultPath};
use serat_parse::CommonMarkParser;
use serat_vault::Vault;

use crate::cli::Boot;
use crate::error::from_vault_error;
use crate::index::{
    IndexActor, IndexHandleSlot, IndexOpen, SeratIndexOpen, StatusPublisher, StatusSink,
    index_dir_for,
};
use crate::ipc::{
    Backlink, CreateFolderArgs, FileContents, FileWritten, FolderCreated, GitignoreAppended,
    GitignoreOffer, IndexRead, IndexState, IndexStatus, IpcError, KeybindingChange,
    KeybindingOverride, NotePathArgs, OpenVaultArgs, REFUSED_NAMES_CAP, ReadFileArgs, RefusedName,
    SearchArgs, SearchOutcome, SetKeybindingArgs, SetLivePreviewArgs, UiSettings, VaultOpened,
    WriteFileArgs,
};
use crate::settings::{MAX_BINDING_TEXT, Settings};
use crate::startup::{self, PaintEvidenceRejected, Startup, WatcherOutcome};
use crate::vault::{Stage, VaultThread};

/// Where `index://status` goes.
///
/// Installed once, after the Tauri application exists. Before that — and in every test —
/// emitting is a no-op, which is deliberate: the index actor starts publishing before
/// anything has subscribed, and a status nobody heard is not an error. The poll
/// (`index_status`) is what a front end that subscribed late uses to catch up, which is
/// exactly why the contract has both.
#[derive(Default)]
pub struct Events {
    sink: RwLock<Option<StatusFn>>,
}

/// The installed sink. Named because the type is long enough that clippy is right to
/// ask, and because `install` and the field should be describing the same thing.
type StatusFn = Box<dyn Fn(&IndexStatus) + Send + Sync>;

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

impl Events {
    /// Direct every future status at `sink`.
    pub fn install(&self, sink: impl Fn(&IndexStatus) + Send + Sync + 'static) {
        *self
            .sink
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }
}

impl StatusSink for Arc<Events> {
    fn emit(&self, status: &IndexStatus) {
        if let Some(sink) = self
            .sink
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(status);
        }
    }
}

/// The status publisher as this crate instantiates it.
pub type Publisher = StatusPublisher<Arc<Events>>;

/// How long the vault thread waits for the first painted frame before walking anyway.
///
/// A window that never painted is already a failed start; refusing to walk the vault as
/// well would turn one problem into two, and a user staring at a broken shell is better
/// served by an index that is at least being built.
const WALK_GATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One open vault and the two threads that serve it.
struct OpenVault {
    vault: Arc<Vault>,
    publisher: Arc<Publisher>,
    /// Kept so the actor outlives the vault it belongs to, and no longer.
    _index: IndexActor,
    /// Where `write_file` finds the writer to submit a save through, once the actor
    /// thread has produced one (task #90; see `index::IndexHandleSlot`'s doc comment
    /// for the window in which this is still empty).
    index_handle: IndexHandleSlot,
    /// The vault's one full-text search connection (§4.5 obligation 2).
    search: Arc<crate::reads::SearchSlot>,
    /// Dropping this stops the watcher and the walk.
    _walk: VaultThread,
}

/// Which of §3.1.1's three outcomes a `write_file` call's index submission took.
///
/// F-382: an empty [`IndexHandleSlot`] is indistinguishable, from `write_file`'s own
/// return value, between a legitimate startup race, an opener that never produces a
/// handle, and an open that failed -- all three fall back silently to the watcher path,
/// and a benchmark measuring "the real save path" (task #94) cannot tell that from a
/// genuine submission unless something counts them apart. This is F-19 one level up:
/// a fast p95 over saves that never reached the writer is exactly the flattering
/// number that finding is named for.
///
/// Snapshotted rather than streamed, because the three counters are process-lifetime
/// totals a caller diffs across a run (`before`/`after`), not an event a subscriber
/// needs told about -- there is no product decision that turns on one save's outcome,
/// only a harness's need to audit many of them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SavePathOutcomes {
    /// The slot had no handle yet (or ever) when `write_file` looked. Falls back to
    /// the watcher's own event for this file; nothing was recorded for echo
    /// suppression, so nothing is lost, only slower (design goal 5).
    pub handle_missing: u64,
    /// A handle was found and `IndexHandle::apply`'s submit was refused --
    /// ordinary backpressure (`IndexError::QueueFull`) or a writer already gone
    /// (`WriterStopped`). Also falls back to the watcher.
    pub writer_refused: u64,
    /// A handle was found and the submit was accepted. This is the count NFR-PRF-03
    /// actually governs.
    pub submitted: u64,
}

/// Everything the shell owns.
pub struct AppState {
    /// The cold-start timeline. Public because `main` and the window builder mark it.
    pub startup: Arc<Startup>,
    /// The event channel, installed once the Tauri application exists.
    pub events: Arc<Events>,
    /// What the process was asked to do.
    pub boot: Boot,
    settings: Mutex<Settings>,
    settings_path: Option<PathBuf>,
    open: RwLock<Option<OpenVault>>,
    vaults_opened: AtomicU64,
    /// Backing counters for [`AppState::save_path_outcomes`] (F-382). Process
    /// lifetime, not per-vault: a benchmark diffs two snapshots around its own run
    /// rather than needing them reset on `open_vault`.
    save_handle_missing: AtomicU64,
    save_writer_refused: AtomicU64,
    save_submitted: AtomicU64,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("boot", &self.boot)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Load settings and build the state. Runs on the main thread, first row of §3.4.
    ///
    /// Returns the state and anything that went wrong reading settings, which is a
    /// message to log rather than an error to handle: a broken settings file must not
    /// stop the window opening.
    #[must_use]
    pub fn new(startup: Arc<Startup>, boot: Boot) -> (Self, Option<String>) {
        let loaded = Settings::load();
        let state = Self {
            startup,
            events: Arc::new(Events::default()),
            boot,
            settings: Mutex::new(loaded.settings),
            settings_path: loaded.path,
            open: RwLock::new(None),
            vaults_opened: AtomicU64::new(0),
            save_handle_missing: AtomicU64::new(0),
            save_writer_refused: AtomicU64::new(0),
            save_submitted: AtomicU64::new(0),
        };
        (state, loaded.problem)
    }

    /// As [`AppState::new`], with settings supplied rather than loaded. For tests.
    #[must_use]
    pub fn with_settings(startup: Arc<Startup>, boot: Boot, settings: Settings) -> Self {
        Self {
            startup,
            events: Arc::new(Events::default()),
            boot,
            settings: Mutex::new(settings),
            settings_path: None,
            open: RwLock::new(None),
            vaults_opened: AtomicU64::new(0),
            save_handle_missing: AtomicU64::new(0),
            save_writer_refused: AtomicU64::new(0),
            save_submitted: AtomicU64::new(0),
        }
    }

    // -- commands -----------------------------------------------------------

    /// `open_vault`. FR-VLT-01, FR-VLT-02, NFR-PRF-01.
    ///
    /// Returns as soon as the root is canonicalised and readable. It does not wait for
    /// the walk, the index open, or a rebuild — all three report over
    /// `index://status`. That is the contract that keeps the window inside 400 ms
    /// (§3.4), and it is what `the_command_returns_without_waiting_for_the_index`
    /// asserts.
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] if the root does not exist, is not a directory, or
    /// cannot be read; [`IpcError::Io`] if a thread cannot be created.
    pub fn open_vault(&self, args: OpenVaultArgs) -> Result<VaultOpened, IpcError> {
        self.open_vault_with(args, SeratIndexOpen::default())
    }

    /// [`AppState::open_vault`] with the open check injected, so the actor's
    /// asynchrony can be tested against something slow enough to catch a caller that
    /// waits.
    ///
    /// # Errors
    ///
    /// As [`AppState::open_vault`].
    pub fn open_vault_with<O: IndexOpen>(
        &self,
        args: OpenVaultArgs,
        opener: O,
    ) -> Result<VaultOpened, IpcError> {
        let requested = PathBuf::from(&args.root);
        let vault = Arc::new(
            Vault::open(&requested).map_err(|_| IpcError::VaultNotFound { path: args.root })?,
        );
        let root = vault.root().to_path_buf();

        let vault_id = format!(
            "v{}",
            self.vaults_opened.fetch_add(1, Ordering::Relaxed) + 1
        );
        let publisher = Arc::new(
            Publisher::new(
                vault_id.clone(),
                Arc::clone(&self.startup),
                Arc::clone(&self.events),
            )
            // FR-VLT-01: a name the walk or the watcher refused is counted into every
            // status, so "N names not shown" is a fact of the status, not a second poll.
            .with_refusals(vault.refusals().clone()),
        );

        // §3.4's first `t_usable` precondition: the vault is not usable until the open
        // check has finished and the index state is *known*. The actor tells us by
        // publishing anything other than `Opening`; `Rebuilding` and `Unavailable`
        // settle it as much as `Ready` does.
        let settled = Arc::clone(&self.startup);
        publisher.on_publish(move |state| {
            if !matches!(state, IndexState::Opening) {
                settled.mark_index_settled();
            }
        });

        let (index, index_handle) = IndexActor::spawn(
            Arc::clone(&publisher),
            opener,
            root.clone(),
            index_dir_for(&root),
        )
        .map_err(|err| IpcError::Io {
            path: root.display().to_string(),
            message: format!("could not start the index thread: {err}"),
        })?;

        // §3.4 puts the walk off the critical path; measured, being on another thread
        // is not enough to achieve that (see `crate::vault`). The gate holds the walk
        // until the shell has painted — after the watcher is registered, so nothing
        // changing in the meantime is missed.
        let gate = Arc::clone(&self.startup);
        let startup = Arc::clone(&self.startup);
        let walked = Arc::clone(&publisher);
        let walked_refusals = vault.refusals().clone();
        let walk = VaultThread::spawn(
            Arc::clone(&vault),
            move || {
                gate.wait_for_first_paint(WALK_GATE_TIMEOUT);
            },
            move |stage| match stage {
                // Half of "vault usable": §3.4 will not call a vault usable while
                // nothing is watching it, because every edit made in that window is
                // lost silently.
                Stage::WatcherRegistered => startup.mark_watcher(WatcherOutcome::Registered),
                Stage::WatcherFailed => startup.mark_watcher(WatcherOutcome::Failed),
                Stage::WalkComplete(entries) => {
                    startup.mark_walk_complete(entries);
                    // The walk clears the log and refills it, so the count is final
                    // here and nothing else announces it (the index state does not
                    // change when a name is refused). Only when there is something to
                    // say: an ordinary vault emits nothing extra.
                    if !walked_refusals.is_empty() {
                        walked.re_emit();
                    }
                }
                Stage::WalkStarted | Stage::WalkFailed => {}
            },
        )
        .map_err(|err| IpcError::Io {
            path: root.display().to_string(),
            message: format!("could not start the vault thread: {err}"),
        })?;

        self.startup.set_vault(Some(root.display().to_string()));

        // Read the state before publishing the handle, so the value returned is the one
        // the actor had at this moment rather than one that raced it.
        let index_state = publisher.status().state;

        *self.open_lock_mut() = Some(OpenVault {
            vault,
            publisher,
            _index: index,
            index_handle,
            search: Arc::default(),
            _walk: walk,
        });

        Ok(VaultOpened {
            root: root.display().to_string(),
            vault_id,
            index: index_state,
        })
    }

    /// `read_file`. FR-VLT-02.
    ///
    /// Reads the filesystem, never the index: the file is the truth (design goal 1),
    /// and the index may be mid-rebuild or absent entirely.
    ///
    /// # Errors
    ///
    /// [`IpcError::PathEscapesVault`], [`IpcError::NotFound`], [`IpcError::NotUtf8`] for
    /// a file that is not valid UTF-8 — deliberately not a lossy read — or
    /// [`IpcError::Io`].
    pub fn read_file(&self, args: ReadFileArgs) -> Result<FileContents, IpcError> {
        let path = Self::vault_path(&args.path)?;
        let vault = self.vault()?;
        let loaded = vault.read(&path).map_err(from_vault_error)?;

        let text = String::from_utf8(loaded.bytes).map_err(|_| IpcError::NotUtf8 {
            path: path.to_string(),
        })?;

        Ok(FileContents {
            path: path.to_string(),
            text,
            hash: loaded.meta.hash.to_hex(),
            mtime_ms: loaded.meta.mtime_ms,
            bytes: loaded.meta.size,
        })
    }

    /// `write_file`. FR-VLT-02, FR-VLT-03.
    ///
    /// Rule 3 then rule 4 then the write, in that order: an extension check is free and
    /// a stale check costs a read, so refusing a `.pdf` must not first read it.
    ///
    /// # Errors
    ///
    /// [`IpcError::NotWritable`] for anything that is not `.md` (FR-BIN-19, threat
    /// T-06); [`IpcError::StaleHash`] when the optimistic lock fails, which is an
    /// expected outcome and not a fault (§6.1, §9.1); [`IpcError::NotFound`] when a
    /// caller holding a hash finds the file gone; otherwise as `serat-vault`.
    pub fn write_file(&self, args: WriteFileArgs) -> Result<FileWritten, IpcError> {
        let path = Self::vault_path(&args.path)?;

        // Rule 3, which defers to `serat_core::VaultPath::is_markdown()` — `.md` or
        // `.markdown` — rather than restating it. One definition of what a note is: a
        // `README.markdown` is walked, indexed and readable through `read_file`, so
        // refusing to save it would be a bug on the C-01 side rather than a guard on
        // the T-06 side. (code-reviewer's D3 finding; the contract now says this.)
        if !path.is_markdown() {
            return Err(IpcError::NotWritable {
                path: path.to_string(),
            });
        }

        // Rule 4's argument, checked before any I/O: a `base_hash` that is not 64 hex
        // digits is a client bug, and the contract gives it its own variant precisely
        // so it is not reported as a conflict with a writer who does not exist.
        let base_hash = args
            .base_hash
            .as_deref()
            .map(ContentHash::from_hex)
            .transpose()
            .map_err(from_vault_error)?;

        let vault = self.vault()?;

        // Rule 4 — the optimistic lock of §6.1 step 2.
        match vault.read(&path) {
            Ok(current) => {
                // `None` asserts the file does not exist. It does, so this is a create
                // over something, which is a stale guard rather than a silent
                // overwrite.
                if base_hash != Some(current.meta.hash) {
                    return Err(IpcError::StaleHash {
                        path: path.to_string(),
                        current_hash: current.meta.hash.to_hex(),
                    });
                }
            }
            Err(VaultError::NotFound { .. }) => {
                // The caller read a hash and the file has since been deleted. That is
                // not a stale guard the caller can resolve by re-reading, so it is
                // reported as what it is.
                if base_hash.is_some() {
                    return Err(IpcError::NotFound {
                        path: path.to_string(),
                    });
                }
            }
            Err(err) => return Err(from_vault_error(err)),
        }

        vault
            .write_atomic(&path, args.text.as_bytes())
            .map_err(from_vault_error)?;

        // §3.1.1's save path, steps 2-4: submit the note to the index writer with no
        // watcher and no debounce. Fire-and-forget by construction -- `submit_to_index`
        // never calls `flush()` (§2.2: a barrier here would put the index back on the
        // keystroke path, design goal 2) -- so this can only add work, never latency
        // or a new way for `write_file` to fail. "Saved" and "indexed" stay two
        // different facts: the command answers for the vault write below, and
        // `index.changed` is how a caller learns the rest.
        self.submit_to_index(&vault, &path);

        // Re-read for the mtime and size the contract returns. It costs a second read
        // of a file we just wrote, and it buys a `hash` that is genuinely "BLAKE3 of
        // what is now on disk" rather than of what we intended to put there. P0 writes
        // whole documents from a text box; when the byte-range patcher arrives in P2
        // this is a stat, not a read.
        let written = vault.read(&path).map_err(from_vault_error)?;

        Ok(FileWritten {
            path: path.to_string(),
            hash: written.meta.hash.to_hex(),
            mtime_ms: written.meta.mtime_ms,
            bytes: written.meta.size,
        })
    }

    /// `index_status`. NFR-PRF-01, NFR-REL-02.
    ///
    /// Never blocks and never touches SQLite. With no vault open it reports
    /// [`IndexState::Opening`] against an empty vault id, which is what "not known yet"
    /// is for.
    #[must_use]
    pub fn index_status(&self) -> IndexStatus {
        match self.open_lock().as_ref() {
            Some(open) => open.publisher.status(),
            None => IndexStatus {
                vault_id: String::new(),
                state: IndexState::Opening,
                since_process_start_ms: self.startup.elapsed_ms() as u64,
                refused_names: None,
            },
        }
    }

    /// `get_ui_settings`. FR-PLT-05. The front end's slice of the settings file.
    #[must_use]
    pub fn get_ui_settings(&self) -> UiSettings {
        let settings = self.lock_settings();
        UiSettings {
            live_preview: settings.editor.live_preview,
            keybindings: settings
                .keybindings
                .iter()
                .map(|(command_id, key)| KeybindingOverride {
                    command_id: command_id.clone(),
                    key: (!key.is_empty()).then(|| key.clone()),
                })
                .collect(),
        }
    }

    /// `set_live_preview`. FR-EDT-10, FR-PLT-05.
    ///
    /// # Errors
    ///
    /// [`IpcError::Io`] if the settings file cannot be written; nothing changes then.
    pub fn set_live_preview(&self, args: SetLivePreviewArgs) -> Result<(), IpcError> {
        self.update_settings(|settings| {
            settings.editor.live_preview = args.enabled;
            Ok(())
        })
    }

    /// `set_keybinding`. FR-PLT-05.
    ///
    /// # Errors
    ///
    /// [`IpcError::Io`] for an empty, over-long or control-character id or key (a client
    /// bug; the contract has no closer variant), or if the file cannot be written.
    pub fn set_keybinding(&self, args: SetKeybindingArgs) -> Result<(), IpcError> {
        let bad = |what: &str| IpcError::Io {
            path: args.command_id.chars().take(MAX_BINDING_TEXT).collect(),
            message: format!("{what} is not acceptable"),
        };
        let valid = |text: &str| {
            !text.is_empty()
                && text.chars().count() <= MAX_BINDING_TEXT
                && !text.chars().any(char::is_control)
        };
        if !valid(&args.command_id) || args.command_id.chars().any(char::is_whitespace) {
            return Err(bad("the command id"));
        }
        if let KeybindingChange::Set { key } = &args.change
            && !valid(key)
        {
            return Err(bad("the key"));
        }
        self.update_settings(|settings| {
            match &args.change {
                KeybindingChange::Set { key } => {
                    settings
                        .keybindings
                        .insert(args.command_id.clone(), key.clone());
                }
                KeybindingChange::Unbind => {
                    settings
                        .keybindings
                        .insert(args.command_id.clone(), String::new());
                }
                KeybindingChange::Reset => {
                    settings.keybindings.remove(&args.command_id);
                }
            }
            Ok(())
        })
    }

    /// Apply `change` to a copy, write it, and only then keep it: a failed write leaves
    /// memory and file agreeing on the old value.
    fn update_settings(
        &self,
        change: impl FnOnce(&mut Settings) -> Result<(), IpcError>,
    ) -> Result<(), IpcError> {
        let mut guard = self.lock_settings();
        let mut next = guard.clone();
        change(&mut next)?;
        if let Some(path) = &self.settings_path {
            next.save_to(path).map_err(|message| IpcError::Io {
                path: path.display().to_string(),
                message,
            })?;
        }
        *guard = next;
        Ok(())
    }

    fn lock_settings(&self) -> std::sync::MutexGuard<'_, Settings> {
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `gitignore_offer_status`. NFR-CMP-04. See [`crate::gitignore`].
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] with no vault open.
    pub fn gitignore_offer_status(&self) -> Result<GitignoreOffer, IpcError> {
        Ok(crate::gitignore::status(self.vault()?.root()))
    }

    /// `append_gitignore_entry`: the single named exception to T-06. Takes no path.
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] with no vault open; [`IpcError::Io`] as
    /// [`crate::gitignore::append`].
    pub fn append_gitignore_entry(&self) -> Result<GitignoreAppended, IpcError> {
        crate::gitignore::append(self.vault()?.root())
    }

    /// `decline_gitignore_offer`. Writes only inside `.serat/`.
    ///
    /// # Errors
    ///
    /// As [`AppState::append_gitignore_entry`].
    pub fn decline_gitignore_offer(&self) -> Result<(), IpcError> {
        crate::gitignore::decline(self.vault()?.root())
    }

    /// `search`. FR-NAV-02, FR-STR-03, FR-BIN-02. Answers `NotReady` rather than
    /// waiting (rule 1). Limits are the core's: `Reader::search` clamps (#15 ruling).
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] with no vault open.
    pub fn search(&self, args: SearchArgs) -> Result<IndexRead<SearchOutcome>, IpcError> {
        let (root, state, search) = self.read_context()?;
        Ok(crate::reads::read_when_ready(
            state,
            &root,
            Some(&search),
            |reader| {
                reader
                    .search(&args.query, args.offset, args.limit)
                    .map(crate::reads::map_outcome)
            },
        ))
    }

    /// `backlinks`. FR-NAV-04.
    ///
    /// # Errors
    ///
    /// [`IpcError::PathEscapesVault`] for a bad path; [`IpcError::VaultNotFound`] with
    /// no vault open.
    pub fn backlinks(&self, args: NotePathArgs) -> Result<IndexRead<Vec<Backlink>>, IpcError> {
        let path = Self::vault_path(&args.path)?;
        let (root, state, _) = self.read_context()?;
        Ok(crate::reads::read_when_ready(
            state,
            &root,
            None,
            |reader| {
                reader
                    .backlinks(path.as_str())
                    .map(|links| links.into_iter().map(crate::reads::map_backlink).collect())
            },
        ))
    }

    /// The root, the last published index state and the search slot, read without blocking.
    fn read_context(
        &self,
    ) -> Result<(PathBuf, IndexState, Arc<crate::reads::SearchSlot>), IpcError> {
        let guard = self.open_lock();
        let open = guard.as_ref().ok_or_else(|| IpcError::VaultNotFound {
            path: String::new(),
        })?;
        Ok((
            open.vault.root().to_path_buf(),
            open.publisher.status().state,
            Arc::clone(&open.search),
        ))
    }

    /// `create_folder`. FR-VLT-11, NFR-SEC-06. See [`crate::folder`].
    ///
    /// # Errors
    ///
    /// As [`crate::folder::create_folder`], plus [`IpcError::VaultNotFound`] with no
    /// vault open.
    pub fn create_folder(&self, args: CreateFolderArgs) -> Result<FolderCreated, IpcError> {
        crate::folder::create_folder(&*self.vault()?, &args.path)
    }

    /// `refused_names`. FR-VLT-01, design goal 5.
    ///
    /// The list behind `IndexStatus::refused_names`: a read of the open vault's
    /// refusal log as it stands, ordered by path in byte order (the log is a
    /// `BTreeMap`), one entry per distinct path, at most [`REFUSED_NAMES_CAP`]. It
    /// never triggers a walk and never waits. Empty with no vault open. Entries
    /// leave only at the next walk, which clears the log, so the list may name a file
    /// that has since been deleted -- it is a report, never a path a command acts on.
    #[must_use]
    pub fn refused_names(&self) -> Vec<RefusedName> {
        let Some(vault) = self.vault().ok() else {
            return Vec::new();
        };
        vault
            .refusals()
            .snapshot()
            .into_iter()
            .take(REFUSED_NAMES_CAP as usize)
            .map(|refusal| RefusedName {
                path: refusal.path,
                reason: refusal.reason,
            })
            .collect()
    }

    /// `mark_first_paint`. NFR-PRF-01, NFR-PRF-08 (F-397, task #113).
    ///
    /// Moved out of `commands.rs` for the reason every other command's logic already
    /// lives here (module doc): so the call site is assertable without a running
    /// application. `commands::mark_first_paint` cannot be driven directly —
    /// `tauri::State` has no public constructor outside the framework, and
    /// `tauri::test`'s `MockRuntime` crashes on this toolchain (F-345) — which is
    /// exactly how the F-377 guard shipped with its own call site untested: every
    /// `f_377_*` test drives [`Startup::mark_first_paint`] directly, and a call site
    /// that discarded its arguments and passed a hardcoded constant left all of them
    /// green. This method is what a test can drive; `commands::mark_first_paint` is
    /// now the one-line delegate the module doc always asked for.
    ///
    /// # Errors
    ///
    /// [`PaintEvidenceRejected`], as [`startup::validate_paint_evidence`] and
    /// [`Startup::mark_first_paint`].
    pub fn mark_first_paint(
        &self,
        first_contentful_paint_ms: Option<f64>,
        raf1_ms: f64,
        raf2_ms: f64,
    ) -> Result<f64, PaintEvidenceRejected> {
        let evidence =
            startup::validate_paint_evidence(first_contentful_paint_ms, raf1_ms, raf2_ms)?;
        self.startup.mark_first_paint(evidence, raf2_ms)?;
        Ok(self.startup.elapsed_ms())
    }

    // -- settings -----------------------------------------------------------

    /// The note to restore for the open vault: `--open` if given, else the settings
    /// file's memory of this vault.
    ///
    /// # The fallback does not apply to a benchmark run
    ///
    /// Under `--exit-after-ready` only `--open` is consulted, and the settings file is
    /// ignored entirely. The fallback is right for a user and dangerous for a budget
    /// job: the settings file is per-machine state, so a job that omitted `--open`
    /// would start passing on any developer machine that had once run the application
    /// interactively, while still failing in clean CI. The local pass is the
    /// reassuring one — somebody sees green and reports a number — which makes it the
    /// flattering-number failure in its purest form (P0 finding F-169).
    ///
    /// Refused here rather than by convention in the harness, because a convention
    /// holds until the first person who does not know about it.
    #[must_use]
    pub fn note_to_open(&self) -> Option<String> {
        if let Some(open) = self.boot.open.clone() {
            return Some(open);
        }
        if self.boot.exit_after_ready {
            return None;
        }
        let root = self.vault_root()?;
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_open_for(&root)
            .map(ToOwned::to_owned)
    }

    /// Remember the open note and write the settings file.
    ///
    /// Never on the critical path: the shell calls this after the vault is usable, and
    /// on shutdown. A failure is returned for logging and changes nothing.
    ///
    /// # Errors
    ///
    /// The filesystem's complaint, as a sentence.
    pub fn remember_open(&self, note: &str) -> Result<(), String> {
        let Some(root) = self.vault_root() else {
            return Ok(());
        };
        let settings = {
            let mut settings = self
                .settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            settings.remember_open(&root, note);
            settings.clone()
        };
        match &self.settings_path {
            Some(path) => settings.save_to(path),
            None => Ok(()),
        }
    }

    /// The canonical root of the open vault, if one is open.
    #[must_use]
    pub fn vault_root(&self) -> Option<PathBuf> {
        self.open_lock()
            .as_ref()
            .map(|open| open.vault.root().to_path_buf())
    }

    // -- internals ----------------------------------------------------------

    /// Rule 2, applied before any filesystem call.
    fn vault_path(raw: &str) -> Result<VaultPath, IpcError> {
        VaultPath::new(raw).map_err(from_vault_error)
    }

    /// The open vault, or the boundary's way of saying there isn't one.
    fn vault(&self) -> Result<Arc<Vault>, IpcError> {
        self.open_lock()
            .as_ref()
            .map(|open| Arc::clone(&open.vault))
            .ok_or_else(|| IpcError::VaultNotFound {
                path: String::new(),
            })
    }

    /// Resolve a vault-relative path against the open vault. Exposed for the doc table
    /// above and used by tests; the commands go through [`AppState::vault_path`].
    ///
    /// # Errors
    ///
    /// As [`AppState::vault_path`], plus [`IpcError::VaultNotFound`] with no vault open.
    pub fn resolve(&self, raw: &str) -> Result<PathBuf, IpcError> {
        let path = Self::vault_path(raw)?;
        self.vault()?.resolve(&path).map_err(from_vault_error)
    }

    /// §3.1.1's save path, steps 2-4, composed at the one call site that needs them
    /// ([`AppState::write_file`]).
    ///
    /// No barrier: `IndexHandle::apply` submits and returns, and this never calls
    /// `writer().flush()` -- doing so here is exactly what §2.2 forbids on the save
    /// path (design goal 2). `apply_and_wait` does not exist in `serat-index` for the
    /// same reason; composing the two primitives at the call site is what keeps a
    /// barrier visible where one is used, and there is deliberately no barrier at this
    /// one.
    ///
    /// A no-op, not an error, in two cases that architecture already accounts for:
    /// the index actor has not yet produced a handle (`IndexHandleSlot`'s doc comment
    /// -- `open_vault` returns before the open check finishes, or the opener never
    /// produced one at all), or the writer refuses under backpressure
    /// (`IndexHandle::apply`'s own doc comment). Both degrade to the watcher's slower
    /// path rather than failing the save (design goal 5) -- the watcher was never told
    /// to suppress this write's echo, since nothing here recorded one, so its own
    /// event for this file indexes it a moment later. F-382: those two cases (and a
    /// third, an open that failed outright) are indistinguishable from each other and
    /// from success at the call site -- `write_file` returns `Ok` in all of them -- so
    /// each branch below counts itself into [`AppState::save_path_outcomes`] rather
    /// than only being distinguishable by reading this function.
    ///
    /// **Echo ordering.** This function does not decide when a write is recorded for
    /// echo suppression -- it decides only whether `apply` is called at all. Ordering
    /// is entirely `apply_via`'s (`serat-index/src/incremental.rs`): the echo hash is
    /// recorded only once `writer.submit(op)` has returned `Ok` (F-224/F-226), never
    /// before and never on a path this function takes if the submit failed. Calling
    /// the same, unmodified `IndexHandle::apply` rather than reimplementing any part
    /// of it is what makes that guarantee travel here unchanged: there is no second
    /// place that could get the ordering wrong.
    ///
    /// **Lock contention, accepted at P0 (F-382).** `serat_index::IndexHandle` is not
    /// `Clone`, so the slot's `Mutex` is held for the duration of `apply` below --
    /// which reads the file and parses it, not only submits. That is not a §2.2
    /// barrier (no waiting on the writer occurs), but it is lock contention on a path
    /// §2.2 otherwise keeps lock-free by ownership. At P0, with one buffer editing one
    /// note at a time, two saves cannot actually contend for it. It stops being free
    /// the day two saves can be in flight together, and the fix is not this function's
    /// to make: `serat_index::writer::WriterHandle` is already `Clone` and cheap
    /// (`writer.rs:82-84`), and `apply_via` already takes one as `&dyn IndexWriter`
    /// rather than the whole `IndexHandle` -- but neither is reachable from outside
    /// `serat-index` today. Cloning a `WriterHandle` out of the slot once, under the
    /// lock, and dropping the lock before parsing is the fix once that seam exists.
    fn submit_to_index(&self, vault: &Vault, path: &VaultPath) {
        let slot = match self.open_lock().as_ref() {
            Some(open) => Arc::clone(&open.index_handle),
            None => {
                self.save_handle_missing.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        let guard = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(handle) = guard.as_ref() else {
            self.save_handle_missing.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let parser = CommonMarkParser::new();
        // The `Err` case is deliberately not propagated to `write_file`'s caller, for
        // the same reason there is no barrier: the command's contract answers for the
        // vault write, not for whether the index heard about it, and a write that
        // reached disk must never be reported as failed because the index writer was
        // busy. It is still counted (F-382), which is the difference between
        // "swallowed" and "unobserved".
        match handle.apply(vault, &parser, vault, path) {
            Ok(()) => self.save_submitted.fetch_add(1, Ordering::Relaxed),
            Err(_) => self.save_writer_refused.fetch_add(1, Ordering::Relaxed),
        };
    }

    /// A snapshot of how many `write_file` calls have taken each branch of the save
    /// path so far (F-382). Diff two snapshots around a benchmark run rather than
    /// reading this mid-write: it answers "how many, of what kind, since the process
    /// started", not "what just happened".
    #[must_use]
    pub fn save_path_outcomes(&self) -> SavePathOutcomes {
        SavePathOutcomes {
            handle_missing: self.save_handle_missing.load(Ordering::Relaxed),
            writer_refused: self.save_writer_refused.load(Ordering::Relaxed),
            submitted: self.save_submitted.load(Ordering::Relaxed),
        }
    }

    fn open_lock(&self) -> std::sync::RwLockReadGuard<'_, Option<OpenVault>> {
        self.open
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn open_lock_mut(&self) -> std::sync::RwLockWriteGuard<'_, Option<OpenVault>> {
        self.open
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::RebuildReason;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("serat-app-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("10-projects")).expect("fixture");
        std::fs::write(
            dir.join("10-projects/serat.md"),
            "---\ntype: project\n---\n\n# Serat\n",
        )
        .expect("fixture");
        std::fs::write(dir.join("contract.pdf"), [0x25, 0x50, 0x44, 0x46]).expect("fixture");
        std::fs::write(dir.join("broken.md"), [0xff, 0xfe, 0x00, 0x9f]).expect("fixture");
        dir
    }

    fn state(dir: &Path) -> AppState {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        state
    }

    #[test]
    fn opening_a_vault_that_is_not_there_is_refused_by_name() {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let err = state
            .open_vault(OpenVaultArgs {
                root: std::env::temp_dir()
                    .join("serat-no-such-vault")
                    .display()
                    .to_string(),
            })
            .expect_err("must not open");
        assert!(matches!(err, IpcError::VaultNotFound { .. }), "{err:?}");
    }

    #[test]
    fn mark_first_paint_forwards_the_real_argument_not_a_hardcoded_one() {
        // F-397 (task #113): the earlier `commands::mark_first_paint` reached into
        // `state.startup` and called `validate_paint_evidence` itself, which put the
        // forwarding logic nowhere a test could drive it — `tauri::State` has no
        // public constructor outside the framework — and a mutation that discarded
        // the caller's arguments and passed a hardcoded constant instead left every
        // existing test green. `AppState::mark_first_paint` is now that logic, and no
        // vault or running application is needed to exercise it. Each case below is
        // refused as the value actually sent — its own number legible in the
        // rejection — which a version substituting a hardcoded value could not
        // produce.
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let err = state
            .mark_first_paint(None, 10.0, 20.0)
            .expect_err("None must be forwarded and refused, not replaced by a valid entry");
        assert!(
            err.reason.contains("no first-contentful-paint entry"),
            "{err:?}"
        );

        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let err = state
            .mark_first_paint(Some(50.0), 20.0, 10.0)
            .expect_err("a non-increasing raf pair must be forwarded and refused");
        assert!(err.reason.contains("10.0"), "{err:?}");
        assert!(err.reason.contains("20.0"), "{err:?}");

        // The positive control: genuinely valid, internally consistent evidence is
        // still accepted, so the two rejections above are rejections of real values
        // crossing the call site, not a version of this method that refuses
        // everything.
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        assert!(
            state.mark_first_paint(Some(50.0), 10.0, 20.0).is_ok(),
            "genuinely valid, internally consistent evidence must still be accepted"
        );
    }

    #[test]
    fn the_command_returns_without_waiting_for_the_index() {
        // §3.4's load-bearing property, at the boundary rather than at the seam: no IPC
        // command blocks on the index. Add a `join` to `open_vault_with` and this fails
        // by a factor of four.
        //
        // Detection power and robustness are independent knobs here, and both bounds
        // below are chosen to buy both at once (task #45, sibling of #44's wall-clock
        // flake in a different crate): a blocking call takes the full sleep, so the
        // *ratio* of sleep to threshold is what catches it, while the threshold's
        // *absolute* size is what has to survive a loaded CI runner's scheduling
        // jitter. The two constants below move together on purpose -- widening the
        // threshold alone without raising the sleep would let a fully blocking call
        // pass, which is exactly the failure mode a future "fix the flaky test" pass
        // could introduce if it touches only one side of this pair.
        struct Slow;
        impl IndexOpen for Slow {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                std::thread::sleep(Duration::from_millis(2_000));
                publish(IndexState::Ready { note_count: 1 });
            }
        }
        const NONBLOCKING_THRESHOLD: Duration = Duration::from_millis(500);

        let dir = fixture("nonblocking");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );

        let started = Instant::now();
        let opened = state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                Slow,
            )
            .expect("open");
        let cost = started.elapsed();

        assert!(
            cost < NONBLOCKING_THRESHOLD,
            "open_vault waited {cost:?} for the index actor"
        );
        assert_eq!(
            opened.index,
            IndexState::Opening,
            "the shell paints whatever state the actor has published, which is nothing yet"
        );
        // And index_status, called immediately, must also not block.
        let started = Instant::now();
        let polled = state.index_status();
        assert!(
            started.elapsed() < NONBLOCKING_THRESHOLD,
            "index_status waited {:?} -- it reads a published value and must never block \
             on the actor",
            started.elapsed()
        );
        assert_eq!(polled.vault_id, opened.vault_id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_note_returns_its_bytes_its_hash_and_its_size() {
        let dir = fixture("read");
        let state = state(&dir);
        let file = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");

        assert_eq!(file.path, "10-projects/serat.md");
        assert!(file.text.starts_with("---\ntype: project"));
        assert_eq!(file.hash.len(), 64);
        assert_eq!(file.bytes, file.text.len() as u64);
        assert!(
            file.mtime_ms > 1_600_000_000_000,
            "mtime_ms is unix milliseconds, not seconds: {}",
            file.mtime_ms
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_utf8_is_refused_rather_than_decoded() {
        // C-04 and design goal 1: a lossy read followed by a save silently rewrites the
        // user's bytes. Replace the `String::from_utf8` with `from_utf8_lossy` and this
        // fails.
        let dir = fixture("notutf8");
        let state = state(&dir);
        let err = state
            .read_file(ReadFileArgs {
                path: "broken.md".into(),
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::NotUtf8 { .. }), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_that_leave_the_vault_are_refused_before_any_filesystem_call() {
        let dir = fixture("escape");
        let state = state(&dir);
        for bad in [
            "../outside.md",
            "/etc/passwd",
            r"C:\Windows\system.ini",
            "a/../../b.md",
        ] {
            let err = state
                .read_file(ReadFileArgs { path: bad.into() })
                .expect_err("must refuse {bad}");
            assert!(
                matches!(err, IpcError::PathEscapesVault { .. }),
                "{bad}: {err:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_markdown_is_writable_and_the_refusal_costs_no_read() {
        // FR-BIN-19 and threat T-06, which P0 finding F-14 found unguarded one layer
        // down. The bytes of the pdf must be exactly what they were.
        let dir = fixture("notwritable");
        let state = state(&dir);
        let before = std::fs::read(dir.join("contract.pdf")).expect("read pdf");

        let err = state
            .write_file(WriteFileArgs {
                path: "contract.pdf".into(),
                text: "destroyed".into(),
                base_hash: None,
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::NotWritable { .. }), "{err:?}");
        assert_eq!(
            std::fs::read(dir.join("contract.pdf")).expect("read pdf"),
            before,
            "the binary was rewritten by a command that claimed to refuse"
        );

        // The refusal is on the extension, so it holds for a file that does not exist
        // either — nothing is read before the rule is applied.
        assert!(matches!(
            state.write_file(WriteFileArgs {
                path: "never/existed.png".into(),
                text: "x".into(),
                base_hash: None,
            }),
            Err(IpcError::NotWritable { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_with_the_right_hash_lands_and_a_write_with_a_stale_one_does_not() {
        let dir = fixture("stale");
        let state = state(&dir);
        let file = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");

        let written = state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "# Serat, edited\n".into(),
                base_hash: Some(file.hash.clone()),
            })
            .expect("write");
        assert_ne!(written.hash, file.hash);
        assert_eq!(written.bytes, "# Serat, edited\n".len() as u64);
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/serat.md")).expect("read back"),
            "# Serat, edited\n"
        );

        // The same base hash a second time is now stale, and the refusal carries the
        // hash the caller needs in order to retry (§6.1, §9.1).
        let err = state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "# clobbered\n".into(),
                base_hash: Some(file.hash),
            })
            .expect_err("must refuse");
        match err {
            IpcError::StaleHash { current_hash, .. } => assert_eq!(current_hash, written.hash),
            other => panic!("expected StaleHash, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/serat.md")).expect("read back"),
            "# Serat, edited\n",
            "a refused write changed the file anyway"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creating_a_note_over_an_existing_one_is_a_stale_guard_not_an_overwrite() {
        let dir = fixture("create");
        let state = state(&dir);

        // A genuine create: no hash, no file.
        let created = state
            .write_file(WriteFileArgs {
                path: "00-inbox/today.md".into(),
                text: "# today\n".into(),
                base_hash: None,
            })
            .expect("create");
        assert_eq!(created.path, "00-inbox/today.md");
        assert!(dir.join("00-inbox/today.md").exists());

        // The same create again is refused, because `None` asserts the file is absent.
        let err = state
            .write_file(WriteFileArgs {
                path: "00-inbox/today.md".into(),
                text: "# clobbered\n".into(),
                base_hash: None,
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::StaleHash { .. }), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("00-inbox/today.md")).expect("read back"),
            "# today\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hash_for_a_file_that_has_been_deleted_is_not_found_rather_than_stale() {
        // The two cases differ in what the caller can do next: a stale hash resolves by
        // re-reading, a deleted file does not.
        let dir = fixture("deleted");
        let state = state(&dir);
        let err = state
            .write_file(WriteFileArgs {
                path: "never/existed.md".into(),
                text: "x".into(),
                base_hash: Some("00".repeat(32)),
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::NotFound { .. }), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_before_a_vault_is_open_is_refused_and_does_not_panic() {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        assert!(matches!(
            state.read_file(ReadFileArgs {
                path: "a.md".into()
            }),
            Err(IpcError::VaultNotFound { .. })
        ));
        assert_eq!(state.index_status().state, IndexState::Opening);
        assert_eq!(state.index_status().vault_id, "");
    }

    #[test]
    fn each_open_vault_gets_its_own_id_so_late_events_can_be_discarded() {
        let dir = fixture("ids");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let first = state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        let second = state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("reopen");
        assert_ne!(first.vault_id, second.vault_id);
        assert_eq!(state.index_status().vault_id, second.vault_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_reaches_an_installed_sink() {
        let dir = fixture("events");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        state.events.install(move |status: &IndexStatus| {
            sink.lock()
                .expect("not poisoned")
                .push(status.state.clone());
        });

        struct Rebuild;
        impl IndexOpen for Rebuild {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 3,
                    walk_complete: false,
                });
                publish(IndexState::Ready { note_count: 3 });
            }
        }

        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                Rebuild,
            )
            .expect("open");

        for _ in 0..200 {
            if seen.lock().expect("not poisoned").len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let seen = seen.lock().expect("not poisoned").clone();
        assert!(
            matches!(seen.first(), Some(IndexState::Rebuilding { .. })),
            "{seen:?}"
        );
        assert!(
            matches!(seen.last(), Some(IndexState::Ready { note_count: 3 })),
            "{seen:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_benchmark_run_never_takes_the_note_from_the_settings_file() {
        // F-169. The hazard: a cold-start job that omitted `--open` would pass on any
        // machine whose settings file remembered a note and fail in clean CI, and the
        // local pass is the one somebody reports a number from. Delete the
        // `exit_after_ready` early return in `note_to_open` and this fails.
        let dir = fixture("benchfallback");
        let canonical = Vault::open(&dir).expect("open").root().to_path_buf();
        let mut settings = Settings::default();
        settings.remember_open(&canonical, "10-projects/serat.md");

        let bench = AppState::with_settings(
            Arc::new(Startup::begin_with(None)),
            Boot {
                exit_after_ready: true,
                ..Boot::default()
            },
            settings.clone(),
        );
        bench
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(
            bench.note_to_open(),
            None,
            "a benchmark run took its note from per-machine state"
        );

        // The same settings, the same vault, with the flag given: the run is defined
        // by its arguments and nothing else.
        let flagged = AppState::with_settings(
            Arc::new(Startup::begin_with(None)),
            Boot {
                exit_after_ready: true,
                open: Some("10-projects/serat.md".into()),
                ..Boot::default()
            },
            settings.clone(),
        );
        flagged
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(
            flagged.note_to_open().as_deref(),
            Some("10-projects/serat.md")
        );

        // And an ordinary session still restores where the user left off, which is the
        // behaviour the guard must not have removed.
        let session = AppState::with_settings(
            Arc::new(Startup::begin_with(None)),
            Boot::default(),
            settings,
        );
        session
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(
            session.note_to_open().as_deref(),
            Some("10-projects/serat.md"),
            "the guard removed the fallback from ordinary sessions too"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_to_open_is_the_flag_first_and_the_settings_file_second() {
        let dir = fixture("lastopen");
        let mut settings = Settings::default();
        let canonical = Vault::open(&dir).expect("open").root().to_path_buf();
        settings.remember_open(&canonical, "10-projects/serat.md");

        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            settings.clone(),
        );
        assert_eq!(
            state.note_to_open(),
            None,
            "no vault open, nothing to restore"
        );
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(
            state.note_to_open().as_deref(),
            Some("10-projects/serat.md")
        );

        let flagged = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot {
                open: Some("other.md".into()),
                ..Boot::default()
            },
            settings,
        );
        flagged
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(flagged.note_to_open().as_deref(), Some("other.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Create a symlink at `link` pointing at `target`, or say why we could not.
    ///
    /// Deliberately not `#[cfg(unix)]`-gated the way `serat-vault`'s own symlink tests
    /// are: those are mechanism tests, safe to skip on one platform because the
    /// property under test has no platform-specific content. These two are the
    /// **acceptance** tests for NFR-SEC-06 (task #29) — the case the requirement is
    /// written at, a symlink reached through the IPC boundary — and a security
    /// acceptance test that silently does not exist on a CI runner is worse than one
    /// that exists and says it could not run (task #29, generalising F-58's "a guard
    /// that cannot fail is worse than none" to "a test that cannot be seen not to run
    /// is worse than none"). Every platform compiles and executes these tests; only
    /// the symlink primitive, and what a missing privilege looks like, differs.
    fn make_symlink(target: &Path, link: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            // F-306: a skip is NOT acceptable here. Symlink creation on Unix needs no
            // privilege, so a failure is a real error rather than a platform limit --
            // and CI's ubuntu leg is the one place these tests genuinely execute.
            // Returning Err on Unix would let the security test report `ok` having
            // asserted nothing, which is the defect this whole task exists to close.
            std::os::unix::fs::symlink(target, link)
                .expect("symlink creation on Unix needs no privilege; a failure here is a real error, not a reason to skip a security test");
            Ok::<(), String>(())
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).map_err(|err| {
                format!(
                    "std::os::windows::fs::symlink_file failed ({err}) -- creating a \
                     symlink on Windows needs Developer Mode or an elevated process, \
                     neither of which a test runner can assume"
                )
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            Err("no symlink primitive is wired for this platform".to_owned())
        }
    }

    /// Print a named, greppable reason a symlink acceptance test did not run, rather
    /// than let it pass with nothing asserted. `cargo test` has no third "skipped"
    /// outcome for a plain `#[test]`, so this is the loudest signal available short of
    /// failing outright -- and failing outright would make the whole suite red on
    /// every unprivileged Windows machine for a platform limitation, not a defect.
    fn skip_loudly(test_name: &str, reason: &str) {
        eprintln!("SKIPPED {test_name}: could not create a symlink -- {reason}");
    }

    #[test]
    fn a_symlinked_note_is_refused_at_read_file_not_followed() {
        // NFR-SEC-06's acceptance criterion, exercised at the level it is written at:
        // "Given a symlink inside the vault pointing to a target outside the vault
        // root, when any read ... operation is invoked on it, then the request is
        // refused and no bytes cross the vault boundary." `serat-vault::resolve` has
        // unit tests for the mechanism; nothing until now called `read_file` -- the
        // IPC boundary the finding (task #18 finding 4, T-07) was actually about --
        // against a real symlink. F-58: a broken guard would make this return `Ok`
        // with the outside file's bytes, not merely error some other way.
        let dir = fixture("symlink-read");
        let outside = std::env::temp_dir().join(format!(
            "serat-app-state-symlink-outside-read-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).expect("outside dir");
        let secret = outside.join("secret.md");
        std::fs::write(&secret, "outside the vault").expect("write outside");

        let link = dir.join("escape.md");
        match make_symlink(&secret, &link) {
            Ok(()) => {
                let state = state(&dir);
                let result = state.read_file(ReadFileArgs {
                    path: "escape.md".into(),
                });
                assert!(
                    matches!(result, Err(IpcError::PathEscapesVault { .. })),
                    "a symlink inside the vault was followed by read_file instead of \
                     refused: {result:?}"
                );
            }
            Err(reason) => {
                skip_loudly(
                    "a_symlinked_note_is_refused_at_read_file_not_followed",
                    &reason,
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_symlinked_note_is_refused_at_write_file_and_the_target_is_untouched() {
        // The write half of the same acceptance criterion, and the sharper one: "no
        // bytes cross the vault boundary" means the file *outside* the vault must be
        // provably unchanged, not merely that write_file returned an error. write_file
        // reaches the symlink through its optimistic-lock read (rule 4) before
        // write_atomic is ever called, so this also exercises that ordering.
        //
        // `base_hash` is set to the outside target's own hash rather than left `None`.
        // With `None`, rule 4 alone would refuse an unguarded write anyway (it reads
        // as "create over something that already exists"), which would pass this test
        // for the wrong reason (F-58) and prove nothing about containment. Matching
        // the hash means that if the symlink guard were removed, the lock would be
        // satisfied by the followed read and the write would proceed to
        // `write_atomic` -- isolating containment as the only thing standing between
        // the request and the bytes outside the vault.
        let dir = fixture("symlink-write");
        let outside = std::env::temp_dir().join(format!(
            "serat-app-state-symlink-outside-write-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).expect("outside dir");
        let secret = outside.join("secret.md");
        let original = "outside, original";
        std::fs::write(&secret, original).expect("write outside");

        let link = dir.join("escape.md");
        match make_symlink(&secret, &link) {
            Ok(()) => {
                let state = state(&dir);
                let base_hash = serat_vault::hash_bytes(original.as_bytes()).to_hex();
                let result = state.write_file(WriteFileArgs {
                    path: "escape.md".into(),
                    text: "clobbered from inside the vault".into(),
                    base_hash: Some(base_hash),
                });
                assert!(
                    matches!(result, Err(IpcError::PathEscapesVault { .. })),
                    "a symlink inside the vault was written through by write_file \
                     instead of refused: {result:?}"
                );
                assert_eq!(
                    std::fs::read_to_string(&secret).expect("read outside"),
                    original,
                    "a refused write_file changed bytes outside the vault root anyway"
                );
            }
            Err(reason) => {
                skip_loudly(
                    "a_symlinked_note_is_refused_at_write_file_and_the_target_is_untouched",
                    &reason,
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_save_through_write_file_reaches_the_index_without_the_watcher() {
        // Task #90: the save path of architecture §3.1.1, landed end to end.
        //
        // A generous deadline here would not be a test: the vault this fixture opens
        // also has a *watcher* running (§3.1.2), which would eventually index the same
        // save through the ordinary filesystem-event path and write the identical row
        // -- same table, same hash, indistinguishable at rest. A first version of this
        // test used a 5s poll and stayed green with `self.submit_to_index(&vault,
        // &path)` in `write_file` commented out, which is exactly the failure this
        // project has a name for (F-13, F-28, F-35, F-50, F-58): the assertion was
        // correct and the setup let the watcher answer for the path under test.
        //
        // The fix is the one property that tells the two paths apart: the watcher
        // cannot possibly produce a row inside `serat_vault::watch::COALESCE_WINDOW`
        // (150 ms, §3.1 step 1) because it does not even look at the event until that
        // window closes. So the poll below gives up well inside that window --
        // comfortably above NFR-PRF-03's own 50 ms budget for the direct path, and
        // comfortably below the watcher's 150 ms floor -- which is a bound the direct
        // path can meet and the watcher path structurally cannot. Mutation-verified:
        // commenting out the `self.submit_to_index(&vault, &path)` call in
        // `write_file` turns this red rather than merely slow.
        //
        // Reads `.serat/index.db` through rusqlite -- a connection this crate uses
        // nowhere else -- rather than through `serat_index`'s own reader, so a bug in
        // the wiring cannot also hide the assertion meant to catch it (Cargo.toml's
        // dev-dependency comment).
        const GIVES_UP_BEFORE_THE_WATCHER_COULD_ANSWER: Duration = Duration::from_millis(100);
        assert!(
            GIVES_UP_BEFORE_THE_WATCHER_COULD_ANSWER < serat_vault::watch::COALESCE_WINDOW,
            "the deadline must stay under the watcher's own coalescing window, or a slow \
             watcher answering in time would pass this test for the wrong reason"
        );

        let dir = fixture("save-reaches-index");
        let state = state(&dir);

        // Wait for the fixture's own initial rebuild to settle before writing. Skipping
        // this is what let the first version of this test pass under the mutation
        // above for the wrong reason: on a vault this small the rebuild can still be
        // reading `serat.md` off disk when `write_file` below overwrites it, so the
        // rebuild itself sometimes indexed the *new* bytes and the test could not tell
        // that from `submit_to_index` having done it. Once the vault is `Ready`, the
        // rebuild is done reading, and only the write below can produce a fresh hash.
        let ready_by = Instant::now() + Duration::from_secs(2);
        while !matches!(state.index_status().state, IndexState::Ready { .. }) {
            assert!(
                Instant::now() < ready_by,
                "the fixture vault never reached IndexState::Ready"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // `Ready` publishes slightly before the `IndexHandleSlot` is filled
        // (index.rs's own doc comment on the type), a gap of at most a couple of
        // instructions on the same thread. This slack is setup, not measurement --
        // the 100ms deadline below starts only once the write happens.
        std::thread::sleep(Duration::from_millis(50));

        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read the fixture note");

        let before = state.save_path_outcomes();

        let written = state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "---\ntype: project\n---\n\n# Serat\n\nWired through apply().\n".into(),
                base_hash: Some(read.hash),
            })
            .expect("write");

        // F-382: the same claim the hash poll below checks against the database, made
        // against the observable counters instead -- this save must count as a real
        // submission, not as one of the two silent degradations.
        let after = state.save_path_outcomes();
        assert_eq!(
            after.submitted,
            before.submitted + 1,
            "a save with the handle ready must count as submitted: {after:?}"
        );
        assert_eq!(
            (after.handle_missing, after.writer_refused),
            (before.handle_missing, before.writer_refused),
            "waiting for Ready means this save had no reason to degrade: {after:?}"
        );

        // No barrier on the save path (§2.2), so the row may not be visible the
        // instant write_file returns -- the poll below is the harness serialising
        // deliberately to observe an async write, which §2.2 permits outside the
        // product path. It is not a substitute for design goal 2: nothing here is on
        // `write_file`'s own call stack.
        let index_db = dir.join(".serat").join("index.db");
        let deadline = Instant::now() + GIVES_UP_BEFORE_THE_WATCHER_COULD_ANSWER;
        let mut seen_hash = None;
        while Instant::now() < deadline {
            if let Ok(conn) = rusqlite::Connection::open(&index_db) {
                let row: rusqlite::Result<String> = conn.query_row(
                    "SELECT hash FROM notes WHERE path = ?1",
                    ["10-projects/serat.md"],
                    |row| row.get(0),
                );
                if let Ok(hash) = row
                    && hash == written.hash
                {
                    seen_hash = Some(hash);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            seen_hash,
            Some(written.hash),
            "a note saved through write_file must reach .serat/index.db well inside the \
             watcher's own 150ms coalescing window, carrying the hash write_file itself \
             returned -- which only the direct submit path in `write_file` can manage"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_vault_becomes_usable_while_the_rebuild_is_still_running() {
        // §3.4's first `t_usable` precondition, pinned through the real
        // `open_vault_with` wiring rather than by calling `mark_index_settled` by
        // hand: the mark fires when the index state is *known*, not when it is good.
        //
        // Recorded as its own test by #119, because #119 makes the actor hold its
        // *terminal* state back until the handle slot is filled and the whole safety
        // of that change rests on `Rebuilding` being exempt. The index-side tests
        // prove `Rebuilding` is still published; only this one proves that publishing
        // it still makes the vault usable -- the observer installed in
        // `open_vault_with` is the link between the two, and nothing else asserted it.
        // Widen the deferral to every state and this opener, which never reaches
        // `Ready` while the assertion runs, leaves the shell waiting forever.
        struct RebuildsUntilReleased(Arc<std::sync::Barrier>);
        impl IndexOpen for RebuildsUntilReleased {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 20_000,
                    walk_complete: false,
                });
                // Still rebuilding, and still holding no handle, for as long as the
                // assertion below takes -- which is the situation §3.4 describes.
                self.0.wait();
            }
        }

        let dir = fixture("usable-mid-rebuild");
        let startup = Arc::new(Startup::begin());
        let state =
            AppState::with_settings(Arc::clone(&startup), Boot::default(), Settings::default());

        // The three `t_usable` terms this test is not about. Set first, so that when
        // the assertion runs the only outstanding term is the index one.
        startup.mark_window_created();
        startup
            .mark_first_paint(42.0, 20.0)
            .expect("valid paint evidence");
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();

        let release = Arc::new(std::sync::Barrier::new(2));
        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                RebuildsUntilReleased(Arc::clone(&release)),
            )
            .expect("open");

        assert!(
            startup.wait_for_usable(Duration::from_secs(5)),
            "a vault whose index is still rebuilding is usable: the shell must come \
             up on exactly the vault that needs a rebuild most (§3.4, and \
             `Startup::mark_index_settled`'s own doc). A save landing here \
             finds an empty slot and degrades to the watcher path, which is \
             design goal 5 and not a defect (F-450)"
        );

        release.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_with_no_index_handle_is_counted_not_silent() {
        // F-382: `write_file` returns `Ok` identically whether the save reached the
        // writer or the slot was empty, and the empty-slot case covers two of F-382's
        // three situations at once here -- an opener whose `into_handle` is never
        // overridden (the trait default, silent by construction) reproduces both "no
        // handle yet" and "no handle ever" without needing to win a real race.
        struct NeverProducesAHandle;
        impl IndexOpen for NeverProducesAHandle {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Ready { note_count: 1 });
                // into_handle() is not overridden: the trait default returns None.
            }
        }

        let dir = fixture("no-handle");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                NeverProducesAHandle,
            )
            .expect("open");

        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");
        state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "# edited with no handle to submit through\n".into(),
                base_hash: Some(read.hash),
            })
            .expect("write_file still succeeds -- the vault write is real regardless");

        let outcomes = state.save_path_outcomes();
        assert_eq!(
            outcomes,
            SavePathOutcomes {
                handle_missing: 1,
                writer_refused: 0,
                submitted: 0,
            },
            "a save with no handle to submit through must be counted as degraded, not \
             merely absent from the submitted count"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- refused names (FR-VLT-01, task #34, contracts/ipc.rs) ---------------------

    #[cfg(unix)]
    fn write_non_utf8_names(dir: &Path, count: usize) {
        use std::os::unix::ffi::OsStrExt as _;
        for i in 0..count {
            let mut name = format!("caf{i:05}").into_bytes();
            name.extend_from_slice(b"\xe9.md");
            std::fs::write(dir.join(std::ffi::OsStr::from_bytes(&name)), b"x").expect("fixture");
        }
    }

    #[test]
    fn refused_names_is_none_with_no_vault_and_counts_from_zero_with_one() {
        // `None` means "not counting"; it must never be confused with a count of zero,
        // or the front end shows "0 names not shown" before any vault exists.
        let bare = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        assert_eq!(bare.index_status().refused_names, None);
        assert!(bare.refused_names().is_empty());

        let dir = fixture("refused-zero");
        let state = state(&dir);
        assert_eq!(state.index_status().refused_names, Some(0));
        assert!(state.refused_names().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_status_count_and_the_list_come_from_the_same_refusal_log() {
        let dir = fixture("refused-some");
        write_non_utf8_names(&dir, 3);
        let state = state(&dir);
        // The vault thread walks after the first paint; drive the walk directly so the
        // test does not wait on that gate.
        state.vault().expect("open").walk().expect("walk");

        let status = state.index_status();
        let list = state.refused_names();
        assert_eq!(status.refused_names, Some(3), "{status:?}");
        assert_eq!(list.len(), 3, "{list:?}");
        let paths: Vec<&str> = list.iter().map(|r| r.path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted, "the list is ordered by path in byte order");
        assert!(list.iter().all(|r| !r.reason.is_empty()), "{list:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_count_and_the_list_saturate_at_the_cap_and_agree() {
        // `RefusalLog::len()` is the number of distinct stored paths; `overflowed()`
        // counts events past the cap and is deliberately not added. 1005 refused
        // names must read as exactly REFUSED_NAMES_CAP in both places.
        let dir = fixture("refused-cap");
        write_non_utf8_names(&dir, REFUSED_NAMES_CAP as usize + 5);
        let state = state(&dir);
        let vault = state.vault().expect("open");
        vault.walk().expect("walk");
        assert!(
            vault.refusals().overflowed() >= 5,
            "the fixture did not overflow the log"
        );

        assert_eq!(state.index_status().refused_names, Some(REFUSED_NAMES_CAP));
        assert_eq!(state.refused_names().len(), REFUSED_NAMES_CAP as usize);
    }

    #[cfg(unix)]
    #[test]
    fn a_deleted_refused_file_leaves_the_list_only_at_the_next_walk() {
        // The contract states this limit rather than hiding it: the list is a report,
        // so it may name a file that is already gone until the walk clears the log.
        use std::os::unix::ffi::OsStrExt as _;
        let dir = fixture("refused-stale");
        let odd = dir.join(std::ffi::OsStr::from_bytes(b"gone\xe9.md"));
        std::fs::write(&odd, b"x").expect("fixture");
        let state = state(&dir);
        let vault = state.vault().expect("open");
        vault.walk().expect("walk");
        assert_eq!(state.refused_names().len(), 1);

        std::fs::remove_file(&odd).expect("rm");
        assert_eq!(
            state.refused_names().len(),
            1,
            "the list is not re-walked by a read"
        );
        vault.walk().expect("walk");
        assert!(state.refused_names().is_empty());
        assert_eq!(state.index_status().refused_names, Some(0));
    }

    // -- index reads (FR-NAV-02, FR-NAV-04) -----------------------------------------

    fn wait_ready(state: &AppState) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if matches!(state.index_status().state, IndexState::Ready { .. }) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the index never became Ready: {:?}", state.index_status());
    }

    #[test]
    fn reads_with_no_vault_open_are_refused_by_name() {
        let bare = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let search = bare.search(SearchArgs {
            query: "x".into(),
            offset: 0,
            limit: 10,
        });
        assert!(matches!(search, Err(IpcError::VaultNotFound { .. })));
        let links = bare.backlinks(NotePathArgs {
            path: "a.md".into(),
        });
        assert!(matches!(links, Err(IpcError::VaultNotFound { .. })));
    }

    #[test]
    fn search_and_backlinks_answer_from_a_ready_index_in_the_contracts_terms() {
        let dir = fixture("reads");
        std::fs::write(dir.join("alpha.md"), "# Alpha\n\nsee [[beta]]\n").expect("fixture");
        std::fs::write(dir.join("beta.md"), "# Beta\n\nzebra stripes\n").expect("fixture");
        let state = state(&dir);
        wait_ready(&state);

        let found = state
            .search(SearchArgs {
                query: "zebra".into(),
                offset: 0,
                limit: 10,
            })
            .expect("search");
        let IndexRead::Ready {
            value: SearchOutcome::Hits { items },
        } = found
        else {
            panic!("expected hits, got {found:?}");
        };
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].path, "beta.md");
        assert!(items[0].block.is_some() && items[0].snippet.is_some());

        let nothing = state
            .search(SearchArgs {
                query: "\"unbalanced".into(),
                offset: 0,
                limit: 10,
            })
            .expect("search");
        assert!(matches!(
            nothing,
            IndexRead::Ready {
                value: SearchOutcome::UnbalancedQuote
            }
        ));

        let links = state
            .backlinks(NotePathArgs {
                path: "beta.md".into(),
            })
            .expect("backlinks");
        let IndexRead::Ready { value } = links else {
            panic!("expected ready, got {links:?}");
        };
        assert_eq!(value.len(), 1, "{value:?}");
        assert_eq!(value[0].source_path, "alpha.md");
        assert!(value[0].excerpt.contains("[[beta]]"), "{value:?}");

        let bad = state.backlinks(NotePathArgs {
            path: "../x.md".into(),
        });
        assert!(matches!(bad, Err(IpcError::PathEscapesVault { .. })));
    }

    #[test]
    fn a_read_before_the_index_is_ready_reports_the_state_instead_of_waiting() {
        // Rule 1. A scripted opener that never finishes keeps the state at `Opening`.
        struct Never;
        impl IndexOpen for Never {
            fn open(&mut self, _: &Path, _: &Path, _: &dyn Fn(IndexState)) {
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        let dir = fixture("reads-not-ready");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                Never,
            )
            .expect("open");
        let started = Instant::now();
        let answer = state
            .search(SearchArgs {
                query: "x".into(),
                offset: 0,
                limit: 10,
            })
            .expect("search");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            matches!(
                answer,
                IndexRead::NotReady {
                    state: IndexState::Opening
                }
            ),
            "{answer:?}"
        );
    }

    // -- ui settings (FR-PLT-05, FR-EDT-10) ------------------------------------------

    fn settings_state(name: &str) -> (AppState, PathBuf) {
        let dir = std::env::temp_dir().join(format!("serat-app-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.toml");
        let mut state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        state.settings_path = Some(path.clone());
        (state, path)
    }

    #[test]
    fn ui_settings_default_to_live_preview_and_no_overrides() {
        let (state, _) = settings_state("defaults");
        let ui = state.get_ui_settings();
        assert!(ui.live_preview);
        assert!(ui.keybindings.is_empty());
    }

    #[test]
    fn live_preview_and_keybindings_persist_through_the_toml_file() {
        let (state, path) = settings_state("persist");
        state
            .set_live_preview(SetLivePreviewArgs { enabled: false })
            .expect("set");
        for (id, change) in [
            (
                "editor.toggle-bold",
                KeybindingChange::Set {
                    key: "Mod-b".into(),
                },
            ),
            ("editor.toggle-italic", KeybindingChange::Unbind),
            (
                "editor.toggle-code",
                KeybindingChange::Set {
                    key: "Mod-e".into(),
                },
            ),
            ("editor.toggle-code", KeybindingChange::Reset),
        ] {
            state
                .set_keybinding(SetKeybindingArgs {
                    command_id: id.into(),
                    change,
                })
                .expect("set");
        }

        // A fresh process reads the same answer from the file.
        let reloaded = Settings::load_from(&path);
        assert!(reloaded.problem.is_none(), "{:?}", reloaded.problem);
        let fresh = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            reloaded.settings,
        );
        let ui = fresh.get_ui_settings();
        assert!(!ui.live_preview);
        let by_id: Vec<(&str, Option<&str>)> = ui
            .keybindings
            .iter()
            .map(|k| (k.command_id.as_str(), k.key.as_deref()))
            .collect();
        assert_eq!(
            by_id,
            vec![
                ("editor.toggle-bold", Some("Mod-b")),
                ("editor.toggle-italic", None),
            ],
            "unbound is Some-less None, and a reset leaves no entry"
        );
    }

    #[test]
    fn nonsense_bindings_are_refused_and_change_nothing() {
        let (state, path) = settings_state("nonsense");
        for (id, change) in [
            ("", KeybindingChange::Unbind),
            ("has space", KeybindingChange::Unbind),
            ("ok.id", KeybindingChange::Set { key: String::new() }),
            ("ok.id", KeybindingChange::Set { key: "a\nb".into() }),
            ("x".repeat(500).leak(), KeybindingChange::Unbind),
        ] {
            let err = state
                .set_keybinding(SetKeybindingArgs {
                    command_id: id.into(),
                    change,
                })
                .expect_err("refused");
            assert!(matches!(err, IpcError::Io { .. }), "{err:?}");
        }
        assert!(state.get_ui_settings().keybindings.is_empty());
        assert!(!path.exists(), "a refused change wrote the file");
    }

    #[test]
    fn a_failed_write_leaves_the_old_value_in_memory() {
        let (mut state, path) = settings_state("unwritable");
        // A file where the directory should be makes the write fail.
        std::fs::create_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
        std::fs::write(path.parent().unwrap(), b"not a dir").unwrap();
        state.settings_path = Some(path);
        let err = state
            .set_live_preview(SetLivePreviewArgs { enabled: false })
            .expect_err("unwritable");
        assert!(matches!(err, IpcError::Io { .. }), "{err:?}");
        assert!(state.get_ui_settings().live_preview);
    }
}
