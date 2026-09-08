//! The index actor thread, and the status it publishes.
//!
//! Architecture §3.4, first row: **the main thread never opens the index.** The actor
//! opens it on its own thread (ADR-0001) and runs the 5–8ms stamp check there (task #68); the
//! shell paints with whatever state the actor has last published. No command in
//! `contracts/ipc.rs` blocks on any of it — `index_status` reads the published value
//! and returns, which is what makes it callable from a render path.
//!
//! That is the load-bearing property of the whole cold start, and it is a property of
//! *this* file: if [`StatusPublisher::status`] ever took a lock the actor holds while
//! doing work, 400 ms would stop being achievable at 20,000 notes for a reason nobody
//! would find by reading `main`.
//!
//! # What P0 has, and does not
//!
//! The thread, the channel, the published state, the throttle and the event are all
//! real and tested. The stamp check itself is written and landed — I6 is
//! `serat_index::open_or_rebuild` — and task #65 is what wires this crate to it:
//! [`SeratIndexOpen`] now constructs the one production [`serat_core::NoteSource`]
//! (`serat_vault::Vault`) and [`serat_core::NoteParser`] (`serat_parse::CommonMarkParser`)
//! `open_or_rebuild` needs and were the reason the wiring was more than a one-line
//! change. [`IndexState::Ready`] is a state the app can now genuinely reach, and
//! [`IndexState::Unavailable`] means what it says: a rebuild failed, or the index
//! could not be reopened after one succeeded. It is still never a fabricated `Ready`
//! — a shell that claimed an index it does not have would make the cold-start number
//! meaningless and would be exactly the kind of test that cannot fail.
//!
//! Task #89 closes the gap #65 deliberately left: the [`serat_index::IndexHandle`]
//! `open_or_rebuild` returns is now kept, handed back through [`IndexOpen::into_handle`]
//! into an [`IndexHandleSlot`], and `AppState::write_file` submits every save through
//! it. Before this, the app opened an index it never updated — correct exactly until
//! the first save (architecture §3.1.1, §3.2).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ipc::{IndexState, IndexStatus, RebuildReason};
use crate::startup::Startup;

/// Minimum gap between two `index://status` emissions that do not change the kind of
/// state.
///
/// Ten per second, from architecture §3.1 step 6. A rebuild that emitted per note would
/// spend its time in the IPC bridge instead of in the parser; this is the mechanism
/// that prevents it, not a politeness.
pub const THROTTLE: Duration = Duration::from_millis(100);

/// Somewhere an [`IndexStatus`] can be sent.
///
/// In the application this is `AppHandle::emit`; in a test it is a `Vec`. The
/// abstraction exists so the throttle can be tested without a running Tauri
/// application, and for no other reason.
pub trait StatusSink: Send + Sync + 'static {
    /// Deliver one status to the front end.
    fn emit(&self, status: &IndexStatus);
}

/// Holds the last published index state and emits transitions.
///
/// Two readers with different needs: the front end, which subscribes to
/// `index://status` and must not be flooded, and `index_status()`, which polls and must
/// never block. Both are served from one small mutex that is never held across any work
/// — see the module note.
pub struct StatusPublisher<S> {
    vault_id: String,
    startup: Arc<Startup>,
    sink: S,
    throttle: Duration,
    inner: Mutex<Published>,
    observer: Mutex<Option<Observer>>,
}

#[derive(Debug)]
struct Published {
    state: IndexState,
    since_process_start_ms: u64,
    last_emit: Option<Instant>,
}

/// Called for every state the actor publishes, throttled or not.
///
/// Separate from [`StatusSink`] deliberately. The sink is the *event*, and events are
/// throttled to ten per second; this is the *fact*, and the startup trace has to see
/// every one of them — a `t_usable` precondition that could be dropped by a throttle
/// would be a precondition in name only.
type Observer = Box<dyn Fn(&IndexState) + Send + Sync>;

