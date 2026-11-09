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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serat_index::read::{HitKind, Reader, SearchConn};
use serat_index::{index_path, read};

use crate::index::{HandleEpoch, index_dir_for};
use crate::ipc::{
    Backlink, BlockLocation, IndexRead, IndexState, SearchHit, SearchOutcome, TreeEntryKind,
};

/// A read-only connection to the index of the vault rooted at `root`.
///
/// Opened per call, deliberately: a rebuild replaces the database file, and a cached
/// connection would keep reading the old inode. Opening read-only is sub-millisecond
/// and the statements are prepared on first use. (The search connection is the one
/// exception, and is replaced by [`SearchSlot`] when the index handle changes.)
fn open_reader(root: &Path, search: Option<&SearchSlot>) -> Result<Reader, IndexRead<()>> {
    let path = index_path(&index_dir_for(root));
    let fail = |err: serat_core::IndexError| unavailable(&path, &err.to_string());
    let reader = Reader::open(&path).map_err(fail)?;
    match search {
        None => Ok(reader),
        Some(slot) => Ok(reader.with_search(slot.get(&path).map_err(fail)?)),
    }
}

/// The open vault's one search connection (architecture §4.5 obligation 2), created
/// once and shared by every search of that vault. It is created on the first search
/// that finds the index `Ready` (the file may not exist when the vault opens) and is
/// replaced when the app's [`HandleEpoch`] moves, that is when the index actor has
/// delivered a new handle because the database was rebuilt.
///
/// # Identity is the epoch, not the file
///
/// File metadata (inode, creation time) is not used: NTFS tunnelling gives a deleted
/// and recreated `index.db` the old creation time, so it cannot tell the two apart.
///
/// # One generation across reopens
///
/// The slot owns the generation counter and hands it to every connection it opens, so
/// a search on a new connection supersedes one still running on the old (§4.5: at most
/// one in flight), whichever connection each is on.
///
/// # Before a runtime rebuild: call [`SearchSlot::close`]
///
/// SQLite on Windows opens without delete sharing, so a held-open `index.db` makes the
/// rebuild's `remove_file` fail (reported as `Corrupt`). Any code that deletes or
/// replaces the index file while the vault is open **must** call `close()` first.
/// The one runtime rebuild, `AppState::rebuild_index`, calls [`SearchSlot::suspend`]
/// (which closes and then refuses to reopen) and [`SearchSlot::resume`] after it.
pub struct SearchSlot {
    pub(crate) epoch: HandleEpoch,
    pub(crate) generation: Arc<AtomicU64>,
    conn: Mutex<Option<(u64, Arc<SearchConn>)>>,
    /// Set while a runtime rebuild owns the file: [`SearchSlot::get`] refuses instead
    /// of reopening it. Read and written only under `conn`'s lock.
    suspended: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for SearchSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchSlot").finish_non_exhaustive()
    }
}

