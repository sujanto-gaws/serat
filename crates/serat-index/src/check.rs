//! `PRAGMA quick_check`, on a connection of its own (task #69; NFR-REL-01, NFR-REL-02,
//! C-02; ADR-0002, ADR-0011, ADR-0001).
//!
//! The page-level check architecture §3.4 names as the background and on-demand home of
//! what the open stamp cannot see (a 4 KB overwrite in the middle of the file passes all
//! six stamp steps). It never runs on the open path: it reads the whole file (1,150 ms on
//! the 20k index, §3.4).
//!
//! * **Read-only, and never the writer's connection.** It opens its own
//!   `SQLITE_OPEN_READ_ONLY` connection with `query_only`, so it takes no part in
//!   ADR-0001's single writer and **does not take the cross-process writer lock** (task
//!   #154, [`crate::lock`]): it may run beside a live writer, in this process or another.
//! * **Coexists with the checkpointer.** It is one read transaction for the length of
//!   one call and none after, the same discipline every reader keeps (a long-held read is
//!   what pins the WAL); it does not hold the file across calls.
//! * **Two ways to say corrupt, one outcome.** `quick_check` sometimes raises
//!   ("database disk image is malformed") and sometimes returns findings as rows, where a
//!   healthy database is the single row `ok` (§3.4). Both are [`QuickCheck::Problems`].
//!   It never panics on a damaged page, and a failure that says nothing about the file's
//!   contents (it is missing, locked) is an `Err`.
//!
//! What a caller does with `Problems` is ADR-0002's one answer: rebuild.

use std::path::Path;

use rusqlite::{Connection, ErrorCode, OpenFlags};
use serat_core::IndexError;

/// The outcome of [`quick_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickCheck {
    /// SQLite found nothing wrong.
    Clean,
    /// SQLite reported damage: its findings, or the error it raised, as text. Never empty.
    Problems(Vec<String>),
}

/// Run `PRAGMA quick_check` on the index in `index_dir`, over a read-only connection this
/// function opens and closes itself.
///
/// # Errors
///
/// [`IndexError::Sqlite`] if the database cannot be opened or read for a reason that says
/// nothing about corruption (no such file, a lock that did not clear in the busy
/// timeout). A malformed database is `Ok(Problems)`, not an error.
pub fn quick_check(index_dir: &Path) -> Result<QuickCheck, IndexError> {
    let path = crate::rebuild::index_path(index_dir);
    let conn = match Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(conn) => conn,
        Err(err) if corrupt(&err) => return Ok(QuickCheck::Problems(vec![err.to_string()])),
        Err(err) => return Err(crate::schema::sqlite(err)),
    };
    run(&conn)
}

fn run(conn: &Connection) -> Result<QuickCheck, IndexError> {
    if let Err(err) = conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;") {
        return if corrupt(&err) {
            Ok(QuickCheck::Problems(vec![err.to_string()]))
        } else {
            Err(crate::schema::sqlite(err))
        };
    }
    let rows: Result<Vec<String>, rusqlite::Error> = conn
        .prepare("PRAGMA quick_check")
        .and_then(|mut stmt| stmt.query_map([], |row| row.get::<_, String>(0))?.collect());
    match rows {
        Ok(rows) if rows.len() == 1 && rows[0] == "ok" => Ok(QuickCheck::Clean),
        Ok(rows) if rows.is_empty() => Ok(QuickCheck::Problems(vec![
            "quick_check returned no rows".to_owned(),
        ])),
        Ok(rows) => Ok(QuickCheck::Problems(rows)),
        Err(err) if corrupt(&err) => Ok(QuickCheck::Problems(vec![err.to_string()])),
        Err(err) => Err(crate::schema::sqlite(err)),
    }
}

/// Whether `err` is SQLite saying the file is damaged or is not a database.
fn corrupt(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}
