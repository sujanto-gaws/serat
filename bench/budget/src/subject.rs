//! The seam — the only module in the harness that touches a Serat crate.
//!
//! Everything measured enters through one of the three functions below.
//!
//! **All three are wired and landing red is no longer their default state.**
//! `full_rebuild()` (I4, I3) calls the real `serat-index`/`serat-parse` seams
//! directly; the `REBUILD_ENTRY_POINT` constant below is kept as history of
//! what it asked for while it was still an error, not as a current status.
//! `cold_start()` and `save_to_index_via_app()` (task #94, following #111)
//! both spawn the real `serat-app` binary (S1) rather than linking it — the
//! dependency rule (architecture §2.1) runs the other way, `bench/budget` may
//! not depend on `serat-app` — and read one line of JSON it writes on exit.
//! Task #52 settled the one open question about what `cold_start` waits for;
//! see [`COLD_START_ENTRY_POINT`] below, which is current rather than
//! aspirational. `save_to_index_via_app` was the last of the three to make
//! this trade: an earlier version called `IndexHandle::apply()`/`flush()`
//! straight into `serat-index`, which skipped `AppState` entirely and so
//! could not see the one thing NFR-PRF-03's harness exists to catch — see its
//! own doc comment.
//!
//! Keeping the seam in one file is deliberate: when a function's blockers
//! land, only that function's body changes and no measurement code moves.
//! The signature in `REBUILD_ENTRY_POINT` is what `rust-core-dev` was asked
//! for, kept for the record even after landing.

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

/// The directories vaultgen assigns to its highest `hub_boost` kinds
/// (`bench/vaultgen/src/plan.rs::Kind::hub_boost`): `_types` (TypeDef, 4.0),
/// `areas` (Area, 6.0 — "the heaviest hubs in the vault"), `projects`
/// (Project, 3.5), `people` (Person, 3.0). [`select_save_sample`]'s proxy for
/// "is this a hub note" — fixed by the generator's own directory layout, not
/// derived from a per-note in-degree the manifest does not publish
/// (`link_degree` is aggregate-only: edges, max, p50/p90/p99, orphans,
/// `top_1pct_share`). Cheapest honest signal available without re-deriving
/// vaultgen's private hub-selection RNG, which is `bench/budget`'s call to
/// make (`crates/serat-app/src/cli.rs`'s own doc on `bench_save_list`: "a
/// sampling decision over the vault's own link-degree distribution —
/// `bench/budget`'s domain, not this crate's").
const HUB_DIRECTORIES: &[&str] = &["_types", "areas", "projects", "people"];

/// Choose `iterations` distinct vault-relative Markdown paths for
/// `--bench-save-list`.
///
/// Stratified rather than a plain random or first-N sample, for the reason
/// this task was given in review: *"a save that touches a hub does far more
/// link-table work than the median, so a sample that misses the hubs posts a
/// p95 the real vault does not honour"* (rust-core-dev, with the 20k vault's
/// own numbers: 25% of inbound links held by 1% of notes, max in-degree 679).
/// Up to a fifth of the sample is drawn from [`HUB_DIRECTORIES`], capped by
/// how many exist; the rest is spread evenly across every other Markdown note
/// so the sample is not exclusively cold long-tail notes either.
///
/// Deterministic over a fixed vault: both partitions are sorted before
/// sampling, so two runs against the same vault ask for the same notes in the
/// same order — same reasoning as `PRESETS`' pinned digests, a number is only
/// comparable with another number taken under the same conditions.
///
/// # Errors
/// The vault could not be opened or walked, or it does not hold `iterations`
/// distinct Markdown notes to sample from.
fn select_save_sample(vault: &Path, iterations: usize) -> Result<Vec<String>, String> {
    let opened = serat_vault::Vault::open(vault)
        .map_err(|e| format!("opening {} to select a save sample: {e}", vault.display()))?;
    let entries = opened
        .walk()
        .map_err(|e| format!("walking {} to select a save sample: {e}", vault.display()))?;

    let mut hub: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for entry in &entries {
        if !entry.path.is_markdown() {
            continue;
        }
        let path = entry.path.as_str().to_string();
        let top = path.split('/').next().unwrap_or("");
        if HUB_DIRECTORIES.contains(&top) {
            hub.push(path);
        } else {
            rest.push(path);
        }
    }
    hub.sort();
    rest.sort();

    let total = hub.len() + rest.len();
    if total < iterations {
        return Err(format!(
            "{} asks for {iterations} distinct notes but the vault only has {total} Markdown \
             notes ({} hub, {} other)",
            vault.display(),
            hub.len(),
            rest.len()
        ));
    }

    let hub_target = (iterations / 5).min(hub.len());
    let rest_target = iterations - hub_target;
    if rest.len() < rest_target {
        return Err(format!(
            "{} needs {rest_target} non-hub notes to fill out a {iterations}-note sample but \
             only has {}",
            vault.display(),
            rest.len()
        ));
    }

    let mut sample = evenly_spaced(&hub, hub_target);
    sample.extend(evenly_spaced(&rest, rest_target));
    Ok(sample)
}

