//! `switcher_query`: the quick switcher, wired (architecture 9.2; FR-NAV-01, FR-EDT-03,
//! NFR-PRF-06, US-04).
//!
//! No matching logic here: the corpus, the matcher, the ranking and the frecency record
//! are `serat_index::switcher` and `serat_index::history`. This module owns what
//! 9.2 assigns to `serat-app`:
//!
//! * **Which corpus answers.** The index's (`Corpus::from_index`) once it is `Ready` and
//!   a snapshot has been built; otherwise the tree's paths ([`TreePaths`], fed by the
//!   tree stream), so the switcher works from the first listing and gains titles,
//!   aliases and headings when the index arrives (goal 5). A query never waits for a
//!   build, and a failed build keeps the last snapshot.
//! * **Freshness.** A snapshot is built when the index publishes `Ready`, and again
//!   [`REFRESH_DELAY`] after the last `index://changed`-worthy change, on the worker
//!   thread below, never on a query's.
//! * **Stale queries.** Queries are serialised. One that is still waiting for the line
//!   when a later arrival with a higher `seq` comes in is discarded (empty answer
//!   carrying its own `seq`, which the front end drops anyway). Neither arrival order
//!   alone (the commands run on a thread pool, so the newest keystroke can arrive
//!   first) nor `seq` alone (it restarts on a webview reload) would be safe. The
//!   answer says `discarded: true`, so the front end ignores it instead of painting
//!   "no matches". One edge remains across a reload: a straggler from before it, still
//!   in flight with a higher `seq`, supersedes a new low-`seq` query, which is then
//!   answered `discarded` and repainted by the user's next keystroke.
//! * **The frecency record** (`.serat/history.json`): read once at open, updated in
//!   memory at once when a note is reported opened, and written [`HISTORY_DELAY`] after
//!   the last open (and when the vault closes) through `Vault::update_history`. Losing
//!   it is safe (C-02): ranking falls back to the lexical rules.
//!
//! # Locks
//!
//! All leaf locks, none held across another: the work state, the corpus slot, the
//! history, the tree-corpus cache, the query gate (held while a query runs, and nothing
//! else is taken inside it but the history).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use serat_core::{TreeEntry, TreeKind, TreeOp};
use serat_index::history::History;
use serat_index::read::PathResolver;
use serat_index::switcher::{Corpus, CorpusSource, Hit, Matched};
use serat_vault::{TreeOut, Vault};

use crate::ipc::{
    HeadingRef, Span, SwitcherCorpus, SwitcherItem, SwitcherMatch, SwitcherQueryArgs,
    SwitcherResults,
};

/// How long after the last change the index snapshot is rebuilt (architecture 9.2).
pub const REFRESH_DELAY: Duration = Duration::from_millis(500);
/// How long after the last open the frecency record is written (architecture 9.2).
pub const HISTORY_DELAY: Duration = Duration::from_secs(2);

/// The note paths the tree stream has delivered, for the corpus used before the index
/// is `Ready`. Fed from the stream itself (listings, upserts, removes), so it is the
/// paths the vault thread has already enumerated and costs no second walk.
#[derive(Debug, Default)]
pub struct TreePaths {
    dirs: Mutex<BTreeMap<String, BTreeSet<String>>>,
    generation: AtomicU64,
    /// The `resolve_links` tree fallback's resolver and the generation it was built at
    /// (task #185). Same lifetime as the switcher's tree corpus, so it shares
    /// `generation` rather than keeping a second counter.
    resolver: Mutex<Option<(u64, Arc<PathResolver>)>>,
    resolver_builds: AtomicU64,
}

