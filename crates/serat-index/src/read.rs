//! The P1 read paths: four fixed-shape statements, no query language
//! (architecture §4.5; FR-NAV-02, FR-NAV-04, FR-STR-03, FR-BIN-02; C-03, ADR-0010).
//!
//! * **R1** note search: `blocks_fts MATCH` with a snippet and the block's location.
//! * **R2** backlinks: one entry per distinct (source note, source block).
//! * **R3** property lookup: `[key:value]`, a sidecar hit returned as its asset.
//! * **R4** the `type` filter of R3, which compares **exactly** (not case-folded) so
//!   that `ix_notes_type` stays usable.
//! * **R1 + R3** together.
//!
//! A statement's text is chosen from a closed set in this file; every string the user
//! typed is a bound parameter. Free text becomes an FTS5 query here in Rust, one
//! quoted string per word, so user text can never be FTS5 syntax.
//!
//! A [`Reader`] owns one read-only connection and is used from one thread; a read
//! pool is several `Reader`s (ADR-0001: never a connection behind a mutex). It never
//! waits for the writer, and before the index is `Ready` the caller does not make one
//! (`contracts/ipc.rs` rule 1).
//!
//! # The one exception: [`SearchConn`]
//!
//! Full-text search (R1, R1 + R3) runs on one dedicated read-only connection per open
//! vault, outside the pool, behind a `Mutex` (architecture §2.2 and §4.5). It is the one
//! connection shared between threads, because a newer search has to reach the older
//! one. At most one full-text search is in flight; each new one bumps a generation
//! counter, interrupts the connection, and waits for the lock. Lock rules: no other
//! lock is taken while the search lock is held (only atomics and the interrupt handle
//! are touched beside it), and a poisoned lock is recovered with `into_inner` because
//! the connection is read-only and a panic leaves no half-written state. R2, R3 and R4
//! stay on the pool and are never interrupted.

use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::{Connection, ErrorCode, InterruptHandle, OpenFlags, named_params};
use serat_core::IndexError;

use crate::schema::sqlite;

/// The most hits a search with free text returns (R1, R1 + R3): `SearchArgs::limit`'s
/// ceiling for the statements that rank.
pub const MAX_LIMIT: u16 = 50;

/// The most hits a property-only `[key:value]` search returns (R3). R3 has no ranking
/// to pay for, so the whole 500 comes back in one statement rather than ten pages
/// (architecture.md §4.5, ruled on task #15's measurement).
pub const MAX_PROPERTY_LIMIT: u16 = 500;

/// How many characters of a block a backlink excerpt keeps.
pub const EXCERPT_CHARS: usize = 300;

/// What a search produced. Degenerate input is an outcome, never an error.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchOutcome {
    /// Hits, best first. Empty is a valid answer.
    Hits(Vec<SearchHit>),
    /// The query tokenised to nothing (or FTS5 still rejected it, which is logged by
    /// the caller from [`Reader::fts_rejections`], never raised).
    NothingToSearch,
    /// More than one `[key:value]` term.
    TooManyPropertyTerms,
    /// An unbalanced `"`. No statement was run.
    UnbalancedQuote,
    /// A newer full-text search superseded this one (architecture §4.5). Not an error.
    Superseded,
}

/// Whether a hit is a note or an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// A Markdown note.
    Note,
    /// A binary, reached through its sidecar note.
    Asset,
}

/// Where a block is, as indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockLocation {
    /// `blocks.id`.
    pub block_id: i64,
    /// Position of the block in its note.
    pub ord: u32,
    /// Heading breadcrumb.
    pub heading_path: Option<String>,
    /// Byte range as indexed.
    pub byte_start: u64,
    /// End of that range.
    pub byte_end: u64,
    /// `notes.hash` the range was indexed from.
    pub note_hash: String,
}

/// One search or property-lookup hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Vault-relative path. For a sidecar hit, the **asset's** path.
    pub path: String,
    /// Title, when known.
    pub title: Option<String>,
    /// Note or asset.
    pub kind: HitKind,
    /// For an asset hit, its MIME type (from the extension).
    pub mime: Option<String>,
    /// For an asset hit, the sidecar note that matched.
    pub sidecar_path: Option<String>,
    /// The matching block for a full-text hit; `None` for a property-only hit.
    pub block: Option<BlockLocation>,
    /// Snippet with matches between U+0002 and U+0003. Build text nodes only (T-09).
    pub snippet: Option<String>,
    /// FTS5's BM25 rank (lower is better) for a full-text hit; `None` for a
    /// property-only hit.
    pub rank: Option<f64>,
    /// `notes.mtime` (ms) and `notes.size` of the note the block belongs to, as indexed:
    /// what a stat is compared with to say a hit may be stale (task #88, architecture
    /// §4.5). `None` for a property-only hit, which has no block and is never stat-ed.
    pub indexed: Option<IndexedStat>,
}

/// `notes.mtime` and `notes.size` as the index holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedStat {
    /// Milliseconds since the epoch, as serat-vault's walk records it.
    pub mtime_ms: i64,
    /// Bytes.
    pub size: i64,
}

