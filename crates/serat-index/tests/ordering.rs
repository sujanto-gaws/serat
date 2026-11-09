//! The actor's ordering guarantees, driven at the actor rather than through `rebuild`.
//!
//! F-74. Five guards in the writer were correct and unreachable: `rebuild()` calls
//! `flush()` before `end_bulk_load`, so the actor's internal drains are never entered
//! from the only caller, and every mutation of them passed the whole suite. They are
//! defence in depth against a hazard nothing demonstrated.
//!
//! These tests reach them the only way that works — by submitting operations and
//! deliberately **not** flushing, which is exactly what a future caller that forgets
//! will do. That is the point: the guards exist for the caller who does not know they
//! need to flush, so the tests have to be that caller.

mod support;

use std::sync::Arc;
use std::time::Duration;

use serat_core::{IndexError, IndexOp, IndexWriter as _, VaultPath};
use serat_index::{IndexActor, IndexState, UnusableReason, schema};
use support::{FakeVault, LineParser, meta};

fn open_index(dir: &std::path::Path) -> (IndexActor, std::path::PathBuf) {
    let path = serat_index::index_path(dir);
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    (IndexActor::with_connection(conn), path)
}

/// Submit a note with `count` blocks, each carrying a distinct searchable word.
fn submit_note(writer: &serat_index::WriterHandle, path: &str, count: usize) {
    let body: String = (0..count).map(|i| format!("blockmarker{i:04}\n")).collect();
    let vault = FakeVault::with(&[(path, body.as_str())]);
    let parser = LineParser::default();
    let vault_path = VaultPath::new(path).expect("path");
    let loaded = {
        use serat_core::NoteSource as _;
        vault.load(&vault_path).expect("load")
    };
    let parsed = {
        use serat_core::NoteParser as _;
        parser.parse(&vault_path, &loaded.bytes).expect("parse")
    };
    writer
        .submit(IndexOp::UpsertNote {
            meta: loaded.meta,
            parsed: Box::new(parsed),
        })
        .expect("submit");
}

fn fts_hits(conn: &rusqlite::Connection, needle: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH ?1",
        [needle],
        |row| row.get(0),
    )
    .expect("fts query")
}

#[test]
fn end_bulk_load_indexes_operations_that_were_still_queued() {
    // The hazard the drain guards: rows submitted but not yet applied when the FTS
    // 'rebuild' runs would not be in `blocks` yet, so the external-content index would
    // be built without them and every one of those blocks would be permanently absent
    // from search — with `blocks` itself perfectly correct, so nothing looks wrong.
    //
    // Deliberately no flush() before end_bulk_load. That is the whole test.
    let dir = tempfile::tempdir().expect("temp");
    let (actor, path) = open_index(dir.path());
    let writer = actor.handle();

    let triggers = writer.begin_bulk_load().expect("begin");
    submit_note(&writer, "10-projects/a.md", 40);
    writer.end_bulk_load(triggers).expect("end");
    actor.shutdown().expect("shutdown");

    let conn = schema::open(&path).expect("reader");
    let blocks: i64 = conn
        .query_row("SELECT count(*) FROM blocks", [], |row| row.get(0))
        .expect("count");
    assert_eq!(blocks, 40, "the note itself did not land");
    assert_eq!(
        fts_hits(&conn, "blockmarker0039"),
        1,
        "a block queued when end_bulk_load ran is absent from search"
    );
    conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
        .expect("fts must be consistent with its content table");
}

#[test]
fn begin_bulk_load_applies_operations_before_the_triggers_come_off() {
    // The mirror hazard. Rows still queued when the triggers are dropped would be
    // applied afterwards with no triggers live — so they would miss the incremental
    // path and, if end_bulk_load had already run, miss the bulk build too.
    let dir = tempfile::tempdir().expect("temp");
    let (actor, path) = open_index(dir.path());
    let writer = actor.handle();

    submit_note(&writer, "10-projects/early.md", 12);
    let triggers = writer.begin_bulk_load().expect("begin");

    let conn = schema::open(&path).expect("reader");
    let blocks: i64 = conn
        .query_row("SELECT count(*) FROM blocks", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        blocks, 12,
        "begin_bulk_load dropped the triggers with operations still queued"
    );

    writer.end_bulk_load(triggers).expect("end");
    actor.shutdown().expect("shutdown");
}

