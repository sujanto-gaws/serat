//! The vault thread: register the watcher, then walk.
//!
//! Architecture §3.4 states the order as correctness rather than optimisation:
//!
//! > **The watcher is registered before the walk starts.** A file changed during the
//! > walk then produces an event rather than being silently missed, and the walk
//! > reconciles anything that changed before the watcher existed.
//!
//! Swap those two and the failure is a note that is missing from the index with nothing
//! anywhere recording that it was ever seen — the one failure C-02 cannot absorb,
//! because a disposable index only helps if a rebuild would find the file. So the order
//! is not left to the shape of the code: [`Stage`] is reported as each step is reached
//! and `the_watcher_is_registered_before_the_walk_starts` asserts the sequence.
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
//! path" cannot mean "on another thread". P0 finding F-12 — `fs::canonicalize` per
//! entry, ~2.0 s on the 20k vault — is most of what that thread was doing.
//!
//! So the walk waits for the first painted frame. The watcher is still registered
//! *before* the wait, which is what makes the delay safe: the ordering rule exists
//! precisely so that anything changing before the walk produces an event instead of
//! being missed, and that guarantee is unchanged. Nothing waits on the walk — P0's
//! "usable" is the buffer and the watcher — so this costs nothing it protects.
//!
//! # What P0 does with the events
//!
//! Nothing, and it says so. The consumer of the work queue is the incremental save path
//! (task I5) over the index actor (I4/I6), and neither exists on this branch. The
//! thread drains the queue and counts what it drained, so the watcher is genuinely
//! running and genuinely coalescing rather than being wired to a socket nobody reads —
//! but no file is reindexed, because there is no index. Echo suppression (§3.2) is
//! likewise not wired here: P0 finding F-13(c) puts that inside
//! `serat_vault::Vault::write_atomic`, which is rust-core-dev's fix on task I2, not
//! something the shell can bolt on from outside.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serat_core::{SourceEntry, VaultPath};
use serat_vault::{Vault, Watcher, WatcherConfig, WorkQueue};

/// How far the vault thread has got, reported as it gets there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The filesystem watcher is subscribed to the root. Always first.
    WatcherRegistered,
    /// The watcher could not be started. The vault still opens and still walks: a
    /// vault with no watcher is stale, not broken (design goal 5).
    WatcherFailed,
    /// The walk is about to enumerate the vault. Always after one of the two above.
    WalkStarted,
    /// The walk finished, having found this many files.
    WalkComplete(u32),
    /// The walk failed outright — the root became unreadable underneath us.
    WalkFailed,
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
    /// and readable (`contracts/ipc.rs`), and the walk arrives afterwards.
    ///
    /// `gate` is called once, **after** the watcher is registered and **before** the
    /// walk starts, and is expected to block until the walk may run — see the module
    /// note for why that is a measurement rather than a preference.
    ///
    /// `on_stage` is called from the vault thread. It must not block — it is used to
    /// stamp the startup trace and to count entries, and a slow callback here would put
    /// the caller's work back on this thread.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread.
    pub fn spawn<F, G>(vault: Arc<Vault>, gate: G, on_stage: F) -> std::io::Result<Self>
    where
        F: Fn(Stage) + Send + 'static,
        G: FnOnce() + Send + 'static,
    {
        Self::spawn_with(vault, gate, on_stage, None)
    }

    /// As [`Self::spawn`], with the consumer of what the watcher reports. Without one
    /// the thread drains the queue and discards it, as at P0.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread.
    pub fn spawn_with<F, G>(
        vault: Arc<Vault>,
        gate: G,
        on_stage: F,
        live: Option<Arc<dyn Live>>,
    ) -> std::io::Result<Self>
    where
        F: Fn(Stage) + Send + 'static,
        G: FnOnce() + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("serat-vault".to_owned())
            .spawn(move || run(&vault, gate, &on_stage, &thread_stop, live.as_deref()))?;

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
/// buffers). Called on the vault thread, so an implementation must not panic into it
/// and must not block for long: the watcher's queue fills behind a slow consumer, and
/// a full queue is a rescan.
pub trait Live: Send + Sync {
    /// A file the watcher reported, after its coalescing window.
    fn path_changed(&self, path: &VaultPath);
    /// The watcher lost events, or a directory changed under it: the whole vault may
    /// disagree with what was last said.
    fn rescan_required(&self);
}

/// The thread body, in the order §3.4 requires.
fn run<F: Fn(Stage), G: FnOnce()>(
    vault: &Arc<Vault>,
    gate: G,
    on_stage: &F,
    stop: &AtomicBool,
    live: Option<&dyn Live>,
) {
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

    // 2. Then wait for the shell to paint, so the walk is not competing with the
    //    webview for the disk and the cores it is starting up on. The watcher is
    //    already live above, so nothing changing during this wait is lost.
    gate();

    // 3. Then the walk.
    on_stage(Stage::WalkStarted);
    match vault.walk() {
        Ok(entries) => on_stage(Stage::WalkComplete(entry_count(&entries))),
        Err(_) => on_stage(Stage::WalkFailed),
    }

    // 4. Then drain, so the queue does not fill and latch an overflow that would be
    //    reported as data loss when in fact there is no consumer yet (P0 finding F-11
    //    is about the recovery path; this is about not manufacturing the condition).
    while !stop.load(Ordering::Relaxed) {
        let popped = queue.pop_timeout(Duration::from_millis(50));
        let Some(live) = live else {
            continue;
        };
        if let Some(path) = popped {
            live.path_changed(&path);
        }
        // A directory changed, the platform dropped events, or the queue overflowed.
        // The ticket is taken before the rescan so an overflow during it is not cleared.
        let events = watcher
            .as_ref()
            .map(Watcher::take_events)
            .unwrap_or_default();
        let ticket = queue.rescan_needed();
        if !events.is_empty() || ticket.is_some() {
            live.rescan_required();
            if let Some(ticket) = ticket {
                queue.rescan_completed(ticket);
            }
        }
    }

    if let Some(watcher) = watcher {
        watcher.stop();
    }
    queue.close();
}

