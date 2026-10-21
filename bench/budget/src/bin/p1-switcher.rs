//! NFR-PRF-06 allocation, Rust side: `Corpus::query` over the benchmark vault's index
//! (architecture §9.2) · reported, not the gate.
//!
//! Serves **NFR-PRF-06**, **FR-NAV-01**, **US-02**. The binding figure is keystroke to
//! painted results, under 50 ms at p95 at 20,000 notes, in the real binary (task #10's
//! job, unmerged). §9.2 allots **20 ms p95** of it to `Switcher::query`, and that is
//! what this times: the matcher alone, in-process, over the corpus built from the vault's
//! index. It excludes IPC, serialisation and render, so it is a lower bound of the gate.
//!
//! The query set is derived from the index (titles, aliases, headings, paths) and is
//! deterministic: three-character title prefixes, three-character subsequences spread
//! across a title, path-stem queries, alias prefixes, queries that match nothing, and
//! the three `#` shapes that put heading mode on the gated third keystroke (`#ab...`,
//! `a#b...`, `ab#...`). Each query is typed one character at a time and every keystroke
//! is timed; keystroke 3 (counting `#`) is the gated one, the rest are reported.
//!
//! History is declared on every line: `absent`, or `fixture-2000` (2,000 deterministic
//! opens, the record's cap), because frecency changes the work and the order.
//!
//! **A run is rejected, not recorded, unless:** every no-match query returns zero
//! results; every heading-mode query shape returns results; every full-title query
//! returns its note in the top 10 (the 3-character queries cannot be held to that: a
//! three-letter prefix of "Activation 12" is shared by hundreds of notes).
//!
//! ```text
//! cargo run --release -p budget --bin p1-switcher -- --vault 20k
//! cargo run --release -p budget --bin p1-switcher -- --vault 50k --passes 3
//! ```

use std::process::ExitCode;
use std::time::Instant;

use budget::{Samples, ensure_vault};
use rusqlite::{Connection, OpenFlags};
use serat_index::history::History;
use serat_index::switcher::Corpus;
use serat_vault::Vault;

/// §9.2's allocation to `Switcher::query`, p95.
const ALLOCATION_MS: f64 = 20.0;
const NOW_MS: i64 = 1_800_000_000_000;

struct Query {
    text: String,
    /// The note a full-length query is built from, when it must come back in the top 10.
    must_find: Option<String>,
    expect_none: bool,
    heading_mode: bool,
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn read_strings(conn: &Connection, sql: &str) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

fn queries(db: &std::path::Path) -> Result<Vec<Query>, String> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let notes = read_strings(
        &conn,
        "SELECT path, title FROM notes WHERE kind = 'note' AND title IS NOT NULL ORDER BY path",
    )?;
    let aliases = read_strings(
        &conn,
        "SELECT n.path, f.val_text FROM fields f JOIN notes n ON n.id = f.note_id \
         WHERE f.source = 'frontmatter' AND f.key IN ('aliases','alias') \
         AND f.val_text IS NOT NULL ORDER BY n.path LIMIT 5000",
    )?;
    let headings = read_strings(
        &conn,
        "SELECT n.path, b.text FROM blocks b JOIN notes n ON n.id = b.note_id \
         WHERE b.kind = 'heading' AND b.text IS NOT NULL AND b.ord < 2 \
         ORDER BY n.path LIMIT 5000",
    )?;
    if notes.len() < 1000 {
        return Err(format!("only {} notes in the index", notes.len()));
    }
    eprintln!(
        "sources: {} notes, {} aliases, {} headings",
        notes.len(),
        aliases.len(),
        headings.len()
    );
    let pick = |v: &[(String, String)], count: usize, offset: usize| -> Vec<(String, String)> {
        if v.is_empty() {
            return Vec::new();
        }
        let step = (v.len() / count).max(1);
        (0..count)
            .filter_map(|i| v.get((i * step + offset) % v.len()).cloned())
            .collect()
    };
    let mut out = Vec::new();
    let usable = |t: &str| t.chars().filter(|c| c.is_alphanumeric()).count() >= 4;

