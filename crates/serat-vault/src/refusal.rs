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

use std::sync::{Arc, Mutex, PoisonError};

/// A vault-relative-ish path that could not become a [`VaultPath`], and why.
///
/// `path` is the best available text for the offending entry — the OS path
/// relative to the root when one exists, lossily rendered, because the whole
/// point is to name a thing the strict type could not represent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The path that was refused, in the least-lossy text available.
    pub path: String,
    /// Why [`VaultPath::new`] or [`super::Vault::relativise`] refused it.
    pub reason: String,
}

/// How many refusals [`RefusalLog`] keeps before it starts dropping the oldest.
///
/// A vault refusing at a rate that would exhaust memory has a bigger problem
/// than this log, so the log itself stays bounded (the same shape as
/// [`crate::echo::EchoSuppressor`]) — "unbounded growth" turns into "unbounded
/// but reported", which is design goal 5's own rule applied to this mechanism.
const CAPACITY: usize = 1000;

/// A bounded, shareable log of [`Refusal`]s, accumulated across the walk and the
/// watcher.
///
/// One log per open [`crate::Vault`] (like [`crate::echo::EchoSuppressor`]),
/// because the walk and the watcher must both be able to add to the same list —
/// a refusal the walk reported and the watcher reported again under a different
/// count would be two mechanisms disagreeing about one fact.
#[derive(Debug, Clone, Default)]
pub struct RefusalLog(Arc<Mutex<Vec<Refusal>>>);

impl RefusalLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a refusal, and print it to stderr.
    ///
    /// The `eprintln!` is the whole of "logged" until `serat-app` wires this to a
    /// real sink — deliberately no new dependency for that: `snapshot` and
    /// `len` are the structured, testable surface, and stderr is a bound of
    /// last resort so a refusal is visible even from a build that never reads
    /// either.
    pub(crate) fn record(&self, refusal: Refusal) {
        eprintln!(
            "serat-vault: refused to index {:?}: {}",
            refusal.path, refusal.reason
        );
        let mut log = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if log.len() >= CAPACITY {
            log.remove(0);
        }
        log.push(refusal);
    }

    /// Every refusal recorded so far, oldest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Refusal> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many refusals are recorded right now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether nothing has been refused.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    #[test]
    fn recording_beyond_capacity_drops_the_oldest() {
        let log = RefusalLog::new();
        for i in 0..CAPACITY + 5 {
            log.record(Refusal {
                path: format!("note-{i}.md"),
                reason: "test".to_owned(),
            });
        }
        assert_eq!(log.len(), CAPACITY);
        let snapshot = log.snapshot();
        assert_eq!(snapshot.first().unwrap().path, "note-5.md");
        assert_eq!(
            snapshot.last().unwrap().path,
            format!("note-{}.md", CAPACITY + 4)
        );
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
