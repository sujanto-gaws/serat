//! The filesystem watcher and its coalescing window.
//!
//! Architecture §3.1 step 1. `notify` reports what the platform reports, which is
//! more events than there were user actions: an editor saving through temp-and-rename
//! produces a create, a write, a rename and an attribute change for one Ctrl-S. This
//! module turns that back into one unit of work per path.
//!
//! # What lands on the queue is a path, not an event kind
//!
//! Deliberately. The watcher does not tell the consumer whether a file was created,
//! modified or deleted, because by the time the consumer looks, that answer may be
//! wrong — a create followed by a delete inside the window is a file that does not
//! exist, and a delete followed by a create is a file that does. The path is the
//! durable fact; the file's state is read when its turn comes, and whatever is true
//! then is what gets indexed. This removes a whole category of event-ordering bugs by
//! not having event ordering.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as _};
use serat_core::{VaultError, VaultPath};

use crate::{Vault, WorkQueue};

/// The coalescing window from architecture §3.1 step 1.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(150);

/// How often the coalescer wakes to flush paths whose window has closed.
///
/// A fifth of the window: fine enough that the effective delay is 150–180 ms rather
/// than 150–300, coarse enough that an idle vault costs 33 wakeups a second on one
/// thread.
const TICK: Duration = Duration::from_millis(30);

/// How the watcher behaves.
#[derive(Debug, Clone, Copy)]
pub struct WatcherConfig {
    /// How long to gather events for one path before enqueueing it.
    ///
    /// This is a floor on how quickly an *external* change reaches the index, and it
    /// is not on the path NFR-PRF-03 measures: when Serat itself saves a buffer, the
    /// application submits the work directly and does not wait to be told about its
    /// own write. Changing this trades responsiveness to other tools against how many
    /// times a file that someone else is actively rewriting gets reparsed.
    pub coalesce_window: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            coalesce_window: COALESCE_WINDOW,
        }
    }
}

/// Something the watcher noticed that is not a single changed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultEvent {
    /// The watcher lost events and the vault must be walked and reconciled.
    ///
    /// Emitted when the platform says it dropped notifications, or when the work
    /// queue overflowed. Both mean the same thing to the consumer, so they are one
    /// signal: the index may now disagree with the disk, and only a walk can settle
    /// it.
    RescanRequired,
}

/// A running watcher.
///
/// Dropping it stops the watching thread. It owns no queue: the caller supplies one,
/// because the queue is also fed by the initial walk and by rescans, and two producers
/// into one bounded queue is exactly the arrangement backpressure is defined against.
#[derive(Debug)]
pub struct Watcher {
    queue: Arc<WorkQueue>,
    events: Arc<Mutex<Vec<VaultEvent>>>,
    stop: Arc<Mutex<bool>>,
    handle: Option<JoinHandle<()>>,
    // Held to keep the platform watcher alive; dropping it unsubscribes.
    _inner: notify::RecommendedWatcher,
}

impl Watcher {
    /// Start watching `vault`, feeding coalesced paths into `queue`.
    ///
    /// # Errors
    ///
    /// [`VaultError::Watcher`] if the platform watcher cannot be created or the root
    /// cannot be subscribed to.
    pub fn start(
        vault: &Vault,
        queue: Arc<WorkQueue>,
        config: WatcherConfig,
    ) -> Result<Self, VaultError> {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut inner = notify::recommended_watcher(move |event| {
            // A send failure means the coalescer is gone, which happens during
            // shutdown and is not worth reporting.
            let _ = tx.send(event);
        })
        .map_err(|err| VaultError::Watcher {
            message: err.to_string(),
        })?;

        inner
            .watch(vault.root(), RecursiveMode::Recursive)
            .map_err(|err| VaultError::Watcher {
                message: format!("watching {}: {err}", vault.root().display()),
            })?;

        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(Mutex::new(false));

        let handle = {
            let vault = vault.clone();
            let queue = Arc::clone(&queue);
            let events = Arc::clone(&events);
            let stop = Arc::clone(&stop);
            let window = config.coalesce_window;
            std::thread::Builder::new()
                .name("serat-watcher".to_owned())
                .spawn(move || coalesce(&vault, &rx, &queue, &events, &stop, window))
                .map_err(|err| VaultError::Watcher {
                    message: format!("spawning the watcher thread: {err}"),
                })?
        };

        Ok(Self {
            queue,
            events,
            stop,
            handle: Some(handle),
            _inner: inner,
        })
    }

