//! The seam — the only module in the harness that touches a Serat crate.
//!
//! Everything measured enters through one of the three functions below. They
//! return an error today because the entry points they need do not exist yet:
//! `serat-index` is scaffolded (task I1) and gets its rebuild in I4, its
//! incremental save path in I5; the cold-start marks are task S1 once the
//! architect's D2 defines them. **The jobs therefore land red on purpose.** A
//! budget job that skips when the thing it measures is absent is worse than no
//! job, because a green build then means nothing.
//!
//! Keeping the seam in one file is deliberate: when I4 lands, exactly one
//! function changes and no measurement code moves. The signatures below are the
//! request to `rust-core-dev` — anything that satisfies them satisfies the
//! harness.

use std::path::Path;
use std::time::Duration;

use crate::Samples;

/// How a rebuild's cost splits. P0 finding **F-10**.
///
/// NFR-PRF-04 is a wall-clock requirement and the budget still asserts the
/// aggregate. But a single number cannot answer architecture.md's open design
/// question 1 — *does the parse pool or the index writer dominate rebuild
/// time?* — and that question is what §1.2 says the parse pool exists to
/// settle. Reported as one figure, "the writer dominates" and "reading 39.4 MiB
/// off disk dominates" are indistinguishable, and they imply completely
/// different fixes.
///
/// The walk enumerates without hashing, so the split is real rather than
/// notional: four phases, four durations, reported beside the total.
#[derive(Clone, Copy, Debug, Default)]
pub struct RebuildPhases {
    /// Enumerating the vault. No file contents read.
    pub walk: Duration,
    /// Reading bytes and hashing them for the change gate (§3.1 step 3).
    pub read_hash: Duration,
    /// Markdown to blocks, fields, links, tasks, on the rayon pool.
    pub parse: Duration,
    /// Everything the single-writer actor did, including the final ANALYZE.
    pub write: Duration,
}

impl RebuildPhases {
    /// The four phases summed. Not the same as the wall clock, and deliberately
    /// so: phases overlap when the parse pool and the writer run concurrently,
    /// and a sum that exceeded the total would be the first evidence of that.
    pub fn sum(&self) -> Duration {
        self.walk + self.read_hash + self.parse + self.write
    }
}

/// One rebuild: the wall clock the budget asserts, and the breakdown F-10 asks
/// for.
#[derive(Clone, Copy, Debug, Default)]
pub struct RebuildTiming {
    /// Entry to `rebuild()` until it returns, writer drained and ANALYZE run.
    /// This is the number NFR-PRF-04 is judged on.
    pub total: Duration,
    /// Where the time went.
    pub phases: RebuildPhases,
    /// Notes the rebuild says it indexed, asserted against the vault manifest.
    pub notes: usize,
}

/// What the harness needs from `serat-index` before NFR-PRF-04 is measurable.
pub const REBUILD_ENTRY_POINT: &str = "NFR-PRF-04 needs, from serat-index (task I4):

    // Dependencies injected as serat-core traits, because a rebuild walks,
    // parses and writes, and no product crate may depend on two of those
    // (architecture 2.1). Shape confirmed by rust-core-dev; the exact spelling
    // is provisional until architect rules on where rebuild orchestration
    // lives. NoteSource is new in I4; NoteParser already exists.
    pub fn rebuild(
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        index_dir: &Path,
    ) -> Result<RebuildReport, IndexError>;

    pub struct RebuildReport {
        pub notes: usize, pub blocks: usize, pub links: usize, // ...
        // F-10: the four phases, so open design question 1 can be answered.
        pub walk: Duration, pub read_hash: Duration,
        pub parse: Duration, pub write: Duration,
    }

Call site:

    let vault  = serat_vault::Vault::open(vault_path)?;
    let parser = serat_parse::CommonMarkParser::new();
    let report = serat_index::rebuild(&vault, &parser, index_dir)?;

Callable from a plain binary with no Tauri app running, returning only after the
writer has drained and ANALYZE has run. The harness times the call with a
monotonic clock, does not time the vault generation, and asserts the returned
note count against the vault manifest -- a rebuild that indexed half the vault
quickly is the failure a wall-clock number cannot see on its own.

The four phase durations are the whole content of open design question 1. The
budget still asserts the total; the phases are recorded beside it.";

/// Two intervals, because P0 finding F-03 showed that "save to index" has two
/// defensible readings that differ by more than the budget itself.
#[derive(Clone, Debug, Default)]
pub struct SaveSamples {
    /// File written to row visible. What a user experiences, and what includes
    /// the 150 ms per-path coalescing window of architecture section 3.1 step 1.
    /// This reading cannot fit inside 50 ms by construction; measuring it anyway
    /// is how the gate report shows *why*, rather than asserting it is impossible.
    pub end_to_end: Samples,
    /// Watcher event dequeued to row visible. The pipeline's own cost, with the
    /// debounce excluded. The only reading of NFR-PRF-03 that is satisfiable under
    /// the current design, and therefore the one asserted -- pending the
    /// architect's ruling on F-03.
    pub pipeline: Samples,
}

/// What the harness needs before NFR-PRF-03 is measurable.
pub const SAVE_ENTRY_POINT: &str = "\
NFR-PRF-03 needs, from serat-index / serat-vault (task I5):

    // Open an already-built index over `vault`.
    pub fn open(vault: &Path, index_dir: &Path) -> Result<IndexHandle, IndexError>;

    // Submit one changed file and block until it is visible to a reader.
    // The budget is save -> queryable, so the harness must be able to observe
    // the point at which a query would see the change (ADR-0001's flush, or an
    // index.changed notification carrying the same guarantee).
    pub fn apply_and_wait(&self, path: &VaultPath, bytes: &[u8]) -> Result<(), IndexError>;

    // And, for F-03, the moment the watcher event left the queue, so the
    // 150 ms coalescing window is reported separately from the work:
    pub fn last_dequeued_at(&self) -> Instant;   // or a timestamped event

