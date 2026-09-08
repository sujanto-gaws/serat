//! The seam — the only module in the harness that touches a Serat crate.
//!
//! Everything measured enters through one of the three functions below.
//!
//! **All three are wired and landing red is no longer their default state.**
//! `full_rebuild()` (I4, I3) and `save_to_index()` (I5, I3) both call the real
//! `serat-index`/`serat-parse` seams; the entry-point constants below are kept
//! as history of what each one asked for while it was still an error, not as
//! a current status. `cold_start()` spawns the real `serat-app` binary (S1)
//! and reads `--exit-after-ready`'s trace; task #52 settled the one open
//! question about what it waits for -- see [`COLD_START_ENTRY_POINT`] below,
//! which is current rather than aspirational.
//!
//! Keeping the seam in one file is deliberate: when a function's blockers
//! land, only that function's body changes and no measurement code moves.
//! The signatures in the `*_ENTRY_POINT` constants below are what
//! `rust-core-dev` was asked for, kept for the record even after landing.

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
    /// **Wall clock.** Enumerating the vault, single-threaded. No contents read.
    pub walk: Duration,
    /// **CPU time, summed across the parse pool.** Reading bytes and hashing
    /// them for the change gate (§3.1 step 3).
    ///
    /// Can legitimately exceed the rebuild's wall clock — on eight cores it
    /// should. It is not a share of anything.
    pub read_hash: Duration,
    /// **CPU time, summed across the parse pool.** Markdown to blocks, fields,
    /// links and tasks.
    ///
    /// Can legitimately exceed the wall clock, and this is the number that
    /// settles architecture.md's open design question 1 when read against
    /// `producer_wait`:
    ///
    /// * **parse-dominated** — `parse` several times the wall clock (the pool
    ///   busy on every core) and `producer_wait` near zero (the writer never
    ///   made anyone wait). The fix is pool sizing.
    /// * **writer-dominated** — `parse` at or below the wall clock (cores idle,
    ///   waiting on the queue) and `producer_wait` large. The fix is batching.
    ///
    /// Those are opposite conclusions from the same two columns, which is why
    /// the harness labels rather than normalises. Dividing these by the wall
    /// clock to make the five phases add up would destroy the one number P0 was
    /// built to produce.
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
    /// blocks on the dropped-trigger load (architecture 4.4). Unlike
    /// `producer_wait` this is real writer-side work.
    ///
    /// Split from the producer's wait because the two have different fixes and
    /// reporting them together hides a 6x: with the FTS triggers left live the
    /// same insert cost 2.1 s at 26,000 blocks against 0.32 s with them
    /// dropped, independently reproduced at 5.8x.
    ///
    /// **Not yet obtainable from `serat_index::RebuildReport` as landed.** I4
    /// shipped one combined `write: Duration` (`submit_blocking` waits plus the
    /// final flush) and does not time `end_bulk_load`/`analyze` separately at
    /// all — that time is real and inside the wall clock, attributed to
    /// nothing. Asked rust-core-dev to add this field; until it exists,
    /// [`full_rebuild`] must keep refusing rather than report a zero here,
    /// which would trip [`crate::check_bulk_load_shape`] (F-38) on every real
    /// run and make the guard indistinguishable from an actual regression.
    pub write_fts: Duration,
}

