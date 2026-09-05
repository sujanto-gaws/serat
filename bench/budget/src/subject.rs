//! The seam — the only module in the harness that touches a Serat crate.
//!
//! Everything measured enters through one of the three functions below. They
//! return an error today because something they need does not exist yet.
//!
//! **I4 (rebuild) and I5 (incremental apply) have both landed** on
//! `serat-index`. What is missing from both `full_rebuild()` and
//! `save_to_index()` is the same thing: a production `NoteParser`.
//! `serat-parse` (task I3) is still the I1 scaffold, and neither seam can be
//! assembled without excluding real parse cost from its total — which for
//! `save_to_index()` specifically would also mean asserting a number `apply()`
//! never actually produced, since parsing is on its critical path. See
//! [`SAVE_ENTRY_POINT`] and [`REBUILD_ENTRY_POINT`].
//!
//! `cold_start()` waits on S1 once architect's D2 defines the vault-usable
//! mark. **The jobs therefore land red on purpose.** A budget job that skips
//! when the thing it measures is absent is worse than no job, because a green
//! build then means nothing.
//!
//! Keeping the seam in one file is deliberate: when a function's last blocker
//! lands, only that function's body changes and no measurement code moves.
//! The signatures in the `*_ENTRY_POINT` constants below are the request to
//! `rust-core-dev` — anything that satisfies them satisfies the harness.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::Samples;
use crate::trace::StartupTrace;

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
    /// **How long the producer was blocked, not what the writer cost.**
    ///
    /// Measured around `submit_blocking` plus the barrier at the end. The
    /// writer's own SQL runs on its own thread, concurrently, and is charged to
    /// nobody. So the reading is the opposite of the obvious one:
    ///
    /// * near zero means the writer **kept up** — the producer never waited —
    ///   not that writing is cheap;
    /// * large means the writer **is** the bottleneck, because the only way
    ///   this grows is the queue being full.
    ///
    /// The field is named for what it measures, because this is the number a
    /// gate reader will use to answer open design question 1 — parse pool
    /// versus index writer — and a column called "write" holding a small value
    /// would settle that question backwards.
    pub producer_wait: Duration,
    /// Building `blocks_fts` with the FTS5 `rebuild` command after the bulk
    /// load, plus the final ANALYZE. Architect measured 1.9 s for 259,971
    /// blocks. Unlike `producer_wait` this is real writer-side work.
    ///
    /// Split from `write_rows` because the two have different fixes and
    /// reporting them together hides a 6x: with the triggers left live, the same
    /// insert cost 2.1-10.3 s at 26,000 blocks against 0.32 s with them dropped,
    /// against 0.32 s with them dropped - about 6x, reproduced independently at 5.8x.
    pub write_fts: Duration,
}

impl RebuildPhases {
    /// The four phases summed. Not the same as the wall clock, and deliberately
    /// so: phases overlap when the parse pool and the writer run concurrently,
    /// and a sum that exceeded the total would be the first evidence of that.
    pub fn sum(&self) -> Duration {
        self.walk + self.read_hash + self.parse + self.producer_wait + self.write_fts
    }

