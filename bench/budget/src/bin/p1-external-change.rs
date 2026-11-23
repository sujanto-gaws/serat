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
//! reported upserted within 10 s, and every edited note's `notes.hash` in the index
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
use serat_index::{IndexHandle, WalHealth, rebuild};
use serat_parse::CommonMarkParser;
use serat_vault::{Vault, Watcher, WatcherConfig, WorkQueue, hash_bytes};

/// FR-VLT-04's acceptance criterion.
const BUDGET_MS: f64 = 500.0;
/// How long one iteration may take before the run is rejected.
const ITERATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Quiet time between iterations, so stragglers (a second event for a path already
/// indexed) belong to the iteration that caused them and not to the next one.
const SETTLE: Duration = Duration::from_millis(300);
// Diagnostic (task #165): `--settle-ms` changes the quiet time between iterations.
// The CI job never passes it; the default stays 300 ms. A settle longer than the
// checkpointer's idle gate (250 ms) plus a checkpoint makes an overlap rarer (each
// checkpoint usually finishes before the next write) but does not remove it: the
// checkpointer also runs on the 2 s idle spacing and the 4,000-frame bound.

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
}

struct Measured {
    total_ms: f64,
    watcher_ms: f64,
    processing_ms: f64,
    /// A WAL checkpoint was running at the write, at the commit, or started or ended
    /// in between (task #165: is the processing tail the checkpointer?).
    checkpoint_overlap: bool,
    /// The FTS5 segment count fell between the write and the commit: a merge ran in
    /// the writer's transaction (the criterion of `p1-save-tail`).
    fts_merge: bool,
    /// Approximation of `Reconciler::reader()` plus the first row lookup, measured on a
    /// fresh connection just before the write (not part of the interval).
    reader_open_ms: f64,
}

impl Run<'_> {
    /// Time what `Reconciler::reader()` does (a new connection, the four connection
    /// pragmas, `query_only`) plus the first row lookup on its cold cache, and read the
    /// FTS5 segment count. An approximation: the product's `schema::open` is not public.
    fn probe_reader(&self) -> Result<(f64, u32), String> {
        let t = Instant::now();
        let conn = Connection::open_with_flags(
            serat_index::index_path(self.index),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL; \
             PRAGMA busy_timeout = 5000; PRAGMA query_only = 1;",
        )
        .map_err(|e| e.to_string())?;
        if let Some(path) = self.candidates.get(self.next) {
            let _: Option<String> = conn
                .query_row("SELECT hash FROM notes WHERE path = ?1", [path], |r| {
                    r.get(0)
                })
                .ok();
        }
        let elapsed = ms(t.elapsed());
        Ok((elapsed, fts_levels(&conn)?.iter().sum()))
    }

    /// Edit the next `count` notes, wait until all are indexed, and verify the index.
    fn iteration(&mut self, count: usize) -> Result<Measured, String> {
        self.sequence += 1;
        let mut written = Vec::with_capacity(count);
        let (reader_open_ms, segments_before) = self.probe_reader()?;
        let stats_before = self.health.stats();
        let runs_before = stats_before.passive_runs + stats_before.restart_runs;
        let running_at_write = self.health.checkpoint_in_progress();
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

        let deadline = t0 + ITERATION_TIMEOUT;
        let (mut upserted, mut first_taken, mut last_commit) = (0, None, t0);
        while upserted < count {
            let wait = deadline.saturating_duration_since(Instant::now());
            let round = self
                .rounds
                .recv_timeout(wait)
                .map_err(|_| format!("only {upserted} of {count} edits were indexed in 10 s"))?;
            upserted += round.outcome.upserted;
            first_taken.get_or_insert(round.first_taken);
            last_commit = round.committed;
        }
        let first_taken = first_taken.unwrap_or(last_commit);
        let checkpoint_overlap = running_at_write || self.health.checkpoint_in_progress() || {
            let now = self.health.stats();
            now.passive_runs + now.restart_runs != runs_before
        };

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
        let fts_merge = fts_levels(&conn)?.iter().sum::<u32>() < segments_before;
        drop(conn);

        std::thread::sleep(self.settle);
        while self.rounds.try_recv().is_ok() {}

        Ok(Measured {
            total_ms: ms(last_commit.duration_since(t0)),
            watcher_ms: ms(first_taken.duration_since(t0)),
            processing_ms: ms(last_commit.duration_since(first_taken)),
            checkpoint_overlap,
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

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn run(args: &[String]) -> Result<bool, String> {
    let (mut vault_name, mut singles, mut batches, mut batch_size, mut assert) =
        ("20k".to_owned(), 100usize, 20usize, 100usize, false);
    let mut settle = SETTLE;
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

    let mut run = Run {
        root: &root,
        index: &index_dir,
        rounds: &rx,
        candidates,
        next: 0,
        sequence: 0,
        settle,
        health: handle.wal_health(),
    };

    let result = (|| -> Result<(Vec<Measured>, Vec<Measured>), String> {
        let mut single = Vec::new();
        let cpu_before = cpu_jiffies();
        for _ in 0..singles {
            single.push(run.iteration(1)?);
        }
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
            if label == "single" {
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
        line("  reader open + row lookup (approx.)", &opens);
        if label == "single" {
            rows.push(("single note".to_owned(), total.percentile(0.95)));
        }
    }
    let stats = handle_stats;
    println!(
        "checkpointer: {} PASSIVE runs, {} incomplete, {} RESTART, longest {:.1} ms, mean {:.1} ms",
        stats.passive_runs,
        stats.incomplete,
        stats.restart_runs,
        stats.passive_max_micros as f64 / 1000.0,
        stats.passive_total_micros as f64 / 1000.0 / stats.passive_runs.max(1) as f64,
    );
    let breaches = budget::gate::breaches(BUDGET_MS, &rows);
    // Task #166: FR-VLT-04's 500 ms is the reference machine's number. On a runner that
    // cannot produce a gate number (hosted windows-latest, 2 cores) the figure is
    // measured, printed and cannot fail the build; a failure to measure still does (the
    // `?` and the REJECTED paths above), and an eligible runner still fails on a breach.
    let environment = budget::env::Environment::detect();
    let eligible = environment.gate_eligible();
    if budget::gate::report_only(assert, eligible) {
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