impl<S> std::fmt::Debug for StatusPublisher<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusPublisher")
            .field("vault_id", &self.vault_id)
            .field("state", &self.lock().state)
            .finish_non_exhaustive()
    }
}

impl<S> StatusPublisher<S> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Published> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<S: StatusSink> StatusPublisher<S> {
    /// Start publishing for `vault_id`, in [`IndexState::Opening`].
    ///
    /// `Opening` is the state the shell paints with. §3.4 measures the open check at
    /// about two milliseconds, so it is rarely seen — but "not known yet" has to be a
    /// value the front end can render rather than a null it has to interpret.
    pub fn new(vault_id: impl Into<String>, startup: Arc<Startup>, sink: S) -> Self {
        Self::with_throttle(vault_id, startup, sink, THROTTLE)
    }

    /// As [`StatusPublisher::new`], with an explicit throttle. For tests.
    pub fn with_throttle(
        vault_id: impl Into<String>,
        startup: Arc<Startup>,
        sink: S,
        throttle: Duration,
    ) -> Self {
        let since_process_start_ms = startup.elapsed_ms() as u64;
        Self {
            vault_id: vault_id.into(),
            startup,
            sink,
            throttle,
            inner: Mutex::new(Published {
                state: IndexState::Opening,
                since_process_start_ms,
                last_emit: None,
            }),
            observer: Mutex::new(None),
        }
    }

    /// Watch every published state, including the ones the throttle swallows.
    ///
    /// One observer; installing a second replaces the first. Called before the actor
    /// is spawned, so nothing is missed.
    pub fn on_publish(&self, observer: impl Fn(&IndexState) + Send + Sync + 'static) {
        *self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(observer));
    }

    /// The current status, without blocking and without touching SQLite.
    ///
    /// This is `index_status()`. It reads what the actor last published and returns.
    #[must_use]
    pub fn status(&self) -> IndexStatus {
        let inner = self.lock();
        IndexStatus {
            vault_id: self.vault_id.clone(),
            state: inner.state.clone(),
            since_process_start_ms: inner.since_process_start_ms,
        }
    }

    /// The vault this publisher belongs to.
    #[must_use]
    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    /// Publish a new state.
    ///
    /// The stored state is updated every time, so a poll never sees a stale value. The
    /// *emission* is throttled, and only within one kind of state: a change of kind —
    /// `Opening` to `Rebuilding`, `Rebuilding` to `Ready` — is always emitted
    /// immediately, because throttling a transition is how a UI ends up showing
    /// "rebuilding" over a finished index.
    ///
    /// Returns whether an event was emitted, which is what the throttle test asserts.
    pub fn publish(&self, state: IndexState) -> bool {
        let now_ms = self.startup.elapsed_ms() as u64;

        // Before the throttle, and outside the state lock: the observer records facts
        // the trace depends on, and a throttled event must not be able to hide one.
        if let Some(observer) = self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            observer(&state);
        }

        let status = {
            let mut inner = self.lock();
            let transition = kind_of(&inner.state) != kind_of(&state);
            let due = inner
                .last_emit
                .is_none_or(|last| last.elapsed() >= self.throttle);

            inner.state = state;
            inner.since_process_start_ms = now_ms;

            if !transition && !due {
                return false;
            }
            inner.last_emit = Some(Instant::now());
            IndexStatus {
                vault_id: self.vault_id.clone(),
                state: inner.state.clone(),
                since_process_start_ms: inner.since_process_start_ms,
            }
        };

        // Emitted outside the lock. Holding it across the IPC bridge would let a slow
        // front end block `index_status()`, which is the one thing this type promises
        // it cannot do.
        self.sink.emit(&status);
        true
    }
}

/// The discriminant of an [`IndexState`], for deciding what counts as a transition.
///
/// A `Rebuilding` whose `done` moved is progress; a `Rebuilding` that became `Ready` is
/// a transition. `PartialEq` on the whole value would make every progress tick a
/// transition and defeat the throttle entirely.
fn kind_of(state: &IndexState) -> u8 {
    match state {
        IndexState::Opening => 0,
        IndexState::Ready { .. } => 1,
        IndexState::Rebuilding { .. } => 2,
        IndexState::Unavailable { .. } => 3,
    }
}

