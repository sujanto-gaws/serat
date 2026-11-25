//! Paths the walk or the watcher could not turn into a [`VaultPath`], reported
//! rather than silently dropped.
//!
//! Task #34's amendment (FR-VLT-01, design goal 5). A name illegal on the
//! platform this binary actually runs on — a reserved Windows device name found
//! on a synced-in vault, say — still cannot be indexed, and dropping it silently
//! is the defect code-reviewer-2 found: three ordinary Linux/macOS files vanished
//! from the tree, search and backlinks with nothing to say why. The fix is not
//! "index it anyway" (`serat-index` still needs one row per real path, on a
//! platform where the name is real), it is "say so": every refusal accumulates
//! here, where a caller — `serat-app`, eventually, as one status fact with the
//! list — can read it rather than infer a gap from a note that never appears.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// A callback fired when the log changes; see [`RefusalLog::on_change`].
type Notify = Arc<dyn Fn() + Send + Sync>;

/// A vault-relative-ish path that could not become a [`VaultPath`], and why.
///
/// `path` is the best available text for the offending entry -- the OS path
/// relative to the root when one exists, lossily rendered, because the whole
/// point is to name a thing the strict type could not represent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The path that was refused, in the least-lossy text available.
    pub path: String,
    /// Why the vault's name rules (`VaultPath::new_for_platform`), reached through
    /// [`super::Vault::relativise`] or the walk, refused it.
    pub reason: String,
}

/// How many distinct refused paths [`RefusalLog`] keeps. Past it, further new
/// paths are counted in [`RefusalLog::overflowed`] but not stored: a vault
/// refusing this many names has a bigger problem than this list, and the count
/// still says so (design goal 5: reported, not silent).
///
/// Must equal `REFUSED_NAMES_CAP` in `contracts/ipc.rs`, which the status count
/// saturates at; this crate cannot depend on the contract, so serat-app asserts it.
pub const CAPACITY: usize = 1000;

#[derive(Default)]
struct Inner {
    by_path: BTreeMap<String, String>,
    overflowed: usize,
    on_change: Option<Notify>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("by_path", &self.by_path)
            .field("overflowed", &self.overflowed)
            .finish_non_exhaustive()
    }
}

impl Inner {
    /// Store one refusal, the log's bound enforced here. Returns whether the log
    /// changed: a new path, a new reason, or one more refusal counted past the cap.
    fn insert(&mut self, refusal: Refusal) -> bool {
        if self.by_path.len() >= CAPACITY && !self.by_path.contains_key(&refusal.path) {
            self.overflowed += 1;
            return true;
        }
        self.by_path
            .insert(refusal.path, refusal.reason.clone())
            .as_ref()
            != Some(&refusal.reason)
    }

    /// Remove every entry at or below `scope` and return them; no clone of the rest.
    fn take_scope(&mut self, scope: &str) -> BTreeMap<String, String> {
        if scope.is_empty() {
            return std::mem::take(&mut self.by_path);
        }
        let below = format!("{scope}/");
        let mut keys: Vec<String> = self
            .by_path
            .range(below.clone()..)
            .map(|(path, _)| path)
            .take_while(|path| path.starts_with(&below))
            .cloned()
            .collect();
        if self.by_path.contains_key(scope) {
            keys.push(scope.to_owned());
        }
        keys.into_iter()
            .filter_map(|key| self.by_path.remove_entry(&key))
            .collect()
    }
}

/// A bounded, shareable record of refused paths, **one entry per file**.
///
/// Keyed by path: a file refused on ten walks is one refusal, re-recording
/// replaces the reason, and [`RefusalLog::clear`] (called at the start of every
/// walk) makes the log describe the vault as the last walk found it, so a file
/// that was renamed or deleted stops being reported. One log per open
/// [`crate::Vault`], shared by the walk and the watcher. The library prints
/// nothing: the data is returned and the application logs it.
#[derive(Debug, Clone, Default)]
pub struct RefusalLog(Arc<Mutex<Inner>>);

