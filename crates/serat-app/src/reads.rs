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
//! # Seam for the in-flight rule (task #47)
//!
//! `SearchArgs` documents at most one full-text search in flight per vault, with
//! `SearchOutcome::Superseded` for the interrupted one. That connection, its interrupt
//! handle and the outcome live in `serat-index`. The match in [`map_outcome`] is
//! exhaustive over the core's outcome with no `_` arm, so the day the core grows a
//! `Superseded` this stops compiling here and the one-line mapping is added;
//! [`open_reader`] is the single place that obtains a connection and the only line that
//! changes when the core hands out its search connection.

use std::path::Path;

use serat_index::read::{HitKind, Reader};
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
fn open_reader(root: &Path) -> Result<Reader, IndexRead<()>> {
    let path = index_path(&index_dir_for(root));
    Reader::open(&path).map_err(|err| unavailable(&path, &err.to_string()))
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
    read: impl FnOnce(&Reader) -> Result<T, serat_core::IndexError>,
) -> IndexRead<T> {
    if !matches!(state, IndexState::Ready { .. }) {
        return IndexRead::NotReady { state };
    }
    let not_ready = |state: IndexState| IndexRead::NotReady { state };
    match open_reader(root) {
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