/// Opening an index, as the actor thread performs it.
///
/// A trait rather than a function so the actor — the thread, the publishing, the
/// ordering — can be driven by a scripted implementation in a test. It is **not** a
/// device for avoiding a dependency edge: `serat-app` depends on `serat-index` already
/// (architecture §2.1), and the real implementation lives one type below.
pub trait IndexOpen: Send + 'static {
    /// Open the index at `index_dir` for the vault rooted at `vault_root`, publishing
    /// each state as it is reached.
    ///
    /// Runs on the actor's own thread and may block for as long as it needs to. The
    /// contract is that it publishes; it returns nothing, because nothing on the main
    /// thread is waiting for it.
    fn open(&mut self, vault_root: &Path, index_dir: &Path, publish: &dyn Fn(IndexState));

    /// The incremental writer this opener produced, if `open` got far enough to build
    /// one. Called once, by [`IndexActor::spawn`]'s thread, immediately after `open`
    /// returns.
    ///
    /// Defaults to `None` so every existing test double keeps compiling unchanged —
    /// most of them have no real [`serat_index::IndexHandle`] to offer and do not need
    /// one, because nothing in this file's own tests submits an incremental write.
    /// [`SeratIndexOpen`] is the one implementation that overrides it.
    fn into_handle(self) -> Option<serat_index::IndexHandle>
    where
        Self: Sized,
    {
        None
    }
}

/// Where [`AppState::write_file`](crate::state::AppState::write_file) finds the
/// [`serat_index::IndexHandle`] once the actor thread has produced one.
///
/// `Arc<Mutex<Option<..>>>` rather than a channel the caller waits on: `open_vault`
/// must return before the open check — possibly a full rebuild — has finished
/// (§3.4), so there is a real window in which a save can land before a handle exists.
/// A save in that window is not an error; it degrades to the watcher's slower path
/// (FR-VLT-04) instead of NFR-PRF-03's, because nothing recorded an echo for it —
/// design goal 5, applied to a race rather than to a missing capability.
pub type IndexHandleSlot = std::sync::Arc<Mutex<Option<serat_index::IndexHandle>>>;

/// The real open check, wired to the vault and the production parser.
///
/// Architecture §3.4 specifies six ordered steps over `user_version`, `index_meta` and
/// `count(*) FROM notes`, measured at 5–8ms on the 20k fixture (task #68); every one of them
/// reads SQLite, so every one of them lives in `serat-index` (§2.1: `serat-app` owns
/// no domain logic) as [`serat_index::open_or_rebuild`]. This type is the wiring, not
/// a second copy of the check: it opens the one production [`serat_vault::Vault`] as
/// the [`serat_core::NoteSource`] and hands `open_or_rebuild` the one production
/// [`serat_parse::CommonMarkParser`] as the [`serat_core::NoteParser`], because a
/// rebuild has no other way to read or parse a note and duplicating either here would
/// be exactly the divergence §2.1 rules out.
///
/// The [`serat_index::IndexHandle`] `open_or_rebuild` returns is used here for
/// [`serat_index::IndexHandle::note_count`] — read once, to answer the one question
/// [`IndexState::Ready`] carries that `open_or_rebuild`'s status stream does not — and
/// then **kept**, in `handle` below, so [`IndexOpen::into_handle`] can hand it to the
/// actor thread that owns this opener. Task #65 dropped it here deliberately, because
/// nothing on that branch submitted incremental writes to it; task #89 is the writer,
/// and holding the handle open is what makes submitting to it possible at all.
#[derive(Debug, Default)]
pub struct SeratIndexOpen {
    handle: Option<serat_index::IndexHandle>,
}