impl SearchSlot {
    /// A slot keyed on `epoch`, which the index actor bumps for each new handle.
    #[must_use]
    pub fn new(epoch: HandleEpoch) -> Self {
        Self {
            epoch,
            generation: Arc::new(AtomicU64::new(0)),
            conn: Mutex::new(None),
            suspended: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The vault's search connection for the index at `path`. The slot's lock is held
    /// only to clone or replace the `Arc`, never while a search runs.
    pub(crate) fn get(&self, path: &Path) -> Result<Arc<SearchConn>, serat_core::IndexError> {
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if self.suspended.load(Ordering::SeqCst) {
            return Err(serat_core::IndexError::Corrupt {
                message: "the index is being rebuilt".to_owned(),
            });
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        if let Some((have, conn)) = slot.as_ref()
            && *have == epoch
        {
            return Ok(Arc::clone(conn));
        }
        let conn = SearchConn::open_with_generation(path, Arc::clone(&self.generation))?;
        *slot = Some((epoch, Arc::clone(&conn)));
        Ok(conn)
    }

    /// Supersede any search in flight, drop the connection releasing the file, and keep
    /// it closed: until [`SearchSlot::resume`], a search is refused rather than
    /// reopening `index.db` (a runtime rebuild is about to delete it, and on Windows an
    /// open connection makes the delete fail). Serialised with [`SearchSlot::get`] by
    /// the slot's lock, so nothing reopens after this returns.
    ///
    /// **Required before any runtime rebuild deletes `index.db`** (Windows; see the
    /// type's documentation).
    pub fn suspend(&self) {
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        self.suspended.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        *slot = None;
    }

    /// Allow searches to open the (new) index again.
    pub fn resume(&self) {
        let _slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        self.suspended.store(false, Ordering::SeqCst);
    }

    /// Supersede any search in flight and drop the connection, releasing the file. A
    /// search still running returns `Superseded` at its next progress-handler call and
    /// then releases its `Arc`; the next search reopens.
    ///
    /// Prefer [`SearchSlot::suspend`] around a rebuild; this alone lets a search
    /// reopen the file straight away.
    pub fn close(&self) {
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        self.generation.fetch_add(1, Ordering::SeqCst);
        *slot = None;
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
///
/// `stat` answers for a vault-relative path (see [`StatResult`]). It is called for at most
/// [`STAT_HITS_PER_PAGE`] hits; see [`changed_since_indexed`].
pub fn map_outcome(
    outcome: read::SearchOutcome,
    stat: &dyn Fn(&str) -> StatResult,
) -> SearchOutcome {
    match outcome {
        read::SearchOutcome::Hits(hits) => SearchOutcome::Hits {
            items: hits
                .into_iter()
                .enumerate()
                .map(|(i, hit)| {
                    let changed = if i < STAT_HITS_PER_PAGE {
                        changed_since_indexed(&hit, stat)
                    } else {
                        None
                    };
                    map_hit(hit, changed)
                })
                .collect(),
        },
        read::SearchOutcome::NothingToSearch => SearchOutcome::NothingToSearch,
        read::SearchOutcome::TooManyPropertyTerms => SearchOutcome::TooManyPropertyTerms,
        read::SearchOutcome::UnbalancedQuote => SearchOutcome::UnbalancedQuote,
        read::SearchOutcome::Superseded => SearchOutcome::Superseded,
    }
}

/// What one stat of a hit's file came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatResult {
    /// `(mtime_ms, size)`.
    Got(i64, u64),
    /// The stat failed (gone, unreadable, escapes the vault): not what was indexed.
    Failed,
    /// Not attempted: the deadline passed or the search was superseded.
    Skipped,
}

/// The whole stat phase's budget (task #88): on a cold network share each stat can
/// cost ~10 ms, and the phase runs on the blocking thread after R1 has returned.
pub const STAT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(50);

/// Wrap `stat` so that it stops at `deadline` from now, or once `superseded()` is true;
/// every hit after that is [`StatResult::Skipped`] and stays `None`.
pub fn budgeted_stat<'a>(
    deadline: std::time::Duration,
    superseded: impl Fn() -> bool + 'a,
    stat: impl Fn(&str) -> StatResult + 'a,
) -> impl Fn(&str) -> StatResult + 'a {
    let cutoff = std::time::Instant::now() + deadline;
    let stopped = std::cell::Cell::new(false);
    move |path| {
        if !stopped.get() && (std::time::Instant::now() >= cutoff || superseded()) {
            stopped.set(true);
        }
        if stopped.get() {
            StatResult::Skipped
        } else {
            stat(path)
        }
    }
}

/// The most hits per page that are stat-ed: R1's ceiling (architecture §4.5).
pub const STAT_HITS_PER_PAGE: usize = 50;

/// Whether a hit's file differs from what the index recorded, from one stat
/// (task #88, FR-NAV-02, US-05; the ruling in architecture §4.5).
///
/// `Some(mtime != row.mtime || size != row.size)`: the catch-up's own comparator, over
/// the same millisecond conversion as the walk (`Vault::stat`). A failed stat (the file
/// vanished, or cannot be read) is `Some(true)`: it is not what was indexed, which is the
/// honest answer to the hint (architecture §4.5). `None` only when the hit has no block
/// (a property-only hit is never stat-ed); the caller also leaves hits beyond
/// [`STAT_HITS_PER_PAGE`] at `None`.
///
/// **It is a hint, not a verdict.** A false positive: a `touch`, or a checkout that
/// rewrites identical bytes, moves the mtime and reads `Some(true)`. A false negative:
/// an edit that keeps both size and mtime (a same-size rewrite inside one coarse mtime
/// tick) reads `Some(false)`. Neither presents a wrong line as exact, because the
/// authority is the hash check at open, which compares `BlockLocation::note_hash` with
/// the bytes it reads.
fn changed_since_indexed(hit: &read::SearchHit, stat: &dyn Fn(&str) -> StatResult) -> Option<bool> {
    hit.block.as_ref()?;
    let indexed = hit.indexed?;
    let (mtime_ms, size) = match stat(&hit.path) {
        StatResult::Got(mtime_ms, size) => (mtime_ms, size),
        StatResult::Failed => return Some(true),
        StatResult::Skipped => return None,
    };
    Some(mtime_ms != indexed.mtime_ms || i64::try_from(size).unwrap_or(i64::MAX) != indexed.size)
}

fn map_hit(hit: read::SearchHit, changed_since_indexed: Option<bool>) -> SearchHit {
    SearchHit {
        path: hit.path,
        title: hit.title,
        kind: map_kind(hit.kind),
        mime: hit.mime,
        sidecar_path: hit.sidecar_path,
        block: hit.block.map(map_block),
        snippet: hit.snippet,
        changed_since_indexed,
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
        // Contract ahead of implementation (task #79): filled once serat-index
        // records links.field_key (INDEX_VERSION 12). None = "not known".
        frontmatter_keys: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_maps_through_unchanged() {
        assert!(matches!(
            map_outcome(read::SearchOutcome::Superseded, &|_| StatResult::Skipped),
            SearchOutcome::Superseded
        ));
    }

    // ---- changed_since_indexed (task #88; FR-NAV-02, US-05) ----
    //
    // The stat is the real `Vault::stat` over real files; only the index row is
    // fabricated, because a running watcher would otherwise re-index the edit.

    use serat_core::VaultPath;
    use serat_vault::Vault;
    use std::time::{Duration, SystemTime};

    fn hit(path: &str, indexed: (i64, u64), with_block: bool) -> read::SearchHit {
        read::SearchHit {
            path: path.to_owned(),
            title: None,
            kind: HitKind::Note,
            mime: None,
            sidecar_path: None,
            block: with_block.then(|| read::BlockLocation {
                block_id: 1,
                ord: 0,
                heading_path: None,
                byte_start: 0,
                byte_end: 1,
                note_hash: "h".into(),
            }),
            snippet: None,
            rank: Some(0.0),
            indexed: Some(read::IndexedStat {
                mtime_ms: indexed.0,
                size: i64::try_from(indexed.1).expect("size"),
            }),
        }
    }

    fn vault_stat(vault: &Vault, p: &str) -> StatResult {
        match VaultPath::new(p).ok().and_then(|p| vault.stat(&p).ok()) {
            Some((m, s)) => StatResult::Got(m, s),
            None => StatResult::Failed,
        }
    }

    #[test]
    fn hits_after_the_deadline_or_a_supersession_are_not_stat_ed() {
        let slow = |_: &str| {
            std::thread::sleep(Duration::from_millis(30));
            StatResult::Got(0, 0)
        };
        let budgeted = budgeted_stat(Duration::from_millis(50), || false, slow);
        let got: Vec<_> = (0..10).map(|_| budgeted("a.md")).collect();
        assert!(got[0] == StatResult::Got(0, 0), "the first runs");
        assert!(
            got[3..].iter().all(|r| *r == StatResult::Skipped),
            "after the cutoff nothing is stat-ed: {got:?}"
        );

        let flag = std::cell::Cell::new(false);
        let budgeted = budgeted_stat(
            Duration::from_secs(60),
            || flag.get(),
            |_| StatResult::Failed,
        );
        assert_eq!(budgeted("a.md"), StatResult::Failed);
        flag.set(true);
        assert_eq!(budgeted("a.md"), StatResult::Skipped);
        flag.set(false);
        assert_eq!(budgeted("a.md"), StatResult::Skipped, "stays stopped");
    }

    fn flags(vault: &Vault, hits: Vec<read::SearchHit>) -> Vec<Option<bool>> {
        let stat = |p: &str| vault_stat(vault, p);
        match map_outcome(read::SearchOutcome::Hits(hits), &stat) {
            SearchOutcome::Hits { items } => {
                items.into_iter().map(|h| h.changed_since_indexed).collect()
            }
            other => panic!("{other:?}"),
        }
    }

    fn set_mtime(path: &Path, to: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_modified(to)
            .expect("set mtime");
    }

    #[test]
    fn staleness_follows_the_catch_ups_comparator() {
        let dir = tempfile::tempdir().expect("dir");
        let vault = Vault::open(dir.path()).expect("vault");
        let file = dir.path().join("a.md");
        std::fs::write(&file, "aaaa").expect("write");
        let stat = |v: &Vault| {
            v.stat(&VaultPath::new("a.md").expect("path"))
                .expect("stat")
        };
        let indexed = stat(&vault);

        // Untouched.
        assert_eq!(
            flags(&vault, vec![hit("a.md", indexed, true)]),
            [Some(false)]
        );

        // Same size, bumped mtime, different bytes.
        std::fs::write(&file, "bbbb").expect("rewrite");
        set_mtime(&file, SystemTime::now() + Duration::from_secs(10));
        assert_eq!(
            flags(&vault, vec![hit("a.md", indexed, true)]),
            [Some(true)]
        );

        // Touched, identical bytes: the documented false positive.
        std::fs::write(&file, "aaaa").expect("restore");
        set_mtime(&file, SystemTime::now() + Duration::from_secs(20));
        assert_eq!(
            flags(&vault, vec![hit("a.md", indexed, true)]),
            [Some(true)]
        );

        // Edited with the same size and the same mtime: the documented false negative.
        std::fs::write(&file, "cccc").expect("edit");
        set_mtime(
            &file,
            SystemTime::UNIX_EPOCH + Duration::from_millis(indexed.0.unsigned_abs()),
        );
        assert_eq!(
            flags(&vault, vec![hit("a.md", indexed, true)]),
            [Some(false)]
        );

        // A different size is a change whatever the mtime says.
        std::fs::write(&file, "longer").expect("grow");
        set_mtime(
            &file,
            SystemTime::UNIX_EPOCH + Duration::from_millis(indexed.0.unsigned_abs()),
        );
        assert_eq!(
            flags(&vault, vec![hit("a.md", indexed, true)]),
            [Some(true)]
        );
    }

    #[test]
    fn the_51st_hit_and_a_blockless_hit_are_none_and_a_vanished_file_is_changed() {
        let dir = tempfile::tempdir().expect("dir");
        let vault = Vault::open(dir.path()).expect("vault");
        std::fs::write(dir.path().join("a.md"), "aaaa").expect("write");
        let indexed = vault
            .stat(&VaultPath::new("a.md").expect("path"))
            .expect("stat");

        let mut hits: Vec<_> = (0..STAT_HITS_PER_PAGE + 1)
            .map(|_| hit("a.md", indexed, true))
            .collect();
        hits.push(hit("gone.md", indexed, true));
        hits.push(hit("a.md", indexed, false));
        let got = flags(&vault, hits);
        assert!(got[..STAT_HITS_PER_PAGE].iter().all(|f| *f == Some(false)));
        assert_eq!(got[STAT_HITS_PER_PAGE], None, "the 51st hit is not stat-ed");
        assert_eq!(got[STAT_HITS_PER_PAGE + 1], None, "beyond 50, even if gone");
        assert_eq!(got[STAT_HITS_PER_PAGE + 2], None);

        // Within the first 50: a vanished file is Some(true), a blockless hit None.
        let got = flags(
            &vault,
            vec![hit("gone.md", indexed, true), hit("a.md", indexed, false)],
        );
        assert_eq!(got, [Some(true), None]);
    }

    fn make_db(db: &Path) {
        let c = rusqlite::Connection::open(db).expect("db");
        c.execute_batch("CREATE TABLE t(x);").expect("ddl");
    }

    #[test]
    fn the_search_slot_reuses_one_connection_until_the_epoch_moves() {
        let dir = tempfile::tempdir().expect("dir");
        let db = dir.path().join("index.db");
        make_db(&db);
        let epoch = HandleEpoch::default();
        let slot = SearchSlot::new(Arc::clone(&epoch));
        let a = slot.get(&db).expect("a");
        assert!(Arc::ptr_eq(&a, &slot.get(&db).expect("again")));
        // A replaced file with no new handle is not noticed: identity is the epoch.
        std::fs::remove_file(&db).expect("rm");
        make_db(&db);
        assert!(Arc::ptr_eq(&a, &slot.get(&db).expect("same epoch")));
        epoch.fetch_add(1, Ordering::SeqCst);
        let b = slot.get(&db).expect("b");
        assert!(!Arc::ptr_eq(&a, &b), "a new handle gets a new connection");
    }

    #[test]
    fn close_drops_the_connection_and_the_next_get_reopens() {
        let dir = tempfile::tempdir().expect("dir");
        let db = dir.path().join("index.db");
        make_db(&db);
        let slot = SearchSlot::new(HandleEpoch::default());
        let a = slot.get(&db).expect("a");
        let weak = Arc::downgrade(&a);
        drop(a);
        slot.close();
        assert!(weak.upgrade().is_none(), "the slot no longer holds it");
        let b = slot.get(&db).expect("reopened");
        assert!(weak.upgrade().is_none() && Arc::strong_count(&b) == 2);
    }
}
