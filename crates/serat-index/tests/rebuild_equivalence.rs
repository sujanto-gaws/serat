//! T1 (task #97): FR-VLT-02's literal acceptance criterion.
//!
//! "Given a fully indexed vault, when .serat/ is deleted and the app restarted,
//! then every query returns identical results after rebuild." Also serves
//! NFR-REL-02 (the index is recoverable by rebuild alone, no user action beyond
//! restarting) and ADR-0002 (there is no repair path — a stale or missing index
//! is always thrown away and rebuilt from the files, never migrated in place).
//! US-01 and US-07 are the stories this is the acceptance test for.
//!
//! What makes this a different test from `a_rebuild_over_an_existing_index_
//! replaces_it_rather_than_merging` in `rebuild.rs`: that test compares
//! `RebuildReport` *counts* (five integers) across two rebuilds into the same
//! directory, without ever removing it. Counts agreeing is necessary and not
//! sufficient — two rebuilds could produce the same number of links with a
//! different one left unresolved, or the same number of fields with one
//! holding the wrong value, and a count-only comparison would not see either.
//! This test instead deletes the whole index directory (`.serat/`, or in this
//! harness the temp directory standing in for it — deliberately the directory
//! itself and not just `index.db`, matching the requirement's own wording),
//! rebuilds through the real "app restarted" entry point (`open_or_rebuild`,
//! not `rebuild` directly), and diffs the full content of every table: every
//! note's declared type, every block's text and heading path, every field's
//! typed value, every link's resolution (including which ones stayed
//! unresolved), every task's status, and what full-text search finds. Rows are
//! compared through stable natural keys (note path, block ord, field key) —
//! never through the `INTEGER PRIMARY KEY` surrogate ids, which a rebuild is
//! free to reassign and which asserting on would make this test fail for a
//! reason the requirement does not care about.
//!
//! `rebuild.rs:196-207` names the one bug class no other test in this crate
//! can see: `IndexOp::Rename` skips the scoped link-resolve that an upsert
//! gets, relying only on the rebuild's safety-net `resolve_links()` pass to
//! catch it. A bug in the scoped calls would leave the *incremental* path's
//! index one rebuild away from disagreeing with itself — invisible to every
//! test that only ever rebuilds once. This test rebuilds twice and diffs.

mod support;

use std::fs;

use serat_index::open_or_rebuild;
use support::{FakeVault, LineParser, vault_fixture};

/// A vault shaped to exercise every table this test diffs: two notes with
/// mutual links and inline fields (from [`vault_fixture`]), plus one more note
/// with a link to nothing (an unresolved link must round-trip as unresolved,
/// not silently vanish or attach to the wrong note) and a second open task.
fn fixture() -> FakeVault {
    let mut vault = vault_fixture();
    vault.files.push((
        serat_core::VaultPath::new("10-projects/orphan.md").expect("path"),
        b"# Orphan\ntype:: project\n[[nowhere/does-not-exist]]\n- [ ] a second open task\n"
            .to_vec(),
    ));
    vault
}

/// Every row of every table a rebuild populates, keyed by something a rebuild
/// has no license to change — never by the surrogate id, which two rebuilds
/// are not obliged to assign the same way.
#[derive(Debug, PartialEq)]
struct Snapshot {
    notes: Vec<String>,
    blocks: Vec<String>,
    fields: Vec<String>,
    links: Vec<String>,
    tasks: Vec<String>,
    /// A representative full-text query, by the note it lands on rather than
    /// a bare count — so a hit moving from the right note to the wrong one
    /// would not read as "unchanged".
    fts_hits: Vec<String>,
}