    /// Take any events raised since this was last called.
    ///
    /// Separate from the queue because these are not work items: [`VaultEvent`] says
    /// something about the watcher, not about one file.
    pub fn take_events(&self) -> Vec<VaultEvent> {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *events)
    }

    /// The queue this watcher feeds.
    #[must_use]
    pub fn queue(&self) -> &Arc<WorkQueue> {
        &self.queue
    }

    /// Stop the watching thread and wait for it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Ok(mut stop) = self.stop.lock() {
            *stop = true;
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The coalescing loop.
///
/// One `HashMap` of path to deadline. An event for a path already pending is absorbed
/// — that is the coalescing — and the deadline is *not* extended, so a file being
/// rewritten continuously is still indexed every window rather than never.
fn coalesce(
    vault: &Vault,
    rx: &mpsc::Receiver<notify::Result<notify::Event>>,
    queue: &WorkQueue,
    events: &Mutex<Vec<VaultEvent>>,
    stop: &Mutex<bool>,
    window: Duration,
) {
    let mut pending: HashMap<VaultPath, Instant> = HashMap::new();

    loop {
        if stop.lock().is_ok_and(|stop| *stop) {
            break;
        }

        match rx.recv_timeout(TICK) {
            Ok(Ok(event)) => {
                if event.need_rescan() {
                    raise(events, VaultEvent::RescanRequired);
                }
                if is_interesting(&event.kind) {
                    let deadline = Instant::now() + window;
                    for path in &event.paths {
                        if let Some(path) = accept(vault, path) {
                            pending.entry(path).or_insert(deadline);
                        }
                    }
                }
            }
            // A watcher error is usually one unreadable directory, not a dead
            // watcher, so it is recorded and the loop continues. Losing the whole
            // watcher because one folder was locked would be worse than the error.
            Ok(Err(_)) => raise(events, VaultEvent::RescanRequired),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        flush(&mut pending, queue, events);
    }

    // Anything still pending at shutdown is flushed: a change we noticed and then
    // dropped on the floor is an index that disagrees with the disk.
    for (path, _) in pending.drain() {
        queue.push(path);
    }
}

/// Enqueue every path whose window has closed.
fn flush(
    pending: &mut HashMap<VaultPath, Instant>,
    queue: &WorkQueue,
    events: &Mutex<Vec<VaultEvent>>,
) {
    let now = Instant::now();
    let ready: Vec<VaultPath> = pending
        .iter()
        .filter(|(_, deadline)| **deadline <= now)
        .map(|(path, _)| path.clone())
        .collect();

    for path in ready {
        pending.remove(&path);
        queue.push(path);
    }

    if queue.take_overflowed() {
        raise(events, VaultEvent::RescanRequired);
    }
}

fn raise(events: &Mutex<Vec<VaultEvent>>, event: VaultEvent) {
    let mut events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !events.contains(&event) {
        events.push(event);
    }
}

/// Whether an event kind can mean the bytes at a path changed.
///
/// Access events are excluded: opening or reading a file is not a change, and on
/// platforms that report them they would otherwise make every search a reindex.
fn is_interesting(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

/// Turn a platform path into a vault path, or drop it.
///
/// Everything the walk skips, the watcher skips, using the same rules — a file the
/// walk ignores and the watcher accepts would appear on edit and vanish on rebuild.
fn accept(vault: &Vault, path: &Path) -> Option<VaultPath> {
    let relative = vault.relativise(path).ok()?;

    if vault.ignore_rules().is_ignored_file(relative.file_name()) {
        return None;
    }
    // Any ignored directory anywhere in the path: .serat/index.db must not enqueue
    // itself, which would be an index that reindexes because it indexed.
    if relative
        .as_str()
        .split('/')
        .any(|segment| vault.ignore_rules().is_ignored_dir(segment))
    {
        return None;
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Wait until `condition` holds, up to five seconds.
    ///
    /// Polling rather than sleeping a fixed amount: filesystem notification latency
    /// varies by an order of magnitude between platforms and between a warm and a
    /// loaded machine, and a fixed sleep is how a watcher test becomes the flaky one
    /// everybody ignores.
    fn eventually(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        condition()
    }

    fn fixture() -> (tempfile::TempDir, Vault, Arc<WorkQueue>) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open");
        (dir, vault, Arc::new(WorkQueue::new()))
    }

    #[test]
    fn a_new_note_reaches_the_queue() {
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::write(dir.path().join("note.md"), b"# New\n").expect("write");

        assert!(
            eventually(|| queue.depth() > 0),
            "the watcher never reported the new note"
        );
        assert_eq!(
            queue.drain(),
            vec![VaultPath::new("note.md").expect("path")]
        );
        watcher.stop();
    }

    #[test]
    fn one_save_becomes_one_queue_entry_however_many_events_it_emitted() {
        // The reason the window exists. A temp-and-rename save emits several events;
        // the queue must show the note once.
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        let target = dir.path().join("note.md");
        for round in 0..5 {
            fs::write(&target, format!("# Round {round}\n")).expect("write");
        }

        assert!(eventually(|| queue.depth() > 0), "nothing was queued");
        // Let the window close on anything still pending.
        std::thread::sleep(COALESCE_WINDOW * 2);

        let queued = queue.drain();
        assert_eq!(
            queued,
            vec![VaultPath::new("note.md").expect("path")],
            "five writes produced {} entries",
            queued.len()
        );
        watcher.stop();
    }

    #[test]
    fn a_deleted_file_is_queued_as_a_path_like_any_other() {
        let (dir, vault, queue) = fixture();
        let target = dir.path().join("doomed.md");
        fs::write(&target, b"# Doomed\n").expect("write");

        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");
        // Drain the creation if the watcher was fast enough to see it.
        std::thread::sleep(COALESCE_WINDOW * 2);
        queue.drain();

        fs::remove_file(&target).expect("remove");

        assert!(
            eventually(|| queue.depth() > 0),
            "the deletion never reached the queue"
        );
        assert_eq!(
            queue.drain(),
            vec![VaultPath::new("doomed.md").expect("path")]
        );
        watcher.stop();
    }

    #[test]
    fn our_own_index_directory_is_never_watched_into() {
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::create_dir_all(dir.path().join(".serat")).expect("mkdir");
        fs::write(dir.path().join(".serat/index.db"), b"pretend database").expect("write");
        fs::write(dir.path().join(".obsidian-workspace.json"), b"{}").expect("write");

        // Then something that must get through, so this is not merely asserting that
        // the watcher is asleep.
        fs::write(dir.path().join("real.md"), b"# Real\n").expect("write");

        assert!(eventually(|| queue.depth() > 0), "nothing was queued");
        std::thread::sleep(COALESCE_WINDOW * 2);

        assert_eq!(
            queue.drain(),
            vec![VaultPath::new("real.md").expect("path")],
            "something from an ignored path reached the queue"
        );
        watcher.stop();
    }

    #[test]
    fn dropping_the_watcher_stops_the_thread() {
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");
        drop(watcher);

        fs::write(dir.path().join("after.md"), b"# After\n").expect("write");
        std::thread::sleep(COALESCE_WINDOW * 3);
        assert_eq!(queue.depth(), 0, "a dropped watcher was still enqueueing");
    }
}
