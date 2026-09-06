//! The bulk-load rebuild shape, and the one place `state` becomes ready.
//!
//! `schema/001_init.sql` describes the rebuild sequence — drop the `blocks_fts`
//! triggers, load, `INSERT INTO blocks_fts(blocks_fts) VALUES('rebuild')`, restore the
//! triggers from the strings read out of `sqlite_master` — as a comment. A comment
//! binds nobody, and the failure it guards against is a rebuild that is fast and
//! produces a subtly different search index, which nothing else in the suite notices.

use rusqlite::Connection;
use serat_index::{IndexActor, schema};

/// Blocks used by both halves of the comparison. Deliberately awkward: diacritics (the
/// tokenizer is `unicode61 remove_diacritics 2`), punctuation, casing, a term repeated
/// across blocks and a term that appears exactly once.
const BLOCKS: &[&str] = &[
    "the reconciler is idempotent",
    "Reconciler, again - with punctuation",
    "naive cafe facade",
    "NAIVE CAFE FACADE",
    "a term that appears exactly once: quinquagenarian",
    "wikilinks [[like this]] and embeds ![[like that]]",
    "trailing whitespace   ",
    "",
];

/// Queries the two indexes must answer identically.
const QUERIES: &[&str] = &[
    "reconciler",
    "idempotent",
    "naive",
    "cafe",
    "quinquagenarian",
    "wikilinks",
    "nonexistent",
];

const TRIGGERS: &[&str] = &["blocks_fts_ai", "blocks_fts_ad", "blocks_fts_au"];

fn open() -> Connection {
    let conn = schema::open_in_memory().expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    conn
}

/// Insert the corpus with the FTS triggers live — the incremental path.
fn load_through_triggers(conn: &Connection) {
    insert_rows(conn);
}

/// Insert the corpus the way a rebuild must: triggers off, then rebuild the index.
///
/// Step 2 reads the trigger text out of `sqlite_master` rather than keeping a second
/// copy of the DDL anywhere, so what is restored is identical to what was delivered by
/// construction rather than by a test that has to catch a stale copy.
fn load_in_bulk(conn: &Connection) -> Vec<String> {
    let definitions = trigger_definitions(conn);
    assert_eq!(
        definitions.len(),
        3,
        "the schema should carry three blocks triggers; found {}",
        definitions.len()
    );

    for trigger in TRIGGERS {
        conn.execute_batch(&format!("DROP TRIGGER {trigger};"))
            .expect("drop trigger");
    }

    insert_rows(conn);

    conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES('rebuild');")
        .expect("rebuild the fts index");

    for definition in &definitions {
        conn.execute_batch(&format!("{definition};"))
            .expect("restore trigger");
    }

    definitions
}

fn insert_rows(conn: &Connection) {
    conn.execute(
        "INSERT INTO notes (id, path, kind, mtime, hash) VALUES (1, 'a.md', 'note', 0, 'h')",
        [],
    )
    .expect("note");
    for (ord, text) in BLOCKS.iter().enumerate() {
        conn.execute(
            "INSERT INTO blocks (note_id, ord, kind, text) VALUES (1, ?1, 'para', ?2)",
            rusqlite::params![ord as i64, text],
        )
        .expect("block");
    }
}

fn hits(conn: &Connection, needle: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH ?1",
        [needle],
        |row| row.get(0),
    )
    .unwrap_or(-1)
}

fn trigger_definitions(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT sql FROM sqlite_master \
             WHERE type = 'trigger' AND tbl_name = 'blocks' ORDER BY name",
        )
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query");
    let mut found: Vec<String> = rows.filter_map(Result::ok).collect();
    found.sort();
    found
}

#[test]
fn the_bulk_load_and_the_trigger_path_produce_the_same_search_index() {
    let incremental = open();
    load_through_triggers(&incremental);

    let bulk = open();
    load_in_bulk(&bulk);

    for query in QUERIES {
        let (fast, slow) = (hits(&bulk, query), hits(&incremental, query));
        assert_eq!(
            fast, slow,
            "the two load paths disagree on {query:?}: bulk {fast} vs triggers {slow}"
        );
    }

    // Both must also be internally consistent. An external-content index can disagree
    // with its content table without either looking wrong on its own.
    for (name, conn) in [("bulk", &bulk), ("incremental", &incremental)] {
        conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
            .unwrap_or_else(|err| panic!("{name} fts index is inconsistent: {err}"));
    }
}

