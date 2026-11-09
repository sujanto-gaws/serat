//! The foreign-key child indexes of user_version 7 (architecture §4.4, task #51).
//!
//! Serves **NFR-PRF-03**. `EXPLAIN QUERY PLAN` does not show foreign-key actions, so a
//! plan check cannot catch their absence; the statement's own `FULLSCAN_STEP` counter
//! can, because the cascade runs as sub-programs of the delete. The second test first
//! proves the counter sees them (with the four indexes dropped it is non-zero), on the
//! SQLite this binary links; the first asserts it is zero with them.

use rusqlite::{Connection, StatementStatus, params};
use serat_index::schema;

const INDEXES: [&str; 4] = [
    "ix_links_src_block",
    "ix_fields_block",
    "ix_tasks_block",
    "ix_tasks_note",
];

/// 60 notes of 13 blocks; each block carries links, inline fields and tasks.
/// Note 1 is the one deleted.
fn fixture() -> Connection {
    let conn = schema::open_in_memory().expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    conn.execute_batch(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 60)
         INSERT INTO notes(id, path, kind, mtime, size, hash)
           SELECT i, 'n' || i || '.md', 'note', 0, 0, 'h' FROM n;
         WITH RECURSIVE b(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM b WHERE i < 780)
         INSERT INTO blocks(id, note_id, ord, kind, text)
           SELECT i, (i - 1) / 13 + 1, (i - 1) % 13, 'para', 't' FROM b;
         WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r WHERE i < 6000)
         INSERT INTO links(src_note, src_block, dst_path, dst_fold, kind)
           SELECT (i - 1) % 780 / 13 + 1, (i - 1) % 780 + 1, 'x', 'x', 'wiki' FROM r;
         WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r WHERE i < 6000)
         INSERT INTO fields(note_id, block_id, key, val_text, source)
           SELECT (i - 1) % 780 / 13 + 1, (i - 1) % 780 + 1, 'k', 'v', 'inline' FROM r;
         WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r WHERE i < 6000)
         INSERT INTO tasks(note_id, block_id, status, done)
           SELECT (i - 1) % 780 / 13 + 1, (i - 1) % 780 + 1, ' ', 0 FROM r;",
    )
    .expect("fixture");
    conn
}

/// Delete note 1 inside a transaction that is rolled back; return the statement's
/// full-scan step count.
fn delete_fullscan_steps(conn: &Connection) -> i32 {
    conn.execute_batch("PRAGMA foreign_keys = ON; BEGIN")
        .expect("begin");
    let mut stmt = conn
        .prepare("DELETE FROM notes WHERE id = ?1")
        .expect("prepare");
    assert_eq!(stmt.execute(params![1]).expect("delete"), 1);
    let steps = stmt.get_status(StatementStatus::FullscanStep);
    drop(stmt);
    conn.execute_batch("ROLLBACK").expect("rollback");
    steps
}

#[test]
fn deleting_a_note_scans_no_child_table_once_the_block_foreign_keys_are_indexed() {
    let conn = fixture();
    for name in INDEXES {
        let found: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [name],
                |r| r.get(0),
            )
            .expect("master");
        assert_eq!(found, 1, "{name} must be in the delivered DDL");
    }
    let steps = delete_fullscan_steps(&conn);
    assert_eq!(
        steps, 0,
        "deleting a 13-block note scanned a child table: a foreign-key child column \
         has lost its index (architecture §4.4, NFR-PRF-03)"
    );
}

#[test]
fn the_counter_can_fail_it_sees_the_cascade_without_the_indexes() {
    let conn = fixture();
    for name in INDEXES {
        conn.execute_batch(&format!("DROP INDEX {name};"))
            .expect("drop");
    }
    let steps = delete_fullscan_steps(&conn);
    eprintln!("fullscan steps without the indexes: {steps}");
    assert!(
        steps > 0,
        "FULLSCAN_STEP did not count the cascade's scans on the bundled SQLite, so the \
         zero in the other test proves nothing; fall back to a timing assertion \
         (architecture §4.4)"
    );
}
