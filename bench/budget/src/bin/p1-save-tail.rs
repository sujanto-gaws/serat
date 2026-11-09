//! The hub-note save tail, attributed by measurement (task #53; architecture §3.1.1,
//! "The hub-note save").
//!
//! Serves **NFR-PRF-03**, **NFR-PRF-04**, **FR-NAV-02**. Honours ADR-0001, ADR-0002,
//! ADR-0013. It is a measurement and applies no remedy: it re-saves the highest
//! in-degree note of the vault `--iterations` times, each save submitted `--gap-ms`
//! after the previous `flush()` returned (ADR-0016's 1 s save debounce; a shorter gap
//! is for iterating on the harness and is not a reporting run), and times, per save:
//!
//! * `read`: `NoteSource::load` (the file read and the blake3 hash);
//! * `parse`: `NoteParser::parse`;
//! * `submit`: `IndexWriter::submit`;
//! * `wake`: submit returning to the writer's `BEGIN` completing (queue and wake-up);
//! * `stmts`: `BEGIN` completing to `COMMIT` starting (every statement in the
//!   transaction, including `sync_note_count`);
//! * `commit`: the last in-transaction statement finishing to the WAL hook being
//!   entered, which is `COMMIT`'s own work: FTS5's flush-and-merge (its `xSync`) and the
//!   WAL write;
//! * `ckpt`: the auto-checkpoint, timed inside the WAL hook. It runs in the same
//!   `sqlite3_step` as `COMMIT` but after the statement's profile event;
//! * `post`: the hook returning to `flush()` returning (the `count(*)` and ANALYZE
//!   check in `writer::commit`, and the reply).
//!
//! `read + parse + submit + wake + stmts + commit + ckpt + post` is the NFR-PRF-03 interval
//! (`apply()` submitted to `flush()` returning), reported as `total`.
//!
//! **How the phases are seen without touching serat-index.** The writer connection is
//! opened here with `serat_index::open`, a `trace_v2` profile callback and a `wal_hook`
//! are installed on it, and it is handed to `IndexActor::with_connection`, so the
//! actor, its commit loop and its pragmas are the product's. The profile callback
//! stamps `BEGIN` and `COMMIT`. **A WAL hook replaces SQLite's own auto-checkpoint**
//! (that is how `wal_autocheckpoint` is implemented), so the hook here re-implements
//! it exactly as `sqlite3WalDefaultHook` does: `PASSIVE` checkpoint of the database
//! when the WAL holds at least `--wal-autocheckpoint` frames (default 1000, SQLite's).
//! `--bare` runs without the callback and the hook, on SQLite's own, to measure the
//! instrumentation's cost.
//!
//! **Since task #56 the writer is the product's:** `IndexActor` sets
//! `wal_autocheckpoint = 0` and runs the checkpointer (idle gate 250 ms, size bound 4,000
//! frames), so `ckpt` is 0 by construction -- the writer's hook only signals -- and the
//! checkpointer's own work is reported beside the table (runs, longest, and how many
//! saves had a checkpoint in progress when they started or ended). The #53 behaviour is
//! still reachable as a diagnostic: `--wal-autocheckpoint 1000` turns the checkpointer
//! off and re-implements SQLite's inline auto-checkpoint in the hook, as before;
//! `--checkpoint-between` does the same with an explicit PASSIVE between saves.
//!
//! ```text
//! cargo run --release -p budget --bin p1-save-tail -- --vault 20k --iterations 1000 --gap-ms 1000
//! ```

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use budget::{Samples, ensure_vault};
use rusqlite::hooks::{CheckpointMode, Wal};
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Connection, OpenFlags};
use serat_core::{
    IndexOp, IndexWriter as _, LoadedNote, NoteKind, NoteParser as _, NoteSource as _, VaultPath,
};
use serat_index::{ActorOptions, CheckpointStats};

/// The auto-checkpoint threshold the hook re-implements for the #53 diagnostic. 0 never
/// checkpoints (the product's checkpointer does).
static AUTOCKPT: AtomicI32 = AtomicI32::new(0);

