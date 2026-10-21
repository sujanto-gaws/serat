//! The file tree's stream, wired: `open_tree` and `mark_tree` over serat-vault's
//! [`TreeSession`] (ADR-0015; FR-VLT-01, FR-VLT-04, FR-VLT-08, FR-BIN-03).
//!
//! This module holds no tree logic. The model, the ordering obligations and the walk
//! are `serat-core`'s and `serat-vault`'s; the index rows are `serat-index`'s. What is
//! here is the plumbing the application owns:
//!
//! * [`CoalescingSink`], the session's sink: converts [`TreeOut`] to the contract's
//!   [`TreeMessage`] and batches the initial population into bursts of at most 256 KiB
//!   or 16 ms (ADR-0015 §2) before they reach the channel.
//! * [`TreeFeed`], one per open vault: starts the stream, owns the threads that
//!   populate it, maps watcher output to `created` / `deleted` / `rescan`, and
//!   **restarts the session when it is poisoned**.
//!
//! # The rules that hold here (architecture §2.2, the `TreeSession` lock)
//!
//! * **The sink takes only leaf locks.** It runs under the session's lock. It takes the
//!   batch mutex and calls the sender; the flusher thread takes the same batch mutex
//!   and nothing else. Neither takes the session lock, the feed's state lock, or any
//!   lock on [`crate::state::AppState`], so no cycle can form through the sink.
//! * **Progress is read lock-free** ([`TreeSession::progress`]); `mark_tree` never
//!   waits behind a stream.
//! * **A poisoned session is dead and is replaced**, never recovered. Every entry point
//!   that touches the session ([`TreeFeed::path_changed`], [`TreeFeed::rescan_all`],
//!   [`TreeFeed::progress`], and the populate thread on its way out) checks
//!   [`TreeSession::is_poisoned`] and calls the restart, which builds a new session
//!   with a new sink on the same channel and re-sends from the root. The restart is
//!   bounded ([`MAX_RESTARTS`]) so a sender that always panics cannot loop.
//! * **The walk runs on a thread the application can lose.** A panic there, or in a
//!   watcher-driven call, is caught at the thread's edge; the vault thread (which owns
//!   the watcher) and the webview never see it.
//!
//! # What "coalescing" is here
//!
//! The contract's [`TreeMessage`] is one listing, upsert or remove, and the front end
//! applies exactly one per call (no batch variant exists). So coalescing is **in
//! time**, not in shape: messages produced during the initial population are held and
//! written to the channel in one burst per 256 KiB or 16 ms, whichever comes first,
//! in order, and the root listing, `Complete` and everything after it are written at
//! once. A burst is a run of `send` calls under one lock; a message is never merged
//! with, dropped for, or reordered against another.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use serat_core::{SourceEntry, TreeEntry, TreeKind, TreeOp, VaultPath, source_entries};
use serat_vault::{TreeOut, TreeProgress, TreeSession, TreeSource, Vault, is_note_file};

use crate::ipc::{self, TreeEntryKind, TreeMessage};

/// ADR-0015 §2: at most this many bytes of walking per burst.
pub const BATCH_BYTES: usize = 256 * 1024;
/// ADR-0015 §2: at most this long per burst.
pub const BATCH_WINDOW: Duration = Duration::from_millis(16);
/// How many times a poisoned session is replaced for one `open_tree`.
pub const MAX_RESTARTS: u32 = 3;

/// Delivers one message to the front end. `false` means the receiver is gone (the
/// channel was closed); the stream then stops sending. In the application it wraps a
/// `tauri::ipc::Channel<TreeMessage>`; in tests, a `Vec`.
///
/// **The sender must never call back into the feed or the session**: it runs under the
/// session's lock, and re-entry would deadlock.
pub type TreeSender = Arc<dyn Fn(TreeMessage) -> bool + Send + Sync>;

/// The contract's source in the vault's terms.
fn map_source(source: TreeSource) -> ipc::TreeSource {
    match source {
        TreeSource::Index => ipc::TreeSource::Index,
        TreeSource::Walk => ipc::TreeSource::Walk,
    }
}

fn map_kind(kind: TreeKind) -> TreeEntryKind {
    match kind {
        TreeKind::Note => TreeEntryKind::Note,
        TreeKind::Asset => TreeEntryKind::Asset,
        TreeKind::Dir => TreeEntryKind::Dir,
    }
}

fn map_entry(entry: TreeEntry) -> ipc::TreeEntry {
    ipc::TreeEntry {
        name: entry.name,
        kind: map_kind(entry.kind),
        sidecar: entry.sidecar,
    }
}

