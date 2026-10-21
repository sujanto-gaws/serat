//! The vault thread: register the watcher, then consume what it reports.
//!
//! Architecture §3.4 states the order as correctness rather than optimisation:
//!
//! > **The watcher is registered before the walk starts.** A file changed during the
//! > walk then produces an event rather than being silently missed, and the walk
//! > reconciles anything that changed before the watcher existed.
//!
//! Swap those two and the failure is a note that is missing from the index with nothing
//! anywhere recording that it was ever seen — the one failure C-02 cannot absorb,
//! because a disposable index only helps if a rebuild would find the file. The walk is
//! no longer this thread's (ADR-0015 section 2: the one authoritative walk at open is
//! the tree's, and it feeds the catch-up), so the rule is kept by a latch:
//! [`Stage::WatcherRegistered`] or [`Stage::WatcherFailed`] is reported first, and the
//! application's tree walk waits on a [`Latch`] set by that stage
//! (`the_tree_walk_waits_for_the_watcher` in `state.rs`).
//!
//! # The walk waits for the first paint, and that is a measurement
//!
//! §3.4 puts the walk off the cold-start critical path and states the property the
//! whole section rests on: *nothing on the critical path is proportional to the size of
//! the vault*. Starting the walk as soon as the vault opens does not achieve that, and
//! the failure is not subtle — measured on this machine, release build, 20 spawns each:
//!
//! | Vault | `t0 -> window visible`, median | `WebviewWindowBuilder::build`, median |
//! |---|---|---|
//! | 2 notes | 808 ms | 636 ms |
//! | 20,000 notes, walk from open | **1410 ms** | **1152 ms** |
//! | 20,000 notes, walk delayed | 821 ms | 604 ms |
//!
//! The third row is the same code with the walk moved out of the window. It lands back
//! on the two-note figure, so the growth was entirely the walk: another thread on the
//! same disk and the same cores is still on the critical path, and "off the critical
//! path" cannot mean "on another thread". So the (tree) walk waits for the first
//! painted frame, after the watcher is registered.
//!
//! # What the thread does with the events
//!
//! It is the consumer of architecture section 3.1.2 step 1: pop one path, drain the
//! rest, hand the batch to [`Live::paths_changed`] (one `Reconciler::index_paths` per
//! batch in the application), act on `DirChanged` and on a rescan, and poll
//! [`Live::tick`] sleeping no longer than it asks. Echo suppression (section 3.2) is
//! the `(path, hash)` test the reconciler is given. The thread never walks the vault:
//! the open's catch-up is fed by the tree walk (ADR-0015 section 2), and only a
//! `RescanRequired` or an overflow walks, from [`Live::rescan_required`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serat_core::VaultPath;
use serat_vault::{Vault, VaultEvent, Watcher, WatcherConfig, WorkQueue};

/// How far the vault thread has got, reported as it gets there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The filesystem watcher is subscribed to the root. Always first.
    WatcherRegistered,
    /// The watcher could not be started. The vault still opens and still walks: a
    /// vault with no watcher is stale, not broken (design goal 5).
    WatcherFailed,
}

/// A one-shot flag that can be waited on: set once, never cleared.
#[derive(Debug, Default)]
pub struct Latch {
    set: Mutex<bool>,
    changed: Condvar,
}

impl Latch {
    /// Set the latch and wake every waiter.
    pub fn set(&self) {
        *self.set.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    /// Whether it is set.
    #[must_use]
    pub fn is_set(&self) -> bool {
        *self.set.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait until it is set or `timeout` passes. Returns whether it is set.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let guard = self.set.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |set| !*set)
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }
}

/// A running vault thread.
#[derive(Debug)]
pub struct VaultThread {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl VaultThread {
    /// Spawn the thread and return immediately.
    ///
    /// Nothing the caller does after this is proportional to the size of the vault,
    /// which is the point: `open_vault` returns as soon as the root is canonicalised
    /// and readable (`contracts/ipc.rs`).
    ///
    /// `on_stage` is called from the vault thread. It must not block.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread.
    pub fn spawn<F>(vault: Arc<Vault>, on_stage: F) -> std::io::Result<Self>
    where
        F: Fn(Stage) + Send + 'static,
    {
        Self::spawn_with(vault, on_stage, None)
    }