impl TreePaths {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, BTreeSet<String>>> {
        self.dirs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fold one stream message in. Called under the tree session's lock; takes only its
    /// own.
    pub fn apply(&self, out: &TreeOut) {
        let TreeOut::Op(op) = out else {
            return;
        };
        let mut dirs = self.lock();
        match op {
            TreeOp::Listing { dir, entries } => {
                let names: BTreeSet<String> = entries
                    .iter()
                    .filter(|e| e.kind == TreeKind::Note)
                    .map(|e| e.name.clone())
                    .collect();
                if names.is_empty() {
                    dirs.remove(dir);
                } else {
                    dirs.insert(dir.clone(), names);
                }
            }
            TreeOp::Upsert { dir, entry, .. } => Self::upsert(&mut dirs, dir, entry),
            TreeOp::Remove { dir, name } => {
                if let Some(names) = dirs.get_mut(dir) {
                    names.remove(name);
                }
                // A removed directory takes everything beneath it.
                let below = if dir.is_empty() {
                    name.clone()
                } else {
                    format!("{dir}/{name}")
                };
                let prefix = format!("{below}/");
                dirs.retain(|d, _| *d != below && !d.starts_with(&prefix));
            }
        }
        drop(dirs);
        // Load-bearing order: the bump comes after the `dirs` change and after its lock
        // is released. `resolver()` and `tree_snapshot()` read the generation, then the
        // paths; a bump before the change would let a pre-change path set be cached under
        // the new generation and served as fresh.
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn upsert(dirs: &mut BTreeMap<String, BTreeSet<String>>, dir: &str, entry: &TreeEntry) {
        if entry.kind == TreeKind::Note {
            dirs.entry(dir.to_owned())
                .or_default()
                .insert(entry.name.clone());
        }
    }

    /// Changes so far; a cached corpus is stale when this moved.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The link resolver over every note path delivered so far, folded once per tree
    /// generation (task #185). The cache lock is held only to compare and to store,
    /// never across the fold: a caller that finds the cache stale reads the paths and
    /// folds on its own thread, so the keystroke path is never queued behind a build.
    /// Two racing callers may both fold; the newer generation wins the store and both
    /// answers are correct for the paths each saw.
    #[must_use]
    pub fn resolver(&self) -> Arc<PathResolver> {
        let generation = self.generation();
        if let Some((have, resolver)) = self
            .resolver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            && *have == generation
        {
            return Arc::clone(resolver);
        }
        // Generation read before the paths: a change in between only makes the entry
        // look older than it is, so the next call rebuilds; it can never look newer.
        let paths = self.note_paths();
        let built = Arc::new(PathResolver::new(paths.iter().map(String::as_str)));
        self.resolver_builds.fetch_add(1, Ordering::SeqCst);
        let mut cache = self.resolver.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.as_ref().is_none_or(|(have, _)| *have < generation) {
            *cache = Some((generation, Arc::clone(&built)));
        }
        built
    }

    /// Times [`Self::resolver`] folded the paths. Tests.
    #[must_use]
    pub fn resolver_builds(&self) -> u64 {
        self.resolver_builds.load(Ordering::SeqCst)
    }

    /// Every note path delivered so far.
    #[must_use]
    pub fn note_paths(&self) -> Vec<String> {
        let dirs = self.lock();
        dirs.iter()
            .flat_map(|(dir, names)| {
                names.iter().map(move |name| {
                    if dir.is_empty() {
                        name.clone()
                    } else {
                        format!("{dir}/{name}")
                    }
                })
            })
            .collect()
    }
}

#[derive(Default)]
struct Work {
    rebuild_at: Option<Instant>,
    history_at: Option<Instant>,
    opens: Vec<(String, i64)>,
    stop: bool,
}

struct Shared {
    vault: Arc<Vault>,
    db: PathBuf,
    /// Whether the index has published `Ready` and not since left it. Set by
    /// [`Switcher::index_state_changed`] (the state the publisher announced), not read
    /// back from the status, which an observer may be called ahead of.
    ready: AtomicBool,
    tree: Arc<TreePaths>,
    work: Mutex<Work>,
    wake: Condvar,
    index_corpus: RwLock<Option<Arc<Corpus>>>,
    tree_corpus: Mutex<Option<(u64, Arc<Corpus>)>>,
    history: Mutex<History>,
    gate: Mutex<()>,
    /// Identity for in-flight queries.
    arrivals: AtomicU64,
    /// Queries that have arrived and not yet answered. A query is superseded only when a
    /// *later arrival* carries a *higher* `seq`: queries reach `query()` in scheduler
    /// order, so arrival order alone would discard the newest keystroke if it got
    /// the earlier ticket, and `seq` alone breaks after a reload (it restarts at 1). A
    /// lower `seq` arriving after a higher one supersedes nothing and is not superseded.
    inflight: Mutex<Vec<Inflight>>,
    builds: AtomicU64,
}

struct Inflight {
    ticket: u64,
    seq: u32,
    superseded: bool,
}

/// Removes a query's in-flight entry on every way out of `query`.
struct Leave<'a> {
    shared: &'a Shared,
    ticket: u64,
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        self.shared
            .inflight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|q| q.ticket != self.ticket);
    }
}