The harness edits 1,000 distinct notes, one at a time, and reports BOTH
intervals: file-written-to-visible, and dequeued-to-visible. Distinct notes
rather than one note a thousand times, because re-saving one note measures a
warm row and not the budget -- and the sample deliberately includes hub notes.
With 25% of inbound links held by 1% of notes and a max in-degree of 679, a save
that touches a hub does far more link-table work than the median, so a sample
that misses the hubs posts a p95 the real vault does not honour.
(rust-core-dev's point, and a good one.)

apply_and_wait bypasses the watcher by design, and that is the honest way to
measure this: no external change can reach the index in under the 150 ms
coalescing window, so the 50 ms budget can only mean Serat's own save path,
where the app submits directly rather than waiting to be told about its own
write. flush() means every operation submitted before the call is committed;
under WAL that is visible to a reader beginning a transaction afterwards, which
is the guarantee worth measuring.

F-03: end to end, the design misses 50 ms by construction, because the
coalescing window alone is 150 ms. Until the architect rules on which interval
the requirement means, the harness asserts the pipeline interval and prints the
end-to-end figure beside it, so the gate report states which number the claim
rests on instead of implying there was only ever one.";

/// What the harness needs before NFR-PRF-01 is measurable.
pub const COLD_START_ENTRY_POINT: &str = "\
NFR-PRF-01 needs, from serat-app (task S1) and a decision from D2:

    SERAT_STARTUP_TRACE=<file> serat.exe --vault <dir> --exit-after-ready

    ... writes one line of JSON and exits:
    {\"t0_to_window_visible_ms\": 312.4, \"t0_to_vault_usable_ms\": 803.1}

t0 is process spawn measured by the harness, not an Instant::now() inside main:
everything before main is cold-start cost the user feels. `window visible` must
be the first painted frame, not window creation. `vault usable` is D2's to
define, and the harness asserts whatever D2 says. Until D2 lands this job is
blocked rather than guessed at.";

/// Time one full rebuild of the vault into a throwaway index directory,
/// including the phase breakdown F-10 asks for.
///
/// # Errors
/// Today: always, because `serat-index` has no rebuild entry point yet.
pub fn full_rebuild(vault: &Path, index_dir: &Path) -> Result<RebuildTiming, String> {
    let _ = (vault, index_dir);
    // Touching the crates keeps the dependency edge real, so the day the entry
    // point lands there is nothing to wire up but the call itself.
    Err(format!(
        "serat-index (schema v{}, sqlite {}) exposes no rebuild entry point yet — task I4.\n\n{}",
        serat_index::SCHEMA_VERSION,
        serat_index::sqlite_version(),
        REBUILD_ENTRY_POINT
    ))
}

/// Time `iterations` single-note saves through to being visible in the index,
/// on both of F-03's intervals.
///
/// # Errors
/// Today: always, because the incremental save path does not exist yet.
pub fn save_to_index(
    vault: &Path,
    index_dir: &Path,
    iterations: usize,
) -> Result<SaveSamples, String> {
    let _ = (vault, index_dir, iterations);
    // serat-vault's hash gate is the first step of the path being measured
    // (architecture §3.1 step 3); calling it here proves the link, and the
    // measurement will use it for real.
    let _gate = serat_vault::hash_bytes(b"---\ntype: note\n---\n");
    Err(format!(
        "the incremental save path does not exist yet — task I5.\n\n{SAVE_ENTRY_POINT}"
    ))
}

/// Run `iterations` cold starts, returning the window-visible and vault-usable
/// timelines.
///
/// # Errors
/// Today: always, because neither the app nor D2's definition of the marks
/// exists.
pub fn cold_start(vault: &Path, iterations: usize) -> Result<(Samples, Samples), String> {
    let _ = (vault, iterations);
    Err(format!(
        "there is no application to start yet — tasks D2 and S1.\n\n{COLD_START_ENTRY_POINT}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seam_fails_loudly_and_says_what_is_missing() {
        // The point of these assertions is that a red job is legible: whoever
        // reads the CI log learns which task unblocks it, not merely that
        // something is wrong.
        let vault = Path::new("nowhere");
        let index = Path::new("nowhere/.serat");

        let e = full_rebuild(vault, index).expect_err("must not silently pass");
        assert!(e.contains("task I4"), "{e}");
        assert!(e.contains("rebuild("), "{e}");
        // F-10: the request for the four phases must survive edits to this text,
        // because they are what closes open design question 1.
        assert!(e.contains("F-10"), "{e}");
        assert!(e.contains("NoteSource"), "{e}");

        // And the phases must sum to something a reader can interpret.
        let phases = RebuildPhases {
            walk: Duration::from_millis(100),
            read_hash: Duration::from_millis(200),
            parse: Duration::from_millis(300),
            write: Duration::from_millis(400),
        };
        assert_eq!(phases.sum(), Duration::from_millis(1_000));

        let e = save_to_index(vault, index, 10).expect_err("must not silently pass");
        assert!(e.contains("task I5"), "{e}");

        let e = cold_start(vault, 20).expect_err("must not silently pass");
        assert!(e.contains("D2"), "{e}");
        assert!(e.contains("SERAT_STARTUP_TRACE"), "{e}");
    }
}
