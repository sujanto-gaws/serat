//! The WAL invariant (architecture §2.2 "WAL checkpoints", task #56): **no reader
//! connection holds a read transaction outside one read call.**
//!
//! A `PASSIVE` checkpoint cannot pass a reader's snapshot, so one leaked read
//! transaction on the long-lived search connection would pin the WAL for ever. These
//! tests assert `sqlite3_txn_state(db, NULL) == SQLITE_TXN_NONE` (rusqlite's
//! `Connection::transaction_state`) on the search connection and on a reader after every
//! read type that exists here, on the paths that leave a statement mid-flight: a search
//! that returns `Superseded`, an interrupted statement, and a read that returned an error.
//!
//! R5 (`Reader::attachments`) is in the list in `every_read_type_leaves_no_transaction_open`,
//! which is the one place to extend when a read is added.

use std::sync::atomic::Ordering;

use rusqlite::{Connection, TransactionState};

use super::*;
use crate::schema;

fn state(conn: &Connection) -> TransactionState {
    conn.transaction_state::<&str>(None).expect("txn state")
}

fn search_state(sc: &SearchConn) -> TransactionState {
    sc.with_connection(state)
}

fn reader_state(reader: &Reader) -> TransactionState {
    reader.with_connection(state)
}

/// An index with a note that has fields and a link, so every read has something to step
/// over.
fn db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("index.db");
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    conn.execute_batch(
        "INSERT INTO notes (path, file_name, path_fold, file_name_fold, title, kind, type, mtime, size, hash)
         VALUES ('a.md', 'a.md', 'a.md', 'a.md', 'A', 'note', 'note', 0, 0, 'h'),
                ('b.md', 'b.md', 'b.md', 'b.md', 'B', 'note', 'note', 0, 0, 'h2');
         INSERT INTO blocks (note_id, ord, kind, text, byte_start, byte_end)
         SELECT 1, i, 'para', 'commonword filler ' || i, 0, 1
           FROM (WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 3000)
                 SELECT i FROM n);
         INSERT INTO fields (note_id, key, val_text, source)
         VALUES (1, 'status', 'open', 'frontmatter');
         INSERT INTO links (src_note, dst_path, dst_fold, dst_note, kind)
         VALUES (2, 'a', 'a', 1, 'wiki');",
    )
    .expect("rows");
    (dir, path)
}

#[test]
fn every_read_type_leaves_no_transaction_open() {
    let (_dir, path) = db();
    let sc = SearchConn::open(&path).expect("search");
    // The search connection is the long-lived one that could leak.
    let on_search = Reader::open(&path).expect("reader").with_search(sc.clone());
    // A reader that does everything on its own connection.
    let alone = Reader::open(&path).expect("reader");

    // (name, the read). R1, R1 + R3, R2, R3 (type form and field form).
    type Read = Box<dyn Fn(&Reader)>;
    let reads: Vec<(&str, Read)> = vec![
        (
            "R1",
            Box::new(|r| {
                assert!(matches!(
                    r.search("commonword", 0, 50),
                    Ok(SearchOutcome::Hits(h)) if !h.is_empty()
                ));
            }),
        ),
        (
            "R1 + R3 (type)",
            Box::new(|r| {
                assert!(matches!(
                    r.search("commonword [type:note]", 0, 50),
                    Ok(SearchOutcome::Hits(h)) if !h.is_empty()
                ));
            }),
        ),
        (
            "R1 + R3 (field)",
            Box::new(|r| {
                assert!(matches!(
                    r.search("commonword [status:open]", 0, 50),
                    Ok(SearchOutcome::Hits(h)) if !h.is_empty()
                ));
            }),
        ),
        (
            "R2",
            Box::new(|r| {
                assert_eq!(r.backlinks("a.md").expect("R2").len(), 1);
            }),
        ),
        (
            "R3 (type)",
            Box::new(|r| {
                assert!(matches!(
                    r.search("[type:note]", 0, 50),
                    Ok(SearchOutcome::Hits(h)) if h.len() == 2
                ));
            }),
        ),
        (
            "R5",
            Box::new(|r| {
                r.attachments().expect("R5");
            }),
        ),
        (
            "R3 (field)",
            Box::new(|r| {
                assert!(matches!(
                    r.search("[status:open]", 0, 50),
                    Ok(SearchOutcome::Hits(h)) if h.len() == 1
                ));
            }),
        ),
    ];

    for (name, read) in &reads {
        read(&on_search);
        assert_eq!(
            search_state(&sc),
            TransactionState::None,
            "{name} left a read transaction on the search connection"
        );
        assert_eq!(
            reader_state(&on_search),
            TransactionState::None,
            "{name} left a read transaction on the reader's own connection"
        );
        read(&alone);
        assert_eq!(
            reader_state(&alone),
            TransactionState::None,
            "{name} left a read transaction on a reader"
        );
    }
}

