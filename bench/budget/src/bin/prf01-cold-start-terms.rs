//! Task #100's own instrument: ADR-0014's seven-term partition of cold start,
//! at median and p95, plus `S_main`/`S_front` and the unguarded paint-evidence
//! mark reported alongside them.
//!
//! Not a gate job -- it asserts nothing and always exits 0 on a clean run.
//! `prf01-cold-start` (the gate binary) still owns the pass/fail verdict on
//! `window_visible`/`vault_usable`; this exists because that binary reduces
//! straight to those two scalars and discards everything ADR-0014 §Decision 1
//! partitions them into, which task #100 was asked to report in full. Reuses
//! `subject::cold_start_traces` -- the same guarded batch, the same
//! `--exit-after-ready` spawn, the same six-rejection validation in
//! `budget::trace` -- so this is a second read of the same data the gate job
//! takes, not a second measurement method.
//!
//! ```text
//! cargo run --release -p budget --bin prf01-cold-start-terms
//! ```

use std::process::ExitCode;

use budget::{Samples, ensure_vault, median_duration};
use std::time::Duration;

fn pct(xs: &[f64], q: f64) -> f64 {
    let mut s = Samples::new();
    for &v in xs {
        s.push(Duration::from_secs_f64(v.max(0.0) / 1000.0));
    }
    s.percentile(q)
}

fn med(xs: &[f64]) -> f64 {
    let ds: Vec<Duration> = xs
        .iter()
        .map(|v| Duration::from_secs_f64(v.max(0.0) / 1000.0))
        .collect();
    median_duration(ds).as_secs_f64() * 1000.0
}

fn report_line(name: &str, xs: &[f64]) {
    println!(
        "  {name:<12} median {:>9.1} ms   p95 {:>9.1} ms",
        med(xs),
        pct(xs, 0.95)
    );
}

fn main() -> ExitCode {
    let vault = match ensure_vault("20k") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("prf01-cold-start-terms: {e}");
            return ExitCode::FAILURE;
        }
    };

    let iterations = 20;
    let traces = match budget::subject::cold_start_traces(&vault.path, iterations) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("prf01-cold-start-terms: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut p_load = vec![];
    let mut s_pre = vec![];
    let mut p_builder = vec![];
    let mut s_setup = vec![];
    let mut p_webview = vec![];
    let mut s_shell = vec![];
    let mut s_settle = vec![];
    let mut s_main = vec![];
    let mut s_front = vec![];
    let mut s_total = vec![];
    let mut p_total = vec![];
    let mut window_visible = vec![];
    let mut vault_usable = vec![];
    let mut paint_evidence: Vec<f64> = vec![];
    let mut paint_evidence_missing = 0usize;
    let mut frame_to_paint_gap: Vec<f64> = vec![];
    let mut raw_lines = Vec::with_capacity(traces.len());

    for (run, t) in traces.iter().enumerate() {
        let terms = t.terms();
        p_load.push(terms.p_load_ms);
        s_pre.push(terms.s_pre_ms);
        p_builder.push(terms.p_builder_ms);
        s_setup.push(terms.s_setup_ms);
        p_webview.push(terms.p_webview_ms);
        s_shell.push(terms.s_shell_ms);
        s_settle.push(terms.s_settle_ms);
        s_main.push(terms.s_main_ms());
        s_front.push(terms.s_front_ms());
        s_total.push(terms.serat_term_ms());
        p_total.push(terms.platform_term_ms());
        window_visible.push(t.window_visible_ms);
        vault_usable.push(t.vault_usable_ms);
        match t.paint_evidence_ms {
            Some(v) => paint_evidence.push(v),
            None => paint_evidence_missing += 1,
        }
        if let Some(gap) = t.frame_to_paint_gap_ms() {
            frame_to_paint_gap.push(gap);
        }
        raw_lines.push(format!(
            "{{\"run\":{run},\"p_load_ms\":{:.1},\"s_pre_ms\":{:.1},\"p_builder_ms\":{:.1},\
             \"s_setup_ms\":{:.1},\"p_webview_ms\":{:.1},\"s_shell_ms\":{:.1},\
             \"s_settle_ms\":{:.1},\"s_main_ms\":{:.1},\"s_front_ms\":{:.1},\
             \"serat_term_ms\":{:.1},\"platform_term_ms\":{:.1},\
             \"window_visible_ms\":{:.1},\"vault_usable_ms\":{:.1},\
             \"paint_evidence_ms\":{},\"raf2_ms\":{},\"frame_to_paint_gap_ms\":{}}}",
            terms.p_load_ms,
            terms.s_pre_ms,
            terms.p_builder_ms,
            terms.s_setup_ms,
            terms.p_webview_ms,
            terms.s_shell_ms,
            terms.s_settle_ms,
            terms.s_main_ms(),
            terms.s_front_ms(),
            terms.serat_term_ms(),
            terms.platform_term_ms(),
            t.window_visible_ms,
            t.vault_usable_ms,
            t.paint_evidence_ms
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "null".to_string()),
            t.raf2_ms
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "null".to_string()),
            t.frame_to_paint_gap_ms()
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "null".to_string()),
        ));
    }

    println!(
        "prf01-cold-start-terms -- {} runs against {}",
        traces.len(),
        vault.path.display()
    );
    println!("ADR-0014 seven-term partition (t0 -> vault_usable):");
    report_line("P_load", &p_load);
    report_line("S_pre", &s_pre);
    report_line("P_builder", &p_builder);
    report_line("S_setup", &s_setup);
    report_line("P_webview", &p_webview);
    report_line("S_shell", &s_shell);
    report_line("S_settle", &s_settle);
    println!("Sub-terms (ADR-0014 Decision 4):");
    report_line("S_main", &s_main);
    report_line("S_front", &s_front);
    println!("Headline totals (ADR-0014 Decision 3):");
    report_line("S (Serat)", &s_total);
    report_line("P (platform)", &p_total);
    println!("Asserted marks (gate job's own scalars, for cross-check):");
    report_line("visible", &window_visible);
    report_line("usable", &vault_usable);
    println!(
        "paint_evidence_ms (F-397: unguarded here -- mark_first_paint's call site is not tested \
         against it by this harness):"
    );
    if paint_evidence.is_empty() {
        println!("  none of {} runs carried a value", traces.len());
    } else {
        report_line("evidence", &paint_evidence);
        println!(
            "  present on {}/{} runs, missing on {}",
            paint_evidence.len(),
            traces.len(),
            paint_evidence_missing
        );
    }
    println!(
        "frame_to_paint_gap_ms = paint_evidence_ms - raf2_ms (F-411: reported, not asserted -- \
         no threshold exists that separates an honest run from the attack F-388 describes \
         without more samples than this run alone can provide):"
    );
    if frame_to_paint_gap.is_empty() {
        println!(
            "  none of {} runs carried both raf2_ms and paint_evidence_ms",
            traces.len()
        );
    } else {
        report_line("gap", &frame_to_paint_gap);
        println!(
            "  present on {}/{} runs",
            frame_to_paint_gap.len(),
            traces.len()
        );
    }

    let out_path = std::env::temp_dir().join("prf01-cold-start-terms.jsonl");
    let body = raw_lines.join("\n") + "\n";
    match std::fs::write(&out_path, &body) {
        Ok(()) => println!("  raw per-run terms  {}", out_path.display()),
        Err(e) => eprintln!(
            "prf01-cold-start-terms: could not write {}: {e}",
            out_path.display()
        ),
    }

    ExitCode::SUCCESS
}
