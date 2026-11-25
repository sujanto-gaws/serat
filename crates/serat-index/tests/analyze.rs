//! The opportunistic `ANALYZE`, and the one condition that switches it off.
//!
//! Its own binary rather than a unit test in `writer.rs`. The original reason was
//! `FAIL_NEXT_COMMIT`: it was a process-wide static, the test that injected through
//! it said in its own comment that a second test sharing the process races it, and
//! these tests commit dozens of batches, so as a unit test they would have consumed
//! that injection and left the other test seeing `Ok(())` instead of the failure it
//! asked for.
//!
//! **That reason is gone.** Task #28 replaced the static with a per-`IndexActor`
//! `FaultInjector`, so nothing here can reach another test's armed fault and these
//! tests could now be unit tests without racing anything. They stay integration
//! tests on a smaller but still real ground: they drive `IndexActor` and
//! `schema` through the public API, which is the surface a caller actually has, and
//! moving a passing test to chase a retired rationale is churn rather than upkeep.

use std::path::Path;

use serat_core::{ContentHash, IndexOp, IndexWriter, NoteKind, NoteMeta, VaultPath};
use serat_index::IndexActor;
use serat_index::schema;

fn asset(path: &str) -> IndexOp {
    let path = VaultPath::new(path).expect("path");
    IndexOp::UpsertAsset {
        meta: NoteMeta {
            kind: NoteKind::Asset,
            hash: ContentHash::from_bytes([2u8; 32]),
            size: 0,
            mtime_ms: 0,
            path,
        },
    }
}

/// How many rows the planner's statistics believe `notes` holds.
///
/// Not the existence of `sqlite_stat1`, and not whether it mentions `notes`:
/// `schema/001_init.sql` ends with its own `ANALYZE`, so the table is there from
/// bootstrap and already carries a row saying `notes` holds 0. Both of those
/// looked like the observable and neither is; the measurement said so twice.
/// What actually distinguishes a load that re-analyzed from one that did not is
/// whether that number moved off the bootstrap value.
fn analyzed_notes(path: &Path) -> Option<i64> {
    let conn = schema::open(path).expect("open");
    conn.query_row(
        // sqlite_stat1.stat begins with the number of rows the index covers. The
        // largest over all of `notes`' indexes: a partial index that happens to be
        // empty (ix_notes_sidecar) reports 0 and must not be the one that is read.
        "SELECT max(CAST(substr(stat, 1, instr(stat || ' ', ' ') - 1) AS INTEGER))
           FROM sqlite_stat1 WHERE tbl = 'notes' AND idx IS NOT NULL",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

/// Fill an index with `n` assets through its own writer and report whether the
/// opportunistic `ANALYZE` ran, with the bulk load either open or not.
fn statistics_after_a_load(bulk: bool) -> Option<i64> {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    let actor = IndexActor::with_connection(conn);
    let writer = actor.handle();

    let triggers = if bulk {
        Some(writer.begin_bulk_load().expect("begin bulk load"))
    } else {
        None
    };

    // `notes` starts empty, so any batch at all touches more than
    // ANALYZE_FRACTION of it. This is exactly the condition that made the
    // opportunistic path fire on nearly every batch of a rebuild.
    //
    // The batching is the writer's to choose, and the 5% rule is applied per batch
    // against what was touched since the last ANALYZE, so which statistics survive
    // depends on where the writer happened to cut the stream (task #200, ci run 468:
    // 39 and 1 left a final batch of 1 touching 1 <= 5% of 40, statistics for 39).
    // Under `test-hooks` the cut is made deterministic: the first op is held at its
    // COMMIT, the other 39 queue behind it, so the batches are exactly [1] and [39]
    // and the last one always crosses the threshold. Without the feature the cut is
    // left to the scheduler and the test accepts any value the
    // bootstrap's 0 cannot be.
    #[cfg(feature = "test-hooks")]
    {
        let hold = actor.commit_gate().hold();
        writer
            .submit_blocking(asset("assets/a0.pdf"))
            .expect("submit");
        assert!(
            hold.wait_reached(std::time::Duration::from_secs(10)),
            "the writer never reached the hold"
        );
        for i in 1..40 {
            writer
                .submit_blocking(asset(&format!("assets/a{i}.pdf")))
                .expect("submit");
        }
        drop(hold);
    }
    #[cfg(not(feature = "test-hooks"))]
    for i in 0..40 {
        writer
            .submit_blocking(asset(&format!("assets/a{i}.pdf")))
            .expect("submit");
    }
    writer.flush().expect("flush");

    let seen = analyzed_notes(&path);
    if let Some(triggers) = triggers {
        writer.end_bulk_load(triggers).expect("end bulk load");
    }
    actor.shutdown().expect("shutdown");
    seen
}

#[test]
fn a_bulk_load_skips_the_opportunistic_analyze_and_an_ordinary_load_does_not() {
    // The 5% rule (§4.3, ANALYZE_FRACTION) is written for a steady-state table:
    // enough saves have touched enough of `notes` that the planner's statistics
    // no longer describe it. During a rebuild `notes` starts EMPTY and grows, so
    // "this batch touched more than 5% of the table" is true of nearly every
    // batch — it ran 35 times on the 20k vault, each superseded by the next, and
    // only the caller's explicit ANALYZE at the end survives.
    //
    // Both halves in one test, and in the same run, so the difference is
    // attributable to the bulk load rather than to scale or timing. Asserting
    // only the suppressed half would pass just as well if the opportunistic
    // ANALYZE had been removed altogether, which is a different and much worse
    // change: the incremental path is the one that needs it.
    let bulk = statistics_after_a_load(true);
    assert_eq!(
        bulk,
        Some(0),
        "a bulk load ran ANALYZE and left statistics describing {bulk:?} notes; the              rebuild throws these away with its own ANALYZE at the end, so every one of              them is wasted work"
    );

    let ordinary = statistics_after_a_load(false);
    #[cfg(feature = "test-hooks")]
    assert_eq!(
        ordinary,
        Some(40),
        "an ordinary load must leave the planner statistics describing all 40 notes;              without them every typed query degrades to a full scan (§4.3)"
    );
    // Batch boundaries are the scheduler's here, so only "ANALYZE ran at all" is
    // decidable: the number moved off the bootstrap's 0 and never past what exists.
    #[cfg(not(feature = "test-hooks"))]
    assert!(
        matches!(ordinary, Some(1..=40)),
        "an ordinary load must run the opportunistic ANALYZE (statistics moved off the              bootstrap 0); got {ordinary:?}"
    );
}
