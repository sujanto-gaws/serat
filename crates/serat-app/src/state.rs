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

use serat_core::{ContentHash, VaultPath};
use serat_parse::CommonMarkParser;
use serat_vault::Vault;

use crate::cli::Boot;
use crate::consumer::{ChangeNotifier, Indexer, OpenCatchUp, SETTLE_BOUND, TREE_GRACE};
use crate::editor::Editor;
use crate::error::from_vault_error;
use crate::index::{
    DetachedWriters, IndexActor, IndexHandleSlot, IndexOpen, SeratIndexOpen, StatusPublisher,
    StatusSink, index_dir_for,
};
use crate::ipc::{
    AppClosing, AssetArgs, AssetInfo, Backlink, CreateFolderArgs, ExternalChange, FileContents,
    FileWritten, FlushAckArgs, FolderCreated, GitignoreAppended, GitignoreOffer, IndexChanged,
    IndexRead, IndexState, IndexStatus, IpcError, KeybindingChange, KeybindingOverride,
    MarkTreeArgs, NoteBufferArgs, NotePathArgs, OpenVaultArgs, Patch, REFUSED_NAMES_CAP,
    ReadAssetRangeArgs, ReadFileArgs, RebaseBufferArgs, RefusedName, SavePatchArgs, SearchArgs,
    SearchOutcome, SetKeybindingArgs, SetLivePreviewArgs, SwitcherQueryArgs, SwitcherResults,
    TreeStatus, UiSettings, VaultOpened, WriteFileArgs,
};
use crate::settings::{MAX_BINDING_TEXT, Settings};
use crate::startup::{self, PaintEvidenceRejected, Startup, WatcherOutcome};
use crate::switcher::{Switcher, TreePaths};
use crate::tree_stream::{
    FeedConfig, TreeFeed, TreeMarks, TreeSender, standalone_name_walk, standalone_walk,
};
use crate::vault::{Latch, Live, Stage, VaultThread};

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
    external: RwLock<Option<ExternalFn>>,
    tree_status: RwLock<Option<TreeStatusFn>>,
    changed: RwLock<Option<ChangedFn>>,
    closing: RwLock<Option<ClosingFn>>,
    opened: RwLock<Option<OpenedFn>>,
}

/// The installed `vault://opened` sink.
type OpenedFn = Box<dyn Fn(&VaultOpened) + Send + Sync>;

/// The installed `app://closing` sink.
type ClosingFn = Box<dyn Fn(&AppClosing) + Send + Sync>;

/// The installed `index://changed` sink.
type ChangedFn = Box<dyn Fn(&IndexChanged) + Send + Sync>;

/// The installed `tree://status` sink.
type TreeStatusFn = Box<dyn Fn(&TreeStatus) + Send + Sync>;

/// The installed sink. Named because the type is long enough that clippy is right to
/// ask, and because `install` and the field should be describing the same thing.
type StatusFn = Box<dyn Fn(&IndexStatus) + Send + Sync>;

/// The installed `note://external-change` sink.
type ExternalFn = Box<dyn Fn(&ExternalChange) + Send + Sync>;

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

impl Events {
    /// Direct every future `vault://opened` at `sink`.
    pub fn install_opened(&self, sink: impl Fn(&VaultOpened) + Send + Sync + 'static) {
        *self
            .opened
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }

    /// Emit `vault://opened` (task #90, architecture §3.4 "Which vault an event belongs
    /// to"). A no-op with no sink installed, which is the launch vault's case.
    pub fn emit_opened(&self, opened: &VaultOpened) {
        if let Some(sink) = self
            .opened
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(opened);
        }
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

impl Events {
    /// Direct every future `note://external-change` at `sink`.
    pub fn install_external(&self, sink: impl Fn(&ExternalChange) + Send + Sync + 'static) {
        *self
            .external
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }

    /// Announce an external change. Unthrottled; a no-op until a sink is installed.
    pub fn emit_external(&self, change: &ExternalChange) {
        if let Some(sink) = self
            .external
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(change);
        }
    }

    /// Direct every future `index://changed` at `sink`.
    pub fn install_changed(&self, sink: impl Fn(&IndexChanged) + Send + Sync + 'static) {
        *self
            .changed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }

    /// Announce that the index changed. Already throttled by [`ChangeNotifier`]; a
    /// no-op until a sink is installed.
    pub fn emit_changed(&self, changed: &IndexChanged) {
        if let Some(sink) = self
            .changed
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(changed);
        }
    }

    /// Direct every future `app://closing` at `sink`.
    pub fn install_closing(&self, sink: impl Fn(&AppClosing) + Send + Sync + 'static) {
        *self
            .closing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }

    /// Announce that the vault is about to close. Returns whether a sink took it: with
    /// none installed nobody can ack, so the caller must not wait.
    pub fn emit_closing(&self, closing: &AppClosing) -> bool {
        if let Some(sink) = self
            .closing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(closing);
            true
        } else {
            false
        }
    }

    /// Direct every future `tree://status` at `sink`.
    pub fn install_tree_status(&self, sink: impl Fn(&TreeStatus) + Send + Sync + 'static) {
        *self
            .tree_status
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }

    /// Announce that the tree stream stopped. A no-op until a sink is installed.
    pub fn emit_tree_status(&self, status: &TreeStatus) {
        if let Some(sink) = self
            .tree_status
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(status);
        }
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
    index: Arc<ActorCell>,
    /// The consumer of external changes, and the owner of `index://changed`'s throttle
    /// (the save path calls into it after a submit).
    indexer: Arc<Indexer>,
    /// The quick switcher's corpus, frecency and refresh worker.
    switcher: Arc<Switcher>,
    /// Where `write_file` finds the writer to submit a save through, once the actor
    /// thread has produced one (task #90; see `index::IndexHandleSlot`'s doc comment
    /// for the window in which this is still empty).
    index_handle: IndexHandleSlot,
    /// The vault's one full-text search connection (§4.5 obligation 2).
    search: Arc<crate::reads::SearchSlot>,
    /// The file tree's stream and the threads that feed it (ADR-0015).
    tree: Arc<TreeFeed>,
    /// The base registry and the buffer commands (ADR-0016).
    editor: Arc<Editor>,
    /// Decides once whether the open's catch-up runs and which walk feeds it.
    catch_up: Arc<OpenCatchUp>,
    /// Dropping this stops the watcher and the consumer.
    walk: VaultThread,
}

/// How long closing a vault waits for the vault thread (it may be inside a
/// reconciliation, which refuses further reads once the close has begun).
const CLOSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// A test hook called with the new vault id.
#[cfg(test)]
pub(crate) type SpawnHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Where a test finds the open's publisher after a failed open.
#[cfg(test)]
pub(crate) type PublisherProbe = Arc<Mutex<Option<Arc<Publisher>>>>;

/// The fallible steps of `open_vault_with` after `vault://opened`, one per `?`.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailAt {
    Switcher,
    IndexActor,
    VaultThread,
}

/// Where the open vault's index actor lives. Usually filled by `open_vault_with` itself;
/// when a detached actor still holds the same `index.db` (task #96) it is filled later,
/// by the thread that waited for that one to exit. Cancelling and filling serialise on
/// the one mutex, so an actor is never spawned for a vault that has already closed.
#[derive(Default)]
struct ActorCell {
    state: Mutex<CellState>,
}

#[derive(Default)]
struct CellState {
    actor: Option<IndexActor>,
    cancelled: bool,
}

impl ActorCell {
    fn lock(&self) -> std::sync::MutexGuard<'_, CellState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Run `spawn` and keep its actor, unless the cell was cancelled first. Returns the
    /// spawn's error so the caller can say so.
    fn install(
        &self,
        spawn: impl FnOnce() -> std::io::Result<IndexActor>,
    ) -> Result<(), std::io::Error> {
        let mut state = self.lock();
        if state.cancelled {
            return Ok(());
        }
        state.actor = Some(spawn()?);
        Ok(())
    }

    /// No actor will be installed from now on; hands back the one that is.
    fn cancel(&self) -> Option<IndexActor> {
        let mut state = self.lock();
        state.cancelled = true;
        state.actor.take()
    }
}

/// What a failed open owes (task #90): once `vault://opened` has announced the id, an
/// open that returns `Err` must leave nothing that can carry it. Armed from the
/// publisher's creation, disarmed only by [`OpenAttempt::commit`] at the `replace`.
/// Retire first, then cancel any deferred spawn and join the actor for at most
/// `deadline`: a join that finishes means no status can be mid-emit and the actor has
/// closed its handle. Past the deadline the actor is detached into `registry` (task
/// #96): retire still stops every further emit, and the registry keeps a re-open of the
/// same root from starting a second writer until the detached one has exited.
struct OpenAttempt {
    publisher: Arc<Publisher>,
    index: Arc<ActorCell>,
    registry: Arc<DetachedWriters>,
    root: Option<serat_vault::FileIdentity>,
    deadline: std::time::Duration,
    armed: bool,
}

impl OpenAttempt {
    fn commit(mut self) {
        self.armed = false;
    }
}

impl Drop for OpenAttempt {
    fn drop(&mut self) {
        if self.armed {
            self.publisher.retire();
            if let Some(mut actor) = self.index.cancel() {
                actor.join_or_register(self.deadline, self.root, &self.registry);
            }
        }
    }
}

/// Test seams on the open path, so the orderings §3.4 states can be forced.
#[derive(Default, Clone)]
pub(crate) struct OpenHooks {
    /// Called on the vault thread first on every stage, before it is acted on.
    pub(crate) on_stage: Option<Arc<dyn Fn(Stage) + Send + Sync>>,
    /// Called when the open's authoritative walk is about to start, with who runs it:
    /// `"tree"` (the stream's walk) or `"standalone"` (the application's trigger).
    pub(crate) on_walk_start: Option<Arc<dyn Fn(&'static str) + Send + Sync>>,
    /// Fail the open at one of the three fallible steps after `vault://opened`
    /// (task #90's "emit, then a spawn fails"): the seam sits at that step's own `?`.
    #[cfg(test)]
    pub(crate) fail_at: Option<FailAt>,
    /// Called with the new vault id right after `IndexActor::spawn` returned.
    #[cfg(test)]
    pub(crate) after_index_spawn: Option<SpawnHook>,
    /// Make the deferred index-actor spawn fail (task #96 review SF-1).
    #[cfg(test)]
    pub(crate) fail_deferred_spawn: bool,
    /// Mutant switch for task #96's test: ignore the detached-writer registry.
    #[cfg(test)]
    pub(crate) skip_registry_check: bool,
    /// Called with the new vault id right after `VaultThread::spawn_with` returned.
    #[cfg(test)]
    pub(crate) after_vault_spawn: Option<SpawnHook>,
    /// Receives the open's publisher as soon as it exists, so a test can inspect it
    /// after the open has failed and dropped everything else.
    #[cfg(test)]
    pub(crate) publisher_probe: Option<PublisherProbe>,
}

impl OpenVault {
    /// Close the vault's index the way the architecture requires (section 3.1.1 item 6):
    /// the consumer stops first, so nothing submits behind the close, then the handle is
    /// taken from the slot and closed cleanly. Without this every session reads as
    /// unclean at the next open.
    ///
    /// Bounded: the stop flag makes every reconciliation (the vault thread's, and the
    /// detached catch-up's) refuse further reads, and the vault thread is joined with a
    /// deadline. The detached open-time walk, which cannot be interrupted, ends on its
    /// own and finds the indexer stopped. A second close request while this runs
    /// waits for it (`AppState::close_after_flush`, under `opening`); if the process is killed mid-close
    /// the marker is unset and the next open reads it as unclean.
    fn shut_down(mut self, detached: &DetachedWriters) {
        // Before the slot is looked in: an actor that fills it afterwards sees the
        // retirement and closes the handle itself (task #96).
        self.publisher.retire();
        self.indexer.notifier().close();
        self.indexer.stop();
        self.catch_up.cancel();
        self.walk.stop_within(CLOSE_DEADLINE);
        // The actor was never joined here, so one still inside an open or a rebuild
        // outlived its vault with nothing recording it (task #96). It is registered so
        // a re-open of this root waits for it; nothing here waits for it.
        if let Some(mut actor) = self.index.cancel() {
            actor.detach_into(self.vault.identity(), detached);
        }
        self.switcher.shut_down();
        let handle = self
            .index_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle
            && let Err(err) = handle.close()
        {
            eprintln!("serat: the index did not close cleanly: {err}");
        }
    }
}

/// What the vault thread's watcher output is handed to: the tree, then the buffers.
/// Each is shielded, so a panic in one (a sink that panics) cannot take down the thread
/// that owns the watcher.
struct LiveBridge {
    tree: Arc<TreeFeed>,
    editor: Arc<Editor>,
    indexer: Arc<Indexer>,
}

impl Live for LiveBridge {
    fn path_changed(&self, path: &VaultPath) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.tree.path_changed(path);
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.editor.path_changed(path);
        }));
    }

    fn paths_changed(&self, paths: &[VaultPath]) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.indexer.index_paths(paths);
        }));
    }

    fn dir_changed(&self, dir: &VaultPath) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.tree.dir_changed(dir);
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.indexer.dir_changed(dir);
        }));
    }

    fn rescan_required(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.tree.rescan_all();
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.indexer.rescan();
        }));
    }

    fn tick(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.indexer.tick(now)))
            .unwrap_or(None)
    }
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
    /// How long the tree walk and the vault thread wait for the first paint.
    walk_gate: std::time::Duration,
    hooks: OpenHooks,
    /// Index actors detached past their deadline, by `index.db` (task #96).
    detached: Arc<DetachedWriters>,
    /// How long a failed open waits for its index actor (default `CLOSE_DEADLINE`).
    actor_deadline: std::time::Duration,
    /// How long the open's catch-up waits for the index to settle (default
    /// `SETTLE_BOUND`), counted from when the actor exists.
    settle_bound: std::time::Duration,
    /// Serialises opens: two concurrent `open_vault` calls must not both find nothing
    /// to close.
    opening: Mutex<()>,
    open: RwLock<Option<OpenVault>>,
    vaults_opened: AtomicU64,
    /// Backing counters for [`AppState::save_path_outcomes`] (F-382). Process
    /// lifetime, not per-vault: a benchmark diffs two snapshots around its own run
    /// rather than needing them reset on `open_vault`.
    save_handle_missing: AtomicU64,
    save_writer_refused: AtomicU64,
    save_submitted: AtomicU64,
    /// ADR-0017: bytes the `serat-asset` handler and `read_asset_range` may hold in
    /// flight at once, across every concurrent request.
    asset_budget: crate::asset::ByteBudget,
    /// The bounded flush before close (NFR-REL-01): the vault id being waited on, and
    /// whether the editor has acked.
    flush: Mutex<FlushState>,
    flush_cv: std::sync::Condvar,
    window_destroyed: std::sync::atomic::AtomicBool,
}

/// The bound on the wait for `flush_ack` (`contracts/ipc.rs::CLOSING_FLUSH_DEADLINE_MS`).
pub const CLOSING_FLUSH_DEADLINE: std::time::Duration =
    std::time::Duration::from_millis(crate::ipc::CLOSING_FLUSH_DEADLINE_MS);

/// The wait in progress, if any: set only while `flush_ack` can end it. A close and a
/// vault switch are serialised by `opening`, not by this.
#[derive(Default)]
struct FlushState {
    wait: Option<FlushWait>,
}

/// Clears [`FlushState`] on drop, panic included.
struct CloseGuard<'a>(&'a AppState);

impl Drop for CloseGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .0
            .flush
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = FlushState::default();
        self.0.flush_cv.notify_all();
    }
}

/// One bounded wait for `flush_ack`.
struct FlushWait {
    vault_id: String,
    acked: bool,
}

