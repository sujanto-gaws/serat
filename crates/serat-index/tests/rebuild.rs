//! The single-writer actor and the full rebuild, driven by stubs.
//!
//! No filesystem walk and no Markdown parser anywhere in this file. That is the
//! dependency rule paying for itself: `serat-index` never links `serat-vault` or
//! `serat-parse`, so its tests supply both as trait objects and the index can be
//! tested against inputs a real vault would be tedious to arrange.

mod support;

use std::sync::atomic::Ordering;

use serat_core::{IndexOp, IndexWriter, VaultPath};
use serat_index::{IndexActor, SCHEMA_VERSION, rebuild, schema};
use support::{FakeVault, LineParser, meta, vault_fixture};

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

#[test]
fn a_rebuild_indexes_every_file_and_counts_what_landed() {
    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");

    // Three files: two notes and one asset. The asset is a row without blocks.
    assert_eq!(report.notes, 3);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.blocks, 5 + 3, "one block per line of the two notes");
    assert_eq!(report.fields, 3, "type, status, type");
    assert_eq!(report.links, 2);
    assert_eq!(report.tasks, 1);
}

#[test]
fn the_report_counts_rows_that_exist_rather_than_operations_sent() {
    // The failure this guards: a rebuild that submits 20,000 notes, loses some, and
    // still reports 20,000 would post a fast benchmark number for half a vault.
    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let actual: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(report.notes as i64, actual);
}

#[test]
fn an_unreadable_file_is_skipped_rather_than_fatal() {
    // Design goal 5. One bad file must not stop a vault indexing.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture().unreadable("20-systems/indexing.md");
    let report = rebuild(&vault, &LineParser::default(), dir.path()).expect("rebuild");

    assert_eq!(report.skipped, 1);
    assert_eq!(report.notes, 2, "the other two files still indexed");
}

#[test]
fn a_file_the_parser_refuses_is_skipped_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("temp");
    let parser = LineParser::default();
    parser
        .refuses
        .lock()
        .expect("lock")
        .push("10-projects/serat.md".to_owned());

    let report = rebuild(&vault_fixture(), &parser, dir.path()).expect("rebuild");
    assert_eq!(report.skipped, 1);
    assert_eq!(report.notes, 2);
}

#[test]
fn a_rebuild_over_an_existing_index_replaces_it_rather_than_merging() {
    // ADR-0002: there is no repair. A second rebuild must produce the same index, not
    // a doubled one.
    let dir = tempfile::tempdir().expect("temp");
    let first = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("first");
    let second = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("second");

    // Counts only. The phase durations differ run to run by construction, so comparing
    // whole reports would be asserting that a clock is deterministic.
    assert_eq!(
        (
            first.notes,
            first.blocks,
            first.fields,
            first.links,
            first.tasks,
            first.links_resolved
        ),
        (
            second.notes,
            second.blocks,
            second.fields,
            second.links,
            second.tasks,
            second.links_resolved
        ),
        "a second rebuild produced a different index"
    );
}