#[test]
fn a_superseded_search_leaves_no_transaction_open() {
    let (_dir, path) = db();
    let sc = SearchConn::open(&path).expect("search");
    let reader = Reader::open(&path).expect("reader").with_search(sc.clone());
    // The generation moves after the statement is prepared and before its first step:
    // the progress handler stops it mid-statement (an interrupted statement), and the
    // call returns Superseded.
    let latest = sc.latest.clone();
    sc.set_after_prepare(move || {
        latest.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(
        reader.search("commonword", 0, 50).expect("search"),
        SearchOutcome::Superseded
    );
    assert_eq!(search_state(&sc), TransactionState::None);

    // A generation bumped before the call is not a supersession of it: the call takes its
    // own, newer, number and runs to completion.
    sc.latest.fetch_add(1, Ordering::SeqCst);
    sc.set_after_prepare(|| {});
    assert!(matches!(
        reader.search("commonword", 0, 50),
        Ok(SearchOutcome::Hits(_))
    ));
    assert_eq!(search_state(&sc), TransactionState::None);
}

#[test]
fn an_interrupted_statement_that_runs_again_leaves_no_transaction_open() {
    let (_dir, path) = db();
    let sc = SearchConn::open(&path).expect("search");
    let reader = Reader::open(&path).expect("reader").with_search(sc.clone());
    // A stale interrupt() lands on the newest statement: SQLITE_INTERRUPT, then the
    // loop in `run` executes it again.
    let racer = sc.clone();
    let fired = std::sync::atomic::AtomicBool::new(false);
    sc.set_after_prepare(move || {
        if !fired.swap(true, Ordering::SeqCst) {
            racer.interrupt.interrupt();
        }
    });
    assert!(matches!(
        reader.search("commonword", 0, 50),
        Ok(SearchOutcome::Hits(_))
    ));
    assert_eq!(search_state(&sc), TransactionState::None);
}

#[test]
fn a_read_that_returned_an_error_leaves_no_transaction_open() {
    let (_dir, path) = db();
    let sc = SearchConn::open(&path).expect("search");
    let on_search = Reader::open(&path).expect("reader").with_search(sc.clone());
    let alone = Reader::open(&path).expect("reader");
    // Warm both so the statements are prepared and cached, then break the index under
    // them: the FTS5 data table is gone, so the step (not only the prepare) fails.
    assert!(matches!(
        on_search.search("commonword", 0, 50),
        Ok(SearchOutcome::Hits(_))
    ));
    assert!(matches!(
        alone.search("commonword", 0, 50),
        Ok(SearchOutcome::Hits(_))
    ));
    let writer = Connection::open(&path).expect("writer");
    writer
        .execute_batch("PRAGMA writable_schema = ON; DROP TABLE blocks_fts_data;")
        .expect("break the index");

    // Whatever the outcome is (an error, or FTS5's "nothing to search for" mapping),
    // the connection must not be left inside a transaction by it.
    let first = on_search.search("commonword", 0, 50);
    assert!(
        !matches!(first, Ok(SearchOutcome::Hits(_))),
        "the broken index still answered: {first:?}"
    );
    assert_eq!(search_state(&sc), TransactionState::None);
    assert_eq!(reader_state(&on_search), TransactionState::None);
    let second = alone.search("commonword", 0, 50);
    assert!(!matches!(second, Ok(SearchOutcome::Hits(_))), "{second:?}");
    assert_eq!(reader_state(&alone), TransactionState::None);

    // R2 against a table that is missing outright.
    writer
        .execute_batch("DROP TABLE links;")
        .expect("drop links");
    assert!(alone.backlinks("a.md").is_err());
    assert_eq!(reader_state(&alone), TransactionState::None);
}

#[test]
fn the_assertion_can_fail_a_half_stepped_statement_is_a_read_transaction() {
    // The control: without it, "None everywhere" could be a state() that never reports
    // anything else. A statement stepped once and not finished holds a read transaction
    // (the leak the invariant exists to rule out); dropping it ends it.
    let (_dir, path) = db();
    let sc = SearchConn::open(&path).expect("search");
    sc.with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM blocks ORDER BY id")
            .expect("prepare");
        let mut rows = stmt.query([]).expect("query");
        assert!(rows.next().expect("step").is_some());
        assert_eq!(state(conn), TransactionState::Read);
        drop(rows);
        assert_eq!(state(conn), TransactionState::None);
    });
}
