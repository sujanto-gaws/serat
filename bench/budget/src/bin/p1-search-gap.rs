//! The search-connection gap, diagnosed (task #53; architecture §4.5, "#47's observation
//! that the search connection is 15-30% slower than a pool reader").
//!
//! Serves **FR-NAV-02**. Honours ADR-0001, ADR-0013. Reports; applies no remedy; gates
//! nothing. It runs the three steps §4.5 lists, and one control the list does not:
//!
//! 1. **Equal warm-up and swapped order.** Each connection gets the same untimed
//!    warm-up. Then the pool reader (P) and the search connection (S) alternate
//!    iteration by iteration, in two blocks, P first and S first, so that a
//!    second-in-the-pair penalty shows up as a gap that changes sign with the order. A
//!    third block runs each connection in a batch of its own, both ways round, which is
//!    how #47 compared them. **The control** is a second pool reader (P2) run against P
//!    the same way: if two ordinary pool readers show a gap of the same size, the gap
//!    belongs to running two connections, not to the search connection.
//! 2. **Pragmas read back** from both connections: `cache_size`, `mmap_size`,
//!    `temp_store`, and the rest that could move a read.
//! 3. **`prepare_cached` hits.** Rusqlite exposes no hit counter, so it is shown from
//!    SQLite's own: `sqlite3_stmt_status(SQLITE_STMTSTATUS_RUN)` is cumulative for one
//!    prepared statement. Across N searches on one connection it climbs by exactly N if
//!    one statement object was reused (a hit), and would read 1 every time if each call
//!    re-prepared. `RePrepare` is read beside it.
//!
//! **Task #56: which block gates.** Block I (R1 on S with pool R2 reads in between,
//! against R1 on S alone) is the product's shape and the criterion at S = 64 MiB.
//! Blocks A-H run R1 on a 2 MiB pool reader as well, which the architecture forbids, so
//! with S at 64 MiB they show S 2-3x slower than P and that does not gate.
//! *Unverified hypothesis:* SQLite's page cache keeps one LRU for every connection in the
//! process, so a full 2 MiB cache recycles the oldest unpinned page of the group, which
//! can be another connection's, instead of its own; at 256 MiB on all three nothing is
//! ever full and the gap vanishes. Source-level support (not proof that it causes the
//! gap): `libsqlite3-sys` 0.38.2's `build.rs` (line 162) compiles SQLite with
//! `SQLITE_ENABLE_MEMORY_MANAGEMENT`, which sets `pcache1.separateCache = 0`
//! (`sqlite3.c`, around line 58370 of the bundled amalgamation), so every connection shares one
//! page-cache group and its LRU. Not tested here (it would need a source-level look at
//! pcache1 or a cache-miss counter per connection).
//!
//! The handler is OFF on S unless a block says otherwise: #47's 15-30% was measured
//! with it off. One block runs it ON for completeness.
//!
//! ```text
//! cargo run --release -p budget --bin p1-search-gap -- --vault 20k --iterations 60
//! ```

use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use budget::{Samples, ensure_vault};
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Connection, StatementStatus};
use serat_index::read::{Reader, SearchOutcome};

fn search_rows(reader: &Reader, query: &str) -> usize {
    match reader.search(query, 0, 50) {
        Ok(SearchOutcome::Hits(hits)) => hits.len(),
        other => panic!("{query:?}: {other:?}"),
    }
}

fn timed(reader: &Reader, term: &str) -> Duration {
    let started = Instant::now();
    let n = search_rows(reader, term);
    let took = started.elapsed();
    assert!(n > 0, "{term:?} matched nothing");
    took
}

/// (Run, RePrepare) of every traced R1 statement, in order.
static SEEN: Mutex<Vec<(i32, i32)>> = Mutex::new(Vec::new());

fn on_profile(event: TraceEvent<'_>) {
    if let TraceEvent::Profile(stmt, _) = event
        && stmt.sql().contains("MATCH")
    {
        SEEN.lock().unwrap_or_else(|e| e.into_inner()).push((
            stmt.get_status(StatementStatus::Run),
            stmt.get_status(StatementStatus::RePrepare),
        ));
    }
}

