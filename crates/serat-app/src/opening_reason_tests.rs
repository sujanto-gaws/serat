//! Task #117 round 2: the `Opening` reason clears when the blocking actor exits, and that
//! clear is emitted, even while the new actor's open check is still running.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cli::Boot;
use crate::index::IndexOpen;
use crate::ipc::{IndexState, IndexStatus, OpenVaultArgs, OpeningReason};
use crate::settings::Settings;
use crate::startup::Startup;
use crate::state::{AppState, OpenHooks};

/// The detached actor of the first session: holds the index for a while, then is Ready.
struct Hold(Duration);

impl IndexOpen for Hold {
    fn open(&mut self, _r: &Path, _d: &Path, publish: &dyn Fn(IndexState)) {
        std::thread::sleep(self.0);
        publish(IndexState::Ready { note_count: 1 });
    }
}

/// The re-opened session's actor: says it has entered `open`, publishes nothing, and holds
/// the open check until released.
struct Gated {
    entered: Sender<()>,
    release: Receiver<()>,
}

impl IndexOpen for Gated {
    fn open(&mut self, _r: &Path, _d: &Path, publish: &dyn Fn(IndexState)) {
        let _ = self.entered.send(());
        let _ = self.release.recv();
        publish(IndexState::Ready { note_count: 1 });
    }
}

fn state() -> AppState {
    AppState::with_settings(
        Arc::new(Startup::begin()),
        Boot::default(),
        Settings::default(),
    )
    .with_walk_gate(Duration::ZERO)
}

// Mutant, run by hand on this commit: removing the `Opening { reason: None }` publish in the
// deferred thread leaves the polled state carrying the reason while the open check runs, and
// no plain `Opening` is ever emitted, so both assertions below fail.
#[test]
fn the_reason_is_cleared_and_announced_before_the_open_check_finishes() {
    let dir = std::env::temp_dir().join(format!("serat-opening-reason-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("fixture");
    std::fs::write(dir.join("a.md"), "# a\n").expect("fixture");
    let root = dir.display().to_string();

    let first = state();
    first
        .open_vault_with(
            OpenVaultArgs { root: root.clone() },
            Hold(Duration::from_millis(400)),
        )
        .expect("first open");
    first.close_vault();
    assert_eq!(
        first.detached_writers().live_count(),
        1,
        "one actor detached"
    );

    let second = state()
        .with_detached(first.detached_writers())
        .with_open_hooks(OpenHooks::default());
    let seen: Arc<Mutex<Vec<IndexStatus>>> = Arc::default();
    let sink = Arc::clone(&seen);
    second
        .events
        .install(move |status| sink.lock().expect("seen").push(status.clone()));
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let opened = second
        .open_vault_with(
            OpenVaultArgs { root },
            Gated {
                entered: entered_tx,
                release: release_rx,
            },
        )
        .expect("second open");
    assert_eq!(
        opened.index,
        IndexState::Opening {
            reason: Some(OpeningReason::PreviousIndexStillClosing)
        }
    );

    // The blocker exits, the new actor enters its open check and is held there.
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the deferred actor never started");
    assert_eq!(
        second.index_status().state,
        IndexState::Opening { reason: None },
        "the previous index has closed: the reason no longer holds"
    );
    assert!(
        seen.lock()
            .expect("seen")
            .iter()
            .any(|s| s.state == IndexState::Opening { reason: None }),
        "the clearing was not announced"
    );

    release_tx.send(()).expect("release");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !matches!(second.index_status().state, IndexState::Ready { .. }) {
        assert!(Instant::now() < deadline, "never Ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    let states: Vec<IndexState> = seen
        .lock()
        .expect("seen")
        .iter()
        .map(|s| s.state.clone())
        .collect();
    let clear = states
        .iter()
        .position(|s| *s == IndexState::Opening { reason: None })
        .expect("a plain Opening was emitted");
    let ready = states
        .iter()
        .position(|s| matches!(s, IndexState::Ready { .. }))
        .expect("Ready was emitted");
    assert!(clear < ready, "{states:?}");

    second.close_vault();
    let _ = std::fs::remove_dir_all(&dir);
}
