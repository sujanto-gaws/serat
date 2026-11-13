//! Task #133: the two reads moved onto the blocking pool do not hold the caller or delay an
//! unrelated command (the #49 pattern: a first poll that returns, and a concurrent command
//! that completes while the slow one is still running).

use super::*;
use crate::cli::Boot;
use crate::ipc::SetLivePreviewArgs;
use crate::startup::Startup;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

fn open_state(files: &[(&str, &str)]) -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).expect("fixture");
    }
    let state = Arc::new(
        AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            crate::settings::Settings::default(),
        )
        .with_walk_gate(Duration::ZERO),
    );
    state
        .open_vault(OpenVaultArgs {
            root: dir.path().display().to_string(),
        })
        .expect("open");
    (state, dir)
}

fn first_poll<F: Future>(fut: std::pin::Pin<&mut F>) -> (Duration, bool) {
    let mut cx = Context::from_waker(Waker::noop());
    let start = Instant::now();
    let done = matches!(fut.poll(&mut cx), Poll::Ready(_));
    (start.elapsed(), done)
}

fn read(path: &str) -> ReadFileArgs {
    ReadFileArgs {
        path: path.to_string(),
    }
}

// Mutant, run by hand on this commit: `backlinks_off_thread` calling `backlinks` inline
// fails the first-poll bound.
#[test]
fn a_slow_backlinks_read_does_not_hold_the_caller_or_delay_an_unrelated_command() {
    let (state, _dir) = open_state(&[("slow.md", "slow\n"), ("fast.md", "fast\n")]);
    *state.slow_read.lock().expect("seam") =
        Some(("slow.md".to_string(), Duration::from_millis(800)));

    let mut slow = std::pin::pin!(state.backlinks_off_thread(NotePathArgs {
        path: "slow.md".to_string()
    }));
    let (took, done) = first_poll(slow.as_mut());
    assert!(!done);
    assert!(
        took < Duration::from_millis(200),
        "the caller was held for {took:?}: backlinks ran inline"
    );
    let other = Instant::now();
    let fast = tauri::async_runtime::block_on(state.read_file_off_thread(read("fast.md")))
        .expect("fast read");
    assert_eq!(fast.text, "fast\n");
    assert!(
        other.elapsed() < Duration::from_millis(400),
        "an unrelated read waited {:?} behind backlinks",
        other.elapsed()
    );
    // The answer is the command's own (not ready or ready; either is a valid outcome here).
    tauri::async_runtime::block_on(slow).expect("backlinks answers");
}

// Mutant, run by hand on this commit: `get_ui_settings_off_thread` calling
// `get_ui_settings` inline fails the first-poll bound.
#[test]
fn get_ui_settings_waiting_on_a_slow_settings_write_does_not_hold_the_caller() {
    let (state, _dir) = open_state(&[("fast.md", "fast\n")]);
    let (taken_tx, taken_rx) = std::sync::mpsc::channel();
    *state.slow_settings_write.lock().expect("seam") = Some((Duration::from_millis(800), taken_tx));

    // A settings write that holds the settings lock for 800 ms, on the pool.
    let mut write =
        std::pin::pin!(state.set_live_preview_off_thread(SetLivePreviewArgs { enabled: false }));
    let _ = first_poll(write.as_mut());
    // Wait for the fact that the write holds the lock, not for a guess at how long it takes.
    taken_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the write never took the settings lock");

    let started = Instant::now();
    let mut settings = std::pin::pin!(state.get_ui_settings_off_thread());
    let (took, done) = first_poll(settings.as_mut());
    assert!(!done);
    assert!(
        took < Duration::from_millis(200),
        "the caller was held for {took:?}: the read waited for the lock inline"
    );
    let other = Instant::now();
    tauri::async_runtime::block_on(state.read_file_off_thread(read("fast.md")))
        .expect("unrelated read");
    assert!(
        other.elapsed() < Duration::from_millis(400),
        "an unrelated read waited {:?} behind the settings read",
        other.elapsed()
    );

    let ui = tauri::async_runtime::block_on(settings).expect("settings");
    assert!(
        !ui.live_preview,
        "the read saw the write that held the lock"
    );
    assert!(started.elapsed() >= Duration::from_millis(500));
    tauri::async_runtime::block_on(write).expect("write");
}
