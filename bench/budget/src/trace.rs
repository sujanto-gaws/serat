//! The startup trace, and the four ways it is allowed to be rejected.
//!
//! NFR-PRF-01 is measured from a single line of JSON that `serat-app` writes to
//! `SERAT_STARTUP_TRACE` and then exits (task S1). Architecture §3.4 defines the
//! two marks the budgets are asserted against; this module parses the line and
//! refuses to turn a dishonest one into a number.
//!
//! Four rejections, each closing a way a wrong number could look like a right
//! one:
//!
//! 1. **`t0_source` must be `harness`.** t0 is the instant the harness called
//!    spawn, passed in through `SERAT_STARTUP_T0_UNIX_NANOS`, so everything
//!    before `main` — loader, DLLs, webview init — is inside the number. The
//!    app falls back to an `Instant` taken inside `main` when the variable is
//!    absent and says so; that figure undercounts pre-main and is a different
//!    measurement, not a slightly worse one.
//! 2. **`profile` must be `release`.** A budget measured in a debug build is
//!    not a measurement. The mark is emitted in both, so it is exercised
//!    locally; only the recording is refused.
//! 3. **The marks must be monotonic.** `main <= window_created <=
//!    window_visible <= vault_usable`. Out of order means the wall clock moved
//!    under the run, and a sample that survived a clock step is not a sample.
//! 4. **`window_visible - window_created >= 5 ms`.** A mutation detector, not a
//!    performance floor: loading the embedded webview bundle costs far more
//!    than 5 ms, so this cannot fire on an honest run, while a paint mark
//!    rewired to fire at window *creation* collapses the gap to about zero.
//!    That rewiring is the specific failure the whole objection was about —
//!    measure window creation and you pass 400 ms while shipping a blank
//!    rectangle. tauri-app-dev asked that the harness own this threshold, so
//!    it is here rather than in the app.
//!
//! The app has no Rust-side path that stamps the paint mark: it comes only from
//! a double `requestAnimationFrame` after the shell's first render, so a shell
//! that paints nothing never marks and the run times out. A blank rectangle is
//! a red job rather than a fast one.

use std::time::Duration;

/// The gap below which `window_visible` is assumed to be mismeasured.
pub const MIN_PAINT_GAP_MS: f64 = 5.0;

/// One cold start, as the app reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct StartupTrace {
    /// Spawn to the first line of `main`. Reported, not asserted: it is the
    /// pre-main cost, and interesting on its own.
    pub main_ms: f64,
    /// Spawn to the Tauri window being constructed.
    pub window_created_ms: f64,
    /// Spawn to the first composited frame of the shell. **Asserted against
    /// 400 ms.**
    pub window_visible_ms: f64,
    /// Spawn to the file tree rendered and the last-open buffer accepting
    /// keystrokes. **Asserted against 1 s.**
    pub vault_usable_ms: f64,
    /// `harness` or `in_process`.
    pub t0_source: String,
    /// `release` or `debug`.
    pub profile: String,
    /// The vault the app says it opened, so a run against the wrong vault is
    /// visible rather than merely fast.
    pub vault: String,
    /// The note the shell actually rendered, or empty when none was.
    ///
    /// `--exit-after-ready` refuses to start without `--open`, but that guard
    /// covers "no note was configured" and not "the configured note failed to
    /// read". In the second case the shell still becomes usable — the editor
    /// takes keystrokes — and the trace honestly records a null. Honest, and
    /// not a number to record: a `vault usable` figure over a shell that
    /// rendered nothing is not what NFR-PRF-01 is about.
    pub last_open: String,
}

impl StartupTrace {
    /// `window_visible - window_created`.
    pub fn paint_gap_ms(&self) -> f64 {
        self.window_visible_ms - self.window_created_ms
    }

    /// The window-painted mark as a `Duration`.
    pub fn window_visible(&self) -> Duration {
        Duration::from_secs_f64(self.window_visible_ms / 1000.0)
    }

    /// The vault-usable mark as a `Duration`.
    pub fn vault_usable(&self) -> Duration {
        Duration::from_secs_f64(self.vault_usable_ms / 1000.0)
    }

    /// Parse and validate one trace line.
    ///
    /// # Errors
    /// A missing field, or any of the four rejections above. The message names
    /// which, because a cold-start job that fails without saying why is a bug
    /// report nobody can act on.
    pub fn parse(line: &str) -> Result<StartupTrace, String> {
        let num = |key: &str| -> Result<f64, String> {
            number(line, key).ok_or_else(|| format!("the startup trace has no {key}: {line}"))
        };
        let text = |key: &str| -> Result<String, String> {
            string(line, key).ok_or_else(|| format!("the startup trace has no {key}: {line}"))
        };

        let trace = StartupTrace {
            main_ms: num("t0_to_main_ms")?,
            window_created_ms: num("t0_to_window_created_ms")?,
            window_visible_ms: num("t0_to_window_visible_ms")?,
            vault_usable_ms: num("t0_to_vault_usable_ms")?,
            t0_source: text("t0_source")?,
            profile: text("profile")?,
            vault: string(line, "vault").unwrap_or_default(),
            last_open: string(line, "last_open").unwrap_or_default(),
        };
        trace.validate()?;
        Ok(trace)
    }

