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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEvent {
    /// The watcher lost events and the vault must be walked and reconciled.
    ///
    /// Emitted when the platform says it dropped notifications, or when the work
    /// queue overflowed. Both mean the same thing to the consumer, so they are one
    /// signal: the index may now disagree with the disk, and only a walk can settle
    /// it. Also raised when the ignore file ([`crate::IGNORE_FILE`]) changed and the
    /// vault's contents changed with it; the rules are already reloaded by then.
    RescanRequired,
    /// A directory appeared, moved or was renamed, so an unknown number of files under
    /// it changed and the platform will not list them. Only that subtree needs
    /// reconciling: walk it with [`Vault::walk_dir`] and the tree's own directory
    /// listing, not the whole vault. Ancestors subsume descendants: one is reported
    /// per subtree, the topmost.
    ///
    /// A directory that has since gone arrives as a changed *path* instead (the
    /// consumer's read fails, which is how a delete is recognised), and the consumer
    /// removes what the index holds beneath it.
    DirChanged(VaultPath),
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
        collapse(std::mem::take(&mut *events))
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
    let mut pending_dirs: HashMap<VaultPath, Instant> = HashMap::new();
    // When the ignore file changed, and so when to reload it.
    let mut ignore_file_due: Option<Instant> = None;

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
                        match accept(vault, path, &event.kind) {
                            Accepted::File(path) => {
                                pending.entry(path).or_insert(deadline);
                            }
                            // A directory event describes a subtree, and no platform
                            // emits per-child events for a folder rename. Queueing the
                            // directory itself would hand the consumer a path it cannot
                            // read, while the notes that actually moved reach nothing.
                            // The honest answer is to say which directory, so the
                            // consumer reconciles that subtree and not the vault.
                            Accepted::Subtree(dir) => {
                                pending_dirs.entry(dir).or_insert(deadline);
                            }
                            Accepted::IgnoreFile => {
                                ignore_file_due.get_or_insert(deadline);
                            }
                            // Task #34's amendment: reported, not dropped
                            // (FR-VLT-01, design goal 5). No further action —
                            // there is still no row to create for a name this
                            // platform's own filesystem could not hold.
                            //
                            // Task #188: a name that has *left* (a delete, or the old
                            // side of a rename) is taken out of the log, not put in;
                            // otherwise the count stays one too high until the next walk.
                            Accepted::Refused(refusal) => {
                                if name_has_left(path) {
                                    vault.refusals().forget(&refusal.path);
                                } else {
                                    vault.refusals().record(refusal);
                                }
                            }
                            Accepted::Ignored => {}
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
        flush_dirs(&mut pending_dirs, events);
        if ignore_file_due.is_some_and(|due| due <= Instant::now()) {
            ignore_file_due = None;
            // The rules change what the vault contains, so a changed file is a
            // rescan; an edit that left the effective patterns alone is not.
            if vault.reload_ignore_file() {
                raise(events, VaultEvent::RescanRequired);
            }
        }
    }

    // Anything still pending at shutdown is flushed: a change we noticed and then
    // dropped on the floor is an index that disagrees with the disk.
    for (path, _) in pending.drain() {
        queue.push(path);
    }
    if ignore_file_due.is_some() && vault.reload_ignore_file() {
        raise(events, VaultEvent::RescanRequired);
    }
    for (dir, _) in pending_dirs.drain() {
        raise(events, VaultEvent::DirChanged(dir));
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

    // Peek, never clear: the queue owns the latch and only the consumer that finishes
    // a walk may clear it (WorkQueue::rescan_completed). raise() deduplicates, so a
    // latch that stays set does not flood the event list.
    if queue.rescan_needed().is_some() {
        raise(events, VaultEvent::RescanRequired);
    }
}

/// Report every directory whose window has closed.
fn flush_dirs(pending: &mut HashMap<VaultPath, Instant>, events: &Mutex<Vec<VaultEvent>>) {
    let now = Instant::now();
    pending.retain(|dir, deadline| {
        if *deadline > now {
            return true;
        }
        raise(events, VaultEvent::DirChanged(dir.clone()));
        false
    });
}

/// What a consumer should see: a rescan of the vault subsumes every directory event,
/// and a directory event subsumes those for directories beneath it. A rename of `a/`
/// with a hundred subdirectories is one event, not a hundred and one.
fn collapse(events: Vec<VaultEvent>) -> Vec<VaultEvent> {
    if events.contains(&VaultEvent::RescanRequired) {
        return vec![VaultEvent::RescanRequired];
    }
    let dirs: Vec<&VaultPath> = events
        .iter()
        .filter_map(|event| match event {
            VaultEvent::DirChanged(dir) => Some(dir),
            VaultEvent::RescanRequired => None,
        })
        .collect();
    let covered = |dir: &VaultPath| {
        dirs.iter().any(|ancestor| {
            *ancestor != dir
                && dir
                    .as_str()
                    .strip_prefix(ancestor.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    };
    events
        .iter()
        .filter(|event| match event {
            VaultEvent::DirChanged(dir) => !covered(dir),
            VaultEvent::RescanRequired => true,
        })
        .cloned()
        .collect()
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

/// Whether a refused name is gone from disk. Existence decides, not the event kind
/// (architecture.md §3.1.2): inotify splits rename cookies, Windows sends old and new
/// as separate events and FSEvents gives no direction. Only `NotFound` means gone; any
/// other stat error records the name (a report, never a drop). On Windows a file
/// deleted while another process holds it open answers access-denied, so it stays
/// recorded until the next walk covering it.
///
/// On Windows the stat goes through the verbatim (`\\?\`) form: Win32 would
/// normalise exactly the names Serat refuses (`note.` and `note ` are looked up as
/// `note`; `AUX.md` can resolve to the device), and `Vault::open` strips the prefix
/// from the root.
fn name_has_left(path: &Path) -> bool {
    // The absolute event path is root + the relative components, which is exactly what
    // a verbatim spelling needs. `None` (a `.`/`..`/empty segment, or a name that is
    // not Unicode) cannot be judged, so the name is treated as present.
    #[cfg(windows)]
    let Some(path) = serat_core::verbatim::to_verbatim(path) else {
        return false;
    };
    std::fs::symlink_metadata(path).is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
}

/// What an event path turned out to be.
#[derive(Debug)]
enum Accepted {
    /// A file to index.
    File(VaultPath),
    /// A directory that appeared, moved or was renamed, so an unknown number of files
    /// under it changed.
    Subtree(VaultPath),
    /// The ignore file itself.
    IgnoreFile,
    /// A name this platform cannot represent as a `VaultPath` at all — reported
    /// rather than silently ignored (task #34's amendment).
    Refused(crate::refusal::Refusal),
    /// Nothing we index.
    Ignored,
}

/// Classify a platform path.
///
/// Everything the walk skips, the watcher skips, using the same rules — a file the
/// walk ignores and the watcher accepts would appear on edit and vanish on rebuild.
fn accept(vault: &Vault, path: &Path, kind: &EventKind) -> Accepted {
    let relative = match vault.relativise(path) {
        Ok(relative) => relative,
        // A name refusal (task #34's amendment) is reported; anything else —
        // most commonly a path outside the vault root, which a recursive watch
        // on the root should not produce but a rename crossing the boundary
        // still could — is the ordinary "not ours" case and stays silent.
        Err(err) => {
            return match crate::refusal::from_invalid_path(&err) {
                Some(refusal) => Accepted::Refused(refusal),
                None => Accepted::Ignored,
            };
        }
    };

    // The ignore file is a dotfile, so the rules below would drop it; it is the one
    // dotfile the watcher must hear about (FR-VLT-07).
    if relative.as_str() == crate::IGNORE_FILE {
        return Accepted::IgnoreFile;
    }

    // Any ignored directory anywhere in the path: .serat/index.db must not enqueue
    // itself, which would be an index that reindexes because it indexed. Checked
    // before the directory test so an ignored folder does not trigger a rescan.
    if relative
        .as_str()
        .split('/')
        .any(|segment| vault.ignore_rules().is_ignored_dir(segment))
    {
        return Accepted::Ignored;
    }

    // A directory still on disk is a subtree event. One that has gone cannot be
    // distinguished from a deleted file by looking, so it is treated as a file: the
    // consumer's read fails, which is how a delete is recognised anyway, and the
    // rename's destination raises the rescan that covers the rest.
    //
    // resolve() now refuses a symlinked path (T-07) rather than returning one to
    // stat — ignored here for the same reason the walk never queues one: a symlink
    // is not a vault file, and this function's whole job is applying the walk's
    // rules to events instead of directory entries.
    let Ok(resolved) = vault.resolve(&relative) else {
        return Accepted::Ignored;
    };
    let is_dir = resolved.is_dir();

    // The ignore file's patterns, on the path. A directory above an ignored one is
    // checked too: nothing under `/archive` is a vault file whatever its own name.
    let user = vault.ignore_rules().user_patterns();
    if !user.is_empty() {
        let parent = relative.as_str().rsplit_once('/').map(|(parent, _)| parent);
        if parent.is_some_and(|parent| vault.ignore_rules().is_ignored_dir_chain(parent))
            || user.is_ignored(relative.as_str(), is_dir)
        {
            return Accepted::Ignored;
        }
    }

    if is_dir {
        // A directory whose own content or metadata changed says nothing new about
        // its children, which report for themselves. Only a directory that came,
        // went or moved is a subtree event; without this every save on a platform
        // that reports the parent (Windows, macOS) would rescan the parent.
        //
        // A whitelist, not a blacklist: on Windows notify reports a directory's
        // MODIFIED (raised on every child create, delete and rename) as
        // `Modify(Any)`, which a blacklist of Data and Metadata lets through.
        return match kind {
            EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Any
            | EventKind::Modify(notify::event::ModifyKind::Name(_)) => Accepted::Subtree(relative),
            _ => Accepted::Ignored,
        };
    }

    if vault.ignore_rules().is_ignored_file(relative.file_name()) {
        return Accepted::Ignored;
    }

    Accepted::File(relative)
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

    #[cfg(unix)]
    #[test]
    fn a_refused_name_is_recorded_not_queued() {
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::write(dir.path().join("a\\b.md"), b"# Odd\n").expect("write");

        assert!(
            eventually(|| !vault.refusals().is_empty()),
            "the watcher never recorded the refusal"
        );
        assert_eq!(
            vault.refusals().len(),
            1,
            "{:?}",
            vault.refusals().snapshot()
        );
        assert_eq!(queue.depth(), 0, "a refused name was queued");
        watcher.stop();
    }

    /// Task #182 found, and task #188 fixed (FR-VLT-01, NFR-REL-01, US-07): the watcher
    /// used to record a refused name on every event for it, a delete included, so the
    /// delete of a file the log never held *added* the name and the refused-name count
    /// stayed one too high until the next walk. A delete now leaves the log untouched
    /// (see `docs/05-testing/reports/flake-hunt-2026-11.md`).
    #[cfg(unix)]
    #[test]
    fn the_delete_of_a_refused_name_does_not_add_it_to_the_log() {
        use std::os::unix::ffi::OsStrExt as _;
        let (dir, vault, queue) = fixture();
        let odd = dir.path().join(std::ffi::OsStr::from_bytes(b"gone\xe9.md"));
        // Written before the watcher exists and never walked: the log has never held it.
        fs::write(&odd, b"x").expect("write");
        assert!(vault.refusals().is_empty());
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::remove_file(&odd).expect("rm");
        // Events are handled in the order they were made, so once the note written after
        // the delete is queued, the delete has been handled.
        fs::write(dir.path().join("after.md"), b"# After\n").expect("sentinel");
        assert!(eventually(|| queue.depth() > 0), "the sentinel never came");
        assert!(
            vault.refusals().is_empty(),
            "a delete put a name the vault no longer holds into the log: {:?}",
            vault.refusals().snapshot()
        );
        watcher.stop();
    }

    /// Task #188: rename events, injected so no filesystem timing is involved. Each path
    /// is judged by whether it exists, whatever the event kind says, in delivery order:
    /// a refused name moved away leaves the log, one moved to is recorded, refused to
    /// refused swaps one entry for the other.
    #[cfg(unix)]
    #[test]
    fn a_rename_removes_the_old_refused_name_and_records_the_new_one() {
        use notify::event::{ModifyKind, RenameMode};
        let (dir, vault, queue) = fixture();
        let root = dir.path();
        let (tx, rx) = mpsc::channel();
        let stop = Mutex::new(false);
        let events = Mutex::new(Vec::new());
        let send = |kind, paths: &[&str]| {
            let mut event = notify::Event::new(kind);
            for path in paths {
                event = event.add_path(root.join(path));
            }
            tx.send(Ok(event)).expect("send");
        };
        let rename = |mode, paths: &[&str]| send(EventKind::Modify(ModifyKind::Name(mode)), paths);
        let paths = || -> Vec<String> {
            vault
                .refusals()
                .snapshot()
                .into_iter()
                .map(|r| r.path)
                .collect()
        };
        std::thread::scope(|scope| {
            let (vault_ref, queue_ref, events_ref, stop_ref) = (&vault, &*queue, &events, &stop);
            scope.spawn(move || {
                coalesce(
                    vault_ref,
                    &rx,
                    queue_ref,
                    events_ref,
                    stop_ref,
                    Duration::from_millis(1),
                );
            });
            // Events are handled in order, so once the sentinel is queued the earlier
            // ones have been.
            let settle = |name: &str| {
                fs::write(root.join(name), b"x").expect("sentinel");
                send(EventKind::Create(notify::event::CreateKind::File), &[name]);
                assert!(eventually(|| queue.depth() > 0), "the sentinel never came");
                queue.drain();
            };

            // Rename-to: the new name exists, so it is recorded.
            fs::write(root.join("a\\one.md"), b"x").expect("write");
            rename(RenameMode::To, &["a\\one.md"]);
            settle("s1.md");
            assert_eq!(paths(), vec!["a\\one.md"]);

            // Refused to refused: old removed, new recorded.
            fs::rename(root.join("a\\one.md"), root.join("a\\two.md")).expect("mv");
            rename(RenameMode::Both, &["a\\one.md", "a\\two.md"]);
            settle("s2.md");
            assert_eq!(paths(), vec!["a\\two.md"]);

            // Rename-away, the old side alone (a lone From).
            fs::rename(root.join("a\\two.md"), root.join("fine.md")).expect("mv");
            rename(RenameMode::From, &["a\\two.md"]);
            settle("s3.md");
            assert!(paths().is_empty(), "{:?}", vault.refusals().snapshot());

            // A direction-less event (FSEvents) is judged the same way.
            fs::write(root.join("a\\three.md"), b"x").expect("write");
            rename(RenameMode::Any, &["a\\three.md"]);
            settle("s4.md");
            assert_eq!(paths(), vec!["a\\three.md"]);
            fs::remove_file(root.join("a\\three.md")).expect("rm");
            rename(RenameMode::Any, &["a\\three.md"]);
            settle("s5.md");
            assert!(paths().is_empty(), "{:?}", vault.refusals().snapshot());

            *stop.lock().expect("stop") = true;
        });
    }

    /// Architecture.md §3.1.2, "A refused name leaves the log when its file does"
    /// (ruling, task #188, 2026-11-25). Hostile, Windows only: `note.` and `note ` are
    /// created through `\\?\` paths, touched so each raises an event, and must stay in the
    /// log (Win32 would look them up as `note` and call them gone); deleted through
    /// `\\?\`, each must leave. Not run on this platform's CI.
    #[cfg(windows)]
    #[test]
    fn a_refused_name_win32_would_alias_stays_until_its_file_is_deleted() {
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");
        let verbatim = |name: &str| {
            serat_core::verbatim::to_verbatim(&dir.path().join(name)).expect("verbatim")
        };
        for name in ["note.", "note "] {
            fs::write(verbatim(name), b"x").expect("create");
        }
        assert!(eventually(|| vault.refusals().len() == 2));
        for name in ["note.", "note "] {
            fs::write(verbatim(name), b"y").expect("touch");
        }
        // The sentinel is handled after both touches.
        fs::write(dir.path().join("after.md"), b"# After\n").expect("sentinel");
        assert!(eventually(|| queue.depth() > 0), "the sentinel never came");
        assert_eq!(
            vault.refusals().len(),
            2,
            "{:?}",
            vault.refusals().snapshot()
        );

        for name in ["note.", "note "] {
            fs::remove_file(verbatim(name)).expect("delete");
        }
        assert!(
            eventually(|| vault.refusals().is_empty()),
            "{:?}",
            vault.refusals().snapshot()
        );
        watcher.stop();
    }

    #[cfg(unix)]
    #[test]
    fn a_name_only_windows_refuses_is_recorded_with_a_reason_under_the_windows_rules() {
        // The watcher's conversion (`Vault::relativise`) judges by the vault's
        // platform, not the host's: mutant `Platform::current()` there queues it.
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path())
            .expect("open")
            .with_platform(serat_core::Platform::Windows);
        let queue = Arc::new(WorkQueue::new());
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::write(dir.path().join("aux.md"), b"# Aux\n").expect("write");

        assert!(
            eventually(|| !vault.refusals().is_empty()),
            "the watcher never recorded the refusal"
        );
        let found = vault.refusals().snapshot();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "aux.md");
        assert!(
            found[0].reason.contains("reserved Windows device"),
            "{found:?}"
        );
        assert_eq!(queue.depth(), 0, "a refused name was queued");
        watcher.stop();
    }

    #[cfg(not(windows))]
    #[test]
    fn a_name_only_windows_would_refuse_reaches_the_queue_on_this_platform() {
        // Task #34's regression, at the watcher rather than only the walk: a
        // colon in a filename is ordinary here, and the watcher must queue it
        // rather than silently drop the event (FR-VLT-01, design goal 5).
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::write(dir.path().join("Meeting 10:30.md"), b"# Meeting\n").expect("write");

        assert!(
            eventually(|| queue.depth() > 0),
            "the watcher never reported a name only illegal on Windows"
        );
        assert_eq!(
            queue.drain(),
            vec![VaultPath::new("Meeting 10:30.md").expect("path")]
        );
        assert!(
            vault.refusals().is_empty(),
            "an ordinary, legal name was reported as refused: {:?}",
            vault.refusals().snapshot()
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
    fn a_folder_rename_never_puts_a_directory_on_the_queue() {
        // BLOCKING 2 from the I2 review. No platform emits per-child events for a
        // folder rename, so before this the queue got two directory paths the consumer
        // cannot read, while the four note paths that actually changed reached
        // nothing — the index keeping rows for files that no longer exist, with no
        // signal. That is the permanent divergence C-02 forbids.
        let (dir, vault, queue) = fixture();
        fs::create_dir_all(dir.path().join("10-projects")).expect("mkdir");
        fs::write(
            dir.path().join("10-projects/alpha.md"),
            b"# Alpha
",
        )
        .expect("write");
        fs::write(
            dir.path().join("10-projects/beta.md"),
            b"# Beta
",
        )
        .expect("write");

        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");
        std::thread::sleep(COALESCE_WINDOW * 2);
        queue.drain();
        let _ = watcher.take_events();

        fs::rename(
            dir.path().join("10-projects"),
            dir.path().join("20-archive"),
        )
        .expect("rename the folder");

        // Which directory, not the whole vault: task #14's path-aware event.
        let mut seen = Vec::new();
        let named = eventually(|| {
            seen.extend(watcher.take_events());
            seen.contains(&VaultEvent::DirChanged(
                VaultPath::new("20-archive").expect("path"),
            ))
        });
        assert!(
            named,
            "a folder rename did not name the new directory: {seen:?}"
        );
        assert!(
            !seen.contains(&VaultEvent::RescanRequired),
            "a single directory change asked for a rescan of the vault: {seen:?}"
        );

        std::thread::sleep(COALESCE_WINDOW * 2);
        for queued in queue.drain() {
            assert!(
                !vault.resolve(&queued).expect("resolve").is_dir(),
                "{queued} is a directory and was queued as if it were a file"
            );
        }
        watcher.stop();
    }

    #[test]
    fn saving_a_file_in_a_folder_does_not_report_the_folder() {
        // Task #14. On platforms that report the parent of a written file, treating
        // that as a directory change would rescan the folder on every save.
        let (dir, vault, queue) = fixture();
        fs::create_dir_all(dir.path().join("notes")).expect("mkdir");
        fs::write(dir.path().join("notes/a.md"), b"# A\n").expect("write");
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");
        std::thread::sleep(COALESCE_WINDOW * 2);
        queue.drain();
        let _ = watcher.take_events();

        fs::write(dir.path().join("notes/a.md"), b"# A, edited\n").expect("write");
        assert!(eventually(|| queue.depth() > 0), "the edit was not seen");
        std::thread::sleep(COALESCE_WINDOW * 2);
        assert_eq!(watcher.take_events(), Vec::<VaultEvent>::new());
        watcher.stop();
    }

    #[test]
    fn an_ignore_file_hides_a_folder_from_the_watcher_and_its_change_asks_for_a_rescan() {
        // FR-VLT-07: the walk and the watcher apply one rule.
        let (dir, vault, queue) = fixture();
        fs::create_dir_all(dir.path().join("archive")).expect("mkdir");
        fs::write(dir.path().join(crate::IGNORE_FILE), b"/archive\n").expect("write");
        assert!(vault.reload_ignore_file(), "the file was not picked up");
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        fs::write(dir.path().join("archive/old.md"), b"# Old\n").expect("write");
        fs::write(dir.path().join("kept.md"), b"# Kept\n").expect("write");
        assert!(eventually(|| queue.depth() > 0));
        std::thread::sleep(COALESCE_WINDOW * 2);
        assert_eq!(
            queue.drain(),
            vec![VaultPath::new("kept.md").expect("path")]
        );
        let _ = watcher.take_events();

        // Un-ignoring it: the rules reload, and the vault must be walked again.
        fs::write(dir.path().join(crate::IGNORE_FILE), b"# nothing now\n").expect("write");
        assert!(
            eventually(|| watcher.take_events().contains(&VaultEvent::RescanRequired)),
            "changing the ignore file did not ask for a rescan"
        );
        assert!(!vault.ignore_rules().is_ignored_at("archive", true));
        watcher.stop();
    }

    #[test]
    fn a_directory_event_covers_the_directories_beneath_it_and_a_rescan_covers_all() {
        let dir = |text: &str| VaultEvent::DirChanged(VaultPath::new(text).expect("path"));
        assert_eq!(
            collapse(vec![dir("a/b"), dir("a"), dir("ab"), dir("a/b/c")]),
            vec![dir("a"), dir("ab")],
            "a/b and a/b/c are under a; ab is a sibling and not"
        );
        assert_eq!(
            collapse(vec![dir("a"), VaultEvent::RescanRequired]),
            vec![VaultEvent::RescanRequired]
        );
    }

    #[test]
    fn the_coalescing_window_actually_delays_the_enqueue() {
        // The old version of this test passed identically with the window set to zero,
        // because it was measuring WorkQueue's path dedup rather than the coalescer.
        // This one cannot: it asserts the path is absent while the window is open and
        // present once it has closed.
        let (dir, vault, queue) = fixture();
        let watcher = Watcher::start(
            &vault,
            Arc::clone(&queue),
            WatcherConfig {
                coalesce_window: Duration::from_millis(1_500),
            },
        )
        .expect("start");

        fs::write(
            dir.path().join("slow.md"),
            b"# Slow
",
        )
        .expect("write");

        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            queue.depth(),
            0,
            "the path was enqueued before its window closed"
        );

        assert!(
            eventually(|| queue.depth() > 0),
            "the path never arrived after its window closed"
        );
        watcher.stop();
    }

    #[test]
    fn repeated_writes_inside_one_window_reach_a_draining_consumer_once() {
        // The other half, with a consumer popping continuously so the queue's dedup
        // cannot be what collapses the writes. With no window this reaches five.
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let consumer = {
            let queue = Arc::clone(&queue);
            let stop = Arc::clone(&stop);
            let pops = Arc::clone(&pops);
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if queue.pop_timeout(Duration::from_millis(10)).is_some() {
                        pops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            })
        };

        let target = dir.path().join("busy.md");
        for round in 0..5 {
            fs::write(
                &target,
                format!(
                    "# Round {round}
"
                ),
            )
            .expect("write");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(COALESCE_WINDOW * 4);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        consumer.join().expect("consumer");

        let observed = pops.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            (1..=2).contains(&observed),
            "five writes 20 ms apart produced {observed} units of work; the window is              not coalescing"
        );
        watcher.stop();
    }

    #[test]
    fn a_file_written_continuously_is_still_indexed_rather_than_starved() {
        // The reviewer found that changing or_insert to insert — extending the deadline
        // on every event instead of leaving it fixed — passed all 129 tests, while the
        // property is documented three lines above the call. That is a real starvation
        // mode: a process writing faster than every 150 ms, a build tool appending to a
        // log in the vault or a sync client streaming a large attachment, would reset
        // the deadline forever and the path would never be enqueued at all.
        //
        // The window is fixed from the first event for exactly this reason, and until
        // now nothing said so in a way that could fail.
        let (dir, vault, queue) = fixture();
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let consumer = {
            let queue = Arc::clone(&queue);
            let stop = Arc::clone(&stop);
            let pops = Arc::clone(&pops);
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if queue.pop_timeout(Duration::from_millis(10)).is_some() {
                        pops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            })
        };

        // One second of writes, every 30 ms — five times faster than the window closes.
        let target = dir.path().join("busy.md");
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut round = 0u32;
        while Instant::now() < deadline {
            fs::write(
                &target,
                format!(
                    "# Round {round}
"
                ),
            )
            .expect("write");
            round += 1;
            std::thread::sleep(Duration::from_millis(30));
        }
        std::thread::sleep(COALESCE_WINDOW * 3);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        consumer.join().expect("consumer");

        let observed = pops.load(std::sync::atomic::Ordering::Relaxed);
        watcher.stop();
        assert!(
            observed >= 2,
            "a file written every 30 ms for a second reached the consumer {observed}              times; with the deadline extended on each event it would be starved              entirely and a vault could hold a file that is never indexed"
        );
    }

    #[test]
    fn a_queue_overflow_raises_rescan_required() {
        // VaultEvent::RescanRequired had no coverage at all, which is partly why the
        // folder-rename defect went unnoticed: nobody had ever watched it fire.
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open");
        let queue = Arc::new(WorkQueue::with_capacity(1));
        let watcher =
            Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default()).expect("start");

        for i in 0..8 {
            fs::write(
                dir.path().join(format!("note-{i}.md")),
                b"# Note
",
            )
            .expect("write");
        }

        assert!(
            eventually(
                || watcher.take_events().contains(&VaultEvent::RescanRequired)
                    || queue.rescan_needed().is_some()
            ),
            "the queue overflowed and nothing raised RescanRequired"
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