    /// Everything charged to the writer's side of the queue.
    pub fn write(&self) -> Duration {
        self.producer_wait + self.write_fts
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
    /// Links whose `dst_note` resolved to a note, asserted against the vault
    /// manifest's `link_degree.edges`.
    ///
    /// Link resolution is necessarily a second pass — a link can name a note
    /// not yet inserted — and rust-core-dev hit exactly the bug this exists to
    /// catch while writing it: counting rows *touched* rather than links
    /// *resolved*, so a dangling link came back reported as resolved. That is
    /// the rebuild-backpressure shape one layer up, a wrong number that looks
    /// right, and the manifest is the only independent oracle for it.
    pub links_resolved: usize,
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
        // notes counts MARKDOWN NOTES ONLY, excluding the 200 assets, so it can
        // be compared directly with the vault manifest's counts.notes. The 20k
        // vault holds 20,200 indexable files; if this returned 20,200 the
        // harness would reject every correct rebuild.
        pub notes: usize, pub blocks: usize, pub links: usize, // ...
        // F-10: the phases, so open design question 1 can be answered.
        // write is split because architect measured the FTS build as a
        // separate cost with a separate fix -- 3.7 s of row insert against
        // 1.9 s of FTS build at 259,971 blocks, and 6x worse with the
        // triggers left live.
        pub walk: Duration, pub read_hash: Duration, pub parse: Duration,
        // producer_wait is time blocked on submit_blocking plus the final
        // drain -- how long the producer WAITED, not what the writer cost.
        pub producer_wait: Duration, pub write_fts: Duration,

        // The oracles the harness checks against the vault manifest.
        pub notes: usize,           // Markdown notes only, excluding assets
        pub links_resolved: usize,  // links whose dst_note resolved to a note
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

The phase durations are the whole content of open design question 1. The budget
still asserts the total; the phases are recorded beside it.";

/// A save's timing, on the interval NFR-PRF-03 actually governs, plus the two
/// watcher-path readings kept for FR-VLT-04 and for continuity.
///
/// **Corrected by F-268, one layer inward from F-227.** Both fields this
/// struct used to have — `end_to_end` and `pipeline` — are watcher-path
/// intervals: `end_to_end` is "file written to row visible" and its own doc
/// said so (the 150 ms coalescing window is inside it); `pipeline` is
/// "watcher event dequeued to row visible". §3.1.1's actual save path has no
/// watcher and no dequeue at all — the buffer is written, the in-memory bytes
/// go straight to the parse pool, submit, visible — so neither field is an
/// interval NFR-PRF-03 can mean, and `measure_save_to_index` was asserting
/// `pipeline` regardless. F-227 warned that *benchmarking against the current
/// shell* would measure the watcher path by accident and report NFR-PRF-03
/// missed by an order of magnitude; this was the same wrong number **by
/// construction, inside the instrument built to prevent it**, and it read as
/// correct because `pipeline` sounds like the pipeline's own cost.
///
/// `save` is the fix: §3.1.1's save path, from submit through
/// `IndexHandle::apply()` to `writer().flush()` returning. It is what gets
/// asserted now. `end_to_end` and `pipeline` stay, reported rather than
/// asserted, because they are real numbers that answer a real question —
/// FR-VLT-04's 500 ms, for a change that arrives through the watcher.
#[derive(Clone, Debug, Default)]
pub struct SaveSamples {
    /// Which code path every sample in `save` was measured on. Must read
    /// `"save"` — [`crate::check_save_path`] refuses anything else. This is
    /// the harness's own claim about how it measured, since `apply()` returns
    /// `Result<(), IndexError>` and hands back no discriminant of its own
    /// (F-229) — the guard is a self-check on the measurement code, the same
    /// role `check_bulk_load_shape` plays for the rebuild. Defaults to `""`,
    /// which fails closed rather than open.
    pub path: &'static str,
    /// §3.1.1's save path: `apply()` submitted through `flush()` returning.
    /// No watcher, no debounce, no dequeue. **The asserted series.**
    pub save: Samples,
    /// File written (via the watcher) to row visible, including the 150 ms
    /// per-path coalescing window of architecture §3.1 step 1. FR-VLT-04's
    /// interval, not NFR-PRF-03's. Reported, never asserted against this
    /// budget (F-227).
    pub end_to_end: Samples,
    /// Watcher event dequeued to row visible: the pipeline's own cost with
    /// the debounce excluded, but still a **watcher-path** interval — the
    /// name is the reason this went unnoticed as long as it did (F-268).
    /// Reported beside `end_to_end`, never asserted.
    pub pipeline: Samples,
}

/// What the harness needs before NFR-PRF-03 is measurable.
///
/// **I5 landed** (`crates/serat-index/src/incremental.rs`), and its own module
/// doc states F-268's split independently: *"A save made in Serat goes
/// bytes → hash → parse → writer, with no watcher and no debounce. That is
/// what `IndexHandle::apply` serves and what NFR-PRF-03's 50 ms governs...
/// An external change goes through the watcher's 150 ms coalescing window...
/// That interval is FR-VLT-04's, whose criterion is 500 ms."* What is missing
/// is `NoteParser` — `apply()` takes one, and `serat-parse` (task I3) still
/// has none, the same blocker [`crate::full_rebuild`] is waiting on.
///
/// Corrected by **F-267** (itself a correction of F-230) and then again once
/// I5's real shape was visible: this constant is a `&str`, not compiled code,
/// so a stale call site here never broke the build — it read as a plausible
/// specification instead, the worse failure mode, because nothing forces a
/// reader to notice. Two things this text got wrong before I5 landed:
/// `IndexHandle::open` takes one argument, not `(vault_root, index_dir)`; and
/// `apply()` returns `Result<(), IndexError>`, not a timing struct — it does
/// not instrument itself, so the harness must time the call from the outside,
/// the same way it already times `rebuild()`'s wall clock.
pub const SAVE_ENTRY_POINT: &str = "\
NFR-PRF-03 needs, from serat-parse (task I3) -- serat-index's own half landed:

    let handle = IndexHandle::open(index_dir)?;
    let started = Instant::now();
    handle.apply(&vault, &parser, &echo, &path)?;   // submits directly, no watcher
    handle.writer().flush()?;                        // the barrier, visible at the call
    let elapsed = started.elapsed();                 // the harness's own timing; apply()
                                                       // returns () and instruments nothing

apply() then flush(), NOT a combined apply_and_wait(): a barrier must stay
visible where it is called (architecture §2.2). apply(); flush(); shows the
wait; a combined call hides one and is a copy-paste from the save path where
waiting is forbidden by design goal 2. There is no apply_and_wait anywhere in
serat-index -- it was never built, only ever the wrong entry this text used
to request (F-267, correcting F-230).

The full, landed signature is apply(&dyn NoteSource, &dyn NoteParser,
&dyn EchoGuard, &VaultPath) -> Result<(), IndexError>, then writer().flush().
NoteSource and NoteParser are the traits rebuild() already takes (architecture
§2.1); EchoGuard is what makes echo suppression (§3.2) a parameter the harness
can see rather than a private detail apply() reaches for on its own.

One submit per flush. flush() drains the whole queue, so submitting fifty and
flushing once would be a throughput figure wearing a latency label, and
NFR-PRF-03 is a p95 of individual saves.

The harness edits 1,000 distinct notes, one at a time, and reports the save
interval (asserted, tagged path = \"save\" -- crate::check_save_path refuses
anything else, F-229) beside the two watcher-path intervals (reported, F-227,
F-268): file-written-to-visible via the watcher, and watcher-event-dequeued-
to-visible. Distinct notes rather than one note a thousand times, because
re-saving one note measures a warm row and not the budget -- and the sample
deliberately includes hub notes. With 25% of inbound links held by 1% of notes
and a max in-degree of 679, a save that touches a hub does far more link-table
work than the median, so a sample that misses the hubs posts a p95 the real
vault does not honour. (rust-core-dev's point, and a good one.)

F-227, F-268: two paths, not one, and both must be visible in the result. A
save made in Serat has no watcher and no debounce -- that is what the 50 ms
governs, through apply()+flush(). An external change carries the watcher's
150 ms coalescing window and is FR-VLT-04's 500 ms. SaveSamples.save is the
former, asserted; .end_to_end and .pipeline are the latter, reported for
FR-VLT-04 and never asserted against NFR-PRF-03.";

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

/// How many rows `ANALYZE` recorded for the `notes` table, read straight out
/// of the index.
///
/// P0 finding **F-47**, and the only place in the project where the assertion
/// can bite. `commit()` re-runs ANALYZE when a batch touches more than 5% of
/// the notes; at test scale 5% is smaller than one batch, so the opportunistic
/// path fires every time and the rebuild's explicit `analyze()` is
/// unobservable. At 20,000 notes the threshold is 1,000 against a batch of 512,
/// so the tail — `20000 % 512` = 32 notes — never crosses it. Remove or reorder
/// the explicit call and the planner's statistics describe the index as it was
/// 32 notes ago, which is architecture §4.3's defect returning at the one scale
/// that can show it: `ix_notes_kind` over `ix_notes_type`, and every typed
/// query a full scan.
///
/// The harness reads `sqlite_stat1` itself rather than trusting a number the
/// code under test reports, for the same reason it recomputes the vault digest.
///
/// # Errors
/// When the index cannot be found or read.
pub fn stat1_note_rows(index_dir: &Path) -> Result<usize, String> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(index_dir)
        .map_err(|e| format!("reading {}: {e}", index_dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect();
    candidates.sort();
    let db = match candidates.len() {
        1 => candidates.remove(0),
        0 => return Err(format!("no .db file in {}", index_dir.display())),
        n => {
            return Err(format!(
                "{n} .db files in {}; the harness cannot tell which is the index",
                index_dir.display()
            ));
        }
    };

    let conn =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("opening {}: {e}", db.display()))?;
    let stat: String = conn
        .query_row(
            "SELECT stat FROM sqlite_stat1 WHERE tbl = 'notes' ORDER BY idx LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            format!(
                "reading sqlite_stat1 for notes from {}: {e}. No row means ANALYZE never ran, \
                 which is F-47 itself.",
                db.display()
            )
        })?;
    stat.split_whitespace()
        .next()
        .and_then(|first| first.parse().ok())
        .ok_or_else(|| format!("sqlite_stat1 for notes holds {stat:?}, which is not a row count"))
}

/// Time one full rebuild of the vault into a throwaway index directory,
/// including the phase breakdown F-10 asks for.
///
/// # Errors
/// Today: always, because `serat-index` has no rebuild entry point yet.
pub fn full_rebuild(vault: &Path, index_dir: &Path) -> Result<RebuildTiming, String> {
    // I3 landed a real `NoteParser`, which is the one thing this seam was
    // waiting for. Wiring it to serat-index's `cfg(test)` fake earlier would
    // have answered open design question 1 -- parse pool versus index writer --
    // by construction rather than by measurement (F-19, F-27), which is why the
    // seam stayed an error rather than being wired to whatever was available.
    let source = serat_vault::Vault::open(vault).map_err(|err| {
        format!(
            "could not open the vault under test at {}: {err}",
            vault.display()
        )
    })?;
    let parser = serat_parse::CommonMarkParser;

    // `total` brackets `rebuild()` alone. Opening the vault is setup and
    // generating the vault is not timed at all -- NFR-PRF-04 is judged on this.
    let started = Instant::now();
    let report = serat_index::rebuild(&source, &parser, index_dir)
        .map_err(|err| format!("rebuild failed: {err}"))?;
    let total = started.elapsed();

    // Phases come from the rebuild rather than from wrapping calls here: the
    // writer runs on its own thread, so only the rebuild can see where its own
    // time went. `producer_wait` reads the opposite of the obvious way -- see
    // RebuildPhases.
    Ok(RebuildTiming {
        total,
        phases: RebuildPhases {
            walk: report.walk,
            read_hash: report.read_hash,
            parse: report.parse,
            producer_wait: report.producer_wait,
            write_fts: report.write_fts,
        },
        notes: report.notes,
        links_resolved: report.links_resolved,
    })
}

/// Time `iterations` single-note saves through to being visible in the
/// index: the save-path interval NFR-PRF-03 governs, and the two watcher-path
/// intervals reported beside it (F-227, F-268).
///
/// # Errors
/// Today: always. `IndexHandle::apply` (I5) is real, on main, but there is no
/// production `NoteParser` to hand it — `serat-parse` (task I3) is still the
/// I1 scaffold, the same blocker [`full_rebuild`] is waiting on. See
/// [`SAVE_ENTRY_POINT`].
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
        "serat-parse exposes no NoteParser implementation yet — task I3.\n\n{SAVE_ENTRY_POINT}"
    ))
}

