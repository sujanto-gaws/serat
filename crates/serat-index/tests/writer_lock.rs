//! One writer per vault, across threads and processes (task #154; NFR-REL-01,
//! NFR-REL-02; ADR-0001, ADR-0002).
//!
//! # What this guards
//!
//! test-engineer-105 found a real SIGBUS (`docs/05-testing/sigbus-105.md`): with no
//! index present, three live index actors on one vault each rebuilt and wrote the same
//! `.serat/index.db`, and one faulted in `memset` called from `walIndexAppend` inside
//! its mapping of `index.db-shm` (`si_code` 2, `BUS_ADRERR`). `rebuild` removed
//! `index.db`, `-wal` and `-shm` while another writer had the old `-shm` mapped, and
//! the next write into that mapping had no page behind it. The core's stack:
//!
//! ```text
//! signal SIGBUS, si_code 2, si_addr inside .../.serat/index.db-shm (offset 0x1000)
//! __memset_avx512_unaligned_erms
//!   <- walIndexAppend <- walFrames <- sqlite3WalFrames
//! ```
//!
//! The fix is an advisory lock on `.serat/index.lock`, taken before any rebuild or
//! writer open and held for the writer's life (`serat_index::lock`). A second opener is
//! refused with `IndexError::HeldByAnotherProcess` instead of rebuilding under the
//! first.
//!
//! # Before and after
//!
//! Without the lock (the lock's `acquire` made to always succeed), the first test fails
//! at once (the second open succeeds and replaces the files under the first), and the
//! storm fails or crashes within a few iterations. With it, all pass. The
//! two-process variant is `#[ignore]` because it spawns this test binary; run it with
//! `cargo test -p serat-index --test writer_lock -- --ignored`.

mod support;

use std::path::Path;
use std::sync::{Arc, Barrier};

use serat_core::IndexError;
use serat_index::{IndexHandle, open_or_rebuild};
use support::{LineParser, vault_fixture};

const ROOT: &str = "C:/vaults/serat";
const CHILD_ENV: &str = "SERAT_WRITER_LOCK_CHILD_DIR";

fn open(index_dir: &Path) -> Result<IndexHandle, IndexError> {
    open_or_rebuild(
        Path::new(ROOT),
        index_dir,
        &vault_fixture(),
        &LineParser::default(),
        &|_| {},
    )
    .map(|(handle, _)| handle)
}

#[test]
fn a_second_open_beside_a_live_writer_is_refused_and_leaves_the_first_intact() {
    let dir = tempfile::tempdir().expect("temp");
    let index_dir = dir.path().join(".serat");
    let first = open(&index_dir).expect("first open");
    let before = first.note_count().expect("count");
    assert_eq!(before, 3);

    // From another thread: the lock is per open file description, not per thread, so a
    // refusal here is the same refusal another process would get.
    let second = std::thread::scope(|s| s.spawn(|| open(&index_dir)).join().expect("thread"));
    assert!(
        matches!(second, Err(IndexError::HeldByAnotherProcess { .. })),
        "a second writer was admitted: {:?}",
        second.map(|_| ())
    );

    // The files the first writer has mapped were not replaced: it still reads and
    // counts the same index.
    assert_eq!(first.note_count().expect("count"), before);
    first.close().expect("close");

    // Released with the writer, so the next session opens normally.
    open(&index_dir).expect("reopen after the holder closed");
}

#[test]
fn a_rebuild_beside_a_live_writer_is_refused() {
    let dir = tempfile::tempdir().expect("temp");
    let index_dir = dir.path().join(".serat");
    let _held = open(&index_dir).expect("open");
    let result = serat_index::rebuild(
        &vault_fixture(),
        &LineParser::default(),
        Path::new(ROOT),
        &index_dir,
    );
    assert!(
        matches!(result, Err(IndexError::HeldByAnotherProcess { .. })),
        "{:?}",
        result.map(|_| ())
    );
}

