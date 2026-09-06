//! The save-to-index path (NFR-PRF-03) and the parse pool.

mod support;

use std::time::Duration;

use serat_core::{IndexOp, IndexWriter as _, NoteSource as _, VaultPath};
use serat_index::{IndexHandle, parse_pool_threads, rebuild, schema};
use support::{FakeVault, LineParser, note_body, vault_fixture};

fn path(text: &str) -> VaultPath {
    VaultPath::new(text).expect("path")
}

fn note_count(index_dir: &std::path::Path) -> i64 {
    let conn = schema::open(&serat_index::index_path(index_dir)).expect("open");
    conn.query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count")
}

#[test]
fn a_save_is_visible_to_a_reader_once_the_writer_has_been_flushed() {
    // NFR-PRF-03's interval, and the only reading of "reflected in the index" a
    // benchmark can honestly take: committed and visible, not merely queued.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();
    let handle = IndexHandle::open(dir.path()).expect("open");

    handle
        .apply(&vault, &parser, &echo, &path("10-projects/serat.md"))
        .expect("save");
    // The composition the ruling requires: apply submits, flush waits, and the wait is
    // visible at the call site rather than hidden inside a verb.
    handle.writer().flush().expect("flush");

    assert_eq!(
        note_count(dir.path()),
        1,
        "a reader could not see the note after apply and flush"
    );
    handle.close().expect("close");
}

#[test]
fn note_count_reads_the_invariant_the_open_check_already_trusts() {
    // contracts/ipc.rs's IndexState::Ready.note_count, and #65's blocker: nothing on
    // IndexHandle answered this without a caller re-deriving it and risking drift from
    // the count schema::inspect's CountMismatch check already stands behind.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();
    let handle = IndexHandle::open(dir.path()).expect("open");

    handle
        .apply(&vault, &parser, &echo, &path("10-projects/serat.md"))
        .expect("save");
    handle.writer().flush().expect("flush");

    assert_eq!(
        handle.note_count().expect("note_count"),
        1,
        "note_count did not see the flushed save"
    );
    assert_eq!(
        handle.note_count().expect("note_count") as i64,
        note_count(dir.path()),
        "note_count disagreed with a plain count(*) over notes"
    );
    handle.close().expect("close");
}

#[test]
fn a_save_does_not_wait_for_the_writer() {
    // Design goal 2, and the reason apply_and_wait does not exist. A save that blocked
    // on the writer would put the index back on the keystroke path. The queue is one
    // deep and the writer is given work it has not drained, so a waiting apply would
    // block here rather than return.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();
    let handle = IndexHandle::with_queue_depth(dir.path(), 1).expect("open");

    let started = std::time::Instant::now();
    for _ in 0..64 {
        // Some of these will be refused with QueueFull, which is backpressure and the
        // save path's cue to let a rescan find the file — not a reason to wait.
        let _ = handle.apply(&vault, &parser, &echo, &path("10-projects/serat.md"));
    }
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "64 saves against a one-deep queue took {elapsed:?}; apply is waiting for the          writer, which puts the index back on the keystroke path"
    );
    handle.close().expect("close");
}