    /// As [`Self::spawn`], with the consumer of what the watcher reports. Without one
    /// the thread drains the queue and discards it.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread.
    pub fn spawn_with<F>(
        vault: Arc<Vault>,
        on_stage: F,
        live: Option<Arc<dyn Live>>,
    ) -> std::io::Result<Self>
    where
        F: Fn(Stage) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("serat-vault".to_owned())
            .spawn(move || run(&vault, &on_stage, &thread_stop, live.as_deref()))?;

        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// Ask the thread to stop and wait for it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for VaultThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What the vault thread hands the watcher's output to (the file tree, the editor
/// buffers, the index). Called on the vault thread, so an implementation must not panic
/// into it and must not block for long: the watcher's queue fills behind a slow
/// consumer, and a full queue is a rescan.
pub trait Live: Send + Sync {
    /// A file the watcher reported, after its coalescing window.
    fn path_changed(&self, path: &VaultPath);
    /// The paths of one drained batch, after each has been given to
    /// [`Live::path_changed`]. The index is brought up to date here, once per batch.
    fn paths_changed(&self, _paths: &[VaultPath]) {}
    /// A directory appeared, moved or was renamed: only that subtree needs
    /// reconciling.
    fn dir_changed(&self, _dir: &VaultPath) {}
    /// The watcher lost events, or the queue overflowed: the whole vault may disagree
    /// with what was last said.
    fn rescan_required(&self);
    /// The loop's tick, once per turn. Returns when it next has something to do, so the
    /// loop sleeps no longer than that.
    fn tick(&self, _now: Instant) -> Option<Instant> {
        None
    }
}

/// The longest the consumer sleeps between turns.
const IDLE_TICK: Duration = Duration::from_millis(50);

/// The thread body, in the order §3.4 requires.
fn run<F: Fn(Stage)>(vault: &Arc<Vault>, on_stage: &F, stop: &AtomicBool, live: Option<&dyn Live>) {
    let queue = Arc::new(WorkQueue::new());

    // 1. Watcher first. A failure is reported and is not fatal.
    let watcher = match Watcher::start(vault, Arc::clone(&queue), WatcherConfig::default()) {
        Ok(watcher) => {
            on_stage(Stage::WatcherRegistered);
            Some(watcher)
        }
        Err(_) => {
            on_stage(Stage::WatcherFailed);
            None
        }
    };

    // 2. Then consume, so the queue does not fill and latch an overflow that would be
    //    reported as data loss when in fact there is no consumer yet (P0 finding F-11
    //    is about the recovery path; this is about not manufacturing the condition).
    let mut next_due: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let wait = next_due.map_or(IDLE_TICK, |due| {
            due.saturating_duration_since(Instant::now()).min(IDLE_TICK)
        });
        let popped = queue.pop_timeout(wait);
        let Some(live) = live else {
            continue;
        };
        if let Some(first) = popped {
            // One path popped, the rest drained: one batch (section 3.1.2 step 1).
            let mut batch = vec![first];
            batch.extend(queue.drain());
            for path in &batch {
                live.path_changed(path);
            }
            live.paths_changed(&batch);
        }
        // A directory changed, the platform dropped events, or the queue overflowed.
        // The ticket is taken before the rescan so an overflow during it is not cleared.
        let events = watcher
            .as_ref()
            .map(Watcher::take_events)
            .unwrap_or_default();
        let ticket = queue.rescan_needed();
        let rescan = ticket.is_some() || events.contains(&VaultEvent::RescanRequired);
        if rescan {
            // A rescan covers every directory event.
            live.rescan_required();
            if let Some(ticket) = ticket {
                queue.rescan_completed(ticket);
            }
        } else {
            for event in &events {
                if let VaultEvent::DirChanged(dir) = event {
                    live.dir_changed(dir);
                }
            }
        }
        next_due = live.tick(Instant::now());
    }

    if let Some(watcher) = watcher {
        watcher.stop();
    }
    queue.close();
}

/// Open a vault root, mapping the failure to the boundary's `VaultNotFound`.
///
/// The one absolute path that crosses the IPC boundary (`contracts/ipc.rs` rule 2), so
/// it is also the one place a host path is canonicalised. Everything downstream is a
/// `VaultPath`.
///
/// # Errors
///
/// The `serat-vault` failure, unchanged; the caller decides how it crosses.
pub fn open(root: &Path) -> Result<Vault, serat_core::VaultError> {
    Vault::open(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "serat-vault-thread-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("10-projects")).expect("fixture");
        std::fs::write(dir.join("10-projects/a.md"), "# a\n").expect("fixture");
        dir
    }

    #[test]
    fn the_watcher_is_reported_registered() {
        let dir = fixture("order");
        let vault = Arc::new(Vault::open(&dir).expect("open"));
        let stages = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stages);
        let mut thread = VaultThread::spawn(vault, move |stage| {
            sink.lock().expect("not poisoned").push(stage);
        })
        .expect("spawn");
        for _ in 0..200 {
            if !stages.lock().expect("not poisoned").is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.stop();
        assert_eq!(
            *stages.lock().expect("not poisoned"),
            vec![Stage::WatcherRegistered]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_latch_wakes_a_waiter_and_times_out_otherwise() {
        let latch = Arc::new(Latch::default());
        assert!(!latch.wait(Duration::from_millis(20)));
        let setter = Arc::clone(&latch);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            setter.set();
        });
        assert!(latch.wait(Duration::from_secs(5)));
        assert!(latch.is_set());
    }

    #[test]
    fn spawning_the_thread_does_not_wait_for_anything() {
        // The property the 400 ms budget rests on. If `spawn` ever became synchronous
        // — a `join` added "to make the test deterministic" is the usual way — this
        // notices.
        let dir = fixture("nonblocking");
        let vault = Arc::new(Vault::open(&dir).expect("open"));
        let started = std::time::Instant::now();
        let mut thread = VaultThread::spawn(vault, |_| {
            std::thread::sleep(Duration::from_millis(200));
        })
        .expect("spawn");
        let cost = started.elapsed();
        thread.stop();

        assert!(
            cost < Duration::from_millis(100),
            "spawning the vault thread blocked the caller for {cost:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
