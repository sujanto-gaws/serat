//! The background `PRAGMA quick_check` after an unclean shutdown (task #69; NFR-REL-01,
//! NFR-REL-02, C-02, ADR-0002, ADR-0011).
//!
//! The open compares mtimes whatever the last shutdown was (ADR-0015 §2); the unclean marker
//! only schedules this check, which SQLite does itself and which can find a damaged page the
//! open-time checks do not read. A `Problems` verdict is ADR-0002's one answer: rebuild.
//!
//! # When it runs
//!
//! * **Once per unclean open**, from the open's catch-up callback, on a thread of its own.
//! * **After the catch-up**, and only once the index writer has been idle for
//!   [`IDLE_BEFORE_CHECK`] (2 s): [`Activity`] is touched by every save and every
//!   reconciliation. It is never on the save path (ADR-0011): a save that lands while the
//!   check waits postpones it, and one that lands in the instant between the decision and the
//!   read is covered by the re-check just before it.
//! * **Only while the index is `Ready` and the handle slot holds its handle**, so no rebuild
//!   is under way in this process. A rebuild that is requested while a check runs waits for it
//!   ([`QuickCheckRunner::exclusive`]): a check that opened the old inode and then reached
//!   for the new `-shm` by path could win the lock in-process and truncate the writer's
//!   mapping (the #153 SIGBUS).
//! * **Spaced**: two checks are at least [`MIN_SPACING`] apart. The check is one read
//!   transaction of about a second, which holds back one `PASSIVE` checkpoint cycle; running
//!   two back to back would hold back two.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serat_core::IndexError;
use serat_index::QuickCheck;

/// How long the index writer must have been idle before a check starts.
pub const IDLE_BEFORE_CHECK: Duration = Duration::from_secs(2);

/// The least time between the end of one check and the start of the next.
pub const MIN_SPACING: Duration = Duration::from_secs(2);

/// How often the wait looks at the clock.
pub const POLL: Duration = Duration::from_millis(200);

/// When the index writer was last given work: a save, a reconciliation, a catch-up.
#[derive(Debug)]
pub struct Activity {
    origin: Instant,
    last_ms: AtomicU64,
}

impl Default for Activity {
    fn default() -> Self {
        Self::new()
    }
}

impl Activity {
    /// Idle since now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    /// The writer was just given work.
    pub fn touch(&self) {
        let now = u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_ms.fetch_max(now, Ordering::Relaxed);
    }