fn snapshot(index_dir: &std::path::Path) -> Snapshot {
    let conn = serat_index::schema::open(&serat_index::index_path(index_dir)).expect("open");

    let query = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).expect("prepare");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect()
    };

    let notes = query(
        "SELECT path || '|' || file_name || '|' || IFNULL(title, '<none>') || '|' \
             || IFNULL(type, '<none>') || '|' || kind
         FROM notes ORDER BY path",
    );

    let blocks = query(
        "SELECT notes.path || '|' || blocks.ord || '|' || blocks.kind || '|' \
             || IFNULL(blocks.text, '<none>') || '|' || IFNULL(blocks.heading_path, '<none>')
         FROM blocks JOIN notes ON notes.id = blocks.note_id
         ORDER BY notes.path, blocks.ord",
    );

    let fields = query(
        "SELECT notes.path || '|' || fields.key || '|' \
             || IFNULL(fields.val_text, '<none>') || '|' || IFNULL(fields.val_num, '<none>') \
             || '|' || IFNULL(fields.val_date, '<none>') || '|' || IFNULL(fields.val_bool, '<none>') \
             || '|' || fields.source || '|' || IFNULL(blocks.ord, '<none>')
         FROM fields
         JOIN notes ON notes.id = fields.note_id
         LEFT JOIN blocks ON blocks.id = fields.block_id
         ORDER BY notes.path, fields.key, IFNULL(blocks.ord, -1)",
    );

    // dst_note is joined back to the target's path (or '<unresolved>') rather than
    // compared as an id, which is the whole point: two rebuilds must resolve a link
    // to the same note by content, not by coincidentally reusing the same integer.
    let links = query(
        "SELECT src.path || '|' || links.kind || '|' || IFNULL(links.dst_path, '<none>') \
             || '|' || IFNULL(dst.path, '<unresolved>')
         FROM links
         JOIN notes AS src ON src.id = links.src_note
         LEFT JOIN notes AS dst ON dst.id = links.dst_note
         ORDER BY src.path, links.dst_path",
    );

    let tasks = query(
        "SELECT notes.path || '|' || blocks.ord || '|' || tasks.status || '|' \
             || tasks.done || '|' || IFNULL(tasks.text, '<none>')
         FROM tasks
         JOIN notes ON notes.id = tasks.note_id
         JOIN blocks ON blocks.id = tasks.block_id
         ORDER BY notes.path, blocks.ord",
    );

    let fts_hits = query(
        "SELECT notes.path || '|' || blocks.ord
         FROM blocks_fts
         JOIN blocks ON blocks.id = blocks_fts.rowid
         JOIN notes ON notes.id = blocks.note_id
         WHERE blocks_fts MATCH 'Serat OR Indexing OR Orphan'
         ORDER BY notes.path, blocks.ord",
    );

    Snapshot {
        notes,
        blocks,
        fields,
        links,
        tasks,
        fts_hits,
    }
}

#[test]
fn every_query_returns_identical_results_after_deleting_and_rebuilding_the_index() {
    // FR-VLT-02's acceptance criterion, verbatim: "Given a fully indexed vault, when
    // .serat/ is deleted and the app restarted, then every query returns identical
    // results after rebuild."
    let dir = tempfile::tempdir().expect("temp");
    let vault = fixture();
    let parser = LineParser::default();

    let (handle, report) = open_or_rebuild(
        std::path::Path::new("C:/vaults/serat"),
        dir.path(),
        &vault,
        &parser,
        &|_| {},
    )
    .expect("first open");
    handle.close().expect("close");
    assert_eq!(
        report.map(|r| r.notes),
        Some(4),
        "fixture grew without this test noticing"
    );

    let before = snapshot(dir.path());

    // Not asserting each table is non-empty individually — a snapshot equal on two
    // empty sets either side of a no-op would pass and prove nothing. Guard the whole
    // fixture landed before trusting the comparison that follows.
    assert!(!before.notes.is_empty());
    assert!(!before.blocks.is_empty());
    assert!(!before.fields.is_empty());
    assert!(!before.links.is_empty());
    assert!(!before.tasks.is_empty());
    assert!(
        !before.fts_hits.is_empty(),
        "the fixture's own text must be findable before the deletion this test is about"
    );
    // The dangling link from the fixture's third note must survive the round trip
    // as unresolved, not merely as present.
    assert!(
        before.links.iter().any(|row| row.ends_with("<unresolved>")),
        "the fixture's dangling link did not index as unresolved: {:?}",
        before.links
    );

    // ".serat/ is deleted" — the whole directory, not just index.db, which is the
    // requirement's own wording and a strictly harder case than truncating or
    // removing the database file alone (already covered by NFR-REL-02's tests in
    // open_or_rebuild.rs). rebuild()'s own create_dir_all(parent) is what has to
    // recreate it; nothing in this test does that by hand.
    fs::remove_dir_all(dir.path()).expect("delete .serat/");
    assert!(
        !dir.path().exists(),
        "the directory this test deleted is still there"
    );

    // "the app restarted" — the real entry point, not rebuild() called directly, so
    // this exercises the same absent-index detection open_or_rebuild.rs tests rather
    // than assuming it.
    let (handle, report) = open_or_rebuild(
        std::path::Path::new("C:/vaults/serat"),
        dir.path(),
        &vault,
        &parser,
        &|_| {},
    )
    .expect("second open, after .serat/ was deleted");
    handle.close().expect("close");
    assert_eq!(
        report.map(|r| r.notes),
        Some(4),
        "the rebuild after deletion did not reindex the whole vault"
    );

    let after = snapshot(dir.path());

    assert_eq!(before.notes, after.notes, "notes disagree after rebuild");
    assert_eq!(before.blocks, after.blocks, "blocks disagree after rebuild");
    assert_eq!(before.fields, after.fields, "fields disagree after rebuild");
    assert_eq!(
        before.links, after.links,
        "links disagree after rebuild — including, possibly, which ones resolved"
    );
    assert_eq!(before.tasks, after.tasks, "tasks disagree after rebuild");
    assert_eq!(
        before.fts_hits, after.fts_hits,
        "full-text search disagrees after rebuild"
    );

    // The whole point stated as one assertion, so a future reader of a failure sees
    // the requirement, not just five separate table diffs.
    assert_eq!(
        before, after,
        "FR-VLT-02: deleting .serat/ and rebuilding must return identical results, and did not"
    );
}