/// Two threads open a vault with no index, a few hundred times. Every round, at least
/// one wins (the first to lock), the loser is refused rather than rebuilding under it,
/// and nothing crashes or reports corruption.
#[test]
fn two_threads_opening_a_vault_with_no_index_never_fault_or_corrupt() {
    for round in 0..300 {
        let dir = tempfile::tempdir().expect("temp");
        let index_dir = dir.path().join(".serat");
        let barrier = Arc::new(Barrier::new(2));
        let outcomes: Vec<Result<u32, IndexError>> = std::thread::scope(|s| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    let index_dir = &index_dir;
                    s.spawn(move || {
                        barrier.wait();
                        let handle = open(index_dir)?;
                        handle.note_count()
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("worker"))
                .collect()
        });
        assert!(
            outcomes.iter().any(Result::is_ok),
            "round {round}: nobody opened: {outcomes:?}"
        );
        for outcome in &outcomes {
            match outcome {
                Ok(count) => assert_eq!(*count, 3, "round {round}"),
                Err(IndexError::HeldByAnotherProcess { .. }) => {}
                Err(other) => panic!("round {round}: {other:?}"),
            }
        }
    }
}

/// Child half of the refusal test: opens once and exits by panicking unless the open is
/// refused with `HeldByAnotherProcess`. Ignored so a normal run never executes it
/// outside the parent that sets [`CHILD_ENV`].
#[test]
#[ignore = "child of two_processes_a_second_process_is_refused_by_a_live_writer"]
fn child_expects_to_be_refused() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return; // `--ignored` run directly, not by a parent
    };
    let result = open(&Path::new(&dir).join(".serat"));
    assert!(
        matches!(result, Err(IndexError::HeldByAnotherProcess { .. })),
        "the child was admitted beside a live writer: {:?}",
        result.map(|_| ())
    );
}

/// Child half of the storm: opens in a loop and prints how often it was refused.
#[test]
#[ignore = "child of two_processes_hammering_one_vault_never_fault"]
fn child_hammers_the_shared_index() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return; // `--ignored` run directly, not by a parent
    };
    let index_dir = Path::new(&dir).join(".serat");
    let mut refused = 0;
    for _ in 0..300 {
        match open(&index_dir) {
            Ok(handle) => assert_eq!(handle.note_count().expect("count"), 3),
            Err(IndexError::HeldByAnotherProcess { .. }) => refused += 1,
            Err(other) => panic!("child: {other:?}"),
        }
    }
    println!("REFUSED={refused}");
}

fn spawn_child(test: &str, dir: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().expect("exe"))
        .args(["--exact", test, "--ignored", "--nocapture"])
        .env(CHILD_ENV, dir)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn")
}

/// Deterministic: the parent holds a writer, a second process opens the same vault and
/// must be refused. Without the lock the child is admitted and exits non-zero.
#[test]
fn two_processes_a_second_process_is_refused_by_a_live_writer() {
    let dir = tempfile::tempdir().expect("temp");
    let held = open(&dir.path().join(".serat")).expect("parent open");
    let out = spawn_child("child_expects_to_be_refused", dir.path())
        .wait_with_output()
        .expect("child");
    assert!(
        out.status.success(),
        "the other process was not refused: {:?}",
        out.status
    );
    drop(held);
}

/// Two processes race to open an index-less vault: neither may die of a signal (SIGBUS
/// is 7) or fail for any reason but the refusal, and across both at least one open was
/// refused. Ignored by default: it spawns this binary and runs a few seconds.
#[test]
#[ignore = "spawns this test binary; run with --ignored"]
fn two_processes_hammering_one_vault_never_fault() {
    let dir = tempfile::tempdir().expect("temp");
    let child = spawn_child("child_hammers_the_shared_index", dir.path());
    let index_dir = dir.path().join(".serat");
    let mut refused = 0;
    for _ in 0..300 {
        match open(&index_dir) {
            Ok(handle) => assert_eq!(handle.note_count().expect("count"), 3),
            Err(IndexError::HeldByAnotherProcess { .. }) => refused += 1,
            Err(other) => panic!("parent: {other:?}"),
        }
    }
    let out = child.wait_with_output().expect("child");
    assert!(
        out.status.success(),
        "the other process died: {:?}",
        out.status
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let theirs: u32 = text
        .lines()
        .find_map(|l| l.strip_prefix("REFUSED="))
        .and_then(|n| n.trim().parse().ok())
        .expect("the child reports its refusals");
    assert!(
        refused + theirs > 0,
        "the two processes never overlapped, so nothing was exercised"
    );
}