/// The switcher for one open vault.
pub struct Switcher {
    shared: Arc<Shared>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for Switcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Switcher").finish_non_exhaustive()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Switcher {
    /// A switcher over the index at `db`, with the tree's paths from `tree` for the
    /// time before the index is `Ready`.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses a thread.
    pub fn new(vault: Arc<Vault>, db: PathBuf, tree: Arc<TreePaths>) -> std::io::Result<Arc<Self>> {
        // The record is a ranking signal: unreadable or absent reads as empty.
        let mut existing: Option<Vec<u8>> = None;
        let _ = vault.update_history(|old| {
            existing = old.map(<[u8]>::to_vec);
            None
        });
        let shared = Arc::new(Shared {
            vault,
            db,
            ready: AtomicBool::new(false),
            tree,
            work: Mutex::new(Work::default()),
            wake: Condvar::new(),
            index_corpus: RwLock::new(None),
            tree_corpus: Mutex::new(None),
            history: Mutex::new(History::from_bytes(existing.as_deref())),
            gate: Mutex::new(()),
            arrivals: AtomicU64::new(0),
            inflight: Mutex::new(Vec::new()),
            builds: AtomicU64::new(0),
        });
        let thread = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("serat-switcher".to_owned())
            .spawn(move || thread.run())?;
        Ok(Arc::new(Self {
            shared,
            worker: Mutex::new(Some(worker)),
        }))
    }

