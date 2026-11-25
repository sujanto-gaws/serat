//! FR-VLT-04 · an external change is visible in the index within 500 ms · reported,
//! with --assert as the gate (p95 over the single-note changes, PRD v1.9 FR-VLT-04; --assert refuses any vault but 20k).
//!
//! Serves **FR-VLT-04**, **NFR-REL-01**, **US-35**; architecture §3.1.2 (the watcher
//! path), not §3.1.1 (the save path, NFR-PRF-03's 50 ms, which has no watcher and no
//! window). The interval is: another process finishes writing a note, to the moment a
//! reader of the index sees the new content.
//!
//! The pipeline measured is the real one, with nothing stubbed: the `notify` watcher
//! on a copy of the benchmark vault, its 150 ms coalescing window, the work queue, a
//! consumer that drains the queue and calls `Reconciler::index_paths` (hash gate,
//! parse with the real `CommonMarkParser`, bulk-side submit, flush), over an index
//! built by a full rebuild of that copy.
//!
//! Two shapes, because the budget names one and the failure mode is the other:
//!
//! * **single**: one note edited, written the way most editors do (temp file, then
//!   rename over the target). Each iteration edits a different note.
//! * **batch**: 100 notes written back to back (a sync tool catching up, a `git
//!   checkout` of a hundred files); the figure is until the *last* is visible.
//!
//! Each is split into the part the design sets and the part code spends: the
//! **watcher delay** (write to the consumer taking the path off the queue, which holds
//! the 150 ms window) and the **processing** (taking it to committed and visible).
//! The 500 ms is judged on the sum; the split says which half moved when it moves.
//! **Only the single-note shape is gated** (FR-VLT-04 says "a file is modified"); the
//! 100-note batch is a stress shape, printed as "reported, not gated", and its
//! cost is the index writer's per-note upsert, not this path.
//!
//! **A run is rejected, not recorded, unless:** every iteration's edits were all
//! reported upserted (within 10 s for a single note, within 60 s for the 100-note batch:
//! task #167, the batch took 7.7 s p95 / 8.1 s max on the 2-core runner of 2026-11-09, so
//! 10 s left 20 % of margin on a shape that is not gated), and every edited note's `notes.hash` in the index
//! equals the blake3 of the bytes written. A fast number over an index that did not
//! change would be the flattering kind (F-19).
//!
//! The vault is read from the pinned preset and **copied** to a temporary directory
//! (and indexed there): the pinned vault is never written to, so its digest check
//! stays valid for every other job. The copy and the rebuild are reported, not timed
//! into anything.
//!
//! ```text
//! cargo run --release -p budget --bin p1-external-change
//! cargo run --release -p budget --bin p1-external-change -- --vault 20k --single 100 --batches 20 --assert
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use budget::{Samples, ensure_vault};
use rusqlite::{Connection, OpenFlags};
use serat_core::VaultPath;
use serat_index::external::{Outcome, Reconciler};
use serat_index::{IndexHandle, RunKind, WalHealth, rebuild};
use serat_parse::CommonMarkParser;
use serat_vault::{Vault, VaultEvent, Watcher, WatcherConfig, WorkQueue, hash_bytes};

