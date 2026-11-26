//! Per-phase wall-clock timing of one save, for `--bench-save-list` (task #207; NFR-PRF-03
//! evidence, NFR-PRF-08, ADR-0019 point 5: report-only).
//!
//! Unlike [`crate::phases`] (compile-time gated, so a hosted CI binary built without the
//! feature carries nothing), this is **armed at run time**, because the binary the budget
//! jobs spawn is the shipped build and must stay so (ADR-0017). Nothing arms it but the
//! `--bench-save-list` driver. What the product path pays while it is not armed is one
//! relaxed load of a `static AtomicBool` per mark and per `begin`: five per
//! `write_file` (four marks, and the arm test in `serat-app`'s `submit_to_index`) against a
//! write that fsyncs; no clock is read, no thread-local touched, nothing
//! allocated.
//!
//! **Who marks what.** serat-vault owns the clock and the four write-side marks
//! ([`SavePhase::Hash`], [`SavePhase::Write`], [`SavePhase::Sync`], [`SavePhase::Rename`]).
//! `Load`, `Parse` and `Submit` are marked by serat-app, from `TimedParser` and
//! `TimedWriter` around the index apply in `AppState::submit_to_index`, and by its two
//! `Other` marks (armed runs only); serat-vault does not see the index.
//!
//! This observes; it never alters the write path. A mark sits between two statements that
//! already existed, and no byte is read, hashed or written differently (C-04, ADR-0008).
//!
//! A phase is named at its *end*: `mark(p)` adds the time since the previous mark (or since
//! [`begin`]) to `p`, so the phases partition the call and their sum is the call up to the
//! last mark.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The phases of the save path ADR-0020 names, in the order they end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePhase {
    /// From `begin` to the end of the vault's pre-write check: the open-vault lock, the
    /// per-path lock (so a wait for either lands here), the read of what is on disk and its
    /// hash against the caller's base, and the hash of the new text.
    Hash,
    /// The directory check, the permissions stat, creating the temp file and `write_all`.
    Write,
    /// The temp file's `sync_all` (fsync / `FlushFileBuffers`).
    Sync,
    /// Permissions on the temp file, the rename over the target and the directory sync.
    Rename,
    /// The index side's load: stat, read and hash of the note, before its parse. Armed runs
    /// mark [`SavePhase::Other`] first, so the vault write's return, the open-vault
    /// bookkeeping and the index-handle slot's lock are not in it.
    Load,
    /// The index side's parse.
    Parse,
    /// Building the op and handing it to the writer's queue.
    Submit,
    /// Marked by serat-app at the start of the index hand-off and once the index-handle slot's
    /// lock is taken, so that the write's return and that lock wait are not charged to
    /// [`SavePhase::Load`]. The harness adds the interval's unmarked remainder to it.
    Other,
}

/// How many phases there are.
pub const COUNT: usize = 8;

impl SavePhase {
    /// Every phase, in the order they end.
    pub const ALL: [SavePhase; COUNT] = [
        SavePhase::Hash,
        SavePhase::Write,
        SavePhase::Sync,
        SavePhase::Rename,
        SavePhase::Load,
        SavePhase::Parse,
        SavePhase::Submit,
        SavePhase::Other,
    ];

    /// The name the exit report and the harness use (`save_phase_<name>_ms`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SavePhase::Hash => "hash",
            SavePhase::Write => "write",
            SavePhase::Sync => "sync",
            SavePhase::Rename => "rename",
            SavePhase::Load => "load",
            SavePhase::Parse => "parse",
            SavePhase::Submit => "submit",
            SavePhase::Other => "other",
        }
    }
}

/// Which handle a durability flush was issued on (test log only).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fsync {
    File,
    Dir,
}
#[cfg(not(test))]
#[derive(Clone, Copy)]
pub(crate) enum Fsync {
    File,
    Dir,
}

/// One entry of the `cfg(test)` event log: a phase mark or an fsync, in call order.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Mark(SavePhase),
    Fsync(Fsync),
}

#[cfg(test)]
thread_local! {
    static LOG: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn log_event(event: Event) {
    LOG.with(|log| log.borrow_mut().push(event));
}

#[cfg(test)]
pub(crate) fn log_fsync(what: Fsync) {
    log_event(Event::Fsync(what));
}

/// Take this thread's event log. Test only; recorded whether or not [`arm`] was called.
#[cfg(test)]
pub(crate) fn drain_log() -> Vec<Event> {
    LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

static ARMED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static RUN: Cell<Option<Run>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct Run {
    last: Instant,
    spent: [Duration; COUNT],
}

/// Turn collection on for the process. Called once by `--bench-save-list`; never by the
/// product path. There is no way to turn it off again: the process that armed it is a
/// benchmark that exits.
#[doc(hidden)]
pub fn arm() {
    ARMED.store(true, Ordering::Relaxed);
}

/// Whether [`arm`] was called.
#[inline]
#[must_use]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Start collecting on this thread, discarding anything already collected. A no-op unless
/// armed.
#[inline]
pub fn begin() {
    if armed() {
        RUN.with(|run| {
            run.set(Some(Run {
                last: Instant::now(),
                spent: [Duration::ZERO; COUNT],
            }));
        });
    }
}

/// Add the time since the previous mark to `phase`. A no-op unless armed and this thread
/// has begun.
#[inline]
pub fn mark(phase: SavePhase) {
    #[cfg(test)]
    log_event(Event::Mark(phase));
    if armed() {
        mark_armed(phase);
    }
}

#[cold]
fn mark_armed(phase: SavePhase) {
    RUN.with(|run| {
        if let Some(mut r) = run.get() {
            let now = Instant::now();
            r.spent[phase as usize] += now.duration_since(r.last);
            r.last = now;
            run.set(Some(r));
        }
    });
}

/// Stop collecting on this thread and return what each phase took, in [`SavePhase::ALL`]
/// order. `None` when nothing was begun here (not armed, or no `begin`).
#[must_use]
pub fn finish() -> Option<[Duration; COUNT]> {
    RUN.with(Cell::take).map(|r| r.spent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_accumulate_by_name_and_finish_takes_the_run() {
        arm();
        assert!(armed());
        // A mark with no begin on this thread records nothing and does not panic.
        mark(SavePhase::Sync);
        assert!(finish().is_none());
        begin();
        std::thread::sleep(Duration::from_millis(3));
        mark(SavePhase::Write);
        std::thread::sleep(Duration::from_millis(3));
        mark(SavePhase::Sync);
        mark(SavePhase::Write);
        let spent = finish().expect("a run was begun");
        assert!(spent[SavePhase::Write as usize] >= Duration::from_millis(3));
        assert!(spent[SavePhase::Sync as usize] >= Duration::from_millis(3));
        assert_eq!(spent[SavePhase::Hash as usize], Duration::ZERO);
        assert!(finish().is_none(), "finish took it");
    }

    #[test]
    fn names_are_the_report_keys_and_cover_every_phase_once() {
        let names: Vec<_> = SavePhase::ALL.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            [
                "hash", "write", "sync", "rename", "load", "parse", "submit", "other"
            ]
        );
        for (i, p) in SavePhase::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i, "the array index is the order");
        }
    }
}
