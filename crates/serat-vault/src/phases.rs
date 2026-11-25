//! Per-phase wall-clock timing of one call, for the benchmarks (task #172; NFR-PRF-03
//! evidence, NFR-PRF-02 evidence, US-01).
//!
//! **Costs nothing in a normal build.** Without the `phase-timing` feature every function
//! here is an empty `#[inline(always)]` body and [`finish`] returns an empty `Vec`, so a
//! `mark` call in the write path compiles to nothing. With the feature on, a mark only
//! records while a collection is active ([`begin`] .. [`finish`], on one thread), so an
//! instrumented build that nobody is observing pays one thread-local flag test.
//!
//! This observes; it never alters the write path. A mark sits between two statements that
//! already existed, and no byte is read, hashed or written differently (C-04, ADR-0008).
//!
//! A phase is named at its *end*: `mark("hash_before")` records the time since the previous
//! mark (or since [`begin`]) under that name, so the phases partition the call exactly and
//! their sum is the call.

use std::time::Duration;

/// Whether the `phase-timing` feature is compiled in. Informational: nothing asserts on it,
/// because it changes no behaviour, only whether [`mark`] records anything.
pub const PHASE_TIMING: bool = cfg!(feature = "phase-timing");

/// One phase of a timed call: its name and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    /// The name given to [`mark`], naming the work that *ended* at the mark.
    pub name: &'static str,
    /// Time since the previous mark, or since [`begin`] for the first.
    pub elapsed: Duration,
}

#[cfg(feature = "phase-timing")]
mod on {
    use super::Phase;
    use std::cell::RefCell;
    use std::time::Instant;

    struct Run {
        last: Instant,
        phases: Vec<Phase>,
    }

    thread_local! {
        static RUN: RefCell<Option<Run>> = const { RefCell::new(None) };
    }

    pub fn begin() {
        RUN.with(|run| {
            *run.borrow_mut() = Some(Run {
                last: Instant::now(),
                phases: Vec::with_capacity(24),
            });
        });
    }

    pub fn mark(name: &'static str) {
        RUN.with(|run| {
            if let Some(run) = run.borrow_mut().as_mut() {
                let now = Instant::now();
                run.phases.push(Phase {
                    name,
                    elapsed: now.duration_since(run.last),
                });
                // The stamp is taken after the push so the push is not charged to the next phase.
                run.last = Instant::now();
            }
        });
    }

    pub fn finish() -> Vec<Phase> {
        RUN.with(|run| {
            run.borrow_mut()
                .take()
                .map(|r| r.phases)
                .unwrap_or_default()
        })
    }
}

/// Start collecting on this thread, discarding anything already collected.
#[inline(always)]
pub fn begin() {
    #[cfg(feature = "phase-timing")]
    on::begin();
}

/// Record the time since the previous mark under `name`. A no-op unless collecting.
#[inline(always)]
pub fn mark(name: &'static str) {
    #[cfg(feature = "phase-timing")]
    on::mark(name);
    #[cfg(not(feature = "phase-timing"))]
    let _ = name;
}

/// Stop collecting on this thread and return the phases in the order they ended. Empty when
/// the feature is off or nothing was begun.
#[inline(always)]
#[must_use]
pub fn finish() -> Vec<Phase> {
    #[cfg(feature = "phase-timing")]
    return on::finish();
    #[cfg(not(feature = "phase-timing"))]
    Vec::new()
}

#[cfg(all(test, feature = "phase-timing"))]
mod tests_on {
    use super::*;
    use std::time::Instant;

    fn spin(ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {}
    }

    #[test]
    fn phases_come_back_in_the_order_they_ended() {
        begin();
        spin(2);
        mark("a");
        spin(2);
        mark("b");
        mark("c");
        let names: Vec<_> = finish().iter().map(|p| p.name).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn each_phase_is_charged_its_own_time_and_the_sum_is_the_call() {
        let started = Instant::now();
        begin();
        spin(5);
        mark("slow");
        mark("instant");
        let phases = finish();
        let total = started.elapsed();
        assert!(phases[0].elapsed >= Duration::from_millis(5), "{phases:?}");
        assert!(phases[1].elapsed < Duration::from_millis(2), "{phases:?}");
        let sum: Duration = phases.iter().map(|p| p.elapsed).sum();
        assert!(
            sum <= total,
            "phases cannot exceed the wall time around them"
        );
        assert!(
            total - sum < Duration::from_millis(2),
            "{sum:?} of {total:?}"
        );
    }

    #[test]
    fn nothing_records_outside_begin_and_finish() {
        mark("before");
        assert!(finish().is_empty());
        begin();
        mark("in");
        assert_eq!(finish().len(), 1);
        mark("after");
        assert!(finish().is_empty(), "finish stops the collection");
    }

    #[test]
    fn begin_discards_an_unfinished_run() {
        begin();
        mark("stale");
        begin();
        mark("fresh");
        let phases = finish();
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].name, "fresh");
    }

    #[test]
    fn a_run_belongs_to_its_thread() {
        begin();
        std::thread::spawn(|| {
            mark("elsewhere");
            assert!(finish().is_empty());
        })
        .join()
        .unwrap();
        mark("here");
        assert_eq!(finish().len(), 1);
    }
}

#[cfg(all(test, not(feature = "phase-timing")))]
mod tests_off {
    use super::*;

    #[test]
    fn the_disabled_build_records_nothing_and_says_so() {
        // Compile-time: the feature-off build must see the constant false
        // (a runtime assert on a constant is clippy::assertions_on_constants).
        const _: () = assert!(!PHASE_TIMING);
        begin();
        mark("a");
        mark("b");
        assert!(finish().is_empty());
    }
}
