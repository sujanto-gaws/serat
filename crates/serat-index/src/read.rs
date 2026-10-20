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

use std::cell::Cell;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, named_params};
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
                    blocks_fts.rank
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

/// The statements, for tests that check their plans.
#[doc(hidden)]
pub const STATEMENTS: [(&str, &str); 6] = [
    ("R1", R1),
    ("R1+type", R1_TYPE),
    ("R1+field", R1_FIELD),
    ("R2", R2),
    ("R3 type", R3_TYPE),
    ("R3 field", R3_FIELD),
];

/// A read-only connection to the index and the four reads over it.
#[derive(Debug)]
pub struct Reader {
    conn: Connection,
    rejections: Cell<u32>,
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
        })
    }

    /// Wrap an existing connection (tests, benches).
    #[must_use]
    pub fn from_connection(conn: Connection) -> Self {
        conn.set_prepared_statement_cache_capacity(16);
        Self {
            conn,
            rejections: Cell::new(0),
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
            (None, Some((key, value))) => self.run_r3(key, value, limit, offset),
            (None, None) => unreachable!("handled above"),
        };
        match result {
            Ok(hits) => Ok(SearchOutcome::Hits(hits)),
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

    fn run_r1(
        &self,
        sql: &str,
        fts: &str,
        property: Option<(&String, &String)>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SearchHit>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(sql)?;
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
            });
        }
        Ok(hits)
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
                mime: is_asset.then(|| mime_for(&path)).flatten(),
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
            });
        }
        Ok(hits)
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

/// A MIME type from the extension, for the asset formats the vault holds. `None` for
/// anything else: a wrong type is worse than none.
fn mime_for(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "csv" => "text/csv",
        "epub" => "application/epub+zip",
        "eml" => "message/rfc822",
        _ => return None,
    };
    Some(mime.to_owned())
}