#[test]
fn the_restored_triggers_are_identical_to_the_delivered_ones() {
    // Reading the definitions out of sqlite_master and putting them back is what makes
    // "identical by construction" true. If a rebuild ever kept its own copy of the
    // trigger text instead, this is what catches it drifting.
    let conn = open();
    let before = trigger_definitions(&conn);
    let captured = load_in_bulk(&conn);
    let after = trigger_definitions(&conn);

    assert_eq!(
        before, captured,
        "captured definitions differ from delivered"
    );
    assert_eq!(after, before, "restored definitions differ from delivered");
}

#[test]
fn the_restored_triggers_still_fire_on_the_incremental_path() {
    // The half a search-equivalence check cannot see: triggers restored as text that
    // parses but never runs leave the bulk index correct and every later save silently
    // absent from search.
    let conn = open();
    load_in_bulk(&conn);

    conn.execute(
        "INSERT INTO blocks (note_id, ord, kind, text) VALUES (1, 99, 'para', 'afterwards')",
        [],
    )
    .expect("insert after restore");
    assert_eq!(
        hits(&conn, "afterwards"),
        1,
        "the insert trigger did not fire"
    );

    conn.execute("UPDATE blocks SET text = 'amended' WHERE ord = 99", [])
        .expect("update");
    assert_eq!(
        hits(&conn, "afterwards"),
        0,
        "the update trigger did not fire"
    );
    assert_eq!(hits(&conn, "amended"), 1);

    conn.execute("DELETE FROM notes WHERE id = 1", [])
        .expect("cascade delete");
    assert_eq!(hits(&conn, "amended"), 0, "the delete trigger did not fire");

    conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
        .expect("consistent after the round trip");
}

fn index_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .expect("query sqlite_master")
        > 0
}

#[test]
fn begin_bulk_load_drops_ix_links_unresolved_and_end_bulk_load_restores_it() {
    // F-373 / task #82. `apply::apply`'s `bulk_load` gate can decline the per-row
    // *query* against ix_links_unresolved, but not the per-row *write* SQLite performs
    // to keep a partial index in step with every insert into `links`. Dropping the
    // index around the bulk load and rebuilding it once, the same trade this file
    // already documents for the blocks_fts triggers, is what actually removes that
    // cost. This drives the real production path (`WriterHandle::begin_bulk_load` /
    // `end_bulk_load`), not the manual reimplementation `load_in_bulk` above uses to
    // isolate the trigger round trip.
    let dir = tempfile::tempdir().expect("temp");
    let path = serat_index::index_path(dir.path());
    let conn = schema::open(&path).expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    let actor = IndexActor::with_connection(conn);
    let writer = actor.handle();

    let reader = schema::open(&path).expect("reader");
    assert!(
        index_exists(&reader, "ix_links_unresolved"),
        "a freshly bootstrapped schema should carry ix_links_unresolved"
    );

    let dropped = writer.begin_bulk_load().expect("begin");
    assert!(
        !index_exists(&reader, "ix_links_unresolved"),
        "begin_bulk_load left ix_links_unresolved in place; every insert into links \
         during the load will pay to keep it in step"
    );

    writer.end_bulk_load(dropped).expect("end");
    assert!(
        index_exists(&reader, "ix_links_unresolved"),
        "end_bulk_load did not restore ix_links_unresolved"
    );

    actor.shutdown().expect("shutdown");
}

#[test]
fn exactly_one_place_in_the_crate_marks_the_index_ready() {
    // F-45. The rebuild self-heals across a crash only because one step is the sole
    // writer of the ready state: if the process dies between dropping the triggers and
    // restoring them, the state is still building and the next open rebuilds. A second
    // write added anywhere would silently remove crash recovery from the rebuild path,
    // and nothing else in the suite would fail.
    //
    // A structural property, so it is checked structurally. Better still is a shape no
    // second write can be added to (F-40); this is the guard worth having while that
    // shape is young.
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let needle: String = ["state=", "'", "ready", "'"].concat();
    let mut writers = Vec::new();

    for entry in std::fs::read_dir(&src).expect("read src") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let whole = std::fs::read_to_string(&path).expect("read");
        // Production code only. Test fixtures legitimately drive the state directly to
        // build the unusable-index cases schema.rs checks, and counting those would
        // make the guard fail for the wrong reason. Every module in this crate puts
        // its tests last, so truncating at the marker is exact rather than a guess.
        let text = match whole.find("#[cfg(test)]") {
            Some(marker) => &whole[..marker],
            None => &whole[..],
        };
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.replace(' ', "").contains(&needle) {
                writers.push(format!(
                    "{}:{}",
                    path.file_name().expect("name").to_string_lossy(),
                    number + 1
                ));
            }
        }
    }

    assert_eq!(
        writers.len(),
        1,
        "the index must be marked ready in exactly one place, so a crash mid-rebuild \
         leaves it building and the next open rebuilds. Found {} writers: {writers:?}",
        writers.len()
    );
}
