use super::*;
use crate::cli::Boot;
use crate::ipc::{FlushAckArgs, Pos};
use crate::startup::Startup;
use std::future::Future;
use std::sync::Mutex;
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

fn read(path: &str) -> ReadFileArgs {
    ReadFileArgs {
        path: path.to_string(),
    }
}

/// Polls `fut` once with a waker that does nothing, and says how long the poll took and
/// whether it finished. A command that does its I/O inline holds the poll for the whole
/// of it; one that hands it to the blocking pool returns `Pending` at once.
fn first_poll<F: Future>(fut: std::pin::Pin<&mut F>) -> (Duration, bool) {
    let mut cx = Context::from_waker(Waker::noop());
    let start = Instant::now();
    let done = matches!(fut.poll(&mut cx), Poll::Ready(_));
    (start.elapsed(), done)
}

// Mutant, run by hand on this commit: making `read_file_off_thread` call
// `self.read_file(args)` inline (no `blocking`) fails this on the first-poll bound.
#[test]
fn a_slow_read_of_one_note_does_not_hold_the_caller_or_delay_an_unrelated_command() {
    let (state, _dir) = open_state(&[("slow.md", "slow\n"), ("fast.md", "fast\n")]);
    *state.slow_read.lock().expect("seam") =
        Some(("slow.md".to_string(), Duration::from_millis(800)));

    let started = Instant::now();
    let mut slow = std::pin::pin!(state.read_file_off_thread(read("slow.md")));
    let (poll_took, done) = first_poll(slow.as_mut());
    assert!(!done, "the slow read cannot already be finished");
    assert!(
        poll_took < Duration::from_millis(200),
        "the caller's thread was held for {poll_took:?}: the read ran inline"
    );

    // While the slow read holds one blocking-pool thread, other commands complete.
    let other = Instant::now();
    let fast = tauri::async_runtime::block_on(state.read_file_off_thread(read("fast.md")))
        .expect("fast read");
    assert_eq!(fast.text, "fast\n");
    let info = tauri::async_runtime::block_on(state.asset_info_off_thread(AssetArgs {
        path: "fast.md".to_string(),
    }));
    assert!(
        other.elapsed() < Duration::from_millis(400),
        "unrelated commands ({info:?}) waited {:?} behind the slow read",
        other.elapsed()
    );

    let slow = tauri::async_runtime::block_on(slow).expect("slow read");
    assert_eq!(slow.text, "slow\n");
    assert!(started.elapsed() >= Duration::from_millis(800));
}

fn insert_x(path: &str, base_hash: &str) -> SavePatchArgs {
    SavePatchArgs {
        path: path.to_string(),
        base_hash: base_hash.to_string(),
        patches: vec![Patch {
            from: Pos { line: 0, col: 0 },
            to: Pos { line: 0, col: 0 },
            insert: "x".to_string(),
        }],
    }
}

// A regression pin on the ordering rules, not a mutant-killer for this module: the hash chain
// is `write_patch`'s, and this shows it still holds with saves arriving on pool threads.
#[test]
fn concurrent_saves_on_one_base_land_exactly_once_and_a_chain_stays_ordered() {
    let (state, dir) = open_state(&[("n.md", "a\n"), ("m.md", "b\n")]);
    let base = state.read_file(read("n.md")).expect("read").hash;

    // 8 racing saves on the same base: one wins, the rest are StaleHash.
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let results: Arc<Mutex<Vec<Result<FileWritten, IpcError>>>> = Arc::default();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (state, barrier, results, base) = (
                Arc::clone(&state),
                Arc::clone(&barrier),
                Arc::clone(&results),
                base.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                let r = tauri::async_runtime::block_on(
                    state.save_patch_off_thread(insert_x("n.md", &base)),
                );
                results.lock().expect("results").push(r);
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }
    let results = results.lock().expect("results");
    let wins: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert_eq!(wins.len(), 1, "{results:?}");
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| matches!(r, Err(IpcError::StaleHash { .. }))),
        "{results:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("n.md")).expect("read"),
        "xa\n"
    );
    // FileWritten pairs the hash with the mtime of the same write.
    let on_disk = state.read_file(read("n.md")).expect("read");
    assert_eq!(wins[0].hash, on_disk.hash);
    assert_eq!(wins[0].mtime_ms, on_disk.mtime_ms);

    // A chain, each save awaited before the next and based on the last answer.
    let mut hash = wins[0].hash.clone();
    for i in 0..20 {
        let wrote =
            tauri::async_runtime::block_on(state.save_patch_off_thread(insert_x("n.md", &hash)))
                .unwrap_or_else(|e| panic!("save {i}: {e:?}"));
        hash = wrote.hash;
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("n.md")).expect("read"),
        format!("{}a\n", "x".repeat(21))
    );

    // A save to another note is neither held up by, nor disturbs, the first.
    let m = state.read_file(read("m.md")).expect("read").hash;
    tauri::async_runtime::block_on(state.save_patch_off_thread(insert_x("m.md", &m)))
        .expect("independent note");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("m.md")).expect("read"),
        "xb\n"
    );
}

// Mutant, run by hand on this commit: `open_vault_off_thread` calling `open_vault` inline fails
// the first-poll bound (the 300 ms ack wait runs on the caller's thread).
#[test]
fn a_switch_off_the_calling_thread_still_flushes_before_it_closes() {
    let (state, _a) = open_state(&[("n.md", "a\n")]);
    let b = tempfile::tempdir().expect("tempdir");
    let hash = state.read_file(read("n.md")).expect("read").hash;
    state
        .note_opened(&NoteBufferArgs {
            path: "n.md".to_string(),
            base_hash: hash,
        })
        .expect("note opened");

    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (l, acker) = (Arc::clone(&log), Arc::clone(&state));
    state.events.install_closing(move |c| {
        l.lock()
            .expect("log")
            .push(format!("closing:{}", c.vault_id));
        let (acker, id) = (Arc::clone(&acker), c.vault_id.clone());
        // The ack arrives later, from another thread, as the webview's would.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            acker.flush_ack(&FlushAckArgs { vault_id: id });
        });
    });
    let old_id = state.vault_id().expect("vault id");
    let l = Arc::clone(&log);
    state.events.install_opened(move |o| {
        l.lock()
            .expect("log")
            .push(format!("opened:{}", o.vault_id));
    });

    let mut switch = std::pin::pin!(state.open_vault_off_thread(OpenVaultArgs {
        root: b.path().display().to_string(),
    }));
    let (poll_took, done) = first_poll(switch.as_mut());
    assert!(!done);
    assert!(
        poll_took < Duration::from_millis(150),
        "the switch's flush wait held the caller for {poll_took:?}"
    );
    let opened = tauri::async_runtime::block_on(switch).expect("switch");
    let log = log.lock().expect("log");
    assert_eq!(
        *log,
        vec![
            format!("closing:{old_id}"),
            format!("opened:{}", opened.vault_id)
        ]
    );
}