#[test]
fn a_rebuild_leaves_the_index_ready_and_the_note_count_true() {
    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");

    // The cold-start check treats a note_count that disagrees with count(*) as
    // corruption, so a rebuild that left them out of step would make every subsequent
    // start rebuild again.
    assert_eq!(
        schema::inspect(&rebuild_path(dir.path())).expect("inspect"),
        serat_index::IndexState::Ready
    );

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let (recorded, version): (i64, i64) = conn
        .query_row(
            "SELECT note_count, schema_version FROM index_meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("meta");
    assert_eq!(recorded as usize, report.notes);
    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn the_statistics_left_behind_describe_the_finished_index() {
    // architecture.md §4.3: without current statistics the planner prefers
    // ix_notes_kind, which matches nearly every row, over ix_notes_type, and every
    // typed query becomes a full scan.
    //
    // The earlier version of this test asserted only that sqlite_stat1 was non-empty,
    // and I mutation-checked it: deleting the rebuild's explicit analyze() call left
    // it passing, because commit() also runs ANALYZE opportunistically when a batch
    // touches more than 5% of the notes. So it was guarding "ANALYZE ran at some
    // point", which is not the property — stale statistics gathered a third of the way
    // through a rebuild are exactly as bad as none (F-35).
    //
    // What matters is that the statistics describe the index as it finally stands.
    //
    // Being straight about what this still does NOT catch, because the rewrite did not
    // fix that half: deleting the rebuild's explicit analyze() leaves this passing too.
    // The reason is arithmetic rather than a weak assertion. commit() re-runs ANALYZE
    // whenever a batch has touched more than 5% of the notes, and at test scale 5% is
    // smaller than one batch, so the opportunistic path fires on the last batch every
    // time and the explicit call changes nothing observable.
    //
    // It is still load-bearing at real scale: 20,000 notes make the threshold 1,000
    // while BATCH_SIZE is 512, so the tail batch (20000 % 512 = 32 notes) does not
    // cross it and the statistics would be left describing the index as it was 32
    // notes ago. Reproducing that needs a 20,000-note rebuild, which belongs in the
    // budget harness against the real vault rather than here. Recorded rather than
    // papered over: this test guards currency, not the explicit call.
    let files: Vec<(String, String)> = (0..400)
        .map(|i| {
            (
                format!("10-projects/note-{i:04}.md"),
                format!(
                    "# Note {i}
type:: project
"
                ),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();

    let dir = tempfile::tempdir().expect("temp");
    let report = serat_index::rebuild_with_queue_depth(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        dir.path(),
        8,
    )
    .expect("rebuild");

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let recorded: String = conn
        .query_row(
            "SELECT stat FROM sqlite_stat1 WHERE tbl = 'notes' AND idx IS NOT NULL LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("ANALYZE must leave statistics for notes");

    // sqlite_stat1.stat begins with the number of rows the index covers.
    let rows: usize = recorded
        .split_whitespace()
        .next()
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("unexpected sqlite_stat1 stat {recorded:?}"));

    assert_eq!(
        rows, report.notes,
        "the statistics describe {rows} notes but the index holds {}; ANALYZE ran          during the rebuild rather than after it",
        report.notes
    );
}

#[test]
fn the_full_text_index_is_searchable_after_a_rebuild() {
    let dir = tempfile::tempdir().expect("temp");
    rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH 'Indexing'",
            [],
            |row| row.get(0),
        )
        .expect("fts query");
    assert!(hits > 0, "the rebuild produced no searchable text");
}

#[test]
fn an_asset_indexes_without_being_parsed() {
    let dir = tempfile::tempdir().expect("temp");
    let parser = LineParser::default();
    rebuild(&vault_fixture(), &parser, dir.path()).expect("rebuild");

    assert_eq!(
        parser.calls.load(Ordering::Relaxed),
        2,
        "the parser was handed something that is not Markdown"
    );

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let kind: String = conn
        .query_row(
            "SELECT kind FROM notes WHERE path = '40-refs/contract.pdf'",
            [],
            |row| row.get(0),
        )
        .expect("asset row");
    assert_eq!(kind, "asset");
}

#[test]
fn a_rebuild_of_a_thousand_notes_indexes_all_of_them() {
    // NOTE, because the original name claimed more than this proves: 1,000 notes
    // against a queue depth of 4,096 never fills the queue, so this cannot tell a
    // blocking producer from a dropping one. I mutation-checked it — reverting the
    // rebuild to the non-blocking submit leaves this passing, which means the test
    // written to guard F-19 did not guard F-19 (F-35).
    //
    // The boundary case now lives in submit_blocking_delivers_every_operation_when_
    // the_queue_is_smaller_than_the_work, which sets the capacity small enough that
    // the refusal path is reached. This one stays as what it honestly is: a
    // multi-batch rebuild that must not lose anything.
    let files: Vec<(String, String)> = (0..1_000)
        .map(|i| {
            (
                format!("10-projects/note-{i:04}.md"),
                format!("# Note {i}\ntype:: project\nstatus:: active\n"),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();

    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        dir.path(),
    )
    .expect("rebuild");

    assert_eq!(report.notes, 1_000, "the rebuild lost notes");
    assert_eq!(report.skipped, 0);
    assert_eq!(report.fields, 2_000);
}

// ---------------------------------------------------------------------------
// The actor
// ---------------------------------------------------------------------------

#[test]
fn flush_means_the_write_is_visible_to_a_new_reader() {
    // What flush() actually promises, and what NFR-PRF-03's "reflected in the index"
    // has to mean: a reader that starts after flush returns sees the row.
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let actor = IndexActor::open(&path).expect("open");
    let writer = actor.handle();

    // Volume, deliberately. With one operation the writer has almost certainly
    // committed before the reader gets there, so the assertion passes whether or not
    // flush() does anything — I mutation-checked the single-operation version by
    // making flush() return Ok(()) immediately, and it still passed while two other
    // tests failed. A guard that only passes because of what the writer happens to
    // have finished is not guarding flush (F-35).
    //
    // Two thousand operations across four batches cannot all have landed by the time
    // submit returns, so an absent barrier shows up as a short count.
    for i in 0..2_000 {
        writer
            .submit(IndexOp::UpsertAsset {
                meta: meta(&format!("40-refs/{i:04}.pdf")),
            })
            .expect("submit");
    }
    writer.flush().expect("flush");

    let conn = schema::open(&path).expect("reader");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        count, 2_000,
        "a reader opened after flush returned saw only {count} of 2000 writes"
    );
}

#[test]
fn operations_are_applied_in_the_order_they_were_submitted() {
    // One writer, one queue: an upsert followed by a remove must not reorder into a
    // remove followed by an upsert.
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let actor = IndexActor::open(&path).expect("open");
    let writer = actor.handle();

    for _ in 0..50 {
        writer
            .submit(IndexOp::UpsertAsset {
                meta: meta("a.pdf"),
            })
            .expect("submit");
        writer
            .submit(IndexOp::Remove {
                path: VaultPath::new("a.pdf").expect("path"),
            })
            .expect("submit");
    }
    writer.flush().expect("flush");

    let conn = schema::open(&path).expect("reader");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 0, "the last operation was a remove");
}

#[test]
fn many_producers_share_one_writer_without_a_lock() {
    // ADR-0001's claim in one test: several threads submit concurrently, nothing
    // coordinates, and every write lands. If this needed a mutex the design would be
    // wrong.
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let actor = IndexActor::open(&path).expect("open");

    std::thread::scope(|scope| {
        for producer in 0..4 {
            let writer = actor.handle();
            scope.spawn(move || {
                for i in 0..100 {
                    writer
                        .submit_blocking(IndexOp::UpsertAsset {
                            meta: meta(&format!("assets/p{producer}-{i}.pdf")),
                        })
                        .expect("submit");
                }
            });
        }
    });

    actor.handle().flush().expect("flush");
    let conn = schema::open(&path).expect("reader");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 400);
}

#[test]
fn work_queued_before_shutdown_is_applied_rather_than_discarded() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    {
        let actor = IndexActor::open(&path).expect("open");
        let writer = actor.handle();
        for i in 0..20 {
            writer
                .submit(IndexOp::UpsertAsset {
                    meta: meta(&format!("assets/{i}.pdf")),
                })
                .expect("submit");
        }
        actor.shutdown().expect("shutdown");
    }

    let conn = schema::open(&path).expect("reader");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 20, "shutdown discarded queued work");
}

#[test]
fn submitting_to_a_stopped_writer_is_an_error_rather_than_a_panic() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let actor = IndexActor::open(&path).expect("open");
    let writer = actor.handle();
    drop(actor);

    assert!(matches!(
        writer.submit(IndexOp::UpsertAsset {
            meta: meta("a.pdf")
        }),
        Err(serat_core::IndexError::WriterStopped)
    ));
    assert!(matches!(
        writer.flush(),
        Err(serat_core::IndexError::WriterStopped)
    ));
}