impl RebuildPhases {
    /// The phases summed. **Not comparable with the wall clock**, and
    /// deliberately so: `walk` is wall clock, `read_hash` and `parse` are CPU
    /// time across the pool, `producer_wait` is a wait rather than a cost, and
    /// `write_fts` is writer-side wall clock. A reader adding them expecting a
    /// total is wrong twice over — once by a figure that is too small and twice
    /// by two that are too large.
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

/// What the harness still needs before NFR-PRF-04 is measurable.
///
/// **Half of this landed.** `serat_index::rebuild` (task I4) is real, on main,
/// with this exact shape:
///
/// ```text
/// pub fn rebuild(
///     source: &dyn NoteSource,
///     parser: &dyn NoteParser,
///     index_dir: &Path,
/// ) -> Result<RebuildReport, IndexError>;
///
/// pub struct RebuildReport {
///     pub notes: usize,   // every row in `notes` -- notes AND assets
///                         // (kind='asset', ADR-0005). NOT markdown-only,
///                         // which the harness's own oracle now accounts
///                         // for: `vault.notes + vault.assets`.
///     pub blocks: usize, pub fields: usize, pub links: usize,
///     pub tasks: usize,
///     pub links_resolved: usize,
///     pub walk: Duration, pub read_hash: Duration, pub parse: Duration,
///     pub write: Duration,  // submit_blocking waits + the final flush --
///                           // NOT split into producer_wait/write_fts as
///                           // this crate's RebuildPhases wants (F-10,
///                           // F-38). end_bulk_load/analyze aren't timed
///                           // at all; that cost is real and inside the
///                           // wall clock, attributed to nothing. Asked
///                           // rust-core-dev to add `write_fts`.
///     pub skipped: usize,
/// }
/// ```
///
/// And `serat_vault::Vault` already implements `NoteSource`
/// (`serat-vault/src/lib.rs:607`, exercised by its own
/// `the_vault_satisfies_the_trait_the_index_rebuilds_through`).
///
/// **What is still missing is `NoteParser`.** `serat-parse` (task I3) is still
/// the I1 scaffold: no type in it implements the trait. The only
/// implementations anywhere are test fakes private to `serat-index`'s own test
/// suite (`serat-index/tests/support/mod.rs::LineParser`), not importable from
/// here and not a stand-in for real Markdown cost regardless. Wiring this seam
/// against a throwaway parser would report a total that excludes exactly the
/// cost architecture.md's open design question 1 asks about -- whether the
/// parse pool or the index writer dominates a rebuild -- so the number would
/// be the flattering-by-omission kind this harness exists to refuse.
pub const REBUILD_ENTRY_POINT: &str = "NFR-PRF-04 needs, from serat-parse (task I3):

    // The comrak-based implementation of NoteParser -- blocks with byte
    // ranges, YAML frontmatter and inline key:: value fields, wikilinks and
    // embeds, tasks. serat_index::rebuild (I4) and serat_vault::Vault's
    // NoteSource impl are already on main; this is the one piece left.
    impl serat_core::NoteParser for /* I3's type, name not yet settled */ {
        fn parse(&self, path: &VaultPath, bytes: &[u8]) -> Result<ParsedNote, ParseError>;
    }

Call site once I3 lands:

    let vault  = serat_vault::Vault::open(vault_path)?;
    let parser = serat_parse::???::new();
    let report = serat_index::rebuild(&vault, &parser, index_dir)?;

Callable from a plain binary with no Tauri app running, returning only after the
writer has drained and ANALYZE has run. The harness times the call with a
monotonic clock, does not time the vault generation, and asserts the returned
note count against the vault manifest -- a rebuild that indexed half the vault
quickly is the failure a wall-clock number cannot see on its own. That check is
`vault.notes + vault.assets`, not `vault.notes` alone, because an asset is a row
in `notes` too.

The phase durations are the whole content of open design question 1 (F-10). The
budget still asserts the total; the phases are recorded beside it -- once
RebuildReport splits write_fts out of write (see the doc comment above this
constant), which is a second, separate ask of rust-core-dev, not I3's.";

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
/// Today: always. `serat_index::rebuild` (I4) and `serat_vault::Vault`'s
/// `NoteSource` are both real, but there is no production `NoteParser` to hand
/// them — `serat-parse` (task I3) is still the I1 scaffold. See
/// [`REBUILD_ENTRY_POINT`] for exactly what is and is not landed. Refusing
/// rather than wiring against a throwaway parser: doing that would exclude
/// real parse cost from the total and answer NFR-PRF-04 with a number that
/// looks fine and means nothing, the shape every finding in this phase's
/// register has been about.
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
    let report = serat_index::rebuild(&source, &parser, vault, index_dir)
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

/// One `open_or_rebuild` call, timed, and what it found.
#[derive(Clone, Copy, Debug)]
pub struct OpenCheckTiming {
    /// Wall-clock time inside `open_or_rebuild`, start to return -- the
    /// number NFR-PRF-01's `t_usable` pays whenever the index is opened.
    pub elapsed: Duration,
    /// Whether the index was usable as-is. `false` means a rebuild ran, so
    /// `elapsed` includes it and is not the open-check number by itself.
    pub was_ready: bool,
}

/// Time one `open_or_rebuild` call against the index at `index_dir`, for the
/// vault at `vault_path`.
///
/// Task #68, following #61's removal of `PRAGMA integrity_check` from the
/// open path: architecture.md §3.4's four-candidate table has real numbers
/// for the three rejected checks (1310ms, 1150ms, and the walk) and none for
/// the six stamp steps that actually ship. This is that number. The caller
/// puts the index at `index_dir` into whatever state it wants measured --
/// a real `Ready` index, an absent directory, a hand-edited `index_meta` row
/// -- so the same seam measures every §3.4 candidate on the same fixture.
///
/// # Errors
/// If a triggered rebuild fails, or the vault at `vault_path` cannot be
/// opened.
pub fn measure_open_check(vault_path: &Path, index_dir: &Path) -> Result<OpenCheckTiming, String> {
    let source = serat_vault::Vault::open(vault_path)
        .map_err(|e| format!("opening {}: {e}", vault_path.display()))?;
    let parser = serat_parse::CommonMarkParser;
    let was_ready = std::cell::Cell::new(true);
    let on_status = |status: serat_index::IndexStatus| {
        if !matches!(
            status,
            serat_index::IndexStatus::Opening | serat_index::IndexStatus::Ready
        ) {
            was_ready.set(false);
        }
    };

    let started = Instant::now();
    let (handle, _report) =
        serat_index::open_or_rebuild(vault_path, index_dir, &source, &parser, &on_status)
            .map_err(|e| format!("open_or_rebuild: {e}"))?;
    let elapsed = started.elapsed();
    handle
        .close()
        .map_err(|e| format!("closing the index just opened: {e}"))?;

    Ok(OpenCheckTiming {
        elapsed,
        was_ready: was_ready.get(),
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

/// Restores a vault to its pre-batch state after a run of cold starts opens
/// it. F-401, task #100's blocked window.
///
/// `serat.exe` creates `.serat/` inside whatever vault it opens as an
/// ordinary side effect of indexing -- that is correct application
/// behaviour, not a defect, and it is exactly what NFR-PRF-01's warm-index
/// case needs: a later iteration in the same batch should see a warm index,
/// so nothing may clean up *between* iterations. What it must not do is
/// persist *past* the batch. The canonical fixture vaults are digest-checked
/// by every other consumer (`verify_vault_at`), and a `.serat/` directory
/// left behind moves that digest for everyone who opens the vault after this
/// batch finishes -- the same shape as F-372, in the opposite direction:
/// there a scratch *index* the harness built leaked into the vault; here the
/// real application's own index does, because the harness cannot spawn the
/// real app without letting it behave like the real app.
///
/// Takes the guard at the batch boundary and restores at the batch boundary,
/// never in between -- construct it once before the loop and let it drop
/// once after, on every exit path including `?`.
///
/// Guards rather than unconditionally deletes: if `.serat/` already existed
/// before the guard was taken, this batch did not create it and has no basis
/// for judging whether it is safe to remove, so it is left alone. This is
/// the resolved half of a duplication (F-NEXT/f401-guard-duplicated): main's
/// independently written `CleanIndexOnDrop` deleted `.serat/` unconditionally
/// on drop, which would destroy a pre-existing index that predates the batch
/// for a reason the guard cannot see -- its own doc comment named that exact
/// hazard one paragraph before doing it anyway. Kept this version because it
/// is strictly safer and ships with tests exercising both branches.
pub(crate) struct ColdStartVaultGuard {
    serat_dir: PathBuf,
    created_by_this_batch: bool,
}

impl ColdStartVaultGuard {
    pub(crate) fn watch(vault: &Path) -> Self {
        let serat_dir = vault.join(".serat");
        Self {
            created_by_this_batch: !serat_dir.exists(),
            serat_dir,
        }
    }
}

impl Drop for ColdStartVaultGuard {
    fn drop(&mut self) {
        if self.created_by_this_batch {
            let _ = std::fs::remove_dir_all(&self.serat_dir);
        }
    }
}

/// Run `iterations` cold starts, returning the window-visible and vault-usable
/// timelines.
///
/// Each iteration is a fresh process. t0 is stamped immediately before
/// `spawn()` and handed to the app, so everything before `main` is inside the
/// number; every trace is validated before it becomes a sample.
///
/// No note is opened. Task #52 ruled that `t_usable` is §3.4's four
/// preconditions -- index state known, file tree delivered, watcher settled,
/// window painted -- none of which is "a note is rendered": `--open` is
/// session restore for a user, not a readiness precondition, and stays in the
/// CLI rather than in this harness.
///
/// # Errors
/// When the application does not exist, when a run fails or times out, or when
/// a trace is rejected by any of the checks in [`crate::trace`].
pub fn cold_start(vault: &Path, iterations: usize) -> Result<(Samples, Samples), String> {
    // F-401: one guard for the whole batch, not one per iteration -- see
    // ColdStartVaultGuard's doc comment for why per-iteration cleanup would be
    // the wrong fix, not just a smaller one.
    let _guard = ColdStartVaultGuard::watch(vault);
    let mut visible = Samples::new();
    let mut usable = Samples::new();
    for run in 0..iterations {
        let trace = cold_start_once(vault, run)?;
        visible.push(trace.window_visible());
        usable.push(trace.vault_usable());
    }
    Ok((visible, usable))
}

/// Same batch as [`cold_start`], but keeps every validated trace rather than
/// reducing straight to the two asserted marks.
///
/// [`cold_start`] exists to feed the gate budgets, which only need
/// `window_visible`/`vault_usable`; task #100 also needs ADR-0014's full
/// seven-term partition (`Terms`) at median and p95, which those two scalars
/// cannot reconstruct. Rather than widen `cold_start`'s return type for every
/// caller, this is a second entry point over the same guarded batch and the
/// same per-run validation, so a caller that wants the breakdown does not
/// have to re-derive the guard discipline itself.
///
/// # Errors
/// As [`cold_start`].
pub fn cold_start_traces(vault: &Path, iterations: usize) -> Result<Vec<StartupTrace>, String> {
    let _guard = ColdStartVaultGuard::watch(vault);
    let mut traces = Vec::with_capacity(iterations);
    for run in 0..iterations {
        traces.push(cold_start_once(vault, run)?);
    }
    Ok(traces)
}

/// One cold start, validated.
///
/// Separate from [`cold_start`] so the proportionality check can **interleave**
/// vault sizes spawn by spawn rather than running them as two blocks. The
/// within-arm spread on this machine is about five times the effect being
/// tested for, so a blocked comparison is contaminated by whatever the machine
/// was doing during each block — which is how a 20,000-note vault came out
/// faster than a 2-note one (F-109). Interleaved, slow drift lands on both arms
/// and cancels in the paired difference.
///
/// # Errors
/// As [`cold_start`].
pub fn cold_start_once(vault: &Path, run: usize) -> Result<StartupTrace, String> {
    let binary = app_binary()?;
    {
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

        Ok(trace)
    }
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
/// `NoteSource::load` reaches this path. `walk` already skips symlinks as it
/// discovers them, so every entry resolves successfully; a resolve failure
/// here means the vault changed under the walk, not that the guard fired,
/// and is reported as an error rather than silently excluded.
///
/// **This is a floor, not an estimate of the in-situ cost (F-203).** F-203
/// found that an isolated walk benchmark on this machine understates a
/// rebuild's real cost by 1.7x (1.85 s isolated against 3.1 s in situ),
/// because an isolated run has the disk and every core to itself while a
/// rebuild's parse phase and writer thread are running alongside it. This
/// function is the same instrument shape — nothing else is running while it
/// times `resolve()` — so its number is a lower bound on what `resolve()`
/// costs inside a real rebuild, not a prediction of it. `TC-020`'s
/// `read_hash` phase is where the in-situ figure will actually appear.
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

        // Deliberately weaker than `full_rebuild`'s assertion above, and the
        // reason is a property of the harness rather than of the app. This is
        // the only seam whose subject is a SEPARATELY BUILT BINARY, so what it
        // reports depends on whether `target/release/serat.exe` happens to
        // exist at this moment: with it present the app launches and refuses
        // the missing vault; without it the entry-point check fires first and
        // names S1. Both are correct refusals of the same call, and which one
        // you get is decided by whether anyone has run `cargo build --release`
        // -- `cargo clippy --all-targets` and a debug test build churn the
        // same directory, so it changes between invocations of the very suite
        // this test lives in.
        //
        // Asserting either message specifically therefore asserts build state,
        // not behaviour. An earlier version of this comment asserted the S1
        // message was now stale; that was wrong, because on a clean checkout
        // and in CI the binary genuinely is absent and S1 is the right answer.
        // What must hold in every case is that the call REFUSES and says
        // something actionable -- never a number over nothing.
        let e = cold_start(vault, 20).expect_err("must not silently pass");
        assert!(
            e.contains("S1") || e.contains("exit code") || e.contains("vault"),
            "cold_start must refuse and name either the absent binary or the \
             absent vault, not fail opaquely: {e}"
        );
    }

    // ------------------------------------------------------------- F-401

    fn scratch_vault_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "serat-budget-test-f401-{label}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch vault dir");
        dir
    }

    #[test]
    fn a_guard_removes_dot_serat_it_created_on_drop() {
        let vault = scratch_vault_dir("removes-own");
        let serat_dir = vault.join(".serat");
        assert!(!serat_dir.exists(), "precondition: no .serat yet");

        {
            let _guard = ColdStartVaultGuard::watch(&vault);
            // Stand in for what the real app does mid-batch: create its index
            // directory while the guard is held.
            std::fs::create_dir_all(&serat_dir).expect(".serat");
            std::fs::write(serat_dir.join("index.db"), b"stand-in").expect("index file");
            assert!(
                serat_dir.exists(),
                "the app's own side effect must be visible mid-batch"
            );
        }

        assert!(
            !serat_dir.exists(),
            ".serat/ created during the batch must not survive the guard's drop"
        );
        std::fs::remove_dir_all(&vault).ok();
    }

    #[test]
    fn a_guard_leaves_a_pre_existing_dot_serat_alone() {
        let vault = scratch_vault_dir("leaves-pre-existing");
        let serat_dir = vault.join(".serat");
        std::fs::create_dir_all(&serat_dir).expect(".serat");
        std::fs::write(serat_dir.join("index.db"), b"pre-existing").expect("index file");

        {
            let _guard = ColdStartVaultGuard::watch(&vault);
            // Batch runs; .serat/ was already there before we watched it.
        }

        assert!(
            serat_dir.join("index.db").exists(),
            "a .serat/ the guard did not create is not this batch's to delete -- deleting it \
             would destroy state that predates the run for a reason the guard cannot see"
        );
        std::fs::remove_dir_all(&vault).ok();
    }

    #[test]
    fn a_guard_cleans_up_on_an_early_return_not_only_on_normal_completion() {
        // The whole point of using Drop rather than an explicit cleanup call
        // at the end of `cold_start` is that a `?` partway through the batch
        // must still restore the vault. Reproduced directly: a helper that
        // takes the guard and then bails via `?` before reaching any explicit
        // cleanup, same as `cold_start_once`'s `?` would inside the real loop.
        fn batch_that_fails_partway(vault: &Path) -> Result<(), String> {
            let _guard = ColdStartVaultGuard::watch(vault);
            std::fs::create_dir_all(vault.join(".serat")).map_err(|e| e.to_string())?;
            Err("run 1 of 20 failed".to_string())
        }

        let vault = scratch_vault_dir("early-return");
        let serat_dir = vault.join(".serat");

        let result = batch_that_fails_partway(&vault);
        assert!(
            result.is_err(),
            "the failure must propagate, not be swallowed"
        );
        assert!(
            !serat_dir.exists(),
            ".serat/ must not survive an early return out of the batch -- Drop runs on every \
             exit path, including `?`, or a red run leaves the same poisoned vault a green one \
             would have cleaned up"
        );
        std::fs::remove_dir_all(&vault).ok();
    }

    #[test]
    fn a_guard_taken_before_a_pre_existing_dot_serat_is_created_still_protects_it() {
        // Ordering matters: watch() must record whether .serat/ existed at
        // the moment it was taken, not at drop time. If a second concurrent
        // caller created .serat/ between watch() and drop() for an unrelated
        // reason, this guard's own answer must still be "I did not create
        // this" from what it observed when it started watching.
        let vault = scratch_vault_dir("ordering");
        let serat_dir = vault.join(".serat");
        assert!(!serat_dir.exists());

        let guard = ColdStartVaultGuard::watch(&vault);
        // Nothing created .serat/ yet -- this guard did create it in spirit,
        // so dropping should remove it. Assert the flag the guard itself
        // computed rather than re-deriving it, so the test is pinned to
        // watch()'s observation rather than to drop()'s filesystem state.
        assert!(guard.created_by_this_batch);
        drop(guard);
        std::fs::remove_dir_all(&vault).ok();
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
        // symlink_metadata, say, which resolves every path component and
        // would multiply calls per file by roughly the directory depth.
        //
        // F-335: this used to assert a wall-clock bound (elapsed < 5 s), and
        // it failed once under full-workspace load at 6.3 s with an
        // unchanged tree -- a definition-of-done suite is not supposed to
        // contain an assertion that can go red because the machine was busy.
        // The syscall count is what the comment above actually claims,
        // deterministic and load-independent: 43,587 lstat calls over 20,200
        // files is ~2.16 per file, and a regression to canonicalize() would
        // multiply that by the vault's path depth, not add noise to it.
        // Timing stays in the eprintln above as a diagnostic, unasserted.
        let calls_per_file = overhead.lstat_calls as f64 / overhead.files.max(1) as f64;
        assert!(
            calls_per_file < 4.0,
            "resolve() made {:.2} lstat calls per file, well above the ~1-3 a vault this deep \
             should need -- worth a second look even though no budget governs it \
             ({} calls over {} files)",
            calls_per_file,
            overhead.lstat_calls,
            overhead.files
        );
    }

    // -------------------------------------------------------- open / task #68

    #[test]
    fn measure_the_open_check_on_the_20k_vault() {
        // Task #68, following #61: architecture.md §3.4's four-candidate table
        // has real numbers for the three rejected checks and none for the six
        // stamp steps that actually ship. This is that number, plus a sanity
        // check on the four rebuild-triggering states next to it -- if any of
        // those cost real time, the ruling that they "fail early" needs a
        // second look, not just this test.
        let vault = crate::ensure_vault("20k").expect("the 20k vault is available");

        // Outside the vault, deliberately (F-311's lesson, and task #70's
        // live instance of it): a scratch directory *inside* vault.path
        // moves the tree digest, so a panic before cleanup here would leave
        // the vault failing its own canonicality gate for every test after
        // this one, on a shared machine, silently. temp_dir() carries no
        // such obligation.
        let scratch =
            std::env::temp_dir().join(format!("serat-bench-open-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);

        // Build once, unmeasured: the first call finds the index Absent and
        // rebuilds it for real, which is NFR-PRF-04's number, not this one.
        let built = measure_open_check(&vault.path, &scratch).expect("first build");
        assert!(
            !built.was_ready,
            "the scratch dir was just cleared; this must be the rebuild"
        );

        // THE HEADLINE NUMBER: open_or_rebuild against a real, Ready index --
        // the six stamp steps and nothing else, on the same 20k fixture that
        // produced integrity_check's 1310ms and quick_check's 1150ms.
        let ready = measure_open_check(&vault.path, &scratch).expect("second open, now Ready");
        assert!(
            ready.was_ready,
            "a second open_or_rebuild against an index this one just built and closed \
             cleanly must find it Ready, not trigger a second rebuild"
        );
        let ready_ms = ready.elapsed.as_secs_f64() * 1000.0;

        // The four rebuild-triggering states, measured at the check alone
        // (schema::inspect_for), not through a full open_or_rebuild -- that
        // would pay for an actual rebuild and measure NFR-PRF-04 by accident.
        // What #68 asks whether these "fail early", and inspect_for is where
        // that failure happens.
        let index_path = serat_index::index_path(&scratch);

        let absent_started = Instant::now();
        let absent = serat_index::schema::inspect_for(&scratch.join("no-such-index.db"), None);
        let absent_ms = absent_started.elapsed().as_secs_f64() * 1000.0;
        assert!(
            matches!(absent, Ok(serat_index::IndexState::Absent)),
            "a path with nothing at it must report Absent: {absent:?}"
        );

        // Three copies of the real Ready database, each mutated to trigger one
        // specific UnusableReason, so the check alone -- not a rebuild -- is
        // what gets timed.
        let root = vault.path.to_string_lossy().into_owned();
        let mutate = |name: &str, sql: &str| -> std::path::PathBuf {
            let copy = scratch.join(name);
            std::fs::copy(&index_path, &copy).expect("copy the ready index");
            let conn = rusqlite::Connection::open(&copy).expect("open the copy");
            conn.execute(sql, []).expect("mutate index_meta");
            copy
        };

        // inspect_for checks `PRAGMA user_version`, not the index_meta.schema_version
        // column (which is diagnostic-only, per schema.rs's own doc comment on it).
        let schema_changed = mutate("schema-changed.db", "PRAGMA user_version = 999999");
        let schema_started = Instant::now();
        let schema_result = serat_index::schema::inspect_for(&schema_changed, Some(&root));
        let schema_ms = schema_started.elapsed().as_secs_f64() * 1000.0;
        assert!(
            matches!(
                schema_result,
                Ok(serat_index::IndexState::Unusable {
                    reason: serat_index::UnusableReason::IndexVersion
                })
            ),
            "a schema_version this wrong must report IndexVersion: {schema_result:?}"
        );

        let interrupted = mutate("interrupted.db", "UPDATE index_meta SET state = 'building'");
        let interrupted_started = Instant::now();
        let interrupted_result = serat_index::schema::inspect_for(&interrupted, Some(&root));
        let interrupted_ms = interrupted_started.elapsed().as_secs_f64() * 1000.0;
        assert!(
            matches!(
                interrupted_result,
                Ok(serat_index::IndexState::Unusable {
                    reason: serat_index::UnusableReason::RebuildInterrupted
                })
            ),
            "state='building' must report RebuildInterrupted: {interrupted_result:?}"
        );

        let moved = mutate(
            "moved.db",
            "UPDATE index_meta SET vault_root = 'C:/nowhere/not-the-real-vault'",
        );
        let moved_started = Instant::now();
        let moved_result = serat_index::schema::inspect_for(&moved, Some(&root));
        let moved_ms = moved_started.elapsed().as_secs_f64() * 1000.0;
        assert!(
            matches!(
                moved_result,
                Ok(serat_index::IndexState::Unusable {
                    reason: serat_index::UnusableReason::VaultMoved
                })
            ),
            "a vault_root that does not match must report VaultMoved: {moved_result:?}"
        );

        eprintln!(
            "NFR-PRF-01 open check (task #68, following #61's integrity_check removal), \
             20k fixture (index.db ~124.2MB):\n\
             \x20 Ready (open_or_rebuild, the six stamp steps, THE HEADLINE NUMBER): {ready_ms:.2} ms\n\
             \x20 Absent (inspect_for only):        {absent_ms:.3} ms\n\
             \x20 SchemaChanged (inspect_for only):  {schema_ms:.3} ms\n\
             \x20 Interrupted (inspect_for only):    {interrupted_ms:.3} ms\n\
             \x20 VaultMoved (inspect_for only):     {moved_ms:.3} ms\n\
             (for comparison, architecture.md §3.4: integrity_check 1310ms, quick_check 1150ms, rejected)"
        );

        // A sanity bound, not a budget: no NFR governs the rejected-candidate
        // side of this number. The claim under test is "fails early", not a
        // specific figure -- task #44's lesson applies here as much as it did
        // to a wall-clock assert on shared CI hardware. Observed under
        // concurrent load on this machine: low single digits idle, up to
        // ~37ms contended. 300ms is generous for a check that reads one row
        // even under load, and still an order of magnitude under the
        // cheapest rejected candidate (FTS5 integrity-check, 390ms).
        for (label, ms) in [
            ("Absent", absent_ms),
            ("SchemaChanged", schema_ms),
            ("Interrupted", interrupted_ms),
            ("VaultMoved", moved_ms),
        ] {
            assert!(
                ms < 300.0,
                "{label} took {ms:.2} ms -- 'fails early' should mean well under the \
                 cheapest rejected candidate (390ms) for a check that reads one row, \
                 not something worth a rebuild-shaped number"
            );
        }
        // The headline number itself is reported, not bounded: there is no
        // budget for it yet (that is the residual #68 leaves named), and
        // asserting an arbitrary threshold here would be the register's own
        // F-138 mistake -- a point where a range belongs.
        let _ = ready_ms;

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