    /// The index published a state. `Ready` schedules a snapshot at once; anything else
    /// drops the index snapshot, so the tree's answers until `Ready` returns (a rebuild
    /// replaces what the old snapshot described).
    pub fn index_state_changed(&self, ready: bool) {
        self.shared.ready.store(ready, Ordering::SeqCst);
        if ready {
            self.shared.schedule_rebuild(Duration::ZERO);
        } else {
            *self
                .shared
                .index_corpus
                .write()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    /// The index changed: rebuild [`REFRESH_DELAY`] after the last change.
    pub fn index_changed(&self) {
        self.shared.schedule_rebuild(REFRESH_DELAY);
    }

    /// A note was opened in an editor pane (never an embed, preview or search snippet):
    /// counted in memory now, written [`HISTORY_DELAY`] after the last open.
    pub fn note_opened(&self, path: &str) {
        let now = now_ms();
        self.shared
            .history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record_open(path, now);
        let mut work = self.shared.work();
        work.opens.push((path.to_owned(), now));
        work.history_at = Some(Instant::now() + HISTORY_DELAY);
        drop(work);
        self.shared.wake.notify_all();
    }

    /// Which corpus a query would be answered from now.
    #[must_use]
    pub fn corpus_source(&self) -> CorpusSource {
        match self.shared.index_snapshot() {
            Some(_) => CorpusSource::Index,
            None => CorpusSource::Tree,
        }
    }

    /// The tree fallback's link resolver, cached per tree generation.
    #[must_use]
    pub fn tree_resolver(&self) -> Arc<PathResolver> {
        self.shared.tree.resolver()
    }

    /// Snapshots built from the index so far. Tests.
    #[must_use]
    pub fn builds(&self) -> u64 {
        self.shared.builds.load(Ordering::SeqCst)
    }

    /// Answer one keystroke. Never fails and never waits for a build.
    #[must_use]
    pub fn query(&self, args: &SwitcherQueryArgs) -> SwitcherResults {
        let shared = &self.shared;
        let ticket = shared.arrivals.fetch_add(1, Ordering::SeqCst);
        {
            let mut inflight = shared
                .inflight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for earlier in inflight.iter_mut().filter(|q| q.seq < args.seq) {
                earlier.superseded = true;
            }
            inflight.push(Inflight {
                ticket,
                seq: args.seq,
                superseded: false,
            });
        }
        let _leave = Leave { shared, ticket };
        // Serialised: a query superseded while it waited for the line is discarded
        // instead of computed.
        let _turn = shared.gate.lock().unwrap_or_else(PoisonError::into_inner);
        let (corpus, source) = match shared.index_snapshot() {
            Some(corpus) => (corpus, SwitcherCorpus::Index),
            None => (shared.tree_snapshot(), SwitcherCorpus::Tree),
        };
        let superseded = shared
            .inflight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|q| q.ticket == ticket && q.superseded);
        if superseded {
            return SwitcherResults {
                seq: args.seq,
                corpus: source,
                items: Vec::new(),
                discarded: true,
            };
        }
        let history = shared
            .history
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let hits = corpus.query(&args.query, usize::from(args.limit), &history, now_ms());
        drop(history);
        SwitcherResults {
            seq: args.seq,
            corpus: source,
            items: hits.into_iter().map(item).collect(),
            discarded: false,
        }
    }

    /// Stop the worker, writing any frecency not yet written. Idempotent.
    pub fn shut_down(&self) {
        self.shared.work().stop = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for Switcher {
    fn drop(&mut self) {
        self.shut_down();
    }
}

impl Shared {
    fn work(&self) -> std::sync::MutexGuard<'_, Work> {
        self.work.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn schedule_rebuild(&self, delay: Duration) {
        self.work().rebuild_at = Some(Instant::now() + delay);
        self.wake.notify_all();
    }

    fn index_snapshot(&self) -> Option<Arc<Corpus>> {
        self.index_corpus
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The tree's snapshot, rebuilt on the querying thread only when the tree moved:
    /// paths and file names, no I/O.
    fn tree_snapshot(&self) -> Arc<Corpus> {
        let generation = self.tree.generation();
        let mut cache = self
            .tree_corpus
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((have, corpus)) = cache.as_ref()
            && *have == generation
        {
            return Arc::clone(corpus);
        }
        let corpus = Arc::new(Corpus::from_paths(self.tree.note_paths()));
        *cache = Some((generation, Arc::clone(&corpus)));
        corpus
    }

    fn rebuild(&self) {
        if !self.ready.load(Ordering::SeqCst) {
            return;
        }
        // A failed build keeps the last snapshot (goal 5).
        if let Ok(corpus) = Corpus::from_index(&self.db) {
            self.builds.fetch_add(1, Ordering::SeqCst);
            // Dropped if the index stopped being ready while this ran.
            if self.ready.load(Ordering::SeqCst) {
                *self
                    .index_corpus
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(corpus));
            }
        }
    }

    fn write_history(&self, opens: &[(String, i64)]) {
        if opens.is_empty() {
            return;
        }
        // Pure over the bytes it is given, as `update_history` requires (it may run twice).
        let _ = self.vault.update_history(|old| {
            let mut bytes = old.map(<[u8]>::to_vec);
            for (path, at) in opens {
                bytes = Some(History::bytes_after_open(bytes.as_deref(), path, *at));
            }
            bytes
        });
    }

    fn run(&self) {
        loop {
            let (rebuild, opens) = {
                let mut work = self.work();
                loop {
                    if work.stop {
                        let opens = std::mem::take(&mut work.opens);
                        drop(work);
                        self.write_history(&opens);
                        return;
                    }
                    let now = Instant::now();
                    let due = |at: Option<Instant>| at.is_some_and(|at| at <= now);
                    if due(work.rebuild_at) || due(work.history_at) {
                        let rebuild = due(work.rebuild_at);
                        if rebuild {
                            work.rebuild_at = None;
                        }
                        let opens = if due(work.history_at) {
                            work.history_at = None;
                            std::mem::take(&mut work.opens)
                        } else {
                            Vec::new()
                        };
                        break (rebuild, opens);
                    }
                    let next = [work.rebuild_at, work.history_at]
                        .into_iter()
                        .flatten()
                        .min();
                    work = match next {
                        Some(at) => {
                            self.wake
                                .wait_timeout(work, at.saturating_duration_since(now))
                                .unwrap_or_else(PoisonError::into_inner)
                                .0
                        }
                        None => self.wake.wait(work).unwrap_or_else(PoisonError::into_inner),
                    };
                }
            };
            if rebuild {
                self.rebuild();
            }
            self.write_history(&opens);
        }
    }
}

fn item(hit: Hit) -> SwitcherItem {
    SwitcherItem {
        path: hit.path,
        title: hit.title,
        matched: match hit.matched {
            Matched::Title => SwitcherMatch::Title,
            Matched::Alias => SwitcherMatch::Alias,
            Matched::Path => SwitcherMatch::Path,
            Matched::Heading => SwitcherMatch::Heading,
        },
        candidate: hit.candidate,
        highlight: hit
            .highlight
            .into_iter()
            .map(|(start, end)| Span { start, end })
            .collect(),
        heading: hit
            .heading
            .map(|(text, byte_start)| HeadingRef { text, byte_start }),
        link_text: hit.link_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serat_core::TreeOp;

    fn entry(name: &str, kind: TreeKind) -> TreeEntry {
        TreeEntry {
            name: name.to_owned(),
            kind,
            sidecar: false,
        }
    }

    fn listing(dir: &str, entries: Vec<TreeEntry>) -> TreeOut {
        TreeOut::Op(TreeOp::Listing {
            dir: dir.to_owned(),
            entries,
        })
    }

    #[test]
    fn the_tree_paths_follow_listings_upserts_and_removes() {
        let paths = TreePaths::default();
        paths.apply(&listing(
            "",
            vec![
                entry("a.md", TreeKind::Note),
                entry("img.png", TreeKind::Asset),
            ],
        ));
        paths.apply(&listing("sub", vec![entry("b.md", TreeKind::Note)]));
        assert_eq!(paths.note_paths(), ["a.md", "sub/b.md"]);

        paths.apply(&TreeOut::Op(TreeOp::Upsert {
            dir: "sub".to_owned(),
            entry: entry("c.md", TreeKind::Note),
            after: None,
        }));
        paths.apply(&TreeOut::Op(TreeOp::Remove {
            dir: "".to_owned(),
            name: "a.md".to_owned(),
        }));
        assert_eq!(paths.note_paths(), ["sub/b.md", "sub/c.md"]);

        // A removed directory takes everything beneath it.
        paths.apply(&TreeOut::Op(TreeOp::Remove {
            dir: "".to_owned(),
            name: "sub".to_owned(),
        }));
        assert!(paths.note_paths().is_empty());
        assert!(paths.generation() >= 5);
    }

    #[test]
    fn apply_bumps_generation_only_after_dirs_change() {
        let paths = Arc::new(TreePaths::default());
        let before = paths.generation();
        let held = paths.lock();
        let worker = Arc::clone(&paths);
        let apply = std::thread::spawn(move || {
            worker.apply(&listing("", vec![entry("a.md", TreeKind::Note)]));
        });
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            paths.generation(),
            before,
            "no bump while the change is pending"
        );
        drop(held);
        apply.join().expect("apply");
        assert!(paths.generation() > before);
        assert_eq!(paths.note_paths(), ["a.md"]);
    }

    // Task #185: the tree fallback's resolver is folded once per tree generation.
    #[test]
    fn the_link_resolver_is_built_once_per_tree_generation() {
        let paths = TreePaths::default();
        paths.apply(&listing("", vec![entry("a.md", TreeKind::Note)]));
        let first = paths.resolver();
        let second = paths.resolver();
        assert_eq!(
            paths.resolver_builds(),
            1,
            "an unchanged tree is not re-folded"
        );
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.resolve("a"), Some("a.md"));
        assert_eq!(first.resolve("b"), None);

        // Every mutation path invalidates: upsert, remove, listing.
        paths.apply(&TreeOut::Op(TreeOp::Upsert {
            dir: "sub".to_owned(),
            entry: entry("b.md", TreeKind::Note),
            after: None,
        }));
        assert_eq!(paths.resolver().resolve("b"), Some("sub/b.md"));
        assert_eq!(paths.resolver_builds(), 2);
        let _ = paths.resolver();
        assert_eq!(paths.resolver_builds(), 2);

        paths.apply(&TreeOut::Op(TreeOp::Remove {
            dir: "".to_owned(),
            name: "a.md".to_owned(),
        }));
        assert_eq!(paths.resolver().resolve("a"), None);
        assert_eq!(paths.resolver_builds(), 3);

        paths.apply(&listing("", vec![entry("z.md", TreeKind::Note)]));
        assert_eq!(paths.resolver().resolve("z"), Some("z.md"));
        assert_eq!(paths.resolver_builds(), 4);

        // The caller's earlier answer is still valid for what it saw.
        assert_eq!(first.resolve("a"), Some("a.md"));
    }

    /// Hold the line, let queries with the given seqs arrive in order, release, and
    /// return whether each was answered with items.
    fn arrive_in_order(seqs: &[u32]) -> Vec<bool> {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::write(dir.path().join("a.md"), "# a\n").expect("note");
        let paths = Arc::new(TreePaths::default());
        paths.apply(&listing("", vec![entry("a.md", TreeKind::Note)]));
        let vault = Arc::new(Vault::open(dir.path()).expect("vault"));
        let switcher = Switcher::new(vault, dir.path().join("none.db"), paths).expect("switcher");
        let turn = switcher
            .shared
            .gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut threads = Vec::new();
        for (n, seq) in seqs.iter().copied().enumerate() {
            let worker = Arc::clone(&switcher);
            threads.push(std::thread::spawn(move || {
                worker.query(&SwitcherQueryArgs {
                    seq,
                    query: "a".to_owned(),
                    limit: 5,
                })
            }));
            while switcher.shared.arrivals.load(Ordering::SeqCst) <= n as u64 {
                std::thread::sleep(Duration::from_millis(2));
            }
            // The entry is registered just after the ticket is taken.
            while switcher.shared.inflight.lock().expect("inflight").len() <= n {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        drop(turn);
        threads
            .into_iter()
            .map(|t| {
                let answer = t.join().expect("query");
                assert_eq!(answer.discarded, answer.items.is_empty());
                !answer.discarded
            })
            .collect()
    }

    #[test]
    fn a_query_is_superseded_only_by_a_later_arrival_with_a_higher_seq() {
        assert_eq!(arrive_in_order(&[1, 2]), [false, true], "the ordinary case");
        // Scheduler order put keystroke 2 first: both are answered, and the newest is
        // never the one discarded.
        assert_eq!(arrive_in_order(&[2, 1]), [true, true]);
        // After a reload the counter restarts: the low seq is not stale.
        assert_eq!(arrive_in_order(&[50, 1]), [true, true]);
    }

    #[test]
    fn messages_that_are_not_ops_change_nothing() {
        let paths = TreePaths::default();
        paths.apply(&TreeOut::WalkSettled { empty_folders: 0 });
        assert_eq!(paths.generation(), 0);
    }
}
