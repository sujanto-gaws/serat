//! The tree's warm path: every indexed path and its kind, in one read
//! (ADR-0015 §2; FR-VLT-01).
//!
//! One prepared statement of fixed shape over `notes(path, kind)` on its own read-only
//! connection: a read, never a request to the writer (ADR-0001). Grouping the rows into
//! per-directory listings is a pure function in `serat-core` (`group_listings`), not SQL,
//! because the order is Rust's collation and the front end must never disagree with it.
//!
//! Its own file, and its own connection, so it adds no state to [`crate::Reader`].

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serat_core::{IndexError, TreeKind};

use crate::schema::sqlite;

/// The statement. `kind` is `'note'` or `'asset'` (schema CHECK); nothing else is read.
pub const TREE_ROWS: &str = "SELECT path, kind FROM notes";

/// Every `(path, kind)` the index holds, unordered, from the index database at
/// `db_path`.
///
/// # Errors
///
/// [`IndexError::Sqlite`] if the database cannot be opened or read.
pub fn read_tree_rows(db_path: &Path) -> Result<Vec<(String, TreeKind)>, IndexError> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite)?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")
        .map_err(sqlite)?;
    let mut statement = conn.prepare(TREE_ROWS).map_err(sqlite)?;
    let rows = statement
        .query_map([], |row| {
            let path: String = row.get(0)?;
            let kind: String = row.get(1)?;
            let kind = if kind == "asset" {
                TreeKind::Asset
            } else {
                TreeKind::Note
            };
            Ok((path, kind))
        })
        .map_err(sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sqlite)
}
