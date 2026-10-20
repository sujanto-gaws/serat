//! The index read commands, as a mapping and nothing else (FR-NAV-02, FR-NAV-04,
//! FR-STR-03, FR-BIN-02; ADR-0001, ADR-0010).
//!
//! Every statement, every limit and every ranking decision belongs to
//! `serat_index::Reader`; this module turns its values into the contract's and applies
//! rule 1: **no command waits for the index**. Anything but `Ready` answers
//! [`IndexRead::NotReady`] with the state, and a read that fails is reported the same
//! way (`Unavailable`), because the contract has no index error: a broken index is a
//! state, not a failed command.
//!
//! # The in-flight rule (task #47, #18b)
//!
//! `SearchArgs` documents at most one full-text search in flight per vault, with
//! `SearchOutcome::Superseded` for the interrupted one. The connection, its interrupt
//! handle and the outcome live in `serat-index`; [`SearchSlot`] holds the vault's one
//! `Arc<SearchConn>` and `search` reads through `Reader::with_search`. Backlinks and
//! property-only reads stay on a plain `Reader`. The match in [`map_outcome`] is
//! exhaustive with no `_` arm, so a new core outcome stops compiling here.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serat_index::read::{HitKind, Reader, SearchConn};
use serat_index::{index_path, read};

use crate::index::index_dir_for;
use crate::ipc::{
    Backlink, BlockLocation, IndexRead, IndexState, SearchHit, SearchOutcome, TreeEntryKind,
};

/// A read-only connection to the index of the vault rooted at `root`.
///
/// Opened per call, deliberately: a rebuild replaces the database file, and a cached
/// connection would keep reading the old inode. Opening read-only is sub-millisecond
/// and the statements are prepared on first use.
fn open_reader(root: &Path, search: Option<&SearchSlot>) -> Result<Reader, IndexRead<()>> {
    let path = index_path(&index_dir_for(root));
    let fail = |err: serat_core::IndexError| unavailable(&path, &err.to_string());
    let reader = Reader::open(&path).map_err(fail)?;
    match search {
        None => Ok(reader),
        Some(slot) => Ok(reader.with_search(slot.get(&path).map_err(fail)?)),
    }
}

/// Identity of the index file: a rebuild replaces it, and a connection opened on the
/// old one would keep reading the old inode.
type FileId = (u64, u64);

#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn file_id(path: &Path) -> Option<FileId> {
    let created = std::fs::metadata(path).ok()?.created().ok()?;
    let nanos = created
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((u64::try_from(nanos).unwrap_or(u64::MAX), 0))
}

/// The open vault's one search connection (architecture §4.5 obligation 2), created
/// once and shared by every search of that vault. It is created on the first search
/// that finds the index `Ready` (the file may not exist when the vault opens) and is
/// replaced only if a rebuild has replaced the file underneath it.
#[derive(Default)]
pub struct SearchSlot {
    conn: Mutex<Option<(FileId, Arc<SearchConn>)>>,
}

impl std::fmt::Debug for SearchSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchSlot").finish_non_exhaustive()
    }
}

impl SearchSlot {
    /// The vault's search connection for the index at `path`. The slot's lock is held
    /// only to clone or replace the `Arc`, never while a search runs.
    fn get(&self, path: &Path) -> Result<Arc<SearchConn>, serat_core::IndexError> {
        let id = file_id(path);
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if let (Some((have, conn)), Some(id)) = (slot.as_ref(), id)
            && *have == id
        {
            return Ok(Arc::clone(conn));
        }
        let conn = SearchConn::open(path)?;
        if let Some(id) = id {
            *slot = Some((id, Arc::clone(&conn)));
        }
        Ok(conn)
    }
}

fn unavailable(path: &Path, message: &str) -> IndexRead<()> {
    IndexRead::NotReady {
        state: IndexState::Unavailable {
            message: message.to_owned(),
            path: path.display().to_string(),
        },
    }
}

/// Run `read` against the index if (and only if) `state` is `Ready`.
pub fn read_when_ready<T>(
    state: IndexState,
    root: &Path,
    search: Option<&SearchSlot>,
    read: impl FnOnce(&Reader) -> Result<T, serat_core::IndexError>,
) -> IndexRead<T> {
    if !matches!(state, IndexState::Ready { .. }) {
        return IndexRead::NotReady { state };
    }
    let not_ready = |state: IndexState| IndexRead::NotReady { state };
    match open_reader(root, search) {
        Err(IndexRead::NotReady { state }) => not_ready(state),
        Err(IndexRead::Ready { .. }) => unreachable!("open_reader never reports Ready"),
        Ok(reader) => match read(&reader) {
            Ok(value) => IndexRead::Ready { value },
            Err(err) => {
                let path = index_path(&index_dir_for(root));
                not_ready(IndexState::Unavailable {
                    message: err.to_string(),
                    path: path.display().to_string(),
                })
            }
        },
    }
}

/// The core's outcome in the contract's terms.
#[must_use]
pub fn map_outcome(outcome: read::SearchOutcome) -> SearchOutcome {
    match outcome {
        read::SearchOutcome::Hits(hits) => SearchOutcome::Hits {
            items: hits.into_iter().map(map_hit).collect(),
        },
        read::SearchOutcome::NothingToSearch => SearchOutcome::NothingToSearch,
        read::SearchOutcome::TooManyPropertyTerms => SearchOutcome::TooManyPropertyTerms,
        read::SearchOutcome::UnbalancedQuote => SearchOutcome::UnbalancedQuote,
        read::SearchOutcome::Superseded => SearchOutcome::Superseded,
    }
}

fn map_hit(hit: read::SearchHit) -> SearchHit {
    SearchHit {
        path: hit.path,
        title: hit.title,
        kind: map_kind(hit.kind),
        mime: hit.mime,
        sidecar_path: hit.sidecar_path,
        block: hit.block.map(map_block),
        snippet: hit.snippet,
    }
}

fn map_kind(kind: HitKind) -> TreeEntryKind {
    match kind {
        HitKind::Note => TreeEntryKind::Note,
        HitKind::Asset => TreeEntryKind::Asset,
    }
}

fn map_block(block: read::BlockLocation) -> BlockLocation {
    BlockLocation {
        block_id: block.block_id,
        ord: block.ord,
        heading_path: block.heading_path,
        byte_start: block.byte_start,
        byte_end: block.byte_end,
        note_hash: block.note_hash,
    }
}

/// A backlink in the contract's terms.
#[must_use]
pub fn map_backlink(link: read::Backlink) -> Backlink {
    Backlink {
        source_path: link.source_path,
        source_title: link.source_title,
        block: link.block.map(map_block),
        excerpt: link.excerpt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_maps_through_unchanged() {
        assert!(matches!(
            map_outcome(read::SearchOutcome::Superseded),
            SearchOutcome::Superseded
        ));
    }

    #[test]
    fn the_search_slot_reuses_one_connection_until_the_file_is_replaced() {
        let dir = std::env::temp_dir().join(format!("serat-slot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let db = dir.join("index.db");
        let make = || {
            let c = rusqlite::Connection::open(&db).expect("db");
            c.execute_batch("CREATE TABLE t(x);").expect("ddl");
        };
        make();
        let slot = SearchSlot::default();
        let a = slot.get(&db).expect("a");
        assert!(Arc::ptr_eq(&a, &slot.get(&db).expect("again")));
        std::fs::remove_file(&db).expect("rm");
        make();
        let b = slot.get(&db).expect("b");
        assert!(
            !Arc::ptr_eq(&a, &b),
            "a replaced file gets a new connection"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