#[test]
fn saving_the_same_note_twice_replaces_its_rows_rather_than_doubling_them() {
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();
    let handle = IndexHandle::open(dir.path()).expect("open");

    let target = path("10-projects/serat.md");
    handle
        .apply(&vault, &parser, &echo, &target)
        .expect("first");
    handle.writer().flush().expect("flush");
    let blocks_after_first = {
        let conn = schema::open(&serat_index::index_path(dir.path())).expect("open");
        conn.query_row("SELECT count(*) FROM blocks", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("count")
    };

    handle
        .apply(&vault, &parser, &echo, &target)
        .expect("second");
    handle.writer().flush().expect("flush");
    let conn = schema::open(&serat_index::index_path(dir.path())).expect("open");
    let blocks: i64 = conn
        .query_row("SELECT count(*) FROM blocks", [], |row| row.get(0))
        .expect("count");

    assert_eq!(note_count(dir.path()), 1);
    assert_eq!(
        blocks, blocks_after_first,
        "a second save doubled the blocks"
    );
    handle.close().expect("close");
}

#[test]
fn a_save_of_a_file_that_has_gone_removes_it() {
    // The save path is told about a path, not about an event kind. If the file is not
    // there, that is a delete — which is how the watcher's path-only queue works too.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();
    let handle = IndexHandle::open(dir.path()).expect("open");

    let target = path("10-projects/serat.md");
    handle.apply(&vault, &parser, &echo, &target).expect("save");
    handle.writer().flush().expect("flush");
    assert_eq!(note_count(dir.path()), 1);

    let emptied = FakeVault::with(&[]);
    handle
        .apply(&emptied, &parser, &echo, &target)
        .expect("removal");
    handle.writer().flush().expect("flush");
    assert_eq!(
        note_count(dir.path()),
        0,
        "a save of a vanished file did not remove its row"
    );
    handle.close().expect("close");
}

#[test]
fn a_save_after_a_rebuild_updates_the_note_rather_than_adding_one() {
    // The two paths meet here: a full rebuild, then an incremental save of a note the
    // rebuild already indexed. Getting this wrong gives a vault that grows a duplicate
    // every time anyone saves.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();

    let report = rebuild(
        &vault,
        &parser,
        std::path::Path::new("C:/vaults/serat"),
        dir.path(),
    )
    .expect("rebuild");
    assert_eq!(report.notes, 3);

    let handle = IndexHandle::open(dir.path()).expect("open");
    handle
        .apply(&vault, &parser, &echo, &path("10-projects/serat.md"))
        .expect("save");
    handle.writer().flush().expect("flush");

    assert_eq!(note_count(dir.path()), 3, "the save added a duplicate note");
    handle.close().expect("close");
}

#[test]
fn a_save_keeps_the_full_text_index_in_step() {
    // The incremental path keeps the FTS triggers, unlike the rebuild which drops
    // them. If a save stopped updating blocks_fts, search would silently return the
    // previous version of every edited note.
    let dir = tempfile::tempdir().expect("temp");
    let vault = FakeVault::with(&[("note.md", "# Before\nthe original wording\n")]);
    let parser = LineParser::default();
    let echo = support::SpyEcho::default();
    let handle = IndexHandle::open(dir.path()).expect("open");
    let target = path("note.md");

    handle.apply(&vault, &parser, &echo, &target).expect("save");
    handle.writer().flush().expect("flush");

    let amended = FakeVault::with(&[("note.md", "# After\nthe amended wording\n")]);
    handle
        .apply(&amended, &parser, &echo, &target)
        .expect("save");
    handle.writer().flush().expect("flush");

    let conn = schema::open(&serat_index::index_path(dir.path())).expect("open");
    let stale: i64 = conn
        .query_row(
            "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH 'original'",
            [],
            |row| row.get(0),
        )
        .expect("fts");
    let current: i64 = conn
        .query_row(
            "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH 'amended'",
            [],
            |row| row.get(0),
        )
        .expect("fts");

    assert_eq!(stale, 0, "search still matches the previous version");
    assert_eq!(current, 1, "search does not match the saved version");
    conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
        .expect("consistent");
    handle.close().expect("close");
}

#[test]
fn parse_pool_threads_leaves_a_core_free() {
    // Architecture §2.2: cores minus one. Parsing is CPU-bound, and a pool sized to
    // every core competes with the thread the user is typing on — design goal 2 says
    // nothing may block the keystroke, and a rebuild that saturates the machine does.
    //
    // This sizes a pool without yet feeding one: `rebuild()` runs sequentially today
    // (a separate, already-merged I4 implementation this branch does not revisit), so
    // the function is tested standalone rather than through `rebuild`'s output.
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let threads = parse_pool_threads();

    assert!(
        threads >= 1,
        "cores minus one must have at least one thread"
    );
    if cores > 1 {
        assert_eq!(
            threads,
            cores - 1,
            "parse_pool_threads took every core; §2.2 says cores minus one"
        );
    } else {
        assert_eq!(threads, 1, "a single-core machine still needs one parser");
    }
}

#[test]
fn two_rebuilds_of_the_same_notes_produce_the_same_index() {
    // FR-VLT-02: delete the index, rebuild, get the same answers. Two hundred notes
    // is enough to make a shape-dependent bug (e.g. a batch boundary) show up.
    let files: Vec<(String, String)> = (0..200)
        .map(|i| (format!("10-projects/note-{i:04}.md"), note_body(i)))
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();

    let dir = tempfile::tempdir().expect("temp");
    let first = rebuild(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        std::path::Path::new("C:/vaults/serat"),
        dir.path(),
    )
    .expect("first");

    let second = rebuild(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        std::path::Path::new("C:/vaults/serat"),
        dir.path(),
    )
    .expect("second");

    assert_eq!(first.notes, 200);
    assert_eq!(
        (
            first.notes,
            first.blocks,
            first.fields,
            first.links,
            first.tasks
        ),
        (
            second.notes,
            second.blocks,
            second.fields,
            second.links,
            second.tasks
        ),
        "two rebuilds of the same notes produced different indexes"
    );
}

#[test]
fn the_rebuild_preserves_the_order_the_walk_produced() {
    // Rowids follow insertion order, and a rebuild that inserted out of the walk's
    // order would make "delete the index, rebuild, get the same answers" (FR-VLT-02)
    // far harder to assert than it needs to be.
    let files: Vec<(String, String)> = (0..300)
        .map(|i| (format!("10-projects/note-{i:04}.md"), note_body(i)))
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();

    let dir = tempfile::tempdir().expect("temp");
    rebuild(
        &FakeVault::with(&borrowed),
        &LineParser::default(),
        std::path::Path::new("C:/vaults/serat"),
        dir.path(),
    )
    .expect("rebuild");

    let conn = schema::open(&serat_index::index_path(dir.path())).expect("open");
    let mut stmt = conn
        .prepare("SELECT path FROM notes ORDER BY id")
        .expect("prepare");
    let by_rowid: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .filter_map(Result::ok)
        .collect();

    let mut sorted = by_rowid.clone();
    sorted.sort();
    assert_eq!(
        by_rowid, sorted,
        "the pool committed notes out of the order the walk produced"
    );
}

#[test]
fn a_writer_handle_from_the_open_index_shares_the_same_actor() {
    // The handle keeps the index open across saves, which is most of what makes 50 ms
    // plausible — opening SQLite and checking the schema costs more than one note's
    // work. A second actor over the same file would also be a second writer, which
    // ADR-0001 exists to prevent.
    let dir = tempfile::tempdir().expect("temp");
    let handle = IndexHandle::open(dir.path()).expect("open");

    let writer = handle.writer();
    writer
        .submit(IndexOp::UpsertAsset {
            meta: support::meta("a.pdf"),
        })
        .expect("submit");
    writer.flush().expect("flush");

    assert_eq!(note_count(dir.path()), 1);
    handle.close().expect("close");
}

#[test]
fn the_source_and_parser_arrive_as_traits_so_the_index_links_neither_crate() {
    // Not a behavioural test: it is the dependency rule paying for itself. This file
    // drives the whole save path with a Vec-backed vault and a line-based parser, so
    // serat-index is exercised end to end without serat-vault or serat-parse existing.
    let vault = vault_fixture();
    let entries = vault.entries().expect("entries");
    assert_eq!(entries.len(), 3);
}