fn pragmas(conn: &Connection) -> Vec<(String, String)> {
    [
        "cache_size",
        "mmap_size",
        "temp_store",
        "page_size",
        "query_only",
        "journal_mode",
        "busy_timeout",
        "synchronous",
        "foreign_keys",
        "cache_spill",
    ]
    .iter()
    .map(|p| {
        let v: String = conn
            .query_row(&format!("PRAGMA {p}"), [], |r| {
                r.get::<_, rusqlite::types::Value>(0).map(|v| match v {
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Text(t) => t,
                    other => format!("{other:?}"),
                })
            })
            .unwrap_or_else(|e| format!("error: {e}"));
        ((*p).to_owned(), v)
    })
    .collect()
}

/// What `prepare_cache_check` asks of the connection under test. One closure handles all
/// three, because rusqlite 0.41's `trace_v2` needs `&mut Connection` and the pool reader
/// cannot be borrowed mutably and shared at once.
enum Step {
    TraceOn,
    TraceOff,
    Search,
}

fn trace(c: &mut Connection, on: bool) {
    let result = if on {
        c.trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(on_profile))
    } else {
        c.trace_v2(TraceEventCodes::empty(), None::<fn(TraceEvent<'_>)>)
    };
    if let Err(e) = result {
        eprintln!("trace_v2 failed: {e}");
    }
}

/// Run `n` searches under a trace on the connection `step` owns, and report what the
/// statement's own counters say.
fn prepare_cache_check(label: &str, n: usize, step: &mut dyn FnMut(Step)) {
    step(Step::TraceOn);
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).clear();
    for _ in 0..n {
        step(Step::Search);
    }
    step(Step::TraceOff);
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let first = seen.first().copied().unwrap_or((0, 0));
    let last = seen.last().copied().unwrap_or((0, 0));
    let reprepares = seen.iter().map(|s| s.1).max().unwrap_or(0);
    println!(
        "  {label:<28} {n} searches -> {} traced R1 statements · Run counter {} -> {} (climbs by {}; one reused statement object would climb by {}) · RePrepare max {reprepares} · {}",
        seen.len(),
        first.0,
        last.0,
        last.0 - first.0,
        n - 1,
        if seen.len() == n && last.0 - first.0 == i32::try_from(n - 1).unwrap_or(0) {
            "prepare_cached HIT every time"
        } else {
            "NOT a clean hit pattern"
        }
    );
}

/// Alternate `a` and `b` per iteration. `a_first` picks who runs first in the pair.
fn alternate(a: &Reader, b: &Reader, term: &str, n: usize, a_first: bool) -> (Samples, Samples) {
    let (mut sa, mut sb) = (Samples::new(), Samples::new());
    for _ in 0..n {
        if a_first {
            sa.push(timed(a, term));
            sb.push(timed(b, term));
        } else {
            sb.push(timed(b, term));
            sa.push(timed(a, term));
        }
    }
    (sa, sb)
}

fn gap(slow: f64, fast: f64) -> f64 {
    (slow / fast - 1.0) * 100.0
}

fn line(label: &str, a: &Samples, b: &Samples, a_name: &str, b_name: &str) {
    println!(
        "  {label:<34} {a_name} p50 {:>7.2} min {:>7.2} p95 {:>7.2} | {b_name} p50 {:>7.2} min {:>7.2} p95 {:>7.2} | {b_name} vs {a_name}: p50 {:+5.1}% · min {:+5.1}%",
        a.percentile(0.5),
        a.min(),
        a.percentile(0.95),
        b.percentile(0.5),
        b.min(),
        b.percentile(0.95),
        gap(b.percentile(0.5), a.percentile(0.5)),
        gap(b.min(), a.min()),
    );
    // The first iterations as pairs, in run order of the arguments, so that a streak
    // (a connection slow throughout) can be told from scatter.
    let pairs: Vec<String> = a
        .values()
        .iter()
        .zip(b.values())
        .take(8)
        .map(|(x, y)| format!("{x:.0}/{y:.0}"))
        .collect();
    println!("      first pairs {a_name}/{b_name}: {}", pairs.join(" "));
}