/// One session message as the contract's, and a cheap estimate of its serialised size.
#[must_use]
pub fn to_message(out: TreeOut) -> (TreeMessage, usize) {
    match out {
        TreeOut::Op(TreeOp::Listing { dir, entries }) => {
            let size = 32 + dir.len() + entries.iter().map(|e| 40 + e.name.len()).sum::<usize>();
            (
                TreeMessage::Listing {
                    dir,
                    entries: entries.into_iter().map(map_entry).collect(),
                },
                size,
            )
        }
        TreeOut::Op(TreeOp::Upsert { dir, entry, after }) => {
            let size = 96 + dir.len() + entry.name.len() + after.as_ref().map_or(0, String::len);
            (
                TreeMessage::Upsert {
                    dir,
                    entry: map_entry(entry),
                    after,
                },
                size,
            )
        }
        TreeOut::Op(TreeOp::Remove { dir, name }) => {
            let size = 64 + dir.len() + name.len();
            (TreeMessage::Remove { dir, name }, size)
        }
        TreeOut::Complete { entries, source } => (
            TreeMessage::Complete {
                entries,
                source: map_source(source),
            },
            48,
        ),
        TreeOut::WalkSettled { empty_folders } => (TreeMessage::WalkSettled { empty_folders }, 48),
    }
}

#[derive(Default)]
struct Batch {
    messages: Vec<TreeMessage>,
    bytes: usize,
    /// When the oldest held message arrived.
    since: Option<Instant>,
    /// After `Complete`: nothing is held back any more.
    live: bool,
    /// The receiver is gone, or the stream was cancelled: drop everything.
    closed: bool,
}

struct Shared {
    batch: Mutex<Batch>,
    wake: Condvar,
    send: TreeSender,
    cancelled: Arc<AtomicBool>,
    /// The sender panicked on a thread other than the session's (the flusher). The
    /// model and the stream no longer agree; the next `push` kills the session so
    /// the feed replaces it, which is what a sink failure means.
    failed: AtomicBool,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Batch> {
        // A poisoned batch can only mean the sender panicked mid-burst. The session's
        // own poison rule applies to that panic (the sink is inside the session), so
        // recovering here costs nothing the session has not already lost.
        self.batch.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write the held messages, in order. Under the batch lock.
    fn flush(&self, batch: &mut Batch) {
        batch.bytes = 0;
        batch.since = None;
        for message in std::mem::take(&mut batch.messages) {
            if batch.closed {
                break;
            }
            if self.cancelled.load(Ordering::Relaxed) {
                batch.closed = true;
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| (self.send)(message))) {
                Ok(true) => {}
                Ok(false) => batch.closed = true,
                Err(panic) => {
                    // On the session's thread this is a sink failure, and unwinding
                    // poisons the session, which is what must happen. On the flusher
                    // it is recorded and the next push raises it there.
                    batch.closed = true;
                    self.failed.store(true, Ordering::Relaxed);
                    if std::thread::current().name() != Some("serat-tree-flush") {
                        std::panic::resume_unwind(panic);
                    }
                }
            }
        }
    }
}

/// The session's sink: converts, holds and writes. See the module doc for the
/// locking rules; it is the only thing the session calls.
pub struct CoalescingSink {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for CoalescingSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoalescingSink").finish_non_exhaustive()
    }
}

