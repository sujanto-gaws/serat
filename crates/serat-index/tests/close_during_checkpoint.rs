//! `IndexHandle::close` stops and joins the checkpointer before it asks the writer to
//! close cleanly (task #56, S2).
//!
//! SQLite takes the checkpoint lock without the busy handler, so the writer's
//! `wal_checkpoint(TRUNCATE)` racing the checkpointer's in-flight `PASSIVE` returns busy
//! at once and the clean-close marker would be left cleared, costing a full catch-up at
//! the next open. The race window is milliseconds wide, so the test lands a close in it
//! on purpose, round after round, and checks the marker every time.

use std::time::{Duration, Instant};

use serat_core::{ContentHash, IndexOp, IndexWriter as _, NoteKind, NoteMeta, VaultPath};
use serat_index::{IndexHandle, schema};

fn assets(from: usize, to: usize) -> impl Iterator<Item = IndexOp> {
    (from..to).map(|i| IndexOp::UpsertAsset {
        meta: NoteMeta {
            kind: NoteKind::Asset,
            hash: ContentHash::from_bytes([3u8; 32]),
            size: 0,
            mtime_ms: 0,
            path: VaultPath::new(format!("assets/a{i}.pdf")).expect("path"),
        },
    })
}

fn clean_close(dir: &std::path::Path) -> i64 {
    let conn = schema::open(&serat_index::index_path(dir)).expect("open");
    conn.query_row(
        "SELECT clean_close FROM index_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .expect("marker")
}

#[test]
fn a_close_landing_during_a_passive_still_sets_the_marker_and_joins_the_checkpointer() {
    let dir = tempfile::tempdir().expect("temp");
    let mut overlaps = 0;
    let mut next = 0;
    for round in 0..60 {
        let handle = IndexHandle::open(dir.path()).expect("open");
        let health = handle.wal_health();
        let writer = handle.writer();
        for op in assets(next, next + 3000) {
            writer.submit_blocking(op).expect("submit");
        }
        next += 3000;
        writer.flush().expect("flush");

        // The idle gate is 250 ms; wait for the checkpointer to be inside a PASSIVE.
        let started = Instant::now();
        let mut inside = false;
        while started.elapsed() < Duration::from_secs(3) {
            if health.checkpoint_in_progress() {
                inside = true;
                break;
            }
            std::thread::sleep(Duration::from_micros(50));
        }
        if inside {
            overlaps += 1;
        }
        drop(writer);
        handle.close().expect("close");

        assert_eq!(
            clean_close(dir.path()),
            1,
            "round {round}: a close {} a PASSIVE left the marker cleared",
            if inside { "landing during" } else { "after" }
        );
        // Joined before the marker was written: nothing is running now, and nothing
        // runs later.
        assert!(!health.checkpoint_in_progress());
        let runs = health.stats().passive_runs;
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            health.stats().passive_runs,
            runs,
            "the thread outlived close"
        );
        if overlaps >= 8 {
            break;
        }
    }
    assert!(
        overlaps >= 1,
        "no round caught the checkpointer mid-PASSIVE, so the race was never exercised"
    );
}
