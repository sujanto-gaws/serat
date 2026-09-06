//! The index actor thread, and the status it publishes.
//!
//! Architecture §3.4, first row: **the main thread never opens the index.** The actor
//! opens it on its own thread (ADR-0001) and runs the 2.4 ms stamp check there; the
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
//! real and tested. The stamp check itself is written and landed — I6 is on main
//! as `serat_index::open_or_rebuild` — but **this crate has not been wired to it**,
//! and that is not a one-line change: `open_or_rebuild` takes a `NoteSource` and a
//! `NoteParser`, and this file constructs neither. [`SeratIndexOpen`] therefore
//! still publishes [`IndexState::Unavailable`] naming I6, which remains the honest
//! state — search and
//! queries are unavailable and the vault is fully editable, which is design goal 1
//! doing its job. It is deliberately not a fabricated `Ready`: a shell that claimed an
//! index it does not have would make the cold-start number meaningless and would be
//! exactly the kind of test that cannot fail.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ipc::{IndexState, IndexStatus};
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
}

/// The seam where the real open check goes — still a seam, but no longer for want
/// of the check.
///
/// Architecture §3.4 specifies six ordered steps over `user_version`, `index_meta` and
/// `count(*) FROM notes`, measured at 2.4 ms on the 20k fixture. Every one of them
/// reads SQLite, so every one of them belongs to `serat-index` (§2.1: `serat-app` owns
/// no domain logic). That is task I6, and it **is** written: `open_or_rebuild` landed
/// on main (F-366). What is still missing is the wiring, not the check.
///
/// Until that wiring lands, this publishes [`IndexState::Unavailable`] with a message
/// that says so. The alternative — a stamp check re-implemented in the app crate —
/// would put schema knowledge in the wrong crate, and I6 having landed does not weaken
/// that argument but strengthens it: the check now exists in the crate that owns it, so
/// a second copy here would be a divergence rather than a stopgap.
#[derive(Debug, Default)]
pub struct SeratIndexOpen;

impl IndexOpen for SeratIndexOpen {
    fn open(&mut self, _vault_root: &Path, index_dir: &Path, publish: &dyn Fn(IndexState)) {
        publish(IndexState::Unavailable {
            message: format!(
                "index open check not wired up yet (task I6 has landed in serat-index; this \
                 app crate is not connected to it yet). serat-index is at index v{} on \
                 sqlite {}, and does expose serat_index::open_or_rebuild. Search and \
                 queries are unavailable; the vault is fully editable.",
                serat_index::INDEX_VERSION,
                serat_index::sqlite_version(),
            ),
            path: index_dir.display().to_string(),
        });
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
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread, in which case the
    /// caller publishes [`IndexState::Unavailable`] and the shell runs on as an editor.
    pub fn spawn<S: StatusSink, O: IndexOpen>(
        publisher: Arc<StatusPublisher<S>>,
        mut opener: O,
        vault_root: PathBuf,
        index_dir: PathBuf,
    ) -> std::io::Result<Self> {
        let handle = std::thread::Builder::new()
            .name("serat-index".to_owned())
            .spawn(move || {
                opener.open(&vault_root, &index_dir, &|state| {
                    publisher.publish(state);
                });
            })?;
        Ok(Self {
            handle: Some(handle),
        })
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
        let mut actor = IndexActor::spawn(
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

    #[test]
    fn the_p0_open_check_says_it_is_missing_rather_than_claiming_an_index() {
        // The state that would make the cold-start number meaningless is a fabricated
        // `Ready`. Assert that this is not it.
        let (publisher, _) = publisher(Duration::ZERO);
        let mut actor = IndexActor::spawn(
            Arc::clone(&publisher),
            SeratIndexOpen,
            PathBuf::from("."),
            index_dir_for(Path::new(".")),
        )
        .expect("spawn");
        actor.join();

        match publisher.status().state {
            IndexState::Unavailable { message, path } => {
                assert!(message.contains("I6"), "{message}");
                assert!(path.ends_with(".serat"), "{path}");
            }
            other => panic!("P0 must not claim an index it does not have: {other:?}"),
        }
    }

    #[test]
    fn the_index_directory_is_dot_serat_under_the_root() {
        assert_eq!(
            index_dir_for(Path::new("/vault")),
            PathBuf::from("/vault").join(".serat")
        );
    }
}