/// `n` entries spread evenly across `from`, in order — not the first `n`,
/// which would sample only one corner of a sorted (and so path-clustered)
/// list.
fn evenly_spaced(from: &[String], n: usize) -> Vec<String> {
    if n == 0 || from.is_empty() {
        return Vec::new();
    }
    if n >= from.len() {
        return from.to_vec();
    }
    (0..n).map(|i| from[i * from.len() / n].clone()).collect()
}

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
    /// Whether the open adopted a changed `index_meta.vault_root` (task #120).
    ///
    /// Usable either way -- an adopt is *not* a rebuild and does not clear
    /// `was_ready` -- but it means `elapsed` also carries the identity probe
    /// and the write-back, which is a different number from the bare stamp and
    /// must not be reported as one.
    pub adopted: bool,
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
    let adopted = std::cell::Cell::new(false);
    let on_status = |status: serat_index::IndexStatus| match status {
        serat_index::IndexStatus::Opening | serat_index::IndexStatus::Ready => {}
        // Task #120: an index recorded under another root is adopted, not rebuilt,
        // so this must not read as "the index was not usable" -- that would report
        // an adopt under the same flag as a 28 s rebuild, which is the conflation
        // F-459 is about.
        serat_index::IndexStatus::RootAdopted { .. } => adopted.set(true),
        _ => was_ready.set(false),
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
        adopted: adopted.get(),
    })
}