/// Locate the application binary the cold-start job drives.
///
/// `SERAT_BIN` overrides; otherwise the release build beside this crate's
/// target directory. Returns an error naming task S1 rather than a path that
/// does not exist, because "file not found" is not a useful CI log line.
///
/// # Errors
/// When the binary is absent.
pub fn app_binary() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("SERAT_BIN") {
        let path = PathBuf::from(explicit);
        return if path.exists() {
            Ok(path)
        } else {
            Err(format!(
                "SERAT_BIN points at {}, which does not exist",
                path.display()
            ))
        };
    }
    let name = if cfg!(windows) { "serat.exe" } else { "serat" };
    let candidate = crate::repo_root().join("target/release").join(name);
    if candidate.exists() {
        Ok(candidate)
    } else {
        Err(format!(
            "there is no application to start yet -- tasks D2 and S1. Looked for {}.

{}",
            candidate.display(),
            COLD_START_ENTRY_POINT
        ))
    }
}

/// Run `iterations` cold starts, returning the window-visible and vault-usable
/// timelines.
///
/// Each iteration is a fresh process. t0 is stamped immediately before
/// `spawn()` and handed to the app, so everything before `main` is inside the
/// number; every trace is validated before it becomes a sample.
///
/// # Errors
/// When the application does not exist, when a run fails or times out, or when
/// a trace is rejected by any of the checks in [`crate::trace`].
pub fn cold_start(vault: &Path, iterations: usize) -> Result<(Samples, Samples), String> {
    let binary = app_binary()?;
    let mut visible = Samples::new();
    let mut usable = Samples::new();

    for run in 0..iterations {
        let trace_path =
            std::env::temp_dir().join(format!("serat-startup-{}-{run}.json", std::process::id()));
        let _ = std::fs::remove_file(&trace_path);

        // t0: the instant the harness asked the OS for a process. Everything
        // before main -- loader, DLLs, webview init -- is on the far side of
        // this, which is the whole point of passing it in rather than letting
        // the app take its own Instant.
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("the system clock is before the epoch: {e}"))?
            .as_nanos();

        let status = Command::new(&binary)
            .arg("--vault")
            .arg(vault)
            .arg("--exit-after-ready")
            .env("SERAT_STARTUP_TRACE", &trace_path)
            .env("SERAT_STARTUP_T0_UNIX_NANOS", t0.to_string())
            .status()
            .map_err(|e| format!("spawning {}: {e}", binary.display()))?;
        if !status.success() {
            return Err(format!(
                "run {run} exited with {status}. If it reports no first-paint mark, the shell                  painted nothing -- which is a red job rather than a fast one, by design."
            ));
        }

        let line = std::fs::read_to_string(&trace_path)
            .map_err(|e| format!("reading {}: {e}", trace_path.display()))?;
        let trace = StartupTrace::parse(line.trim()).map_err(|e| format!("run {run}: {e}"))?;
        let _ = std::fs::remove_file(&trace_path);

        visible.push(trace.window_visible());
        usable.push(trace.vault_usable());
    }
    Ok((visible, usable))
}

