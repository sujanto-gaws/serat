//! The startup trace: the marks NFR-PRF-01 is measured on, and the reasons none of
//! them can be faked.
//!
//! NFR-PRF-01 is two budgets on one cold start — an interactive window within 400 ms
//! and a usable vault within one second (architecture §3.4). The budget job that
//! asserts them is `bench/budget/src/bin/prf01-cold-start.rs`, and it cannot assert
//! anything the application does not tell it. This module is what it reads.
//!
//! # The contract with the harness
//!
//! ```text
//! SERAT_STARTUP_T0_UNIX_NANOS=<u128>  SERAT_STARTUP_TRACE=<file>  \
//!     serat --vault <dir> --open <note.md> --exit-after-ready
//! ```
//!
//! writes one line of JSON to the trace file and exits.
//!
//! # Why t0 arrives from outside
//!
//! `t0` is process spawn *as the harness measured it*, not an `Instant::now()` inside
//! `main`. Everything before `main` — the loader, the WebView2 runtime, relocations —
//! is cold-start cost a user feels, and a clock started inside `main` cannot see it.
//! Reading the operating system's process-creation time would need `unsafe`, which the
//! workspace forbids, so the harness reads a wall clock immediately before
//! `Command::spawn` and passes it in.
//!
//! Without that variable the origin falls back to `main`, and the trace records
//! `"t0_source":"in_process"`. The budget job rejects that value: a figure that
//! undercounts pre-main is not the measurement NFR-PRF-01 is about, and once both are
//! just a number of milliseconds they are indistinguishable.
//!
//! ## The wall clock is used exactly once
//!
//! `SystemTime` is subject to NTP steps, and a step mid-run would corrupt a sample
//! rather than fail it. So [`Startup::begin`] reads `SystemTime::now()` **and**
//! `Instant::now()` together, computes the t0-to-`main` offset from the wall clock in
//! that one sub-50 ms window, and derives every later mark from the monotonic clock.
//! Marks are then monotonic by construction rather than by luck. (test-engineer's
//! refinement, and a good one.)
//!
//! # Why the paint mark cannot be the creation mark
//!
//! test-engineer's standing objection: if "window painted" is instrumented at window
//! *creation*, the budget passes and the product ships a blank rectangle. F-377 found
//! that a call **taken from the right place could still say the wrong thing** — a
//! substring test asserted `boot.js` mentions the mark and calls it from inside a
//! `requestAnimationFrame`, and two mutations survived it: collapsing the double rAF
//! to one (the mark then lands before the compositor has the frame), and hoisting the
//! `invoke` call out of both callbacks to script start with a no-op rAF left behind to
//! satisfy the substring. Four things answer it now, and the third is the one that
//! matters:
//!
//! 1. [`Startup::mark_first_paint`] is called from exactly one place — the
//!    `mark_first_paint` IPC command, which the webview invokes.
//! 2. **The call must carry evidence a script cannot have before the frame is
//!    composited.** The argument is the `startTime` of the browser's own
//!    `first-contentful-paint` `PerformancePaintTiming` entry (`app/src/startup.ts`),
//!    which the Paint Timing API does not report until content has actually been
//!    painted — not a value derived from counting `requestAnimationFrame` callbacks,
//!    which a script can call as many or as few times as it likes regardless of
//!    whether anything was ever composited. [`Startup::mark_first_paint`] rejects
//!    (returns [`PaintEvidenceRejected`], sets nothing) a non-finite or non-positive
//!    value rather than defaulting — a call with nothing genuine to send has nothing
//!    accepted, on any platform, including one where the Paint Timing API does not
//!    exist at all. Task #112, F-396(b): the paint entry alone is sound only as long
//!    as `app/index.html` stays contentless, a property nothing in this crate
//!    asserts, so [`validate_paint_evidence`] additionally checks the two
//!    `requestAnimationFrame` timestamps the caller sends against the paint entry,
//!    before either reaches this function.
//! 3. **There is no other way to set it.** No Rust path stamps it: not `main`, not the
//!    window builder, not a `WindowEvent`. A shell that paints nothing, or whose
//!    evidence is rejected, never marks, so [`Startup::trace`] fails with
//!    [`TraceIncomplete`] and `--exit-after-ready` exits non-zero. A blank rectangle is
//!    a red job, not a fast one.
//! 4. `t0_to_window_created_ms` is emitted beside it, and the budget job fails a run
//!    where the gap is under 5 ms — test-engineer's threshold, a mutation detector
//!    rather than a performance floor.
//!
//! # What "usable" requires
//!
//! §3.4's table row — "the tree and last-open buffer render as they arrive" — is a
//! summary. The binding definition is the three preconditions stated below it, and
//! `t_usable` is the **latest** of all of them, not the first:
//!
//! | §3.4 precondition | Here |
//! |---|---|
//! | the open check has finished, so the index state is known | [`Startup::mark_index_settled`], from the actor's first published state that is not `Opening` |
//! | the file tree has been delivered | [`Startup::mark_tree_first_screen`] — the root listing applied and painted (ADR-0015 §3, §5), stamped from [`crate::tree_stream::TreeMarks::first_screen_ms`]. P0's stand-in, the last-open buffer ([`Startup::mark_buffer_rendered`]), is still recorded but is no longer a term |
//! | the watcher is registered | [`Startup::mark_watcher`], on either outcome |
//!
//! Every one of them fails *invisibly* if it is left out. Nothing errors; the number
//! is simply better than the requirement it claims to measure. The watcher clause is
//! the sharpest case, and it is test-engineer's: a vault reported usable before it is
//! watched has a window in which an edit is silently missed.
//!
//! The watcher gate is on the stage having **resolved**, not on it having succeeded.
//! [`WatcherOutcome::Failed`] is legitimate — a vault with no watcher is stale rather
//! than broken, which is design goal 5 — and the trace records which outcome it was,
//! because a usable figure taken with no watcher is not comparable to one taken with
//! one and the gate report should not have to guess.
//!
//! A fourth term is not in §3.4 and is here anyway: the shell must have **painted**.
//! Both webview marks are IPC round trips and the buffer read can finish first; this
//! shell's very first run exited with `vault_usable` taken and `window_visible` still
//! in flight, and produced no trace at all. Requiring the paint makes the ordering
//! structural rather than something a front end has to remember.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

/// Environment variable carrying the harness's spawn instant, in nanoseconds since
/// the Unix epoch.
pub const T0_ENV: &str = "SERAT_STARTUP_T0_UNIX_NANOS";

/// Environment variable naming the file the trace line is written to.
pub const TRACE_ENV: &str = "SERAT_STARTUP_TRACE";

/// Environment variable overriding [`DEFAULT_DEADLINE`], in milliseconds. Read only when
/// [`TRACE_ENV`] is set (task #178). Anything unparsable or zero means the default.
pub const DEADLINE_ENV: &str = "SERAT_STARTUP_DEADLINE_MS";

/// When the diagnostic watchdog prints the marks snapshot if the shell has not painted:
/// 55 s, so the line lands before a 60 s harness kill ([`crate::cli::DEFAULT_READY_TIMEOUT`]).
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(55);

/// Which clock the trace was measured against, as it appears in the JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum T0Source {
    /// t0 came from the harness, before the process existed. The only value the budget
    /// job accepts.
    Harness,
    /// t0 is the first line of `main`. Undercounts by whatever the loader cost.
    InProcess,
}

/// Which build produced the number.
///
/// In the trace because a budget measured in a debug build is not a measurement, and
/// the job rejects `debug` the same way it rejects an in-process t0. Always emitted —
/// a mark that only exists in release is a mark nobody exercises until CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// `cfg!(debug_assertions)`.
    Debug,
    /// Anything else.
    Release,
}

impl Profile {
    /// This build's profile.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Debug
        } else {
            Self::Release
        }
    }
}

/// What became of the filesystem watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatcherOutcome {
    /// Subscribed to the vault root. Edits made from here on produce events.
    Registered,
    /// The platform watcher could not be started. The vault is still editable and
    /// still walks; it is stale rather than broken (design goal 5). Recorded so a
    /// cold-start number taken in this state is legible as one.
    Failed,
}

/// The zero of the timeline, and the monotonic clock everything after `main` uses.
#[derive(Debug, Clone, Copy)]
struct Clock {
    source: T0Source,
    /// Milliseconds from t0 to the first line of `main`. Zero when there is no harness.
    main_offset_ms: f64,
    /// Taken in the same breath as the wall-clock reading above.
    since_main: Instant,
}

impl Clock {
    fn elapsed_ms(&self) -> f64 {
        self.main_offset_ms + self.since_main.elapsed().as_secs_f64() * 1000.0
    }
}

/// The marks, in the order they can legally occur.
#[derive(Debug, Default)]
struct Marks {
    state_ready: Option<f64>,
    runtime_ready: Option<f64>,
    setup_done: Option<f64>,
    index_settled: Option<f64>,
    window_created: Option<f64>,
    window_visible: Option<f64>,
    /// The raw `first-contentful-paint` `PerformancePaintTiming` `startTime` the
    /// webview sent as evidence for `window_visible`, kept only for audit — it is on
    /// the browser's navigation-origin clock, not this process's `t0`, and is never
    /// compared against another mark. See [`PaintEvidenceRejected`].
    paint_evidence_ms: Option<f64>,
    /// The second `requestAnimationFrame` timestamp the webview sent alongside
    /// `paint_evidence_ms`, already validated against it by
    /// [`validate_paint_evidence`] before this call — kept here for the same reason
    /// `paint_evidence_ms` is: audit, on the same navigation-origin clock, never
    /// compared against another mark in this struct. Task #115 (F-403, F-411, F-416):
    /// `bench/budget`'s `frame_to_paint_gap_ms()` diagnostic has had a place to read
    /// this from since #106, and nothing wrote it until now — every trace read `None`.
    raf2_ms: Option<f64>,
    watcher: Option<(f64, WatcherOutcome)>,
    buffer_rendered: Option<f64>,
    /// `tree_first_screen` (ADR-0015 §3), on the same `t0` clock, as the tree stream
    /// recorded it. The tree term of `t_usable` from P1 (ADR-0015 §5).
    tree_first_screen: Option<f64>,
    walk_complete: Option<f64>,
    notes_walked: Option<u32>,
    last_open: Option<String>,
    vault: Option<String>,
}

impl Marks {
    /// §3.4's three preconditions plus the paint, as the latest of the four.
    ///
    /// See the module note for why each term is here and what its absence would hide.
    /// Every one of them can only move the mark *later*, which is the point: a
    /// precondition that could be dropped without the number changing would not be a
    /// precondition.
    fn vault_usable(&self) -> Option<f64> {
        let painted = self.window_visible?;
        let tree = self.tree_first_screen?;
        let index = self.index_settled?;
        let (watcher, _) = self.watcher?;
        Some(painted.max(tree).max(index).max(watcher))
    }
}