    // Three-character title prefixes.
    for (path, title) in pick(&notes, 80, 0) {
        if usable(&title) {
            out.push(Query {
                text: first_chars(&title, 3).to_lowercase(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
            out.push(Query {
                text: title.to_lowercase(),
                must_find: Some(path),
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Three-character subsequences spread across a title.
    for (_, title) in pick(&notes, 40, 7) {
        let chars: Vec<char> = title
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        if chars.len() >= 4 {
            out.push(Query {
                text: [chars[0], chars[chars.len() / 2], chars[chars.len() - 1]]
                    .iter()
                    .collect(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Path-stem queries: a folder name and the start of a file name.
    for (path, _) in pick(&notes, 30, 3) {
        if let Some((dir, file)) = path.rsplit_once('/') {
            let dir = dir.rsplit('/').next().unwrap_or(dir);
            out.push(Query {
                text: format!("{}/{}", first_chars(dir, 3), first_chars(file, 3)).to_lowercase(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Alias prefixes.
    for (_, alias) in pick(&aliases, 20, 0) {
        if usable(&alias) {
            out.push(Query {
                text: first_chars(&alias, 3).to_lowercase(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Nothing matches these: no note contains a section sign or a tilde.
    for i in 0..30 {
        let c = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
        let text = match i % 3 {
            0 => format!("\u{a7}{c}\u{a7}"),
            1 => format!("~~{c}"),
            _ => format!("{c}\u{a7}~\u{a7}"),
        };
        out.push(Query {
            text,
            must_find: None,
            expect_none: true,
            heading_mode: false,
        });
    }
    // Heading mode, the three shapes of §9.2: `#ab...`, `a#b...`, `ab#...`.
    for (_, heading) in pick(&headings, 20, 0) {
        out.push(Query {
            text: format!("#{}", first_chars(&heading, 3).to_lowercase()),
            must_find: None,
            expect_none: false,
            heading_mode: true,
        });
    }
    for ((_, title), (_, heading)) in pick(&notes, 20, 11).into_iter().zip(pick(&headings, 20, 5)) {
        out.push(Query {
            text: format!(
                "{}#{}",
                first_chars(&title, 1).to_lowercase(),
                first_chars(&heading, 3).to_lowercase()
            ),
            must_find: None,
            expect_none: false,
            heading_mode: true,
        });
    }
    for (_, title) in pick(&notes, 20, 13) {
        out.push(Query {
            text: format!("{}#", first_chars(&title, 2).to_lowercase()),
            must_find: None,
            expect_none: false,
            heading_mode: true,
        });
    }
    Ok(out)
}

/// 2,000 deterministic opens over the vault's notes, spread over a month.
fn history_fixture(corpus_paths: &[String]) -> History {
    let mut h = History::default();
    let step = (corpus_paths.len() / 2000).max(1);
    for (n, path) in corpus_paths.iter().step_by(step).take(2000).enumerate() {
        let age = i64::try_from(n * 997 % 30).unwrap_or(0) * 86_400_000;
        h.record_open(path, NOW_MS - age);
        if n % 3 == 0 {
            h.record_open(path, NOW_MS - age / 2);
        }
    }
    h
}

fn line(label: &str, s: &Samples) {
    println!(
        "{label:<34} median {:>7.2} ms   p95 {:>7.2} ms   max {:>7.2} ms   (n={})",
        s.median(),
        s.percentile(0.95),
        s.max(),
        s.len()
    );
}

fn run(args: &[String]) -> Result<(), String> {
    let mut vault_name = "20k".to_owned();
    let mut passes = 2usize;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--vault" => vault_name = it.next().ok_or("--vault needs a name")?.clone(),
            "--passes" => {
                passes = it
                    .next()
                    .ok_or("--passes needs a number")?
                    .parse()
                    .map_err(|e| format!("--passes: {e}"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let under_test = ensure_vault(&vault_name)?;
    let vault = Vault::open(&under_test.path).map_err(|e| format!("open vault: {e}"))?;
    let scratch = std::env::temp_dir().join(format!("serat-bench-switcher-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    serat_index::rebuild(
        &vault,
        &serat_parse::CommonMarkParser,
        &under_test.path,
        &scratch,
    )
    .map_err(|e| format!("rebuild (untimed setup): {e}"))?;
    let db = serat_index::index_path(&scratch);

    // Corpus build, reported.
    let mut build = Samples::new();
    let mut corpus = None;
    for _ in 0..5 {
        let started = Instant::now();
        let c = Corpus::from_index(&db).map_err(|e| e.to_string())?;
        build.push(started.elapsed());
        corpus = Some(c);
    }
    let corpus = corpus.ok_or("no corpus")?;
    let tree_started = Instant::now();
    let paths: Vec<String> = {
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        read_strings(
            &conn,
            "SELECT path, path FROM notes WHERE kind = 'note' ORDER BY path",
        )?
        .into_iter()
        .map(|(p, _)| p)
        .collect()
    };
    let tree = Corpus::from_paths(&paths);
    let tree_build = tree_started.elapsed();

    let set = queries(&db)?;
    let absent = History::default();
    let fixture = history_fixture(&paths);

    // Validity: rejected, not recorded.
    for q in &set {
        let hits = corpus.query(&q.text, 50, &absent, NOW_MS);
        if q.expect_none && !hits.is_empty() {
            return Err(format!(
                "no-match query {:?} returned {} results; run rejected",
                q.text,
                hits.len()
            ));
        }
        if q.heading_mode && hits.is_empty() {
            return Err(format!(
                "heading-mode query {:?} returned nothing; run rejected",
                q.text
            ));
        }
        if let Some(want) = &q.must_find
            && !hits.iter().take(10).any(|h| &h.path == want)
        {
            return Err(format!(
                "{:?} did not return {want} in its top 10; run rejected",
                q.text
            ));
        }
    }
    let empty = corpus.query("", 20, &fixture, NOW_MS);
    if empty.is_empty() {
        return Err("empty query returned nothing with a history; run rejected".into());
    }

    println!(
        "p1-switcher · vault {vault_name} · {} notes, {} aliases, {} headings · {} queries · \
         {passes} passes",
        corpus.len(),
        corpus.alias_count(),
        corpus.heading_count(),
        set.len()
    );
    println!("query set: derived from the index, deterministic; headings drawn from vaultgen's");
    println!("small fixed vocabulary, so heading-mode match counts overstate a real vault.\n");
    line("corpus build (from index)", &build);
    println!(
        "{:<34} {:.1} ms  (Tree corpus of {} notes from {} paths)",
        "corpus build (from paths)",
        tree_build.as_secs_f64() * 1000.0,
        tree.len(),
        paths.len()
    );

    for (label, history) in [("absent", &absent), ("fixture-2000", &fixture)] {
        let (mut gated, mut gated_default, mut gated_heading, mut all) = (
            Samples::new(),
            Samples::new(),
            Samples::new(),
            Samples::new(),
        );
        for _ in 0..passes {
            for q in &set {
                let chars: Vec<(usize, char)> = q.text.char_indices().collect();
                for (k, &(at, c)) in chars.iter().enumerate() {
                    let typed = &q.text[..at + c.len_utf8()];
                    let started = Instant::now();
                    let hits = corpus.query(typed, 50, history, NOW_MS);
                    let took = started.elapsed();
                    std::hint::black_box(&hits);
                    all.push(took);
                    if k == 2 {
                        gated.push(took);
                        if q.heading_mode {
                            gated_heading.push(took);
                        } else {
                            gated_default.push(took);
                        }
                    }
                }
            }
        }
        println!("\nhistory: {label}");
        line("  keystroke 3 (gated position)", &gated);
        line("    default mode", &gated_default);
        line("    heading mode", &gated_heading);
        line("  every keystroke (reported)", &all);
        let p95 = gated.percentile(0.95);
        println!(
            "  NFR-PRF-06 allocation, history {label}: Switcher::query p95 {p95:.2} ms against \
             {ALLOCATION_MS:.0} ms ({:.0}% of allocation). {}",
            100.0 * p95 / ALLOCATION_MS,
            if p95 <= ALLOCATION_MS {
                "Within."
            } else {
                "OVER: a finding, not a failure."
            }
        );
    }
    println!(
        "\nBasis: in-process, one thread, release build expected; excludes IPC, serialisation \
         and render. The binding figure is keystroke to painted results in the real binary (task #10)."
    );
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("p1-switcher: {e}");
            ExitCode::FAILURE
        }
    }
}