fn run(args: &[String]) -> Result<(), String> {
    let mut vault_name = "20k".to_owned();
    let mut n = 60usize;
    let mut extra: Vec<String> = Vec::new();
    let mut only: Option<Vec<String>> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--vault" => vault_name = it.next().ok_or("--vault needs a name")?.clone(),
            "--iterations" => {
                n = it
                    .next()
                    .ok_or("--iterations needs a number")?
                    .parse()
                    .map_err(|e| format!("--iterations: {e}"))?;
            }
            // A diagnostic setting applied to all three connections after they open,
            // e.g. --pragma temp_store=2. Repeatable. The reported run uses none.
            "--pragma" => extra.push(it.next().ok_or("--pragma needs name=value")?.clone()),
            // Comma-separated terms in place of the default set.
            "--terms" => {
                only = Some(
                    it.next()
                        .ok_or("--terms needs a list")?
                        .split(',')
                        .map(str::to_owned)
                        .collect(),
                );
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let settings = Settings { extra, only };
    let vault = ensure_vault(&vault_name)?;
    let index_dir = std::env::temp_dir().join(format!("serat-bench-gap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&index_dir);
    let source = serat_vault::Vault::open(&vault.path).map_err(|e| format!("open vault: {e}"))?;
    serat_index::rebuild(
        &source,
        &serat_parse::CommonMarkParser,
        &vault.path,
        &index_dir,
    )
    .map_err(|e| format!("rebuild: {e}"))?;
    let result = measure(
        &serat_index::index_path(&index_dir),
        &vault_name,
        n,
        &settings,
    );
    let _ = std::fs::remove_dir_all(&index_dir);
    result
}

/// Diagnostic options; the reported run uses neither.
struct Settings {
    extra: Vec<String>,
    only: Option<Vec<String>>,
}

fn measure(
    db: &std::path::Path,
    vault_name: &str,
    n: usize,
    settings: &Settings,
) -> Result<(), String> {
    let env = budget::env::Environment::detect();
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!(
        "P1 search-connection gap · {vault_name} vault · {n} iterations per block · {} · load average {}",
        env.summary(),
        load.split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
    );

    let sc = serat_index::SearchConn::open(db).map_err(|e| e.to_string())?;
    sc.set_progress_handler(false).map_err(|e| e.to_string())?;
    let s = Reader::open(db)
        .map_err(|e| e.to_string())?
        .with_search(sc.clone());
    let mut p = Reader::open(db).map_err(|e| e.to_string())?;
    let p2 = Reader::open(db).map_err(|e| e.to_string())?;
    for pragma in &settings.extra {
        let sql = format!("PRAGMA {pragma}");
        let apply = |c: &Connection| c.execute_batch(&sql).map_err(|e| e.to_string());
        p.with_connection(apply)?;
        p2.with_connection(apply)?;
        sc.with_connection(apply)?;
        println!("diagnostic setting applied to all three connections: {sql}");
    }

    // Step 2: pragmas, read back from the connections themselves.
    println!();
    println!(
        "step 2 · pragmas read back (pool reader P | search connection S | second pool reader P2):"
    );
    let (pp, sp, p2p) = (
        p.with_connection(pragmas),
        sc.with_connection(pragmas),
        p2.with_connection(pragmas),
    );
    let mut all_equal = true;
    for ((name, pv), (sv, p2v)) in pp
        .iter()
        .zip(sp.iter().map(|x| &x.1).zip(p2p.iter().map(|x| &x.1)))
    {
        let same = pv == sv && pv == p2v;
        all_equal &= same;
        println!(
            "  {name:<14} {pv:<12} | {sv:<12} | {p2v:<12} {}",
            if same { "" } else { "<- DIFFERS" }
        );
    }
    println!(
        "  -> pragmas {}",
        if all_equal {
            "identical on all three"
        } else {
            "DIFFER"
        }
    );

    // Terms: those #47 used, plus the commonest inside the 20% line.
    let probe = Connection::open(db).map_err(|e| e.to_string())?;
    probe
        .execute_batch("CREATE VIRTUAL TABLE temp.v USING fts5vocab(main, blocks_fts, 'row')")
        .map_err(|e| e.to_string())?;
    let blocks: i64 = probe
        .query_row("SELECT count(*) FROM blocks", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let doc = |t: &str| -> i64 {
        probe
            .query_row("SELECT doc FROM temp.v WHERE term = ?1", [t], |r| r.get(0))
            .unwrap_or(0)
    };
    let wanted: Vec<String> = settings
        .only
        .clone()
        .unwrap_or_else(|| ["the", "due", "of"].map(str::to_owned).to_vec());
    let terms: Vec<(String, i64)> = wanted
        .iter()
        .map(|t| (t.clone(), doc(t)))
        .filter(|(_, d)| *d > 0)
        .collect();
    // The product's other reads, for block I: the highest in-degree note's backlinks (R2).
    let hub: String = probe
        .query_row(
            "SELECT p.path FROM links l JOIN notes p ON p.id = l.dst_note
              GROUP BY l.dst_note ORDER BY count(*) DESC, p.path LIMIT 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    drop(probe);
    println!();
    println!(
        "terms: {}",
        terms
            .iter()
            .map(|(t, d)| format!("{t:?} ({d} of {blocks} blocks)"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Step 3: prepare_cached hits, after the same warm-up.
    println!();
    println!("step 3 · prepare_cached hits (Run counter of the traced R1 statement):");
    let term0 = &terms[0].0;
    for _ in 0..4 {
        search_rows(&p, term0);
        search_rows(&s, term0);
    }
    prepare_cache_check("pool reader P", 20, &mut |st| match st {
        Step::TraceOn => p.with_connection_mut(|c| trace(c, true)),
        Step::TraceOff => p.with_connection_mut(|c| trace(c, false)),
        Step::Search => {
            search_rows(&p, term0);
        }
    });
    prepare_cache_check("search connection S", 20, &mut |st| match st {
        Step::TraceOn => sc.with_connection_mut(|c| trace(c, true)),
        Step::TraceOff => sc.with_connection_mut(|c| trace(c, false)),
        Step::Search => {
            search_rows(&s, term0);
        }
    });

    // Step 1: the timings.
    println!();
    println!(
        "step 1 · timings, ms (handler OFF on S unless stated; the same untimed warm-up of 6 runs each, interleaved, before every term):"
    );
    for (term, docs) in &terms {
        println!();
        println!("{term:?} ({docs} blocks match):");
        let warm = |x: &Reader, y: &Reader| {
            for _ in 0..6 {
                search_rows(x, term);
                search_rows(y, term);
            }
        };
        warm(&p, &s);
        let (sp1, ss1) = alternate(&p, &s, term, n, true);
        line("A  P then S, per iteration", &sp1, &ss1, "P", "S");
        let (sp2, ss2) = alternate(&p, &s, term, n, false);
        line("B  S then P, per iteration", &sp2, &ss2, "P", "S");
        // Batches, each connection on its own, both orders.
        warm(&p, &s);
        let (mut bp, mut bs) = (Samples::new(), Samples::new());
        for _ in 0..n {
            bp.push(timed(&p, term));
        }
        for _ in 0..n {
            bs.push(timed(&s, term));
        }
        line("C  batch of P, then batch of S", &bp, &bs, "P", "S");
        let (mut dp, mut ds) = (Samples::new(), Samples::new());
        for _ in 0..n {
            ds.push(timed(&s, term));
        }
        for _ in 0..n {
            dp.push(timed(&p, term));
        }
        line("D  batch of S, then batch of P", &dp, &ds, "P", "S");
        // Control: two ordinary pool readers.
        warm(&p, &p2);
        let (cp1, cq1) = alternate(&p, &p2, term, n, true);
        line("E  control P then P2", &cp1, &cq1, "P", "P2");
        let (cp2, cq2) = alternate(&p, &p2, term, n, false);
        line("F  control P2 then P", &cp2, &cq2, "P", "P2");
        // Handler ON.
        sc.set_progress_handler(true).map_err(|e| e.to_string())?;
        warm(&p, &s);
        let (hp, hs) = alternate(&p, &s, term, n, true);
        line("G  P then S, handler ON", &hp, &hs, "P", "S");
        let (hp2, hs2) = alternate(&p, &s, term, n, false);
        line("H  S then P, handler ON", &hp2, &hs2, "P", "S");
        sc.set_progress_handler(false).map_err(|e| e.to_string())?;
        // I: the product's shape (task #56). R1 runs only on S; the pool does R2. S's
        // R1 between pool backlinks reads, against S alone in a batch (the same
        // connection, so a cache stolen by the pool shows as the difference).
        warm(&p, &s);
        let (mut alone, mut mixed) = (Samples::new(), Samples::new());
        for _ in 0..n {
            alone.push(timed(&s, term));
        }
        for _ in 0..n {
            let _ = p.backlinks(&hub).map_err(|e| e.to_string())?;
            mixed.push(timed(&s, term));
        }
        line(
            "I  S alone | S between pool R2 reads",
            &alone,
            &mixed,
            "S",
            "S+R2",
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("p1-search-gap: {e}");
            ExitCode::FAILURE
        }
    }
}