impl IndexOpen for SeratIndexOpen {
    fn open(&mut self, vault_root: &Path, index_dir: &Path, publish: &dyn Fn(IndexState)) {
        let source = match serat_vault::Vault::open(vault_root) {
            Ok(vault) => vault,
            Err(err) => {
                // The caller already opened this same root successfully before
                // spawning the actor (`AppState::open_vault_with`); reaching this
                // arm means the directory went away or became unreadable in the
                // gap, which is design goal 5's territory, not a bug to unwrap past.
                publish(IndexState::Unavailable {
                    message: format!(
                        "could not reopen the vault to check its index: {err}. Search and \
                         queries are unavailable; the vault may still be editable if the \
                         underlying problem clears."
                    ),
                    path: index_dir.display().to_string(),
                });
                return;
            }
        };
        let parser = serat_parse::CommonMarkParser;

        let result =
            serat_index::open_or_rebuild(vault_root, index_dir, &source, &parser, &|status| {
                // `Ready` is deliberately not published from here: `open_or_rebuild`
                // reaches it before the `IndexHandle` it is about to return exists
                // (see the function's own ordering guarantee), so there is nothing to
                // read `note_count` from yet. The real `Ready` — carrying a real count
                // — is published below, once the handle is in hand. Skipping it here
                // does not reorder anything §3.4 depends on: the load-bearing ordering
                // this task must preserve is `Rebuilding` before the rebuild starts,
                // which this closure still forwards immediately, and untouched.
                if let Some(state) = map_open_status(status, index_dir) {
                    publish(state);
                }
            });

        match result {
            Ok((handle, _report)) => match handle.note_count() {
                Ok(note_count) => {
                    publish(IndexState::Ready { note_count });
                    // Kept, not dropped (task #89): this is the handle `write_file`
                    // will submit saves through, once `into_handle` hands it on.
                    self.handle = Some(handle);
                }
                Err(err) => publish(IndexState::Unavailable {
                    message: format!(
                        "the index opened but its note count could not be read: {err}"
                    ),
                    path: index_dir.display().to_string(),
                }),
            },
            Err(err) => publish(IndexState::Unavailable {
                message: format!(
                    "the index could not be opened or rebuilt: {err}. Search and queries \
                     are unavailable for the rest of this session; the vault remains fully \
                     editable."
                ),
                path: index_dir.display().to_string(),
            }),
        }
    }

    fn into_handle(self) -> Option<serat_index::IndexHandle> {
        self.handle
    }
}

/// Map one [`serat_index::IndexStatus`] onto the contract's [`IndexState`], or
/// `None` for the one variant this file answers itself (see [`SeratIndexOpen::open`]).
///
/// Exhaustive by construction — no wildcard arm — because `serat_index::IndexStatus`
/// and `IndexState` are deliberately the same outcomes under two names (§2.1:
/// `serat-index` may not link `contracts/`), and this match is the one place the two
/// could silently drift if a variant were ever added to one and not mapped here.
fn map_open_status(status: serat_index::IndexStatus, index_dir: &Path) -> Option<IndexState> {
    match status {
        serat_index::IndexStatus::Opening => Some(IndexState::Opening),
        serat_index::IndexStatus::Rebuilding { reason } => Some(IndexState::Rebuilding {
            reason: map_rebuild_reason(reason),
            // `open_or_rebuild` does not report walk/index progress yet — there is
            // no producer of it on this branch — so these are an honest "unknown",
            // not a fabricated count. A future progress-reporting task widens
            // `open_or_rebuild`'s status stream, not this mapping.
            done: 0,
            total: 0,
            walk_complete: false,
        }),
        serat_index::IndexStatus::Ready => None,
        serat_index::IndexStatus::Unavailable => Some(IndexState::Unavailable {
            message: "the rebuild failed; search and queries are unavailable for the rest \
                       of this session, and the vault remains fully editable"
                .to_owned(),
            path: index_dir.display().to_string(),
        }),
    }
}

