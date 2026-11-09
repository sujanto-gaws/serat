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
use serat_index::{IndexHandle, rebuild};
use serat_parse::CommonMarkParser;
use serat_vault::{Vault, Watcher, WatcherConfig, WorkQueue, hash_bytes};

/// FR-VLT-04's acceptance criterion.
const BUDGET_MS: f64 = 500.0;
/// How long one iteration may take before the run is rejected.
const ITERATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Quiet time between iterations, so stragglers (a second event for a path already
/// indexed) belong to the iteration that caused them and not to the next one.
const SETTLE: Duration = Duration::from_millis(300);

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
}

struct Measured {
    total_ms: f64,
    watcher_ms: f64,
    processing_ms: f64,
}

impl Run<'_> {
    /// Edit the next `count` notes, wait until all are indexed, and verify the index.
    fn iteration(&mut self, count: usize) -> Result<Measured, String> {
        self.sequence += 1;
        let mut written = Vec::with_capacity(count);
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
        drop(conn);

        std::thread::sleep(SETTLE);
        while self.rounds.try_recv().is_ok() {}

        Ok(Measured {
            total_ms: ms(last_commit.duration_since(t0)),
            watcher_ms: ms(first_taken.duration_since(t0)),
            processing_ms: ms(last_commit.duration_since(first_taken)),
        })
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn run(args: &[String]) -> Result<bool, String> {
    let (mut vault_name, mut singles, mut batches, mut batch_size, mut assert) =
        ("20k".to_owned(), 100usize, 20usize, 100usize, false);
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
    };

    let result = (|| -> Result<(Vec<Measured>, Vec<Measured>), String> {
        let mut single = Vec::new();
        for _ in 0..singles {
            single.push(run.iteration(1)?);
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
        if label == "single" {
            rows.push(("single note".to_owned(), total.percentile(0.95)));
        }
    }
    let breaches = budget::gate::breaches(BUDGET_MS, &rows);
    if assert {
        for b in &breaches {
            eprintln!("FR-VLT-04: over budget (--assert): {b}");
        }
    }
    Ok(budget::gate::passes(assert, &breaches))
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