/// FR-VLT-04's acceptance criterion.
const BUDGET_MS: f64 = 500.0;
/// How long one iteration may take before the run is rejected.
const ITERATION_TIMEOUT: Duration = Duration::from_secs(10);
/// The same for the 100-note batch shape (task #167). Not gated, so it may not reject a run
/// by being slow; it still rejects a run that never completes.
const BATCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Quiet time between iterations, so stragglers (a second event for a path already
/// indexed) belong to the iteration that caused them and not to the next one.
const SETTLE: Duration = Duration::from_millis(300);
// Diagnostic (task #165): `--settle-ms` changes the quiet time between iterations.
// The CI job never passes it; the default stays 300 ms. A settle longer than the
// checkpointer's idle gate (250 ms) plus a checkpoint makes an overlap rarer (each
// checkpoint usually finishes before the next write) but does not remove it: the
// checkpointer also runs on the 2 s idle spacing and the 4,000-frame bound.

/// The state of a stalled iteration, as one line, so a failure to measure says which stage
/// stopped (task #167, FR-VLT-04): the watcher never queued the paths (`queue depth 0`,
/// `rounds 0`), the watcher lost events (`rescan raised`), or the consumer is slow
/// (`queue depth` falls, `rounds` grow, the last commit is far from the write).
fn describe_stall(
    upserted: usize,
    count: usize,
    limit: Duration,
    rounds: &[(f64, f64, usize)],
    rescan_raised: bool,
    queue_depth: usize,
) -> String {
    let shown: Vec<String> = rounds
        .iter()
        .take(8)
        .map(|(taken, committed, n)| {
            format!("{n} upserted taken {taken:.0} ms committed {committed:.0} ms")
        })
        .collect();
    format!(
        "only {upserted} of {count} edits were indexed in {} s; consumer rounds {} [{}]; \
         watcher rescan {}; queue depth {queue_depth}",
        limit.as_secs(),
        rounds.len(),
        shown.join("; "),
        if rescan_raised {
            "RAISED since the watcher started (events may have been lost)"
        } else {
            "not raised"
        },
    )
}

/// What the consumer did with one drain of the queue.
struct Round {
    first_taken: Instant,
    committed: Instant,
    outcome: Outcome,
}

fn copy_tree(from: &Path, to: &Path) -> Result<usize, String> {
    let mut files = 0;
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, target)) = stack.pop() {
        std::fs::create_dir_all(&target).map_err(|e| format!("mkdir {}: {e}", target.display()))?;
        for entry in std::fs::read_dir(&source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                if name != ".serat" {
                    stack.push((entry.path(), target.join(&name)));
                }
            } else if kind.is_file() {
                std::fs::copy(entry.path(), target.join(&name)).map_err(|e| e.to_string())?;
                files += 1;
            }
        }
    }
    Ok(files)
}

fn line(label: &str, s: &Samples) {
    println!(
        "{label:<34} median {:>7.1} ms   p95 {:>7.1} ms   max {:>7.1} ms   (n={})",
        s.median(),
        s.percentile(0.95),
        s.max(),
        s.len()
    );
}

