//! Task #155 (NFR-REL-01, NFR-REL-02, US-01, US-07): another process holding the index writer
//! lock is a wait with a stated reason, not the end of the session's index.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::index::{IndexActor, SeratIndexOpen, StatusPublisher, StatusSink, held_gave_up_message};
use crate::ipc::{IndexState, IndexStatus, OpeningReason};
use crate::startup::Startup;

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<IndexStatus>>>);

impl Recorder {
    fn states(&self) -> Vec<IndexState> {
        self.0
            .lock()
            .expect("recorder")
            .iter()
            .map(|s| s.state.clone())
            .collect()
    }
}

impl StatusSink for Recorder {
    fn emit(&self, status: &IndexStatus) {
        self.0.lock().expect("recorder").push(status.clone());
    }
}

const HELD: IndexState = IndexState::Opening {
    reason: Some(OpeningReason::IndexHeldByAnotherProcess),
};
const PLAIN: IndexState = IndexState::Opening { reason: None };

fn vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.md"), "# a\n").expect("fixture");
    dir
}

/// Another "process": opens the index and keeps its writer lock until closed.
fn holder(root: &Path) -> serat_index::IndexHandle {
    let source = serat_vault::Vault::open(root).expect("vault");
    serat_index::open_or_rebuild(
        root,
        &root.join(".serat"),
        &source,
        &serat_parse::CommonMarkParser,
        &|_| {},
    )
    .map(|(handle, _)| handle)
    .expect("the holder opens first")
}

fn publisher(recorder: &Recorder) -> Arc<StatusPublisher<Recorder>> {
    Arc::new(StatusPublisher::with_throttle(
        "v1",
        Arc::new(Startup::begin()),
        recorder.clone(),
        Duration::from_millis(1),
    ))
}

fn spawn(
    root: &Path,
    publisher: &Arc<StatusPublisher<Recorder>>,
    every: Duration,
    bound: Duration,
) -> IndexActor {
    let (actor, _slot) = IndexActor::spawn(
        Arc::clone(publisher),
        SeratIndexOpen::default().with_retry(every, bound),
        root.to_path_buf(),
        root.join(".serat"),
        Arc::default(),
    )
    .expect("spawn");
    actor
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Mutant, run by hand on this commit: making `open_cancellable` publish `Unavailable` at the
// first `HeldByAnotherProcess` (the pre-#155 behaviour) fails three of the four tests in this
// file (the wait-out, the bound and the retire test); the verbatim-wording test does not depend
// on it.
#[test]
fn a_held_lock_is_waited_out_and_the_open_then_proceeds_in_the_stated_order() {
    let dir = vault();
    let other = holder(dir.path());
    let recorder = Recorder::default();
    let publisher = publisher(&recorder);
    let mut actor = spawn(
        dir.path(),
        &publisher,
        Duration::from_millis(40),
        Duration::from_secs(30),
    );

    wait_for("the reason to be published", || {
        recorder.states().contains(&HELD)
    });
    // Several attempts happen while it is held, and none flaps the reason.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        publisher.status().state,
        HELD,
        "the reason must stay while the lock is held: {:?}",
        recorder.states()
    );
    assert!(
        recorder
            .states()
            .iter()
            .all(|s| !matches!(s, IndexState::Unavailable { .. })),
        "gave up while the holder was still alive"
    );

    other.close().expect("the holder closes");
    wait_for("Ready", || {
        matches!(publisher.status().state, IndexState::Ready { .. })
    });
    assert!(actor.join_within(Duration::from_secs(10)));

    let states = recorder.states();
    let held = states.iter().position(|s| *s == HELD).expect("held");
    let clear = states
        .iter()
        .rposition(|s| *s == PLAIN)
        .expect("the reason is cleared");
    let ready = states
        .iter()
        .position(|s| matches!(s, IndexState::Ready { .. }))
        .expect("ready");
    assert!(held < clear && clear < ready, "{states:?}");
    assert_eq!(
        states.iter().filter(|s| **s == HELD).count(),
        1,
        "the reason is published once, not per attempt: {states:?}"
    );
}

#[test]
fn a_lock_held_past_the_bound_ends_in_unavailable_with_the_waited_text() {
    let dir = vault();
    let other = holder(dir.path());
    let recorder = Recorder::default();
    let publisher = publisher(&recorder);
    let mut actor = spawn(
        dir.path(),
        &publisher,
        Duration::from_millis(30),
        Duration::from_millis(300),
    );
    assert!(actor.join_within(Duration::from_secs(10)), "never gave up");

    let states = recorder.states();
    assert!(states.contains(&HELD), "{states:?}");
    match states.last().expect("a last state") {
        IndexState::Unavailable { message, .. } => {
            assert_eq!(message, &held_gave_up_message(Duration::from_millis(300)));
            assert!(message.contains("after waiting"), "{message}");
            assert!(message.contains("You can keep editing."), "{message}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    other.close().expect("close");
}

#[test]
fn retiring_during_the_wait_ends_it_and_emits_nothing_more() {
    let dir = vault();
    let other = holder(dir.path());
    let recorder = Recorder::default();
    let publisher = publisher(&recorder);
    let mut actor = spawn(
        dir.path(),
        &publisher,
        // A 10 s interval: only the 25 ms slicing inside the sleep can end the wait within
        // 500 ms. Dropping the in-sleep `cancelled()` check lets the 10 s sleep run out.
        Duration::from_secs(10),
        Duration::from_secs(300),
    );
    wait_for("the reason to be published", || {
        recorder.states().contains(&HELD)
    });
    publisher.retire();
    let emitted = recorder.states().len();
    assert!(
        actor.join_within(Duration::from_millis(500)),
        "the wait did not end promptly when the publisher was retired"
    );
    // Released after the retire: a still-waiting actor would now open and publish.
    other.close().expect("close");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(recorder.states().len(), emitted, "a retired wait emitted");
}

#[test]
fn the_wording_past_the_default_bound_is_the_ratified_text_verbatim() {
    assert_eq!(
        held_gave_up_message(crate::index::HELD_RETRY_BOUND),
        "Another Serat session still has this vault's index open after waiting 60 seconds. \
         Search and queries are unavailable until you reopen the vault after that session \
         ends. You can keep editing."
    );
}
