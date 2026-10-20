//! P1 read paths (architecture §4.5) · latency expectations, not gates.
//!
//! Serves **FR-NAV-02**, **FR-NAV-04**, **FR-STR-03**, **FR-BIN-02**. No NFR budgets
//! search, backlinks or property lookup, so §4.5 states **expectations**: R1 top-50
//! search <= 100 ms p95 (the single most common term reported separately as the
//! worst case), R2 backlinks of the highest in-degree note <= 20 ms, R3 a 500-row
//! property lookup <= 20 ms, R1 + R3 <= 100 ms. A miss is a finding for architect and
//! product-analyst, not a red build, so this job exits 0 whatever it measures and
//! only fails when it cannot measure.
//!
//! It rebuilds the index of the named benchmark vault once (untimed; NFR-PRF-04 times
//! that), then reads it through `serat_index::read::Reader` on a read-only connection,
//! the way the application will. The terms and keys it queries are discovered in the
//! index, not hard-coded, so it follows the generator.
//!
//! ```text
//! cargo run --release -p budget --bin p1-read-paths -- --vault 20k
//! cargo run --release -p budget --bin p1-read-paths -- --vault 50k --iterations 50
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use budget::{Samples, ensure_vault};
use rusqlite::Connection;
use serat_index::read::{Reader, SearchOutcome};

/// One line of the report: a name, what was run, and its expectation in ms.
struct Row {
    name: String,
    samples: Samples,
    rows: usize,
    expectation_ms: f64,
}

fn time<F: FnMut() -> usize>(name: &str, expectation_ms: f64, iterations: usize, mut f: F) -> Row {
    // One untimed run first: the page cache and the statement cache are warm in the
    // application by the second search, and a first-ever query is not what this measures.
    let rows = f();
    let mut samples = Samples::new();
    for _ in 0..iterations {
        let started = Instant::now();
        let n = f();
        samples.push(started.elapsed());
        assert_eq!(
            n, rows,
            "{name}: a repeated read returned a different count"
        );
    }
    Row {
        name: name.to_owned(),
        samples,
        rows,
        expectation_ms,
    }
}

fn search_rows(reader: &Reader, query: &str, offset: u32, limit: u16) -> usize {
    match reader.search(query, offset, limit) {
        Ok(SearchOutcome::Hits(hits)) => hits.len(),
        Ok(other) => panic!("{query:?} produced {other:?}, not hits"),
        Err(e) => panic!("{query:?} failed: {e}"),
    }
}

/// Page through a property lookup the way the panel does, up to `want` rows.
fn paged_rows(reader: &Reader, query: &str, want: usize) -> usize {
    let mut total = 0;
    while total < want {
        let n = search_rows(reader, query, u32::try_from(total).unwrap_or(u32::MAX), 50);
        total += n;
        if n < 50 {
            break;
        }
    }
    total
}