#[test]
fn resolve_links_drains_the_queue_before_querying_it() {
    // F-74's guard: resolve_links commits before running its own query, so unflushed
    // work is visible to it rather than invisible.
    //
    // Task #13 changed what this test can observe. apply::upsert_note now resolves a
    // link the moment both ends exist, as a side effect of the transaction that
    // inserts them — not held back for a caller who remembers to call resolve_links
    // separately. A same-batch pair like this one is therefore fully resolved by the
    // commit alone, and the explicit resolve_links query correctly finds nothing left
    // to do. That is progress, not a hole, but it does mean the resolved *count* this
    // test used to assert (2) no longer distinguishes "the guard ran" from "the guard
    // was skipped": either way, the final `flush` inside `shutdown()` below would
    // apply the pending ops regardless, and per-note resolution would then resolve
    // both links whether or not resolve_links's own drain ever ran. What still
    // isolates the guard is whether the commit happened *before* resolve_links
    // returns — which a separate connection, read right here rather than after
    // shutdown, can see directly.
    let dir = tempfile::tempdir().expect("temp");
    let (actor, path) = open_index(dir.path());
    let writer = actor.handle();

    let vault = FakeVault::with(&[
        ("10-projects/a.md", "# A\n[[10-projects/b]]\n"),
        ("10-projects/b.md", "# B\n[[10-projects/a]]\n"),
    ]);
    let parser = LineParser::default();
    for name in ["10-projects/a.md", "10-projects/b.md"] {
        let vault_path = VaultPath::new(name).expect("path");
        let loaded = {
            use serat_core::NoteSource as _;
            vault.load(&vault_path).expect("load")
        };
        let parsed = {
            use serat_core::NoteParser as _;
            parser.parse(&vault_path, &loaded.bytes).expect("parse")
        };
        writer
            .submit(IndexOp::UpsertNote {
                meta: loaded.meta,
                parsed: Box::new(parsed),
            })
            .expect("submit");
    }

    // No flush. If resolve_links queried before draining, a reader right after this
    // call would see an index with neither note in it yet.
    writer.resolve_links().expect("resolve");

    let reader = schema::open(&path).expect("reader");
    let notes: i64 = reader
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        notes, 2,
        "resolve_links returned before the queued notes were committed"
    );
    let unresolved: i64 = reader
        .query_row(
            "SELECT count(*) FROM links WHERE dst_note IS NULL AND kind <> 'tag'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(
        unresolved, 0,
        "a link between two notes committed in the same batch was not resolved"
    );

    actor.shutdown().expect("shutdown");
}

#[test]
fn a_barrier_arriving_mid_batch_is_answered_rather_than_lost() {
    // The deferred slot, which no test entered. A command taken off the channel while
    // the actor is draining a batch cannot be folded into that batch and cannot be put
    // back — a channel has no un-receive — so it is held for the next iteration.
    // Dropping it instead would drop its reply sender, and the caller waiting on that
    // reply gets WriterStopped from an actor that is running perfectly well.
    //
    // A queue of one guarantees the inner drain always has something waiting, so the
    // barrier reliably arrives while a batch is in flight.
    let dir = tempfile::tempdir().expect("temp");
    let path = serat_index::index_path(dir.path());
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    let actor = IndexActor::with_capacity(conn, 1);
    let writer = Arc::new(actor.handle());

    let producer = {
        let writer = Arc::clone(&writer);
        std::thread::spawn(move || {
            for i in 0..400 {
                writer
                    .submit_blocking(IndexOp::UpsertAsset {
                        meta: meta(&format!("assets/{i:04}.pdf")),
                    })
                    .expect("submit");
            }
        })
    };

    // Fire barriers into the middle of that burst.
    let mut answered = 0;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(1));
        match writer.resolve_links() {
            Ok(_) => answered += 1,
            Err(IndexError::WriterStopped) => {
                panic!(
                    "a barrier was dropped mid-batch; its caller was told the writer had stopped"
                )
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
    producer.join().expect("producer");

    assert_eq!(answered, 20);
    actor.shutdown().expect("shutdown");
}

#[test]
fn the_index_is_not_marked_ready_while_its_triggers_are_off() {
    // mark_ready last is what makes a crash mid-rebuild recoverable: die between
    // dropping the triggers and restoring them and the state is still 'building', so
    // the next open rebuilds. exactly_one_place_in_the_crate_marks_the_index_ready
    // pins where that happens; nothing pinned when.
    //
    // This is the observable form of "when": at the one moment a rebuild is least
    // recoverable — triggers off, rows half applied — an opener must not be told the
    // index is usable.
    let dir = tempfile::tempdir().expect("temp");
    let (actor, path) = open_index(dir.path());
    let writer = actor.handle();

    let triggers = writer.begin_bulk_load().expect("begin");
    submit_note(&writer, "10-projects/a.md", 5);
    writer.flush().expect("flush");

    assert_eq!(
        schema::inspect(&path).expect("inspect"),
        IndexState::Unusable {
            reason: UnusableReason::RebuildInterrupted
        },
        "the index reported itself usable while its FTS triggers were dropped; a crash \
         here would leave a database that opens and silently never updates search"
    );

    writer.end_bulk_load(triggers).expect("end");
    writer.mark_ready("vault", 0).expect("ready");
    actor.shutdown().expect("shutdown");

    assert_eq!(
        schema::inspect(&path).expect("inspect"),
        IndexState::Ready,
        "a completed rebuild left the index unusable"
    );
}