/// Map one [`serat_index::RebuildReason`] onto the contract's [`RebuildReason`].
///
/// Exhaustive by construction, for the same reason as [`map_open_status`]:
/// `serat-index` cannot depend on `contracts/` (§2.1), so this file is the one place
/// the two enumerations are asserted to agree. `UserRequested` has no `serat_index`
/// counterpart — the open check cannot produce it, only a future manual-rebuild
/// command can — so it is not a case this match needs to cover.
fn map_rebuild_reason(reason: serat_index::RebuildReason) -> RebuildReason {
    match reason {
        serat_index::RebuildReason::NoIndex => RebuildReason::NoIndex,
        serat_index::RebuildReason::Corrupt => RebuildReason::Corrupt,
        serat_index::RebuildReason::SchemaChanged => RebuildReason::IndexVersion,
        serat_index::RebuildReason::Interrupted => RebuildReason::Interrupted,
        serat_index::RebuildReason::VaultMoved => RebuildReason::VaultMoved,
    }
}

/// The index actor: one thread, owning the index, publishing what it finds.
#[derive(Debug)]
pub struct IndexActor {
    handle: Option<std::thread::JoinHandle<()>>,
}

impl IndexActor {
    /// Spawn the actor.
    ///
    /// Returns as soon as the thread is created. Nothing about this call is
    /// proportional to the size of the vault, and nothing on the main thread waits for
    /// what the thread does next — which is the first row of §3.4's table, expressed as
    /// code rather than as a comment.
    ///
    /// Also returns an [`IndexHandleSlot`], empty until `opener` finishes and
    /// [`IndexOpen::into_handle`] fills it (task #89). The caller polls it rather than
    /// waiting on it, for the same reason the caller does not wait on this thread at
    /// all: a save that lands before the slot is filled degrades to the watcher path
    /// instead of blocking.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread, in which case the
    /// caller publishes [`IndexState::Unavailable`] and the shell runs on as an editor.
    pub fn spawn<S: StatusSink, O: IndexOpen>(
        publisher: Arc<StatusPublisher<S>>,
        mut opener: O,
        vault_root: PathBuf,
        index_dir: PathBuf,
    ) -> std::io::Result<(Self, IndexHandleSlot)> {
        let slot: IndexHandleSlot = Arc::new(Mutex::new(None));
        let slot_for_thread = Arc::clone(&slot);
        let handle = std::thread::Builder::new()
            .name("serat-index".to_owned())
            .spawn(move || {
                opener.open(&vault_root, &index_dir, &|state| {
                    publisher.publish(state);
                });
                if let Some(index_handle) = opener.into_handle() {
                    *slot_for_thread
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(index_handle);
                }
            })?;
        Ok((
            Self {
                handle: Some(handle),
            },
            slot,
        ))
    }