/// Terms by how many blocks contain them, most common first.
fn terms_by_frequency(conn: &Connection, take: usize) -> Result<Vec<String>, String> {
    conn.execute_batch("CREATE VIRTUAL TABLE temp.v USING fts5vocab(main, blocks_fts, 'row')")
        .map_err(|e| format!("fts5vocab: {e}"))?;
    let mut stmt = conn
        .prepare(
            // Only plain letters: a term like `0` or `---` is not what a person
            // searches for, and quoting makes the rest of the vocabulary safe.
            "SELECT term FROM temp.v WHERE term GLOB '[a-z][a-z][a-z]*'
              ORDER BY doc DESC, term LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([i64::try_from(take).unwrap_or(i64::MAX)], |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

fn one_string(conn: &Connection, sql: &str) -> Result<String, String> {
    conn.query_row(sql, [], |r| r.get(0))
        .map_err(|e| format!("{sql}: {e}"))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut vault_name = "20k".to_owned();
    let mut iterations = 50usize;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--vault" => vault_name = it.next().ok_or("--vault needs a name")?.clone(),
            "--iterations" => {
                iterations = it
                    .next()
                    .ok_or("--iterations needs a number")?
                    .parse()
                    .map_err(|e| format!("--iterations: {e}"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let vault = ensure_vault(&vault_name)?;
    let index_dir = std::env::temp_dir().join(format!("serat-bench-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&index_dir);
    let source = serat_vault::Vault::open(&vault.path).map_err(|e| format!("open vault: {e}"))?;
    let rebuilt = Instant::now();
    serat_index::rebuild(
        &source,
        &serat_parse::CommonMarkParser,
        &vault.path,
        &index_dir,
    )
    .map_err(|e| format!("rebuild: {e}"))?;
    eprintln!(
        "p1-read-paths: rebuilt {} notes in {:.1} s (untimed setup)",
        vault.notes,
        rebuilt.elapsed().as_secs_f64()
    );
    let result = measure(
        &serat_index::index_path(&index_dir),
        &vault_name,
        iterations,
    );
    let _ = std::fs::remove_dir_all(&index_dir);
    result
}

fn measure(db: &Path, vault_name: &str, iterations: usize) -> Result<(), String> {
    // Discovery runs on its own read-write connection (a temp vocabulary table), never
    // on the Reader under test.
    let probe = Connection::open(db).map_err(|e| e.to_string())?;
    let terms = terms_by_frequency(&probe, 1100)?;
    if terms.len() < 3 {
        return Err("the index has too few terms to build a query set".to_owned());
    }
    let commonest = terms[0].clone();
    let dense = one_string(
        &probe,
        "SELECT p.path FROM links l JOIN notes p ON p.id = l.dst_note
          WHERE l.kind IN ('wiki','embed','md')
          GROUP BY l.dst_note ORDER BY count(*) DESC, p.path LIMIT 1",
    )?;
    let indegree: i64 = probe
        .query_row(
            "SELECT count(*) FROM links WHERE kind IN ('wiki','embed','md')
                AND dst_note = (SELECT id FROM notes WHERE path = ?1)",
            [&dense],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let big_key = one_string(
        &probe,
        "SELECT key FROM fields WHERE source = 'frontmatter' AND val_text IS NOT NULL
          AND key <> 'type' GROUP BY key ORDER BY count(*) DESC, key LIMIT 1",
    )?;
    // The value of the largest key whose frontmatter row count is nearest 500.
    let (near_500, near_500_n): (String, i64) = probe
        .query_row(
            "SELECT val_text, count(*) c FROM fields
              WHERE key = ?1 AND source = 'frontmatter' AND val_text IS NOT NULL
              GROUP BY val_text ORDER BY abs(count(*) - 500), val_text LIMIT 1",
            [&big_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let (type_val, type_n): (String, i64) = probe
        .query_row(
            "SELECT type, count(*) FROM notes WHERE type IS NOT NULL
              GROUP BY type ORDER BY abs(count(*) - 500), type LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    drop(probe);

    let reader = Reader::open(db).map_err(|e| e.to_string())?;
    let mut rows: Vec<Row> = Vec::new();

    // R1: the single most common term, alone (the worst case) ...
    rows.push(time(
        &format!("R1 worst case: most common term {commonest:?}, top 50"),
        100.0,
        iterations,
        || search_rows(&reader, &commonest, 0, 50),
    ));
    // ... and a query set spread across the vocabulary, reported as one p95.
    let picks = [0usize, 1, 9, 99, 499, 999];
    let mut set = Samples::new();
    let mut set_rows = 0;
    for &i in picks.iter().filter(|&&i| i < terms.len()) {
        let q = &terms[i];
        let r = time("", 100.0, iterations, || search_rows(&reader, q, 0, 50));
        for v in r.samples.values() {
            set.push(std::time::Duration::from_secs_f64(v / 1000.0));
        }
        set_rows += r.rows;
    }
    let pair = format!("{} {}", terms[0], terms[1]);
    let r = time("", 100.0, iterations, || search_rows(&reader, &pair, 0, 50));
    for v in r.samples.values() {
        set.push(std::time::Duration::from_secs_f64(v / 1000.0));
    }
    rows.push(Row {
        name: format!(
            "R1 query set: terms ranked {picks:?} by frequency + the AND of the top two ({} queries)",
            picks.len() + 1
        ),
        samples: set,
        rows: set_rows + r.rows,
        expectation_ms: 100.0,
    });

    // R2: the highest in-degree note.
    rows.push(time(
        &format!("R2 backlinks of {dense:?} (in-degree {indegree})"),
        20.0,
        iterations,
        || reader.backlinks(&dense).map_or(0, |b| b.len()),
    ));

    // R3: one page, then the whole 500-row lookup paged by 50.
    let by_key = format!("[{big_key}:{near_500}]");
    rows.push(time(
        &format!("R3 {by_key} ({near_500_n} rows), first page of 50"),
        20.0,
        iterations,
        || search_rows(&reader, &by_key, 0, 50),
    ));
    rows.push(time(
        &format!("R3 {by_key}, paged to 500 rows"),
        20.0,
        iterations,
        || paged_rows(&reader, &by_key, 500),
    ));
    let by_type = format!("[type:{type_val}]");
    rows.push(time(
        &format!("R4 {by_type} ({type_n} rows, exact), paged to 500 rows"),
        20.0,
        iterations,
        || paged_rows(&reader, &by_type, 500),
    ));

    // R1 + R3: the commonest term filtered by a property.
    let combined = format!("{commonest} {by_key}");
    rows.push(time(
        &format!("R1+R3 {combined:?}, top 50"),
        100.0,
        iterations,
        || search_rows(&reader, &combined, 0, 50),
    ));

    println!(
        "P1 read paths · {vault_name} vault · {iterations} timed runs per read (expectations, not gates)"
    );
    println!();
    for row in &rows {
        let p95 = row.samples.percentile(0.95);
        let verdict = if p95 <= row.expectation_ms {
            "within"
        } else {
            "OVER (a finding for architect and product-analyst, not a red build)"
        };
        println!(
            "{}\n    p50 {:.2} ms · p95 {:.2} ms · max {:.2} ms · {} rows · expectation {:.0} ms: {}",
            row.name,
            row.samples.percentile(0.5),
            p95,
            row.samples.max(),
            row.rows,
            row.expectation_ms,
            verdict
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("p1-read-paths: {e}");
            ExitCode::FAILURE
        }
    }
}