/// One backlink entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    /// Vault-relative path of the linking note.
    pub source_path: String,
    /// Its title, when known.
    pub source_title: Option<String>,
    /// The linking block; `None` when the link is in frontmatter.
    pub block: Option<BlockLocation>,
    /// Block text cut to [`EXCERPT_CHARS`] characters; empty for frontmatter.
    pub excerpt: String,
}

/// One attachment, as link insertion needs it (FR-VLT-08; architecture §7.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Vault-relative path.
    pub path: String,
    /// MIME type (`assets.mime`, from `serat_core::mime_for_path`).
    pub mime: String,
    /// Size in bytes (`assets.bytes`).
    pub bytes: u64,
    /// Its sidecar note, if one exists (ADR-0005).
    pub sidecar_path: Option<String>,
    /// The shortest text that resolves back to this asset (§9.3), without brackets:
    /// the file name when the resolver would pick this file for it, else the path.
    /// `None` when neither the name nor the path resolves back to this asset.
    pub link_text: Option<String>,
    /// What inserting it writes: `![[text]]` for an image or PDF, `[[text]]` otherwise.
    pub link: Option<String>,
}

/// A search box string, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedQuery {
    /// An unbalanced `"`.
    UnbalancedQuote,
    /// More than one `[key:value]`.
    TooManyPropertyTerms,
    /// What remains: an FTS5 query (words and phrases ANDed, each quoted) and/or one
    /// property term. Both `None` means nothing to search for.
    Terms {
        /// The FTS5 query text, or `None` if there is no free text.
        fts: Option<String>,
        /// The `[key:value]` term.
        property: Option<(String, String)>,
    },
}

fn has_word_char(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// Parse the search box (architecture §4.5 "Search syntax").
///
/// Free words and `"phrases"`, ANDed; at most one `[key:value]`. A word or phrase with
/// no letter or digit in it is dropped; an unbalanced `"` is refused, not repaired.
#[must_use]
pub fn parse_query(input: &str) -> ParsedQuery {
    let chars: Vec<char> = input.chars().collect();
    let mut terms: Vec<String> = Vec::new();
    let mut properties: Vec<(String, String)> = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, terms: &mut Vec<String>| {
        if has_word_char(word) {
            terms.push(format!("\"{}\"", word.replace('"', "\"\"")));
        }
        word.clear();
    };

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            flush(&mut word, &mut terms);
            let Some(close) = chars[i + 1..].iter().position(|&x| x == '"') else {
                return ParsedQuery::UnbalancedQuote;
            };
            let phrase: String = chars[i + 1..i + 1 + close].iter().collect();
            if has_word_char(&phrase) {
                terms.push(format!("\"{phrase}\""));
            }
            i += close + 2;
        } else if c == '[' {
            let property = chars[i + 1..]
                .iter()
                .position(|&x| x == ']')
                .map(|close| {
                    (
                        close,
                        chars[i + 1..i + 1 + close].iter().collect::<String>(),
                    )
                })
                .and_then(|(close, inner)| {
                    let (key, value) = inner.split_once(':')?;
                    let (key, value) = (key.trim(), value.trim());
                    (!key.is_empty() && !value.is_empty())
                        .then(|| (close, (key.to_owned(), value.to_owned())))
                });
            if let Some((close, pair)) = property {
                flush(&mut word, &mut terms);
                properties.push(pair);
                i += close + 2;
            } else {
                // Not a property term: the bracket is punctuation.
                flush(&mut word, &mut terms);
                i += 1;
            }
        } else if c.is_whitespace() || c == ']' {
            flush(&mut word, &mut terms);
            i += 1;
        } else {
            word.push(c);
            i += 1;
        }
    }
    flush(&mut word, &mut terms);

    if properties.len() > 1 {
        return ParsedQuery::TooManyPropertyTerms;
    }
    ParsedQuery::Terms {
        fts: (!terms.is_empty()).then(|| terms.join(" ")),
        property: properties.pop(),
    }
}

// ---------------------------------------------------------------------------
// The closed set of statements. Composed from fragments at compile time, so each
// final text exists once, is cached per connection, and contains no user string.
// ---------------------------------------------------------------------------

/// R3's filter for `[type:x]` (R4): exact, so `ix_notes_type` serves it.
macro_rules! type_filter {
    () => {
        "n.id IN (SELECT id FROM notes WHERE type = :val)"
    };
}

/// R3's filter for any other key: case-insensitive value, exact key, frontmatter only.
macro_rules! field_filter {
    () => {
        "n.id IN (SELECT note_id FROM fields
                   WHERE key = :key AND val_text = :val COLLATE NOCASE
                     AND source = 'frontmatter')"
    };
}