impl CoalescingSink {
    /// A sink writing to `send`. `cancelled` stops it from writing (the stream was
    /// replaced by a newer `open_tree`).
    #[must_use]
    pub fn new(send: TreeSender, cancelled: Arc<AtomicBool>) -> Self {
        let shared = Arc::new(Shared {
            batch: Mutex::new(Batch::default()),
            wake: Condvar::new(),
            send,
            cancelled,
            failed: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        // If the thread cannot be created the sink still works: it flushes on the
        // size bound and on the next arrival, and `Complete` flushes everything.
        let _ = std::thread::Builder::new()
            .name("serat-tree-flush".to_owned())
            .spawn(move || flusher(&weak));
        Self { shared }
    }

    /// Accept one message from the session.
    pub fn push(&self, out: TreeOut) {
        let immediate = matches!(
            &out,
            TreeOut::Op(TreeOp::Listing { dir, .. }) if dir.is_empty()
        ) || matches!(&out, TreeOut::Complete { .. });
        let completes = matches!(&out, TreeOut::Complete { .. });
        let (message, size) = to_message(out);

        assert!(
            !self.shared.failed.load(Ordering::Relaxed),
            "the tree sender failed on the flusher thread; this session is dead"
        );
        let mut batch = self.shared.lock();
        if batch.closed || self.shared.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let was_empty = batch.messages.is_empty();
        batch.messages.push(message);
        batch.bytes += size;
        let now = Instant::now();
        let since = *batch.since.get_or_insert(now);
        if completes {
            batch.live = true;
        }
        if immediate
            || batch.live
            || batch.bytes >= BATCH_BYTES
            || now.duration_since(since) >= BATCH_WINDOW
        {
            self.shared.flush(&mut batch);
        } else if was_empty {
            self.shared.wake.notify_one();
        }
    }
}

/// Writes a held batch whose window has closed, so a walk that stalls on a slow
/// directory does not hold the messages it already produced.
fn flusher(weak: &Weak<Shared>) {
    loop {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        {
            let mut batch = shared.lock();
            let wait = match batch.since {
                Some(since) => BATCH_WINDOW.saturating_sub(since.elapsed()),
                None => Duration::from_millis(250),
            };
            if batch.closed {
                return;
            }
            if !wait.is_zero() {
                let (guard, _) = shared
                    .wake
                    .wait_timeout(batch, wait)
                    .unwrap_or_else(PoisonError::into_inner);
                batch = guard;
            }
            if let Some(since) = batch.since
                && since.elapsed() >= BATCH_WINDOW
            {
                shared.flush(&mut batch);
            }
        }
        // Drop the strong reference between turns so a discarded sink can free itself.
        drop(shared);
    }
}

/// What the front end's two marks were, as `mark_tree` recorded them (ADR-0015 §3).
/// Milliseconds on the process's monotonic clock (`t0`), like every other mark.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TreeMarks {
    /// `tree_first_screen`, set once, only after the root listing was sent.
    pub first_screen_ms: Option<f64>,
    /// `tree_complete`, set once, only after `Complete` was sent.
    pub complete_ms: Option<f64>,
    /// File entries the front end held at `tree_complete`.
    pub entries_in_model: Option<u32>,
    /// File entries and source `Complete` carried.
    pub delivered: Option<(u32, ipc::TreeSource)>,
    /// Marks refused because the stream had not sent what they claim to have painted.
    pub rejected: u32,
    /// Empty folders the authoritative walk found (`WalkSummary`), and when it settled.
    pub empty_folders: Option<u32>,
}

/// One live stream: its session, the sender it writes to, and its cancel flag.
struct Active {
    session: Arc<TreeSession>,
    send: TreeSender,
    cancelled: Arc<AtomicBool>,
    restarts: u32,
}

#[derive(Default)]
struct FeedState {
    active: Option<Active>,
}

/// What a [`TreeFeed`] needs from the application, injected so the feed is testable
/// without a window.
pub struct FeedConfig {
    /// The open vault.
    pub vault: Arc<Vault>,
    /// The index database file, for the warm path.
    pub index_db: PathBuf,
    /// Whether the index is `Ready` now. Read once per stream start.
    pub index_ready: Box<dyn Fn() -> bool + Send + Sync>,
    /// Blocks until the walk may start: the first paint, in the application
    /// (ADR-0015 §2: "the walk always runs, after the first paint").
    pub gate: Arc<dyn Fn() + Send + Sync>,
    /// Called on the warm path just before the authoritative walk starts, so the
    /// application can journal what the watcher reports until the catch-up is done.
    pub on_walk_start: Box<dyn Fn() + Send + Sync>,
    /// Called on the warm path (index `Ready` at the start of the stream) with every
    /// file the authoritative walk found, sorted by path, once the walk has completed
    /// (ADR-0015 section 2, *One walk at open*). The application hands it to the open's
    /// catch-up on a thread of its own. Never called on the no-index path, where the
    /// rebuild is the reconciliation, nor when the walk failed.
    pub on_walked: Box<dyn Fn(Vec<SourceEntry>) + Send + Sync>,
    /// Fed every message of the stream, for the switcher's corpus before the index is
    /// `Ready` (architecture 9.2).
    pub paths: Arc<crate::switcher::TreePaths>,
    /// Called with the number of files the authoritative walk admitted, on either path,
    /// when it completes (the cold-start trace's walk mark; the refusal count is final).
    pub on_walk_complete: Box<dyn Fn(u32) + Send + Sync>,
    /// Called once when the stream has exhausted [`MAX_RESTARTS`] and will not update
    /// again (the application emits `tree://status`). Must not call back into the feed.
    pub on_stopped: Box<dyn Fn() + Send + Sync>,
}

/// One per open vault.
pub struct TreeFeed {
    config: FeedConfig,
    state: Mutex<FeedState>,
    marks: Mutex<TreeMarks>,
    /// Generations of populate threads, so a stale one's `WalkSummary` is not reported.
    populating: AtomicU32,
    /// `on_stopped` has fired for this `open`.
    stopped: AtomicBool,
}

impl std::fmt::Debug for FeedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedConfig")
            .field("index_db", &self.index_db)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for TreeFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeFeed").finish_non_exhaustive()
    }
}

