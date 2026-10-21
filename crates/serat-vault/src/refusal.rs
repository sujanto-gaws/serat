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

/// A vault-relative-ish path that could not become a [`VaultPath`], and why.
///
/// `path` is the best available text for the offending entry -- the OS path
/// relative to the root when one exists, lossily rendered, because the whole
/// point is to name a thing the strict type could not represent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The path that was refused, in the least-lossy text available.
    pub path: String,
    /// Why [`VaultPath::new`] or [`super::Vault::relativise`] refused it.
    pub reason: String,
}

/// How many distinct refused paths [`RefusalLog`] keeps. Past it, further new
/// paths are counted in [`RefusalLog::overflowed`] but not stored: a vault
/// refusing this many names has a bigger problem than this list, and the count
/// still says so (design goal 5: reported, not silent).
const CAPACITY: usize = 1000;

#[derive(Debug, Default)]
struct Inner {
    by_path: BTreeMap<String, String>,
    overflowed: usize,
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

    /// Record a refusal, replacing any earlier one for the same path.
    pub(crate) fn record(&self, refusal: Refusal) {
        let mut inner = self.lock();
        if inner.by_path.len() >= CAPACITY && !inner.by_path.contains_key(&refusal.path) {
            inner.overflowed += 1;
            return;
        }
        inner.by_path.insert(refusal.path, refusal.reason);
    }

    /// Replace every entry at or below `scope` (`""`: the whole vault) with `found`,
    /// the refusals a walk of that subtree has just made. This is how entries leave:
    /// at the next walk that covers them, whichever walk it is (`walk`, `walk_tree`,
    /// `walk_dir`), so a renamed or deleted file stops being reported and the log is
    /// never empty while a walk is still running (#13 N1; task #14).
    ///
    /// `overflowed` counts refusals past the capacity since the last whole-vault
    /// walk; a walk of the whole vault resets it, a partial one leaves it.
    pub(crate) fn replace_scope(&self, scope: &str, found: Vec<Refusal>) {
        let mut inner = self.lock();
        if scope.is_empty() {
            inner.by_path.clear();
            inner.overflowed = 0;
        } else {
            let below = format!("{scope}/");
            inner
                .by_path
                .retain(|path, _| path != scope && !path.starts_with(&below));
        }
        drop(inner);
        for refusal in found {
            self.record(refusal);
        }
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