#[test]
fn a_rename_keeps_inbound_links_that_a_delete_would_have_severed() {
    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");
    assert_eq!(report.links, 2);

    let path = rebuild_path(dir.path());
    let actor = IndexActor::open(&path).expect("open");
    let writer = actor.handle();
    writer
        .submit(IndexOp::Rename {
            from: VaultPath::new("20-systems/indexing.md").expect("path"),
            to: VaultPath::new("30-notes/indexing.md").expect("path"),
        })
        .expect("submit");
    writer.flush().expect("flush");
    drop(actor);

    let conn = schema::open(&path).expect("reader");
    let links: i64 = conn
        .query_row("SELECT count(*) FROM links", [], |row| row.get(0))
        .expect("count");
    assert_eq!(links, 2, "a rename lost links");

    let moved: i64 = conn
        .query_row(
            "SELECT count(*) FROM notes WHERE path = '30-notes/indexing.md'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(moved, 1);
}

// ---------------------------------------------------------------------------

fn rebuild_path(dir: &std::path::Path) -> std::path::PathBuf {
    serat_index::index_path(dir)
}

// ---------------------------------------------------------------------------
// The producer asymmetry (ADR-0011), at a capacity where it can actually bite
// ---------------------------------------------------------------------------

#[test]
fn submit_blocking_delivers_every_operation_when_the_queue_is_smaller_than_the_work() {
    // F-19's real guard. The queue holds four; the producer sends five hundred. A
    // non-blocking submit refuses most of them, so if this passes with a dropping
    // producer, the test is worthless — which is exactly what was wrong with the
    // 1,000-note version against a 4,096-deep queue.
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    let actor = IndexActor::with_capacity(conn, 4);
    let writer = actor.handle();

    for i in 0..500 {
        writer
            .submit_blocking(IndexOp::UpsertAsset {
                meta: meta(&format!("assets/{i:04}.pdf")),
            })
            .expect("submit_blocking must not refuse");
    }
    actor.shutdown().expect("shutdown");

    let conn = schema::open(&path).expect("reader");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 500, "a blocking producer lost work");
}