impl Drop for TreeFeed {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(active) = state.active.take() {
            active.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

impl TreeFeed {
    /// A feed with no stream open.
    #[must_use]
    pub fn new(config: FeedConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            state: Mutex::new(FeedState::default()),
            marks: Mutex::new(TreeMarks::default()),
            populating: AtomicU32::new(0),
            stopped: AtomicBool::new(false),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FeedState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn marks_lock(&self) -> std::sync::MutexGuard<'_, TreeMarks> {
        self.marks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `open_tree`: replace any earlier stream and start this one from the root.
    /// Returns at once; population runs on its own thread.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create the populate thread.
    pub fn open(self: &Arc<Self>, send: TreeSender) -> std::io::Result<()> {
        *self.marks_lock() = TreeMarks::default();
        self.stopped.store(false, Ordering::SeqCst);
        self.start(send, 0)
    }

    fn start(self: &Arc<Self>, send: TreeSender, restarts: u32) -> std::io::Result<()> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let sink = CoalescingSink::new(Arc::clone(&send), Arc::clone(&cancelled));
        let paths = Arc::clone(&self.config.paths);
        let session = Arc::new(TreeSession::new(move |out| {
            paths.apply(&out);
            sink.push(out);
        }));
        {
            let mut state = self.state();
            if let Some(old) = state.active.take() {
                old.cancelled.store(true, Ordering::Relaxed);
            }
            state.active = Some(Active {
                session: Arc::clone(&session),
                send,
                cancelled,
                restarts,
            });
        }
        let feed = Arc::clone(self);
        let generation = self.populating.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::Builder::new()
            .name("serat-tree".to_owned())
            .spawn(move || {
                // A thread the application can lose: a panic here must reach nobody.
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    feed.populate(&session, generation);
                }));
                if outcome.is_err() || session.is_poisoned() {
                    feed.restart_if_poisoned();
                }
            })
            .map(|_| ())
    }

    /// The warm path if the index is `Ready`, then the authoritative walk.
    fn populate(&self, session: &TreeSession, generation: u32) {
        let warm = (self.config.index_ready)();
        if warm && let Some(snapshot) = session.begin_index_read() {
            match serat_index::tree::read_tree_rows(&self.config.index_db) {
                Ok(rows) => {
                    let _ = session.populate_from_index(
                        snapshot,
                        rows.iter().map(|(path, kind)| (path.as_str(), *kind)),
                    );
                }
                Err(_) => session.abandon_index_read(snapshot),
            }
        }
        (self.config.gate)();
        // One walk at open (ADR-0015 section 2): on the warm path it carries size and
        // mtime and feeds the catch-up; on the no-index path it stays name-and-kind.
        let walked = if warm {
            (self.config.on_walk_start)();
            let mut acc: Vec<SourceEntry> = Vec::new();
            let result = session
                .populate_from_walk_with_metadata(&self.config.vault, &mut |dir, entries| {
                    acc.extend(source_entries(dir, entries))
                });
            if result.is_ok() {
                acc.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
                (self.config.on_walked)(acc);
            }
            result
        } else {
            session.populate_from_walk(&self.config.vault)
        };
        if let Ok(summary) = walked {
            (self.config.on_walk_complete)(summary.files);
        }
        if let Ok(summary) = walked
            && self.populating.load(Ordering::SeqCst) == generation
        {
            self.marks_lock().empty_folders = Some(summary.empty_folders);
        }
        // A restarted stream that got as far as `Complete` is healthy again: the
        // budget is for consecutive failures, not for a vault's whole lifetime.
        if session.progress().complete.is_some()
            && let Some(active) = self.state().active.as_mut()
            && std::ptr::eq(Arc::as_ptr(&active.session), session)
        {
            active.restarts = 0;
        }
    }

    /// Replace a poisoned session with a fresh one on the same channel. Bounded.
    fn restart_if_poisoned(self: &Arc<Self>) {
        let (send, restarts) = {
            let state = self.state();
            match state.active.as_ref() {
                Some(active) if active.session.is_poisoned() => {
                    (Arc::clone(&active.send), active.restarts)
                }
                _ => return,
            }
        };
        if restarts >= MAX_RESTARTS {
            // Goal 5: a tree that stops updating is a fact the user is told, once.
            if !self.stopped.swap(true, Ordering::SeqCst) {
                eprintln!("serat: the file tree stream failed {MAX_RESTARTS} restarts and stopped");
                (self.config.on_stopped)();
            }
            return;
        }
        let _ = self.start(send, restarts + 1);
    }

    /// The live session, restarting a dead one first. `None` with no stream open.
    fn session(self: &Arc<Self>) -> Option<Arc<TreeSession>> {
        let session = {
            let state = self.state();
            Arc::clone(&state.active.as_ref()?.session)
        };
        if session.is_poisoned() {
            self.restart_if_poisoned();
            return None;
        }
        Some(session)
    }

    /// What the stream has delivered, lock-free. `None` with no stream open.
    #[must_use]
    pub fn progress(&self) -> Option<TreeProgress> {
        let state = self.state();
        state.active.as_ref().map(|a| a.session.progress())
    }

    /// A watcher-reported path changed. The watcher gives a path, not a kind, so the
    /// disk decides: present means created (a modification sends nothing, because
    /// the model already holds the entry), absent means deleted. A created directory
    /// is rescanned unconditionally ([`serat_vault::RescanNeeded`] is `#[must_use]`).
    pub fn path_changed(self: &Arc<Self>, path: &VaultPath) {
        let Some(session) = self.session() else {
            return;
        };
        let vault = &self.config.vault;
        let present = vault
            .resolve(path)
            .ok()
            .and_then(|real| std::fs::symlink_metadata(real).ok())
            .filter(|meta| !meta.file_type().is_symlink());
        let outcome = catch_unwind(AssertUnwindSafe(|| match present {
            Some(meta) => {
                let kind = if meta.is_dir() {
                    TreeKind::Dir
                } else if is_note_file(path.file_name()) {
                    TreeKind::Note
                } else {
                    TreeKind::Asset
                };
                // The unconditional rescan after a created directory.
                let _ = session.created(path.as_str(), kind).run(&session, vault);
            }
            None => session.deleted(path.as_str()),
        }));
        if outcome.is_err() || session.is_poisoned() {
            self.restart_if_poisoned();
        }
    }

    /// A directory appeared, moved or was renamed (`VaultEvent::DirChanged`): re-list
    /// that subtree, sending only listings that differ.
    pub fn dir_changed(self: &Arc<Self>, dir: &VaultPath) {
        let Some(session) = self.session() else {
            return;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = session.rescan(&self.config.vault, dir.as_str());
        }));
        if outcome.is_err() || session.is_poisoned() {
            self.restart_if_poisoned();
        }
    }

    /// The watcher lost events (or a directory moved): re-list everything, sending
    /// only listings that differ (ADR-0011, ADR-0015 §1).
    pub fn rescan_all(self: &Arc<Self>) {
        let Some(session) = self.session() else {
            return;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = session.rescan(&self.config.vault, "");
        }));
        if outcome.is_err() || session.is_poisoned() {
            self.restart_if_poisoned();
        }
    }

    /// `mark_tree`: stamp a front-end mark, if the stream has sent what the front end
    /// claims to have painted (`TreeSession::progress`). The first valid mark of each
    /// kind wins. A mark the stream cannot have earned is counted and ignored.
    pub fn mark(&self, args: &ipc::MarkTreeArgs, now_ms: f64) {
        let progress = self.progress();
        let mut marks = self.marks_lock();
        match args.mark {
            ipc::TreeMark::FirstScreen => {
                if progress.is_some_and(|p| p.root_sent) {
                    marks.first_screen_ms.get_or_insert(now_ms);
                } else {
                    marks.rejected += 1;
                }
            }
            ipc::TreeMark::Complete => match progress.and_then(|p| p.complete) {
                Some((entries, source)) => {
                    if marks.complete_ms.is_none() {
                        marks.complete_ms = Some(now_ms);
                        marks.entries_in_model = Some(args.entries_in_model);
                        marks.delivered = Some((entries, map_source(source)));
                    }
                }
                None => marks.rejected += 1,
            },
        }
    }

    /// The marks recorded so far.
    #[must_use]
    pub fn marks(&self) -> TreeMarks {
        self.marks_lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    type Log = Arc<Mutex<Vec<TreeMessage>>>;

    fn collecting() -> (TreeSender, Log) {
        let log: Log = Arc::default();
        let sink = Arc::clone(&log);
        (
            Arc::new(move |m| {
                sink.lock().expect("log").push(m);
                true
            }),
            log,
        )
    }

    fn write(root: &Path, relative: &str) {
        let target = root.join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        fs::write(target, b"x").expect("write");
    }

    fn feed_for(dir: &Path, ready: bool) -> Arc<TreeFeed> {
        TreeFeed::new(FeedConfig {
            vault: Arc::new(Vault::open(dir).expect("open")),
            index_db: dir.join(".serat/none.db"),
            index_ready: Box::new(move || ready),
            gate: Arc::new(|| {}),
            paths: Arc::default(),
            on_walk_start: Box::new(|| {}),
            on_walked: Box::new(|_| {}),
            on_walk_complete: Box::new(|_| {}),
            on_stopped: Box::new(|| {}),
        })
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        for _ in 0..400 {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn has_complete(log: &Log) -> bool {
        log.lock()
            .expect("log")
            .iter()
            .any(|m| matches!(m, TreeMessage::Complete { .. }))
    }

    fn settled(log: &Log) -> bool {
        log.lock()
            .expect("log")
            .iter()
            .any(|m| matches!(m, TreeMessage::WalkSettled { .. }))
    }

    fn listing<'a>(log: &'a [TreeMessage], dir: &str) -> Option<&'a Vec<ipc::TreeEntry>> {
        log.iter().rev().find_map(|m| match m {
            TreeMessage::Listing { dir: d, entries } if d == dir => Some(entries),
            _ => None,
        })
    }

    #[test]
    fn the_no_index_path_sends_root_first_then_complete_from_the_walk() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        write(dir.path(), "sub/b.md");
        fs::create_dir_all(dir.path().join("empty")).expect("mkdir");
        let feed = feed_for(dir.path(), false);
        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("settled", || settled(&log));

        let log = log.lock().expect("log");
        assert!(
            matches!(&log[0], TreeMessage::Listing { dir, .. } if dir.is_empty()),
            "the root listing is first: {:?}",
            log[0]
        );
        let complete = log
            .iter()
            .position(|m| matches!(m, TreeMessage::Complete { .. }))
            .expect("complete");
        assert!(matches!(
            &log[complete],
            TreeMessage::Complete {
                entries: 2,
                source: ipc::TreeSource::Walk
            }
        ));
        assert!(listing(&log, "sub").is_some());
        assert!(listing(&log, "empty").is_some_and(Vec::is_empty));
    }

    type Walked = Arc<Mutex<Vec<Vec<SourceEntry>>>>;

    fn feed_with_walk_hooks(dir: &Path, ready: bool) -> (Arc<TreeFeed>, Walked, Arc<AtomicU32>) {
        let walked: Walked = Arc::default();
        let sink = Arc::clone(&walked);
        let starts = Arc::new(AtomicU32::new(0));
        let started = Arc::clone(&starts);
        let feed = TreeFeed::new(FeedConfig {
            vault: Arc::new(Vault::open(dir).expect("open")),
            index_db: dir.join(".serat/none.db"),
            index_ready: Box::new(move || ready),
            gate: Arc::new(|| {}),
            paths: Arc::default(),
            on_walk_start: Box::new(move || {
                started.fetch_add(1, Ordering::SeqCst);
            }),
            on_walked: Box::new(move |entries| sink.lock().expect("walked").push(entries)),
            on_walk_complete: Box::new(|_| {}),
            on_stopped: Box::new(|| {}),
        });
        (feed, walked, starts)
    }

    #[test]
    fn the_warm_walk_carries_metadata_and_feeds_catch_up_once_sorted() {
        // ADR-0015 section 2, One walk at open: the tree walk is the catch-up's input.
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "b.md");
        write(dir.path(), "a/z.md");
        write(dir.path(), "a/img.png");
        let (feed, walked, starts) = feed_with_walk_hooks(dir.path(), true);
        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("settled", || settled(&log));
        wait_for("the hand-off", || {
            !walked.lock().expect("walked").is_empty()
        });

        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let walked = walked.lock().expect("walked");
        assert_eq!(walked.len(), 1, "catch-up is fed once");
        let paths: Vec<&str> = walked[0].iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["a/img.png", "a/z.md", "b.md"]);
        assert!(
            walked[0].iter().all(|e| e.mtime_ms > 0 && e.size == 1),
            "the warm walk carries size and mtime: {:?}",
            walked[0]
        );
    }

    #[test]
    fn the_no_index_walk_stays_name_and_kind_and_feeds_no_catch_up() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let (feed, walked, starts) = feed_with_walk_hooks(dir.path(), false);
        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("settled", || settled(&log));
        std::thread::sleep(Duration::from_millis(50));
        assert!(walked.lock().expect("walked").is_empty());
        assert_eq!(starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_unready_or_unreadable_index_falls_back_to_the_walk() {
        // `index_ready` true but no database: the read fails, the snapshot is
        // abandoned and the walk populates alone.
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let feed = feed_for(dir.path(), true);
        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("complete", || has_complete(&log));
        let log = log.lock().expect("log");
        assert!(log.iter().any(|m| matches!(
            m,
            TreeMessage::Complete {
                source: ipc::TreeSource::Walk,
                ..
            }
        )));
    }

    #[test]
    fn a_created_directory_that_arrives_with_contents_is_always_rescanned() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let feed = feed_for(dir.path(), false);
        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("settled", || settled(&log));

        write(dir.path(), "moved/deep/x.md");
        write(dir.path(), "moved/y.md");
        feed.path_changed(&VaultPath::new("moved").expect("path"));

        let log = log.lock().expect("log");
        assert_eq!(
            listing(&log, "moved").map(|l| l.iter().map(|e| e.name.as_str()).collect::<Vec<_>>()),
            Some(vec!["deep", "y.md"])
        );
        assert!(listing(&log, "moved/deep").is_some());
    }

    #[test]
    fn a_save_costs_the_tree_nothing_and_a_delete_is_one_remove() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        write(dir.path(), "b.md");
        let feed = feed_for(dir.path(), false);
        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("settled", || settled(&log));
        let before = log.lock().expect("log").len();

        fs::write(dir.path().join("a.md"), b"changed").expect("write");
        feed.path_changed(&VaultPath::new("a.md").expect("path"));
        assert_eq!(log.lock().expect("log").len(), before, "a modification");

        fs::remove_file(dir.path().join("b.md")).expect("rm");
        feed.path_changed(&VaultPath::new("b.md").expect("path"));
        let log = log.lock().expect("log");
        assert_eq!(log.len(), before + 1);
        assert!(matches!(
            &log[before],
            TreeMessage::Remove { dir, name } if dir.is_empty() && name == "b.md"
        ));
    }

    #[test]
    fn a_poisoned_session_is_replaced_and_the_tree_is_resent_from_the_root() {
        // The first sender call (the root listing) panics inside the session's lock:
        // that poisons it. The feed must discard it and stream again.
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let feed = feed_for(dir.path(), false);
        let calls = Arc::new(AtomicU32::new(0));
        let (inner, log) = collecting();
        let counter = Arc::clone(&calls);
        let send: TreeSender = Arc::new(move |m| {
            assert!(
                counter.fetch_add(1, Ordering::SeqCst) != 0,
                "sink failure (test)"
            );
            inner(m)
        });
        feed.open(send).expect("open");
        wait_for("a restarted stream completing", || has_complete(&log));
        wait_for("the restart budget to reset", || {
            feed.state()
                .active
                .as_ref()
                .is_some_and(|a| a.restarts == 0)
        });
        let log = log.lock().expect("log");
        assert!(
            matches!(&log[0], TreeMessage::Listing { dir, .. } if dir.is_empty()),
            "the restarted stream begins at the root: {:?}",
            log[0]
        );
    }

    #[test]
    fn a_sender_that_always_panics_is_given_up_on_not_looped() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let stops = Arc::new(AtomicU32::new(0));
        let stopped = Arc::clone(&stops);
        let feed = TreeFeed::new(FeedConfig {
            vault: Arc::new(Vault::open(dir.path()).expect("open")),
            index_db: dir.path().join(".serat/none.db"),
            index_ready: Box::new(|| false),
            gate: Arc::new(|| {}),
            paths: Arc::default(),
            on_walk_start: Box::new(|| {}),
            on_walked: Box::new(|_| {}),
            on_walk_complete: Box::new(|_| {}),
            on_stopped: Box::new(move || {
                stopped.fetch_add(1, Ordering::SeqCst);
            }),
        });
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let send: TreeSender = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            panic!("always (test)");
        });
        feed.open(send).expect("open");
        wait_for("the stream to give up", || {
            stops.load(Ordering::SeqCst) == 1
        });
        // Further events find a dead stream and do not announce it again.
        feed.path_changed(&VaultPath::new("a.md").expect("path"));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(stops.load(Ordering::SeqCst), 1, "announced once");
        let seen = calls.load(Ordering::SeqCst);
        assert!(
            seen <= MAX_RESTARTS + 1,
            "{seen} sends: the restart is bounded"
        );
    }

    #[test]
    fn a_replaced_stream_stops_writing_to_the_old_channel() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let feed = feed_for(dir.path(), false);
        let (first, first_log) = collecting();
        let (second, second_log) = collecting();
        feed.open(first).expect("open");
        wait_for("first", || settled(&first_log));
        let seen = first_log.lock().expect("log").len();
        feed.open(second).expect("reopen");
        wait_for("second", || settled(&second_log));
        feed.path_changed(&VaultPath::new("a.md").expect("path"));
        write(dir.path(), "late.md");
        feed.path_changed(&VaultPath::new("late.md").expect("path"));
        assert_eq!(first_log.lock().expect("log").len(), seen);
        assert!(matches!(
            second_log.lock().expect("log")[0],
            TreeMessage::Listing { .. }
        ));
    }

    #[test]
    fn marks_are_refused_until_the_stream_has_sent_what_they_claim() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), "a.md");
        let feed = feed_for(dir.path(), false);
        let first = ipc::MarkTreeArgs {
            mark: ipc::TreeMark::FirstScreen,
            entries_in_model: 0,
        };
        let complete = ipc::MarkTreeArgs {
            mark: ipc::TreeMark::Complete,
            entries_in_model: 1,
        };
        // No stream at all.
        feed.mark(&first, 1.0);
        assert_eq!(feed.marks().rejected, 1);
        assert_eq!(feed.marks().first_screen_ms, None);

        let (send, log) = collecting();
        feed.open(send).expect("open");
        wait_for("settled", || settled(&log));
        feed.mark(&first, 5.0);
        feed.mark(&first, 9.0);
        feed.mark(&complete, 6.0);
        let marks = feed.marks();
        assert_eq!(
            marks.first_screen_ms,
            Some(5.0),
            "the first valid mark wins"
        );
        assert_eq!(marks.complete_ms, Some(6.0));
        assert_eq!(marks.entries_in_model, Some(1));
        assert_eq!(marks.delivered, Some((1, ipc::TreeSource::Walk)));
        assert_eq!(marks.empty_folders, Some(0));
    }

    #[test]
    fn the_initial_population_is_held_to_a_burst_and_the_tail_is_flushed_on_time() {
        // Held messages reach the channel within the window even when nothing else
        // arrives to trigger a flush: the flusher thread, not the next arrival.
        let (send, log) = collecting();
        let sink = CoalescingSink::new(send, Arc::new(AtomicBool::new(false)));
        sink.push(TreeOut::Op(TreeOp::Listing {
            dir: String::new(),
            entries: Vec::new(),
        }));
        assert_eq!(
            log.lock().expect("log").len(),
            1,
            "the root is written at once"
        );
        sink.push(TreeOut::Op(TreeOp::Listing {
            dir: "a".to_owned(),
            entries: Vec::new(),
        }));
        assert_eq!(log.lock().expect("log").len(), 1, "a later listing is held");
        wait_for("the window to close", || {
            log.lock().expect("log").len() == 2
        });
    }

    #[test]
    fn a_burst_is_cut_at_the_size_bound_and_keeps_order() {
        let (send, log) = collecting();
        let sink = CoalescingSink::new(send, Arc::new(AtomicBool::new(false)));
        let listing = |dir: &str, n: usize| {
            TreeOut::Op(TreeOp::Listing {
                dir: dir.to_owned(),
                entries: (0..n)
                    .map(|i| TreeEntry {
                        name: format!("note-with-a-longish-name-{i:05}.md"),
                        kind: TreeKind::Note,
                        sidecar: false,
                    })
                    .collect(),
            })
        };
        sink.push(listing("", 0));
        sink.push(listing("a", 1));
        // One message past 256 KiB: it and everything held before it are written now,
        // in the order they were pushed, whether or not the timer got to "a" first.
        sink.push(listing("b", 4000));
        let log = log.lock().expect("log");
        assert_eq!(log.len(), 3);
        assert!(matches!(&log[1], TreeMessage::Listing { dir, .. } if dir == "a"));
        assert!(matches!(&log[2], TreeMessage::Listing { dir, .. } if dir == "b"));
    }

    #[test]
    fn a_closed_receiver_stops_the_stream() {
        let sends = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&sends);
        let send: TreeSender = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            false
        });
        let sink = CoalescingSink::new(send, Arc::new(AtomicBool::new(false)));
        // The root listing is written at once; the refusal closes the sink.
        sink.push(TreeOut::Op(TreeOp::Listing {
            dir: String::new(),
            entries: Vec::new(),
        }));
        for i in 0..5 {
            sink.push(TreeOut::Op(TreeOp::Remove {
                dir: String::new(),
                name: format!("n{i}"),
            }));
        }
        assert_eq!(sends.load(Ordering::SeqCst), 1, "no send after a refusal");
    }
}