impl RefusalLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Call `f` whenever the stored set or the overflow count changes (a path added,
    /// removed or given a new reason; a refusal counted past the cap). Not called when
    /// a walk finds exactly what the log already holds. `f` runs on the thread that
    /// changed the log (a walk or the watcher), outside the log's lock; it must not
    /// block. One callback: a second call replaces the first.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        self.lock().on_change = Some(Arc::new(f));
    }

    /// Run `change` under the lock, which returns whether it altered what the log
    /// holds, and fire the callback, after releasing the lock, if it did.
    fn mutate(&self, change: impl FnOnce(&mut Inner) -> bool) {
        let mut inner = self.lock();
        let callback = if change(&mut inner) {
            inner.on_change.clone()
        } else {
            None
        };
        drop(inner);
        if let Some(callback) = callback {
            callback();
        }
    }

    /// Record a refusal, replacing any earlier one for the same path.
    pub(crate) fn record(&self, refusal: Refusal) {
        self.mutate(|inner| inner.insert(refusal));
    }

    /// Remove a path and every entry beneath it (a refused directory is one entry), as
    /// the watcher does when a refused name is found gone (task #188). Nothing removed
    /// is not a change and fires nothing.
    pub(crate) fn forget(&self, path: &str) {
        self.mutate(|inner| !inner.take_scope(path).is_empty());
    }

    /// Record a refusal as the watcher would, so serat-app's tests can drive a storm
    /// past the cap. Not for production use.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn record_for_test(&self, path: &str, reason: &str) {
        self.record(Refusal {
            path: path.to_owned(),
            reason: reason.to_owned(),
        });
    }

    /// Replace every entry at or below `scope` (`""`: the whole vault) with `found`,
    /// the refusals a walk of that subtree has just made. This is how entries leave:
    /// at the next walk that covers them, whichever walk it is (`walk`, `walk_tree`,
    /// `walk_dir`), so a renamed or deleted file stops being reported and the log is
    /// never empty while a walk is still running (#13 N1; task #14).
    ///
    /// `overflowed` counts refusals past the capacity since the last whole-vault
    /// walk; a walk of the whole vault resets it, a partial one leaves it. The
    /// replacement is one change: the callback fires at most once.
    pub(crate) fn replace_scope(&self, scope: &str, found: Vec<Refusal>) {
        self.mutate(|inner| {
            let overflowed = inner.overflowed;
            let removed = inner.take_scope(scope);
            if scope.is_empty() {
                inner.overflowed = 0;
            }
            for refusal in found {
                inner.insert(refusal);
            }
            // Changed unless the scope holds exactly what it held before.
            let now = if scope.is_empty() {
                std::mem::take(&mut inner.by_path)
            } else {
                inner.take_scope(scope)
            };
            let changed = now != removed || inner.overflowed != overflowed;
            inner.by_path.extend(now);
            changed
        });
    }

    /// Every refused path, in path order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Refusal> {
        self.lock()
            .by_path
            .iter()
            .map(|(path, reason)| Refusal {
                path: path.clone(),
                reason: reason.clone(),
            })
            .collect()
    }

    /// How many distinct paths are recorded right now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().by_path.len()
    }

    /// Whether nothing has been refused.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many further paths were refused after the log filled up.
    #[must_use]
    pub fn overflowed(&self) -> usize {
        self.lock().overflowed
    }
}