/// An editor's save: write a sibling temp file, then rename it over the target.
fn save_like_an_editor(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let target = root.join(relative);
    let temp = target.with_extension("md.tmp");
    std::fs::write(&temp, bytes).map_err(|e| format!("write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, &target).map_err(|e| format!("rename {}: {e}", target.display()))
}

struct Run<'a> {
    root: &'a Path,
    index: &'a Path,
    rounds: &'a mpsc::Receiver<Round>,
    candidates: Vec<String>,
    next: usize,
    sequence: usize,
    settle: Duration,
    health: Arc<WalHealth>,
    queue: &'a WorkQueue,
    watcher: &'a Watcher,
}

struct Measured {
    total_ms: f64,
    watcher_ms: f64,
    processing_ms: f64,
    /// A WAL checkpoint was running at the write, at the commit, or started or ended
    /// in between (task #165: is the processing tail the checkpointer?).
    checkpoint_overlap: bool,
    /// Which kind of run overlapped (task #199): the idle gate or the size bound. `None`
    /// when none did, or when the one that did ended before its kind was read. Reported,
    /// never part of a verdict (ADR-0019).
    checkpoint_kind: Option<RunKind>,
    /// The FTS5 segment count fell between the write and the commit: a merge ran in
    /// the writer's transaction (the criterion of `p1-save-tail`).
    fts_merge: bool,
    /// Read-only approximation of `Reconciler::reader()` plus a first row lookup (a new
    /// connection, the connection PRAGMAs, `query_only`, one `notes` row), measured
    /// AFTER the commit, on a row already written, so it warms nothing the next write
    /// touches and is not part of the interval. The product's `schema::open` is not
    /// public, hence an approximation.
    reader_open_ms: f64,
}

impl Run<'_> {
    /// The FTS5 segment count, on a fresh read-only connection, read just before the
    /// write. It looks up no `notes` row, so it does not warm the row the write targets.
    fn segments_now(&self) -> Result<u32, String> {
        let conn = Connection::open_with_flags(
            serat_index::index_path(self.index),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        Ok(fts_levels(&conn)?.iter().sum())
    }

    /// Edit the next `count` notes, wait until all are indexed, and verify the index.
    fn iteration(&mut self, count: usize) -> Result<Measured, String> {
        self.sequence += 1;
        let mut written = Vec::with_capacity(count);
        let segments_before = self.segments_now()?;
        let stats_before = self.health.stats();
        let runs_before = stats_before.passive_runs + stats_before.restart_runs;
        let running_at_write = self.health.checkpoint_in_progress();
        let kind_at_write = self.health.checkpoint_kind_running();
        let t0 = Instant::now();
        for _ in 0..count {
            let relative = self
                .candidates
                .get(self.next)
                .ok_or("ran out of candidate notes; use fewer iterations")?
                .clone();
            self.next += 1;
            let mut bytes = std::fs::read(self.root.join(&relative)).map_err(|e| e.to_string())?;
            bytes.extend_from_slice(
                format!("\nEdited from outside, edit {}.\n", self.sequence).as_bytes(),
            );
            save_like_an_editor(self.root, &relative, &bytes)?;
            written.push((relative, bytes));
        }

        let limit = if count == 1 {
            ITERATION_TIMEOUT
        } else {
            BATCH_TIMEOUT
        };
        let deadline = t0 + limit;
        let (mut upserted, mut first_taken, mut last_commit) = (0, None, t0);
        let mut seen: Vec<(f64, f64, usize)> = Vec::new();
        let mut rescan = false;
        while upserted < count {
            let wait = deadline.saturating_duration_since(Instant::now());
            let Ok(round) = self.rounds.recv_timeout(wait) else {
                // `take_events` was never drained before, so this is "since the watcher
                // started" (the settle and the single series included); the queue's
                // latch is the live view and is OR-ed in.
                rescan |= self
                    .watcher
                    .take_events()
                    .contains(&VaultEvent::RescanRequired)
                    || self.queue.rescan_needed().is_some();
                return Err(describe_stall(
                    upserted,
                    count,
                    limit,
                    &seen,
                    rescan,
                    self.queue.depth(),
                ));
            };
            seen.push((
                ms(round.first_taken.duration_since(t0)),
                ms(round.committed.duration_since(t0)),
                round.outcome.upserted,
            ));
            upserted += round.outcome.upserted;
            first_taken.get_or_insert(round.first_taken);
            last_commit = round.committed;
        }
        let first_taken = first_taken.unwrap_or(last_commit);
        // One snapshot of each, so the flag and the kind cannot disagree.
        let (kind_now, stats_now) = (self.health.checkpoint_kind_running(), self.health.stats());
        let checkpoint_overlap = running_at_write
            || kind_now.is_some()
            || stats_now.passive_runs + stats_now.restart_runs != runs_before;
        let checkpoint_kind =
            serat_index::overlap_kind(kind_at_write, kind_now, &stats_before, &stats_now);

        let conn = Connection::open_with_flags(
            serat_index::index_path(self.index),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        for (relative, bytes) in &written {
            let stored: String = conn
                .query_row("SELECT hash FROM notes WHERE path = ?1", [relative], |r| {
                    r.get(0)
                })
                .map_err(|e| format!("{relative} is not in the index: {e}"))?;
            if stored != hash_bytes(bytes).to_hex() {
                return Err(format!(
                    "{relative}: the index does not hold the bytes that were written"
                ));
            }
        }
        let reader_open_ms = {
            let t = Instant::now();
            let probe = Connection::open_with_flags(
                serat_index::index_path(self.index),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| e.to_string())?;
            probe
                .execute_batch(
                    "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; \
                     PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; \
                     PRAGMA query_only = 1;",
                )
                .map_err(|e| e.to_string())?;
            if let Some((relative, _)) = written.first() {
                let _: Option<String> = probe
                    .query_row("SELECT hash FROM notes WHERE path = ?1", [relative], |r| {
                        r.get(0)
                    })
                    .ok();
            }
            ms(t.elapsed())
        };
        let fts_merge = fts_levels(&conn)?.iter().sum::<u32>() < segments_before;
        drop(conn);

        std::thread::sleep(self.settle);
        while self.rounds.try_recv().is_ok() {}

        Ok(Measured {
            total_ms: ms(last_commit.duration_since(t0)),
            watcher_ms: ms(first_taken.duration_since(t0)),
            processing_ms: ms(last_commit.duration_since(first_taken)),
            checkpoint_overlap,
            checkpoint_kind,
            fts_merge,
            reader_open_ms,
        })
    }
}

fn varint(b: &[u8], i: &mut usize) -> u64 {
    let mut v = 0u64;
    for n in 0..9 {
        let byte = b[*i];
        *i += 1;
        if n == 8 {
            return (v << 8) | u64::from(byte);
        }
        v = (v << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            break;
        }
    }
    v
}

/// The shape of `blocks_fts`'s structure record: segments per level.
fn fts_levels(conn: &Connection) -> Result<Vec<u32>, String> {
    let blob: Vec<u8> = conn
        .query_row("SELECT block FROM blocks_fts_data WHERE id = 10", [], |r| {
            r.get(0)
        })
        .map_err(|e| format!("blocks_fts_data structure record: {e}"))?;
    // fts5StructureDecode: 4-byte cookie, optional 4-byte V2 marker, varints nLevel,
    // nSegment, nWriteCounter, then per level nMerge, nSeg and nSeg segment records.
    let mut i = 4;
    let v2 = blob[i..i + 4] == [0xFF, 0, 0, 1];
    if v2 {
        i += 4;
    }
    let n_level = varint(&blob, &mut i);
    let _n_segment = varint(&blob, &mut i);
    let _counter = varint(&blob, &mut i);
    let mut levels = Vec::new();
    for _ in 0..n_level {
        let _n_merge = varint(&blob, &mut i);
        let n_seg = varint(&blob, &mut i);
        for _ in 0..n_seg {
            for _ in 0..3 {
                varint(&blob, &mut i);
            }
            if v2 {
                for _ in 0..5 {
                    varint(&blob, &mut i);
                }
            }
        }
        levels.push(u32::try_from(n_seg).unwrap_or(u32::MAX));
    }
    Ok(levels)
}

/// Cumulative CPU jiffies from /proc/stat: (busy, total), and this process's own.
fn cpu_jiffies() -> Option<(u64, u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?;
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 5 {
        return None;
    }
    // user nice system idle iowait irq softirq steal
    let idle = v[3] + v[4];
    let total: u64 = v.iter().take(8).sum();
    let me = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after = me.rsplit_once(')')?.1;
    let f: Vec<&str> = after.split_whitespace().collect();
    // After ")": state is field 3, so utime (14) and stime (15) are at 11 and 12.
    let own = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some((total - idle, total, own))
}

/// Foreign CPU between two `cpu_jiffies` samples, in cores' worth of the whole host
/// (everything busy but this process). Linux only; `None` elsewhere.
fn foreign_cores(before: Option<(u64, u64, u64)>, after: Option<(u64, u64, u64)>) -> Option<f64> {
    let ((b0, t0, o0), (b1, t1, o1)) = (before?, after?);
    let cpus = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .count();
    let total = t1.checked_sub(t0).filter(|t| *t > 0)? as f64;
    let foreign = (b1 - b0).saturating_sub(o1 - o0) as f64;
    Some(foreign / total * cpus as f64)
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// The p95 of processing time split by checkpoint overlap (task #183), so "with-overlap
/// p95 within 2x of without" is a printed number. A class with no samples has no p95.
#[derive(Debug, PartialEq)]
struct OverlapSplit {
    with_n: usize,
    without_n: usize,
    with_p95: Option<f64>,
    without_p95: Option<f64>,
}

impl OverlapSplit {
    /// `samples`: (processing ms, a checkpoint overlapped it).
    fn of(samples: &[(f64, bool)]) -> Self {
        let class = |overlap: bool| {
            let mut s = Samples::new();
            for (ms, o) in samples {
                if *o == overlap {
                    s.push(Duration::from_secs_f64(ms / 1000.0));
                }
            }
            (s.len(), (!s.is_empty()).then(|| s.percentile(0.95)))
        };
        let (with_n, with_p95) = class(true);
        let (without_n, without_p95) = class(false);
        Self {
            with_n,
            without_n,
            with_p95,
            without_p95,
        }
    }

    /// with / without, when both exist and the denominator is above zero.
    fn ratio(&self) -> Option<f64> {
        match (self.with_p95, self.without_p95) {
            (Some(w), Some(wo)) if wo > 0.0 => Some(w / wo),
            _ => None,
        }
    }

    fn describe(&self) -> String {
        let p = |v: Option<f64>| v.map_or("n/a".to_owned(), |v| format!("{v:.1} ms"));
        format!(
            "  checkpoint overlap p95 processing: with {} (n={}), without {} (n={}), ratio {}",
            p(self.with_p95),
            self.with_n,
            p(self.without_p95),
            self.without_n,
            self.ratio()
                .map_or("n/a".to_owned(), |r| format!("{r:.2}x")),
        )
    }
}

/// How many overlapped iterations had an idle run, a size-bound run, or neither seen
/// (task #199: an idle run is the defect the frame floor removes; a size-bound one goes
/// to the architect). `samples`: (overlapped, kind).
fn describe_kinds(samples: &[(bool, Option<RunKind>)]) -> String {
    let count = |want: Option<RunKind>| {
        samples
            .iter()
            .filter(|(overlapped, kind)| *overlapped && *kind == want)
            .count()
    };
    format!(
        "  checkpoint overlap by kind: idle {}, size-bound {}, kind not seen {} (checkpoint=idle|size-bound|no)",
        count(Some(RunKind::Idle)),
        count(Some(RunKind::SizeBound)),
        count(None),
    )
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn run(args: &[String]) -> Result<bool, String> {
    let (mut vault_name, mut singles, mut batches, mut batch_size, mut assert) =
        ("20k".to_owned(), 100usize, 20usize, 100usize, false);
    let mut settle = SETTLE;
    let mut quiesce = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut number = |what: &str| -> Result<usize, String> {
            it.next()
                .ok_or(format!("{what} needs a number"))?
                .parse()
                .map_err(|e| format!("{what}: {e}"))
        };
        match arg.as_str() {
            "--vault" => vault_name = it.next().ok_or("--vault needs a name")?.clone(),
            "--single" => singles = number("--single")?,
            "--batches" => batches = number("--batches")?,
            "--batch-size" => batch_size = number("--batch-size")?,
            // Diagnostic negative control (task #165): wait until the checkpointer has
            // been silent for 3 s (longer than its 2 s idle spacing) before the series,
            // so a short series with a small `--settle-ms` has no checkpoint to overlap.
            "--quiesce-checkpointer" => quiesce = true,
            "--settle-ms" => {
                let millis = number("--settle-ms")?;
                if millis == 0 {
                    return Err("--settle-ms must be above 0".to_owned());
                }
                settle = Duration::from_millis(millis as u64);
            }
            "--assert" => assert = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    budget::gate::require_only_20k(assert, &vault_name)?;
    let pinned = ensure_vault(&vault_name)?;
    let work: PathBuf =
        std::env::temp_dir().join(format!("serat-p1-external-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let root = work.join("vault");
    let index_dir = work.join("index");

    let started = Instant::now();
    let files = copy_tree(&pinned.path, &root)?;
    eprintln!(
        "copied {files} files from the pinned {vault_name} vault in {:.1} s",
        started.elapsed().as_secs_f64()
    );

    let vault = Arc::new(Vault::open(&root).map_err(|e| format!("open vault: {e}"))?);
    let parser = Arc::new(CommonMarkParser::new());
    let started = Instant::now();
    let report = rebuild(&*vault, &*parser, vault.root(), &index_dir)
        .map_err(|e| format!("rebuild: {e}"))?;
    eprintln!(
        "rebuilt the index of the copy: {} notes in {:.1} s",
        report.notes,
        started.elapsed().as_secs_f64()
    );

    let handle = IndexHandle::open(&index_dir).map_err(|e| e.to_string())?;
    let reconciler = Reconciler::new(&handle);

    // Candidate notes: markdown notes spread across the vault, never reused.
    let candidates: Vec<String> = {
        let conn = Connection::open_with_flags(
            serat_index::index_path(&index_dir),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        let mut statement = conn
            .prepare("SELECT path FROM notes WHERE kind = 'note' ORDER BY path")
            .map_err(|e| e.to_string())?;
        let all: Vec<String> = statement
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let needed = singles + batches * batch_size;
        if all.len() < needed {
            return Err(format!("{} notes cannot supply {needed} edits", all.len()));
        }
        let step = all.len() / needed;
        (0..needed).map(|i| all[i * step].clone()).collect()
    };

    let queue = Arc::new(WorkQueue::new());
    let armed = Instant::now();
    let watcher = Watcher::start(&vault, Arc::clone(&queue), WatcherConfig::default())
        .map_err(|e| format!("watcher: {e}"))?;
    let (tx, rx) = mpsc::channel::<Round>();
    let stop = Arc::new(AtomicBool::new(false));
    let failure = Arc::new(Mutex::new(None::<String>));
    let consumer = {
        let (vault, parser, queue) = (Arc::clone(&vault), Arc::clone(&parser), Arc::clone(&queue));
        let (stop, failure) = (Arc::clone(&stop), Arc::clone(&failure));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some(first) = queue.pop_timeout(Duration::from_millis(10)) else {
                    continue;
                };
                let first_taken = Instant::now();
                let mut paths: Vec<VaultPath> = vec![first];
                paths.extend(queue.drain());
                let echo = |path: &VaultPath, hash| vault.echo().take_echo(path, hash);
                match reconciler.index_paths(&*vault, &*parser, &echo, &paths) {
                    Ok(outcome) => {
                        let _ = tx.send(Round {
                            first_taken,
                            committed: Instant::now(),
                            outcome,
                        });
                    }
                    Err(e) => {
                        *failure.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                        return;
                    }
                }
            }
        })
    };

    // Let the watcher settle after registration and the copy's own events.
    std::thread::sleep(Duration::from_secs(1));
    while rx.try_recv().is_ok() {}

    if quiesce {
        let mut quiet_since = Instant::now();
        let mut seen = handle.wal_health().stats().passive_runs;
        while quiet_since.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
            let now = handle.wal_health().stats().passive_runs;
            if now != seen || handle.wal_health().checkpoint_in_progress() {
                seen = now;
                quiet_since = Instant::now();
            }
        }
        while rx.try_recv().is_ok() {}
    }

    let mut run = Run {
        root: &root,
        index: &index_dir,
        rounds: &rx,
        candidates,
        next: 0,
        sequence: 0,
        settle,
        health: handle.wal_health(),
        queue: &queue,
        watcher: &watcher,
    };
    eprintln!(
        "watcher armed {:.1} s before the first edit; backend {}",
        armed.elapsed().as_secs_f64(),
        if cfg!(windows) {
            "ReadDirectoryChangesW"
        } else if cfg!(target_os = "macos") {
            "FSEvents"
        } else {
            "inotify"
        },
    );

    let result = (|| -> Result<(Vec<Measured>, Vec<Measured>), String> {
        let mut single = Vec::new();
        let cpu_before = cpu_jiffies();
        for _ in 0..singles {
            single.push(run.iteration(1)?);
        }
        // Printed before the batch shape runs, so a batch that cannot be measured does
        // not take the gated series' figure with it (task #167).
        let mut early = Samples::new();
        for m in &single {
            early.push(Duration::from_secs_f64(m.total_ms / 1000.0));
        }
        eprintln!(
            "single series done: p95 {:.1} ms over {} edits (full report follows the batch shape)",
            early.percentile(0.95),
            early.len()
        );
        if let Some(cores) = foreign_cores(cpu_before, cpu_jiffies()) {
            println!(
                "foreign CPU during the single series: {cores:.2} cores of the host's \
                 (everything busy except this process; the machine is quiet at about 0)"
            );
        }
        let mut batch = Vec::new();
        for _ in 0..batches {
            batch.push(run.iteration(batch_size)?);
        }
        Ok((single, batch))
    })();

    stop.store(true, Ordering::Relaxed);
    let _ = consumer.join();
    watcher.stop();
    let handle_stats = handle.wal_health().stats();
    let _ = handle.close();
    let _ = std::fs::remove_dir_all(&work);
    if let Some(message) = failure.lock().unwrap_or_else(|p| p.into_inner()).take() {
        return Err(format!("the consumer failed: {message}"));
    }
    let (single, batch) = result.map_err(|e| format!("REJECTED: {e}"))?;

    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    println!(
        "FR-VLT-04 external change, {vault_name} vault ({} notes), {cores} cores, \
         window 150 ms, budget {BUDGET_MS} ms",
        pinned.notes
    );
    let environment = budget::env::Environment::detect();
    let eligible = environment.gate_eligible();
    let report_only = budget::gate::report_only(assert, eligible);
    let mut rows = Vec::new();
    for (label, runs, size) in [("single", &single, 1), ("batch", &batch, batch_size)] {
        let series = |f: fn(&Measured) -> f64| {
            let mut samples = Samples::new();
            for measured in runs.iter() {
                samples.push(Duration::from_secs_f64(f(measured) / 1000.0));
            }
            samples
        };
        let total = series(|m| m.total_ms);
        println!("--- {label} ({size} note(s) per iteration)");
        line("write to visible", &total);
        line("  watcher delay (window)", &series(|m| m.watcher_ms));
        line("  processing to committed", &series(|m| m.processing_ms));
        let within = total.percentile(0.95) <= BUDGET_MS;
        println!(
            "{label}: p95 {:.1} ms, max {:.1} ms, against {BUDGET_MS} ms: {}{}",
            total.percentile(0.95),
            total.max(),
            if within { "within" } else { "OVER" },
            if label == "single" && report_only {
                " (report-only here: not gate eligible; max reported, not gated)"
            } else if label == "single" {
                " (gated on p95, PRD v1.9 FR-VLT-04; max reported, not gated)"
            } else {
                " (reported, not gated)"
            }
        );
        // Report-only: split the iterations by whether a checkpoint overlapped them.
        let slow = runs.iter().filter(|m| m.processing_ms > 100.0).count();
        let slow_overlapped = runs
            .iter()
            .filter(|m| m.processing_ms > 100.0 && m.checkpoint_overlap)
            .count();
        let overlapped: Vec<&Measured> = runs.iter().filter(|m| m.checkpoint_overlap).collect();
        println!(
            "  checkpoint overlap: {} of {} iterations overlapped one; processing over 100 ms: \
             {slow}, of which with a checkpoint overlapping: {slow_overlapped}; mean processing \
             with / without overlap: {:.1} / {:.1} ms",
            overlapped.len(),
            runs.len(),
            mean(overlapped.iter().map(|m| m.processing_ms)),
            mean(
                runs.iter()
                    .filter(|m| !m.checkpoint_overlap)
                    .map(|m| m.processing_ms)
            ),
        );
        let pairs: Vec<(f64, bool)> = runs
            .iter()
            .map(|m| (m.processing_ms, m.checkpoint_overlap))
            .collect();
        println!("{}", OverlapSplit::of(&pairs).describe());
        let kinds: Vec<(bool, Option<RunKind>)> = runs
            .iter()
            .map(|m| (m.checkpoint_overlap, m.checkpoint_kind))
            .collect();
        println!("{}", describe_kinds(&kinds));
        let merged: Vec<&Measured> = runs.iter().filter(|m| m.fts_merge).collect();
        let slow_merged = runs
            .iter()
            .filter(|m| m.processing_ms > 100.0 && m.fts_merge)
            .count();
        println!(
            "  FTS5 merge: {} of {} iterations saw the segment count fall; processing over 100 ms: \
             {slow}, of which with a merge: {slow_merged}; mean processing with / without a \
             merge: {:.1} / {:.1} ms",
            merged.len(),
            runs.len(),
            mean(merged.iter().map(|m| m.processing_ms)),
            mean(
                runs.iter()
                    .filter(|m| !m.fts_merge)
                    .map(|m| m.processing_ms)
            ),
        );
        let mut opens = Samples::new();
        for m in runs.iter() {
            opens.push(Duration::from_secs_f64(m.reader_open_ms / 1000.0));
        }
        line(
            "  reader open + row lookup (read-only approximation)",
            &opens,
        );
        if label == "single" {
            rows.push(("single note".to_owned(), total.percentile(0.95)));
        }
    }
    let stats = handle_stats;
    println!(
        "checkpointer: {} PASSIVE runs ({} idle, {} size-bound), {} incomplete, {} RESTART, {} idle \
         starts deferred to work in flight, {} idle wake-ups skipped below the {}-frame floor, \
         {:.0} frames per idle run, longest {:.1} ms, mean {:.1} ms",
        stats.passive_runs,
        stats.idle_runs,
        stats.size_runs,
        stats.incomplete,
        stats.restart_runs,
        stats.deferred_busy,
        stats.floor_skips,
        serat_index::Bounds::default().idle_floor_frames,
        stats.idle_frames as f64 / stats.idle_runs.max(1) as f64,
        stats.passive_max_micros as f64 / 1000.0,
        stats.passive_total_micros as f64 / 1000.0 / stats.passive_runs.max(1) as f64,
    );
    let breaches = budget::gate::breaches(BUDGET_MS, &rows);
    // Task #166: FR-VLT-04's 500 ms is the reference machine's number. On a runner that
    // cannot produce a gate number (hosted windows-latest: the real-time scanner is off) the figure is
    // measured, printed and cannot fail the build; a failure to measure still does (the
    // `?` and the REJECTED paths above), and an eligible runner still fails on a breach.
    if report_only {
        println!(
            "FR-VLT-04: REPORT-ONLY, not asserted: {} (asserted on the reference machine, \
             and on any gate-eligible runner); single note p95 {}",
            environment
                .ineligibility_reason()
                .unwrap_or("not gate eligible"),
            rows.first().map_or_else(String::new, |(_, ms)| format!(
                "{ms:.1} ms against {BUDGET_MS} ms"
            )),
        );
    } else if assert {
        for b in &breaches {
            eprintln!("FR-VLT-04: over budget (--assert): {b}");
        }
    }
    Ok(budget::gate::passes_when_eligible(
        assert, eligible, &breaches,
    ))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("FR-VLT-04: over budget (--assert)");
            ExitCode::FAILURE
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_names_the_stage_that_stopped() {
        // FR-VLT-04, task #167: the watcher delivered nothing.
        let none = describe_stall(0, 100, Duration::from_secs(60), &[], false, 0);
        assert!(
            none.contains("only 0 of 100") && none.contains("in 60 s"),
            "{none}"
        );
        assert!(none.contains("consumer rounds 0 []") && none.contains("rescan not raised"));
        // The watcher lost events (an overflowed ReadDirectoryChangesW buffer).
        let lost = describe_stall(
            1,
            100,
            Duration::from_secs(10),
            &[(170.0, 240.0, 1)],
            true,
            0,
        );
        assert!(
            lost.contains("RAISED since the watcher started (events may have been lost)"),
            "{lost}"
        );
        assert!(
            lost.contains("1 upserted taken 170 ms committed 240 ms"),
            "{lost}"
        );
        // The consumer is behind: a queue with work in it.
        let slow = describe_stall(
            1,
            100,
            Duration::from_secs(10),
            &[(170.0, 240.0, 1)],
            false,
            99,
        );
        assert!(slow.contains("queue depth 99"), "{slow}");
    }

    #[test]
    fn the_overlap_split_reports_p95_per_class_and_the_ratio() {
        // 20 without overlap (10..=29 ms), 10 with (100, 110, ..., 190 ms).
        let mut series: Vec<(f64, bool)> = (10..30).map(|v| (f64::from(v), false)).collect();
        series.extend((0..10).map(|i| (100.0 + 10.0 * f64::from(i), true)));
        let split = OverlapSplit::of(&series);
        assert_eq!((split.with_n, split.without_n), (10, 20));
        // Nearest rank: ceil(0.95 * 20) = 19th = 28; ceil(0.95 * 10) = 10th = 190.
        assert!((split.without_p95.unwrap() - 28.0).abs() < 1e-6);
        assert!((split.with_p95.unwrap() - 190.0).abs() < 1e-6);
        assert!((split.ratio().unwrap() - 190.0 / 28.0).abs() < 1e-6);
        assert!(split.describe().contains("(n=10)") && split.describe().contains("6.79x"));
        // One class empty: no p95, no ratio, no panic.
        let none = OverlapSplit::of(&[(5.0, false)]);
        assert_eq!((none.with_n, none.with_p95, none.ratio()), (0, None, None));
        assert!(none.describe().contains("with n/a (n=0)"));
    }

    #[test]
    fn the_overlap_kinds_are_counted_for_overlapped_iterations_only() {
        // Task #199: report-only; a kind with no overlap is not counted.
        let line = describe_kinds(&[
            (true, Some(RunKind::Idle)),
            (true, Some(RunKind::Idle)),
            (true, Some(RunKind::SizeBound)),
            (true, None),
            (false, None),
        ]);
        assert!(
            line.contains("idle 2, size-bound 1, kind not seen 1"),
            "{line}"
        );
    }

    #[test]
    fn only_the_batch_has_the_long_limit() {
        assert!(BATCH_TIMEOUT > ITERATION_TIMEOUT);
        assert_eq!(ITERATION_TIMEOUT, Duration::from_secs(10));
    }
}