/// What `Vault::resolve`'s symlink guard (T-07) costs across a whole vault.
///
/// **Not a budget.** There is no NFR threshold for this, and `resolve`'s own
/// doc comment already rules that design goal 1 (the vault survives the
/// application) outranks the cost if the two are ever in tension — that
/// ruling is not in question. What was missing is the number, not the
/// argument (task #30, F-279): the guard sits on `NoteSource::load`'s path,
/// so a rebuild pays it once per file, and F-123 says a justified decision
/// with only one side measured is not a decision the next person can check.
#[derive(Clone, Copy, Debug)]
pub struct ResolveOverhead {
    /// How many vault entries were resolved.
    pub files: usize,
    /// `symlink_metadata` (lstat) calls the guard made — one per path
    /// component per file, summed exactly rather than estimated, since every
    /// call is counted as it happens.
    pub lstat_calls: usize,
    /// Wall-clock time spent inside `resolve()`, summed across every file.
    pub elapsed: Duration,
}

impl ResolveOverhead {
    /// Average cost per file, in microseconds.
    #[must_use]
    pub fn micros_per_file(&self) -> f64 {
        if self.files == 0 {
            return 0.0;
        }
        self.elapsed.as_secs_f64() * 1_000_000.0 / self.files as f64
    }
}

