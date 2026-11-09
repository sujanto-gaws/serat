//! FR-VLT-01 tree path, Rust side (ADR-0015 §3) · the CI gate with `--assert`, as a
//! lower bound of the binding interval.
//!
//! Serves **FR-VLT-01**, **NFR-CMP-03**, **NFR-PRF-01**. FR-VLT-01's binding interval
//! is `tree_complete - T_open` in the running application (task #24's in-app mark). This times
//! the part of it that is this repository's Rust: from "the tree session starts" to
//! the moment `TreeOut::Complete` is written to the sink, on each of ADR-0015's two
//! arms, with the sink doing the work a channel does that is not IPC (it counts
//! messages and name bytes, so the stream's size is on the page). It excludes
//! `ipc::Channel` serialisation, the webview's apply and the second
//! `requestAnimationFrame`; so it is a **lower bound** of the gated number, and the
//! verdict line says so.
//!
//! * **warm arm** — a valid index: `read_tree_rows`, `group_listings`, the session.
//!   `source = index`. The index is built once, untimed (NFR-PRF-04 times that).
//! * **no-index arm** — no `.serat/`: the streaming walk populates the tree **while
//!   a rebuild runs in another thread**, because that concurrency is the product's
//!   behaviour on first open and measuring the walk alone is the category error
//!   ADR-0015 §3 names. `source = walk`. The rebuild runs to the end (untimed) so
//!   each iteration starts from the same disk state.
//!
//! **Not recorded here:** ADR-0015 §3's validity condition 4 (`cpu_load_percent`,
//! `concurrent_build_processes`, `gate_eligible`). The bin does not print them; the
//! `p1-tree-path` job in budgets.yml runs the `environment` bin beside it and records
//! them, so this bin's number is gate-eligible evidence only with that line.
//!
//! Each run is rejected, not recorded, unless `Complete.entries` equals the manifest's
//! `notes + assets` and `source` matches the arm (ADR-0015 §3, validity conditions 1-2).
//! After the warm arm's `Complete` the authoritative walk is timed too (catch-up and
//! `WalkSettled`), since that is the cost the first screen does not wait for.
//!
//! ```text
//! cargo run --release -p budget --bin p1-tree-path -- --vault 20k
//! cargo run --release -p budget --bin p1-tree-path -- --vault 50k --iterations 5 --cold-iterations 3
//! ```
//!
//! Without `--assert` it exits 0 whatever it measures; it fails only when it cannot
//! measure or a run is invalid. With `--assert` (NFR-PRF-08) it also exits 1 if either
//! arm's p95 is over `BUDGET_MS`: the Rust side is a lower bound of the gated interval,
//! so a breach here is a breach of the real figure, though a pass proves only that this
//! part fits. At the default n=10 / n=5 the nearest-rank p95 is the max sample. Only
//! meaningful on the 20k vault, so `--assert` refuses any other.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use budget::{Samples, ensure_vault};
use serat_core::TreeOp;
use serat_vault::{TreeOut, TreeSession, TreeSource, Vault};

/// FR-VLT-01's budget at 20,000 notes, p95.
const BUDGET_MS: f64 = 2000.0;

#[derive(Default)]
struct Stream {
    started: Option<Instant>,
    first_listing: Option<Duration>,
    complete: Option<(Duration, u32, TreeSource)>,
    settled: Option<Duration>,
    messages: usize,
    name_bytes: usize,
}

fn session(started: Instant) -> (TreeSession, Arc<Mutex<Stream>>) {
    let stream = Arc::new(Mutex::new(Stream {
        started: Some(started),
        ..Stream::default()
    }));
    let sink = Arc::clone(&stream);
    let session = TreeSession::new(move |out| {
        let mut s = sink.lock().expect("stream");
        let at = s.started.expect("started").elapsed();
        s.messages += 1;
        match out {
            TreeOut::Op(TreeOp::Listing { entries, .. }) => {
                s.first_listing.get_or_insert(at);
                s.name_bytes += entries.iter().map(|e| e.name.len() + 2).sum::<usize>();
            }
            TreeOut::Op(_) => {}
            TreeOut::Complete { entries, source } => s.complete = Some((at, entries, source)),
            TreeOut::WalkSettled { .. } => s.settled = Some(at),
        }
    });
    (session, stream)
}

fn line(label: &str, s: &Samples) {
    println!(
        "{label:<28} median {:>8.1} ms   p95 {:>8.1} ms   max {:>8.1} ms   (n={})",
        s.median(),
        s.percentile(0.95),
        s.max(),
        s.len()
    );
}