    /// How long since the last [`Activity::touch`] (or since creation).
    #[must_use]
    pub fn idle_for(&self) -> Duration {
        self.origin
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// What [`QuickCheckRunner::run_when_idle`] did.
#[derive(Debug)]
pub enum Ran {
    /// It did not run, and why.
    Skipped(&'static str),
    /// It ran.
    Checked(Result<QuickCheck, IndexError>),
}

/// One per open vault: serialises checks against rebuilds and spaces them.
#[derive(Debug)]
pub struct QuickCheckRunner {
    activity: std::sync::Arc<Activity>,
    running: Mutex<()>,
    last_end: Mutex<Option<Instant>>,
    runs: AtomicU64,
}

impl QuickCheckRunner {
    /// A runner watching `activity`.
    #[must_use]
    pub fn new(activity: std::sync::Arc<Activity>) -> Self {
        Self {
            activity,
            running: Mutex::new(()),
            last_end: Mutex::new(None),
            runs: AtomicU64::new(0),
        }
    }

    /// How many checks have run (not skipped).
    #[must_use]
    pub fn runs(&self) -> u64 {
        self.runs.load(Ordering::Relaxed)
    }

    /// Held by a rebuild for its whole duration: it waits for a check that is running, and
    /// a check that wants to start waits for it.
    pub fn exclusive(&self) -> MutexGuard<'_, ()> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait until the writer has been idle for `idle`, then run `check` once, provided
    /// `eligible` still says the index is `Ready` with its handle held and the last check
    /// ended at least `spacing` ago. Returns without running if `stopped` says the vault is
    /// closing.
    pub fn run_when_idle(
        &self,
        idle: Duration,
        spacing: Duration,
        poll: Duration,
        stopped: &dyn Fn() -> bool,
        eligible: &dyn Fn() -> bool,
        check: &dyn Fn() -> Result<QuickCheck, IndexError>,
    ) -> Ran {
        loop {
            if stopped() {
                return Ran::Skipped("the vault is closing");
            }
            if self.activity.idle_for() >= idle {
                let guard = self.exclusive();
                let spaced = self
                    .last_end
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some_and(|ended| ended.elapsed() < spacing);
                if spaced {
                    return Ran::Skipped("a check ended less than the spacing ago");
                }
                if stopped() {
                    return Ran::Skipped("the vault is closing");
                }
                if !eligible() {
                    return Ran::Skipped("the index is not Ready with its handle held");
                }
                // A save that landed while the lock was being taken: wait for quiet again.
                if self.activity.idle_for() >= idle {
                    self.runs.fetch_add(1, Ordering::Relaxed);
                    let verdict = check();
                    *self.last_end.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Instant::now());
                    return Ran::Checked(verdict);
                }
                drop(guard);
            }
            std::thread::sleep(poll.min(idle.max(Duration::from_millis(1))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn runner() -> (Arc<Activity>, Arc<QuickCheckRunner>) {
        let activity = Arc::new(Activity::new());
        (
            Arc::clone(&activity),
            Arc::new(QuickCheckRunner::new(activity)),
        )
    }

    const FAST: Duration = Duration::from_millis(5);

    fn clean() -> Result<QuickCheck, IndexError> {
        Ok(QuickCheck::Clean)
    }

    // Mutant, run by hand: dropping the idle comparison lets the check start at once, and the
    // gap asserted below is negative.
    #[test]
    fn it_waits_for_the_writer_to_be_idle_and_never_starts_inside_a_save() {
        let (activity, runner) = runner();
        let busy = Arc::new(AtomicBool::new(true));
        let toucher = {
            let (activity, busy) = (Arc::clone(&activity), Arc::clone(&busy));
            std::thread::spawn(move || {
                // Saves for 600 ms, then quiet.
                let until = Instant::now() + Duration::from_millis(600);
                while Instant::now() < until {
                    activity.touch();
                    std::thread::sleep(Duration::from_millis(20));
                }
                busy.store(false, Ordering::SeqCst);
            })
        };
        let started_at = Arc::new(Mutex::new(None));
        let record = Arc::clone(&started_at);
        let probe = Arc::clone(&activity);
        let ran = runner.run_when_idle(
            Duration::from_millis(300),
            MIN_SPACING,
            FAST,
            &|| false,
            &|| true,
            &move || {
                *record.lock().expect("record") = Some(probe.idle_for());
                clean()
            },
        );
        toucher.join().expect("toucher");
        assert!(
            matches!(ran, Ran::Checked(Ok(QuickCheck::Clean))),
            "{ran:?}"
        );
        let idle_at_start = started_at.lock().expect("record").expect("it ran");
        assert!(
            idle_at_start >= Duration::from_millis(300),
            "the check started {idle_at_start:?} after the last save, inside the idle window"
        );
        assert!(!busy.load(Ordering::SeqCst));
    }

    #[test]
    fn two_triggers_inside_the_spacing_run_once() {
        let (_activity, runner) = runner();
        let runs = AtomicU64::new(0);
        let check = || {
            runs.fetch_add(1, Ordering::SeqCst);
            clean()
        };
        let first = runner.run_when_idle(
            Duration::ZERO,
            Duration::from_secs(2),
            FAST,
            &|| false,
            &|| true,
            &check,
        );
        let second = runner.run_when_idle(
            Duration::ZERO,
            Duration::from_secs(2),
            FAST,
            &|| false,
            &|| true,
            &check,
        );
        assert!(matches!(first, Ran::Checked(_)));
        assert!(matches!(second, Ran::Skipped(_)), "{second:?}");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn it_does_not_run_unless_the_index_is_ready_with_its_handle() {
        let (_activity, runner) = runner();
        let ran = runner.run_when_idle(
            Duration::ZERO,
            MIN_SPACING,
            FAST,
            &|| false,
            &|| false,
            &|| panic!("the check ran against an index that is not Ready"),
        );
        assert!(matches!(ran, Ran::Skipped(_)), "{ran:?}");
        let ran = runner.run_when_idle(
            Duration::from_secs(60),
            MIN_SPACING,
            FAST,
            &|| true,
            &|| true,
            &|| panic!("the check ran in a closing vault"),
        );
        assert!(matches!(ran, Ran::Skipped(_)), "{ran:?}");
    }

    // Mutant, run by hand: not taking `exclusive()` in the check path lets the rebuild's
    // guard be had while the check is still inside `check`, and `rebuild_got_in` is set early.
    #[test]
    fn a_rebuild_requested_during_a_check_waits_for_it() {
        let (_activity, runner) = runner();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let checking = {
            let runner = Arc::clone(&runner);
            std::thread::spawn(move || {
                runner.run_when_idle(
                    Duration::ZERO,
                    MIN_SPACING,
                    FAST,
                    &|| false,
                    &|| true,
                    &|| {
                        entered_tx.send(()).expect("entered");
                        release_rx.recv().expect("release");
                        clean()
                    },
                )
            })
        };
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the check started");
        let got_in = Arc::new(AtomicBool::new(false));
        let rebuild = {
            let (runner, got_in) = (Arc::clone(&runner), Arc::clone(&got_in));
            std::thread::spawn(move || {
                let _exclusive = runner.exclusive();
                got_in.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !got_in.load(Ordering::SeqCst),
            "a rebuild started while the check held the old inode"
        );
        release_tx.send(()).expect("release");
        checking.join().expect("check");
        rebuild.join().expect("rebuild");
        assert!(got_in.load(Ordering::SeqCst));
    }
}