/// Entries as a `u32`, saturating. A vault with more than four billion files is not a
/// case worth a `u64` in a progress counter.
fn entry_count(entries: &[SourceEntry]) -> u32 {
    u32::try_from(entries.len()).unwrap_or(u32::MAX)
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
        std::fs::write(dir.join("10-projects/b.md"), "# b\n").expect("fixture");
        std::fs::write(dir.join("readme.md"), "# r\n").expect("fixture");
        dir
    }

    fn record(dir: &Path) -> Vec<Stage> {
        let vault = Arc::new(Vault::open(dir).expect("open"));
        let stages = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stages);
        let mut thread = VaultThread::spawn(
            vault,
            || {},
            move |stage| {
                sink.lock().expect("not poisoned").push(stage);
            },
        )
        .expect("spawn");

        // Wait for the walk to report, then stop.
        for _ in 0..200 {
            if stages
                .lock()
                .expect("not poisoned")
                .iter()
                .any(|s| matches!(s, Stage::WalkComplete(_) | Stage::WalkFailed))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.stop();
        stages.lock().expect("not poisoned").clone()
    }

    #[test]
    fn the_watcher_is_registered_before_the_walk_starts() {
        // §3.4's first ordering rule, which is correctness and not optimisation. Move
        // `Watcher::start` below `vault.walk()` in `run` and this fails.
        let dir = fixture("order");
        let stages = record(&dir);

        let watcher = stages
            .iter()
            .position(|s| matches!(s, Stage::WatcherRegistered | Stage::WatcherFailed))
            .expect("the thread reports what it did with the watcher");
        let walk = stages
            .iter()
            .position(|s| matches!(s, Stage::WalkStarted))
            .expect("the walk started");
        assert!(
            watcher < walk,
            "the walk started before the watcher was registered: {stages:?}"
        );
        assert_eq!(stages[watcher], Stage::WatcherRegistered, "{stages:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_watcher_is_registered_before_the_gate_and_the_walk_after_it() {
        // The whole safety argument for delaying the walk: the watcher is live during
        // the wait, so a file changed in that window produces an event rather than
        // being missed. Move `gate()` above `Watcher::start` in `run` and this fails.
        let dir = fixture("gate");
        let vault = Arc::new(Vault::open(&dir).expect("open"));
        let stages = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::new(Mutex::new(Vec::new()));

        let sink = Arc::clone(&stages);
        let seen_at_gate = Arc::clone(&opened);
        let recorded = Arc::clone(&stages);
        let mut thread = VaultThread::spawn(
            vault,
            move || {
                // What the thread had reported by the time the gate was reached.
                *seen_at_gate.lock().expect("not poisoned") =
                    recorded.lock().expect("not poisoned").clone();
                std::thread::sleep(Duration::from_millis(40));
            },
            move |stage| sink.lock().expect("not poisoned").push(stage),
        )
        .expect("spawn");

        for _ in 0..200 {
            if stages
                .lock()
                .expect("not poisoned")
                .iter()
                .any(|s| matches!(s, Stage::WalkComplete(_) | Stage::WalkFailed))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.stop();

        let at_gate = opened.lock().expect("not poisoned").clone();
        assert!(
            at_gate
                .iter()
                .any(|s| matches!(s, Stage::WatcherRegistered | Stage::WatcherFailed)),
            "the gate was reached before the watcher was registered: {at_gate:?}"
        );
        assert!(
            !at_gate.iter().any(|s| matches!(s, Stage::WalkStarted)),
            "the walk started before the gate: {at_gate:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_walk_finds_the_notes_and_reports_a_count() {
        let dir = fixture("count");
        let stages = record(&dir);
        let count = stages
            .iter()
            .find_map(|s| match s {
                Stage::WalkComplete(n) => Some(*n),
                _ => None,
            })
            .expect("the walk completed");
        assert_eq!(count, 3, "{stages:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawning_the_thread_does_not_wait_for_the_walk() {
        // The property the 400 ms budget rests on. If `spawn` ever became synchronous
        // — a `join` added "to make the test deterministic" is the usual way — this
        // notices.
        let dir = fixture("nonblocking");
        let vault = Arc::new(Vault::open(&dir).expect("open"));
        let started = std::time::Instant::now();
        let mut thread = VaultThread::spawn(
            vault,
            || {},
            |_| {
                std::thread::sleep(Duration::from_millis(200));
            },
        )
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
