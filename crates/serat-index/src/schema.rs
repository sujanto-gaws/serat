//! Creating and validating the index database.
//!
//! There is no migration path and there will not be one (ADR-0002). A database whose
//! schema does not match this build is deleted and rebuilt, which is a direct
//! consequence of the index being derived: every row in it can be recomputed from
//! files on disk, so the cheapest correct response to any doubt is to throw it away.

use std::path::Path;

use rusqlite::Connection;
use serat_core::IndexError;

/// The DDL, compiled into the binary.
///
/// `include_str!` rather than a runtime read: the schema is not configuration, and a
/// shipped binary that looked for a `.sql` file beside itself would have a failure
/// mode ("someone deleted the schema file") that cannot exist if the bytes are in the
/// executable. It also means the version test below compares the constant against the
/// exact text this build will execute, not against whatever is on disk.
pub const DDL: &str = include_str!("../../../schema/001_init.sql");

/// The index version this build expects.
///
/// Named `INDEX_VERSION` rather than `SCHEMA_VERSION` (F-134/F-171, ruled at F-134):
/// the stamp is bumped both when the DDL changes *and* when the indexer starts
/// extracting something different from unchanged input — a parser that recognises a
/// new inline field, a block-kind mapping that stops dropping HTML, a different
/// tokenizer. A name that says only "schema" is why that second case went unnoticed
/// for as long as it did. One number covers both; there is no separate
/// derivation-only constant.
///
/// Bumping it is the whole of a change in either category: a database reporting any
/// other value is deleted and rebuilt. It must equal the `PRAGMA user_version` in
/// [`DDL`], which is asserted by a test rather than remembered by two people — under
/// ADR-0002 a silent drift costs every user a full reindex.
pub const INDEX_VERSION: i64 = 6;

/// What an existing database turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexState {
    /// Schema matches and the last rebuild completed. Usable as-is.
    Ready,
    /// Nothing there. Build it.
    Absent,
    /// Every check passed, and `index_meta.vault_root` is not the root being opened.
    ///
    /// A value, not a fault (§3.4 step 5, task #120). `notes.path` is vault-relative,
    /// so an index recorded under another root may still describe this tree exactly —
    /// and until F-459 this was classified `Unusable` and bought a full rebuild on the
    /// strength of two strings differing. Deciding what it means needs the vault, which
    /// this function does not have; the caller does.
    RootChanged {
        /// What the index says it was built from, for the report. `None` if the column
        /// is null, which is what an index built before this column was written looks
        /// like.
        recorded: Option<String>,
    },
    /// Present but unusable: wrong version, failed integrity check, or a rebuild that
    /// never finished. Every one of these has the same answer (ADR-0002), which is why
    /// they are one variant with a reason rather than three.
    Unusable {
        /// Why, for the log and for the UI's "rebuilding because…" line.
        reason: UnusableReason,
    },
}

/// Why an index could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusableReason {
    /// Built by a different index version — the DDL or what the indexer extracts from
    /// unchanged input, or both (F-134).
    IndexVersion,
    /// A row this check needs to read (`index_meta.state`, `.vault_root` or the
    /// `notes`/`index_meta` count pair) could not be read at all.
    ///
    /// Not `PRAGMA integrity_check` (task #61): that ran on this path once, at
    /// 1310ms against NFR-PRF-01's 400ms window — three times the whole budget,
    /// and rejected for exactly that cost in architecture.md §3.4's four-candidate
    /// table before this line was written to run it anyway. C-02 is why the
    /// residual is acceptable here: the corruption this variant no longer catches
    /// degrades to wrong query results until the next rebuild, never to a lost
    /// byte, because the index is disposable and the vault is not. §3.4 still
    /// wants that coverage back — as a background or on-demand `quick_check`, not
    /// on the open path — and #58 has that as a named home with nothing built in
    /// it yet.
    Corrupt,
    /// `state` is still 'building': a rebuild was interrupted.
    RebuildInterrupted,
    /// `note_count` disagrees with `count(*)` on notes.
    ///
    /// The DDL makes this an invariant maintained inside each transaction, so a
    /// mismatch means a transaction was torn — which is corruption that
    /// `integrity_check` cannot see, because the file is structurally fine and
    /// semantically wrong.
    CountMismatch,
    /// The file exists but SQLite will not open it as a database.
    Unopenable,
}