macro_rules! r1 {
    ($filter:expr) => {
        concat!(
            "SELECT n.path, n.title, n.hash, b.id, b.ord, b.heading_path,
                    b.byte_start, b.byte_end,
                    snippet(blocks_fts, 0, char(2), char(3), '…', 16),
                    blocks_fts.rank, n.mtime, n.size
               FROM blocks_fts
               JOIN blocks b ON b.id = blocks_fts.rowid
               JOIN notes n ON n.id = b.note_id
              WHERE blocks_fts MATCH :fts AND n.kind = 'note'",
            $filter,
            " ORDER BY blocks_fts.rank, b.id LIMIT :limit OFFSET :offset"
        )
    };
}

macro_rules! r3 {
    ($filter:expr) => {
        concat!(
            "SELECT COALESCE(a.path, n.path), COALESCE(n.title, a.title),
                    a.id IS NOT NULL, CASE WHEN a.id IS NOT NULL THEN n.path END
               FROM notes n
               LEFT JOIN notes a ON a.sidecar_path = n.path
              WHERE n.kind = 'note' AND ",
            $filter,
            " ORDER BY n.path LIMIT :limit OFFSET :offset"
        )
    };
}

const R1: &str = r1!("");
const R1_TYPE: &str = r1!(concat!(" AND ", type_filter!()));
const R1_FIELD: &str = r1!(concat!(" AND ", field_filter!()));
const R3_TYPE: &str = r3!(type_filter!());
const R3_FIELD: &str = r3!(field_filter!());

/// R2. `GROUP BY (src_note, src_block)` is the "one entry per distinct source block":
/// two links in one paragraph are one entry, and every frontmatter link of a note
/// (`src_block IS NULL`) is one entry.
const R2: &str = "SELECT s.path, s.title, s.hash, b.id, b.ord, b.heading_path,
                         b.byte_start, b.byte_end, b.text
                    FROM links l
                    JOIN notes s ON s.id = l.src_note
                    LEFT JOIN blocks b ON b.id = l.src_block
                   WHERE l.dst_note = (SELECT id FROM notes WHERE path = :path)
                     AND l.kind IN ('wiki', 'embed', 'md')
                   GROUP BY l.src_note, l.src_block
                   ORDER BY s.path, b.ord";

/// The path the link resolver would choose for the folded target `$t`: the
/// `resolve.rs` rule (path, path + `.md`, file name, file name + `.md`; shortest path,
/// then binary order). `resolve.rs` guards the file-name arms with
/// `instr(dst_fold, '/') = 0`; it is dropped here because a file name never contains
/// '/', so for a target that has one those arms match nothing anyway.
macro_rules! resolver_choice {
    ($t:literal) => {
        concat!(
            "(SELECT c.path FROM (
                SELECT n.path AS path FROM notes n WHERE n.path_fold = ",
            $t,
            "
                UNION ALL
                SELECT n.path FROM notes n WHERE n.path_fold = ",
            $t,
            " || '.md'
                UNION ALL
                SELECT n.path FROM notes n WHERE n.file_name_fold = ",
            $t,
            "
                UNION ALL
                SELECT n.path FROM notes n WHERE n.file_name_fold = ",
            $t,
            " || '.md'
             ) c ORDER BY length(c.path), c.path LIMIT 1)"
        )
    };
}

/// Attachments (FR-VLT-08), one statement. The last two columns are what the resolver
/// would choose for the bare file name and for the full path, so the shortest resolving
/// form is the name when that is the asset itself, else the path when that is, else
/// neither (two assets whose names differ only in case on a case-sensitive file system
/// can leave one of them unreachable by any link).
const ATTACHMENTS: &str = concat!(
    "SELECT a.path, x.mime, x.bytes, a.sidecar_path, a.file_name, ",
    resolver_choice!("a.file_name_fold"),
    ", ",
    resolver_choice!("a.path_fold"),
    "
   FROM notes a JOIN assets x ON x.note_id = a.id
  WHERE a.kind = 'asset'
  ORDER BY a.path"
);

/// The statements, for tests that check their plans.
#[doc(hidden)]
pub const STATEMENTS: [(&str, &str); 7] = [
    ("R1", R1),
    ("R1+type", R1_TYPE),
    ("R1+field", R1_FIELD),
    ("R2", R2),
    ("R3 type", R3_TYPE),
    ("R3 field", R3_FIELD),
    ("attachments", ATTACHMENTS),
];

/// A read-only connection to the index and the four reads over it.
#[derive(Debug)]
pub struct Reader {
    conn: Connection,
    rejections: Cell<u32>,
    /// The vault's search connection. Free-text searches run there when it is set;
    /// without it (tests, benches) they run on `conn` and cannot be superseded.
    search: Option<Arc<SearchConn>>,
}

