//! The open-time costs task #14 added · reported, not gated.
//!
//! Serves **FR-VLT-07**, **NFR-REL-01**, **NFR-PRF-01** (the walk is off the critical
//! path but contends with it, F-12); working agreement 4 (a change that adds a walk
//! step or an index read path adds the benchmark that measures it).
//!
//! * **The walk with an ignore file.** `Vault::walk` and `walk_tree_with_metadata` over
//!   the preset vault with no `.seratignore`, a realistic 50-line one, and the largest
//!   one the reader accepts (1,000 lines). The patterns are applied in memory through
//!   `IgnoreRules::set_ignore_file`, so the pinned vault is never written to. None of
//!   the patterns matches a note, so every run walks the same 20,000 files: what moves
//!   is the matching cost per entry.
//! * **The catch-up on an unchanged vault**, `CatchUp::PathsOnly` (a clean shutdown)
//!   and `CatchUp::PathsAndMtime` (an unclean one): one read of `notes`, one pass over
//!   the walk's entries, nothing to submit. The index is built into a temporary
//!   directory from the pinned vault (read only).
//!
//! ```text
//! cargo run --release -p budget --bin p1-catch-up -- --vault 20k --passes 5
//! ```

use std::process::ExitCode;
use std::time::Instant;

use budget::{Samples, ensure_vault};
use serat_index::external::{CatchUp, Reconciler};
use serat_index::{IndexHandle, rebuild};
use serat_parse::CommonMarkParser;
use serat_vault::Vault;

fn line(label: &str, s: &Samples) {
    println!(
        "{label:<44} median {:>8.1} ms   p95 {:>8.1} ms   max {:>8.1} ms   (n={})",
        s.median(),
        s.percentile(0.95),
        s.max(),
        s.len()
    );
}

/// `lines` patterns, none of which matches a generated note.
fn ignore_file(lines: usize) -> String {
    let mut text = String::new();
    for i in 0..lines {
        match i % 5 {
            0 => text.push_str(&format!("/never-{i}/\n")),
            1 => text.push_str(&format!("*.never{i}\n")),
            2 => text.push_str(&format!("**/scratch-{i}/**\n")),
            3 => text.push_str(&format!("!keep-{i}.md\n")),
            _ => text.push_str(&format!("drafts/never-{i}-[a-c]?.md\n")),
        }
    }
    text
}

fn run(args: &[String]) -> Result<(), String> {
    let (mut vault_name, mut passes) = ("20k".to_owned(), 5usize);
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

    let pinned = ensure_vault(&vault_name)?;
    let vault = Vault::open(&pinned.path).map_err(|e| format!("open vault: {e}"))?;
    println!(
        "open-time costs, {vault_name} vault ({} notes), reported not gated",
        pinned.notes
    );

    let baseline = vault.walk().map_err(|e| e.to_string())?.len();
    for (label, lines) in [
        ("no ignore file", 0usize),
        ("50-line ignore file", 50),
        ("1,000-line ignore file (the cap)", 1_000),
    ] {
        let text = ignore_file(lines);
        vault.ignore_rules().set_ignore_file(if lines == 0 {
            None
        } else {
            Some(text.as_bytes())
        });
        let (mut walk, mut tree) = (Samples::new(), Samples::new());
        for _ in 0..passes {
            let started = Instant::now();
            let found = vault.walk().map_err(|e| e.to_string())?.len();
            walk.push(started.elapsed());
            if found != baseline {
                return Err(format!(
                    "REJECTED: {label} changed the walk from {baseline} to {found} files"
                ));
            }
            let started = Instant::now();
            vault
                .walk_tree_with_metadata("", |_, _| {})
                .map_err(|e| e.to_string())?;
            tree.push(started.elapsed());
        }
        line(&format!("Vault::walk, {label}"), &walk);
        line(&format!("walk_tree_with_metadata, {label}"), &tree);
    }
    vault.ignore_rules().set_ignore_file(None);

    let work = std::env::temp_dir().join(format!("serat-p1-catch-up-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let parser = CommonMarkParser::new();
    let started = Instant::now();
    rebuild(&vault, &parser, vault.root(), &work).map_err(|e| format!("rebuild: {e}"))?;
    eprintln!("rebuilt in {:.1} s", started.elapsed().as_secs_f64());
    let handle = IndexHandle::open(&work).map_err(|e| e.to_string())?;
    let reconciler = Reconciler::new(&handle);
    let entries = vault.walk().map_err(|e| e.to_string())?;
    for (label, mode) in [
        ("catch_up PathsOnly (clean shutdown)", CatchUp::PathsOnly),
        (
            "catch_up PathsAndMtime (unclean shutdown)",
            CatchUp::PathsAndMtime,
        ),
    ] {
        let mut samples = Samples::new();
        for _ in 0..passes {
            let started = Instant::now();
            let outcome = reconciler
                .catch_up(&vault, &parser, &entries, mode)
                .map_err(|e| e.to_string())?;
            samples.push(started.elapsed());
            if outcome.changed() {
                return Err(format!("REJECTED: {label} changed an unchanged index"));
            }
        }
        line(label, &samples);
    }
    let _ = handle.close();
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}