fn run(args: &[String]) -> Result<bool, String> {
    let mut vault_name = "20k".to_owned();
    let mut iterations = 10usize;
    let mut cold_iterations = 5usize;
    let mut assert = false;
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
            "--iterations" => iterations = number("--iterations")?,
            "--cold-iterations" => cold_iterations = number("--cold-iterations")?,
            "--assert" => assert = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    budget::gate::require_only_20k(assert, &vault_name)?;
    let under_test = ensure_vault(&vault_name)?;
    let expected =
        u32::try_from(under_test.notes + under_test.assets).map_err(|e| e.to_string())?;
    let vault = Arc::new(Vault::open(&under_test.path).map_err(|e| format!("open vault: {e}"))?);
    let scratch = std::env::temp_dir().join(format!("serat-bench-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    // ---- warm arm -------------------------------------------------------------
    let warm_dir = scratch.join("warm");
    serat_index::rebuild(
        &*vault,
        &serat_parse::CommonMarkParser,
        &under_test.path,
        &warm_dir,
    )
    .map_err(|e| format!("rebuild (untimed setup): {e}"))?;
    let db = serat_index::index_path(&warm_dir);

    let (mut complete, mut first, mut catch_up, mut settle) = (
        Samples::new(),
        Samples::new(),
        Samples::new(),
        Samples::new(),
    );
    let mut shape = (0, 0);
    for round in 0..=iterations {
        let started = Instant::now();
        let (session, stream) = session(started);
        // The snapshot goes before the read, as it must in the application.
        let snapshot = session.begin_index_read().ok_or("session poisoned")?;
        let rows = serat_index::tree::read_tree_rows(&db).map_err(|e| format!("rows: {e}"))?;
        if !session.populate_from_index(snapshot, rows.iter().map(|(p, k)| (p.as_str(), *k))) {
            return Err("warm arm: the index tree was not sent".into());
        }
        let (at, entries, source) = stream
            .lock()
            .expect("stream")
            .complete
            .ok_or("warm arm: no Complete")?;
        check("warm", entries, source, TreeSource::Index, expected)?;
        let after_complete = Instant::now();
        let summary = session
            .populate_from_walk(&vault)
            .map_err(|e| format!("walk: {e}"))?;
        let walk_took = after_complete.elapsed();
        let s = stream.lock().expect("stream");
        if round > 0 {
            complete.push(at);
            first.push(s.first_listing.ok_or("no listing")?);
            catch_up.push(walk_took);
            settle.push(s.settled.ok_or("no WalkSettled")? - at);
            shape = (s.messages, s.name_bytes);
        }
        if round == 0 {
            println!(
                "walk summary: {} dirs, {} files, {} empty folders",
                summary.dirs, summary.files, summary.empty_folders
            );
        }
    }

    // ---- no-index arm ---------------------------------------------------------
    let (mut cold_complete, mut cold_first) = (Samples::new(), Samples::new());
    for round in 0..cold_iterations {
        let index_dir = scratch.join(format!("cold-{round}"));
        let started = Instant::now();
        let rebuild = {
            let (vault, root, index_dir) = (
                Arc::clone(&vault),
                under_test.path.clone(),
                index_dir.clone(),
            );
            thread::spawn(move || {
                serat_index::rebuild(&*vault, &serat_parse::CommonMarkParser, &root, &index_dir)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
        };
        let (session, stream) = session(started);
        session
            .populate_from_walk(&vault)
            .map_err(|e| format!("walk: {e}"))?;
        let (at, entries, source) = stream
            .lock()
            .expect("stream")
            .complete
            .ok_or("no-index arm: no Complete")?;
        check("no-index", entries, source, TreeSource::Walk, expected)?;
        cold_complete.push(at);
        cold_first.push(
            stream
                .lock()
                .expect("stream")
                .first_listing
                .ok_or("no listing")?,
        );
        rebuild
            .join()
            .map_err(|_| "rebuild thread panicked".to_owned())??;
        let _ = std::fs::remove_dir_all(&index_dir);
    }
    let _ = std::fs::remove_dir_all(&scratch);

    println!("p1-tree-path · vault {vault_name} · {expected} file entries expected\n");
    println!("warm arm (index Ready, source = index)");
    line("  first listing (root)", &first);
    line("  Complete", &complete);
    println!(
        "  stream: {} messages, ~{} KiB of names",
        shape.0,
        shape.1 / 1024
    );
    line("  walk catch-up after", &catch_up);
    line("  WalkSettled - Complete", &settle);
    println!("\nno-index arm (rebuild running beside the walk, source = walk)");
    line("  first listing (root)", &cold_first);
    line("  Complete", &cold_complete);

    println!();
    let mut rows = Vec::new();
    for (arm, s) in [("warm", &complete), ("no-index", &cold_complete)] {
        let p95 = s.percentile(0.95);
        rows.push((format!("{arm} arm"), p95));
        println!(
            "FR-VLT-01 Rust-side lower bound, {arm} arm: p95 {p95:.1} ms against {BUDGET_MS:.0} ms \
             ({:.1}% of budget; n={}, and at n<=20 nearest-rank p95 is the max sample). \
             Excludes channel serialisation and the webview apply+paint; a breach here is a \
             breach of the binding figure, a pass proves only this part fits. The in-app \
             tree_complete - T_open belongs to task #24.",
            100.0 * p95 / BUDGET_MS,
            s.len()
        );
    }
    let breaches = budget::gate::breaches(BUDGET_MS, &rows);
    if assert {
        for b in &breaches {
            eprintln!("FR-VLT-01: Rust-side lower bound over budget (--assert): {b}");
        }
    }
    Ok(budget::gate::passes(assert, &breaches))
}

fn check(
    arm: &str,
    entries: u32,
    source: TreeSource,
    want_source: TreeSource,
    expected: u32,
) -> Result<(), String> {
    if entries != expected {
        return Err(format!(
            "{arm} arm: Complete.entries {entries} != manifest notes + assets {expected}; run rejected"
        ));
    }
    if source != want_source {
        return Err(format!(
            "{arm} arm: source {source:?}, wanted {want_source:?}; run rejected"
        ));
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("p1-tree-path: {e}");
            ExitCode::FAILURE
        }
    }
}