/// Build a real, `Ready` index at `vault_root.join(".serat")` before the real
/// application is ever spawned against it -- **F-450**, found live during the
/// first real #95 run and issued its number after the fact.
///
/// NFR-PRF-03 governs a save made in an *already-open* vault -- "a note saved
/// through Serat's own write path", not a race against the vault's own first
/// index build. But `--exit-after-ready`'s "usable" mark does not wait for
/// that build to finish: `Startup::mark_index_settled`'s own doc names its
/// trigger as "the actor's first published state that is not `Opening`",
/// which a vault needing a full rebuild reaches almost immediately, long
/// before `AppState`'s `IndexHandleSlot` actually holds a handle. Against a
/// vault with no pre-existing `.serat/`, every one of a benchmark's saves can
/// therefore land inside that window and find the slot empty -- not F-382's
/// documented narrow race, but a near-certain miss for as long as a 20,000-
/// note rebuild takes. Measured directly: a first real run against the
/// canonical (cold) 20k vault reported `handle_missing: 1000` of 1000.
///
/// This calls the exact same `serat_index::open_or_rebuild` `serat-app`'s own
/// `index.rs` calls (same four arguments, same status callback shape), so the
/// `.serat/` this leaves behind is indistinguishable from one a real user's
/// first open would produce -- the next real app spawn finds it `Ready` and
/// installs a handle at task #68's few-millisecond open-check cost rather
/// than triggering a second rebuild. A no-op if `.serat/` already exists
/// (warm from an earlier run in the same guarded batch): NFR-PRF-03's own
/// premise is a warm, already-open vault, and re-warming one that already is
/// would just be a second, wasted rebuild.
///
/// # Errors
/// The vault cannot be opened, or the rebuild-or-open itself fails.
fn prewarm_index(vault: &Path) -> Result<(), String> {
    let index_dir = vault.join(".serat");
    if index_dir.exists() {
        return Ok(());
    }
    let source = serat_vault::Vault::open(vault)
        .map_err(|e| format!("opening {} to pre-warm the index: {e}", vault.display()))?;
    // **The root is `source.root()`, not `vault`** -- F-458. Kept, and the
    // reason it is kept has changed, so the tense below has to as well.
    //
    // *What it was.* `open_or_rebuild` recorded the vault's identity as
    // `vault_root.to_string_lossy()` and rebuilt from scratch on
    // `RebuildReason::VaultMoved` when a later open presented a different
    // string. `serat-app` reaches its root through `Vault::open(..).root()`,
    // which canonicalises and strips the verbatim prefix; this harness reaches
    // the same directory through `repo_root().join("bench/vaults/vault-20k")`,
    // which is textually different (mixed separators from `CARGO_MANIFEST_DIR`)
    // for the same bytes on disk. Pre-warming under the harness's spelling
    // therefore left an index the application declared moved and rebuilt --
    // measured: `Rebuilding { reason: VaultMoved }` on every spawn, and
    // `handle_missing` unchanged at 25 of 25.
    //
    // *What it is since task #120.* The product half is fixed: the identity is
    // `identity::canonical_root`, on record and on comparison, so neither
    // spelling can produce a rebuild any more. This line is therefore no longer
    // load-bearing for correctness -- it is load-bearing for the *measurement*.
    // Pre-warming under a second spelling would leave an index the next open
    // has to adopt, and an adopt costs the identity probe and a write-back
    // (23.59 ms against 6.95 ms for the bare stamp) on the very open this
    // harness is timing. Identical by construction rather than by coincidence
    // was the point either way.
    let root = source.root().to_path_buf();
    let index_dir = root.join(".serat");
    let parser = serat_parse::CommonMarkParser;
    let on_status = |_status: serat_index::IndexStatus| {};
    let (handle, _report) =
        serat_index::open_or_rebuild(&root, &index_dir, &source, &parser, &on_status)
            .map_err(|e| format!("pre-warming the index at {}: {e}", index_dir.display()))?;
    handle.close().map_err(|e| {
        format!(
            "closing the pre-warmed index at {}: {e}",
            index_dir.display()
        )
    })
}