/// Open a connection with the pragmas every connection needs.
///
/// # Errors
///
/// [`IndexError::Sqlite`] if the file cannot be opened or the pragmas rejected.
pub fn open(path: &Path) -> Result<Connection, IndexError> {
    let conn = Connection::open(path).map_err(sqlite)?;
    configure(&conn)?;
    Ok(conn)
}

/// An in-memory database, for tests.
///
/// # Errors
///
/// [`IndexError::Sqlite`] if SQLite refuses.
pub fn open_in_memory() -> Result<Connection, IndexError> {
    let conn = Connection::open_in_memory().map_err(sqlite)?;
    configure(&conn)?;
    Ok(conn)
}

/// How many prepared statements one connection keeps.
///
/// `apply` prepares its inserts through the statement cache, because rusqlite's
/// `execute` re-prepares on every call and those calls sit in per-row loops — a 20k
/// rebuild was re-parsing the same eight SQL strings about a million times, which
/// measured as 16 s of a 21 s writer (F-324).
///
/// Set here rather than left to rusqlite's default of 16, because the failure mode
/// of outgrowing it is silent: the ninth, seventeenth or thirty-third distinct
/// statement does not error, it evicts another one and quietly returns the writer to
/// re-preparing on every call. Eight are cached today; this leaves room to add
/// without the addition being the thing that undoes the fix.
const STATEMENT_CACHE: usize = 32;

/// The pragmas, applied to every connection, writer and reader alike.
fn configure(conn: &Connection) -> Result<(), IndexError> {
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )
    .map_err(sqlite)
}

/// Create every table, index and trigger.
///
/// # Errors
///
/// [`IndexError::Sqlite`] if any statement fails.
pub fn bootstrap(conn: &Connection) -> Result<(), IndexError> {
    conn.execute_batch(DDL).map_err(sqlite)
}

/// Create the schema if this database is empty.
///
/// Idempotent, so opening an existing index is not a special case for the caller.
///
/// # Errors
///
/// [`IndexError::Sqlite`] if the check or the DDL fails.
pub fn bootstrap_if_empty(conn: &Connection) -> Result<(), IndexError> {
    let has_tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'notes'",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    if has_tables == 0 {
        bootstrap(conn)?;
    }
    Ok(())
}

/// Decide whether an existing database can be used.
///
/// Everything that is not `Ready` leads to the same action — delete and rebuild — so
/// this deliberately does not try to repair anything.
///
/// # Errors
///
/// Never returns an error for a bad database; a database it cannot make sense of is
/// [`IndexState::Unusable`], which is a value and not a failure.
pub fn inspect(path: &Path) -> Result<IndexState, IndexError> {
    inspect_for(path, None)
}