impl Reader {
    /// Open the index database at `db_path` read-only.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if it cannot be opened.
    pub fn open(db_path: &Path) -> Result<Self, IndexError> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sqlite)?;
        conn.set_prepared_statement_cache_capacity(16);
        conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")
            .map_err(sqlite)?;
        Ok(Self {
            conn,
            rejections: Cell::new(0),
            search: None,
        })
    }

    /// Run `f` on this reader's own connection.
    ///
    /// For `bench/budget`'s diagnoses only (task #53: reading pragmas back, counting
    /// statement runs). Not part of any caller's contract; the application never calls it.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn with_connection<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        f(&self.conn)
    }

    /// Route this reader's free-text searches (R1, R1 + R3) to the vault's search
    /// connection. R2, R3 and R4 stay on this reader's own connection.
    #[must_use]
    pub fn with_search(mut self, search: Arc<SearchConn>) -> Self {
        self.search = Some(search);
        self
    }

    /// Wrap an existing connection (tests, benches).
    #[must_use]
    pub fn from_connection(conn: Connection) -> Self {
        conn.set_prepared_statement_cache_capacity(16);
        Self {
            conn,
            rejections: Cell::new(0),
            search: None,
        }
    }

    /// How many queries FTS5 rejected after Rust had quoted them. Each is a defect to
    /// log; the user saw "nothing to search for".
    #[must_use]
    pub fn fts_rejections(&self) -> u32 {
        self.rejections.get()
    }

    /// Search (R1, R3 or R1 + R3, by what the query contains).
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] for a failure that is not an FTS5 rejection of the query.
    pub fn search(
        &self,
        query: &str,
        offset: u32,
        limit: u16,
    ) -> Result<SearchOutcome, IndexError> {
        let (fts, property) = match parse_query(query) {
            ParsedQuery::UnbalancedQuote => return Ok(SearchOutcome::UnbalancedQuote),
            ParsedQuery::TooManyPropertyTerms => return Ok(SearchOutcome::TooManyPropertyTerms),
            ParsedQuery::Terms {
                fts: None,
                property: None,
            } => {
                return Ok(SearchOutcome::NothingToSearch);
            }
            ParsedQuery::Terms { fts, property } => (fts, property),
        };
        let offset = i64::from(offset);
        // Clamped, never rejected (`SearchArgs::limit`). The ceiling is the statement's:
        // a property-only query is R3 and may ask for 500, anything with free text is
        // ranked and stops at 50. Always bound as `:limit`; the SQL text never varies (C-03).
        let ceiling = if fts.is_none() {
            MAX_PROPERTY_LIMIT
        } else {
            MAX_LIMIT
        };
        let limit = i64::from(limit.min(ceiling));

        let result = match (&fts, &property) {
            (Some(fts), None) => self.run_r1(R1, fts, None, limit, offset),
            (Some(fts), Some((key, value))) => {
                let sql = if key == "type" { R1_TYPE } else { R1_FIELD };
                self.run_r1(sql, fts, Some((key, value)), limit, offset)
            }
            (None, Some((key, value))) => self.run_r3(key, value, limit, offset).map(Some),
            (None, None) => unreachable!("handled above"),
        };
        match result {
            Ok(Some(hits)) => Ok(SearchOutcome::Hits(hits)),
            Ok(None) => Ok(SearchOutcome::Superseded),
            // FTS5 refused a query Rust had already quoted: a defect, surfaced as the
            // same "nothing to search for" and counted, never raised (architecture
            // §4.5).
            Err(error) if fts.is_some() && is_fts_syntax_error(&error) => {
                self.rejections.set(self.rejections.get() + 1);
                Ok(SearchOutcome::NothingToSearch)
            }
            Err(error) => Err(sqlite(error)),
        }
    }

    /// R1 and R1 + R3: on the search connection when one is attached (`None` is
    /// "superseded"), otherwise on this reader's own connection.
    fn run_r1(
        &self,
        sql: &str,
        fts: &str,
        property: Option<(&String, &String)>,
        limit: i64,
        offset: i64,
    ) -> Result<Option<Vec<SearchHit>>, rusqlite::Error> {
        match &self.search {
            Some(search) => {
                search.run(&|conn, hook| r1_on(conn, sql, fts, property, limit, offset, hook))
            }
            None => r1_on(&self.conn, sql, fts, property, limit, offset, &|| {}).map(Some),
        }
    }

    fn run_r3(
        &self,
        key: &str,
        value: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SearchHit>, rusqlite::Error> {
        let mut stmt;
        if key == "type" {
            stmt = self.conn.prepare_cached(R3_TYPE)?;
            bind_all(
                &mut stmt,
                &[
                    (":val", &value as &dyn rusqlite::ToSql),
                    (":limit", &limit),
                    (":offset", &offset),
                ],
            )?;
        } else {
            stmt = self.conn.prepare_cached(R3_FIELD)?;
            bind_all(
                &mut stmt,
                &[
                    (":key", &key as &dyn rusqlite::ToSql),
                    (":val", &value),
                    (":limit", &limit),
                    (":offset", &offset),
                ],
            )?;
        }
        let mut rows = stmt.raw_query();
        let mut hits = Vec::new();
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            let is_asset: bool = row.get(2)?;
            hits.push(SearchHit {
                mime: is_asset.then(|| serat_core::mime_for_path(&path).to_owned()),
                path,
                title: row.get(1)?,
                kind: if is_asset {
                    HitKind::Asset
                } else {
                    HitKind::Note
                },
                sidecar_path: row.get(3)?,
                block: None,
                snippet: None,
                rank: None,
                indexed: None,
            });
        }
        Ok(hits)
    }

    /// Every attachment in the vault, by path, with the link that inserts it
    /// (FR-VLT-08). Unpaged: it is the assets only (~200 on the 20k vault).
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if the statement fails.
    pub fn attachments(&self) -> Result<Vec<Attachment>, IndexError> {
        let mut stmt = self.conn.prepare_cached(ATTACHMENTS).map_err(sqlite)?;
        let mut rows = stmt.query([]).map_err(sqlite)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite)? {
            let path: String = row.get(0).map_err(sqlite)?;
            let mime: String = row.get(1).map_err(sqlite)?;
            let file_name: String = row.get(4).map_err(sqlite)?;
            let by_name: Option<String> = row.get(5).map_err(sqlite)?;
            let by_path: Option<String> = row.get(6).map_err(sqlite)?;
            let link_text = if by_name.as_deref() == Some(path.as_str()) {
                Some(file_name)
            } else if by_path.as_deref() == Some(path.as_str()) {
                Some(path.clone())
            } else {
                None
            };
            out.push(Attachment {
                link: link_text
                    .as_deref()
                    .map(|text| serat_core::attachment_link(text, &mime)),
                link_text,
                bytes: row.get::<_, i64>(2).map_err(sqlite)?.max(0) as u64,
                sidecar_path: row.get(3).map_err(sqlite)?,
                path,
                mime,
            });
        }
        Ok(out)
    }

    /// Backlinks (R2) to the note or asset at `path`: one entry per distinct
    /// (source note, source block), ordered by source path then block.
    ///
    /// Unresolved links never appear (they name no note), and an ambiguous
    /// `[[Meeting]]` is listed only by the note the resolver chose.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if the statement fails.
    pub fn backlinks(&self, path: &str) -> Result<Vec<Backlink>, IndexError> {
        let mut stmt = self.conn.prepare_cached(R2).map_err(sqlite)?;
        let mut rows = stmt
            .query(named_params! { ":path": path })
            .map_err(sqlite)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite)? {
            let block_id: Option<i64> = row.get(3).map_err(sqlite)?;
            let block = match block_id {
                None => None,
                Some(block_id) => Some(BlockLocation {
                    block_id,
                    ord: row.get(4).map_err(sqlite)?,
                    heading_path: row.get(5).map_err(sqlite)?,
                    byte_start: row.get::<_, Option<i64>>(6).map_err(sqlite)?.unwrap_or(0) as u64,
                    byte_end: row.get::<_, Option<i64>>(7).map_err(sqlite)?.unwrap_or(0) as u64,
                    note_hash: row.get(2).map_err(sqlite)?,
                }),
            };
            let text: Option<String> = row.get(8).map_err(sqlite)?;
            out.push(Backlink {
                source_path: row.get(0).map_err(sqlite)?,
                source_title: row.get(1).map_err(sqlite)?,
                block,
                excerpt: text
                    .map(|t| t.chars().take(EXCERPT_CHARS).collect())
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }
}

/// The vault's one search connection (architecture §2.2, §4.5): read-only, outside the
/// read pool, behind a `Mutex`, shared by every thread that searches. At most one
/// full-text search is in flight; a newer one supersedes the older.
///
/// Why a generation and not just `interrupt()`: `sqlite3_interrupt` only stops a
/// statement that is *running*, so a call that lands before the old statement has
/// started does nothing, and `Mutex` is not FIFO, so with three quick searches the
/// third could run before the second. Each search therefore
///
/// 1. bumps `latest` and keeps the new value as its own;
/// 2. calls `interrupt()` (lock-free, from outside the lock);
/// 3. takes the lock, and returns superseded unrun if `latest` has moved on;
/// 4. publishes its value as `running` and runs. A progress handler on the connection
///    (a callback, not an SQL function: C-03) interrupts the statement whenever
///    `latest != running`, so an interrupt that arrived early is not lost.
///
/// **A new search cannot be the one interrupted.** `interrupt()` is only a hint; the
/// authority is `latest`. If a delayed `interrupt()` from an older search lands on a
/// statement that is still the newest, `run` sees `SQLITE_INTERRUPT` with `latest`
/// still its own and simply runs the statement again, so only a search that has really
/// been superseded ever returns `None`.
pub struct SearchConn {
    latest: Arc<AtomicU64>,
    running: Arc<AtomicU64>,
    interrupt: InterruptHandle,
    conn: Mutex<Connection>,
    /// Test hook: called after the statement is prepared and bound, before its first step.
    #[cfg(any(test, feature = "test-hooks"))]
    after_prepare: Mutex<Option<TestHook>>,
    /// Test hook: how many times the progress handler has been called.
    #[cfg(test)]
    handler_calls: Arc<AtomicU64>,
}

#[cfg(any(test, feature = "test-hooks"))]
type TestHook = Box<dyn Fn() + Send>;

impl std::fmt::Debug for SearchConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchConn").finish_non_exhaustive()
    }
}