/// Save `iterations` distinct notes through the real application, driving the
/// compiled binary via `--bench-save-list` (task #94, following #111).
///
/// **Why the compiled binary, not `IndexHandle::apply()` + `flush()` directly
/// against `serat-index`.** An earlier version of this function called
/// straight into `serat-index`, which is §3.1.1's save path in name only: it
/// never enters `AppState::write_file`, so it never touches the empty-
/// handle-slot race `AppState::save_path_outcomes` exists to count (F-382) or
/// the writer-refused backpressure ADR-0011 sanctions. A harness that
/// bypasses the slot cannot detect that the slot degraded — precisely the
/// shape F-19 already caught once, in the rebuild job's own history, where a
/// fast number was posted over a partially indexed vault because nothing
/// measured the thing that dropped work. Driving the real process is the
/// same trade [`cold_start_once`] already made for NFR-PRF-01, for the same
/// reason: this is the only seam that measures `serat-app`, which `bench/
/// budget` may not link under architecture §2.1's dependency rule.
///
/// Calls [`prewarm_index`] first — NFR-PRF-03 governs an already-open vault,
/// not a race against its first rebuild (see that function's own doc for the
/// defect this closes, found live on the first real run of this instrument).
/// Then selects the sample with [`select_save_sample`], writes it to a
/// scratch list file, spawns the binary the same way [`cold_start_once`] does
/// — `t0` stamped by the harness immediately before `spawn()`, handed in
/// through `SERAT_STARTUP_T0_UNIX_NANOS` — and parses
/// [`crate::trace::SaveBenchmarkTrace`] out of the one line the run writes to
/// `SERAT_STARTUP_TRACE`.
///
/// # Errors
/// The application binary is absent (task S1's message), the vault cannot
/// supply `iterations` distinct notes, pre-warming the index fails, the run
/// exits non-zero (the process's own contract: abort rather than report a
/// shorter series than was asked for), or the exit report cannot be parsed.
pub fn save_to_index_via_app(
    vault: &Path,
    iterations: usize,
) -> Result<crate::trace::SaveBenchmarkTrace, String> {
    let binary = app_binary()?;

    // F-401: the same hazard `cold_start`'s guard exists for, and for the same
    // reason -- both this function's own spawn and `prewarm_index` below
    // create `.serat/` inside `vault` as an ordinary side effect of indexing.
    // One guard for the whole run, taken before either touches the vault and
    // dropped after this function returns on every exit path including `?`.
    let _guard = ColdStartVaultGuard::watch(vault);
    prewarm_index(vault)?;

    let paths = select_save_sample(vault, iterations)?;

    let list_path =
        std::env::temp_dir().join(format!("serat-bench-save-list-{}.txt", std::process::id()));
    std::fs::write(&list_path, paths.join("\n"))
        .map_err(|e| format!("writing the save list to {}: {e}", list_path.display()))?;

    let trace_path = std::env::temp_dir().join(format!(
        "serat-bench-save-trace-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&trace_path);

    let t0 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("the system clock is before the epoch: {e}"))?
        .as_nanos();

    let status = Command::new(&binary)
        .arg("--vault")
        .arg(vault)
        .arg("--bench-save-list")
        .arg(&list_path)
        .env("SERAT_STARTUP_TRACE", &trace_path)
        .env("SERAT_STARTUP_T0_UNIX_NANOS", t0.to_string())
        .status()
        .map_err(|e| format!("spawning {}: {e}", binary.display()))?;
    let _ = std::fs::remove_file(&list_path);

    if !status.success() {
        let _ = std::fs::remove_file(&trace_path);
        return Err(format!(
            "the --bench-save-list run exited with {status}. A non-zero exit means the run \
             aborted before reporting anything -- run_save_benchmark's own contract is to abort \
             rather than report a shorter series than the {iterations} notes asked for."
        ));
    }

    let line = std::fs::read_to_string(&trace_path)
        .map_err(|e| format!("reading {}: {e}", trace_path.display()))?;
    let _ = std::fs::remove_file(&trace_path);
    crate::trace::SaveBenchmarkTrace::parse(line.trim())
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
///
/// **Scope: guards one batch's own side effect, not arbitrary concurrent writers.**
/// `created_by_this_batch` is computed once, from a single existence check at
/// [`ColdStartVaultGuard::watch`] time, and is never re-evaluated. It cannot distinguish "this
/// batch's own `serat.exe` created `.serat/` after `watch()`" from "an unrelated concurrent
/// process created it after `watch()`" -- both look identical and both are removed at drop. That
/// is by design for the single-process batches this crate runs (F-401's own hazard is one
/// process, not several racing), and it is not a promise this guard makes about arbitrary
/// concurrent callers.
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
        let trace = StartupTrace::parse_requiring_tree(line.trim())
            .map_err(|e| format!("run {run}: {e}"))?;
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

        // `save_to_index_via_app` is now in the same category as `cold_start`
        // below, not `full_rebuild` above: it drives a SEPARATELY BUILT BINARY
        // (task #94, following #111), so its refusal names either the absent
        // binary (S1, on a clean checkout) or the absent vault (once a binary
        // exists) -- both correct, and which one you get depends on build
        // state, not on this seam's own correctness.
        let e = save_to_index_via_app(vault, 10).expect_err("must not silently pass");
        assert!(
            e.contains("S1") || e.contains("opening") || e.contains("vault"),
            "save_to_index_via_app must refuse and name either the absent binary or the \
             absent vault, not fail opaquely: {e}"
        );

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

    // `a_guard_taken_before_a_pre_existing_dot_serat_is_created_still_protects_it` stood here and
    // is removed rather than fixed in place -- code-reviewer's SHOULD-FIX (F-107's shape). Its
    // comment claimed the guard "must still answer 'I did not create this'" for a `.serat/` a
    // *second, concurrent* caller creates between `watch()` and `drop()`. The body never set up
    // that scenario: it called `watch()` on an empty vault, asserted `created_by_this_batch`
    // (true), and dropped -- which is `a_guard_removes_dot_serat_it_created_on_drop` with the
    // internal field asserted directly instead of the filesystem effect. No mutation kills it
    // that the removes-own-on-drop and cleans-up-on-early-return tests do not already kill.
    //
    // Chose "fix the comment" over "fix the code" and then chose deletion over a corrected
    // comment, because the property the comment described is not one this guard can provide.
    // `created_by_this_batch` is computed once, at `watch()` time, from a single existence check,
    // and is never re-evaluated -- so the guard has no way to tell "this batch's own `serat.exe`
    // created `.serat/` after watch()" apart from "an unrelated concurrent process created it
    // after watch()". Both look identical and both are removed at drop. That is the guard's
    // documented contract (see `ColdStartVaultGuard`'s own doc comment: it guards a single
    // batch's own side effect, not arbitrary concurrent writers), so a test claiming it
    // "protects" the concurrent case would be asserting a property that does not hold, which is
    // worse than asserting nothing.

    // ---------------------------------------------------------------- #94

    fn write_note(dir: &Path, rel: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dirs");
        }
        std::fs::write(&path, format!("---\ntitle: {rel}\n---\nbody")).expect("write note");
    }

    /// A tiny vault with the same directory shape vaultgen uses for its
    /// highest-`hub_boost` kinds, so `select_save_sample` has something real
    /// to stratify. Not a fixture the generator produced -- this crate's own
    /// digest gate would reject a hand-built tree -- just the shape
    /// `HUB_DIRECTORIES` looks for.
    fn hub_shaped_vault(label: &str, hub_notes: usize, other_notes: usize) -> PathBuf {
        let dir = scratch_vault_dir(label);
        for i in 0..hub_notes {
            // Spread across all four hub directories, not just one, so a test
            // asserting "at least one hub directory shows up" cannot pass by
            // accident on a sample that only ever finds `_types`.
            let top = HUB_DIRECTORIES[i % HUB_DIRECTORIES.len()];
            write_note(&dir, &format!("{top}/hub-{i}.md"));
        }
        for i in 0..other_notes {
            write_note(&dir, &format!("notes/other-{i}.md"));
        }
        dir
    }

    #[test]
    fn a_prewarmed_index_survives_the_root_spelling_serat_app_will_open_it_with() {
        // F-458, whose product half is F-459 and task #120. The pre-warm
        // exists to hand `serat-app` a `Ready` index; an index the application
        // declares moved and rebuilds from scratch is worth nothing, and
        // nothing else in this crate would notice, because `open_or_rebuild`
        // succeeds either way. The identity it recorded *was*
        // `vault_root.to_string_lossy()` -- a string, not a resolved directory
        // -- so two spellings of the same directory were two vaults to it.
        // `serat-app` reaches its root through `Vault::open(..).root()`; this
        // harness reaches the same bytes through
        // `repo_root().join("bench/vaults/vault-20k")`. Measured before the
        // fix: `Rebuilding { reason: VaultMoved }` on every spawn and
        // `handle_missing` unmoved at 25 of 25.
        //
        // **Task #120 fixed the product half, and this test is kept rather
        // than retired** -- it is now the regression test for the fix instead
        // of the demonstration of the bug, and it is the only one that runs
        // through `serat_vault::Vault` and the real `CommonMarkParser` rather
        // than through `serat-index`'s own stubs. What it asserts is unchanged
        // and is stricter than it looks: `report.is_none()` fails on a rebuild,
        // and under #120 a *foreign* index still rebuilds, so this would still
        // go red if canonicalisation regressed far enough to make the probe
        // find nothing.
        let vault = hub_shaped_vault("prewarm-root-spelling", 4, 8);
        // Textually different, byte-identical directory -- the same class of
        // difference as the mixed separators the real path carries, without
        // depending on how any particular platform spells a temp directory.
        let second_spelling = vault.join(".");
        assert_ne!(
            second_spelling.display().to_string(),
            vault.display().to_string(),
            "precondition: the two spellings must actually differ"
        );
        prewarm_index(&second_spelling).expect("pre-warming through the second spelling");

        let source = serat_vault::Vault::open(&vault).expect("reopening the vault");
        let root = source.root().to_path_buf();
        let parser = serat_parse::CommonMarkParser;
        let (handle, report) = serat_index::open_or_rebuild(
            &root,
            &root.join(".serat"),
            &source,
            &parser,
            &|_status: serat_index::IndexStatus| {},
        )
        .expect("reopening the pre-warmed index");
        handle.close().expect("closing the reopened index");
        let _ = std::fs::remove_dir_all(&vault);

        assert!(
            report.is_none(),
            "the pre-warmed index was rebuilt rather than opened: {report:?}. A pre-warm the              application throws away is a pre-warm that measured nothing."
        );
    }

    #[test]
    fn evenly_spaced_is_empty_for_zero_and_the_whole_slice_when_n_meets_or_exceeds_it() {
        let from: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        assert!(evenly_spaced(&from, 0).is_empty());
        assert!(evenly_spaced(&[], 5).is_empty());
        assert_eq!(evenly_spaced(&from, 10), from);
        assert_eq!(
            evenly_spaced(&from, 20),
            from,
            "n > len must not panic or truncate wrongly"
        );
    }

    #[test]
    fn evenly_spaced_returns_n_distinct_entries_spread_across_the_slice() {
        let from: Vec<String> = (0..100).map(|i| format!("{i:03}")).collect();
        let picked = evenly_spaced(&from, 10);
        assert_eq!(picked.len(), 10);
        let mut sorted = picked.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 10, "every pick must be distinct: {picked:?}");
        // Spread, not clustered at one end: the first and last picks should
        // sit near the two ends of a 100-entry slice sampled ten ways.
        assert!(picked[0].parse::<usize>().unwrap() < 20, "{picked:?}");
        assert!(picked[9].parse::<usize>().unwrap() >= 80, "{picked:?}");
    }

    #[test]
    fn select_save_sample_draws_up_to_a_fifth_from_hub_directories() {
        // 20 hub notes (5 per HUB_DIRECTORIES entry) and 200 plain ones -- the
        // 20k vault's own proportions are far more skewed than this, but the
        // point under test is the 1-in-5 cap and the stratification, not a
        // realistic ratio.
        let vault = hub_shaped_vault("select-sample-basic", 20, 200);
        let sample = select_save_sample(&vault, 50).expect("50 of 220 notes");

        assert_eq!(sample.len(), 50, "must return exactly what was asked for");
        let mut sorted = sample.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 50, "every entry must be distinct: {sample:?}");

        let hub_count = sample
            .iter()
            .filter(|p| HUB_DIRECTORIES.contains(&p.split('/').next().unwrap_or("")))
            .count();
        // hub_target = (50 / 5).min(20) = 10 exactly, by construction.
        assert_eq!(
            hub_count, 10,
            "expected the 1-in-5 cap (min(50/5, 20 available)) to draw exactly 10 hub notes: \
             {sample:?}"
        );
        std::fs::remove_dir_all(&vault).ok();
    }

    #[test]
    fn select_save_sample_is_capped_by_however_many_hub_notes_actually_exist() {
        // Only 4 hub notes exist (one per directory) against a target of
        // 100/5 = 20 -- the cap must yield to what is actually there rather
        // than erroring or padding with duplicates.
        let vault = hub_shaped_vault("select-sample-scarce-hub", 4, 200);
        let sample = select_save_sample(&vault, 100).expect("100 of 204 notes");
        assert_eq!(sample.len(), 100);
        let hub_count = sample
            .iter()
            .filter(|p| HUB_DIRECTORIES.contains(&p.split('/').next().unwrap_or("")))
            .count();
        assert_eq!(
            hub_count, 4,
            "capped by availability, not by the 1-in-5 target: {sample:?}"
        );
        std::fs::remove_dir_all(&vault).ok();
    }

    #[test]
    fn select_save_sample_refuses_a_vault_smaller_than_the_request() {
        let vault = hub_shaped_vault("select-sample-too-small", 2, 3);
        let err =
            select_save_sample(&vault, 1_000).expect_err("5 notes cannot fill a 1,000-note sample");
        assert!(err.contains('5'), "{err}");
        assert!(err.contains("1000") || err.contains("1,000"), "{err}");
        std::fs::remove_dir_all(&vault).ok();
    }

    #[test]
    fn select_save_sample_ignores_non_markdown_entries() {
        let vault = hub_shaped_vault("select-sample-assets", 5, 8);
        write_note(&vault, "assets/report.pdf");
        let sample = select_save_sample(&vault, 10).expect("10 markdown notes exist");
        assert!(
            sample.iter().all(|p| p.ends_with(".md")),
            "an asset must never be handed to --bench-save-list: {sample:?}"
        );
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
        assert!(
            !ready.adopted,
            "a second open of the vault it was built from must have nothing to adopt; \
             if it does, the record side is stamping a spelling rather than the \
             resolved directory (task #120, F-458)"
        );
        let ready_ms = ready.elapsed.as_secs_f64() * 1000.0;

        let index_path = serat_index::index_path(&scratch);

        // Task #120's new read path on the open critical path, measured because it
        // is one: an index whose recorded root differs is no longer rebuilt, it is
        // probed against the tree (32 `notes.path` rows stat-ed) and adopted. That
        // is the branch that replaced a 28.6 s rebuild, so what it costs instead
        // belongs on the record beside the stamp it follows. Run on the real 20k
        // index, in place, so the probe pays real filesystem calls.
        {
            let conn = rusqlite::Connection::open(&index_path).expect("open the ready index");
            conn.execute(
                "UPDATE index_meta SET vault_root = 'C:/nowhere/not-the-real-vault'",
                [],
            )
            .expect("re-stamp the root");
        }
        let adopt = measure_open_check(&vault.path, &scratch).expect("open with a changed root");
        let adopt_ms = adopt.elapsed.as_secs_f64() * 1000.0;
        assert!(
            adopt.was_ready,
            "a changed vault_root must not trigger a rebuild any more (task #120)"
        );
        assert!(
            adopt.adopted,
            "a changed vault_root must be adopted and reported, not silently ignored"
        );

        // The four rebuild-triggering states, measured at the check alone
        // (schema::inspect_for), not through a full open_or_rebuild -- that
        // would pay for an actual rebuild and measure NFR-PRF-04 by accident.
        // What #68 asks whether these "fail early", and inspect_for is where
        // that failure happens.

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
        //
        // `canonical_root`, not `to_string_lossy` (task #120, F-458): the index
        // records the resolved directory, so an expectation built from the raw
        // path would report RootChanged on every one of these and measure the
        // wrong branch.
        let root = serat_index::canonical_root(&vault.path);
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

        // Not a rebuild trigger since task #120, so no longer one of the four
        // "fails early" states -- but still a branch of the stamp, and the one
        // that grew a filesystem probe, so it is still measured here. What it
        // reports changed with it: RootChanged, a value, not Unusable.
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
                Ok(serat_index::IndexState::RootChanged { .. })
            ),
            "a vault_root that does not match must report RootChanged, not a fault \
             (task #120): {moved_result:?}"
        );

        eprintln!(
            "NFR-PRF-01 open check (task #68, following #61's integrity_check removal), \
             20k fixture (index.db ~124.2MB):\n\
             \x20 Ready (open_or_rebuild, the six stamp steps, THE HEADLINE NUMBER): {ready_ms:.2} ms\n\
             \x20 Absent (inspect_for only):        {absent_ms:.3} ms\n\
             \x20 SchemaChanged (inspect_for only):  {schema_ms:.3} ms\n\
             \x20 Interrupted (inspect_for only):    {interrupted_ms:.3} ms\n\
             \x20 RootChanged (inspect_for only, not a rebuild trigger since #120): {moved_ms:.3} ms\n\
             \x20 RootAdopted (open_or_rebuild: stamp + identity probe + write-back, task #120): {adopt_ms:.2} ms\n\
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
            ("RootChanged", moved_ms),
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