/// How a [`AppState::close_after_flush`] call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    /// No vault was open, or no note is open in the editor (nothing to flush): closed
    /// at once.
    Immediate,
    /// The editor acked; the vault was closed.
    Acked,
    /// No ack inside the deadline; the vault was closed anyway.
    TimedOut,
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
            flush: Mutex::new(FlushState::default()),
            flush_cv: std::sync::Condvar::new(),
            window_destroyed: std::sync::atomic::AtomicBool::new(false),
            boot,
            settings: Mutex::new(loaded.settings),
            settings_path: loaded.path,
            walk_gate: WALK_GATE_TIMEOUT,
            hooks: OpenHooks::default(),
            detached: Arc::default(),
            actor_deadline: CLOSE_DEADLINE,
            settle_bound: SETTLE_BOUND,
            opening: Mutex::new(()),
            open: RwLock::new(None),
            vaults_opened: AtomicU64::new(0),
            save_handle_missing: AtomicU64::new(0),
            save_writer_refused: AtomicU64::new(0),
            save_submitted: AtomicU64::new(0),
            asset_budget: crate::asset::ByteBudget::new(crate::asset::IN_FLIGHT_BYTES),
        };
        (state, loaded.problem)
    }

    /// As [`AppState::new`], with settings supplied rather than loaded. For tests.
    #[must_use]
    pub fn with_settings(startup: Arc<Startup>, boot: Boot, settings: Settings) -> Self {
        Self {
            startup,
            events: Arc::new(Events::default()),
            flush: Mutex::new(FlushState::default()),
            flush_cv: std::sync::Condvar::new(),
            window_destroyed: std::sync::atomic::AtomicBool::new(false),
            boot,
            settings: Mutex::new(settings),
            settings_path: None,
            walk_gate: WALK_GATE_TIMEOUT,
            hooks: OpenHooks::default(),
            detached: Arc::default(),
            actor_deadline: CLOSE_DEADLINE,
            settle_bound: SETTLE_BOUND,
            opening: Mutex::new(()),
            open: RwLock::new(None),
            vaults_opened: AtomicU64::new(0),
            save_handle_missing: AtomicU64::new(0),
            save_writer_refused: AtomicU64::new(0),
            save_submitted: AtomicU64::new(0),
            asset_budget: crate::asset::ByteBudget::new(crate::asset::IN_FLIGHT_BYTES),
        }
    }

    /// How long the tree walk and the vault thread wait for the first paint before
    /// walking anyway (default five seconds). For tests and harnesses that never paint.
    #[must_use]
    pub fn with_walk_gate(mut self, gate: std::time::Duration) -> Self {
        self.walk_gate = gate;
        self
    }

    /// A shorter wait for a failed open's index actor, so a test can detach one.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_actor_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.actor_deadline = deadline;
        self
    }

    /// A shorter settle bound, so a test can outlast it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_settle_bound(mut self, bound: std::time::Duration) -> Self {
        self.settle_bound = bound;
        self
    }

    /// Share another state's detached-writer registry: the registry is process-wide in
    /// effect (one `index.db`, one writer), and a test needs a second state to re-open
    /// without the first one's failure seam.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_detached(mut self, detached: Arc<DetachedWriters>) -> Self {
        self.detached = detached;
        self
    }

    /// Whether the open vault's indexer owes a whole-vault rescan, for tests.
    #[cfg(test)]
    pub(crate) fn rescan_owed(&self) -> bool {
        self.open_lock()
            .as_ref()
            .is_some_and(|open| open.indexer.rescan_owed())
    }

    /// Detached index actors still live, for tests.
    #[cfg(test)]
    pub(crate) fn detached_writers(&self) -> Arc<DetachedWriters> {
        Arc::clone(&self.detached)
    }

    /// Install test seams on the open path.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_open_hooks(mut self, hooks: OpenHooks) -> Self {
        self.hooks = hooks;
        self
    }

    // -- commands -----------------------------------------------------------

    /// The test seam for one fallible step of the open.
    #[cfg(test)]
    fn inject(&self, at: FailAt, root: &std::path::Path) -> Result<(), IpcError> {
        if self.hooks.fail_at == Some(at) {
            return Err(IpcError::Io {
                path: root.display().to_string(),
                message: format!("injected failure at {at:?} after vault://opened"),
            });
        }
        Ok(())
    }

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
        let opening = self.lock_opening();
        let requested = PathBuf::from(&args.root);
        let vault = Arc::new(
            Vault::open(&requested).map_err(|_| IpcError::VaultNotFound { path: args.root })?,
        );
        let root = vault.root().to_path_buf();

        // The new root is valid; only now is the old vault closed, and *before* the new
        // one's actor, watcher and consumer exist. Two writers on one `index.db` (the
        // same root reopened), or the old close setting `clean_close` after the new
        // open cleared it, are the failures this order rules out. The editor gets its
        // bounded flush first (NFR-REL-01): this runs off the webview's thread.
        self.close_after_flush(&opening, CLOSING_FLUSH_DEADLINE);

        let vault_id = format!(
            "v{}",
            self.vaults_opened.fetch_add(1, Ordering::Relaxed) + 1
        );
        // The ruled point (architecture §3.4 "Which vault an event belongs to"): after the
        // old vault is closed and the new id is minted, before anything that can carry
        // that id exists. The three steps below can still fail through `?`; the front end
        // then treats the window as having no vault open (see `VAULT_OPENED_EVENT`).
        self.events.emit_opened(&VaultOpened {
            root: root.display().to_string(),
            vault_id: vault_id.clone(),
            index: IndexState::Opening,
        });
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
        #[cfg(test)]
        if let Some(probe) = &self.hooks.publisher_probe {
            *probe.lock().expect("probe") = Some(Arc::clone(&publisher));
        }
        let index_cell = Arc::new(ActorCell::default());
        let attempt = OpenAttempt {
            publisher: Arc::clone(&publisher),
            index: Arc::clone(&index_cell),
            registry: Arc::clone(&self.detached),
            root: vault.identity(),
            deadline: self.actor_deadline,
            armed: true,
        };

        // §3.4's first `t_usable` precondition: the vault is not usable until the open
        // check has finished and the index state is *known*. The actor tells us by
        // publishing anything other than `Opening`; `Rebuilding` and `Unavailable`
        // settle it as much as `Ready` does.
        let settled = Arc::clone(&self.startup);
        // ADR-0001: one writer per `index.db`. If an actor detached by an earlier close
        // or failed open still holds this root's index, the vault opens without one --
        // the publisher stays at `Opening` -- and a thread starts it once that one has
        // exited. Nothing here waits, and `opening` is not held across the wait. The
        // catch-up gate's settle clock does not start until that actor exists.
        #[cfg(test)]
        let skip_check = self.hooks.skip_registry_check;
        #[cfg(not(test))]
        let skip_check = false;
        let blocker = if skip_check {
            None
        } else {
            self.detached.live(vault.identity())
        };
        let catch_up_gate = Arc::new(if blocker.is_some() {
            OpenCatchUp::awaiting_actor(self.settle_bound, TREE_GRACE)
        } else {
            OpenCatchUp::new(self.settle_bound, TREE_GRACE)
        });
        let publishing = Arc::clone(&catch_up_gate);
        let tree_paths = Arc::new(TreePaths::default());
        #[cfg(test)]
        self.inject(FailAt::Switcher, &root)?;
        let switcher = Switcher::new(
            Arc::clone(&vault),
            serat_index::index_path(&index_dir_for(&root)),
            Arc::clone(&tree_paths),
        )
        .map_err(|err| IpcError::Io {
            path: root.display().to_string(),
            message: format!("could not start the switcher thread: {err}"),
        })?;
        let observing = Arc::clone(&switcher);
        publisher.on_publish(move |state| {
            publishing.index_published(state);
            if !matches!(state, IndexState::Opening) {
                settled.mark_index_settled();
            }
            observing.index_state_changed(matches!(state, IndexState::Ready { .. }));
        });

        let epoch = crate::index::HandleEpoch::default();
        #[cfg(test)]
        self.inject(FailAt::IndexActor, &root)?;
        let index_handle: IndexHandleSlot = Arc::new(Mutex::new(None));
        if let Some(blocker) = blocker {
            let (cell, publisher, slot, epoch, gate) = (
                Arc::clone(&index_cell),
                Arc::clone(&publisher),
                Arc::clone(&index_handle),
                Arc::clone(&epoch),
                Arc::clone(&catch_up_gate),
            );
            let (deferred_root, dir) = (root.clone(), index_dir_for(&root));
            #[cfg(test)]
            let fail_spawn = self.hooks.fail_deferred_spawn;
            let unavailable = Arc::clone(&publisher);
            std::thread::Builder::new()
                .name("serat-index-deferred".to_owned())
                .spawn(move || {
                    while !blocker.done() {
                        if cell.lock().cancelled {
                            gate.actor_started();
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let unavailable_path = dir.display().to_string();
                    let installed = cell.install(|| {
                        #[cfg(test)]
                        if fail_spawn {
                            return Err(std::io::Error::other("injected spawn failure"));
                        }
                        IndexActor::spawn_into(slot, publisher, opener, deferred_root, dir, epoch)
                    });
                    // The vault is already open here, so a failed spawn must be seen:
                    // left alone the index would read `Opening` for the whole session
                    // (goal 5).
                    if let Err(err) = installed {
                        eprintln!("serat: the deferred index thread could not start: {err}");
                        unavailable.publish(IndexState::Unavailable {
                            message: format!("could not start the index thread: {err}"),
                            path: unavailable_path,
                        });
                    }
                    // Now, not before: the claimants' settle clocks start here, whether
                    // or not the spawn succeeded (a failed one then times out as usual).
                    gate.actor_started();
                })
                .map_err(|err| IpcError::Io {
                    path: root.display().to_string(),
                    message: format!("could not start the index thread: {err}"),
                })?;
        } else {
            let spawned = IndexActor::spawn_into(
                Arc::clone(&index_handle),
                Arc::clone(&publisher),
                opener,
                root.clone(),
                index_dir_for(&root),
                Arc::clone(&epoch),
            );
            // Immediately after the spawn, before its `?`, so no later statement can
            // sit between the thread starting and this point (the ordering test's hold).
            #[cfg(test)]
            if let Some(hook) = &self.hooks.after_index_spawn {
                hook(&vault_id);
            }
            let index = spawned.map_err(|err| IpcError::Io {
                path: root.display().to_string(),
                message: format!("could not start the index thread: {err}"),
            })?;
            let _ = index_cell.install(|| Ok(index));
        }

        // §3.4 puts the walk off the critical path; measured, being on another thread
        // is not enough to achieve that (see `crate::vault`). The tree's gate holds the
        // walk until the shell has painted *and* the watcher has settled -- so nothing
        // changing in the meantime is missed (§3.4's ordering rule).
        let startup = Arc::clone(&self.startup);
        let tree_gate = Arc::clone(&self.startup);
        let gate_timeout = self.walk_gate;
        let watcher_settled = Arc::new(Latch::default());
        let gate_latch = Arc::clone(&watcher_settled);
        let ready = Arc::clone(&publisher);
        let events = Arc::clone(&self.events);
        let stopped_id = vault_id.clone();
        // The gate: the first paint, then the watcher (section 3.4's order). A false
        // wait on the watcher is recorded, not swallowed: the walk then starts without
        // the watcher's guarantee.
        // Returns whether the first paint arrived: the trigger's grace for a tree is
        // pointless when it did not (a headless run, a window that never painted).
        let gate: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            let painted = tree_gate.wait_for_first_paint(gate_timeout);
            if !gate_latch.wait(WALK_GATE_TIMEOUT) {
                eprintln!(
                    "serat: the watcher was not registered after {WALK_GATE_TIMEOUT:?}; \
                     the walk starts without it"
                );
            }
            painted
        });
        let tree_gate_fn = Arc::clone(&gate);

        let notifier = Arc::new(ChangeNotifier::new(
            vault_id.clone(),
            Arc::clone(&self.events),
        ));
        let refreshing = Arc::clone(&switcher);
        notifier.on_change(move || refreshing.index_changed());
        let indexer = Arc::new(Indexer::new(
            Arc::clone(&vault),
            Arc::clone(&index_handle),
            notifier,
        ));
        // A claim that gives up because the index never settled owes a rescan rather than
        // dropping the open's catch-up (task #96 review B1).
        let owing = Arc::clone(&indexer);
        catch_up_gate.on_unsettled(move || owing.owe_rescan());
        let walk_start = Arc::clone(&indexer);
        let walk_failed = Arc::clone(&indexer);
        let catch_up = Arc::clone(&indexer);
        let walk_failed_on_spawn = Arc::clone(&indexer);
        let walk_startup = Arc::clone(&self.startup);
        let walk_done = Arc::clone(&publisher);
        let walk_refusals = vault.refusals().clone();
        let start_hook = self.hooks.on_walk_start.clone();
        let stage_hook = self.hooks.on_stage.clone();
        let on_start: Arc<dyn Fn(&'static str) + Send + Sync> = Arc::new(move |who| {
            if let Some(hook) = &start_hook {
                hook(who);
            }
            walk_start.walk_started();
        });
        let on_failed: Arc<dyn Fn() + Send + Sync> = Arc::new(move || walk_failed.walk_failed());
        let stamp_guard = Arc::clone(&indexer);
        let on_complete: Arc<dyn Fn(u32) + Send + Sync> = Arc::new(move |files| {
            // A walk that outlives its vault (a switch, an exit) must not stamp the
            // next vault's trace.
            if stamp_guard.stopped() {
                return;
            }
            walk_startup.mark_walk_complete(files);
            // The walk clears the log and refills it, so the count is final here
            // and nothing else announces it. Only when there is something to say.
            if !walk_refusals.is_empty() {
                walk_done.re_emit();
            }
        });

        let (tree_attach, tree_detach, tree_claim) = (
            Arc::clone(&catch_up_gate),
            Arc::clone(&catch_up_gate),
            Arc::clone(&catch_up_gate),
        );
        let (tree_start, tree_failed, tree_complete) = (
            Arc::clone(&on_start),
            Arc::clone(&on_failed),
            Arc::clone(&on_complete),
        );
        let tree = TreeFeed::new(FeedConfig {
            vault: Arc::clone(&vault),
            index_db: serat_index::index_path(&index_dir_for(&root)),
            index_ready: Box::new(move || matches!(ready.status().state, IndexState::Ready { .. })),
            paths: tree_paths,
            gate: Arc::new(move || {
                tree_gate_fn();
            }),
            attach: Box::new(move || tree_attach.attach_tree()),
            detach: Box::new(move || tree_detach.detach_tree()),
            claim: Box::new(move || tree_claim.claim(true)),
            on_walk_failed: Box::new(move || tree_failed()),
            on_walk_start: Box::new(move || tree_start("tree")),
            on_walked: Box::new(move |entries| {
                // ADR-0015 section 2: once, on a thread of its own, from the one walk.
                let indexer = Arc::clone(&catch_up);
                let spawned = std::thread::Builder::new()
                    .name("serat-catch-up".to_owned())
                    .spawn(move || indexer.catch_up(&entries));
                if let Err(err) = spawned {
                    // The journal opened for this walk must not stay open: close it
                    // and owe a rescan, which is what the catch-up would have been.
                    eprintln!("serat: the open's catch-up could not start: {err}");
                    walk_failed_on_spawn.walk_failed();
                }
            }),
            on_walk_complete: Box::new(move |files| tree_complete(files)),
            on_stopped: Box::new(move || {
                events.emit_tree_status(&TreeStatus {
                    vault_id: stopped_id.clone(),
                    stopped: true,
                });
            }),
        });
        let editor = Editor::new(
            Arc::clone(&vault),
            vault_id.clone(),
            Arc::clone(&self.events),
        );
        let live: Arc<dyn Live> = Arc::new(LiveBridge {
            tree: Arc::clone(&tree),
            editor: Arc::clone(&editor),
            indexer: Arc::clone(&indexer),
        });
        // ADR-0015 section 2 as amended (task #78): the catch-up window starts at the
        // wall-clock time taken immediately before the watcher is armed.
        indexer.window_opened(serat_index::rebuild::now_ms());
        #[cfg(test)]
        self.inject(FailAt::VaultThread, &root)?;
        let spawned = VaultThread::spawn_with(
            Arc::clone(&vault),
            move |stage| {
                if let Some(hook) = &stage_hook {
                    hook(stage);
                }
                match stage {
                    // Half of "vault usable": §3.4 will not call a vault usable while
                    // nothing is watching it, because every edit made in that window is
                    // lost silently.
                    Stage::WatcherRegistered => {
                        startup.mark_watcher(WatcherOutcome::Registered);
                        watcher_settled.set();
                    }
                    Stage::WatcherFailed => {
                        startup.mark_watcher(WatcherOutcome::Failed);
                        watcher_settled.set();
                    }
                }
            },
            Some(live),
        );
        #[cfg(test)]
        if let Some(hook) = &self.hooks.after_vault_spawn {
            hook(&vault_id);
        }
        let walk = spawned.map_err(|err| IpcError::Io {
            path: root.display().to_string(),
            message: format!("could not start the vault thread: {err}"),
        })?;

        // The open's own trigger (ADR-0015 section 2, FR-VLT-04): a vault reconciles what
        // changed while it was closed whether or not anything ever calls `open_tree`. It
        // stands aside for a tree stream, whose walk then serves both; the claim is
        // granted once.
        let bench_walk = self.boot.exit_after_walk;
        {
            let (vault, gate, claim) = (
                Arc::clone(&vault),
                Arc::clone(&gate),
                Arc::clone(&catch_up_gate),
            );
            let (indexer, on_start, on_failed, on_complete) = (
                Arc::clone(&indexer),
                Arc::clone(&on_start),
                Arc::clone(&on_failed),
                Arc::clone(&on_complete),
            );
            let spawned = std::thread::Builder::new()
                .name("serat-open-catch-up".to_owned())
                .spawn(move || {
                    let painted = gate();
                    if !claim.claim_standalone(painted, bench_walk) {
                        // Refused. If that is because the index was rebuilt, nothing
                        // else walks; the benchmark's `--exit-after-walk` still needs
                        // the walk mark (FR-VLT-01's baseline), so it gets a
                        // name-only walk, which feeds no catch-up.
                        if bench_walk
                            && !claim.claimed()
                            && !claim.tree_attached()
                            && let Some(files) = standalone_name_walk(&vault)
                        {
                            on_complete(files);
                        }
                        return;
                    }
                    let walked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        standalone_walk(&vault, &|| on_start("standalone"), &*on_failed)
                    }));
                    match walked {
                        Ok(Some((files, entries))) => {
                            on_complete(files);
                            indexer.catch_up(&entries);
                        }
                        // `standalone_walk` reported its own failure; a panic did not.
                        Ok(None) => {}
                        Err(_) => on_failed(),
                    }
                });
            if let Err(err) = spawned {
                eprintln!("serat: the open's catch-up could not start: {err}");
            }
        }

        self.startup.set_vault(Some(root.display().to_string()));

        // Read the state before publishing the handle, so the value returned is the one
        // the actor had at this moment rather than one that raced it.
        let index_state = publisher.status().state;

        let previous = self.open_lock_mut().replace(OpenVault {
            vault,
            publisher,
            index: index_cell,
            indexer,
            switcher,
            index_handle,
            search: Arc::new(crate::reads::SearchSlot::new(epoch)),
            tree,
            editor,
            catch_up: catch_up_gate,
            walk,
        });
        // The vault that was open is closed cleanly, outside the lock.
        if let Some(previous) = previous {
            previous.shut_down(&self.detached);
        }
        attempt.commit();

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

    /// `asset_info`. FR-BIN-01, FR-BIN-11, FR-BIN-12. A vault operation, not an
    /// index read: it works before the index is `Ready`.
    ///
    /// # Errors
    ///
    /// [`IpcError::PathEscapesVault`] for a spelling `VaultPath` refuses,
    /// [`IpcError::VaultNotFound`] with no vault open, [`IpcError::NotFound`] for
    /// everything else (a refusal carries no detail).
    pub fn asset_info(&self, args: AssetArgs) -> Result<AssetInfo, IpcError> {
        let path = Self::vault_path(&args.path)?;
        let vault = self.vault()?;
        crate::asset::info(&vault, &path)
    }

    /// `read_asset_range`, ADR-0017 section Fallback. Raw bytes, at most 1,000 KiB.
    ///
    /// # Errors
    ///
    /// As [`AppState::asset_info`], plus [`IpcError::Io`] if the read fails.
    pub fn read_asset_range(&self, args: ReadAssetRangeArgs) -> Result<Vec<u8>, IpcError> {
        let path = Self::vault_path(&args.path)?;
        let vault = self.vault()?;
        crate::asset::read_range(
            &vault,
            &self.asset_budget,
            &path,
            args.start,
            args.len,
            args.if_match.as_deref(),
        )
    }

    /// The `serat-asset` scheme's entry point: the open vault, if any, answered
    /// by [`crate::asset::handle`]. No vault open is a 404, never a panic.
    pub(crate) fn serve_asset(
        &self,
        platform: crate::nav_guard::Platform,
        request: &tauri::http::Request<Vec<u8>>,
    ) -> crate::asset::Reply {
        let vault = self.vault().ok();
        crate::asset::handle(vault.as_deref(), &self.asset_budget, platform, request)
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

        // The open-vault lock is held shared for the write and the submit, so that
        // `close_vault` (which takes it exclusively) cannot close the index between a
        // write that reached disk and its submit: the last save is always submitted
        // before the clean-close marker is set.
        let guard = self.open_lock();
        let open = guard.as_ref().ok_or_else(Self::not_open)?;
        let vault = &open.vault;

        // Rule 4 — the optimistic lock of §6.1 step 2, checked by the vault under its
        // per-path lock so no other in-process writer can land between the check and
        // the rename (ADR-0016 §2). `None` asserts the file does not exist.
        let expect = base_hash.map_or(serat_vault::Expect::Absent, serat_vault::Expect::Hash);
        let wrote = vault
            .write_atomic_if(&path, expect, args.text.as_bytes())
            .map_err(from_vault_error)?;

        // §3.1.1's save path, steps 2-4: submit the note to the index writer with no
        // watcher and no debounce. Fire-and-forget by construction -- `submit_to_index`
        // never calls `flush()` (§2.2: a barrier here would put the index back on the
        // keystroke path, design goal 2) -- so this can only add work, never latency
        // or a new way for `write_file` to fail. "Saved" and "indexed" stay two
        // different facts: the command answers for the vault write below, and
        // `index.changed` is how a caller learns the rest.
        self.submit_to_index(open, &path);

        // Hash, mtime and size come from the write itself (ADR-0015 §2): the mtime was
        // statted right after the rename under the path lock, never from a later read
        // that an external write could have moved past the hash.
        Ok(FileWritten {
            path: path.to_string(),
            hash: wrote.hash.to_hex(),
            mtime_ms: wrote.mtime_ms,
            bytes: wrote.size,
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
                wal_pinned_bytes: None,
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
        let vault = self.vault()?;
        crate::gitignore::append(&vault)
    }

    /// `decline_gitignore_offer`. Writes only inside `.serat/`.
    ///
    /// # Errors
    ///
    /// As [`AppState::append_gitignore_entry`].
    pub fn decline_gitignore_offer(&self) -> Result<(), IpcError> {
        let vault = self.vault()?;
        crate::gitignore::decline(&vault)
    }

    /// `search`. FR-NAV-02, FR-STR-03, FR-BIN-02. Answers `NotReady` rather than
    /// waiting (rule 1). Limits are the core's: `Reader::search` clamps (#15 ruling).
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] with no vault open.
    pub fn search(&self, args: SearchArgs) -> Result<IndexRead<SearchOutcome>, IpcError> {
        let (root, state, search, vault) = self.read_context()?;
        // After the statement and outside SQLite; `Vault::stat` keeps containment.
        // Bounded by a deadline, and abandoned if a newer search has begun.
        let raw = |path: &str| match Self::vault_path(path)
            .ok()
            .and_then(|path| vault.stat(&path).ok())
        {
            Some((mtime, size)) => crate::reads::StatResult::Got(mtime, size),
            None => crate::reads::StatResult::Failed,
        };
        Ok(crate::reads::read_when_ready(
            state,
            &root,
            Some(&search),
            |reader| {
                reader
                    .search(&args.query, args.offset, args.limit)
                    .map(|outcome| {
                        let before = search.generation.load(std::sync::atomic::Ordering::SeqCst);
                        let stat = crate::reads::budgeted_stat(
                            crate::reads::STAT_DEADLINE,
                            || {
                                search.generation.load(std::sync::atomic::Ordering::SeqCst)
                                    != before
                            },
                            raw,
                        );
                        crate::reads::map_outcome(outcome, &stat)
                    })
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
        let (root, state, _, _) = self.read_context()?;
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
    ) -> Result<
        (
            PathBuf,
            IndexState,
            Arc<crate::reads::SearchSlot>,
            Arc<Vault>,
        ),
        IpcError,
    > {
        let guard = self.open_lock();
        let open = guard.as_ref().ok_or_else(|| IpcError::VaultNotFound {
            path: String::new(),
        })?;
        Ok((
            open.vault.root().to_path_buf(),
            open.publisher.status().state,
            Arc::clone(&open.search),
            Arc::clone(&open.vault),
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

    /// `open_tree`. FR-VLT-01, FR-VLT-04. See [`crate::tree_stream`].
    ///
    /// Replaces any earlier stream and starts this one from the root. Returns at once;
    /// the index read, the walk and everything after them run on threads of their own.
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] with no vault open; [`IpcError::Io`] if the
    /// operating system refuses a thread.
    pub fn open_tree(&self, send: TreeSender) -> Result<(), IpcError> {
        self.tree()?.open(send).map_err(|err| IpcError::Io {
            path: String::new(),
            message: format!("could not start the tree thread: {err}"),
        })
    }

    /// `mark_tree`. NFR-PRF-01, ADR-0015 §3. A mark the stream has not earned (the
    /// front end claims a paint of a message Rust never sent) is counted and ignored;
    /// see [`TreeFeed::mark`]. With no vault open there is nothing to mark.
    pub fn mark_tree(&self, args: &MarkTreeArgs) {
        if let Ok(tree) = self.tree() {
            tree.mark(args, self.startup.elapsed_ms());
        }
    }

    /// The tree marks recorded for the open vault. `None` with no vault open.
    #[must_use]
    pub fn tree_marks(&self) -> Option<TreeMarks> {
        self.tree().ok().map(|tree| tree.marks())
    }

    /// `note_opened`. FR-VLT-05. See [`Editor::note_opened`].
    ///
    /// # Errors
    ///
    /// As [`Editor::note_opened`], plus [`IpcError::VaultNotFound`] with no vault open.
    pub fn note_opened(&self, args: &NoteBufferArgs) -> Result<(), IpcError> {
        self.editor()?.note_opened(args)?;
        // A note counts as opened when an editor pane reports it (architecture 9.2):
        // this is the frecency signal for the switcher.
        if let Some(open) = self.open_lock().as_ref() {
            open.switcher.note_opened(&args.path);
        }
        Ok(())
    }

    /// `switcher_query`. FR-NAV-01, FR-EDT-03, NFR-PRF-06. Never fails and never waits
    /// for the index: before it is `Ready` the answer comes from the tree's paths
    /// (`corpus: Tree`), and with no vault open it is empty.
    #[must_use]
    pub fn switcher_query(&self, args: &SwitcherQueryArgs) -> SwitcherResults {
        let switcher = self
            .open_lock()
            .as_ref()
            .map(|open| Arc::clone(&open.switcher));
        match switcher {
            Some(switcher) => switcher.query(args),
            None => SwitcherResults {
                seq: args.seq,
                corpus: crate::ipc::SwitcherCorpus::Tree,
                items: Vec::new(),
                discarded: false,
            },
        }
    }

    /// `note_closed`. Nothing to release with no vault open.
    pub fn note_closed(&self, args: &NoteBufferArgs) {
        if let Ok(editor) = self.editor() {
            editor.note_closed(args);
        }
    }

    /// `save_patch`. FR-VLT-03, NFR-REL-01, C-04; ADR-0016 §2.
    ///
    /// The write is [`Vault::write_patch`]. A save is a write like `write_file`'s, so it
    /// is submitted to the index the same way afterwards (§3.1.1).
    ///
    /// # Errors
    ///
    /// As [`Editor::save_patch`], plus [`IpcError::VaultNotFound`] with no vault open.
    pub fn save_patch(&self, args: &SavePatchArgs) -> Result<FileWritten, IpcError> {
        // Shared for the write and the submit; see `write_file`.
        let guard = self.open_lock();
        let open = guard.as_ref().ok_or_else(Self::not_open)?;
        let (path, written) = open.editor.save_patch(args)?;
        self.submit_to_index(open, &path);
        Ok(written)
    }

    /// `request_external_diff`. FR-VLT-05. See [`Editor::request_external_diff`].
    ///
    /// # Errors
    ///
    /// As [`Editor::request_external_diff`], plus [`IpcError::VaultNotFound`] with no
    /// vault open.
    pub fn request_external_diff(
        &self,
        args: &NoteBufferArgs,
    ) -> Result<Option<ExternalChange>, IpcError> {
        self.editor()?.request_external_diff(args)
    }

    /// `rebase_buffer`. FR-VLT-05. See [`Editor::rebase_buffer`].
    ///
    /// # Errors
    ///
    /// As [`Editor::rebase_buffer`], plus [`IpcError::VaultNotFound`] with no vault
    /// open.
    pub fn rebase_buffer(&self, args: &RebaseBufferArgs) -> Result<Vec<Patch>, IpcError> {
        self.editor()?.rebase_buffer(args)
    }

    fn tree(&self) -> Result<Arc<TreeFeed>, IpcError> {
        self.open_lock()
            .as_ref()
            .map(|open| Arc::clone(&open.tree))
            .ok_or_else(|| IpcError::VaultNotFound {
                path: String::new(),
            })
    }

    fn editor(&self) -> Result<Arc<Editor>, IpcError> {
        self.open_lock()
            .as_ref()
            .map(|open| Arc::clone(&open.editor))
            .ok_or_else(|| IpcError::VaultNotFound {
                path: String::new(),
            })
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
    fn submit_to_index(&self, open: &OpenVault, path: &VaultPath) {
        let vault = &*open.vault;
        let outcome = {
            let guard = open
                .index_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(handle) = guard.as_ref() else {
                self.save_handle_missing.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let parser = CommonMarkParser::new();
            // The `Err` case is deliberately not propagated to `write_file`'s caller,
            // for the same reason there is no barrier: the command's contract answers
            // for the vault write, not for whether the index heard about it, and a
            // write that reached disk must never be reported as failed because the
            // index writer was busy. It is still counted (F-382), which is the
            // difference between "swallowed" and "unobserved".
            handle.apply(vault, &parser, vault, path)
        };
        match outcome {
            Ok(()) => {
                self.save_submitted.fetch_add(1, Ordering::Relaxed);
                // Architecture 3.1 step 6: every save-path submit is owed an
                // `index://changed` (throttled). The slot's lock is already released.
                open.indexer.notifier().changed();
            }
            Err(_) => {
                self.save_writer_refused.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn not_open() -> IpcError {
        IpcError::VaultNotFound {
            path: String::new(),
        }
    }

    /// Rebuild the open vault's index while it stays open (`rebuild_in_place`): the user
    /// asked for it, or an owner of the index found it unusable. Not a command -- the
    /// contract names `RebuildReason::UserRequested` and no command to ask for it yet.
    ///
    /// **`release`'s obligations (S7, not yet met).** `rebuild_in_place` deletes
    /// `index.db`, which on Windows fails while any other connection holds it. The search
    /// slot is suspended here, but the switcher's snapshot build and the reconciler's
    /// short-lived read connections (`Reconciler::reader`, one per call, on the vault
    /// thread and the catch-up thread) are not. Before this is reachable from a command,
    /// `release` must quiesce both: park the switcher's build and make the consumer
    /// defer, then wait for in-flight reads to finish.
    ///
    /// Publishes `Rebuilding`, then `Ready` (or `Unavailable`). Runs on the caller's
    /// thread, which must not be the webview's. The search connection is suspended first
    /// (`rebuild_in_place`'s `release`, and the slot must not reopen the file before it
    /// is deleted), the handle epoch is bumped for the new handle, and anything the
    /// consumer received meanwhile is kept and indexed afterwards. Saves in the window
    /// find no handle and degrade to the watcher path, as during an open-time rebuild
    /// (F-450).
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] with no vault open; [`IpcError::Io`] if there is no
    /// index handle to rebuild (the open check has not produced one, or failed), or if
    /// the rebuild failed -- in which case the index is `Unavailable` for the rest of the
    /// session.
    pub fn rebuild_index(&self) -> Result<(), IpcError> {
        // TODO(S7): quiesce the switcher build and the reconciler's readers before the
        // delete; until then this is for tests and the Linux/macOS unlink semantics.
        // TODO(S7, task #96): this runs `rebuild_in_place` on the caller's thread,
        // outside the index actor and `ActorCell`, so a close during it registers
        // nothing and a re-open can remove the db under it. Before it is wired to a
        // command it must run on the actor, or hold an exit flag in `DetachedWriters`
        // under this root for its whole duration.
        let (vault, publisher, slot, search, indexer) = {
            let guard = self.open_lock();
            let open = guard.as_ref().ok_or_else(Self::not_open)?;
            (
                Arc::clone(&open.vault),
                Arc::clone(&open.publisher),
                Arc::clone(&open.index_handle),
                Arc::clone(&open.search),
                Arc::clone(&open.indexer),
            )
        };
        let index_dir = index_dir_for(vault.root());
        let handle = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| IpcError::Io {
                path: index_dir.display().to_string(),
                message: "there is no open index to rebuild".to_owned(),
            })?;
        publisher.publish(IndexState::Rebuilding {
            reason: crate::ipc::RebuildReason::UserRequested,
            done: 0,
            total: 0,
            walk_complete: false,
        });
        search.suspend();
        let parser = CommonMarkParser::new();
        let rebuilt = handle.rebuild_in_place(&|| search.close(), &*vault, &parser, vault.root());
        match rebuilt {
            Ok((handle, _report)) => {
                let note_count = handle.note_count().unwrap_or(0);
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
                search.epoch.fetch_add(1, Ordering::SeqCst);
                search.resume();
                publisher.publish(IndexState::Ready { note_count });
                indexer.notifier().changed();
                Ok(())
            }
            Err(err) => {
                // No handle is left (rebuild_in_place's contract): the index is
                // unavailable for the rest of the session and the vault stays editable.
                search.resume();
                publisher.publish(IndexState::Unavailable {
                    message: format!("the index could not be rebuilt: {err}"),
                    path: index_dir.display().to_string(),
                });
                Err(IpcError::Io {
                    path: index_dir.display().to_string(),
                    message: err.to_string(),
                })
            }
        }
    }

    /// Close the open vault, if any: the consumer stops, the index is closed cleanly
    /// (`IndexHandle::close`: queued work applied, WAL checkpointed, clean-close marker
    /// set) and later commands answer `VaultNotFound`. Idempotent.
    ///
    /// Called when another vault is opened, and at exit. Without it every session reads
    /// as unclean at the next open and pays an mtime comparison per file.
    ///
    /// **Ordering.** The open-vault lock is taken exclusively, which waits for every
    /// `write_file`/`save_patch` that holds it shared (each holds it across its write
    /// and its submit), so the last save is submitted before the close. The close itself
    /// runs after the lock is released, on the caller's thread: the application calls
    /// this from a thread of its own, never the webview's, and before the window is
    /// destroyed.
    pub fn close_vault(&self) {
        self.close_vault_now();
    }

    /// The id of the open vault, if any.
    #[must_use]
    pub fn vault_id(&self) -> Option<String> {
        self.open_lock()
            .as_ref()
            .map(|open| open.publisher.status().vault_id)
    }

    /// `flush_ack`: the editor has submitted what it had pending. Ends the wait of
    /// [`AppState::close_after_flush`] early. An ack for any other vault, or with no
    /// wait in progress, is ignored.
    pub fn flush_ack(&self, args: &FlushAckArgs) {
        let mut state = self
            .flush
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(wait) = state.wait.as_mut()
            && wait.vault_id == args.vault_id
        {
            wait.acked = true;
            self.flush_cv.notify_all();
        }
    }

    /// Give the editor a bounded chance to flush, then close the vault (NFR-REL-01).
    ///
    /// With at least one note open it emits `app://closing`, then waits for `flush_ack`
    /// or `deadline`, whichever is first; with none there is nothing to flush and the
    /// close is immediate. Throughout, `save_patch` and `write_file` are accepted
    /// because the vault is still open; only then is it closed. Blocks the caller: run
    /// it on a thread of its own, never the webview's.
    ///
    /// Callers hold `opening` (proved by the guard argument; lock order: `opening`, then
    /// the close wait), so a window close and a vault switch are serialised: the later
    /// one finds the vault already closed and does nothing.
    pub(crate) fn close_after_flush(
        &self,
        _opening: &std::sync::MutexGuard<'_, ()>,
        deadline: std::time::Duration,
    ) -> FlushOutcome {
        // The guard clears the wait however this ends.
        let _guard = CloseGuard(self);
        let pending = self.open_lock().as_ref().map(|open| {
            (
                open.publisher.status().vault_id,
                open.editor.buffers().open_count(),
            )
        });
        let outcome = match pending {
            Some((vault_id, open_notes)) if open_notes > 0 => {
                self.flush
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .wait = Some(FlushWait {
                    vault_id: vault_id.clone(),
                    acked: false,
                });
                // Outside the lock; a panicking sink is treated as no listener.
                let event = AppClosing {
                    vault_id,
                    deadline_ms: u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX),
                };
                let announced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.events.emit_closing(&event)
                }))
                .unwrap_or(false);
                if announced {
                    self.wait_for_ack(deadline)
                } else {
                    FlushOutcome::Immediate
                }
            }
            _ => FlushOutcome::Immediate,
        };
        self.close_vault_now();
        outcome
    }

    fn wait_for_ack(&self, deadline: std::time::Duration) -> FlushOutcome {
        let state = self
            .flush
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (_state, result) = self
            .flush_cv
            .wait_timeout_while(state, deadline, |s| {
                s.wait.as_ref().is_some_and(|w| !w.acked)
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if result.timed_out() {
            eprintln!(
                "serat: the editor did not acknowledge the flush within {deadline:?}; \
                 closing the vault anyway"
            );
            FlushOutcome::TimedOut
        } else {
            FlushOutcome::Acked
        }
    }

    /// The open/switch lock. Lock order (architecture §3.1.1 item 6): this is taken
    /// before the close-wait lock, never the reverse; the window-close thread takes it
    /// too, which serialises a close against a vault switch.
    pub(crate) fn lock_opening(&self) -> std::sync::MutexGuard<'_, ()> {
        self.opening
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Destroy the window once: the first caller to claim it gets `true`.
    pub(crate) fn claim_window_destroy(&self) -> bool {
        !self.window_destroyed.swap(true, Ordering::SeqCst)
    }

    fn close_vault_now(&self) {
        let open = self.open_lock_mut().take();
        if let Some(open) = open {
            self.startup.set_vault(None);
            open.shut_down(&self.detached);
        }
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
        // A generous deadline alone would not be a test: the vault this fixture opens
        // also has a *watcher* running (§3.1.2), which would eventually index the same
        // save through the ordinary filesystem-event path and write the identical row
        // -- same table, same hash, indistinguishable at rest. A first version of this
        // test used a 5s poll and stayed green with `self.submit_to_index(&vault,
        // &path)` in `write_file` commented out, which is exactly the failure this
        // project has a name for (F-13, F-28, F-35, F-50, F-58): the assertion was
        // correct and the setup let the watcher answer for the path under test.
        //
        // The property that tells the two paths apart, by structure and not by time
        // (task #111; this used to give up after 100 ms, under the watcher's 150 ms
        // window, and failed 6 runs in 50 under load because the direct path is no
        // faster than a starved thread): the watcher's batches are consumed on the
        // vault thread, and the stage hook runs on that thread before the consume
        // loop starts. Holding the hook holds the watcher's only route to the index,
        // so any row that appears while it is held came from the save's own submit,
        // and the wait for it can be as long as a loaded host needs.
        // The open's catch-up is excluded by the fresh fixture (a rebuild refuses the
        // standalone claim), not by the hold, which only delays it up to
        // `WALK_GATE_TIMEOUT` (5 s).
        //
        // Mutations: commenting out the `self.submit_to_index(&vault, &path)` call in
        // `write_file` is caught by the `submitted` counter assertion, not by the hold.
        // The hold-specific mutant is `handle.apply` replaced by `Ok(())` inside
        // `submit_to_index`: the counter still moves, no row is written, and the wait
        // times out at 10 s. With that mutant and the hold also removed, the test
        // passes, through the watcher: which is what the hold is for.
        //
        // Reads `.serat/index.db` through rusqlite -- a connection this crate uses
        // nowhere else -- rather than through `serat_index`'s own reader, so a bug in
        // the wiring cannot also hide the assertion meant to catch it (Cargo.toml's
        // dev-dependency comment).
        let (release, held_until) = std::sync::mpsc::channel::<()>();
        let held_until = Mutex::new(held_until);
        let dir = fixture("save-reaches-index");
        let state = state_with(OpenHooks {
            on_stage: Some(Arc::new(move |stage| {
                if stage == Stage::WatcherRegistered {
                    // Returns when the sender is dropped.
                    let _ = held_until.lock().expect("hold").recv();
                }
            })),
            on_walk_start: None,
            ..OpenHooks::default()
        });
        // Re-bound after `state` so that, on a failed assertion, the sender drops (and
        // frees the held vault thread) before `state` does: locals drop in reverse
        // order, and `state`'s drop joins that thread.
        let release = release;
        state.open_vault(open_args(&dir)).expect("open");

        // Wait for the fixture's own initial rebuild to settle before writing: on a
        // vault this small the rebuild can still be reading `serat.md` when
        // `write_file` overwrites it, and then the rebuild itself indexes the new bytes
        // and the test cannot tell that from `submit_to_index` having done it. `Ready`
        // is published only once the handle slot is filled (#119), so no settling sleep
        // follows it.
        wait_ready(&state);

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
        let mut seen_hash = None;
        wait_until(
            "the save's own row in .serat/index.db (watcher held)",
            || {
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
                        return true;
                    }
                }
                false
            },
        );
        assert_eq!(
            seen_hash.as_deref(),
            Some(written.hash.as_str()),
            "the row carries the hash write_file itself returned"
        );

        // Let the watcher through; the close must not wait on a held vault thread.
        drop(release);
        state.close_vault();
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

    /// Hold the first search on `slot`'s connection after its statement is prepared.
    /// Returns (signalled when parked, send to release).
    fn park_first_search(
        slot: &crate::reads::SearchSlot,
        path: &Path,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (parked_tx, parked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let first = AtomicBool::new(true);
        slot.get(path)
            .expect("search connection")
            .set_after_prepare(move || {
                if first.swap(false, Ordering::SeqCst) {
                    parked_tx.send(()).expect("parked");
                    release_rx.lock().expect("lock").recv().expect("release");
                }
            });
        (parked_rx, release_tx)
    }

    fn zebra(state: &AppState) -> IndexRead<SearchOutcome> {
        state
            .search(SearchArgs {
                query: "zebra".into(),
                offset: 0,
                limit: 10,
            })
            .expect("search")
    }

    fn is_superseded(r: &IndexRead<SearchOutcome>) -> bool {
        matches!(
            r,
            IndexRead::Ready {
                value: SearchOutcome::Superseded
            }
        )
    }

    fn is_hit(r: &IndexRead<SearchOutcome>) -> bool {
        matches!(
            r,
            IndexRead::Ready {
                value: SearchOutcome::Hits { items }
            } if items.len() == 1
        )
    }

    fn wait_generation(slot: &crate::reads::SearchSlot, at_least: u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while slot.generation.load(std::sync::atomic::Ordering::SeqCst) < at_least {
            assert!(Instant::now() < deadline, "the search never started");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// FR-NAV-02, NFR-PRF-02, architecture 4.5. Nothing else fails if `search` stops
    /// routing through `Reader::with_search`: this does (task #52).
    #[test]
    fn a_second_search_supersedes_the_first_and_backlinks_do_not_wait() {
        let dir = fixture("supersede");
        std::fs::write(dir.join("alpha.md"), "# Alpha\n\nsee [[beta]]\n").expect("fixture");
        std::fs::write(dir.join("beta.md"), "# Beta\n\nzebra stripes\n").expect("fixture");
        let state = Arc::new(state(&dir));
        wait_ready(&state);
        let (root, _, slot, _) = state.read_context().expect("context");
        let path = serat_index::index_path(&crate::index::index_dir_for(&root));
        let (parked, release) = park_first_search(&slot, &path);

        let first = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || zebra(&state))
        };
        parked
            .recv_timeout(Duration::from_secs(30))
            .expect("A parks");

        // A holds the search lock mid-statement; backlinks use the pool, not that lock.
        let started = Instant::now();
        let links = state
            .backlinks(NotePathArgs {
                path: "beta.md".into(),
            })
            .expect("backlinks");
        assert!(matches!(links, IndexRead::Ready { .. }), "{links:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "backlinks waited on the search: {:?}",
            started.elapsed()
        );

        let second = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || zebra(&state))
        };
        // A bumped the generation to 1; B's bump makes it 2. Then A may finish.
        wait_generation(&slot, 2);
        release.send(()).expect("release");
        assert!(is_superseded(&first.join().expect("A")));
        assert!(is_hit(&second.join().expect("B")));
    }

    #[test]
    fn a_search_on_a_reopened_connection_supersedes_one_on_the_old() {
        let dir = fixture("supersede-reopen");
        std::fs::write(dir.join("beta.md"), "# Beta\n\nzebra stripes\n").expect("fixture");
        let state = Arc::new(state(&dir));
        wait_ready(&state);
        let (root, _, slot, _) = state.read_context().expect("context");
        let path = serat_index::index_path(&crate::index::index_dir_for(&root));
        let (parked, release) = park_first_search(&slot, &path);

        let first = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || zebra(&state))
        };
        parked
            .recv_timeout(Duration::from_secs(30))
            .expect("A parks");
        // A new handle arrives: the next search is on a different connection.
        slot.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // B runs on its own connection, so it does not need A released to finish.
        assert!(is_hit(&zebra(&state)));
        wait_generation(&slot, 2);
        release.send(()).expect("release");
        assert!(is_superseded(&first.join().expect("A")));
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

    // -- task #18c: the tree stream and the editor-buffer commands, through AppState --

    type TreeLog = Arc<Mutex<Vec<crate::ipc::TreeMessage>>>;

    fn collecting_sender() -> (TreeSender, TreeLog) {
        let log: TreeLog = Arc::default();
        let sink = Arc::clone(&log);
        (
            Arc::new(move |message| {
                sink.lock().expect("log").push(message);
                true
            }),
            log,
        )
    }

    fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn settled(log: &TreeLog) -> bool {
        log.lock()
            .expect("log")
            .iter()
            .any(|m| matches!(m, crate::ipc::TreeMessage::WalkSettled { .. }))
    }

    fn gateless(dir: &Path) -> AppState {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        )
        .with_walk_gate(Duration::ZERO);
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        state
    }

    #[test]
    fn open_tree_with_no_vault_is_refused_by_name() {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let (send, _) = collecting_sender();
        assert!(matches!(
            state.open_tree(send),
            Err(IpcError::VaultNotFound { .. })
        ));
        assert!(state.tree_marks().is_none());
        state.mark_tree(&MarkTreeArgs {
            mark: crate::ipc::TreeMark::Complete,
            entries_in_model: 0,
        });
    }

    #[test]
    fn a_ready_index_populates_the_tree_and_the_walk_then_settles_it() {
        // The warm path end to end: the real index file, the real `read_tree_rows`.
        let dir = fixture("tree-warm");
        let state = gateless(&dir);
        wait_ready(&state);
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));

        let log = log.lock().expect("log");
        assert!(matches!(
            &log[0],
            crate::ipc::TreeMessage::Listing { dir, .. } if dir.is_empty()
        ));
        let complete = log
            .iter()
            .find_map(|m| match m {
                crate::ipc::TreeMessage::Complete { entries, source } => Some((*entries, *source)),
                _ => None,
            })
            .expect("complete");
        // serat.md, broken.md (indexed or not, the tree lists what the index holds and
        // the walk corrects the rest) -- the source is the point.
        assert_eq!(complete.1, crate::ipc::TreeSource::Index);
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_index_that_is_not_ready_leaves_the_walk_to_populate_alone() {
        struct NeverReady;
        impl IndexOpen for NeverReady {
            fn open(&mut self, _: &Path, _: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 10,
                    walk_complete: false,
                });
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        let dir = fixture("tree-cold");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        )
        .with_walk_gate(Duration::ZERO);
        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                NeverReady,
            )
            .expect("open");
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        assert!(log.lock().expect("log").iter().any(|m| matches!(
            m,
            crate::ipc::TreeMessage::Complete {
                source: crate::ipc::TreeSource::Walk,
                ..
            }
        )));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_watcher_reaches_the_tree_as_one_upsert_and_one_remove() {
        // FR-VLT-04 through the real watcher and the vault thread's consumer.
        let dir = fixture("tree-live");
        let state = gateless(&dir);
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        let before = log.lock().expect("log").len();

        std::fs::write(dir.join("arrived.md"), "# arrived\n").expect("write");
        wait_until("the upsert", || {
            log.lock()
                .expect("log")
                .iter()
                .skip(before)
                .any(|m| matches!(m, crate::ipc::TreeMessage::Upsert { entry, .. } if entry.name == "arrived.md"))
        });
        std::fs::remove_file(dir.join("arrived.md")).expect("rm");
        wait_until("the remove", || {
            log.lock().expect("log").iter().skip(before).any(|m| {
                matches!(m, crate::ipc::TreeMessage::Remove { name, .. } if name == "arrived.md")
            })
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_tree_is_stamped_only_after_the_stream_sent_what_it_claims() {
        let dir = fixture("tree-marks");
        let state = gateless(&dir);
        let first = MarkTreeArgs {
            mark: crate::ipc::TreeMark::FirstScreen,
            entries_in_model: 0,
        };
        state.mark_tree(&first);
        assert_eq!(state.tree_marks().expect("marks").rejected, 1);

        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        state.mark_tree(&first);
        state.mark_tree(&MarkTreeArgs {
            mark: crate::ipc::TreeMark::Complete,
            entries_in_model: 3,
        });
        let marks = state.tree_marks().expect("marks");
        assert!(marks.first_screen_ms.is_some() && marks.complete_ms.is_some());
        assert_eq!(marks.entries_in_model, Some(3));
        assert!(marks.empty_folders.is_some(), "WalkSummary is reported");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_patch_save_is_submitted_to_the_index_like_any_other_write() {
        let dir = fixture("save-patch");
        let state = state(&dir);
        wait_ready(&state);
        std::thread::sleep(Duration::from_millis(50));
        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");
        let buffer = NoteBufferArgs {
            path: "10-projects/serat.md".into(),
            base_hash: read.hash.clone(),
        };
        state.note_opened(&buffer).expect("opened");
        let before = state.save_path_outcomes();
        let written = state
            .save_patch(&SavePatchArgs {
                path: "10-projects/serat.md".into(),
                base_hash: read.hash,
                patches: vec![Patch {
                    from: crate::ipc::Pos { line: 4, col: 2 },
                    to: crate::ipc::Pos { line: 4, col: 7 },
                    insert: "Patched".into(),
                }],
            })
            .expect("save");
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/serat.md")).expect("read"),
            "---\ntype: project\n---\n\n# Patched\n"
        );
        assert_eq!(
            state.save_path_outcomes().submitted,
            before.submitted + 1,
            "a patch save is a write, so the index hears about it"
        );
        // A second save on the returned hash is accepted: the base moved with the save.
        state
            .save_patch(&SavePatchArgs {
                path: "10-projects/serat.md".into(),
                base_hash: written.hash,
                patches: vec![],
            })
            .expect("no-op save on the new base");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_made_by_another_tool_reaches_the_event_sink_for_an_open_note_only() {
        let dir = fixture("external-change");
        let state = gateless(&dir);
        let seen: Arc<Mutex<Vec<crate::ipc::ExternalChange>>> = Arc::default();
        let sink = Arc::clone(&seen);
        state
            .events
            .install_external(move |change| sink.lock().expect("seen").push(change.clone()));
        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");
        state
            .note_opened(&NoteBufferArgs {
                path: "10-projects/serat.md".into(),
                base_hash: read.hash.clone(),
            })
            .expect("opened");

        // The watcher is registered before the walk ends, so a settled tree means it is live.
        let (send, tree) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&tree));

        // Not open: no event however the watcher sees it. Not "wait a while and see
        // nothing": the index row for broken.md carries the final hash only once the
        // batch that held it has been through every `Live::path_changed` (the editor's
        // included) and `paths_changed`, on the one vault thread. So after this wait the
        // editor has provably had its say about broken.md, and any event for it is in
        // `seen` already.
        wait_ready(&state);
        let valid = "now valid\n";
        std::fs::write(dir.join("broken.md"), valid).expect("write");
        let valid_hash = serat_vault::hash_bytes(valid.as_bytes()).to_hex();
        wait_until("the index to hold broken.md's final bytes", || {
            index_hash(&dir, "broken.md").as_deref() == Some(valid_hash.as_str())
        });

        let edited = "---\ntype: project\n---\n\n# Serat, edited elsewhere\n";
        let edited_hash = serat_vault::hash_bytes(edited.as_bytes()).to_hex();
        std::fs::write(dir.join("10-projects/serat.md"), edited).expect("external edit");
        // `fs::write` is a truncate and then a write. Starved between the two for longer
        // than the 150 ms window, the watcher flushes the empty file as its own change
        // and the final bytes follow as a second one (each correct, each from the
        // registry's newest base). So the wait is for the event that carries the final
        // hash, never for the first event, and the assertions hold for any such chain.
        wait_until("note://external-change carrying the final hash", || {
            seen.lock().expect("seen").iter().any(|change| {
                matches!(change, crate::ipc::ExternalChange::Changed { new_hash, .. }
                    if new_hash == &edited_hash)
            })
        });
        let seen = seen.lock().expect("seen");
        assert!(
            seen.iter().all(|change| matches!(
                change,
                crate::ipc::ExternalChange::Changed { path, .. } if path == "10-projects/serat.md"
            )),
            "only the open note is announced: {seen:?}"
        );
        assert!(matches!(
            &seen[0],
            crate::ipc::ExternalChange::Changed { base_hash, .. } if base_hash == &read.hash
        ));
        // Each change starts from where the last one ended: the registry advanced.
        for pair in seen.windows(2) {
            if let (
                crate::ipc::ExternalChange::Changed { new_hash, .. },
                crate::ipc::ExternalChange::Changed { base_hash, .. },
            ) = (&pair[0], &pair[1])
            {
                assert_eq!(new_hash, base_hash, "{seen:?}");
            }
        }
        drop(seen);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- external changes, index://changed, the open sequence, close (task #18c) ----

    fn index_hash(dir: &Path, path: &str) -> Option<String> {
        let conn = rusqlite::Connection::open(dir.join(".serat").join("index.db")).ok()?;
        conn.query_row("SELECT hash FROM notes WHERE path = ?1", [path], |row| {
            row.get(0)
        })
        .ok()
    }

    fn wait_for_row(dir: &Path, path: &str) -> String {
        let mut found = None;
        wait_until(&format!("a row for {path}"), || {
            found = index_hash(dir, path);
            found.is_some()
        });
        found.expect("row")
    }

    fn collect_changed(state: &AppState) -> Arc<Mutex<Vec<crate::ipc::IndexChanged>>> {
        let seen: Arc<Mutex<Vec<crate::ipc::IndexChanged>>> = Arc::default();
        let sink = Arc::clone(&seen);
        state
            .events
            .install_changed(move |event| sink.lock().expect("seen").push(event.clone()));
        seen
    }

    /// Open `dir`, wait for `Ready`, and for the tree walk (and so the watcher) to settle.
    fn settled_state(dir: &Path) -> AppState {
        let state = gateless(dir);
        wait_ready(&state);
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        // The catch-up runs on its own thread after the walk; it has nothing to do on
        // a fresh index, so a short pause is enough for it to finish and stop.
        std::thread::sleep(Duration::from_millis(100));
        state
    }

    #[test]
    fn an_edit_made_by_another_tool_is_indexed_and_announced_as_index_changed() {
        // Architecture 3.1.2 step 1 and step 6, end to end: the watcher's batch goes
        // through one Reconciler::index_paths and the front end is told.
        let dir = fixture("consumer-external");
        let state = gateless(&dir);
        let seen = collect_changed(&state);
        wait_ready(&state);
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        std::thread::sleep(Duration::from_millis(100));
        let before = seen.lock().expect("seen").len();

        std::fs::write(dir.join("external.md"), "# Written by another tool\n").expect("write");
        let hash = wait_for_row(&dir, "external.md");
        assert_eq!(hash.len(), 64);
        wait_until("index://changed", || {
            seen.lock().expect("seen").len() > before
        });

        let seen = seen.lock().expect("seen");
        let latest = seen.last().expect("an event");
        assert_eq!(latest.vault_id, state.index_status().vault_id);
        assert!(latest.generation >= 1);
        assert!(
            seen.windows(2).all(|w| w[0].generation < w[1].generation),
            "generations are strictly increasing: {seen:?}"
        );
        drop(seen);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_own_save_is_announced_once_and_its_echo_says_nothing() {
        // The save path calls ChangeSignal.changed after its submit; the watcher's
        // event for the same (path, hash) is an echo and must not announce again.
        let dir = fixture("consumer-echo");
        let state = settled_state(&dir);
        let seen = collect_changed(&state);

        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");
        state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "---\ntype: project\n---\n\n# Serat, saved by us\n".into(),
                base_hash: Some(read.hash),
            })
            .expect("write");
        wait_until("the save's own index://changed", || {
            !seen.lock().expect("seen").is_empty()
        });
        // Long enough for the 150 ms coalescing window and a consumer turn.
        std::thread::sleep(Duration::from_millis(600));
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1, "the echo announced again: {seen:?}");
        drop(seen);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_moved_into_the_vault_is_reconciled_as_a_subtree() {
        // VaultEvent::DirChanged: the platform lists nothing beneath it.
        let dir = fixture("consumer-dir");
        let state = settled_state(&dir);
        let staging = dir.with_file_name(format!(
            "{}-staging",
            dir.file_name().expect("name").to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(staging.join("moved/inner")).expect("staging");
        std::fs::write(staging.join("moved/a.md"), "# a\n").expect("staging");
        std::fs::write(staging.join("moved/inner/b.md"), "# b\n").expect("staging");

        std::fs::rename(staging.join("moved"), dir.join("moved")).expect("move in");
        wait_for_row(&dir, "moved/a.md");
        wait_for_row(&dir, "moved/inner/b.md");

        // And moved away again: every row beneath it leaves.
        std::fs::rename(dir.join("moved"), staging.join("moved-again")).expect("move out");
        wait_until("the rows to leave", || {
            index_hash(&dir, "moved/a.md").is_none()
                && index_hash(&dir, "moved/inner/b.md").is_none()
        });
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn closing_the_vault_makes_the_next_open_clean_and_not_closing_does_not() {
        let dir = fixture("close-clean");
        {
            let state = settled_state(&dir);
            state.close_vault();
            state.close_vault(); // idempotent
        }
        let handle = serat_index::IndexHandle::open(&dir.join(".serat")).expect("reopen");
        assert!(
            !handle.opened_after_unclean_shutdown(),
            "a closed vault must read as a clean shutdown"
        );
        drop(handle);

        // The same vault, opened and dropped without a close, reads as unclean.
        {
            let state = gateless(&dir);
            wait_ready(&state);
            std::thread::sleep(Duration::from_millis(50));
        }
        let handle = serat_index::IndexHandle::open(&dir.join(".serat")).expect("reopen");
        assert!(
            handle.opened_after_unclean_shutdown(),
            "without close() the session reads as unclean"
        );
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_save_is_submitted_before_the_close_and_none_follows_it() {
        // Close takes the open-vault lock exclusively; every save holds it shared across
        // its write and its submit. Whatever returned Ok before the close is in the
        // index when close() returns, and nothing is accepted after it.
        let dir = fixture("close-after-saves");
        let state = Arc::new(settled_state(&dir));
        let last_ok: Arc<Mutex<Option<String>>> = Arc::default();
        let writer = {
            let state = Arc::clone(&state);
            let last_ok = Arc::clone(&last_ok);
            std::thread::spawn(move || {
                let mut base = state
                    .read_file(ReadFileArgs {
                        path: "10-projects/serat.md".into(),
                    })
                    .expect("read")
                    .hash;
                for n in 0..2000 {
                    match state.write_file(WriteFileArgs {
                        path: "10-projects/serat.md".into(),
                        text: format!("# save {n}\n"),
                        base_hash: Some(base.clone()),
                    }) {
                        Ok(written) => {
                            base = written.hash.clone();
                            *last_ok.lock().expect("last") = Some(written.hash);
                        }
                        Err(IpcError::VaultNotFound { .. }) => return true,
                        Err(other) => panic!("unexpected: {other:?}"),
                    }
                }
                false
            })
        };
        wait_until("some saves", || last_ok.lock().expect("last").is_some());
        state.close_vault();
        let refused_after_close = writer.join().expect("writer");
        assert!(
            refused_after_close,
            "the writer finished before the close; raise the count"
        );

        let last = last_ok.lock().expect("last").clone().expect("a save");
        assert_eq!(
            index_hash(&dir, "10-projects/serat.md"),
            Some(last),
            "the last accepted save must be in the index once close() has returned"
        );
        let handle = serat_index::IndexHandle::open(&dir.join(".serat")).expect("reopen");
        assert!(!handle.opened_after_unclean_shutdown());
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_another_vault_closes_the_first_cleanly() {
        let first = fixture("close-switch-a");
        let second = fixture("close-switch-b");
        let state = gateless(&first);
        wait_ready(&state);
        std::thread::sleep(Duration::from_millis(50));
        state
            .open_vault(OpenVaultArgs {
                root: second.display().to_string(),
            })
            .expect("open the second");
        let handle = serat_index::IndexHandle::open(&first.join(".serat")).expect("reopen");
        assert!(!handle.opened_after_unclean_shutdown());
        drop(handle);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    /// Open `dir` and close it cleanly, so the next open is warm.
    fn open_and_close_cleanly(dir: &Path) {
        let state = settled_state(dir);
        state.close_vault();
    }

    fn change_while_closed(dir: &Path) {
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("appeared.md"), "# appeared\n").expect("write");
        std::fs::write(
            dir.join("10-projects/serat.md"),
            "---\ntype: project\n---\n\n# Serat, changed while closed\n",
        )
        .expect("write");
        std::fs::remove_file(dir.join("broken.md")).expect("remove");
    }

    /// An opener whose open check takes `.1` ms, so the index is `Opening` meanwhile.
    struct Slow(SeratIndexOpen, u64);
    impl IndexOpen for Slow {
        fn open(&mut self, root: &Path, dir: &Path, publish: &dyn Fn(IndexState)) {
            std::thread::sleep(Duration::from_millis(self.1));
            self.0.open(root, dir, publish);
        }
        fn into_handle(self) -> Option<serat_index::IndexHandle> {
            self.0.into_handle()
        }
    }

    fn state_with(hooks: OpenHooks) -> AppState {
        AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        )
        .with_walk_gate(Duration::ZERO)
        .with_open_hooks(hooks)
    }

    fn open_args(dir: &Path) -> OpenVaultArgs {
        OpenVaultArgs {
            root: dir.display().to_string(),
        }
    }

    #[test]
    fn the_warm_open_after_a_clean_shutdown_still_compares_mtimes() {
        // ADR-0015 section 2 as ruled: the open compares mtimes whatever the last
        // shutdown was; `clean_close` only schedules the background quick_check.
        let dir = fixture("open-catch-up");
        open_and_close_cleanly(&dir);
        let before = index_hash(&dir, "10-projects/serat.md").expect("indexed");
        change_while_closed(&dir);

        let state = state_with(OpenHooks::default());
        let rebuilt = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&rebuilt);
        state.events.install(move |status| {
            if matches!(status.state, IndexState::Rebuilding { .. }) {
                flag.store(true, Ordering::SeqCst);
            }
        });
        state.open_vault(open_args(&dir)).expect("open");
        wait_ready(&state);
        assert!(
            !rebuilt.load(Ordering::SeqCst),
            "a rebuild would be the reconciliation, not the catch-up"
        );
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        wait_for_row(&dir, "appeared.md");
        wait_until("the deleted note to leave", || {
            index_hash(&dir, "broken.md").is_none()
        });
        wait_until("the changed note to be re-indexed", || {
            index_hash(&dir, "10-projects/serat.md").is_some_and(|hash| hash != before)
        });
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vault_opened_with_no_open_tree_still_catches_up() {
        // FR-VLT-04: nothing in the product need call open_tree for a vault to
        // reconcile what changed while it was closed.
        let dir = fixture("open-catch-up-no-tree");
        open_and_close_cleanly(&dir);
        change_while_closed(&dir);
        let state = gateless(&dir);
        wait_for_row(&dir, "appeared.md");
        wait_until("the deleted note to leave", || {
            index_hash(&dir, "broken.md").is_none()
        });
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_open_tree_that_arrives_before_the_index_is_ready_still_catches_up() {
        let dir = fixture("open-catch-up-early-tree");
        open_and_close_cleanly(&dir);
        change_while_closed(&dir);
        let state = state_with(OpenHooks::default());
        state
            .open_vault_with(open_args(&dir), Slow(SeratIndexOpen::default(), 400))
            .expect("open");
        assert!(matches!(state.index_status().state, IndexState::Opening));
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        wait_for_row(&dir, "appeared.md");
        wait_until("the deleted note to leave", || {
            index_hash(&dir, "broken.md").is_none()
        });
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_open_tree_calls_make_one_walk_with_one_catch_up_and_it_is_the_trees() {
        let dir = fixture("open-catch-up-once");
        open_and_close_cleanly(&dir);
        let starts: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let recorded = Arc::clone(&starts);
        let state = state_with(OpenHooks {
            on_stage: None,
            on_walk_start: Some(Arc::new(move |who| {
                recorded.lock().expect("starts").push(who)
            })),
            ..OpenHooks::default()
        });
        // Both streams are opened while the index is still Opening, so the tree is
        // attached before the application's own trigger can claim: the claimed walk
        // must be the tree's, and the second stream must not start another.
        state
            .open_vault_with(open_args(&dir), Slow(SeratIndexOpen::default(), 300))
            .expect("open");
        let (first, log) = collecting_sender();
        state.open_tree(first).expect("open_tree");
        let (second, log2) = collecting_sender();
        state.open_tree(second).expect("open_tree again");
        wait_until("WalkSettled", || settled(&log) || settled(&log2));
        wait_until("the second stream", || settled(&log2));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(*starts.lock().expect("starts"), ["tree"]);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_no_index_open_with_no_tree_still_marks_walk_complete_under_the_bench_flag() {
        // --exit-after-walk on a vault whose index is rebuilt: the claim is refused (the
        // rebuild is the reconciliation), and the walk mark must still arrive.
        let dir = fixture("bench-walk-no-index");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot {
                exit_after_walk: true,
                ..Boot::default()
            },
            Settings::default(),
        )
        .with_walk_gate(Duration::ZERO);
        let rebuilt = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&rebuilt);
        state.events.install(move |status| {
            if matches!(status.state, IndexState::Rebuilding { .. }) {
                flag.store(true, Ordering::SeqCst);
            }
        });
        state.open_vault(open_args(&dir)).expect("open");
        assert!(
            state
                .startup
                .wait_for_walk_complete(Duration::from_secs(20)),
            "no walk mark"
        );
        assert!(
            rebuilt.load(Ordering::SeqCst),
            "this test is about the rebuilt-index case, so a Rebuilding must have been seen"
        );
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_catch_up_window_starts_no_later_than_the_watcher_registering() {
        // Task #78: the stamp the open's catch-up records predates `WatcherRegistered`. This
        // guards against stamping at the walk's start (#66's interim); the exact "just
        // before arming" position is the code's, not pinned by this test.
        let dir = fixture("window-before-watcher");
        open_and_close_cleanly(&dir);
        let old = window_of(&dir);
        std::thread::sleep(Duration::from_millis(30));
        let registered: Arc<Mutex<Option<i64>>> = Arc::default();
        let at = Arc::clone(&registered);
        let state = state_with(OpenHooks {
            on_stage: Some(Arc::new(move |stage| {
                if stage == Stage::WatcherRegistered {
                    *at.lock().expect("at") = Some(serat_index::rebuild::now_ms());
                    std::thread::sleep(Duration::from_millis(200));
                }
            })),
            on_walk_start: None,
            ..OpenHooks::default()
        });
        state.open_vault(open_args(&dir)).expect("open");
        wait_until("the catch-up to record its window", || {
            window_of(&dir) > old
        });
        let window = window_of(&dir);
        let registered = registered.lock().expect("at").expect("stage seen");
        assert!(
            window <= registered,
            "window {window} is after the watcher registered at {registered}"
        );
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn window_of(dir: &Path) -> i64 {
        rusqlite::Connection::open(dir.join(".serat").join("index.db"))
            .and_then(|c| {
                c.query_row(
                    "SELECT caught_up_from_ms FROM index_meta WHERE id = 1",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap_or(0)
    }

    #[test]
    fn the_tree_walk_waits_for_the_watcher() {
        // Architecture 3.4: the watcher is registered before the walk starts. The stage
        // hook holds `WatcherRegistered` back; the walk's start must follow it.
        let dir = fixture("watcher-before-walk");
        open_and_close_cleanly(&dir);
        let log: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let (staged, started) = (Arc::clone(&log), Arc::clone(&log));
        let state = state_with(OpenHooks {
            on_stage: Some(Arc::new(move |stage| {
                if stage == Stage::WatcherRegistered {
                    std::thread::sleep(Duration::from_millis(400));
                    staged.lock().expect("log").push("watcher");
                }
            })),
            on_walk_start: Some(Arc::new(move |_| {
                started.lock().expect("log").push("walk");
            })),
            ..OpenHooks::default()
        });
        state.open_vault(open_args(&dir)).expect("open");
        wait_until("the walk to start", || {
            log.lock().expect("log").contains(&"walk")
        });
        assert_eq!(*log.lock().expect("log"), ["watcher", "walk"]);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_the_same_root_shuts_the_first_open_down_before_the_second_starts() {
        struct Probe(SeratIndexOpen, Arc<Mutex<Option<i64>>>);
        impl IndexOpen for Probe {
            fn open(&mut self, root: &Path, dir: &Path, publish: &dyn Fn(IndexState)) {
                let marker = rusqlite::Connection::open(dir.join("index.db"))
                    .and_then(|c| {
                        c.query_row("SELECT clean_close FROM index_meta WHERE id = 1", [], |r| {
                            r.get::<_, i64>(0)
                        })
                    })
                    .ok();
                *self.1.lock().expect("probe") = marker;
                self.0.open(root, dir, publish);
            }
            fn into_handle(self) -> Option<serat_index::IndexHandle> {
                self.0.into_handle()
            }
        }
        let dir = fixture("reopen-same-root");
        let state = gateless(&dir);
        wait_ready(&state);
        let seen: Arc<Mutex<Option<i64>>> = Arc::default();
        state
            .open_vault_with(
                open_args(&dir),
                Probe(SeratIndexOpen::default(), Arc::clone(&seen)),
            )
            .expect("reopen");
        wait_ready(&state);
        assert_eq!(
            *seen.lock().expect("probe"),
            Some(1),
            "the first open had closed cleanly before the second's actor looked"
        );
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_warm_open_after_an_unclean_shutdown_compares_mtimes() {
        let dir = fixture("open-catch-up-unclean");
        {
            let state = gateless(&dir);
            wait_ready(&state);
            std::thread::sleep(Duration::from_millis(50));
            // Dropped without close_vault: the next session reads as unclean.
        }
        let before = index_hash(&dir, "10-projects/serat.md").expect("indexed");
        std::fs::write(
            dir.join("10-projects/serat.md"),
            "---\ntype: project\n---\n\n# Serat, changed while closed\n",
        )
        .expect("write");

        let state = gateless(&dir);
        wait_ready(&state);
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        wait_until("the changed note to be re-indexed", || {
            index_hash(&dir, "10-projects/serat.md").is_some_and(|hash| hash != before)
        });
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_runtime_rebuild_replaces_the_index_and_searches_follow_the_new_handle() {
        let dir = fixture("rebuild-in-place");
        std::fs::write(dir.join("beta.md"), "# Beta\n\nzebra stripes\n").expect("fixture");
        let state = settled_state(&dir);
        let seen = collect_changed(&state);
        let zebra = |state: &AppState| {
            state.search(SearchArgs {
                query: "zebra".into(),
                offset: 0,
                limit: 10,
            })
        };
        // A search connection is open on the old file.
        assert!(matches!(
            zebra(&state).expect("search"),
            IndexRead::Ready {
                value: SearchOutcome::Hits { .. }
            }
        ));

        state.rebuild_index().expect("rebuild");
        assert!(matches!(
            state.index_status().state,
            IndexState::Ready { .. }
        ));
        match zebra(&state).expect("search") {
            IndexRead::Ready {
                value: SearchOutcome::Hits { items },
            } => assert_eq!(items[0].path, "beta.md"),
            other => panic!("the search did not follow the new handle: {other:?}"),
        }
        assert!(
            !seen.lock().expect("seen").is_empty(),
            "views are told to re-query"
        );

        // And the rebuilt handle takes saves and closes cleanly.
        let read = state
            .read_file(ReadFileArgs {
                path: "beta.md".into(),
            })
            .expect("read");
        let before = state.save_path_outcomes();
        state
            .write_file(WriteFileArgs {
                path: "beta.md".into(),
                text: "# Beta\n\nquagga\n".into(),
                base_hash: Some(read.hash),
            })
            .expect("write");
        assert_eq!(state.save_path_outcomes().submitted, before.submitted + 1);
        state.close_vault();
        let handle = serat_index::IndexHandle::open(&dir.join(".serat")).expect("reopen");
        assert!(!handle.opened_after_unclean_shutdown());
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rebuild_with_no_vault_or_no_index_is_refused_by_name() {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        assert!(matches!(
            state.rebuild_index(),
            Err(IpcError::VaultNotFound { .. })
        ));
    }

    // -- switcher_query (task #18c) ---------------------------------------------------

    fn switch(state: &AppState, seq: u32, query: &str) -> crate::ipc::SwitcherResults {
        state.switcher_query(&crate::ipc::SwitcherQueryArgs {
            seq,
            query: query.into(),
            limit: 20,
        })
    }

    #[test]
    fn the_switcher_answers_from_the_tree_before_the_index_is_ready() {
        struct StillRebuilding;
        impl IndexOpen for StillRebuilding {
            fn open(&mut self, _: &Path, _: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 10,
                    walk_complete: false,
                });
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        let dir = fixture("switcher-tree");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        )
        .with_walk_gate(Duration::ZERO);
        // Nothing yet: not an error, an empty tree answer.
        let before = switch(&state, 1, "serat");
        assert!(before.items.is_empty());
        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                StillRebuilding,
            )
            .expect("open");
        let (send, log) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));

        let found = switch(&state, 2, "serat");
        assert_eq!(found.seq, 2);
        assert_eq!(found.corpus, crate::ipc::SwitcherCorpus::Tree);
        assert_eq!(found.items[0].path, "10-projects/serat.md");
        assert!(!found.items[0].highlight.is_empty());
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_switcher_gains_the_index_and_follows_it_and_ranks_what_was_opened() {
        let dir = fixture("switcher-index");
        std::fs::write(
            dir.join("notes.md"),
            "---\ntitle: Field Notes\naliases: [zettel]\n---\n\n# Field Notes\n\n## Quagga habits\n",
        )
        .expect("fixture");
        let state = settled_state(&dir);

        // The index snapshot arrives on the worker thread after Ready.
        wait_until("an index-backed answer", || {
            switch(&state, 1, "zettel").corpus == crate::ipc::SwitcherCorpus::Index
        });
        let alias = switch(&state, 2, "zettel");
        assert_eq!(alias.items[0].path, "notes.md");
        assert_eq!(alias.items[0].matched, crate::ipc::SwitcherMatch::Alias);
        assert_eq!(alias.items[0].title.as_deref(), Some("Field Notes"));

        // Heading mode: the text before `#` filters notes, the text after matches headings.
        let heading = switch(&state, 3, "notes#quag");
        let first = &heading.items[0];
        assert_eq!(first.path, "notes.md");
        assert_eq!(first.matched, crate::ipc::SwitcherMatch::Heading);
        assert_eq!(
            first.heading.as_ref().expect("heading").text,
            "Quagga habits"
        );

        // After a webview reload the front end's `seq` restarts at 1: that must not be
        // read as "older than the 9 before the reload" (the discard rule is Rust's
        // arrival order, not the counter).
        assert!(!switch(&state, 9, "notes").items.is_empty());
        let after_reload = switch(&state, 1, "notes");
        assert_eq!(after_reload.seq, 1);
        assert!(
            !after_reload.items.is_empty(),
            "a lower seq that arrives later is still answered"
        );

        // The corpus follows the index: a note written by another tool is found
        // within the refresh delay and the build.
        std::fs::write(dir.join("zebra-crossing.md"), "# Zebra crossing\n").expect("write");
        wait_until("the new note in the switcher", || {
            switch(&state, 100, "zebra")
                .items
                .iter()
                .any(|i| i.path == "zebra-crossing.md")
        });

        // Opening a note records frecency: it leads the empty query, and the record is
        // on disk once the vault is closed.
        let read = state
            .read_file(ReadFileArgs {
                path: "notes.md".into(),
            })
            .expect("read");
        state
            .note_opened(&NoteBufferArgs {
                path: "notes.md".into(),
                base_hash: read.hash,
            })
            .expect("opened");
        let empty = switch(&state, 200, "");
        assert_eq!(empty.items[0].path, "notes.md", "{:?}", empty.items);
        state.close_vault();
        let history = std::fs::read_to_string(dir.join(".serat/history.json")).expect("history");
        assert!(history.contains("notes.md"), "{history}");

        // And it is read back at the next open (a vault with no index state of its own
        // to lean on: the record is a sibling of index.db).
        let state = settled_state(&dir);
        wait_until("an index-backed answer", || {
            switch(&state, 300, "").corpus == crate::ipc::SwitcherCorpus::Index
        });
        assert_eq!(switch(&state, 301, "").items[0].path, "notes.md");
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- the bounded flush before close (NFR-REL-01, #63) ---------------------------

    /// `close_after_flush` as its callers run it: under `opening`.
    fn close_flush(state: &AppState, deadline: Duration) -> FlushOutcome {
        let opening = state.lock_opening();
        state.close_after_flush(&opening, deadline)
    }

    /// A settled state on `dir` whose `app://closing` events are collected.
    fn closing_state(dir: &Path) -> (Arc<AppState>, Arc<Mutex<Vec<AppClosing>>>) {
        let state = Arc::new(settled_state(dir));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        state
            .events
            .install_closing(move |event| sink.lock().expect("seen").push(event.clone()));
        open_note(&state);
        (state, seen)
    }

    /// The editor has a note open, so a close has something to flush.
    fn open_note(state: &AppState) {
        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");
        state
            .note_opened(&NoteBufferArgs {
                path: "10-projects/serat.md".into(),
                base_hash: read.hash,
            })
            .expect("opened");
    }

    fn patch_args(state: &AppState, text: &str) -> SavePatchArgs {
        let read = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");
        SavePatchArgs {
            path: "10-projects/serat.md".into(),
            base_hash: read.hash,
            patches: vec![Patch {
                from: crate::ipc::Pos { line: 0, col: 0 },
                to: crate::ipc::Pos { line: 0, col: 0 },
                insert: text.into(),
            }],
        }
    }

    #[test]
    fn a_save_after_the_closing_event_and_before_the_ack_lands_on_disk() {
        let dir = fixture("flush-save");
        let (state, seen) = closing_state(&dir);
        let args = patch_args(&state, "LAST-SECOND ");
        let closer = Arc::clone(&state);
        let closing = std::thread::spawn(move || close_flush(&closer, Duration::from_secs(10)));
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        let event = seen.lock().expect("seen")[0].clone();
        assert_eq!(event.deadline_ms, 10_000);
        // The window has been asked to close; the debounced save still gets in.
        state.save_patch(&args).expect("accepted during the wait");
        state.flush_ack(&FlushAckArgs {
            vault_id: event.vault_id,
        });
        assert_eq!(closing.join().expect("close"), FlushOutcome::Acked);
        let text = std::fs::read_to_string(dir.join("10-projects/serat.md")).expect("read");
        assert!(text.starts_with("LAST-SECOND "), "{text}");
        // After the close the same save is refused, as before.
        assert!(matches!(
            state.save_patch(&args),
            Err(IpcError::VaultNotFound { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ack_ends_the_wait_early_and_a_stale_ack_does_not() {
        let dir = fixture("flush-ack");
        let (state, seen) = closing_state(&dir);
        let closer = Arc::clone(&state);
        let started = Instant::now();
        let closing = std::thread::spawn(move || close_flush(&closer, Duration::from_secs(30)));
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        state.flush_ack(&FlushAckArgs {
            vault_id: "not-this-vault".into(),
        });
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !closing.is_finished(),
            "an ack for another vault ended the wait"
        );
        let id = seen.lock().expect("seen")[0].vault_id.clone();
        state.flush_ack(&FlushAckArgs { vault_id: id });
        assert_eq!(closing.join().expect("close"), FlushOutcome::Acked);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(state.vault_root().is_none(), "closed after the ack");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_ack_closes_at_the_deadline_and_a_second_request_does_not_restart_it() {
        let dir = fixture("flush-timeout");
        let (state, seen) = closing_state(&dir);
        let (count, destroy) = destroyer();
        let started = Instant::now();
        assert!(crate::on_close_requested(&state, destroy.clone()));
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        assert!(crate::on_close_requested(&state, destroy));
        wait_count(&count, 1);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(1900), "{waited:?}");
        assert!(waited < Duration::from_secs(8), "{waited:?}");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 1, "destroyed once");
        assert_eq!(seen.lock().expect("seen").len(), 1, "one event per close");
        assert!(state.vault_root().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_a_listener_but_no_note_open_the_close_is_immediate() {
        let dir = fixture("flush-none");
        let state = Arc::new(settled_state(&dir));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        state
            .events
            .install_closing(move |event| sink.lock().expect("seen").push(event.clone()));
        let started = Instant::now();
        assert_eq!(
            close_flush(&state, Duration::from_secs(30)),
            FlushOutcome::Immediate
        );
        assert_eq!(
            close_flush(&state, Duration::from_secs(30)),
            FlushOutcome::Immediate
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(seen.lock().expect("seen").is_empty(), "nothing to flush");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Counts destroys.
    fn destroyer() -> (
        Arc<std::sync::atomic::AtomicUsize>,
        impl Fn() + Send + Clone + 'static,
    ) {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        (count, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn wait_count(count: &Arc<std::sync::atomic::AtomicUsize>, want: usize) {
        wait_until("the window destroy", || {
            count.load(Ordering::SeqCst) >= want
        });
    }

    #[test]
    fn a_sink_that_panics_does_not_wedge_the_close() {
        let dir = fixture("flush-panic");
        let state = Arc::new(settled_state(&dir));
        state.events.install_closing(|_| panic!("sink exploded"));
        open_note(&state);
        let (count, destroy) = destroyer();
        assert!(crate::on_close_requested(&state, destroy));
        wait_count(&count, 1);
        assert!(state.vault_root().is_none());
        // A later close (another vault, say) is not stuck behind a leaked flag.
        assert_eq!(
            close_flush(&state, Duration::from_secs(30)),
            FlushOutcome::Immediate
        );
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_close_requests_destroy_the_window_once_and_emit_one_event() {
        let dir = fixture("flush-two");
        let (state, seen) = closing_state(&dir);
        let (count, destroy) = destroyer();
        assert!(crate::on_close_requested(&state, destroy.clone()));
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        assert!(crate::on_close_requested(&state, destroy));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 0, "still waiting for the ack");
        let id = seen.lock().expect("seen")[0].vault_id.clone();
        state.flush_ack(&FlushAckArgs { vault_id: id });
        wait_count(&count, 1);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(seen.lock().expect("seen").len(), 1);
        // With the vault gone a further request finds nothing to close and, the window
        // being claimed already, destroys nothing.
        let (_c, destroy) = destroyer();
        assert!(crate::on_close_requested(&state, destroy));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_close_in_progress_holds_a_vault_switch_until_the_old_vault_is_closed() {
        let dir = fixture("flush-race-a");
        let other = fixture("flush-race-a2");
        let (state, seen) = closing_state(&dir);
        let (count, destroy) = destroyer();
        assert!(crate::on_close_requested(&state, destroy));
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        let switcher = Arc::clone(&state);
        let target = other.display().to_string();
        let switching =
            std::thread::spawn(move || switcher.open_vault(OpenVaultArgs { root: target }));
        std::thread::sleep(Duration::from_millis(200));
        assert!(!switching.is_finished(), "the switch raced the close");
        assert_eq!(
            state
                .vault_root()
                .map(|r| r.ends_with(dir.file_name().unwrap())),
            Some(true)
        );
        let id = seen.lock().expect("seen")[0].vault_id.clone();
        state.flush_ack(&FlushAckArgs { vault_id: id });
        switching.join().expect("switch").expect("opened");
        wait_count(&count, 1);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state
                .vault_root()
                .map(|r| r.ends_with(other.file_name().unwrap())),
            Some(true),
            "the switch completed after the close: B is open"
        );
        state.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_window_close_during_a_vault_switch_is_not_dropped() {
        let dir = fixture("flush-race-b");
        let other = fixture("flush-race-b2");
        let (state, seen) = closing_state(&dir);
        let switcher = Arc::clone(&state);
        let target = other.display().to_string();
        let switching =
            std::thread::spawn(move || switcher.open_vault(OpenVaultArgs { root: target }));
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        let (count, destroy) = destroyer();
        assert!(crate::on_close_requested(&state, destroy));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(count.load(Ordering::SeqCst), 0, "waits for the same close");
        let id = seen.lock().expect("seen")[0].vault_id.clone();
        state.flush_ack(&FlushAckArgs { vault_id: id });
        switching.join().expect("switch").expect("opened");
        wait_count(&count, 1);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 1, "destroyed once");
        assert!(
            state.vault_root().is_none(),
            "the close ran after the switch and closed B before the window went"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_window_close_during_a_switch_to_a_missing_root_closes_the_kept_vault_once() {
        let dir = fixture("flush-race-c");
        let (state, seen) = closing_state(&dir);
        let switcher = Arc::clone(&state);
        let target = dir.join("no-such-vault").display().to_string();
        // The switch validates B before touching A, so A stays open; hold `opening`
        // long enough for the close to queue behind it.
        let opening = state.lock_opening();
        let switching =
            std::thread::spawn(move || switcher.open_vault(OpenVaultArgs { root: target }));
        let (count, destroy) = destroyer();
        assert!(crate::on_close_requested(&state, destroy));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(count.load(Ordering::SeqCst), 0);
        drop(opening);
        assert!(switching.join().expect("switch").is_err());
        wait_until("the closing event", || {
            !seen.lock().expect("seen").is_empty()
        });
        let id = seen.lock().expect("seen")[0].vault_id.clone();
        state.flush_ack(&FlushAckArgs { vault_id: id });
        wait_count(&count, 1);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(seen.lock().expect("seen").len(), 1);
        assert!(state.vault_root().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- task #90: `vault://opened` and the order it is emitted in (FR-VLT-01, FR-VLT-04) --
    //
    // These prove the order at the sink: one Vec records every event in the order the
    // Rust side handed it to its emitter. Whether the webview *receives* them in that
    // order across event names is Tauri's, and is the open question in architecture
    // "Open design questions"; nothing here claims it.

    type EventLog = Arc<Mutex<Vec<(&'static str, String)>>>;

    fn logged(log: &EventLog, kind: &str, id: &str) -> bool {
        log.lock()
            .expect("log")
            .iter()
            .any(|(k, i)| *k == kind && i == id)
    }

    /// An opener whose first act is to publish `Rebuilding`, so the actor's first
    /// status is on its way the moment its thread starts; then the real open.
    struct Announcing(SeratIndexOpen);

    impl IndexOpen for Announcing {
        fn open(&mut self, root: &Path, dir: &Path, publish: &dyn Fn(IndexState)) {
            publish(IndexState::Rebuilding {
                reason: RebuildReason::NoIndex,
                done: 0,
                total: 1,
                walk_complete: false,
            });
            self.0.open(root, dir, publish);
        }

        fn into_handle(self) -> Option<serat_index::IndexHandle> {
            self.0.into_handle()
        }
    }

    /// Every event kind with the vault id it carries, in emission order. `app://closing`
    /// is acked from inside the sink so a switch with an open note does not wait.
    fn log_all_events(state: &Arc<AppState>, log: &EventLog, with_opened: bool) {
        let l = Arc::clone(log);
        state.events.install(move |s| {
            l.lock()
                .expect("log")
                .push(("index://status", s.vault_id.clone()));
        });
        let l = Arc::clone(log);
        state.events.install_tree_status(move |s| {
            l.lock()
                .expect("log")
                .push(("tree://status", s.vault_id.clone()));
        });
        let l = Arc::clone(log);
        state.events.install_changed(move |e| {
            l.lock()
                .expect("log")
                .push(("index://changed", e.vault_id.clone()));
        });
        let l = Arc::clone(log);
        state.events.install_external(move |e| {
            let id = match e {
                crate::ipc::ExternalChange::Changed { vault_id, .. }
                | crate::ipc::ExternalChange::Deleted { vault_id, .. } => vault_id.clone(),
            };
            l.lock().expect("log").push(("note://external-change", id));
        });
        if with_opened {
            let l = Arc::clone(log);
            state.events.install_opened(move |o| {
                l.lock()
                    .expect("log")
                    .push(("vault://opened", o.vault_id.clone()));
            });
        }
        let l = Arc::clone(log);
        let acker = Arc::clone(state);
        state.events.install_closing(move |c| {
            l.lock()
                .expect("log")
                .push(("app://closing", c.vault_id.clone()));
            acker.flush_ack(&FlushAckArgs {
                vault_id: c.vault_id.clone(),
            });
        });
    }

    fn switch_state(
        name: &str,
        hooks: impl FnOnce(&EventLog) -> OpenHooks,
        with_opened: bool,
    ) -> (Arc<AppState>, EventLog, PathBuf, PathBuf) {
        let (a, b) = (fixture(&format!("{name}-a")), fixture(&format!("{name}-b")));
        let log: EventLog = Arc::default();
        let state = Arc::new(
            AppState::with_settings(
                Arc::new(Startup::begin()),
                Boot::default(),
                Settings::default(),
            )
            .with_walk_gate(Duration::ZERO)
            .with_open_hooks(hooks(&log)),
        );
        log_all_events(&state, &log, with_opened);
        (state, log, a, b)
    }

    fn open_dir(state: &AppState, dir: &Path) -> Result<VaultOpened, IpcError> {
        state.open_vault_with(
            OpenVaultArgs {
                root: dir.display().to_string(),
            },
            Announcing(SeratIndexOpen::default()),
        )
    }

    /// Hooks that, for the vault "v2" only, hold the open thread right after the given
    /// spawn until the actor's first `index://status` for "v2" is in the log. Then the
    /// race the ordering test needs is not a race: an emit placed after that spawn
    /// loses every time, because the status is already logged when the emit comes.
    fn hold_until_first_status(log: &EventLog) -> OpenHooks {
        let hold = |log: &EventLog| -> SpawnHook {
            let log = Arc::clone(log);
            Arc::new(move |id: &str| {
                if id == "v2" {
                    wait_until("the actor's first status for v2", || {
                        logged(&log, "index://status", "v2")
                    });
                }
            })
        };
        OpenHooks {
            after_index_spawn: Some(hold(log)),
            after_vault_spawn: Some(hold(log)),
            ..OpenHooks::default()
        }
    }

    // Mutation check, run by hand on this commit (each moves the one `emit_opened`
    // call in `open_vault_with`):
    //   after Publisher::new ........... passes: equivalent, nothing can carry the id yet
    //   after Switcher::new ............ passes: equivalent, same reason
    //   after IndexActor::spawn ........ FAILS every time (hold hook: the actor's first
    //                                    status is logged before the emit comes)
    //   after VaultThread::spawn_with .. FAILS every time (same)
    //   after the `replace` ............ FAILS
    // `tree://status` is deliberately absent from the required kinds: it is emitted only
    // when the tree stream gives up after MAX_RESTARTS poisoned sessions, which no seam
    // reaches here. Its id comes from the same mint and its closure is built after the
    // emit, so the emit-point argument covers it; it is not proven by this test.
    #[test]
    fn vault_opened_precedes_every_event_carrying_the_new_id_and_the_old_id_never_follows_it() {
        let (state, log, a, b) = switch_state("opened-order", hold_until_first_status, true);
        let first = open_dir(&state, &a).expect("open A");
        wait_ready(&state);
        open_note(&state);
        let a_id = first.vault_id;
        // Switch with a note open so A's app://closing is emitted too.
        let second = open_dir(&state, &b).expect("open B");
        let b_id = second.vault_id;
        assert_ne!(a_id, b_id);
        wait_ready(&state);
        let (send, tree) = collecting_sender();
        state.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&tree));
        std::fs::write(b.join("external.md"), "# other tool\n").expect("write");
        wait_for_row(&b, "external.md");
        // The row being in the index is not the event being logged: `index://changed`
        // is emitted from the change signal's own thread, throttled, after the commit
        // (task #122: sampling the log here failed once in ten loaded runs with "no
        // index://changed for B"). Wait on the event itself, bounded, before sampling;
        // the negative assertions below still read the whole log.
        wait_until("index://changed for B", || {
            logged(&log, "index://changed", &b_id)
        });
        open_note(&state);
        close_flush(&state, Duration::from_secs(10));

        let log = log.lock().expect("log").clone();
        let opened_b = log
            .iter()
            .position(|e| *e == ("vault://opened", b_id.clone()))
            .expect("vault://opened for B");
        for (i, (kind, id)) in log.iter().enumerate() {
            // (a) nothing carrying B's id precedes its announcement.
            if *id == b_id && *kind != "vault://opened" {
                assert!(
                    i > opened_b,
                    "{kind} for B at {i} precedes opened at {opened_b}: {log:?}"
                );
            }
            // (d) the old id is dead once the new vault has been announced.
            if i > opened_b {
                assert_ne!(*id, a_id, "{kind} carries the old id after opened: {log:?}");
            }
        }
        for kind in ["index://status", "index://changed", "app://closing"] {
            assert!(
                log.iter().any(|(k, id)| *k == kind && *id == b_id),
                "no {kind} for B in the run, so the ordering of it was not tested: {log:?}"
            );
        }
        assert!(
            log.iter()
                .any(|(k, id)| *k == "app://closing" && *id == a_id),
            "A's closing event is part of the run: {log:?}"
        );
        assert_eq!(
            log.iter().filter(|(k, _)| *k == "vault://opened").count(),
            2
        );
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_failed_open_after_vault_opened_retires_everything_that_could_carry_the_id() {
        // One run per `?` after the announcement (task #90, review B1).
        for at in [FailAt::Switcher, FailAt::IndexActor, FailAt::VaultThread] {
            let probe: PublisherProbe = Arc::default();
            let hooked = Arc::clone(&probe);
            let (state, log, a, _b) = switch_state(
                &format!("opened-fails-{at:?}"),
                move |_| OpenHooks {
                    fail_at: Some(at),
                    publisher_probe: Some(hooked),
                    ..OpenHooks::default()
                },
                true,
            );
            let err = open_dir(&state, &a).expect_err("the open fails after the emit");
            assert!(matches!(err, IpcError::Io { .. }), "{at:?}: {err:?}");
            assert!(state.vault_root().is_none(), "{at:?}: no vault is open");
            assert_eq!(
                state.index_status().vault_id,
                "",
                "{at:?}: no vault reports"
            );
            let announced = log
                .lock()
                .expect("log")
                .iter()
                .find(|(k, _)| *k == "vault://opened")
                .map(|(_, id)| id.clone())
                .expect("the announcement was made");

            // Positive, not timing: the publisher is retired, and a publish on it now
            // reaches nobody. The actor was joined before `open_vault_with` returned, so
            // nothing is mid-emit either.
            let publisher = probe.lock().expect("probe").clone();
            let publisher = publisher.unwrap_or_else(|| panic!("{at:?}: a publisher existed"));
            assert!(publisher.is_retired(), "{at:?}");
            let before = log.lock().expect("log").len();
            assert!(!publisher.publish(IndexState::Ready { note_count: 1 }));
            publisher.re_emit();
            assert_eq!(log.lock().expect("log").len(), before, "{at:?}");
            assert!(
                log.lock()
                    .expect("log")
                    .iter()
                    .skip_while(|(k, _)| *k != "vault://opened")
                    .skip(1)
                    .all(|(k, id)| *id != announced || *k == "index://status"),
                "{at:?}: only the actor's own status may fall in the window before Err"
            );

            // Only that a failed attempt does not wedge a later open of the same root.
            // It does not prove the join: the fixture is tiny, so the actor is finished
            // long before this runs. `a_failed_open_waits_for_the_index_thread...` pins it.
            let retry = AppState::with_settings(
                Arc::new(Startup::begin()),
                Boot::default(),
                Settings::default(),
            )
            .with_walk_gate(Duration::ZERO);
            let ok = open_dir(&retry, &a).expect("a re-open works");
            wait_ready(&retry);
            assert_eq!(retry.index_status().vault_id, ok.vault_id);
            retry.close_vault();
            let _ = std::fs::remove_dir_all(&a);
        }
    }

    #[test]
    fn a_failed_open_waits_for_the_index_thread_before_returning_err() {
        // SF-1 / task #90: the guard retires the publisher and then joins the actor
        // (bounded by CLOSE_DEADLINE). Removing the join makes `finished` false here.
        struct Sleeper(Arc<std::sync::atomic::AtomicBool>);
        impl IndexOpen for Sleeper {
            fn open(&mut self, _r: &Path, _d: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Opening);
                std::thread::sleep(Duration::from_millis(400));
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (state, _log, a, _b) = switch_state(
            "opened-join",
            |_| OpenHooks {
                fail_at: Some(FailAt::VaultThread),
                ..OpenHooks::default()
            },
            true,
        );
        let started = Instant::now();
        let err = state
            .open_vault_with(
                OpenVaultArgs {
                    root: a.display().to_string(),
                },
                Sleeper(Arc::clone(&finished)),
            )
            .expect_err("fails at the vault thread");
        assert!(matches!(err, IpcError::Io { .. }), "{err:?}");
        assert!(
            finished.load(std::sync::atomic::Ordering::SeqCst),
            "open_vault_with returned Err while the index thread was still running"
        );
        assert!(started.elapsed() < CLOSE_DEADLINE, "bounded, not unbounded");
        let _ = std::fs::remove_dir_all(&a);
    }

    // -- task #96: one index writer per index.db (ADR-0001) -------------------------
    //
    // Mutant, run by hand on this commit: `OpenHooks::skip_registry_check = true` on the
    // re-opening state makes both tests below fail (`max_running == 2`).

    /// An opener that holds the "index" for `hold`, counting how many are inside `open`
    /// at once. It stands for an actor in a long rebuild.
    #[derive(Clone)]
    struct Counting {
        running: Arc<std::sync::atomic::AtomicUsize>,
        max: Arc<std::sync::atomic::AtomicUsize>,
        hold: Duration,
        rebuild: bool,
    }

    impl IndexOpen for Counting {
        fn open(&mut self, _r: &Path, _d: &Path, publish: &dyn Fn(IndexState)) {
            use std::sync::atomic::Ordering::SeqCst;
            let now = self.running.fetch_add(1, SeqCst) + 1;
            self.max.fetch_max(now, SeqCst);
            if self.rebuild {
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 1,
                    walk_complete: false,
                });
            }
            std::thread::sleep(self.hold);
            // Released before the terminal word: the exit flag fires after this.
            self.running.fetch_sub(1, SeqCst);
            publish(IndexState::Ready { note_count: 1 });
        }
    }

    fn counting(hold_ms: u64) -> Counting {
        Counting {
            running: Arc::default(),
            max: Arc::default(),
            hold: Duration::from_millis(hold_ms),
            rebuild: true,
        }
    }

    fn plain_state(deadline: Duration) -> AppState {
        AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        )
        .with_walk_gate(Duration::ZERO)
        .with_actor_deadline(deadline)
    }

    fn reopen_after_detach(
        first: &AppState,
        reopen_hooks: OpenHooks,
        slow: &Counting,
        dir: &Path,
    ) -> AppState {
        let second = plain_state(Duration::from_secs(5))
            .with_detached(first.detached_writers())
            .with_open_hooks(reopen_hooks);
        assert_eq!(
            first.detached_writers().live_count(),
            1,
            "one actor detached"
        );
        let fast = Counting {
            hold: Duration::ZERO,
            ..slow.clone()
        };
        let opened = second
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                fast,
            )
            .expect("the re-open succeeds");
        assert_eq!(
            opened.index,
            IndexState::Opening,
            "no actor yet: it must wait"
        );
        wait_ready(&second);
        assert_eq!(
            slow.max.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "two writers on one index.db at once"
        );
        assert_eq!(first.detached_writers().live_count(), 0);
        second
    }

    #[test]
    fn a_reopen_waits_for_an_actor_detached_by_a_failed_open() {
        let dir = fixture("detached-failed");
        let slow = counting(700);
        let first = plain_state(Duration::from_millis(100)).with_open_hooks(OpenHooks {
            fail_at: Some(FailAt::VaultThread),
            ..OpenHooks::default()
        });
        let err = first
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                slow.clone(),
            )
            .expect_err("fails after the actor spawned");
        assert!(matches!(err, IpcError::Io { .. }), "{err:?}");
        let second = reopen_after_detach(&first, OpenHooks::default(), &slow, &dir);
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reopen_waits_for_an_actor_still_running_when_the_vault_closed() {
        let dir = fixture("detached-closed");
        let slow = counting(700);
        let first = plain_state(Duration::from_secs(5));
        first
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                slow.clone(),
            )
            .expect("open");
        // The close does not wait for the actor (it never did): it must record it.
        first.close_vault();
        let second = reopen_after_detach(&first, OpenHooks::default(), &slow, &dir);
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deferred_reopen_still_runs_the_open_catch_up_after_the_settle_bound() {
        // Review B1: the settle clock must start when the actor exists, not when the
        // open starts. Here the detached actor holds for 1.5 s and the bound is 500 ms;
        // on the previous logic the claims gave up at 500 ms, before the deferred actor
        // had even been spawned, and no walk ran.
        let dir = fixture("detached-catch-up");
        let slow = counting(1500);
        let first = plain_state(Duration::from_secs(5));
        first
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                slow.clone(),
            )
            .expect("open");
        first.close_vault();
        assert_eq!(first.detached_writers().live_count(), 1);

        let walks: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let recorded = Arc::clone(&walks);
        let second = plain_state(Duration::from_secs(5))
            .with_detached(first.detached_writers())
            .with_settle_bound(Duration::from_millis(500))
            .with_open_hooks(OpenHooks {
                on_walk_start: Some(Arc::new(move |who| {
                    recorded.lock().expect("walks").push(who);
                })),
                ..OpenHooks::default()
            });
        let fast = Counting {
            hold: Duration::ZERO,
            rebuild: false,
            ..slow
        };
        let opened = second
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                fast,
            )
            .expect("re-open");
        assert_eq!(opened.index, IndexState::Opening);
        wait_ready(&second);
        wait_until("the catch-up walk", || {
            !walks.lock().expect("walks").is_empty()
        });
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A state that closed a vault whose actor (holding `hold_ms`) is still running, and
    /// a second state on the same registry with a short settle bound.
    fn detached_pair(
        name: &str,
        hold_ms: u64,
        bound_ms: u64,
    ) -> (AppState, AppState, PathBuf, Counting) {
        let dir = fixture(name);
        let slow = counting(hold_ms);
        let first = plain_state(Duration::from_secs(5));
        first
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                slow.clone(),
            )
            .expect("open");
        first.close_vault();
        assert_eq!(first.detached_writers().live_count(), 1);
        let second = plain_state(Duration::from_secs(5))
            .with_detached(first.detached_writers())
            .with_settle_bound(Duration::from_millis(bound_ms));
        (first, second, dir, slow)
    }

    #[test]
    fn a_deferred_reopen_populates_the_tree_without_waiting_for_the_detached_actor() {
        // Review B1 (round 2): the tree's claim must not sit behind an actor that does
        // not exist yet. The detached actor holds 2.5 s; the tree must settle well
        // inside FR-VLT-01's 2 s while it is still live, and the catch-up it was
        // refused is owed as a rescan, which runs once the real actor is up.
        let (first, second, dir, _slow) = detached_pair("detached-tree", 2500, 30_000);
        let started = Instant::now();
        let opened = second
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                SeratIndexOpen::default(),
            )
            .expect("re-open");
        assert_eq!(opened.index, IndexState::Opening);
        let (send, log) = collecting_sender();
        second.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "the tree waited {:?} for the detached actor",
            started.elapsed()
        );
        assert_eq!(
            first.detached_writers().live_count(),
            1,
            "the detached actor must still be live when the tree settled"
        );
        assert!(
            second.rescan_owed(),
            "the refused catch-up is owed as a rescan"
        );
        wait_ready(&second);
        wait_until("the owed rescan to run", || !second.rescan_owed());
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_catch_up_that_never_settles_owes_a_rescan() {
        // Review S1: pins `on_unsettled` -> `Indexer::owe_rescan`. The deferred actor
        // starts but publishes nothing for 1.5 s; the bound is 100 ms. Removing the
        // `on_unsettled` wiring in `open_vault_with`, or making `owe_rescan` a no-op,
        // leaves the flag false and fails this test (both mutants run by hand).
        let (_first, second, dir, slow) = detached_pair("detached-unsettled", 400, 100);
        let silent = Counting {
            hold: Duration::from_millis(1500),
            rebuild: false,
            ..slow
        };
        second
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                silent,
            )
            .expect("re-open");
        wait_until("the unsettled claim to owe a rescan", || {
            second.rescan_owed()
        });
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_deferred_spawn_leaves_the_index_unavailable_not_opening_forever() {
        let (_first, second, dir, _slow) = detached_pair("detached-spawn-fails", 300, 30_000);
        let second = second.with_open_hooks(OpenHooks {
            fail_deferred_spawn: true,
            ..OpenHooks::default()
        });
        let opened = second
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                SeratIndexOpen::default(),
            )
            .expect("the vault opens without an actor");
        assert_eq!(opened.index, IndexState::Opening);
        wait_until("Unavailable", || {
            matches!(second.index_status().state, IndexState::Unavailable { .. })
        });
        let IndexState::Unavailable { message, path } = second.index_status().state else {
            unreachable!()
        };
        assert!(
            message.contains("could not start the index thread"),
            "{message}"
        );
        assert!(path.contains(".serat"), "{path}");
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deferred_reopen_indexes_what_changed_while_the_vault_was_closed() {
        // Review SF-2: the effect of the owed rescan, not only that the flag was taken.
        // A valid index exists (a real open and close), a `Counting` session is detached,
        // `new.md` appears while closed, and the deferred re-open indexes it.
        let dir = fixture("detached-effect");
        let real = plain_state(Duration::from_secs(5));
        open_dir_real(&real, &dir);
        wait_ready(&real);
        real.close_vault();
        let slow = counting(1500);
        let detacher = plain_state(Duration::from_secs(5));
        detacher
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                slow,
            )
            .expect("open");
        detacher.close_vault();
        assert_eq!(detacher.detached_writers().live_count(), 1);
        std::fs::write(dir.join("new.md"), "# written while closed\n").expect("write");

        let second = plain_state(Duration::from_secs(5)).with_detached(detacher.detached_writers());
        let opened = open_dir_real(&second, &dir);
        assert_eq!(opened.index, IndexState::Opening);
        // The tree's refused catch-up is owed as a rescan; the real actor runs it.
        let (send, log) = collecting_sender();
        second.open_tree(send).expect("open_tree");
        wait_until("WalkSettled", || settled(&log));
        wait_ready(&second);
        wait_for_row(&dir, "new.md");
        second.close_vault();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn open_dir_real(state: &AppState, dir: &Path) -> VaultOpened {
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open")
    }

    #[test]
    fn the_boot_info_carries_the_id_the_first_index_status_reports() {
        // Mirrors lib.rs `setup`: step 2 (`state.open_vault(..)`, keeping `opened.root`
        // and `opened.vault_id`) and the `BootInfo { vault, vault_id, open, .. }` it
        // serialises. `setup` installs the `vault://opened` sink after the boot open, so
        // here none is installed either (FR-VLT-04).
        let (state, log, a, _b) = switch_state("opened-boot", |_| OpenHooks::default(), false);
        let opened = open_dir(&state, &a).expect("boot open");
        let boot = crate::ipc::BootInfo {
            vault: Some(opened.root),
            vault_id: Some(opened.vault_id),
            open: None,
            exit_after_ready: false,
        };
        wait_ready(&state);
        assert_eq!(
            boot.vault_id.as_deref(),
            Some(state.index_status().vault_id.as_str())
        );
        let log = log.lock().expect("log");
        let first = log
            .iter()
            .find(|(k, _)| *k == "index://status")
            .expect("a status event");
        assert_eq!(Some(first.1.as_str()), boot.vault_id.as_deref());
        assert!(log.iter().all(|(k, _)| *k != "vault://opened"));
        drop(log);
        state.close_vault();
        let _ = std::fs::remove_dir_all(&a);
    }
}