/// What `SearchConn::run` runs: the statement on the connection, calling the hook once
/// the statement is prepared and bound (a no-op outside tests).
type Statement<'a> = dyn Fn(&Connection, &dyn Fn()) -> Result<Vec<SearchHit>, rusqlite::Error> + 'a;

/// VM instructions between progress-handler calls: about 0.7 ms of work on the 20k
/// vault. It was 1,000, which cost +8-12 ms on a 50-100 ms search (task #47); the
/// generation needs noticing within about a millisecond, not a microsecond
/// (architecture 4.5, task #51).
const PROGRESS_OPS: i32 = 10_000;

/// The supersession progress handler (a callback, not an SQL function: C-03): stops the
/// running statement whenever `latest != running`. One construction for `open` and the
/// bench toggle.
fn install_handler(
    conn: &Connection,
    latest: &Arc<AtomicU64>,
    running: &Arc<AtomicU64>,
    #[cfg(test)] calls: &Arc<AtomicU64>,
) -> Result<(), rusqlite::Error> {
    let (l, r) = (Arc::clone(latest), Arc::clone(running));
    #[cfg(test)]
    let calls = Arc::clone(calls);
    conn.progress_handler(
        PROGRESS_OPS,
        Some(move || {
            #[cfg(test)]
            calls.fetch_add(1, Ordering::SeqCst);
            l.load(Ordering::SeqCst) != r.load(Ordering::SeqCst)
        }),
    )
}