/// One commit's WAL hook report.
#[derive(Clone, Copy, Default, Debug)]
struct WalEv {
    /// WAL frames after this commit (the hook's `pages` argument).
    frames: i32,
    /// Whether the hook ran a checkpoint.
    ran: bool,
    /// Frames in the WAL, and frames checkpointed, as `wal_checkpoint_v2` reports.
    n_log: i32,
    n_ckpt: i32,
    /// The checkpoint call failed (e.g. SQLITE_BUSY). Like sqlite3WalDefaultHook, the hook
    /// ignores that and returns Ok; the failure is only counted.
    ckpt_err: bool,
    /// The checkpoint's own duration.
    dur: Duration,
}

/// One transaction as the writer connection saw it.
#[derive(Clone, Copy, Debug)]
struct TxRec {
    begin_end: Instant,
    /// When the last statement before `COMMIT` finished.
    commit_start: Instant,
    /// When the WAL hook was entered, which is when `COMMIT`'s own work is done.
    commit_end: Instant,
    stmts: u32,
    wal: WalEv,
    analyze: u32,
}

struct Probe {
    begin_end: Option<Instant>,
    last_stmt_end: Option<Instant>,
    stmts: u32,
    last: Option<TxRec>,
}

static PROBE: Mutex<Probe> = Mutex::new(Probe {
    begin_end: None,
    last_stmt_end: None,
    stmts: 0,
    last: None,
});

// SQLite's profile callback fires inside sqlite3Step *before* doWalCallbacks (verified in
// the bundled amalgamation), so the WAL hook, and with it any auto-checkpoint, runs after
// the COMMIT statement's profile event. The first version of this probe attached the hook
// event to the *next* transaction and misplaced every checkpoint by one save. The hook
// therefore attaches to the transaction whose COMMIT has just been profiled.
//
// The profile duration is quantised to 1 ms (the unix VFS clock is a millisecond Julian
// day), so it is not used: the boundaries are `Instant`s taken in the callbacks.
fn on_profile(event: TraceEvent<'_>) {
    let TraceEvent::Profile(stmt, _) = event else {
        return;
    };
    let now = Instant::now();
    let sql = stmt.sql();
    let mut p = PROBE.lock().unwrap_or_else(|e| e.into_inner());
    if sql.starts_with("BEGIN") {
        p.begin_end = Some(now);
        p.last_stmt_end = Some(now);
        p.stmts = 0;
        p.last = None;
    } else if sql.trim() == "COMMIT" {
        if let Some(begin_end) = p.begin_end.take() {
            let commit_start = p.last_stmt_end.unwrap_or(now);
            p.last = Some(TxRec {
                begin_end,
                commit_start,
                commit_end: now,
                stmts: p.stmts,
                wal: WalEv::default(),
                analyze: 0,
            });
        }
    } else if p.begin_end.is_some() {
        p.stmts += 1;
        p.last_stmt_end = Some(now);
    } else if let Some(last) = p.last.as_mut()
        && sql.trim_start().starts_with("ANALYZE")
    {
        last.analyze += 1;
    }
}

fn on_wal(wal: &Wal, frames: i32) -> rusqlite::Result<()> {
    let entered = Instant::now();
    let mut ev = WalEv {
        frames,
        ..WalEv::default()
    };
    let threshold = AUTOCKPT.load(Ordering::Relaxed);
    if threshold > 0 && frames >= threshold {
        // sqlite3WalDefaultHook: sqlite3_wal_checkpoint(db, zDb), which is PASSIVE.
        match wal.checkpoint_v2(CheckpointMode::PASSIVE) {
            Ok((n_log, n_ckpt)) => {
                ev.n_log = n_log;
                ev.n_ckpt = n_ckpt;
            }
            Err(_) => ev.ckpt_err = true,
        }
        ev.ran = true;
        ev.dur = entered.elapsed();
    }
    if let Some(last) = PROBE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last
        .as_mut()
    {
        last.commit_end = entered;
        last.wal = ev;
    }
    Ok(())
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

fn loadavg() -> (String, u32) {
    let s = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let f: Vec<&str> = s.split_whitespace().collect();
    let running = f
        .get(3)
        .and_then(|r| r.split('/').next())
        .and_then(|r| r.parse().ok())
        .unwrap_or(0);
    (
        f.iter().take(3).copied().collect::<Vec<_>>().join(" "),
        running,
    )
}

/// CPU-bound build processes elsewhere on the host: `env.rs` reports none on Linux,
/// so the attestation is taken here from /proc/*/comm.
fn build_processes() -> usize {
    const NAMES: [&str; 8] = [
        "rustc", "cc1", "cc1plus", "ld", "lld", "rust-lld", "collect2", "link",
    ];
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return 0;
    };
    dir.filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter(|e| {
            std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| NAMES.contains(&c.trim()))
        })
        .count()
}