    /// The four rejections.
    ///
    /// # Errors
    /// As [`StartupTrace::parse`].
    pub fn validate(&self) -> Result<(), String> {
        if self.t0_source != "harness" {
            return Err(format!(
                "t0_source is {:?}, so t0 was taken inside main and everything before it — \
                 loader, DLLs, webview init — is missing from the number. That is a different \
                 measurement, not a slightly worse one. Pass SERAT_STARTUP_T0_UNIX_NANOS.",
                self.t0_source
            ));
        }
        if self.profile != "release" {
            return Err(format!(
                "the trace came from a {:?} build; a budget measured in a debug build is not a \
                 measurement",
                self.profile
            ));
        }
        let marks = [
            ("t0_to_main_ms", self.main_ms),
            ("t0_to_window_created_ms", self.window_created_ms),
            ("t0_to_window_visible_ms", self.window_visible_ms),
            ("t0_to_vault_usable_ms", self.vault_usable_ms),
        ];
        for (name, value) in marks {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("{name} is {value}, which is not a duration"));
            }
        }
        for pair in marks.windows(2) {
            let (earlier, later) = (pair[0], pair[1]);
            if later.1 < earlier.1 {
                return Err(format!(
                    "{} ({:.1} ms) precedes {} ({:.1} ms). The marks are out of order, which \
                     means the clock moved under the run — a sample that survived a clock step \
                     is not a sample.",
                    later.0, later.1, earlier.0, earlier.1
                ));
            }
        }
        if self.last_open.is_empty() {
            return Err(
                "the trace records no last_open, so the shell became usable without rendering \
                 a note. --exit-after-ready refuses when none is configured, so this is the \
                 other case: the note was configured and failed to read. The figure is honest \
                 and is not a gate number."
                    .to_string(),
            );
        }
        if self.paint_gap_ms() < MIN_PAINT_GAP_MS {
            return Err(format!(
                "window_visible is only {:.2} ms after window_created (floor {MIN_PAINT_GAP_MS} \
                 ms). That gap is where compositing the first frame lives, so a value this small \
                 means the paint mark is firing at window creation — which passes 400 ms while \
                 shipping a blank rectangle.",
                self.paint_gap_ms()
            ));
        }
        Ok(())
    }
}

// The trace is one line of JSON written by code in this repository, so a reader
// beats a dependency. Same reasoning as the manifest reader in lib.rs.
fn number(text: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> String {
        r#"{"t0_to_window_visible_ms": 312.4, "t0_to_vault_usable_ms": 803.1,
            "t0_to_main_ms": 41.2, "t0_to_window_created_ms": 96.8,
            "t0_source": "harness", "profile": "release",
            "vault": "bench/vaults/vault-20k", "last_open": "_types/Project.md",
            "notes_walked": null, "schema_version": 1}"#
            .replace('\n', " ")
    }

    #[test]
    fn an_honest_trace_parses() {
        let t = StartupTrace::parse(&good()).expect("valid trace");
        assert_eq!(t.window_visible_ms, 312.4);
        assert_eq!(t.vault_usable_ms, 803.1);
        assert_eq!(t.t0_source, "harness");
        assert_eq!(t.vault, "bench/vaults/vault-20k");
        assert!((t.paint_gap_ms() - 215.6).abs() < 0.001);
        assert_eq!(t.window_visible(), Duration::from_secs_f64(0.3124));
    }

    #[test]
    fn a_trace_that_undercounts_pre_main_is_refused() {
        let line = good().replace("\"harness\"", "\"in_process\"");
        let err = StartupTrace::parse(&line).expect_err("in_process must not pass");
        assert!(err.contains("t0_source"), "{err}");
        assert!(err.contains("before it"), "{err}");
    }

    #[test]
    fn a_debug_build_cannot_produce_a_budget_number() {
        let line = good().replace("\"release\"", "\"debug\"");
        let err = StartupTrace::parse(&line).expect_err("debug must not pass");
        assert!(err.contains("debug build is not a measurement"), "{err}");
    }

    #[test]
    fn a_paint_mark_wired_to_window_creation_is_caught() {
        // The mutation the whole objection was about: measure window creation
        // and you pass 400 ms with a blank rectangle on screen.
        let line = good().replace(
            "\"t0_to_window_visible_ms\": 312.4",
            "\"t0_to_window_visible_ms\": 96.9",
        );
        let err = StartupTrace::parse(&line).expect_err("a collapsed gap must not pass");
        assert!(err.contains("blank rectangle"), "{err}");

        // And the floor does not fire on an honest fast paint.
        let ok = good().replace(
            "\"t0_to_window_visible_ms\": 312.4",
            "\"t0_to_window_visible_ms\": 102.0",
        );
        StartupTrace::parse(&ok).expect("a 5.2 ms gap is above the floor");
    }

    #[test]
    fn marks_out_of_order_mean_the_clock_moved() {
        let line = good().replace(
            "\"t0_to_vault_usable_ms\": 803.1",
            "\"t0_to_vault_usable_ms\": 12.0",
        );
        let err = StartupTrace::parse(&line).expect_err("out-of-order marks must not pass");
        assert!(err.contains("clock moved"), "{err}");

        let negative = good().replace("\"t0_to_main_ms\": 41.2", "\"t0_to_main_ms\": -3.0");
        let err = StartupTrace::parse(&negative).expect_err("a negative mark must not pass");
        assert!(err.contains("not a duration"), "{err}");
    }

    #[test]
    fn a_shell_that_rendered_no_note_is_not_a_gate_number() {
        // --exit-after-ready covers "no note configured"; this covers "the
        // configured note failed to read", where the shell still becomes usable
        // and the trace honestly records a null.
        let line = good().replace("\"last_open\": \"_types/Project.md\", ", "");
        let err = StartupTrace::parse(&line).expect_err("a null last_open must not pass");
        assert!(err.contains("without rendering"), "{err}");
    }

    #[test]
    fn a_missing_field_names_itself() {
        let line = good().replace("\"profile\": \"release\", ", "");
        let err = StartupTrace::parse(&line).expect_err("a missing field must not pass");
        assert!(err.contains("profile"), "{err}");
    }
}