impl SearchConn {
    /// Open the vault's search connection, read-only, configured like a pool member.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if it cannot be opened.
    pub fn open(db_path: &Path) -> Result<Arc<Self>, IndexError> {
        Self::open_with_generation(db_path, Arc::new(AtomicU64::new(0)))
    }

    /// As [`SearchConn::open`], with the generation counter supplied by the caller.
    ///
    /// The counter is what makes "at most one full-text search in flight" (§4.5) true
    /// *across* connections: an owner that reopens the connection (the index file was
    /// replaced) passes the same counter to the new one, so a search on either
    /// supersedes a search on the other. The owner may also bump it to supersede
    /// whatever is in flight before it drops the connection.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if it cannot be opened.
    pub fn open_with_generation(
        db_path: &Path,
        latest: Arc<AtomicU64>,
    ) -> Result<Arc<Self>, IndexError> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sqlite)?;
        conn.set_prepared_statement_cache_capacity(16);
        // 64 MiB, a ceiling and not an allocation (architecture §2.2, task #56): the
        // only reads whose working set exceeds SQLite's default 2 MiB run here, and
        // under alternation with another connection that default is bistable (#53).
        conn.execute_batch(&format!(
            "PRAGMA query_only = ON; PRAGMA busy_timeout = 5000; PRAGMA cache_size = -{};",
            crate::checkpoint::SEARCH_CACHE_BYTES / 1024
        ))
        .map_err(sqlite)?;
        let running = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let handler_calls = Arc::new(AtomicU64::new(0));
        install_handler(
            &conn,
            &latest,
            &running,
            #[cfg(test)]
            &handler_calls,
        )
        .map_err(sqlite)?;
        let interrupt = conn.get_interrupt_handle();
        Ok(Arc::new(Self {
            latest,
            running,
            interrupt,
            conn: Mutex::new(conn),
            #[cfg(any(test, feature = "test-hooks"))]
            after_prepare: Mutex::new(None),
            #[cfg(test)]
            handler_calls,
        }))
    }

    /// Run `f` on the search connection, under its lock. For `bench/budget`'s diagnoses
    /// only (task #53), like [`Self::set_progress_handler`].
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn with_connection<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        f(&conn)
    }

    /// Install a hook called once a statement is prepared and bound, before its first
    /// step, so a test can hold a search mid-flight. Not for production use.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_after_prepare(&self, hook: impl Fn() + Send + 'static) {
        *self
            .after_prepare
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(hook));
    }

    /// Install (`true`) or remove (`false`) the progress handler on this same connection.
    ///
    /// For `bench/budget`'s p1-read-paths only: it measures the handler's overhead on
    /// ONE connection, warmed the same way, because comparing two connections measured
    /// page-cache warmth rather than the handler (task #51). Behind `test-hooks`
    /// because `set_progress_handler(false)` silently disables supersession (§4.5);
    /// the application never enables that feature.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`].
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_progress_handler(&self, on: bool) -> Result<(), IndexError> {
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if on {
            install_handler(
                &conn,
                &self.latest,
                &self.running,
                #[cfg(test)]
                &self.handler_calls,
            )
        } else {
            conn.progress_handler(0, None::<fn() -> bool>)
        }
        .map_err(sqlite)
    }

    /// Run one full-text search as the newest. `None` means superseded.
    fn run(&self, f: &Statement) -> Result<Option<Vec<SearchHit>>, rusqlite::Error> {
        let own = self.latest.fetch_add(1, Ordering::SeqCst) + 1;
        self.interrupt.interrupt();
        // Only atomics and the interrupt handle are touched beside this lock.
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if self.latest.load(Ordering::SeqCst) != own {
                return Ok(None);
            }
            self.running.store(own, Ordering::SeqCst);
            let before_step = || {
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(hook) = self
                    .after_prepare
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    hook();
                }
            };
            let result = f(&conn, &before_step);
            if self.latest.load(Ordering::SeqCst) != own {
                return Ok(None);
            }
            match result {
                Ok(hits) => return Ok(Some(hits)),
                // Interrupted by a stale, late `interrupt()` while still the newest:
                // run again.
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == ErrorCode::OperationInterrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// R1 and R1 + R3 on `conn`: one fixed statement, user text bound.
fn r1_on(
    conn: &Connection,
    sql: &str,
    fts: &str,
    property: Option<(&String, &String)>,
    limit: i64,
    offset: i64,
    before_step: &dyn Fn(),
) -> Result<Vec<SearchHit>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(sql)?;
    bind_all(
        &mut stmt,
        &[
            (":fts", &fts as &dyn rusqlite::ToSql),
            (":limit", &limit),
            (":offset", &offset),
        ],
    )?;
    if let Some((key, value)) = property {
        bind_all(
            &mut stmt,
            &[(":key", key as &dyn rusqlite::ToSql), (":val", value)],
        )?;
    }
    before_step();
    let mut rows = stmt.raw_query();
    let mut hits = Vec::new();
    while let Some(row) = rows.next()? {
        hits.push(SearchHit {
            path: row.get(0)?,
            title: row.get(1)?,
            kind: HitKind::Note,
            mime: None,
            sidecar_path: None,
            block: Some(BlockLocation {
                block_id: row.get(3)?,
                ord: row.get(4)?,
                heading_path: row.get(5)?,
                byte_start: row.get::<_, Option<i64>>(6)?.unwrap_or(0) as u64,
                byte_end: row.get::<_, Option<i64>>(7)?.unwrap_or(0) as u64,
                note_hash: row.get(2)?,
            }),
            snippet: row.get(8)?,
            rank: row.get(9)?,
            indexed: Some(IndexedStat {
                mtime_ms: row.get(10)?,
                size: row.get(11)?,
            }),
        });
    }
    Ok(hits)
}