/// Build a [`Refusal`] from a `VaultError`, using the path it already carries.
///
/// Only [`serat_core::VaultError::InvalidPath`] is a name refusal in the sense
/// this module reports — a name the current platform cannot have at all.
/// Anything else (a path outside the vault root, an I/O failure) is a different
/// class of problem, decided by the caller, so this returns `None` rather than
/// guessing.
#[must_use]
pub(crate) fn from_invalid_path(error: &serat_core::VaultError) -> Option<Refusal> {
    match error {
        serat_core::VaultError::InvalidPath { path, reason } => Some(Refusal {
            path: path.clone(),
            reason: reason.clone(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(path: &str, reason: &str) -> Refusal {
        Refusal {
            path: path.to_owned(),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn forget_removes_the_path_and_what_is_beneath_it_and_nothing_else() {
        let log = RefusalLog::new();
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        log.on_change(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        for path in ["a\\b", "a\\b/c.md", "a\\bc", "z.md"] {
            log.record(refusal(path, "r"));
        }
        fired.store(0, std::sync::atomic::Ordering::SeqCst);

        log.forget("a\\b");
        let left: Vec<String> = log.snapshot().into_iter().map(|r| r.path).collect();
        assert_eq!(left, vec!["a\\bc", "z.md"]);
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Nothing to remove: no change, no callback.
        log.forget("a\\b");
        log.forget("absent");
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(log.len(), 2);
        assert_eq!(log.overflowed(), 0);
    }

    #[test]
    fn one_file_refused_many_times_is_one_entry_with_the_latest_reason() {
        let log = RefusalLog::new();
        for i in 0..10 {
            log.record(refusal("a\\b.md", &format!("reason {i}")));
        }
        assert_eq!(log.len(), 1);
        assert_eq!(log.snapshot()[0].reason, "reason 9");
    }

    #[test]
    fn a_whole_vault_replacement_forgets_what_the_walk_no_longer_finds() {
        let log = RefusalLog::new();
        log.record(refusal("x.md", "r"));
        log.replace_scope("", vec![refusal("y.md", "r")]);
        assert_eq!(log.len(), 1);
        assert_eq!(log.snapshot()[0].path, "y.md");
        log.replace_scope("", Vec::new());
        assert!(log.is_empty());
        assert_eq!(log.overflowed(), 0);
    }

    #[test]
    fn a_subtree_replacement_leaves_the_rest_alone_and_matches_whole_segments() {
        let log = RefusalLog::new();
        log.record(refusal("a/x.md", "r"));
        log.record(refusal("a/b/y.md", "r"));
        log.record(refusal("ab/z.md", "r"));
        log.record(refusal("c.md", "r"));
        log.replace_scope("a", vec![refusal("a/new.md", "r")]);
        let kept: Vec<String> = log.snapshot().into_iter().map(|r| r.path).collect();
        assert_eq!(kept, ["a/new.md", "ab/z.md", "c.md"]);
    }

    #[test]
    fn past_capacity_new_paths_are_counted_not_stored_and_known_paths_still_update() {
        let log = RefusalLog::new();
        for i in 0..CAPACITY + 5 {
            log.record(refusal(&format!("note-{i:05}.md"), "test"));
        }
        assert_eq!(log.len(), CAPACITY);
        assert_eq!(log.overflowed(), 5);
        log.record(refusal("note-00000.md", "updated"));
        assert_eq!(log.len(), CAPACITY);
        assert_eq!(log.snapshot()[0].reason, "updated");
    }

    #[test]
    fn on_change_fires_exactly_when_the_stored_set_or_overflow_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let log = RefusalLog::new();
        let fired = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&fired);
        log.on_change(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let count = || fired.load(Ordering::SeqCst);

        log.record(refusal("a.md", "r"));
        assert_eq!(count(), 1, "a new path");
        log.record(refusal("a.md", "r"));
        assert_eq!(count(), 1, "the same path and reason is no change");
        log.record(refusal("a.md", "r2"));
        assert_eq!(count(), 2, "a new reason");

        log.replace_scope("", vec![refusal("a.md", "r2")]);
        assert_eq!(count(), 2, "a walk that finds what is held is no change");
        log.replace_scope("", vec![refusal("b.md", "r"), refusal("c.md", "r")]);
        assert_eq!(count(), 3, "a replacement is one change");
        log.replace_scope("", Vec::new());
        assert_eq!(count(), 4, "entries leaving");
        log.replace_scope("", Vec::new());
        assert_eq!(count(), 4, "an empty log walked empty");

        for i in 0..CAPACITY {
            log.record(refusal(&format!("n-{i:05}.md"), "r"));
        }
        let full = count();
        log.record(refusal("one-more.md", "r"));
        assert_eq!(count(), full + 1, "a refusal counted past the cap");
        log.replace_scope("", Vec::new());
        assert_eq!(count(), full + 2, "emptying a full, overflowed log");
    }

    #[test]
    fn an_empty_log_reports_itself_as_empty() {
        let log = RefusalLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.snapshot().is_empty());
    }

    #[test]
    fn from_invalid_path_only_recognises_invalid_path_errors() {
        let invalid = serat_core::VaultError::InvalidPath {
            path: "CON.md".to_owned(),
            reason: "names a reserved Windows device".to_owned(),
        };
        let refusal = from_invalid_path(&invalid).expect("should recognise");
        assert_eq!(refusal.path, "CON.md");
        assert_eq!(refusal.reason, "names a reserved Windows device");

        let escapes = serat_core::VaultError::PathEscapesVault {
            path: "../elsewhere.md".to_owned(),
        };
        assert!(from_invalid_path(&escapes).is_none());
    }
}