/// One cold start's timeline.
///
/// Shared by the whole process: `main` builds it, the window builder marks creation,
/// the vault thread marks the watcher, and the two instrumentation commands mark paint
/// and buffer from the webview.
#[derive(Debug)]
pub struct Startup {
    clock: Clock,
    main_ms: f64,
    marks: Mutex<Marks>,
    /// Signalled when either half of "usable" arrives, so `--exit-after-ready` can wait
    /// without polling.
    usable: Condvar,
}

impl Startup {
    /// Start the timeline. Call this on the first line of `main` and nowhere else.
    ///
    /// Reads [`T0_ENV`]. A malformed value is treated as absent rather than as an
    /// error — a bad environment variable must not stop the application from starting,
    /// and the fallback is recorded in the trace, so the failure is visible where it
    /// matters instead of fatal where it does not.
    #[must_use]
    pub fn begin() -> Self {
        Self::begin_with(std::env::var(T0_ENV).ok().as_deref())
    }

    /// [`Startup::begin`] against a supplied `t0`, so the parse and the fallback can be
    /// tested without an environment variable other tests share.
    #[must_use]
    pub fn begin_with(t0: Option<&str>) -> Self {
        // The two clocks are read together and the wall clock is not read again. See
        // the module note: this is the whole of the process's exposure to NTP.
        let wall = SystemTime::now();
        let since_main = Instant::now();

        let spawned = t0
            .and_then(|raw| raw.trim().parse::<u128>().ok())
            .and_then(|nanos| u64::try_from(nanos).ok())
            .and_then(|nanos| SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(nanos)));