/// As [`inspect`], and also compare the recorded identity against `vault_root`.
///
/// §3.4 step 5. `vault_root` is the *canonical* root ([`crate::canonical_root`]), not
/// whatever spelling a caller holds — recording one and comparing the other is F-459.
/// Passing `None` skips the comparison, which is what a caller with no vault in hand —
/// a test, or a tool inspecting a database — actually wants.
///
/// A difference is reported as [`IndexState::RootChanged`] and **is not**
/// [`IndexState::Unusable`]: since task #120 nothing rebuilds on this alone. See
/// [`crate::identity`] for what does.
///
/// # Errors
///
/// As [`inspect`].
pub fn inspect_for(path: &Path, vault_root: Option<&str>) -> Result<IndexState, IndexError> {
    if !path.exists() {
        return Ok(IndexState::Absent);
    }

    let unusable = |reason| Ok(IndexState::Unusable { reason });

    let Ok(conn) = open(path) else {
        return unusable(UnusableReason::Unopenable);
    };

    let version: Result<i64, _> = conn.query_row("PRAGMA user_version", [], |row| row.get(0));
    match version {
        Ok(found) if found == INDEX_VERSION => {}
        Ok(_) => return unusable(UnusableReason::IndexVersion),
        Err(_) => return unusable(UnusableReason::Unopenable),
    }

    let state: Result<String, _> =
        conn.query_row("SELECT state FROM index_meta WHERE id = 1", [], |row| {
            row.get(0)
        });
    match state.as_deref() {
        Ok("ready") => {}
        Ok(_) => return unusable(UnusableReason::RebuildInterrupted),
        Err(_) => return unusable(UnusableReason::Corrupt),
    }

    // §3.4 step 5. Read here and decided last: since task #120 a root that differs is
    // not a fault, so it may not short-circuit the count check below — that one IS a
    // fault, and letting a spelling difference hide a torn transaction would trade one
    // silent failure for a worse one.
    let root_change = match vault_root {
        None => None,
        Some(expected) => {
            let recorded: Result<Option<String>, _> = conn.query_row(
                "SELECT vault_root FROM index_meta WHERE id = 1",
                [],
                |row| row.get(0),
            );
            match recorded {
                Ok(Some(found)) if found == expected => None,
                Ok(recorded) => Some(recorded),
                Err(_) => return unusable(UnusableReason::Corrupt),
            }
        }
    };

    // The invariant the DDL asks serat-index to maintain: note_count is updated in
    // the same transaction as any change to the number of notes. A mismatch is a torn
    // transaction, which integrity_check cannot see because the file is structurally
    // fine and semantically wrong.
    let counts: Result<(i64, i64), _> = conn.query_row(
        "SELECT (SELECT count(*) FROM notes), (SELECT note_count FROM index_meta WHERE id = 1)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    );
    match counts {
        Ok((actual, recorded)) if actual == recorded => match root_change {
            None => Ok(IndexState::Ready),
            Some(recorded) => Ok(IndexState::RootChanged { recorded }),
        },
        Ok(_) => unusable(UnusableReason::CountMismatch),
        Err(_) => unusable(UnusableReason::Corrupt),
    }
}

pub(crate) fn sqlite(error: rusqlite::Error) -> IndexError {
    IndexError::Sqlite {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ddl_runs_as_delivered() {
        // architecture.md §4 claims "the full DDL is schema/001_init.sql and runs as
        // delivered". Until this test existed that was an assertion in a document.
        // The system sqlite3 CLI cannot run it — the one on PATH here is the Android
        // platform-tools build, which has no fts5 — so this proves the only thing
        // that matters: it runs against the SQLite this binary links.
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("the delivered DDL must run with no statement failing");
    }

    #[test]
    fn the_constant_and_the_ddl_agree_on_the_index_version() {
        // Under ADR-0002 a drift between these costs every user a full reindex, and
        // it would be discovered at runtime by the rebuild firing for no reason.
        let declared = DDL
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("PRAGMA user_version = ")?
                    .trim_end_matches(';')
                    .trim()
                    .parse::<i64>()
                    .ok()
            })
            .expect("schema/001_init.sql must set PRAGMA user_version");

        assert_eq!(
            declared, INDEX_VERSION,
            "schema/001_init.sql sets user_version = {declared} but INDEX_VERSION is \
             {INDEX_VERSION}. Bump both in the same commit (ADR-0002)."
        );

        // And the value the DDL actually leaves behind, not just the text of it.
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        let applied: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(applied, INDEX_VERSION);
    }

    #[test]
    fn the_bundled_sqlite_has_fts5() {
        // FR-NAV-02 (note bodies) and, in P2, FR-NAV-08 (the "phrase inside a PDF
        // findable from the note search box" gate) both sit on this. The bundled
        // build is not the host's SQLite, so this asserts a property of what we ship.
        let conn = open_in_memory().expect("open");
        conn.execute_batch("CREATE VIRTUAL TABLE probe USING fts5(body);")
            .expect("the bundled SQLite must have FTS5 compiled in");
    }

    #[test]
    fn foreign_keys_are_on_for_every_connection() {
        // Our bundled libsqlite3-sys happens to compile with DEFAULT_FOREIGN_KEYS, so
        // this is already 1 before we ask. We set it anyway and assert it here,
        // because depending on a vendored build flag is exactly what changes silently
        // at a version bump — and with it off, every ON DELETE CASCADE in the schema
        // becomes decoration and the FTS delete triggers never fire.
        let conn = open_in_memory().expect("open");
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("pragma");
        assert_eq!(enabled, 1);
    }

    #[test]
    fn the_fts_delete_path_keeps_the_index_in_step() {
        // architecture.md §4.4 flags this as the case that breaks quietly: an
        // external-content FTS table keeps returning hits for deleted rows unless the
        // delete trigger fires. The hard version is deleting the PARENT note and
        // letting ON DELETE CASCADE remove the block, because then the trigger only
        // runs if the cascade does.
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");

        conn.execute(
            "INSERT INTO notes (id, path, kind, mtime, hash) \
             VALUES (1, '10-projects/a.md', 'note', 0, 'h')",
            [],
        )
        .expect("insert note");
        conn.execute(
            "INSERT INTO blocks (id, note_id, ord, kind, text) \
             VALUES (1, 1, 0, 'para', 'the reconciler is idempotent')",
            [],
        )
        .expect("insert block");

        assert_eq!(
            fts_hits(&conn, "reconciler"),
            1,
            "insert trigger did not fire"
        );

        conn.execute("DELETE FROM notes WHERE id = 1", [])
            .expect("delete note");

        let blocks: i64 = conn
            .query_row("SELECT count(*) FROM blocks", [], |row| row.get(0))
            .expect("count");
        assert_eq!(blocks, 0, "ON DELETE CASCADE did not remove the block");
        assert_eq!(
            fts_hits(&conn, "reconciler"),
            0,
            "blocks_fts still matches text whose row no longer exists"
        );

        // Structural check as well as behavioural: an external-content index can
        // disagree with its content table without either looking wrong on its own.
        conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
            .expect("fts index must be internally consistent");
    }

    #[test]
    fn an_update_replaces_the_fts_row_rather_than_adding_one() {
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        conn.execute(
            "INSERT INTO notes (id, path, kind, mtime, hash) VALUES (1, 'a.md', 'note', 0, 'h')",
            [],
        )
        .expect("note");
        conn.execute(
            "INSERT INTO blocks (id, note_id, ord, kind, text) VALUES (1, 1, 0, 'para', 'before')",
            [],
        )
        .expect("block");
        conn.execute("UPDATE blocks SET text = 'after' WHERE id = 1", [])
            .expect("update");

        assert_eq!(fts_hits(&conn, "before"), 0, "the old text still matches");
        assert_eq!(fts_hits(&conn, "after"), 1, "the new text does not match");
        conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check');")
            .expect("consistent");
    }

    #[test]
    fn an_absent_index_is_absent_rather_than_an_error() {
        let dir = tempfile::tempdir().expect("temp");
        assert_eq!(
            inspect(&dir.path().join("index.db")).expect("inspect"),
            IndexState::Absent
        );
    }

    #[test]
    fn a_wrong_index_version_is_unusable() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        {
            let conn = open(&path).expect("open");
            bootstrap(&conn).expect("bootstrap");
            conn.execute_batch("UPDATE index_meta SET state = 'ready'; PRAGMA user_version = 99;")
                .expect("meddle");
        }
        assert_eq!(
            inspect(&path).expect("inspect"),
            IndexState::Unusable {
                reason: UnusableReason::IndexVersion
            }
        );
    }

    #[test]
    fn an_interrupted_rebuild_is_unusable() {
        // state stays 'building' until the first full rebuild commits, so a process
        // killed mid-rebuild leaves a database that must not be trusted.
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        {
            let conn = open(&path).expect("open");
            bootstrap(&conn).expect("bootstrap");
        }
        assert_eq!(
            inspect(&path).expect("inspect"),
            IndexState::Unusable {
                reason: UnusableReason::RebuildInterrupted
            }
        );
    }

    #[test]
    fn a_note_count_that_disagrees_with_the_notes_table_is_unusable() {
        // The invariant the DDL delegates to this crate. A mismatch is a torn
        // transaction: integrity_check passes, because the file is structurally fine
        // and semantically wrong.
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        {
            let conn = open(&path).expect("open");
            bootstrap(&conn).expect("bootstrap");
            conn.execute_batch(
                "INSERT INTO notes (id, path, kind, mtime, hash) \
                     VALUES (1, 'a.md', 'note', 0, 'h');
                 UPDATE index_meta SET state = 'ready', note_count = 7;",
            )
            .expect("meddle");
        }
        assert_eq!(
            inspect(&path).expect("inspect"),
            IndexState::Unusable {
                reason: UnusableReason::CountMismatch
            }
        );
    }

    #[test]
    fn a_file_that_is_not_a_database_is_unusable_rather_than_a_panic() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        std::fs::write(&path, b"this is not a database, it is a text file").expect("write");
        assert!(matches!(
            inspect(&path).expect("inspect"),
            IndexState::Unusable { .. }
        ));
    }

    fn fts_hits(conn: &Connection, needle: &str) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH ?1",
            [needle],
            |row| row.get(0),
        )
        .expect("fts query")
    }
}