/// Walk `vault` and call `resolve()` on every entry, timing the total.
///
/// Uses the real `Vault::walk` and `Vault::resolve` — not a synthetic loop —
/// so the count and the timing are the same call a rebuild will make once
/// `NoteSource::load` reaches this path (I3, task #8). `walk` already skips
/// symlinks as it discovers them, so every entry resolves successfully; a
/// resolve failure here means the vault changed under the walk, not that the
/// guard fired, and is reported as an error rather than silently excluded.
///
/// # Errors
/// A walk failure, or a resolve failure on a path the walk just reported.
pub fn measure_resolve_overhead(vault_path: &Path) -> Result<ResolveOverhead, String> {
    let vault = serat_vault::Vault::open(vault_path)
        .map_err(|e| format!("opening {}: {e}", vault_path.display()))?;
    let entries = vault
        .walk()
        .map_err(|e| format!("walking {}: {e}", vault_path.display()))?;

    let mut lstat_calls = 0usize;
    let started = Instant::now();
    for entry in &entries {
        vault.resolve(&entry.path).map_err(|e| {
            format!(
                "resolving {:?}, which the walk just reported: {e}",
                entry.path
            )
        })?;
        lstat_calls += entry.path.as_str().split('/').count();
    }
    let elapsed = started.elapsed();

    Ok(ResolveOverhead {
        files: entries.len(),
        lstat_calls,
        elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remaining_seams_fail_loudly_and_say_what_is_missing() {
        // The point of these assertions is that a red job is legible: whoever
        // reads the CI log learns which task unblocks it, not merely that
        // something is wrong.
        //
        // `full_rebuild` is no longer among them. I3 landed a real `NoteParser`
        // and the seam is now a live call, so its former assertions -- that it
        // errors naming task I4 -- would assert the opposite of the truth. What
        // replaces them is below: it must fail on the *vault*, not on a seam.
        let vault = Path::new("nowhere");
        let index = Path::new("nowhere/.serat");

        // Wired, so a missing vault is what stops it -- and it must still refuse
        // rather than return a number over nothing.
        let e = full_rebuild(vault, index).expect_err("a missing vault is not a rebuild");
        assert!(
            e.contains("could not open the vault"),
            "full_rebuild is wired now, so it should fail on the vault rather than on a seam: {e}"
        );
        assert!(
            !e.contains("task I4"),
            "the seam message outlived the seam; it now describes work that is done: {e}"
        );

        // And the phases must sum to something a reader can interpret.
        let phases = RebuildPhases {
            walk: Duration::from_millis(100),
            read_hash: Duration::from_millis(200),
            parse: Duration::from_millis(300),
            producer_wait: Duration::from_millis(370),
            write_fts: Duration::from_millis(190),
        };
        assert_eq!(phases.sum(), Duration::from_millis(1_160));
        assert_eq!(phases.write(), Duration::from_millis(560));

        let e = save_to_index(vault, index, 10).expect_err("must not silently pass");
        // I5 (apply/flush) is real on main; I3 (NoteParser) has now landed too,
        // so this seam is next and its request must stay legible.
        assert!(e.contains("task I3") || e.contains("I5"), "{e}");
        // F-267 corrected the entry point away from apply_and_wait, which §2.2
        // ruled against; the request must name the real composition and the
        // F-229 path discriminant, or a future implementer builds the wrong
        // shape from a plausible-looking spec (F-267's own point).
        assert!(e.contains("EchoGuard"), "{e}");
        assert!(e.contains("\"save\""), "{e}");
        assert!(e.contains("writer().flush()"), "{e}");

        let e = cold_start(vault, 20).expect_err("must not silently pass");
        assert!(e.contains("S1"), "{e}");
        assert!(e.contains("SERAT_STARTUP_TRACE"), "{e}");
    }

    // ------------------------------------------------------ T-07 / task #30

    #[test]
    fn measure_resolve_overhead_on_the_20k_vault() {
        // F-123: the doc comment on Vault::resolve rules that design goal 1
        // outranks this cost, and that ruling stands. What was missing is the
        // number, not the argument -- report it rather than leave "roughly
        // 40,000 syscalls" as an estimate nobody measured.
        let preset = crate::preset("20k").expect("the 20k preset exists");
        let vault = crate::ensure_vault("20k").expect("the 20k vault is available");
        let overhead = measure_resolve_overhead(&vault.path).expect("resolving every walked entry");

        assert_eq!(
            overhead.files,
            vault.notes + preset.assets,
            "resolve() ran over a different file count than the vault manifest reports -- \
             the walk skipped something or double-counted it"
        );
        // The claim task #30 opened on: this is the shape a full rebuild pays
        // it in, once NoteSource::load reaches this path (I3, task #8).
        assert!(
            overhead.lstat_calls >= overhead.files,
            "every file has at least one path component, so there must be at least one \
             lstat call per file"
        );

        eprintln!(
            "T-07 resolve() overhead (F-279, task #30): {} files, {} lstat calls, \
             {:.1} ms total, {:.2} us/file",
            overhead.files,
            overhead.lstat_calls,
            overhead.elapsed.as_secs_f64() * 1000.0,
            overhead.micros_per_file()
        );

        // A sanity bound, not a budget: no NFR governs this, and the doc
        // comment's ruling is not relitigated here. This exists only to catch
        // resolve() becoming pathologically slow -- canonicalize() instead of
        // symlink_metadata, say -- which the doc comment explicitly rejected
        // for exactly this reason.
        assert!(
            overhead.elapsed < Duration::from_secs(5),
            "resolve() over the whole 20k vault took {:?}, far more than the handful of \
             lstat calls per file this is supposed to cost -- worth a second look even \
             though no budget governs it",
            overhead.elapsed
        );
    }
}