/// Bind named parameters, skipping any the statement does not contain.
fn bind_all(
    stmt: &mut rusqlite::Statement<'_>,
    params: &[(&str, &dyn rusqlite::ToSql)],
) -> Result<(), rusqlite::Error> {
    for (name, value) in params {
        if let Some(index) = stmt.parameter_index(name)? {
            stmt.raw_bind_parameter(index, value)?;
        }
    }
    Ok(())
}

fn is_fts_syntax_error(error: &rusqlite::Error) -> bool {
    error.to_string().contains("fts5")
}

#[cfg(test)]
mod txn_invariant;

#[cfg(test)]
mod tests {
    //! Supersession on the search connection (architecture §4.5; task #47).

    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;
    use crate::schema;

    /// An index with `blocks` blocks that all contain "commonword".
    fn db_with(blocks: u32) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        conn.execute(
            "INSERT INTO notes (path, file_name, path_fold, file_name_fold, title, kind, mtime, size, hash)
             VALUES ('a.md', 'a.md', 'a.md', 'a.md', 'A', 'note', 0, 0, 'h')",
            [],
        )
        .expect("note");
        conn.execute(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < ?1 - 1)
             INSERT INTO blocks (note_id, ord, kind, text, byte_start, byte_end)
             SELECT 1, i, 'para', 'commonword filler ' || i, 0, 1 FROM n",
            [blocks],
        )
        .expect("blocks");
        (dir, path)
    }

    fn reader_on(path: &Path, sc: &Arc<SearchConn>) -> Reader {
        Reader::open(path).expect("reader").with_search(sc.clone())
    }

    /// Park the first statement after it is prepared until `release` is sent.
    fn park_first(sc: &SearchConn) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (parked_tx, parked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let first = AtomicBool::new(true);
        *sc.after_prepare.lock().unwrap() = Some(Box::new(move || {
            if first.swap(false, Ordering::SeqCst) {
                parked_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        }));
        (parked_rx, release_tx)
    }

    fn is_hits(outcome: &SearchOutcome) -> bool {
        matches!(outcome, SearchOutcome::Hits(h) if !h.is_empty())
    }

    #[test]
    fn the_first_of_two_is_superseded_the_second_gets_hits_and_r2_does_not_wait() {
        let (_dir, path) = db_with(4000);
        let sc = SearchConn::open(&path).expect("search conn");
        let (parked, release) = park_first(&sc);
        let pool = Reader::open(&path).expect("pool reader");
        std::thread::scope(|s| {
            let first = s.spawn(|| reader_on(&path, &sc).search("commonword", 0, 50));
            parked.recv().unwrap(); // A holds the search lock, mid-statement.
            // R2 on the pool completes while A is still held: it never waits for it.
            pool.backlinks("a.md").expect("R2");
            let second = s.spawn(|| reader_on(&path, &sc).search("commonword", 0, 50));
            while sc.latest.load(Ordering::SeqCst) < 2 {
                std::thread::yield_now();
            }
            release.send(()).unwrap();
            assert_eq!(first.join().unwrap().unwrap(), SearchOutcome::Superseded);
            assert!(is_hits(&second.join().unwrap().unwrap()));
        });
    }

    #[test]
    fn three_back_to_back_only_the_last_returns_hits() {
        let (_dir, path) = db_with(4000);
        let sc = SearchConn::open(&path).expect("search conn");
        let (parked, release) = park_first(&sc);
        std::thread::scope(|s| {
            let a = s.spawn(|| reader_on(&path, &sc).search("commonword", 0, 50));
            parked.recv().unwrap();
            let b = s.spawn(|| reader_on(&path, &sc).search("commonword", 0, 50));
            while sc.latest.load(Ordering::SeqCst) < 2 {
                std::thread::yield_now();
            }
            let c = s.spawn(|| reader_on(&path, &sc).search("commonword", 0, 50));
            while sc.latest.load(Ordering::SeqCst) < 3 {
                std::thread::yield_now();
            }
            release.send(()).unwrap();
            assert_eq!(a.join().unwrap().unwrap(), SearchOutcome::Superseded);
            assert_eq!(b.join().unwrap().unwrap(), SearchOutcome::Superseded);
            assert!(is_hits(&c.join().unwrap().unwrap()));
        });
    }

    #[test]
    fn a_generation_bumped_after_prepare_but_before_the_first_step_stops_the_statement() {
        let (_dir, path) = db_with(20_000);
        let sc = SearchConn::open(&path).expect("search conn");
        let reader = reader_on(&path, &sc);
        let t = Instant::now();
        assert!(is_hits(&reader.search("commonword", 0, 50).unwrap()));
        let full = t.elapsed();
        // The interrupt() that would have stopped it "arrived early": only the
        // generation moved. The progress handler must still stop the statement.
        let latest = sc.latest.clone();
        *sc.after_prepare.lock().unwrap() = Some(Box::new(move || {
            latest.fetch_add(1, Ordering::SeqCst);
        }));
        let t = Instant::now();
        assert_eq!(
            reader.search("commonword", 0, 50).unwrap(),
            SearchOutcome::Superseded
        );
        assert!(
            t.elapsed() < full / 2,
            "stopped in {:?}, a full run takes {full:?}",
            t.elapsed()
        );
    }

    #[test]
    fn a_superseded_statement_stops_within_two_handler_calls() {
        // Deterministic bound (architecture 4.5): once the generation moves, the
        // statement stops within two handler calls, which is at most 20,000 VM steps.
        // Counted through the test hook, so CI load cannot fail it. Wall clock is
        // asserted only at a generous 50 ms, as a gross-regression net.
        let (_dir, path) = db_with(20_000);
        let sc = SearchConn::open(&path).expect("search conn");
        let reader = reader_on(&path, &sc);
        let before = sc.handler_calls.load(Ordering::SeqCst);
        assert!(is_hits(&reader.search("commonword", 0, 50).unwrap()));
        let full_run_calls = sc.handler_calls.load(Ordering::SeqCst) - before;
        // Enough work that a handler which never stopped it would be called many times.
        assert!(
            full_run_calls > 2,
            "fixture too small: {full_run_calls} calls"
        );

        let latest = sc.latest.clone();
        *sc.after_prepare.lock().unwrap() = Some(Box::new(move || {
            latest.fetch_add(1, Ordering::SeqCst);
        }));
        let before = sc.handler_calls.load(Ordering::SeqCst);
        let t = Instant::now();
        assert_eq!(
            reader.search("commonword", 0, 50).unwrap(),
            SearchOutcome::Superseded
        );
        let wall = t.elapsed();
        let calls = sc.handler_calls.load(Ordering::SeqCst) - before;
        eprintln!(
            "stopped after {calls} handler call(s) in {wall:?}; a full run is {full_run_calls} calls"
        );
        assert!(
            (1..=2).contains(&calls),
            "{calls} handler calls after the generation moved; the bound is 2 (20,000 VM steps)"
        );
        assert!(
            wall < std::time::Duration::from_millis(50),
            "stopped in {wall:?}"
        );
    }

    #[test]
    fn a_late_stale_interrupt_does_not_supersede_the_newest_search() {
        let (_dir, path) = db_with(2000);
        let sc = SearchConn::open(&path).expect("search conn");
        let reader = reader_on(&path, &sc);
        // An older search's delayed interrupt() lands, repeatedly, on the newest
        // statement for a while. It is still the newest, so it must run again and win.
        let racer = sc.clone();
        let fired = AtomicBool::new(false);
        *sc.after_prepare.lock().unwrap() = Some(Box::new(move || {
            if !fired.swap(true, Ordering::SeqCst) {
                let racer = racer.clone();
                std::thread::spawn(move || {
                    let end = Instant::now() + std::time::Duration::from_millis(30);
                    while Instant::now() < end {
                        racer.interrupt.interrupt();
                    }
                });
            }
        }));
        assert!(is_hits(&reader.search("commonword", 0, 50).unwrap()));
    }
}