        let clock = match spawned {
            Some(t0) => Clock {
                source: T0Source::Harness,
                main_offset_ms: wall
                    .duration_since(t0)
                    .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0),
                since_main,
            },
            None => Clock {
                source: T0Source::InProcess,
                main_offset_ms: 0.0,
                since_main,
            },
        };

        Self {
            clock,
            main_ms: clock.main_offset_ms,
            marks: Mutex::new(Marks::default()),
            usable: Condvar::new(),
        }
    }

    /// Milliseconds since t0, as a float, because 400 ms is a budget and rounding to
    /// whole milliseconds throws away a quarter of a percent of it for nothing.
    ///
    /// Monotonic after `main`, by construction.
    #[must_use]
    pub fn elapsed_ms(&self) -> f64 {
        self.clock.elapsed_ms()
    }

    /// Which clock this timeline is measured against.
    #[must_use]
    pub fn t0_source(&self) -> T0Source {
        self.clock.source
    }

    /// Settings are loaded and `AppState` is built: the end of the work this crate
    /// does on the main thread *before* handing control to Tauri.
    ///
    /// One of the two brackets that make "Serat's own main-thread work" a measurement
    /// rather than a subtraction. Everything between [`Startup::begin`] and this mark
    /// is ours — argument parsing, the settings TOML read, state construction. See
    /// [`Trace::serat_main_thread_ms`].
    pub fn mark_state_ready(&self) {
        let now = self.elapsed_ms();
        self.lock().state_ready.get_or_insert(now);
    }

    /// Our work inside Tauri's `setup` is finished and the window is about to be
    /// built.
    ///
    /// The closing bracket. Everything between [`Startup::mark_runtime_ready`] and
    /// this mark is ours — installing the event sink, opening the vault, spawning the
    /// index and vault threads, resolving the note to open, serialising the boot info.
    pub fn mark_setup_done(&self) {
        let now = self.elapsed_ms();
        self.lock().setup_done.get_or_insert(now);
    }

    /// The Tauri runtime is up and `setup` has been entered.
    ///
    /// Diagnostic, and the one that turned out to matter: it splits the pre-paint cost
    /// into "Tauri and the platform webview starting" and "Serat building a window",
    /// which are different problems with different owners. Without it, everything
    /// before the first paint is one number nobody can act on.
    pub fn mark_runtime_ready(&self) {
        let now = self.elapsed_ms();
        self.lock().runtime_ready.get_or_insert(now);
    }

    /// The window object now exists.
    ///
    /// Diagnostic only. **This is not "window visible"** and nothing here touches
    /// `window_visible`; the whole point of emitting both is that the harness can see
    /// the distance between them.
    pub fn mark_window_created(&self) {
        let now = self.elapsed_ms();
        self.lock().window_created.get_or_insert(now);
    }

    /// The first frame has been composited, as reported by the webview.
    ///
    /// `first_contentful_paint_ms` must be the `startTime` of the browser's own
    /// `first-contentful-paint` `PerformancePaintTiming` entry (see `webview/boot.js`
    /// and the module note above). It is evidence, not a timestamp this function
    /// trusts on its own clock — the mark itself is still taken from
    /// [`Startup::elapsed_ms`] at the moment this call arrives, so every mark in the
    /// trace stays on one clock. What the argument buys is that a call with nothing
    /// real to send — script-start, a mutated callback, any path that runs before the
    /// frame is composited — has no positive, finite value to offer and is refused.
    ///
    /// `raf2_ms` is the second `requestAnimationFrame` timestamp the caller also
    /// validated `first_contentful_paint_ms` against (task #112's
    /// [`validate_paint_evidence`]). This method does not re-check it — that already
    /// happened — it only records it, so [`Trace::raf2_ms`] can carry a real value
    /// instead of the `None` every trace reported before task #115.
    ///
    /// # Errors
    ///
    /// [`PaintEvidenceRejected`] if `first_contentful_paint_ms` is not finite and
    /// positive. **Rejection, not a default**: on a platform where the Paint Timing API
    /// does not exist at all, the caller has nothing to send either, and the correct
    /// behaviour is the same — no mark, not a fabricated one (F-377).
    ///
    /// Returns `Ok(false)` if the mark was already set. A repeat is ignored rather than
    /// refused: a webview that reloads must not be able to move a mark that has already
    /// been taken, and it must not crash the shell for trying either.
    pub fn mark_first_paint(
        &self,
        first_contentful_paint_ms: f64,
        raf2_ms: f64,
    ) -> Result<bool, PaintEvidenceRejected> {
        if !first_contentful_paint_ms.is_finite() || first_contentful_paint_ms <= 0.0 {
            return Err(PaintEvidenceRejected::new(first_contentful_paint_ms));
        }
        let now = self.elapsed_ms();
        let mut marks = self.lock();
        if marks.window_visible.is_some() {
            return Ok(false);
        }
        marks.window_visible = Some(now);
        marks.paint_evidence_ms = Some(first_contentful_paint_ms);
        marks.raf2_ms = Some(raf2_ms);
        drop(marks);
        self.usable.notify_all();
        Ok(true)
    }

    /// Block until the shell has painted, or `timeout` elapses. Returns whether it
    /// painted.
    ///
    /// The vault thread waits on this before starting the walk. §3.4 puts the walk off
    /// the cold-start critical path, and *measured*, putting it on another thread is
    /// not enough to achieve that: another thread on the same disk and the same cores
    /// is still on the critical path. See `crate::vault` for the numbers.
    ///
    /// On timeout the caller proceeds anyway. A window that never painted is already a
    /// failed start; refusing to walk the vault as well would turn one problem into
    /// two.
    pub fn wait_for_first_paint(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut marks = self.lock();
        while marks.window_visible.is_none() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (guard, wait) = self
                .usable
                .wait_timeout(marks, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            marks = guard;
            if wait.timed_out() && marks.window_visible.is_none() {
                return false;
            }
        }
        true
    }

    /// The index open check finished: the actor has published a state that is not
    /// `Opening`, so the index state is known.
    ///
    /// §3.4's first precondition for `t_usable`. `Rebuilding` and `Unavailable` settle
    /// it as much as `Ready` does — the requirement is that the state is *known*, not
    /// that it is good, and a shell that waited for `Ready` would hang for the length
    /// of a rebuild on exactly the vault where the window most needs to be up.
    pub fn mark_index_settled(&self) {
        let now = self.elapsed_ms();
        let mut marks = self.lock();
        if marks.index_settled.is_none() {
            marks.index_settled = Some(now);
            drop(marks);
            self.usable.notify_all();
        }
    }

    /// The filesystem watcher settled, one way or the other.
    ///
    /// Half of "usable": a vault nobody is watching is not usable, it is quietly
    /// diverging. See the module note.
    pub fn mark_watcher(&self, outcome: WatcherOutcome) {
        let now = self.elapsed_ms();
        let mut marks = self.lock();
        if marks.watcher.is_none() {
            marks.watcher = Some((now, outcome));
            drop(marks);
            self.usable.notify_all();
        }
    }

    /// The file tree has been delivered — in P0, the last-open buffer is rendered and
    /// the editor takes keystrokes.
    ///
    /// §3.4's second precondition for `t_usable`. Note that the binding definition is
    /// the list of preconditions, not §3.4's summary table row: this mark is **one of
    /// four** terms, and on its own it says nothing about whether the vault is usable.
    /// See the module note.
    ///
    /// Returns `false` if already marked, for the same reason as
    /// [`Startup::mark_first_paint`].
    pub fn mark_buffer_rendered(&self) -> bool {
        let now = self.elapsed_ms();
        let mut marks = self.lock();
        if marks.buffer_rendered.is_some() {
            return false;
        }
        marks.buffer_rendered = Some(now);
        drop(marks);
        self.usable.notify_all();
        true
    }

    /// The file tree's first screen was applied and painted (ADR-0015 §3): §3.4's tree
    /// precondition for `t_usable` from P1 (ADR-0015 §5), in place of
    /// [`Startup::mark_buffer_rendered`].
    ///
    /// `ms` is [`crate::tree_stream::TreeMarks::first_screen_ms`] exactly as the stream
    /// recorded it (a front-end mark the stream has earned, never one it merely claimed),
    /// so the trace and the tree's own marks cannot disagree. Returns `false` if already
    /// marked; the first vault's first screen is the cold start's.
    pub fn mark_tree_first_screen(&self, ms: f64) -> bool {
        let mut marks = self.lock();
        if marks.tree_first_screen.is_some() {
            return false;
        }
        marks.tree_first_screen = Some(ms);
        drop(marks);
        self.usable.notify_all();
        true
    }

    /// The vault walk finished, having found `entries` files.
    ///
    /// Unbudgeted and deliberately reported: §3.4 puts the walk *off* the critical
    /// path, and P0 finding F-12 measured it at ~2.0 s on the 20k vault. A trace that
    /// showed only the two budgets would let a reader assume the walk was inside them.
    pub fn mark_walk_complete(&self, entries: u32) {
        let now = self.elapsed_ms();
        let mut marks = self.lock();
        marks.walk_complete.get_or_insert(now);
        marks.notes_walked = Some(entries);
        drop(marks);
        self.usable.notify_all();
    }

    /// Block until the vault walk has finished, or `timeout` elapses.
    ///
    /// Only `--exit-after-walk` waits on this. Nothing a user does waits for the walk;
    /// that is the whole point of deferring it.
    pub fn wait_for_walk_complete(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut marks = self.lock();
        while marks.walk_complete.is_none() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (guard, wait) = self
                .usable
                .wait_timeout(marks, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            marks = guard;
            if wait.timed_out() && marks.walk_complete.is_none() {
                return false;
            }
        }
        true
    }

    /// Record which note the shell opened, so a run that opened nothing is legible as
    /// one.
    pub fn set_last_open(&self, path: Option<String>) {
        self.lock().last_open = path;
    }

    /// Record which vault this start was against, so the harness can check it opened
    /// the one it meant.
    pub fn set_vault(&self, root: Option<String>) {
        self.lock().vault = root;
    }

    /// Block until the vault is usable — both halves — or `timeout` elapses.
    ///
    /// Returns `true` if it became usable. Used only by `--exit-after-ready`; the
    /// application never waits on this.
    pub fn wait_for_usable(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut marks = self.lock();
        while marks.vault_usable().is_none() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (guard, wait) = self
                .usable
                .wait_timeout(marks, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            marks = guard;
            if wait.timed_out() && marks.vault_usable().is_none() {
                return false;
            }
        }
        true
    }

    /// Block until the trace can be written: the vault is usable **and** the buffer mark
    /// (which `trace()` still requires, though it is no longer a term of usable) has
    /// arrived, or `timeout` elapses. Returns `true` if both did.
    ///
    /// `--exit-after-ready` waits on this, not on [`Startup::wait_for_usable`]
    /// (task #192): `vault_usable` is complete the instant the last term lands, and
    /// after a frame-gate retry (#186) that last term is the paint, whose reply is what
    /// the page awaits before it sends `mark_vault_usable`. Exiting on usable alone
    /// read the trace in that round trip and failed a healthy launch with code 2.
    /// `t_usable` is unchanged: this only delays the read, not any stamped mark.
    pub fn wait_for_trace_terms(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut marks = self.lock();
        while marks.vault_usable().is_none() || marks.buffer_rendered.is_none() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            marks = self
                .usable
                .wait_timeout(marks, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    /// The trace line, or the reason there isn't one.
    ///
    /// # Errors
    ///
    /// [`TraceIncomplete`] naming the missing mark. This is the guard that makes a
    /// blank window a failure: a shell that never painted has no `window_visible`, and
    /// there is no fallback that would supply one.
    pub fn trace(&self) -> Result<Trace, TraceIncomplete> {
        let marks = self.lock();
        let window_visible = marks.window_visible.ok_or(TraceIncomplete {
            missing: "window_visible",
            because: "the webview never reported a first painted frame. \
                      That is a shell that did not paint, not a shell that painted fast.",
        })?;
        let buffer = marks.buffer_rendered.ok_or(TraceIncomplete {
            missing: "vault_usable",
            because: "the webview never reported the buffer rendered. The buffer is no \
                      longer a term of vault_usable (the tree's first screen replaced it, \
                      ADR-0015 section 5), but the trace still records it and the shell \
                      reports it through mark_vault_usable, so its absence means that \
                      call never arrived.",
        })?;
        let tree_first_screen = marks.tree_first_screen.ok_or(TraceIncomplete {
            missing: "vault_usable",
            because: "the file tree's first screen was never reported (ADR-0015 section 5): \
                      a vault with no tree on screen is not usable yet.",
        })?;
        let index_ms = marks.index_settled.ok_or(TraceIncomplete {
            missing: "vault_usable",
            because: "the index open check never finished, so the index state is still \
                      unknown (architecture 3.4). A shell cannot report a vault usable \
                      while it cannot say what its index is doing.",
        })?;
        let (watcher_ms, watcher) = marks.watcher.ok_or(TraceIncomplete {
            missing: "vault_usable",
            because: "the filesystem watcher never settled. A vault that is usable but \
                      unwatched silently misses every edit made in that window \
                      (architecture §3.4), so it is not usable yet.",
        })?;

        Ok(Trace {
            t0_to_window_visible_ms: window_visible,
            paint_evidence_ms: marks.paint_evidence_ms,
            raf2_ms: marks.raf2_ms,
            t0_to_vault_usable_ms: marks.vault_usable().expect("every term was just unwrapped"),
            t0_to_main_ms: self.main_ms,
            t0_to_state_ready_ms: marks.state_ready,
            t0_to_runtime_ready_ms: marks.runtime_ready,
            t0_to_setup_done_ms: marks.setup_done,
            t0_to_window_created_ms: marks.window_created,
            t0_to_buffer_rendered_ms: buffer,
            tree: true,
            t0_to_tree_first_screen_ms: tree_first_screen,
            t0_to_index_settled_ms: index_ms,
            t0_to_watcher_ms: watcher_ms,
            watcher,
            t0_to_walk_complete_ms: marks.walk_complete,
            t0_source: self.clock.source,
            profile: Profile::current(),
            schema_version: serat_index::INDEX_VERSION,
            notes_walked: marks.notes_walked,
            vault: marks.vault.clone(),
            last_open: marks.last_open.clone(),
            usable_definition: USABLE_DEFINITION,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Marks> {
        self.marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One line naming every mark as set or missing, for a launch that never reported a
    /// first painted frame (task #178, NFR-PRF-01, NFR-PRF-08).
    ///
    /// `main` is always set (the timeline starts in it). `tree_first_screen`,
    /// `index_settled` and `watcher` are the preconditions of `vault_usable` besides the
    /// paint (ADR-0014 as amended by ADR-0015 section 5); `buffer_rendered` is still
    /// recorded but no longer a term (task #141). `walk_complete` is reported, not a
    /// term: the walk waits on the first paint, so it says whether that gate timed out.
    /// `vault_usable` is derived, set only when all its terms are. The watchdog prints
    /// this only while `window_visible` is unmarked, so it describes a launch that never
    /// painted, not one that painted and never settled.
    #[must_use]
    pub fn marks_snapshot_line(&self) -> String {
        let elapsed = self.elapsed_ms();
        let marks = self.lock();
        let table: [(&str, bool); 12] = [
            ("main", true),
            ("state_ready", marks.state_ready.is_some()),
            ("runtime_ready", marks.runtime_ready.is_some()),
            ("setup_done", marks.setup_done.is_some()),
            ("window_created", marks.window_created.is_some()),
            ("window_visible", marks.window_visible.is_some()),
            ("buffer_rendered", marks.buffer_rendered.is_some()),
            ("tree_first_screen", marks.tree_first_screen.is_some()),
            ("index_settled", marks.index_settled.is_some()),
            ("watcher", marks.watcher.is_some()),
            ("walk_complete", marks.walk_complete.is_some()),
            ("vault_usable", marks.vault_usable().is_some()),
        ];
        drop(marks);
        let names = |want: bool| {
            table
                .iter()
                .filter(|(_, set)| *set == want)
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "serat: startup marks after {elapsed:.0} ms: set=[{}] missing=[{}]",
            names(true),
            names(false)
        )
    }

    /// Start the diagnostic thread: if `window_visible` has not been marked within
    /// `deadline`, call `emit` once with [`Startup::marks_snapshot_line`]; then, either
    /// way, call `emit` once with the web view runtime line from `version`. The query
    /// runs here, after the paint or the deadline, so it never competes with the cold
    /// start (it would otherwise sit inside `S_setup`). It only reports; it never exits
    /// the process or changes any mark.
    ///
    /// The thread sleeps on the same condvar `mark_first_paint` signals, so it costs
    /// nothing while waiting. The join handle yields whether the deadline fired. Callers
    /// start it only when [`TRACE_ENV`] is set; its one remaining cost is this spawn.
    ///
    /// # Errors
    ///
    /// The thread could not be spawned.
    pub fn spawn_watchdog(
        self: &Arc<Self>,
        deadline: Duration,
        emit: impl Fn(&str) + Send + 'static,
        version: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) -> std::io::Result<std::thread::JoinHandle<bool>> {
        let startup = Arc::clone(self);
        std::thread::Builder::new()
            .name("serat-startup-watchdog".to_owned())
            .spawn(move || {
                let limit = Instant::now() + deadline;
                let mut marks = startup.lock();
                let mut fired = false;
                while marks.window_visible.is_none() {
                    let Some(remaining) = limit.checked_duration_since(Instant::now()) else {
                        fired = true;
                        break;
                    };
                    marks = startup
                        .usable
                        .wait_timeout(marks, remaining)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
                drop(marks);
                if fired {
                    emit(&startup.marks_snapshot_line());
                }
                emit(&webview_version_line(version()));
                fired
            })
    }
}

/// The watchdog deadline from [`DEADLINE_ENV`]'s value: whole milliseconds above zero,
/// otherwise [`DEFAULT_DEADLINE`]. Never panics; capped at a day so the arithmetic on
/// it cannot overflow.
#[must_use]
pub fn deadline_from(value: Option<&str>) -> Duration {
    const CAP_MS: u64 = 24 * 60 * 60 * 1000;
    match value.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(ms) if ms > 0 => Duration::from_millis(ms.min(CAP_MS)),
        _ => DEFAULT_DEADLINE,
    }
}

/// The line printed around `WebviewWindowBuilder::build` under the trace env (task #192):
/// `phase` is `"entering"` or `"returned"`. Run 432 had a launch with `setup_done` set and
/// `window_created` never set for 55 s; whether `entering` appeared and `returned` did
/// not (blocked in the build, which creates the WebView2 environment and controller) or
/// neither (blocked between `setup_done` and the build) is what separates the causes.
/// `data_folder` is `WEBVIEW2_USER_DATA_FOLDER` as the process sees it.
#[must_use]
pub fn window_build_line(phase: &str, elapsed_ms: f64, data_folder: Option<&str>) -> String {
    format!(
        "serat: main window build {phase} at {elapsed_ms:.0} ms (webview data folder: {})",
        data_folder.unwrap_or("default")
    )
}

/// The line naming the web view runtime this launch runs on. `version` is the result of
/// `tauri::webview_version()`; a failure or an empty string reads `unknown`.
#[must_use]
pub fn webview_version_line(version: Result<String, String>) -> String {
    match version {
        Ok(v) if !v.trim().is_empty() => format!("serat: webview runtime {}", v.trim()),
        _ => "serat: webview runtime unknown".to_owned(),
    }
}

/// The line printed for one [`PaintEvidenceRejected`], at the point of rejection.
#[must_use]
pub fn rejection_line(elapsed_ms: f64, rejected: &PaintEvidenceRejected) -> String {
    format!(
        "serat: paint evidence rejected at {elapsed_ms:.0} ms: {}",
        rejected.reason
    )
}

/// Report one rejection through `sink` (stderr in the application). A seam so a test
/// can see the line without capturing the process's stderr.
pub fn report_rejection(sink: &dyn Fn(&str), elapsed_ms: f64, rejected: &PaintEvidenceRejected) {
    sink(&rejection_line(elapsed_ms, rejected));
}

/// Everything #178 adds, behind [`TRACE_ENV`]: the watchdog on [`DEADLINE_ENV`] and the
/// runtime version line, on one thread. Returns `None` without starting anything when
/// `trace_enabled` is false. The deadline is not clamped to `--ready-timeout-ms`: a run
/// that lowers that to 55 s or less must set [`DEADLINE_ENV`] below it too, or the app
/// exits before the snapshot prints.
pub fn start_trace_diagnostics(
    startup: &Arc<Startup>,
    trace_enabled: bool,
    deadline_env: Option<&str>,
    emit: impl Fn(&str) + Send + 'static,
    version: impl FnOnce() -> Result<String, String> + Send + 'static,
) -> Option<std::thread::JoinHandle<bool>> {
    if !trace_enabled {
        return None;
    }
    match startup.spawn_watchdog(deadline_from(deadline_env), emit, version) {
        Ok(handle) => Some(handle),
        Err(err) => {
            eprintln!("serat: startup watchdog could not start: {err}");
            None
        }
    }
}

/// What "vault usable" means in this build, carried in the trace so a gate report
/// cannot quietly change its mind about it.
///
/// From P1 (ADR-0015 §5, task #141) §3.4's "file tree delivered" precondition is the
/// tree's first screen: the root listing applied and painted. P0's stand-in, the
/// last-open buffer, is still recorded in the trace but no longer a term. The walk, which
/// runs on the vault thread, is reported separately and is not waited on.
pub const USABLE_DEFINITION: &str =
    "painted_and_index_settled_and_tree_first_screen_and_watcher_settled";

/// One cold start, as the budget harness reads it.
///
/// The two keys the harness asserts are first. Everything after them is diagnostic and
/// exists so a number that passes can still be interrogated.
#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    /// t0 to the first composited frame. NFR-PRF-01's 400 ms.
    pub t0_to_window_visible_ms: f64,
    /// The `first-contentful-paint` `startTime` the webview sent as evidence for
    /// `t0_to_window_visible_ms` (F-377). On the browser's navigation-origin clock,
    /// **not** `t0` — never compare this against another field in this struct. Present
    /// for audit only: every trace with a `t0_to_window_visible_ms` has one, because
    /// [`Startup::mark_first_paint`] cannot set the former without the latter.
    pub paint_evidence_ms: Option<f64>,
    /// The second `requestAnimationFrame` timestamp `paint_evidence_ms` was validated
    /// against (task #112). Same navigation-origin clock as `paint_evidence_ms`,
    /// **not** `t0` — never compare this against another field in this struct except
    /// `paint_evidence_ms` itself, which is what `bench/budget`'s
    /// `frame_to_paint_gap_ms()` does (F-403, F-411, F-416: a diagnostic with no
    /// assertion on it, reported because the gap is currently unmeasured, not because
    /// a threshold is known — see F-406). `None` on any trace taken before task #115
    /// wired this through, or on a platform with no Paint Timing API to validate
    /// against in the first place.
    pub raf2_ms: Option<f64>,
    /// t0 to the vault being usable. NFR-PRF-01's 1 s. The latest of the paint, the index
    /// settling, `t0_to_tree_first_screen_ms` and `t0_to_watcher_ms`.
    pub t0_to_vault_usable_ms: f64,
    /// t0 to the first line of `main`: the loader's share. `0.0` when `t0_source` is
    /// `in_process`, by construction.
    pub t0_to_main_ms: f64,
    /// t0 to settings being loaded and `AppState` built — the end of our own
    /// pre-Tauri work on the main thread.
    pub t0_to_state_ready_ms: Option<f64>,
    /// t0 to the Tauri runtime being up and `setup` entered.
    pub t0_to_runtime_ready_ms: Option<f64>,
    /// t0 to our work inside `setup` finishing, just before the window is built.
    pub t0_to_setup_done_ms: Option<f64>,
    /// t0 to the window object existing. The budget job fails a run where
    /// `window_visible - window_created < 5 ms`.
    pub t0_to_window_created_ms: Option<f64>,
    /// t0 to the last-open buffer rendering. Recorded, but not a term of
    /// `t0_to_vault_usable_ms` from task #141.
    pub t0_to_buffer_rendered_ms: f64,
    /// Whether this build renders a tree. Always `true` here: a trace exists only once
    /// the tree's first screen was reported, so the budget harness requires
    /// `t0_to_tree_first_screen_ms` whenever it reads this.
    pub tree: bool,
    /// t0 to the root listing applied and painted (ADR-0015 §3): `t_usable`'s tree term.
    pub t0_to_tree_first_screen_ms: f64,
    /// t0 to the index open check finishing. §3.4's first `t_usable` precondition.
    pub t0_to_index_settled_ms: f64,
    /// t0 to the filesystem watcher settling.
    pub t0_to_watcher_ms: f64,
    /// Whether the watcher registered or failed.
    pub watcher: WatcherOutcome,
    /// t0 to the vault walk finishing. Unbudgeted; off the critical path (§3.4).
    pub t0_to_walk_complete_ms: Option<f64>,
    /// Which clock t0 came from. Must be `harness` for a recorded number.
    pub t0_source: T0Source,
    /// Which build produced it. Must be `release` for a recorded number.
    pub profile: Profile,
    /// The index schema version this build writes.
    pub schema_version: i64,
    /// Files the walk found, so a fast start over an empty directory is visible.
    pub notes_walked: Option<u32>,
    /// The canonical vault root, so the harness can check it opened the one it meant.
    pub vault: Option<String>,
    /// The note the shell opened, or `None` — which means "usable" meant nothing was
    /// rendered, and the number should not be trusted.
    pub last_open: Option<String>,
    /// See [`USABLE_DEFINITION`].
    pub usable_definition: &'static str,
}

impl Trace {
    /// **Code this crate wrote, executing on the main thread, before the first paint.**
    ///
    /// The two intervals bracketed by our own marks, and nothing else:
    ///
    /// * `main` → `state_ready` — argument parsing, the settings TOML read, building
    ///   `AppState`.
    /// * `runtime_ready` → `setup_done` — installing the event sink, opening the
    ///   vault, spawning the index and vault threads, resolving the note to open,
    ///   serialising the boot info.
    ///
    /// It deliberately excludes the loader, `tauri::Builder`'s own construction, and
    /// `WebviewWindowBuilder::build`. Those are real costs and three of them are large,
    /// but none of them is code in this repository, and a figure that mixes them
    /// answers a different question than the one its name implies.
    ///
    /// This exists because the first version of that claim was a subtraction rather
    /// than a measurement — two published intervals added together, one of which was
    /// mostly Tauri's runtime. An aggregate has to name its basis, and the safest way
    /// to make that true is for the basis to be the thing measured.
    #[must_use]
    pub fn serat_main_thread_ms(&self) -> Option<f64> {
        let pre_tauri = self.t0_to_state_ready_ms? - self.t0_to_main_ms;
        let in_setup = self.t0_to_setup_done_ms? - self.t0_to_runtime_ready_ms?;
        Some(pre_tauri + in_setup)
    }

    /// The single line written to [`TRACE_ENV`].
    ///
    /// # Errors
    ///
    /// Propagates a `serde_json` failure, which cannot occur for this type but is not
    /// worth an `expect` in a path that runs on every benchmark iteration.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// A mark the trace needs was never taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceIncomplete {
    /// The missing field's name in the JSON.
    pub missing: &'static str,
    /// What its absence means, for whoever reads the failing job.
    pub because: &'static str,
}

impl std::fmt::Display for TraceIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no {} mark: {}", self.missing, self.because)
    }
}

impl std::error::Error for TraceIncomplete {}

/// A `mark_first_paint` call was refused because it did not carry a positive, finite
/// `first-contentful-paint` `startTime` — the only thing this argument may
/// legitimately hold. F-377: the earlier `mark_first_paint(&self) -> bool` accepted
/// any call unconditionally, so a call taken before the frame was ever composited
/// passed exactly like a real one. No mark is set when this is returned; the IPC
/// command's caller sees the rejection instead of the milliseconds it would otherwise
/// hand back to the webview.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaintEvidenceRejected {
    /// A human-readable account of what was sent instead, built with `{:?}` so `NaN`
    /// and the infinities are legible rather than causing this type to fail to
    /// serialise at the IPC boundary the way a raw non-finite `f64` field would.
    pub reason: String,
}

impl PaintEvidenceRejected {
    fn new(sent: f64) -> Self {
        Self {
            reason: format!(
                "mark_first_paint refused {sent:?} as first-contentful-paint evidence: \
                 it is not a positive, finite timestamp a real PerformancePaintTiming \
                 entry could report"
            ),
        }
    }

    /// Task #112, F-396(c): the caller sent no paint entry at all —
    /// `first_contentful_paint_ms: None`. Distinct from [`PaintEvidenceRejected::new`],
    /// whose message is about an implausible *value*; this is about the complete
    /// absence of one, which a mandatory `f64` parameter used to turn into a raw IPC
    /// deserialisation failure the caller could not tell apart from "the guard was
    /// never reached" (F-396(c)). `None` is legitimate — a platform with no Paint
    /// Timing API has nothing else to send — and the correct outcome is still no
    /// mark, recorded as the reason it is missing rather than as an opaque wire error.
    fn missing_entry() -> Self {
        Self {
            reason: "mark_first_paint refused: no first-contentful-paint entry was sent. \
                      Either the Paint Timing API is unavailable on this platform, or \
                      the call ran before the browser recorded one — both are refusals, \
                      not defaults (F-377)."
                .to_owned(),
        }
    }

    /// Task #112, F-396(b): the two `requestAnimationFrame` timestamps the caller also
    /// sends are not two distinct, increasing frames. F-377's mutation A (the double
    /// rAF collapsed to one) and a fabricated pair computed synchronously in one
    /// callback both produce this shape.
    fn frame_gate_failed(raf1_ms: f64, raf2_ms: f64) -> Self {
        Self {
            reason: format!(
                "mark_first_paint refused: the second requestAnimationFrame timestamp \
                 ({raf2_ms:?}) did not follow the first ({raf1_ms:?}) — that is not two \
                 distinct composited frames"
            ),
        }
    }
}

/// F-406: what [`validate_paint_evidence`] prints when the second requestAnimationFrame
/// timestamp precedes the paint entry by more than its tolerance — a *diagnostic*, not
/// [`PaintEvidenceRejected`]'s reason text, because this shape is no longer refused. See
/// [`validate_paint_evidence`]'s own doc comment for why.
///
/// Returns `Option<String>` rather than always building the message, F-424: a version
/// that always returns `String` and lets the caller decide whether to print it makes
/// the decision "does this shape still get reported" observable only by reading
/// stderr, and deleting the whole `eprintln!` call at the site below left every test
/// green (code-reviewer, by deletion mutation). Returning `None` when the tolerance
/// holds is the thing a test can assert on without capturing output.
fn frame_precedes_paint_diagnostic(raf2_ms: f64, first_contentful_paint_ms: f64) -> Option<String> {
    // Empirically set, not theoretical, and the two are far apart: the first version
    // of this check used 2ms, reasoned from clock-quantisation noise on the
    // assumption that the second rAF callback should land at or after the paint
    // entry. A real WebView2 run rejected every honest launch — measured, over three
    // separate cold starts against the same real binary, raf2 legitimately precedes
    // first-contentful-paint by 51.4, 56.7 and 63.4 ms. WebView2's renderer process
    // and its GPU-compositor "the frame actually displayed" signal are not the same
    // clock tick; rAF fires on the renderer's own schedule, Paint Timing measures the
    // compositor's, and the two are consistently ~50-65ms apart on this platform,
    // not ~2ms. 200ms is set with real margin over that measured range.
    //
    // F-406: with FCP measured at ~184ms on this shell, this condition is
    // arithmetically unreachable as a rejection — it would need `raf2_ms < -16`.
    // Kept as a diagnostic rather than deleted: the arithmetic is a property of
    // *this* shell's current content, not a law, and a future page with more to
    // paint before FCP could make the gap wide enough for this to mean something
    // again. Reporting it costs one `eprintln!` at the call site and keeps the
    // measurement running; deleting it would lose the one signal that could ever
    // tell a future reader the threshold has become meaningful.
    const FRAME_PRECEDES_PAINT_TOLERANCE_MS: f64 = 200.0;
    if raf2_ms + FRAME_PRECEDES_PAINT_TOLERANCE_MS < first_contentful_paint_ms {
        Some(format!(
            "mark_first_paint: the second requestAnimationFrame timestamp ({raf2_ms:?}) \
             precedes the first-contentful-paint entry ({first_contentful_paint_ms:?}) by \
             more than clock-quantisation noise can explain (F-406, reported not refused)"
        ))
    } else {
        None
    }
}

impl std::fmt::Display for PaintEvidenceRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for PaintEvidenceRejected {}

/// Validates a `mark_first_paint` call's full evidence bundle before any of it reaches
/// [`Startup::mark_first_paint`] — task #112, F-396(c). Two things this checks that
/// `Startup::mark_first_paint`'s single `f64` cannot: that a genuinely absent Paint
/// Timing entry is a typed rejection rather than a parameter a mandatory `f64` would
/// have refused to deserialise at all, and that the two `requestAnimationFrame`
/// timestamps the caller also sends are internally consistent with the paint entry
/// they are meant to corroborate.
///
/// F-396(b): `first-contentful-paint` alone is sound only as long as `app/index.html`
/// stays contentless, a property nothing in this crate asserts and that lives in a
/// different file from this guard — a splash screen or loading skeleton, an entirely
/// ordinary future addition, would silently degrade an FCP-only guard to F-377's
/// mutation B with nothing going red.
///
/// **That independence was real when #112 wrote this and is not real now (F-482).**
/// The sentence that stood here claimed checking the frame timestamps *against the
/// paint entry* made this guard's soundness independent of `index.html`'s contents.
/// Read the body: the only rejection is `!raf1.is_finite() || !raf2.is_finite() ||
/// raf2 <= raf1` — the two frame timestamps compared **to each other**, never to the
/// paint entry. The sole check that ever compared them to the paint entry was
/// `frame_precedes_paint`, and **F-406 demoted it to a non-rejecting diagnostic**, so
/// the sentence became false the moment that landed. This guard is FCP-only again,
/// F-396(b)'s dependency on `app/index.html` staying contentless is live again, and
/// nothing asserts that property.
///
/// Stated here rather than fixed here because the fix is a decision, not an edit:
/// re-arming a rejection is ADR-0014's table to change (F-406's demotion was itself
/// sound and correctly pinned — see F-444), and a doc that quietly promised the
/// property was the more expensive half. What #125 closes is the *textual* route
/// (`lib.rs`'s call-site count, F-481); this runtime route stays open and is named as
/// open.
///
/// All three inputs are on the *webview's own* `performance.now()` clock, never this
/// process's `elapsed_ms` — they only ever need to be compared to each other, which is
/// all this function does; nothing here needs cross-clock reconciliation.
///
/// # Errors
///
/// [`PaintEvidenceRejected`], naming which check failed: the entry is absent, or the
/// two frame timestamps are not strictly increasing.
///
/// # F-406 — the frame-precedes-paint check is a diagnostic, not a rejection
///
/// A third check used to live here: refuse when the second requestAnimationFrame
/// timestamp precedes the paint entry by more than a clock-quantisation tolerance
/// (F-388's shape — two honestly-composited frames scheduled before this shell painted
/// anything visible). ADR-0014 pre-committed the fallback for exactly this situation,
/// before the data existed: **a rejection if an honest run showed the ordering holding
/// with room, a reported diagnostic if it did not.**
///
/// F-393 measured `first_contentful_paint_ms` at ~184 ms on this shell. Rejection
/// requires `raf2_ms < first_contentful_paint_ms − 200`, which at that FCP needs
/// `raf2_ms < −16` — impossible on a monotonic clock, for *any* `raf2_ms` a caller
/// could honestly or dishonestly send. The honest measured gap is 51–65 ms and the
/// widest gap the check could ever act on tops out at FCP itself (~184 ms); separating
/// an attack from noise would need a threshold around 65–130 ms, and three samples
/// against F-327's ~35% session-to-session drift cannot establish one. The pre-
/// committed condition did not hold, so per ADR-0014 this became a diagnostic
/// ([`frame_precedes_paint_diagnostic`], printed to stderr): the check still runs and
/// still names what it found, but no longer refuses the call for it. A check that
/// cannot fire is worse than no check, because it reads as coverage — this is the
/// tenth guard-that-cannot-fail on record, and the first found by its own designer's
/// advance criterion rather than by a reviewer's mutation.
pub(crate) fn validate_paint_evidence(
    first_contentful_paint_ms: Option<f64>,
    raf1_ms: f64,
    raf2_ms: f64,
) -> Result<f64, PaintEvidenceRejected> {
    let first_contentful_paint_ms =
        first_contentful_paint_ms.ok_or_else(PaintEvidenceRejected::missing_entry)?;
    // Written as three explicit conditions rather than `!(raf2_ms > raf1_ms)`: `f64` is
    // only partially ordered, and a negated `>` would let a NaN pair slip through
    // (`!(NaN > x)` is true, matching "reject", but `NaN <= x` is false — the two are
    // not equivalent, which is exactly what clippy's `neg_cmp_op_on_partial_ord`
    // flags). Spelling out "finite and strictly increasing" says what is actually
    // required and rejects NaN by name rather than by the coincidence of `!`.
    if !raf1_ms.is_finite() || !raf2_ms.is_finite() || raf2_ms <= raf1_ms {
        return Err(PaintEvidenceRejected::frame_gate_failed(raf1_ms, raf2_ms));
    }
    if let Some(diagnostic) = frame_precedes_paint_diagnostic(raf2_ms, first_contentful_paint_ms) {
        eprintln!("serat: {diagnostic}");
    }
    Ok(first_contentful_paint_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page retries a refusal only when its reason contains this text
    /// (`isFrameGateRefusal` in `app/src/startup.ts`, task #186). Rewording the
    /// message without updating that function would silently turn the retry off.
    #[test]
    fn frame_gate_failed_reason_keeps_the_text_the_page_matches() {
        assert!(
            PaintEvidenceRejected::frame_gate_failed(1.0, 1.0)
                .reason
                .contains("did not follow the first")
        );
    }

    /// A plausible `first-contentful-paint` `startTime` for tests that need the mark
    /// taken but are not exercising [`PaintEvidenceRejected`] itself.
    const FCP_MS: f64 = 42.0;

    /// A plausible second `requestAnimationFrame` timestamp to go with `FCP_MS`, for
    /// the same tests. `Startup::mark_first_paint` does not itself validate the
    /// relationship between the two — that is `validate_paint_evidence`'s job, one
    /// layer up — so any finite value works here.
    const RAF2_MS: f64 = 20.0;

    fn in_process() -> Startup {
        Startup::begin_with(None)
    }

    /// A start that did everything except the mark under test.
    fn complete(startup: &Startup) {
        startup.mark_window_created();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());
    }

    #[test]
    fn the_snapshot_names_every_missing_mark_and_no_set_one() {
        // Task #178: with some marks taken, each name appears on exactly the side it
        // belongs. A snapshot that drops a name (a mutant removing one table row) fails
        // the membership check below.
        let startup = in_process();
        startup.mark_state_ready();
        startup.mark_runtime_ready();
        startup.mark_setup_done();
        startup.mark_window_created();
        let line = startup.marks_snapshot_line();
        assert!(line.starts_with("serat: startup marks after "), "{line}");
        let (_, rest) = line.split_once("set=[").expect("set list");
        let (set, rest) = rest.split_once("] missing=[").expect("missing list");
        let missing = rest.strip_suffix(']').expect("closing bracket");
        let split = |list: &str| -> Vec<String> {
            list.split(", ")
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let (set, missing) = (split(set), split(missing));
        let all = [
            "main",
            "state_ready",
            "runtime_ready",
            "setup_done",
            "window_created",
            "window_visible",
            "buffer_rendered",
            "tree_first_screen",
            "index_settled",
            "watcher",
            "walk_complete",
            "vault_usable",
        ];
        for name in all {
            let in_set = set.iter().any(|n| n == name);
            let in_missing = missing.iter().any(|n| n == name);
            assert!(
                in_set != in_missing,
                "{name} must be on exactly one side: {line}"
            );
        }
        assert_eq!(set.len() + missing.len(), all.len(), "{line}");
        for name in [
            "main",
            "state_ready",
            "runtime_ready",
            "setup_done",
            "window_created",
        ] {
            assert!(set.iter().any(|n| n == name), "{name} is set: {line}");
        }
        for name in [
            "window_visible",
            "tree_first_screen",
            "vault_usable",
            "watcher",
        ] {
            assert!(
                missing.iter().any(|n| n == name),
                "{name} is missing: {line}"
            );
        }
    }

    #[test]
    fn a_fully_marked_start_has_nothing_missing() {
        let startup = in_process();
        startup.mark_state_ready();
        startup.mark_runtime_ready();
        startup.mark_setup_done();
        complete(&startup);
        startup.mark_walk_complete(0);
        let line = startup.marks_snapshot_line();
        assert!(line.ends_with("missing=[]"), "{line}");
    }

    type Lines = Arc<Mutex<Vec<String>>>;

    fn collector() -> (Lines, impl Fn(&str) + Send + 'static) {
        let lines: Lines = Arc::default();
        let sink = Arc::clone(&lines);
        (lines, move |l: &str| {
            sink.lock().unwrap().push(l.to_owned())
        })
    }

    #[test]
    fn the_watchdog_fires_once_when_the_shell_never_paints() {
        let startup = Arc::new(in_process());
        let (lines, emit) = collector();
        let fired = startup
            .spawn_watchdog(Duration::from_millis(30), emit, || Ok("1.2.3".to_owned()))
            .expect("spawn")
            .join()
            .expect("no panic");
        assert!(fired);
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("serat: startup marks after "),
            "{lines:?}"
        );
        assert!(
            lines[0].contains("missing=[") && lines[0].contains("window_visible"),
            "{lines:?}"
        );
        assert_eq!(lines[1], "serat: webview runtime 1.2.3");
    }

    #[test]
    fn the_watchdog_stays_silent_when_the_paint_lands_before_the_deadline() {
        let startup = Arc::new(in_process());
        let (lines, emit) = collector();
        let handle = startup
            .spawn_watchdog(Duration::from_secs(5), emit, || Ok("1.2.3".to_owned()))
            .expect("spawn");
        startup.mark_window_created();
        startup.mark_first_paint(FCP_MS, RAF2_MS).expect("valid");
        assert!(!handle.join().expect("no panic"), "must not fire");
        let lines = lines.lock().unwrap();
        assert_eq!(*lines, ["serat: webview runtime 1.2.3"], "no snapshot line");
    }

    #[test]
    fn diagnostics_spawn_nothing_without_the_trace_env() {
        let startup = Arc::new(in_process());
        let spawned = start_trace_diagnostics(
            &startup,
            false,
            Some("1"),
            |_| panic!("must not emit"),
            || panic!("must not query"),
        );
        assert!(spawned.is_none(), "no thread without the trace env");
        let (_lines, emit) = collector();
        let spawned =
            start_trace_diagnostics(&startup, true, Some("1"), emit, || Ok(String::new()));
        assert!(
            spawned
                .expect("armed under the env")
                .join()
                .expect("no panic")
        );
    }

    #[test]
    fn an_unparsable_deadline_is_the_default_and_never_a_panic() {
        for bad in [
            "",
            "abc",
            "-5",
            "1.5",
            "0",
            " ",
            "99999999999999999999999999",
        ] {
            let got = deadline_from(Some(bad));
            assert!(
                got == DEFAULT_DEADLINE
                    || bad.starts_with("9999") && got == Duration::from_secs(86_400),
                "{bad:?} -> {got:?}"
            );
        }
        assert_eq!(deadline_from(None), DEFAULT_DEADLINE);
        assert_eq!(deadline_from(Some("1500")), Duration::from_millis(1500));
        assert_eq!(deadline_from(Some(" 20 ")), Duration::from_millis(20));
        assert_eq!(
            deadline_from(Some("18446744073709551615")),
            Duration::from_secs(86_400)
        );
    }

    #[test]
    fn the_webview_version_line_says_unknown_on_failure() {
        assert_eq!(
            webview_version_line(Ok("142.0.3595.53".to_owned())),
            "serat: webview runtime 142.0.3595.53"
        );
        assert_eq!(
            webview_version_line(Err("not installed".to_owned())),
            "serat: webview runtime unknown"
        );
        assert_eq!(
            webview_version_line(Ok(String::new())),
            "serat: webview runtime unknown"
        );
    }

    #[test]
    fn a_rejection_is_reported_through_the_sink_with_its_reason() {
        let seen = Mutex::new(Vec::<String>::new());
        let rejected = validate_paint_evidence(None, 1.0, 2.0).expect_err("rejected");
        report_rejection(
            &|l| seen.lock().unwrap().push(l.to_owned()),
            123.4,
            &rejected,
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0],
            format!(
                "serat: paint evidence rejected at 123 ms: {}",
                rejected.reason
            )
        );
    }

    #[test]
    fn creating_the_window_does_not_make_it_visible() {
        // The mutation this test exists to catch: someone stamps window_visible from
        // the window builder because it is easier than plumbing a command through the
        // webview, and 400 ms starts passing over a blank rectangle. Make
        // `mark_window_created` also set `window_visible` and this fails.
        let startup = in_process();
        startup.mark_window_created();
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());

        let err = startup
            .trace()
            .expect_err("a window that never painted has no trace");
        assert_eq!(err.missing, "window_visible");
        assert!(err.because.contains("did not paint"), "{err}");
    }

    #[test]
    fn a_shell_that_paints_but_renders_nothing_has_no_trace_either() {
        let startup = in_process();
        startup.mark_window_created();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);

        let err = startup.trace().expect_err("the buffer never rendered");
        assert_eq!(err.missing, "vault_usable");
        assert!(err.because.contains("buffer"), "{err}");
    }

    #[test]
    fn a_vault_nobody_is_watching_is_not_usable() {
        // Architect's §3.4 check: a vault that is usable but unwatched silently misses
        // every edit made in that window. Delete the watcher term from
        // `Marks::vault_usable` and this fails.
        let startup = in_process();
        startup.mark_window_created();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());

        let err = startup.trace().expect_err("the watcher never settled");
        assert_eq!(err.missing, "vault_usable");
        assert!(err.because.contains("unwatched"), "{err}");

        assert!(
            !startup.wait_for_usable(Duration::from_millis(30)),
            "waiting for usable returned before the watcher settled"
        );

        startup.mark_watcher(WatcherOutcome::Registered);
        assert!(startup.trace().is_ok());
    }

    #[test]
    fn a_shell_is_not_usable_before_it_has_painted() {
        // The race this shell actually hit on its first run, kept as a test because it
        // cost a trace: both marks are IPC round trips from the webview, the buffer
        // read finished first, `--exit-after-ready` saw "usable", and the process
        // exited with the paint mark still in flight — producing no trace at all.
        //
        // Delete the `window_visible` term from `Marks::vault_usable` and this fails.
        let startup = in_process();
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());
        assert!(
            !startup.wait_for_usable(Duration::from_millis(30)),
            "a shell that has not painted was reported usable"
        );

        std::thread::sleep(Duration::from_millis(3));
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        let trace = startup.trace().expect("complete");
        assert!(
            (trace.t0_to_vault_usable_ms - trace.t0_to_window_visible_ms).abs() < f64::EPSILON,
            "usable {} should be the paint mark {}, which was the last of the three",
            trace.t0_to_vault_usable_ms,
            trace.t0_to_window_visible_ms
        );
    }

    #[test]
    fn a_vault_whose_index_state_is_unknown_is_not_usable() {
        // §3.4's first `t_usable` precondition. Like the watcher clause, leaving it out
        // fails invisibly: nothing errors, the number is simply better than the
        // requirement it claims to measure. Delete the `index_settled` term from
        // `Marks::vault_usable` and this fails.
        let startup = in_process();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());

        let err = startup.trace().expect_err("the open check never finished");
        assert_eq!(err.missing, "vault_usable");
        assert!(err.because.contains("index"), "{err}");
        assert!(
            !startup.wait_for_usable(Duration::from_millis(30)),
            "usable was reported while the index state was still unknown"
        );

        startup.mark_index_settled();
        assert!(startup.trace().is_ok());
    }

    #[test]
    fn a_rebuilding_or_unavailable_index_settles_the_mark_as_much_as_a_ready_one() {
        // The requirement is that the state is *known*, not that it is good. A shell
        // that waited for `Ready` would hang for the length of a rebuild on exactly the
        // vault where the window most needs to be up — and P0 never reaches `Ready` at
        // all, so it would never report usable.
        let startup = in_process();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());
        startup.mark_index_settled();
        assert!(startup.trace().is_ok());
    }

    #[test]
    fn usable_is_the_later_of_the_buffer_and_the_watcher() {
        // Both orders, because taking the first rather than the later one would pass a
        // test that only ever exercised one of them.
        let watcher_last = in_process();
        watcher_last
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        watcher_last.mark_index_settled();
        std::thread::sleep(Duration::from_millis(2));
        watcher_last.mark_buffer_rendered();
        watcher_last.mark_tree_first_screen(watcher_last.elapsed_ms());
        std::thread::sleep(Duration::from_millis(5));
        watcher_last.mark_watcher(WatcherOutcome::Registered);
        let trace = watcher_last.trace().expect("complete");
        assert!(
            (trace.t0_to_vault_usable_ms - trace.t0_to_watcher_ms).abs() < f64::EPSILON,
            "usable {} should be the watcher mark {}",
            trace.t0_to_vault_usable_ms,
            trace.t0_to_watcher_ms
        );

        let tree_last = in_process();
        tree_last
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        tree_last.mark_index_settled();
        tree_last.mark_buffer_rendered();
        std::thread::sleep(Duration::from_millis(2));
        tree_last.mark_watcher(WatcherOutcome::Registered);
        std::thread::sleep(Duration::from_millis(5));
        tree_last.mark_tree_first_screen(tree_last.elapsed_ms());
        let trace = tree_last.trace().expect("complete");
        assert!(
            (trace.t0_to_vault_usable_ms - trace.t0_to_tree_first_screen_ms).abs() < f64::EPSILON,
            "usable {} should be the tree mark {}",
            trace.t0_to_vault_usable_ms,
            trace.t0_to_tree_first_screen_ms
        );
        assert!(trace.tree, "a trace that exists declares a tree");
    }

    #[test]
    fn the_window_build_line_names_the_phase_and_the_data_folder() {
        let line = window_build_line("entering", 12.4, Some("C:\\wv"));
        assert_eq!(
            line,
            "serat: main window build entering at 12 ms (webview data folder: C:\\wv)"
        );
        assert!(window_build_line("returned", 1.0, None).ends_with("folder: default)"));
    }

    #[test]
    fn the_benchmark_exit_waits_for_the_buffer_mark_when_the_paint_lands_last() {
        // Task #192 (runs 431/432): after a frame-gate retry the paint is the last term
        // of usable, and the buffer mark follows it by one IPC round trip. Waiting on
        // `wait_for_usable` alone read the trace in that gap and exited 2 at ~0.5 s.
        // Make `wait_for_trace_terms` return on usable alone and this fails.
        let startup = Arc::new(in_process());
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_tree_first_screen(startup.elapsed_ms());
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        assert!(startup.wait_for_usable(Duration::from_millis(30)));
        assert!(
            startup.trace().is_err(),
            "usable is complete but the trace is not: the buffer mark is in flight"
        );
        assert!(
            !startup.wait_for_trace_terms(Duration::from_millis(30)),
            "the exit thread would read the trace before the buffer mark"
        );
        let late = Arc::clone(&startup);
        let sender = std::thread::spawn(move || {
            late.mark_buffer_rendered();
        });
        assert!(startup.wait_for_trace_terms(Duration::from_secs(5)));
        startup.trace().expect("complete once the buffer arrived");
        sender.join().expect("sender");
    }

    #[test]
    fn a_vault_with_no_tree_on_screen_is_not_usable_and_the_buffer_is_not_a_term() {
        // ADR-0015 section 5 (task #141): the tree's first screen replaced the buffer as
        // `t_usable`'s tree term. Put `buffer_rendered` back into `Marks::vault_usable`
        // in place of `tree_first_screen` and the first two assertions fail; leave it in
        // as well and the last one fails.
        let startup = in_process();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        let err = startup.trace().expect_err("no tree was reported");
        assert_eq!(err.missing, "vault_usable");
        assert!(err.because.contains("tree"), "{err}");
        assert!(
            !startup.wait_for_usable(Duration::from_millis(30)),
            "usable was reported with no tree on screen"
        );

        let early = startup.elapsed_ms();
        assert!(startup.mark_tree_first_screen(early));
        assert!(!startup.mark_tree_first_screen(early + 1.0), "set once");
        // A buffer that renders after everything else does not move usable.
        let late = in_process();
        late.mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        late.mark_index_settled();
        late.mark_watcher(WatcherOutcome::Registered);
        late.mark_tree_first_screen(late.elapsed_ms());
        assert!(late.wait_for_usable(Duration::from_secs(1)));
        std::thread::sleep(Duration::from_millis(5));
        late.mark_buffer_rendered();
        let trace = late.trace().expect("complete");
        assert!(
            trace.t0_to_vault_usable_ms < trace.t0_to_buffer_rendered_ms,
            "usable {} was moved by the buffer mark {}",
            trace.t0_to_vault_usable_ms,
            trace.t0_to_buffer_rendered_ms
        );
        let line = trace.to_line().expect("json");
        assert!(line.contains("\"tree\":true"), "{line}");
        assert!(line.contains("\"t0_to_tree_first_screen_ms\":"), "{line}");
        assert_eq!(trace.usable_definition, USABLE_DEFINITION);
        assert!(USABLE_DEFINITION.contains("tree_first_screen"));
        assert!(!USABLE_DEFINITION.contains("buffer_rendered"));
    }

    #[test]
    fn a_watcher_that_failed_still_settles_and_is_recorded_as_failed() {
        // Design goal 5: a vault with no watcher is stale, not broken. But a number
        // taken in that state must say so.
        let startup = in_process();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Failed);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());
        let trace = startup.trace().expect("complete");
        assert_eq!(trace.watcher, WatcherOutcome::Failed);
        assert!(
            trace
                .to_line()
                .expect("json")
                .contains("\"watcher\":\"failed\""),
            "the failure must be visible in the artifact"
        );
    }

    #[test]
    fn the_paint_mark_is_strictly_after_the_creation_mark() {
        // The property the budget job asserts on its side, with a 5 ms floor. Asserted
        // here too, because a harness assertion over a value the application computes
        // wrongly is not an independent check.
        let startup = in_process();
        startup.mark_window_created();
        std::thread::sleep(Duration::from_millis(6));
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());

        let trace = startup.trace().expect("complete");
        let created = trace.t0_to_window_created_ms.expect("created was marked");
        assert!(
            trace.t0_to_window_visible_ms - created >= 5.0,
            "visible {} is not 5 ms after created {created}",
            trace.t0_to_window_visible_ms
        );
    }

    #[test]
    fn serat_main_thread_work_is_measured_rather_than_subtracted() {
        // The figure "Serat's own main-thread work" was first published as a
        // subtraction -- two of the reported intervals added together, one of which was
        // mostly Tauri's runtime rather than ours. This asserts the shape that makes it
        // a measurement: two intervals bracketed by our own marks, excluding the loader,
        // Tauri's builder and the webview build.
        let startup = in_process();
        startup.mark_state_ready();
        startup.mark_runtime_ready();
        std::thread::sleep(Duration::from_millis(5));
        startup.mark_setup_done();
        startup.mark_window_created();
        startup
            .mark_first_paint(FCP_MS, RAF2_MS)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        startup.mark_tree_first_screen(startup.elapsed_ms());

        let trace = startup.trace().expect("complete");
        let ours = trace.serat_main_thread_ms().expect("both brackets marked");
        let setup_gap = trace.t0_to_setup_done_ms.expect("setup")
            - trace.t0_to_runtime_ready_ms.expect("runtime");

        assert!(ours >= 5.0, "the 5 ms slept inside setup is ours: {ours}");
        assert!(
            ours >= setup_gap,
            "the setup interval alone cannot exceed the total: {ours} < {setup_gap}"
        );
        // And it must not have swallowed the phases that are not ours. The window
        // build is between setup_done and window_created; nothing here may include it.
        assert!(
            ours < trace.t0_to_window_visible_ms,
            "our share {ours} cannot be the whole pre-paint phase {}",
            trace.t0_to_window_visible_ms
        );
    }

    #[test]
    fn a_trace_missing_any_one_bracket_reports_no_share_rather_than_a_wrong_one() {
        // The failure mode being prevented: a partially marked run yielding a number
        // that looks like a measurement. `None` is the honest answer.
        //
        // Each of the three `?` sites is exercised on its own. The previous version of
        // this test marked *none* of them, so it proved only "all three missing gives
        // None" while the docstring claims "either bracket missing" -- and each `?`
        // could be deleted individually with the other two still returning None. That
        // is P0 finding F-148, and it is the fourth time this figure has needed
        // correcting, so the per-term case is exactly where the fifth would go wrong.
        let whole = |trace: &Trace| trace.serat_main_thread_ms();

        let complete_trace = Trace {
            t0_to_window_visible_ms: 800.0,
            paint_evidence_ms: Some(798.0),
            raf2_ms: Some(750.0),
            t0_to_vault_usable_ms: 805.0,
            t0_to_main_ms: 60.0,
            t0_to_state_ready_ms: Some(65.0),
            t0_to_runtime_ready_ms: Some(90.0),
            t0_to_setup_done_ms: Some(90.5),
            t0_to_window_created_ms: Some(700.0),
            t0_to_buffer_rendered_ms: 805.0,
            tree: true,
            t0_to_tree_first_screen_ms: 805.0,
            t0_to_index_settled_ms: 91.0,
            t0_to_watcher_ms: 92.0,
            watcher: WatcherOutcome::Registered,
            t0_to_walk_complete_ms: None,
            t0_source: T0Source::Harness,
            profile: Profile::current(),
            schema_version: 1,
            notes_walked: None,
            vault: None,
            last_open: None,
            usable_definition: USABLE_DEFINITION,
        };

        // The control: with every bracket present it is a number, and the right one.
        // Without this, all three cases below would pass against a function that always
        // returned None.
        let share = whole(&complete_trace).expect("every bracket is marked");
        assert!(
            (share - 5.5).abs() < 1e-9,
            "expected (65-60) + (90.5-90) = 5.5, got {share}"
        );

        // One case per `?`. Delete any single one of them from
        // `serat_main_thread_ms` and exactly one of these fails.
        /// Clears one of the three brackets. Named because the tuple type is long
        /// enough that clippy is right to ask.
        type ClearBracket = fn(&mut Trace);

        let cases: [(&str, ClearBracket); 3] = [
            ("t0_to_state_ready_ms", |t| t.t0_to_state_ready_ms = None),
            ("t0_to_runtime_ready_ms", |t| {
                t.t0_to_runtime_ready_ms = None
            }),
            ("t0_to_setup_done_ms", |t| t.t0_to_setup_done_ms = None),
        ];
        for (field, clear) in cases {
            let mut trace = complete_trace.clone();
            clear(&mut trace);
            assert!(
                whole(&trace).is_none(),
                "a share was reported with {field} unmarked; the other two brackets \
                 cannot stand in for it"
            );
        }
    }

    #[test]
    fn every_mark_is_monotonic() {
        // test-engineer fails a run whose marks are out of order, on the grounds that a
        // sample which survived a clock step is not a sample. Here it is a property of
        // the clock rather than a hope: everything after `main` comes from one
        // `Instant`.
        let startup = in_process();
        complete(&startup);
        startup.mark_walk_complete(3);
        let trace = startup.trace().expect("complete");

        let created = trace.t0_to_window_created_ms.expect("created");
        assert!(trace.t0_to_main_ms <= created, "{trace:?}");
        assert!(created <= trace.t0_to_window_visible_ms, "{trace:?}");
        assert!(
            trace.t0_to_window_visible_ms <= trace.t0_to_vault_usable_ms,
            "{trace:?}"
        );
    }

    #[test]
    fn a_mark_is_taken_once_and_a_repeat_moves_nothing() {
        let startup = in_process();
        assert!(
            startup
                .mark_first_paint(FCP_MS, RAF2_MS)
                .expect("valid paint evidence")
        );
        let first = startup.lock().window_visible.expect("marked");

        std::thread::sleep(Duration::from_millis(2));
        assert!(
            !startup
                .mark_first_paint(FCP_MS, RAF2_MS)
                .expect("valid paint evidence"),
            "a second paint must be refused"
        );
        assert!(
            (startup.lock().window_visible.expect("still marked") - first).abs() < f64::EPSILON,
            "a webview reload moved a mark that had already been taken"
        );
    }

    // F-377: `the_placeholder_bundle_exists_and_carries_the_two_marks`
    // (`crates/serat-app/src/lib.rs`) asserted only that `mark_first_paint`,
    // `mark_vault_usable` and `requestAnimationFrame` appear *somewhere* in
    // `boot.js`, and two mutations survived `cargo test -p serat-app` against it:
    //
    //   A. the double `requestAnimationFrame` collapsed to a single one, taking the
    //      mark before the frame reached the compositor;
    //   B. `invoke("mark_first_paint")` hoisted out of both callbacks to script start,
    //      with a no-op `requestAnimationFrame(() => {})` left behind to satisfy the
    //      substring.
    //
    // The fix replaces frame-counting with evidence: the argument must be the
    // `startTime` of a real `first-contentful-paint` `PerformancePaintTiming` entry,
    // which the Paint Timing API does not report before the frame is composited. Under
    // either mutation reapplied to that mechanism, the script has nothing genuine at
    // its call site — `performance.getEntriesByType("paint")` is still empty, so the
    // value it can send is `undefined` (crossing as a missing/non-finite argument) or,
    // if a developer defensively coalesces the missing entry to a sentinel, `0` or
    // less. The two tests below are exactly those two shapes, asserted red at the
    // point the old test could not reach: `Startup::mark_first_paint` itself, not
    // `boot.js`'s text. (The literal mutations still need a mutated `boot.js` run
    // against the real binary to confirm end to end — this crate has no JS runtime to
    // automate that — but the failure mode they both reduce to is asserted here on
    // every `cargo test -p serat-app`.)
    #[test]
    fn f_377_mutation_a_shape_a_call_with_nothing_composited_yet_is_rejected() {
        // What `undefined?.startTime` becomes once it crosses the IPC boundary as a
        // mandatory `f64`: not a number at all.
        let startup = in_process();
        for bogus in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = startup
                .mark_first_paint(bogus, RAF2_MS)
                .expect_err("a non-finite value is not evidence of a real paint");
            assert!(
                err.reason.contains("first-contentful-paint"),
                "the rejection must say what it refused: {err}"
            );
        }
        assert!(
            startup.trace().is_err(),
            "a rejected call must leave window_visible unset, not stamp a wrong one"
        );
    }

    #[test]
    fn f_377_mutation_b_shape_a_defaulted_sentinel_is_rejected() {
        // What a script defensively coalescing a missing paint-timing entry to a
        // placeholder looks like: zero or negative, never a real startTime.
        let startup = in_process();
        for bogus in [0.0, -1.0, -0.001] {
            startup
                .mark_first_paint(bogus, RAF2_MS)
                .expect_err("zero or negative is not evidence of a real paint");
        }
        assert!(startup.trace().is_err());
    }

    #[test]
    fn genuine_paint_evidence_is_accepted_and_kept_verbatim_for_audit() {
        let startup = in_process();
        assert!(
            startup
                .mark_first_paint(517.25, 488.5)
                .expect("a positive, finite value is evidence")
        );
        let marks = startup.lock();
        assert_eq!(
            marks.paint_evidence_ms,
            Some(517.25),
            "the raw evidence is kept so a trace can be audited against it"
        );
        // Task #115: raf2_ms is kept verbatim too, and distinctly from
        // paint_evidence_ms -- a version that stored one value for both, or dropped
        // raf2_ms on the floor, would not be caught by asserting only the first.
        assert_eq!(
            marks.raf2_ms,
            Some(488.5),
            "raf2_ms must be kept so bench/budget's frame_to_paint_gap_ms can read it"
        );
    }

    // Task #112, F-396(c) and F-396(b): `validate_paint_evidence` is the layer above
    // `Startup::mark_first_paint` that turns a missing paint entry into a typed
    // rejection instead of a raw IPC deserialisation failure, and checks the two
    // requestAnimationFrame timestamps against the paint entry rather than trusting
    // it alone — F-396(b)'s "conjunction is robust" conclusion, against the mutation
    // vue-frontend-dev's own argument describes: two real, honestly-scheduled frames,
    // just too early, before the shell painted anything.

    #[test]
    fn a_genuinely_absent_paint_entry_is_a_typed_rejection_not_a_deserialise_failure() {
        let err = validate_paint_evidence(None, 10.0, 20.0)
            .expect_err("None must be refused, not defaulted");
        assert!(
            err.reason.contains("first-contentful-paint"),
            "the rejection must say what was missing: {err}"
        );
    }

    #[test]
    fn a_collapsed_or_fabricated_frame_pair_fails_the_gate() {
        for (raf1_ms, raf2_ms) in [(20.0, 20.0), (20.0, 10.0)] {
            validate_paint_evidence(Some(517.25), raf1_ms, raf2_ms)
                .expect_err("raf2 must strictly follow raf1");
        }
    }

    #[test]
    fn two_real_frames_scheduled_before_the_paint_entry_are_a_diagnostic_not_a_rejection() {
        // F-388's original argument, unaffected by F-396(a)'s correction of F-388's
        // own reasoning: two honestly-composited frames, honestly reported, just
        // scheduled before this shell painted anything visible. The gap here (480ms)
        // is well past the measured honest range (51-63ms) and past the 200ms
        // tolerance — the exact shape the frame-precedes-paint check was written to
        // catch.
        //
        // F-406: it no longer can, on this shell. FCP measures ~184ms in practice, and
        // rejecting this shape needs `raf2_ms < first_contentful_paint_ms - 200`, which
        // at that FCP needs a negative `raf2_ms` -- impossible on a monotonic clock,
        // for any input. ADR-0014's pre-committed fallback applies: a diagnostic, not
        // a rejection, is what this shape gets now. The evidence is still accepted --
        // the call is still `mark_first_paint`'s -- because refusing it would be
        // rejecting on a check this project's own arithmetic shows cannot mean
        // anything at these numbers.
        let evidence = 500.0;
        assert_eq!(
            validate_paint_evidence(Some(evidence), 10.0, 20.0),
            Ok(evidence),
            "F-406: this shape is reported, not refused, so the call must still succeed"
        );
    }

    #[test]
    fn frame_precedes_paint_diagnostic_pins_the_threshold_and_that_it_still_runs() {
        // F-424: deleting the whole diagnostic (the `eprintln!` call in
        // validate_paint_evidence, or this function itself) left the workspace green
        // -- nothing asserted the check still ran or still named what it found. These
        // two cases pin the decision as a return value rather than as an observed
        // side effect, so removing either arm goes red without stderr capture.
        //
        // Some(_): the 480ms shape the test above already uses, past the 200ms
        // tolerance -- both values must be legible in the message.
        let diagnostic = frame_precedes_paint_diagnostic(20.0, 500.0)
            .expect("a gap past the tolerance must still produce a diagnostic");
        assert!(diagnostic.contains("20.0"), "{diagnostic}");
        assert!(diagnostic.contains("500.0"), "{diagnostic}");

        // None: the measured honest range (task #112, F-406) -- raf2 precedes FCP by
        // tens of milliseconds, well inside the 200ms tolerance, so no diagnostic.
        assert_eq!(
            frame_precedes_paint_diagnostic(170.0, 184.0),
            None,
            "the honest measured gap must not produce a diagnostic"
        );
    }

    #[test]
    fn a_frame_within_tolerance_of_the_paint_entry_is_accepted() {
        // The paint entry itself, and a frame timestamp equal to it, is the boundary
        // case: FCP is reported for the frame it painted in, so the confirming frame
        // arriving at or after it — never strictly before — is the legitimate shape.
        assert_eq!(
            validate_paint_evidence(Some(184.0), 170.0, 184.0),
            Ok(184.0)
        );
        // A frame a hair after the entry, inside the clock-quantisation tolerance.
        assert_eq!(
            validate_paint_evidence(Some(184.0), 170.0, 185.5),
            Ok(184.0)
        );
        // The measured honest shape (task #112): raf2 legitimately precedes the
        // paint entry by tens of milliseconds on WebView2 — one of the three real
        // gaps this file's tolerance is set against (see
        // FRAME_PRECEDES_PAINT_TOLERANCE_MS's comment), not a boundary value chosen
        // to make the test pass.
        assert_eq!(
            validate_paint_evidence(Some(264.0), 150.0, 200.6),
            Ok(264.0)
        );
    }

    #[test]
    fn genuine_evidence_survives_the_full_validation_and_marks_the_shell() {
        let startup = in_process();
        let raf2_ms = 184.0;
        let evidence =
            validate_paint_evidence(Some(184.0), 170.0, raf2_ms).expect("valid evidence");
        assert!(
            startup
                .mark_first_paint(evidence, raf2_ms)
                .expect("accepted")
        );
        let marks = startup.lock();
        assert_eq!(marks.paint_evidence_ms, Some(184.0));
        assert_eq!(marks.raf2_ms, Some(raf2_ms));
    }

    #[test]
    fn the_trace_says_which_clock_and_which_build_it_used() {
        let startup = in_process();
        complete(&startup);
        let line = startup.trace().expect("complete").to_line().expect("json");
        assert!(line.contains("\"t0_source\":\"in_process\""), "{line}");
        assert!(
            line.contains("\"profile\":\"debug\"") || line.contains("\"profile\":\"release\""),
            "{line}"
        );
        assert!(!line.contains('\n'), "the trace is one line: {line}");
    }

    #[test]
    fn a_harness_t0_puts_the_pre_main_cost_inside_every_mark() {
        let t0 = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_nanos()
            - 120_000_000; // 120 ms before now
        let startup = Startup::begin_with(Some(&t0.to_string()));
        complete(&startup);

        let trace = startup.trace().expect("complete");
        assert_eq!(trace.t0_source, T0Source::Harness);
        assert!(
            trace.t0_to_main_ms >= 120.0,
            "the loader's share was lost: {}",
            trace.t0_to_main_ms
        );
        assert!(
            trace.t0_to_window_visible_ms >= trace.t0_to_main_ms,
            "a mark landed before main did"
        );
    }

    #[test]
    fn a_malformed_t0_falls_back_rather_than_failing_the_start() {
        // A bad environment variable must not stop the application from starting, and
        // the fallback has to be visible in the artifact rather than assumed.
        for bad in [
            "",
            "   ",
            "not-a-number",
            "-1",
            "99999999999999999999999999999999",
        ] {
            let startup = Startup::begin_with(Some(bad));
            assert_eq!(
                startup.t0_source(),
                T0Source::InProcess,
                "{bad:?} was accepted as a t0"
            );
        }
    }

    #[test]
    fn waiting_for_usable_times_out_rather_than_hanging() {
        let startup = in_process();
        assert!(
            !startup.wait_for_usable(Duration::from_millis(30)),
            "waited on a mark that was never taken and claimed it arrived"
        );

        let startup = std::sync::Arc::new(in_process());
        let signaller = std::sync::Arc::clone(&startup);
        std::thread::spawn(move || {
            signaller.mark_watcher(WatcherOutcome::Registered);
            signaller.mark_index_settled();
            signaller
                .mark_first_paint(FCP_MS, RAF2_MS)
                .expect("valid paint evidence");
            std::thread::sleep(Duration::from_millis(10));
            signaller.mark_tree_first_screen(signaller.elapsed_ms());
        });
        assert!(startup.wait_for_usable(Duration::from_secs(5)));
    }

    #[test]
    fn waiting_for_the_walk_returns_when_it_lands_and_times_out_when_it_does_not() {
        // F-151's mark. The trace does not require it -- see the next test -- so the
        // wait has to be the thing that makes it present, and it has to give up rather
        // than hang on a vault whose walk failed.
        let startup = in_process();
        assert!(
            !startup.wait_for_walk_complete(Duration::from_millis(30)),
            "waited on a walk that never completed and claimed it did"
        );

        let startup = std::sync::Arc::new(in_process());
        let walker = std::sync::Arc::clone(&startup);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            walker.mark_walk_complete(20_200);
        });
        assert!(startup.wait_for_walk_complete(Duration::from_secs(5)));
    }

    #[test]
    fn the_walk_is_reported_but_is_not_a_mark_the_trace_requires() {
        // §3.4 puts the walk off the critical path. A trace that refused to exist
        // without it would have quietly put it back on.
        let startup = in_process();
        complete(&startup);
        let trace = startup.trace().expect("complete without a walk");
        assert!(trace.t0_to_walk_complete_ms.is_none());
        assert!(trace.notes_walked.is_none());

        startup.mark_walk_complete(20_200);
        startup.set_vault(Some("D:/bench/vault-20k".to_owned()));
        let trace = startup.trace().expect("complete");
        assert_eq!(trace.notes_walked, Some(20_200));
        assert!(trace.t0_to_walk_complete_ms.is_some());
        assert_eq!(trace.vault.as_deref(), Some("D:/bench/vault-20k"));
    }
}