    /// Wait for the actor to finish. Tests only; the application never joins it on a
    /// path a user is waiting on.
    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The index directory for a vault: `.serat/` under the root (C-02 — disposable).
#[must_use]
pub fn index_dir_for(vault_root: &Path) -> PathBuf {
    vault_root.join(".serat")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::RebuildReason;

    /// Collects everything emitted, so the throttle can be counted.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<IndexStatus>>>);

    impl Recorder {
        fn taken(&self) -> Vec<IndexStatus> {
            self.0.lock().expect("not poisoned").clone()
        }
    }

    impl StatusSink for Recorder {
        fn emit(&self, status: &IndexStatus) {
            self.0.lock().expect("not poisoned").push(status.clone());
        }
    }

    fn publisher(throttle: Duration) -> (Arc<StatusPublisher<Recorder>>, Recorder) {
        let recorder = Recorder::default();
        let publisher = Arc::new(StatusPublisher::with_throttle(
            "vault-1",
            Arc::new(Startup::begin()),
            recorder.clone(),
            throttle,
        ));
        (publisher, recorder)
    }

    fn rebuilding(done: u32) -> IndexState {
        IndexState::Rebuilding {
            reason: RebuildReason::NoIndex,
            done,
            total: 20_200,
            walk_complete: true,
        }
    }

    #[test]
    fn a_poll_always_sees_the_latest_state_even_when_the_event_was_throttled() {
        // The distinction the type exists for. Throttling the *event* is required;
        // throttling the *value* would make index_status() lie.
        let (publisher, recorder) = publisher(Duration::from_secs(60));
        publisher.publish(rebuilding(1));
        for done in 2..=500 {
            publisher.publish(rebuilding(done));
        }

        assert_eq!(
            publisher.status().state,
            rebuilding(500),
            "the poll must see the newest state, not the last one emitted"
        );
        assert_eq!(
            recorder.taken().len(),
            1,
            "500 progress ticks inside one throttle window are one event"
        );
    }

    #[test]
    fn progress_is_throttled_but_a_change_of_state_never_is() {
        // The mutation this catches: dropping the `transition` term from `publish` so
        // the throttle applies to everything. The UI then shows "rebuilding" over a
        // finished index until the next tick that happens to be due.
        let (publisher, recorder) = publisher(Duration::from_secs(60));

        assert!(
            publisher.publish(rebuilding(1)),
            "Opening -> Rebuilding is a transition"
        );
        assert!(
            !publisher.publish(rebuilding(2)),
            "progress inside the window"
        );
        assert!(
            !publisher.publish(rebuilding(3)),
            "progress inside the window"
        );
        assert!(
            publisher.publish(IndexState::Ready { note_count: 20_200 }),
            "Rebuilding -> Ready must be emitted however recently we emitted"
        );

        let emitted = recorder.taken();
        assert_eq!(emitted.len(), 2, "{emitted:?}");
        assert_eq!(emitted[0].state, rebuilding(1));
        assert_eq!(emitted[1].state, IndexState::Ready { note_count: 20_200 });
    }

    #[test]
    fn the_throttle_lets_progress_through_once_the_window_has_passed() {
        // Without this, `progress_is_throttled...` would also pass against an
        // implementation that emitted progress exactly once and then never again.
        let (publisher, recorder) = publisher(Duration::from_millis(10));
        publisher.publish(rebuilding(1));
        for done in 2..=6 {
            std::thread::sleep(Duration::from_millis(12));
            assert!(
                publisher.publish(rebuilding(done)),
                "tick {done} was due and should have been emitted"
            );
        }
        assert_eq!(recorder.taken().len(), 6);
    }

    #[test]
    fn the_observer_sees_states_the_throttle_swallows() {
        // The startup trace's index-settled mark rides on this. If the observer were
        // called from the emit path instead, a `Rebuilding` that lost its event to the
        // throttle would never settle the mark, and `vault_usable` would wait for a
        // state change that had already happened.
        let (publisher, recorder) = publisher(Duration::from_secs(60));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        publisher.on_publish(move |state| sink.lock().expect("not poisoned").push(state.clone()));

        for done in 1..=5 {
            publisher.publish(rebuilding(done));
        }

        assert_eq!(
            recorder.taken().len(),
            1,
            "the throttle should have swallowed four of the five events"
        );
        assert_eq!(
            seen.lock().expect("not poisoned").len(),
            5,
            "the observer must see every published state, throttled or not"
        );
    }

    #[test]
    fn every_status_carries_the_vault_it_belongs_to() {
        // A front end that has switched vaults discards statuses that do not match
        // (contracts/ipc.rs, IndexStatus::vault_id). It cannot do that if we do not
        // stamp them.
        let (publisher, recorder) = publisher(Duration::ZERO);
        publisher.publish(IndexState::Ready { note_count: 3 });
        assert_eq!(publisher.status().vault_id, "vault-1");
        assert!(recorder.taken().iter().all(|s| s.vault_id == "vault-1"));
    }

    #[test]
    fn the_actor_publishes_from_its_own_thread_and_the_caller_does_not_wait() {
        /// Publishes a full rebuild, slowly enough that a caller who waited would be
        /// caught doing so.
        struct SlowRebuild;
        impl IndexOpen for SlowRebuild {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                std::thread::sleep(Duration::from_millis(80));
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 0,
                    walk_complete: false,
                });
                std::thread::sleep(Duration::from_millis(80));
                publish(IndexState::Ready { note_count: 7 });
            }
        }

        let (publisher, recorder) = publisher(Duration::ZERO);
        let started = Instant::now();
        let (mut actor, _handle) = IndexActor::spawn(
            Arc::clone(&publisher),
            SlowRebuild,
            PathBuf::from("."),
            PathBuf::from("./.serat"),
        )
        .expect("spawn");
        let spawn_cost = started.elapsed();

        // §3.4: the main thread never opens the index. If `spawn` ever became
        // synchronous this is the assertion that notices.
        assert!(
            spawn_cost < Duration::from_millis(50),
            "spawning the index actor blocked the caller for {spawn_cost:?}"
        );
        assert_eq!(
            publisher.status().state,
            IndexState::Opening,
            "the shell paints Opening while the actor is still working"
        );

        actor.join();
        assert_eq!(
            publisher.status().state,
            IndexState::Ready { note_count: 7 }
        );
        assert_eq!(recorder.taken().len(), 2);
    }

    /// A throwaway, empty vault directory, unique per test run.
    ///
    /// Real directories rather than `.` or a fixed name: task #65's wiring runs a
    /// genuine `open_or_rebuild` (real SQLite file, real `.serat/`), and a fixed or
    /// relative root would create that state in whatever directory `cargo test`
    /// happens to run from — the failure this replaces (`PathBuf::from(".")` against
    /// the crate's own source tree) rather than merely a narrower one.
    fn throwaway_vault(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("serat-app-index-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    #[test]
    fn the_p0_open_check_now_reaches_a_real_ready_over_an_empty_vault() {
        // #65: SeratIndexOpen is wired to the real check. An empty vault has no
        // `.serat/`, so the check reaches `NoIndex`, `open_or_rebuild` runs a
        // (trivial) rebuild, and the state that would make the cold-start number
        // meaningless — a fabricated `Ready` — is exactly what this is not: the
        // count came from a database `open_or_rebuild` actually built.
        let root = throwaway_vault("ready");
        let (publisher, _) = publisher(Duration::ZERO);
        let (mut actor, handle_slot) = IndexActor::spawn(
            Arc::clone(&publisher),
            SeratIndexOpen::default(),
            root.clone(),
            index_dir_for(&root),
        )
        .expect("spawn");
        actor.join();

        assert_eq!(
            publisher.status().state,
            IndexState::Ready { note_count: 0 },
            "an empty vault should index to zero notes, not stay Unavailable"
        );
        assert!(
            handle_slot.lock().expect("not poisoned").is_some(),
            "task #89: a successful open must hand its IndexHandle on through \
             into_handle, not drop it"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_wiring_publishes_rebuilding_before_the_rebuild_starts_and_ready_after() {
        // The ordering `open_or_rebuild`'s own doc calls load-bearing (§3.4:
        // `t_usable` waits for the index state to be *known*, and `Rebuilding`
        // settles that as much as `Ready` does): reordering or swallowing it here
        // would let the window's cold-start budget silently start including a full
        // reindex. `on_publish` sees every state, throttled or not, so this pins
        // the sequence rather than only the final value.
        let root = throwaway_vault("ordering");
        let (publisher, _) = publisher(Duration::from_secs(60));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        publisher.on_publish(move |state| sink.lock().expect("not poisoned").push(state.clone()));

        let (mut actor, _handle) = IndexActor::spawn(
            Arc::clone(&publisher),
            SeratIndexOpen::default(),
            root.clone(),
            index_dir_for(&root),
        )
        .expect("spawn");
        actor.join();

        assert_eq!(
            seen.lock().expect("not poisoned").clone(),
            vec![
                IndexState::Opening,
                IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 0,
                    walk_complete: false,
                },
                IndexState::Ready { note_count: 0 },
            ]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_index_directory_is_dot_serat_under_the_root() {
        assert_eq!(
            index_dir_for(Path::new("/vault")),
            PathBuf::from("/vault").join(".serat")
        );
    }
}