#[test]
fn submit_refuses_rather_than_waiting_when_the_queue_is_full() {
    // The other half of ADR-0011's asymmetry, and the one that protects the argument
    // rather than a line of code: if a later change made both producers block, the
    // watcher's coalescer would stop draining the OS event buffer and the platform
    // would discard notifications without telling us.
    //
    // The writer is held off with a barrier so the queue cannot drain underneath us.
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    let actor = IndexActor::with_capacity(conn, 2);
    let writer = actor.handle();

    let mut refused = None;
    let started = std::time::Instant::now();
    for i in 0..64 {
        if let Err(err) = writer.submit(IndexOp::UpsertAsset {
            meta: meta(&format!("assets/{i}.pdf")),
        }) {
            refused = Some(err);
            break;
        }
    }

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "submit appears to be waiting for space; it must refuse"
    );
    assert!(
        matches!(refused, Some(serat_core::IndexError::QueueFull { .. })),
        "submit never refused against a queue of two, so the refusal path is untested"
    );
}

#[test]
fn a_rebuild_loses_nothing_when_it_outruns_the_writer() {
    // The guard F-19 actually needed, and the one the 1,000-note test only appeared
    // to be. The queue holds four and the rebuild submits five hundred, so the
    // refusal path is reached on nearly every operation: with a dropping submit the
    // rebuild completes successfully and indexes a fraction of the vault, which is
    // the failure a wall-clock benchmark number cannot see.
    let files: Vec<(String, String)> = (0..500)
        .map(|i| {
            (
                format!("10-projects/note-{i:04}.md"),
                format!("# Note {i}\ntype:: project\n"),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();

    let dir = tempfile::tempdir().expect("temp");
    let report = serat_index::rebuild_with_queue_depth(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        dir.path(),
        4,
    )
    .expect("rebuild");

    assert_eq!(
        report.notes, 500,
        "the rebuild lost notes under backpressure"
    );
    assert_eq!(report.skipped, 0, "the rebuild skipped notes it could read");
}

// ---------------------------------------------------------------------------
// The rebuild sequence: bulk load, link resolution, phase timings
// ---------------------------------------------------------------------------

#[test]
fn the_rebuild_leaves_the_fts_triggers_in_place() {
    // The rebuild drops the triggers to bulk load and restores them from
    // sqlite_master. If it forgot the restore, the bulk index would be correct and
    // every later save would be silently absent from search — so the check that
    // matters is that an insert made *after* the rebuild is findable.
    let dir = tempfile::tempdir().expect("temp");
    rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");

    let path = rebuild_path(dir.path());
    let conn = schema::open(&path).expect("open");

    let triggers: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master \
             WHERE type = 'trigger' AND tbl_name = 'blocks'",
            [],
            |row| row.get(0),
        )
        .expect("count triggers");
    assert_eq!(triggers, 3, "the rebuild did not restore the FTS triggers");

    conn.execute(
        "INSERT INTO notes (id, path, kind, mtime, hash) \
         VALUES (99, 'later.md', 'note', 0, 'h')",
        [],
    )
    .expect("note");
    conn.execute(
        "INSERT INTO blocks (note_id, ord, kind, text) \
         VALUES (99, 0, 'para', 'written after the rebuild')",
        [],
    )
    .expect("block");

    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH 'afterwards OR rebuild'",
            [],
            |row| row.get(0),
        )
        .expect("fts");
    assert!(hits > 0, "a save after the rebuild is not searchable");

    conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
        .expect("the bulk-built index must be internally consistent");
}

#[test]
fn links_are_resolved_once_every_note_exists() {
    // The second pass the foreign key forces. A link can name a note that has not been
    // inserted yet, so dst_note starts NULL and is filled at the end.
    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(&vault_fixture(), &LineParser::default(), dir.path()).expect("rebuild");

    assert_eq!(report.links, 2);
    assert_eq!(
        report.links_resolved, 2,
        "the two mutual links between the fixture's notes were not resolved"
    );

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let unresolved: i64 = conn
        .query_row(
            "SELECT count(*) FROM links WHERE dst_note IS NULL AND kind <> 'tag'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(unresolved, 0, "links remained unresolved after the rebuild");
}

#[test]
fn a_link_to_a_note_that_does_not_exist_stays_unresolved_rather_than_wrong() {
    // The half that matters more: resolution must leave a dangling link NULL, not
    // point it at whatever happened to be nearby. A link to a note nobody has written
    // yet is how people write.
    let vault = FakeVault::with(&[(
        "10-projects/serat.md",
        "# Serat\n[[20-systems/does-not-exist]]\n",
    )]);
    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(&vault, &LineParser::default(), dir.path()).expect("rebuild");

    assert_eq!(report.links, 1);
    assert_eq!(
        report.links_resolved, 0,
        "a dangling link was resolved to something"
    );

    let conn = schema::open(&rebuild_path(dir.path())).expect("open");
    let dst: Option<i64> = conn
        .query_row("SELECT dst_note FROM links", [], |row| row.get(0))
        .expect("link row");
    assert_eq!(dst, None);
}

#[test]
fn the_report_breaks_the_rebuild_into_phases() {
    // F-10. A single wall-clock number cannot settle open design question 1, which
    // architecture §1.2 says the P0 benchmark answers: reported as one figure, "the
    // writer dominates" and "reading the vault off disk dominates" are
    // indistinguishable while implying completely different fixes.
    let files: Vec<(String, String)> = (0..200)
        .map(|i| {
            (
                format!("10-projects/note-{i:04}.md"),
                format!("# Note {i}\ntype:: project\nstatus:: active\n"),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();

    let dir = tempfile::tempdir().expect("temp");
    let report = rebuild(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        dir.path(),
    )
    .expect("rebuild");

    assert_eq!(report.notes, 200);
    assert!(
        report.read_hash > std::time::Duration::ZERO,
        "the read phase was not measured"
    );
    assert!(
        report.parse > std::time::Duration::ZERO,
        "the parse phase was not measured"
    );
    // walk and write can legitimately round to zero on a fast in-memory fixture, so
    // asserting they are non-zero would be asserting the clock's resolution rather
    // than the code. What must hold is that no phase exceeds the whole.
    let total = report.walk + report.read_hash + report.parse + report.write;
    assert!(
        total < std::time::Duration::from_secs(60),
        "the phases sum to {total:?}, which is not plausible for 200 notes"
    );
}