#[derive(Clone, Debug, Default)]
struct Rec {
    read: f64,
    parse: f64,
    submit: f64,
    wake: f64,
    stmts: f64,
    commit: f64,
    ckpt: f64,
    post: f64,
    total: f64,
    n_stmts: u32,
    frames: i32,
    ran: bool,
    n_log: i32,
    n_ckpt: i32,
    ckpt_err: bool,
    /// The checkpointer had a checkpoint in progress when the save started or ended.
    ckpt_overlap: bool,
    segs_before: u32,
    segs_after: u32,
    levels_after: Vec<u32>,
    /// Share of the machine's CPU not used by this process over the cycle ending here.
    foreign_busy: f64,
    running_procs: u32,
}

/// A named phase and how to read it from one save.
type PhaseRow = (&'static str, fn(&Rec) -> f64);

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct Args {
    vault: String,
    iterations: usize,
    gap_ms: u64,
    warmup: usize,
    note: Option<String>,
    autockpt: i32,
    automerge: Option<i32>,
    ckpt_between: bool,
    bare: bool,
    csv: Option<String>,
}

impl Args {
    /// Whether the product's checkpointer runs: not when a diagnostic asks for the #53
    /// behaviour (inline auto-checkpoint, or an explicit checkpoint between saves).
    fn checkpointer(&self) -> bool {
        self.autockpt == 0 && !self.ckpt_between
    }
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        vault: "20k".to_owned(),
        iterations: 1000,
        gap_ms: 1000,
        warmup: 5,
        note: None,
        autockpt: 0,
        automerge: None,
        ckpt_between: false,
        bare: false,
        csv: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut val = || -> Result<String, String> {
            it.next().cloned().ok_or(format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--vault" => a.vault = val()?,
            "--iterations" => a.iterations = val()?.parse().map_err(|e| format!("{arg}: {e}"))?,
            "--gap-ms" => a.gap_ms = val()?.parse().map_err(|e| format!("{arg}: {e}"))?,
            "--warmup" => a.warmup = val()?.parse().map_err(|e| format!("{arg}: {e}"))?,
            "--note" => a.note = Some(val()?),
            "--wal-autocheckpoint" => {
                a.autockpt = val()?.parse().map_err(|e| format!("{arg}: {e}"))?;
            }
            "--fts-automerge" => {
                a.automerge = Some(val()?.parse().map_err(|e| format!("{arg}: {e}"))?);
            }
            "--checkpoint-between" => a.ckpt_between = true,
            "--bare" => a.bare = true,
            "--csv" => a.csv = Some(val()?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if a.iterations == 0 {
        return Err("--iterations must be at least 1".to_owned());
    }
    Ok(a)
}

fn hub_note(db: &Path) -> Result<String, String> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT p.path FROM links l JOIN notes p ON p.id = l.dst_note
          WHERE l.kind IN ('wiki','embed','md')
          GROUP BY l.dst_note ORDER BY count(*) DESC, p.path LIMIT 1",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn run(args: &[String]) -> Result<(), String> {
    let a = parse_args(args)?;
    let vault = ensure_vault(&a.vault)?;
    let index_dir = std::env::temp_dir().join(format!("serat-bench-tail-{}", std::process::id()));
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
        "p1-save-tail: rebuilt {} notes in {:.1} s (untimed setup)",
        vault.notes,
        rebuilt.elapsed().as_secs_f64()
    );
    let result = measure(&a, &index_dir, &vault.path, vault.notes);
    let _ = std::fs::remove_dir_all(&index_dir);
    result
}

fn measure(a: &Args, index_dir: &Path, vault_root: &Path, notes: usize) -> Result<(), String> {
    let db = serat_index::index_path(index_dir);
    let note = match &a.note {
        Some(n) => n.clone(),
        None => hub_note(&db)?,
    };
    let indegree: i64 = {
        let c = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        c.query_row(
            "SELECT count(*) FROM links WHERE kind IN ('wiki','embed','md')
                AND dst_note = (SELECT id FROM notes WHERE path = ?1)",
            [&note],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
    };
    let path = VaultPath::new(&note).map_err(|e| format!("{note:?}: {e}"))?;
    let source = serat_vault::Vault::open(vault_root).map_err(|e| format!("open vault: {e}"))?;
    let parser = serat_parse::CommonMarkParser;

    // The writer connection, built as `IndexHandle::open` builds it, then instrumented.
    // The actor sets the writer's pragmas and starts the checkpointer; the hook below is
    // its observer, so it sees the same commits the checkpointer is signalled for.
    AUTOCKPT.store(a.autockpt, Ordering::Relaxed);
    let conn = serat_index::open(&db).map_err(|e| e.to_string())?;
    serat_index::schema::bootstrap_if_empty(&conn).map_err(|e| e.to_string())?;
    if let Some(n) = a.automerge {
        conn.execute(
            "INSERT INTO blocks_fts(blocks_fts, rank) VALUES('automerge', ?1)",
            [n],
        )
        .map_err(|e| format!("automerge: {e}"))?;
    }
    if !a.bare {
        conn.trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(on_profile));
    } else if !a.checkpointer() {
        // SQLite's own hook at the requested threshold, as before #56 (the actor leaves
        // the pragma alone when the checkpointer is off).
        conn.query_row(
            &format!("PRAGMA wal_autocheckpoint = {}", a.autockpt),
            [],
            |r| r.get::<_, i32>(0),
        )
        .map_err(|e| e.to_string())?;
    }
    let actor = serat_index::IndexActor::with_options(
        conn,
        ActorOptions {
            checkpointer: a.checkpointer(),
            // Bare: no observer. With the checkpointer off and no observer SQLite's own
            // auto-checkpoint stays on, at its default of 1000 frames.
            wal_observer: (!a.bare).then_some(on_wal as serat_index::checkpoint::WalObserver),
            ..ActorOptions::default()
        },
    );
    let writer = actor.handle();
    let health = actor.wal_health();

    // Side connections for the FTS5 structure and the explicit checkpoint; used only in
    // the gap, never inside a timed interval.
    let side = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let ckpt_conn = Connection::open(&db).map_err(|e| e.to_string())?;
    ckpt_conn
        .busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;

    let one_save = |rec: &mut Rec| -> Result<(), String> {
        let ck_at_start = health.checkpoint_in_progress();
        let t0 = Instant::now();
        let loaded: LoadedNote = source.load(&path).map_err(|e| format!("load: {e}"))?;
        let t1 = Instant::now();
        if loaded.meta.kind != NoteKind::Note {
            return Err("the hub is not a note".to_owned());
        }
        let parsed = parser
            .parse(&path, &loaded.bytes)
            .map_err(|e| format!("parse: {e}"))?;
        let t2 = Instant::now();
        let op = IndexOp::UpsertNote {
            meta: loaded.meta,
            parsed: Box::new(parsed),
        };
        writer.submit(op).map_err(|e| e.to_string())?;
        let t3 = Instant::now();
        writer.flush().map_err(|e| e.to_string())?;
        let t4 = Instant::now();
        rec.ckpt_overlap = ck_at_start || health.checkpoint_in_progress();
        rec.read = ms(t1 - t0);
        rec.parse = ms(t2 - t1);
        rec.submit = ms(t3 - t2);
        rec.total = ms(t4 - t0);
        if a.bare {
            rec.post = ms(t4 - t3);
            return Ok(());
        }
        let tx = PROBE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last
            .take()
            .ok_or("no transaction was observed for the save")?;
        rec.wake = ms(tx.begin_end.saturating_duration_since(t3));
        rec.stmts = ms(tx.commit_start.saturating_duration_since(tx.begin_end));
        rec.commit = ms(tx.commit_end.saturating_duration_since(tx.commit_start));
        rec.post = ms(t4.saturating_duration_since(tx.commit_end + tx.wal.dur));
        rec.ckpt = ms(tx.wal.dur);
        rec.n_stmts = tx.stmts;
        rec.frames = tx.wal.frames;
        rec.ran = tx.wal.ran;
        rec.n_log = tx.wal.n_log;
        rec.n_ckpt = tx.wal.n_ckpt;
        rec.ckpt_err = tx.wal.ckpt_err;
        if tx.analyze > 0 {
            return Err("an ANALYZE ran inside a save; the cause list does not cover it".into());
        }
        Ok(())
    };

    let env = budget::env::Environment::detect();
    let (load_start, _) = loadavg();
    let builds_start = build_processes();
    eprintln!(
        "p1-save-tail: {} saves of {note:?} (in-degree {indegree}) at gap {} ms, warm-up {}",
        a.iterations, a.gap_ms, a.warmup
    );

    // Warm-up: untimed, nothing excluded afterwards.
    let mut scratch = Rec::default();
    for _ in 0..a.warmup {
        one_save(&mut scratch)?;
        std::thread::sleep(Duration::from_millis(a.gap_ms.min(200)));
    }
    let mut levels = if a.bare {
        Vec::new()
    } else {
        fts_levels(&side)?
    };
    let wal_path = db.with_extension("db-wal");
    let wal_bytes_start = std::fs::metadata(&wal_path).map_or(0, |m| m.len());

    let mut recs: Vec<Rec> = Vec::with_capacity(a.iterations);
    // A run that dies (the host's disk filled once, at save 903) still reports what it has.
    let mut aborted: Option<String> = None;
    let mut build_samples: Vec<usize> = vec![builds_start];
    let mut jiff = cpu_jiffies();
    let run_started = Instant::now();
    let mut last_flush = Instant::now();
    for i in 0..a.iterations {
        // Cadence: this save starts `gap` after the previous flush() returned.
        let due = last_flush + Duration::from_millis(a.gap_ms);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let mut rec = Rec {
            segs_before: levels.iter().sum(),
            ..Rec::default()
        };
        if let Err(e) = one_save(&mut rec) {
            aborted = Some(format!("save {i} failed: {e}"));
            break;
        }
        last_flush = Instant::now();
        // Everything below is in the gap, outside every timed interval.
        let now_j = cpu_jiffies();
        if let (Some((b0, t0, o0)), Some((b1, t1, o1))) = (jiff, now_j) {
            let total = t1.saturating_sub(t0).max(1);
            let foreign = b1.saturating_sub(b0).saturating_sub(o1.saturating_sub(o0));
            rec.foreign_busy = foreign as f64 / total as f64 * 100.0;
        }
        jiff = now_j;
        rec.running_procs = loadavg().1.saturating_sub(1);
        if i % 10 == 0 {
            build_samples.push(build_processes());
        }
        if !a.bare {
            levels = fts_levels(&side)?;
            rec.segs_after = levels.iter().sum();
            rec.levels_after.clone_from(&levels);
        }
        if a.ckpt_between {
            ckpt_conn
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
                .map_err(|e| e.to_string())?;
        }
        recs.push(rec);
        if i % 100 == 99 {
            eprintln!(
                "p1-save-tail: {} / {} saves, {:.0} s elapsed",
                i + 1,
                a.iterations,
                run_started.elapsed().as_secs_f64()
            );
        }
    }
    let (load_end, _) = loadavg();
    build_samples.push(build_processes());
    let wal_bytes_end = std::fs::metadata(&wal_path).map_or(0, |m| m.len());
    drop(side);
    drop(ckpt_conn);
    let ck_stats = (a.checkpointer() && !a.bare).then(|| {
        (
            health.stats(),
            health.pinned_bytes(),
            run_started.elapsed().as_secs_f64(),
        )
    });
    let shutdown = actor.shutdown().map_err(|e| e.to_string());
    if let Some(why) = &aborted {
        println!(
            "ABORTED after {} of {} saves: {why}",
            recs.len(),
            a.iterations
        );
    } else {
        shutdown?;
    }
    if recs.is_empty() {
        return Err("no save completed".to_owned());
    }

    report(
        a,
        &Subject {
            note: &note,
            indegree,
            notes,
        },
        &env.summary(),
        (&load_start, &load_end),
        &build_samples,
        (wal_bytes_start, wal_bytes_end, ck_stats),
        &recs,
    );
    if let Some(csv) = &a.csv {
        let mut out = String::from(
            "i,total,read,parse,submit,wake,stmts,commit,ckpt,post,n_stmts,frames,ckpt_ran,n_log,n_ckpt,segs_before,segs_after,levels,foreign_busy,running\n",
        );
        for (i, r) in recs.iter().enumerate() {
            let lv = r
                .levels_after
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("/");
            let _ = writeln!(
                out,
                "{i},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{lv},{:.1},{}",
                r.total,
                r.read,
                r.parse,
                r.submit,
                r.wake,
                r.stmts,
                r.commit,
                r.ckpt,
                r.post,
                r.n_stmts,
                r.frames,
                u8::from(r.ran),
                r.n_log,
                r.n_ckpt,
                r.segs_before,
                r.segs_after,
                r.foreign_busy,
                r.running_procs
            );
        }
        std::fs::write(csv, out).map_err(|e| format!("{csv}: {e}"))?;
    }
    Ok(())
}

fn stat_line(name: &str, f: fn(&Rec) -> f64, recs: &[Rec]) -> String {
    let mut s = Samples::new();
    for r in recs {
        s.push(Duration::from_secs_f64(f(r) / 1000.0));
    }
    format!(
        "{name:<24} p50 {:>8.2} · p95 {:>8.2} · max {:>8.2}",
        s.percentile(0.5),
        s.percentile(0.95),
        s.max()
    )
}

fn median_of(recs: &[Rec], f: fn(&Rec) -> f64) -> f64 {
    let mut v: Vec<f64> = recs.iter().map(f).collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

struct Subject<'a> {
    note: &'a str,
    indegree: i64,
    notes: usize,
}

/// The checkpointer's stats, its pinned fact and the run's seconds; `None` when it is off.
type CkReport = Option<(CheckpointStats, Option<u64>, f64)>;

fn report(
    a: &Args,
    subject: &Subject<'_>,
    env: &str,
    (load_start, load_end): (&str, &str),
    builds: &[usize],
    (wal_start, wal_end, ck_stats): (u64, u64, CkReport),
    recs: &[Rec],
) {
    let n = recs.len();
    println!(
        "P1 save tail · {} vault ({} notes) · {n} consecutive saves of {:?} ({} inbound links) · gap {} ms after the previous flush() · warm-up {} · {}{}",
        a.vault,
        subject.notes,
        subject.note,
        subject.indegree,
        a.gap_ms,
        a.warmup,
        if a.bare {
            "BARE (SQLite's own hook, no trace)"
        } else {
            "instrumented (trace_v2 profile + wal_hook observer)"
        },
        if a.gap_ms == 1000 && a.iterations >= 1000 {
            ""
        } else {
            " · NOT the reporting configuration (gap 1000 ms, 1000 saves)"
        },
    );
    println!(
        "settings: checkpointer {} · wal_autocheckpoint {} · fts5 automerge {} · explicit PASSIVE checkpoint between saves: {}",
        if a.checkpointer() {
            "the product's (idle 250 ms, size 4,000 frames)"
        } else {
            "OFF (#53 diagnostic)"
        },
        if a.checkpointer() {
            "0".to_owned()
        } else {
            a.autockpt.to_string()
        },
        a.automerge.map_or("default".to_owned(), |v| v.to_string()),
        a.ckpt_between
    );
    println!("environment: {env}");
    let (mn, mx) = (
        builds.iter().min().copied().unwrap_or(0),
        builds.iter().max().copied().unwrap_or(0),
    );
    let busy = builds.iter().filter(|&&b| b > 0).count();
    println!("machine load average (1/5/15 min): start {load_start} · end {load_end}");
    println!(
        "concurrent_build_processes (rustc, cc1, cc1plus, ld, lld, rust-lld, collect2, link; {} samples, one per 10 saves plus start/end): min {mn} · max {mx} · samples with any: {busy}{}",
        builds.len(),
        if mx == 0 {
            " · attested 0"
        } else {
            " · NOT 0, the run is not quiet"
        }
    );
    println!("wal file at start {wal_start} B · at end {wal_end} B");
    println!();
    println!("per-phase, ms (nearest-rank percentiles over {n} saves):");
    let rows: [PhaseRow; 9] = [
        ("total (apply..flush)", |r| r.total),
        ("read", |r| r.read),
        ("parse", |r| r.parse),
        ("submit", |r| r.submit),
        ("wake (queue + BEGIN)", |r| r.wake),
        ("txn statements", |r| r.stmts),
        ("COMMIT (excl. checkpoint)", |r| r.commit),
        ("checkpoint (wal hook)", |r| r.ckpt),
        ("post-commit", |r| r.post),
    ];
    for (name, f) in rows {
        println!("  {}", stat_line(name, f, recs));
    }
    if let Some((stats, pinned, secs)) = ck_stats {
        let overlapped = recs.iter().filter(|r| r.ckpt_overlap).count();
        println!(
            "checkpointer (its own thread): {} PASSIVE runs · {} incomplete · {} RESTART · longest {:.1} ms · mean {:.1} ms · saves with a checkpoint in progress at their start or end: {overlapped} of {n} · index WAL pinned: {} · {:.1} checkpoints per minute · {:.0} frames per checkpoint (mean) · idle spacing {} s",
            stats.passive_runs,
            stats.incomplete,
            stats.restart_runs,
            stats.passive_max_micros as f64 / 1000.0,
            stats.passive_total_micros as f64 / 1000.0 / stats.passive_runs.max(1) as f64,
            pinned.map_or("no".to_owned(), |b| format!("{b} B")),
            stats.passive_runs as f64 / (secs / 60.0).max(f64::EPSILON),
            stats.frames_checkpointed as f64 / stats.passive_runs.max(1) as f64,
            serat_index::Bounds::default().idle_spacing.as_secs(),
        );
    }
    if a.bare {
        return;
    }
    let ckpts: Vec<usize> = recs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.ran)
        .map(|(i, _)| i)
        .collect();
    let gaps: Vec<usize> = ckpts.windows(2).map(|w| w[1] - w[0]).collect();
    println!();
    println!(
        "statements per transaction: min {} · max {} · median {}",
        recs.iter().map(|r| r.n_stmts).min().unwrap_or(0),
        recs.iter().map(|r| r.n_stmts).max().unwrap_or(0),
        {
            let mut v: Vec<u32> = recs.iter().map(|r| r.n_stmts).collect();
            v.sort_unstable();
            v[v.len() / 2]
        }
    );
    let fr: Vec<i32> = recs.iter().map(|r| r.frames).collect();
    println!(
        "wal_hook frames after commit: min {} · max {} · frames added per save (median step) {}",
        fr.iter().min().unwrap_or(&0),
        fr.iter().max().unwrap_or(&0),
        {
            let mut d: Vec<i32> = fr
                .windows(2)
                .map(|w| w[1] - w[0])
                .filter(|&x| x > 0)
                .collect();
            d.sort_unstable();
            d.get(d.len() / 2).copied().unwrap_or(0)
        }
    );
    println!(
        "checkpoints run by the hook: {} · intervals between them (saves): {gaps:?}",
        ckpts.len(),
    );
    let ck: Vec<&Rec> = recs.iter().filter(|r| r.ran).collect();
    let partial: Vec<usize> = recs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.ran && !r.ckpt_err && r.n_ckpt != r.n_log)
        .map(|(i, _)| i)
        .collect();
    let failed = ck.iter().filter(|r| r.ckpt_err).count();
    println!(
        "  checkpoints with n_ckpt != n_log (a pinned reader held the WAL): {} {:?} · checkpoint calls that errored: {failed}",
        partial.len(),
        &partial[..partial.len().min(20)]
    );
    if let (Some(first), Some(last)) = (ck.first(), ck.last()) {
        let mut d: Vec<f64> = ck.iter().map(|r| r.ckpt).collect();
        d.sort_by(f64::total_cmp);
        println!(
            "  checkpoint frames moved (n_ckpt/n_log): first {}/{} · last {}/{} ; checkpoint ms: p50 {:.2} · max {:.2}",
            first.n_ckpt,
            first.n_log,
            last.n_ckpt,
            last.n_log,
            d[d.len() / 2],
            d[d.len() - 1]
        );
    }
    let falls: Vec<usize> = recs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.segs_after < r.segs_before)
        .map(|(i, _)| i)
        .collect();
    let fall_gaps: Vec<usize> = falls.windows(2).map(|w| w[1] - w[0]).collect();
    println!(
        "FTS5 segments: before first save {} · min after {} · max after {} · saves where the count fell (a merge): {} · intervals {fall_gaps:?}",
        recs.first().map_or(0, |r| r.segs_before),
        recs.iter().map(|r| r.segs_after).min().unwrap_or(0),
        recs.iter().map(|r| r.segs_after).max().unwrap_or(0),
        falls.len(),
    );
    println!(
        "FTS5 segments per level after the last save: {:?}",
        recs.last()
            .map(|r| r.levels_after.clone())
            .unwrap_or_default()
    );
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| recs[y].total.total_cmp(&recs[x].total));
    println!();
    println!(
        "slowest 25 saves (index: total | read parse wake stmts COMMIT ckpt post | wal frames, fts segs before->after, foreign cpu %, runnable):"
    );
    for &i in order.iter().take(25) {
        let r = &recs[i];
        println!(
            "  #{i:<4} {:>7.1} | {:>5.1} {:>5.1} {:>5.1} {:>7.1} {:>7.1} {:>6.1}{} {:>5.1} | {:>4} {}->{} {:>5.1}% {}",
            r.total,
            r.read,
            r.parse,
            r.wake,
            r.stmts,
            r.commit,
            r.ckpt,
            if r.ran { "*" } else { " " },
            r.post,
            r.frames,
            r.segs_before,
            r.segs_after,
            r.foreign_busy,
            r.running_procs
        );
    }
    // For the saves above the run's own p95 total: which phase carries the excess, and
    // what coincided with them.
    let mut totals = Samples::new();
    for r in recs {
        totals.push(Duration::from_secs_f64(r.total / 1000.0));
    }
    let (p50, p95) = (totals.percentile(0.5), totals.percentile(0.95));
    let tail: Vec<&Rec> = recs.iter().filter(|r| r.total > p95).collect();
    println!();
    println!(
        "the {} saves above the run's p95 total ({p95:.1} ms; p50 {p50:.1}): mean excess over the all-saves median, per phase (ms):",
        tail.len()
    );
    if !tail.is_empty() {
        let phases: [PhaseRow; 8] = [
            ("read", |r| r.read),
            ("parse", |r| r.parse),
            ("submit", |r| r.submit),
            ("wake", |r| r.wake),
            ("txn statements", |r| r.stmts),
            ("COMMIT", |r| r.commit),
            ("checkpoint", |r| r.ckpt),
            ("post-commit", |r| r.post),
        ];
        for (name, f) in phases {
            let mean = tail.iter().map(|r| f(r)).sum::<f64>() / tail.len() as f64;
            println!("  {name:<24} {:+8.2}", mean - median_of(recs, f));
        }
        println!(
            "  of those saves: {} had a checkpoint · {} had the FTS5 segment count fall · mean foreign CPU {:.1}% (all saves: {:.1}%)",
            tail.iter().filter(|r| r.ran).count(),
            tail.iter().filter(|r| r.segs_after < r.segs_before).count(),
            tail.iter().map(|r| r.foreign_busy).sum::<f64>() / tail.len() as f64,
            recs.iter().map(|r| r.foreign_busy).sum::<f64>() / n as f64
        );
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("p1-save-tail: {e}");
            ExitCode::FAILURE
        }
    }
}
