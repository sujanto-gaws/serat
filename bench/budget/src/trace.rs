//! The startup trace, and the ways it is allowed to be rejected.
//!
//! NFR-PRF-01 is measured from a single line of JSON that `serat-app` writes to
//! `SERAT_STARTUP_TRACE` and then exits (task S1). Architecture §3.4 defines the
//! two marks the budgets are asserted against; this module parses the line and
//! refuses to turn a dishonest one into a number.
//!
//! Six rejections, each closing a way a wrong number could look like a right
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
//! 3. **All seven of ADR-0014's marks are required, not optional.** `main`,
//!    `state_ready`, `runtime_ready`, `setup_done`, `window_created`,
//!    `window_visible`, `vault_usable` — every one is parsed with `?`, the same
//!    way `t0_source` and `profile` always were. F-381: the application already
//!    carried the middle three as `Option<f64>` and nothing required them, so a
//!    mark that stopped being emitted failed no test — it just made the Serat
//!    term silently uncomputable. Required here is what ADR-0014 §Consequences
//!    asks for: "a trace missing any of the seven marks is a rejection, not an
//!    `Option`."
//! 4. **The marks must be monotonic, all seven of them, in partition order.**
//!    `main <= state_ready <= runtime_ready <= setup_done <= window_created <=
//!    window_visible <= vault_usable`. Out of order means the wall clock moved
//!    under the run, and a sample that survived a clock step is not a sample.
//! 5. **`window_visible - window_created >= 5 ms`.** A mutation detector, not a
//!    performance floor: loading the embedded webview bundle costs far more
//!    than 5 ms, so this cannot fire on an honest run, while a paint mark
//!    rewired to fire at window *creation* collapses the gap to about zero.
//!    That rewiring is the specific failure the whole objection was about —
//!    measure window creation and you pass 400 ms while shipping a blank
//!    rectangle. tauri-app-dev asked that the harness own this threshold, so
//!    it is here rather than in the app.
//! 6. **ADR-0014's sum identity: `Σ(terms) = t0_to_vault_usable_ms`.** [`Terms`]
//!    computes the seven intervals the ADR names — three platform, four Serat —
//!    from consecutive mark differences, and [`StartupTrace::validate`] checks
//!    they sum to `vault_usable_ms` within floating-point tolerance. Given
//!    rejection 4 (monotonic) and rejection 3 (all seven present), this
//!    identity is a **tautology of how the terms are defined** — a telescoping
//!    sum of consecutive differences always equals the last minus the first,
//!    for any seven numbers at all, ordered or not. It therefore cannot fail on
//!    real data; what it guards is this module's own arithmetic, so a future
//!    edit that mislabels which field feeds which term, or that adds an eighth
//!    term without updating the sum, fails loudly instead of silently. Read
//!    ADR-0014 §Decision 1 before assuming this check does more than that.
//!
//! **What this module still cannot catch, and why, stated because ADR-0014
//! §Consequences asks for it to be stated rather than implied.** F-377 found a
//! mutation — `mark_first_paint` hoisted to script start — that survives every
//! rejection above. F-386 proves why rejection 6 structurally cannot be
//! extended to catch it: `Startup::vault_usable()` takes the *max* of four
//! candidates and `window_visible` does not settle that maximum (§3.4 measures
//! `painted → usable` at 4.7–7.5 ms), so moving the paint mark earlier by Δ
//! shrinks `S_shell` by Δ and grows `S_settle` by exactly Δ — every sum stays
//! invariant, by construction, for any interior redistribution between two
//! adjacent terms. [`terms_redistribution_between_adjacent_marks_is_invisible_
//! to_the_sum_identity`] demonstrates this directly rather than leaving it
//! asserted in a comment. Catching it needs semantic evidence the frontend
//! itself emits — task #108's post-composite paint evidence — which this
//! module does not yet consume.
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
    /// Spawn to settings being loaded and `AppState` built — ADR-0014's
    /// `S_pre` boundary.
    pub state_ready_ms: f64,
    /// Spawn to the Tauri runtime being up and `setup` entered — ADR-0014's
    /// `P_builder` boundary.
    pub runtime_ready_ms: f64,
    /// Spawn to our work inside `setup` finishing, just before the window is
    /// built — ADR-0014's `S_setup` boundary.
    pub setup_done_ms: f64,
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
    /// Empty on every cold-start run the harness itself takes: task #52 ruled
    /// that `t_usable` is §3.4's four preconditions -- index state known, file
    /// tree delivered, watcher settled, window painted -- and none of them is
    /// "a note is rendered", so the harness no longer passes `--open` at all
    /// (task #54). Recorded rather than dropped because a user session that
    /// did restore a note still reports it here, and because a non-empty value
    /// on a harness run would itself be worth noticing.
    pub last_open: String,
    /// `first-contentful-paint`, as `app/src/startup.ts` reads it via
    /// `PerformanceObserver` and reports beside the two `requestAnimationFrame`
    /// marks (task #108, task #114). **Not required and not validated by
    /// [`StartupTrace::validate`]** -- F-397: nothing yet asserts that
    /// `mark_first_paint`'s call site passes this rather than a placeholder.
    /// `raf1_ms`/`raf2_ms` now do reach a Rust parameter (`commands::mark_first_paint`
    /// -> `startup::validate_paint_evidence`, closing F-403's original gap) and are
    /// checked there against each other and against this value, but neither crosses
    /// back into the trace this harness reads -- see [`StartupTrace::raf2_ms`].
    /// Carried here so a caller can report the value the app claims, explicitly
    /// labelled as unguarded, rather than the harness silently discarding evidence it
    /// receives.
    pub paint_evidence_ms: Option<f64>,
    /// The second `requestAnimationFrame` callback's own timestamp, on the same
    /// browser navigation-origin clock as [`StartupTrace::paint_evidence_ms`] --
    /// F-411. **Not required, not validated, and not compared against any
    /// `t0_to_*` field** (those are on the harness's `t0` clock; this and
    /// `paint_evidence_ms` are not). Its only use here is
    /// [`StartupTrace::frame_to_paint_gap_ms`], a diagnostic with no assertion on
    /// it: architect's ruling on F-411 was that a real rejection needs a
    /// separating threshold this project does not yet have the samples to set
    /// (honest gap ~51-65 ms, attack bound ~184 ms, against F-405's ~19%
    /// unexplained batch spread), so building one now would be an eleventh
    /// guard-that-cannot-fire rather than a working check. This field exists to
    /// grow the distribution a future reading could set a real threshold from.
    /// `None` on any trace taken before the field existed, or if the app never
    /// sent it.
    pub raf2_ms: Option<f64>,
    /// `true` when the build renders a file tree and says so (`"tree": true`, task #141,
    /// ADR-0015 section 5). `false` on a trace from a build that predates the field, which
    /// parses exactly as before.
    pub tree: bool,
    /// Spawn to the tree's first screen (root listing applied and painted, ADR-0015
    /// section 3): `t_usable`'s tree term. **Present if and only if `tree` is true**: a
    /// declared tree with no first screen is an incomplete trace and [`StartupTrace::parse`]
    /// refuses it, so a build that stops reporting the tree fails the budget job rather
    /// than passing a number that no longer waits for it.
    pub tree_first_screen_ms: Option<f64>,
}

impl StartupTrace {
    /// `window_visible - window_created`.
    pub fn paint_gap_ms(&self) -> f64 {
        self.window_visible_ms - self.window_created_ms
    }

    /// `paint_evidence_ms - raf2_ms` -- F-411's diagnostic, reported and never
    /// asserted. Both operands are on the browser's own navigation-origin clock
    /// (see their doc comments), so the difference is meaningful without any
    /// cross-clock reconciliation to `t0`, unlike every `t0_to_*` field on this
    /// struct. `None` when either input is absent, which is the normal case on
    /// any trace taken before `raf2_ms` was wired through (or against a build
    /// that does not send it).
    #[must_use]
    pub fn frame_to_paint_gap_ms(&self) -> Option<f64> {
        Some(self.paint_evidence_ms? - self.raf2_ms?)
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
    /// A missing field, or any of the six rejections above. The message names
    /// which, because a cold-start job that fails without saying why is a bug
    /// report nobody can act on.
    pub fn parse(line: &str) -> Result<StartupTrace, String> {
        Self::parse_with(line, false)
    }

    /// As [`StartupTrace::parse`], for the **current** build: a trace with no `tree` field
    /// is refused rather than read as an older build. Without this a P1 build that stopped
    /// emitting `tree` would pass the cold-start job with a `t_usable` that never waited
    /// for the tree (task #141). The cold-start budget job calls this; `parse` stays
    /// lenient so archived traces from older builds still read.
    ///
    /// # Errors
    /// As [`StartupTrace::parse`], plus a trace that does not declare `"tree": true`.
    pub fn parse_requiring_tree(line: &str) -> Result<StartupTrace, String> {
        Self::parse_with(line, true)
    }

    fn parse_with(line: &str, require_tree: bool) -> Result<StartupTrace, String> {
        let num = |key: &str| -> Result<f64, String> {
            number(line, key).ok_or_else(|| format!("the startup trace has no {key}: {line}"))
        };
        let text = |key: &str| -> Result<String, String> {
            string(line, key).ok_or_else(|| format!("the startup trace has no {key}: {line}"))
        };

        // `"tree":true` (compact) or `"tree": true`; absent on older builds.
        let tree = bool_true(line, "tree");
        let tree_first_screen_ms = number(line, "t0_to_tree_first_screen_ms");
        if require_tree && !tree {
            return Err(format!(
                "the startup trace does not declare a tree (no \"tree\":true) but this build is \
                 required to: t_usable may not have waited for the file tree (ADR-0015 section \
                 5), so the run is refused rather than read as an older build: {line}"
            ));
        }
        if tree && tree_first_screen_ms.is_none() {
            return Err(format!(
                "the startup trace declares a tree but has no t0_to_tree_first_screen_ms: the \
                 trace is incomplete (ADR-0015 section 5), so vault_usable cannot be trusted \
                 to have waited for the tree: {line}"
            ));
        }
        let trace = StartupTrace {
            main_ms: num("t0_to_main_ms")?,
            state_ready_ms: num("t0_to_state_ready_ms")?,
            runtime_ready_ms: num("t0_to_runtime_ready_ms")?,
            setup_done_ms: num("t0_to_setup_done_ms")?,
            window_created_ms: num("t0_to_window_created_ms")?,
            window_visible_ms: num("t0_to_window_visible_ms")?,
            vault_usable_ms: num("t0_to_vault_usable_ms")?,
            t0_source: text("t0_source")?,
            profile: text("profile")?,
            vault: string(line, "vault").unwrap_or_default(),
            last_open: string(line, "last_open").unwrap_or_default(),
            paint_evidence_ms: number(line, "paint_evidence_ms"),
            raf2_ms: number(line, "raf2_ms"),
            tree,
            tree_first_screen_ms,
        };
        trace.validate()?;
        Ok(trace)
    }

    /// ADR-0014's seven marks, in partition order, as `(name, value)` pairs.
    /// The single place both the monotonicity check and [`Terms::from_marks`]
    /// read from, so the two can never disagree about what the seven are.
    fn marks(&self) -> [(&'static str, f64); 7] {
        [
            ("t0_to_main_ms", self.main_ms),
            ("t0_to_state_ready_ms", self.state_ready_ms),
            ("t0_to_runtime_ready_ms", self.runtime_ready_ms),
            ("t0_to_setup_done_ms", self.setup_done_ms),
            ("t0_to_window_created_ms", self.window_created_ms),
            ("t0_to_window_visible_ms", self.window_visible_ms),
            ("t0_to_vault_usable_ms", self.vault_usable_ms),
        ]
    }

    /// ADR-0014's seven-term partition, computed from consecutive mark
    /// differences.
    #[must_use]
    pub fn terms(&self) -> Terms {
        Terms::from_marks(self.marks())
    }

    /// The six rejections.
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
        let marks = self.marks();
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
        // ADR-0014's sum identity. A tautology of how Terms::from_marks computes its
        // seven fields (module doc, rejection 6) -- what this actually guards is that
        // computation matching this validator's own field list, not the application's
        // honesty. 1 microsecond of tolerance for JSON round-trip and summation-order
        // floating-point error, which is generous against durations reported to 0.1 ms.
        let terms = Terms::from_marks(marks);
        let sum = terms.sum_ms();
        if (sum - self.vault_usable_ms).abs() > 0.001 {
            return Err(format!(
                "the seven terms sum to {sum:.4} ms but t0_to_vault_usable_ms is \
                 {:.4} ms -- ADR-0014 Decision 1's partition does not hold for this trace",
                self.vault_usable_ms
            ));
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
        // Task #141: `vault_usable` is the latest of paint, tree first screen, index
        // settled and watcher, so the tree mark can never be later than it. A trace
        // where it is was not produced by an app that waited on the tree.
        if let Some(tree_ms) = self.tree_first_screen_ms {
            if !tree_ms.is_finite() || tree_ms < 0.0 {
                return Err(format!(
                    "t0_to_tree_first_screen_ms is {tree_ms}, which is not a duration"
                ));
            }
            if tree_ms > self.vault_usable_ms + 0.001 {
                return Err(format!(
                    "t0_to_tree_first_screen_ms ({tree_ms:.1} ms) is after t0_to_vault_usable_ms \
                     ({:.1} ms), but usable is the later of the tree and the other \
                     preconditions (ADR-0015 section 5): this app did not wait for the tree.",
                    self.vault_usable_ms
                ));
            }
        }
        Ok(())
    }
}

/// ADR-0014 Decision 1's seven-term partition of `t0 → vault_usable`: three
/// platform terms and four Serat terms, each the interval between two
/// consecutive marks. `P + S = t0_to_vault_usable_ms` exactly (module doc,
/// rejection 6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Terms {
    /// `t0 → main`. Platform: loader, DLLs, runtime init.
    pub p_load_ms: f64,
    /// `main → state_ready`. Serat: arguments, settings TOML, `AppState`.
    pub s_pre_ms: f64,
    /// `state_ready → runtime_ready`. Platform: `tauri::Builder` construction.
    /// Watched rather than budgeted (ADR-0014 §Decision 2's caveat).
    pub p_builder_ms: f64,
    /// `runtime_ready → setup_done`. Serat: event sink, `open_vault`, thread
    /// spawns, boot info.
    pub s_setup_ms: f64,
    /// `setup_done → window_created`. Platform: `WebviewWindowBuilder::build`.
    pub p_webview_ms: f64,
    /// `window_created → window_visible`. Serat: our bundle in their
    /// renderer, to the first composited frame. Same value as
    /// [`StartupTrace::paint_gap_ms`].
    pub s_shell_ms: f64,
    /// `window_visible → vault_usable`. Serat: the remaining preconditions of
    /// `t_usable`.
    pub s_settle_ms: f64,
}

impl Terms {
    /// Six consecutive differences over the seven marks, in the order
    /// [`StartupTrace::marks`] produces them -- which is ADR-0014's partition
    /// order, so this has no ordering decision left to make of its own.
    fn from_marks(marks: [(&'static str, f64); 7]) -> Terms {
        let v: Vec<f64> = marks.iter().map(|(_, value)| *value).collect();
        Terms {
            p_load_ms: v[0],
            s_pre_ms: v[1] - v[0],
            p_builder_ms: v[2] - v[1],
            s_setup_ms: v[3] - v[2],
            p_webview_ms: v[4] - v[3],
            s_shell_ms: v[5] - v[4],
            s_settle_ms: v[6] - v[5],
        }
    }

    /// ADR-0014 §Decision 1's `Σ(terms)`, which the identity says must equal
    /// `t0_to_vault_usable_ms`.
    #[must_use]
    pub fn sum_ms(&self) -> f64 {
        self.p_load_ms
            + self.s_pre_ms
            + self.p_builder_ms
            + self.s_setup_ms
            + self.p_webview_ms
            + self.s_shell_ms
            + self.s_settle_ms
    }

    /// `P = P_load + P_builder + P_webview`. Recorded with every figure and
    /// carries no budget (ADR-0014 §Decision 3): no change in this repository
    /// moves it.
    #[must_use]
    pub fn platform_term_ms(&self) -> f64 {
        self.p_load_ms + self.p_builder_ms + self.p_webview_ms
    }

    /// `S = S_pre + S_setup + S_shell + S_settle`. The binding quantity: what
    /// C-06 enforces for NFR-PRF-01 from the re-baseline forward (ADR-0014
    /// §Decision 3).
    #[must_use]
    pub fn serat_term_ms(&self) -> f64 {
        self.s_pre_ms + self.s_setup_ms + self.s_shell_ms + self.s_settle_ms
    }

    /// `S_main = S_pre + S_setup`. Rust, main thread -- ADR-0014 §Decision 4's
    /// first sub-term. Matches `Trace::serat_main_thread_ms()` on the
    /// application side (`crates/serat-app/src/startup.rs`); nothing enforces
    /// the two agree, which is a gap for whoever wires this into #101's
    /// report.
    #[must_use]
    pub fn s_main_ms(&self) -> f64 {
        self.s_pre_ms + self.s_setup_ms
    }

    /// `S_front = S_shell + S_settle`. Our front end in their renderer --
    /// ADR-0014 §Decision 4's second sub-term, and where the editor lands.
    #[must_use]
    pub fn s_front_ms(&self) -> f64 {
        self.s_shell_ms + self.s_settle_ms
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

/// Whether `"key":true` appears (any whitespace after the colon). Absent or `false` is `false`.
fn bool_true(text: &str, key: &str) -> bool {
    let needle = format!("\"{key}\":");
    text.find(&needle)
        .is_some_and(|i| text[i + needle.len()..].trim_start().starts_with("true"))
}

fn string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}

/// As [`number`], for a flat JSON array of numbers: `"key":[1.0,2.5,3.0]` or
/// `"key":[]`. Assumes no nested arrays or objects inside it, which holds for
/// every array `serde_json::to_string` produces from a `Vec<f64>` — the only
/// shape [`BenchmarkExitReport`]'s (`crates/serat-app/src/lib.rs`) `_ms`
/// fields ever are.
fn number_array(text: &str, key: &str) -> Option<Vec<f64>> {
    let needle = format!("\"{key}\":[");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find(']')? + start;
    let body = text[start..end].trim();
    if body.is_empty() {
        return Some(Vec::new());
    }
    body.split(',').map(|tok| tok.trim().parse().ok()).collect()
}

/// As [`bool_array`], for `"key":["idle","no"]` (plain words, no escapes). `Ok(None)`
/// when the key is absent.
fn string_array(text: &str, key: &str) -> Result<Option<Vec<String>>, String> {
    let needle = format!("\"{key}\":[");
    let Some(at) = text.find(&needle) else {
        return Ok(None);
    };
    let start = at + needle.len();
    let end = text[start..]
        .find(']')
        .ok_or_else(|| format!("the save benchmark report's {key} is not closed"))?
        + start;
    let body = text[start..end].trim();
    if body.is_empty() {
        return Ok(Some(Vec::new()));
    }
    body.split(',')
        .map(|tok| {
            tok.trim()
                .strip_prefix('"')
                .and_then(|t| t.strip_suffix('"'))
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!("the save benchmark report's {key} has an entry that is not a string: {tok:?}")
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// As [`number_array`], for `"key":[true,false]`. `Ok(None)` when the key is absent;
/// an entry that is not a boolean is an error naming the key, never a silent absence.
fn bool_array(text: &str, key: &str) -> Result<Option<Vec<bool>>, String> {
    let needle = format!("\"{key}\":[");
    let Some(at) = text.find(&needle) else {
        return Ok(None);
    };
    let start = at + needle.len();
    let end = text[start..]
        .find(']')
        .ok_or_else(|| format!("the save benchmark report's {key} is not closed"))?
        + start;
    let body = text[start..end].trim();
    if body.is_empty() {
        return Ok(Some(Vec::new()));
    }
    body.split(',')
        .map(|tok| match tok.trim() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!(
                "the save benchmark report's {key} has an entry that is not a boolean: {other:?}"
            )),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// One `--bench-save-list` run, as `serat-app` reported it (task #111's
/// `BenchmarkExitReport`, task #94's reader). Parsed from the same exit-report
/// line [`StartupTrace`] reads the seven marks from — `BenchmarkExitReport`
/// (`crates/serat-app/src/lib.rs`) flattens the startup trace and adds these
/// fields beside it — but kept as a
/// separate type rather than folded into `StartupTrace`, deliberately: a
/// `--bench-save-list` run's startup marks are real and worth validating on
/// their own terms, but the six startup rejections have nothing to say about
/// whether the save path degraded, and coupling the two would fail an
/// NFR-PRF-03 run for an NFR-PRF-01 reason, or the reverse.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveBenchmarkTrace {
    /// How many paths `--bench-save-list` named — the denominator task #94's
    /// completeness check is against, not just a symptom count.
    pub attempted: usize,
    /// Elapsed ms of every save whose outcome was `submitted`. **The series
    /// NFR-PRF-03's p95 is asserted against.**
    pub submitted_ms: Vec<f64>,
    /// Attempted saves that found no index handle. F-382's own legitimate
    /// race is a narrow window at `open_vault`; a hard zero is expected over
    /// a run this long — `bench/budget`'s [`crate::check_save_completeness`]
    /// is what enforces that, not this parser.
    pub handle_missing: u64,
    /// Attempted saves the writer refused — ADR-0011's sanctioned
    /// backpressure. May legitimately be non-zero.
    pub writer_refused: u64,
    /// Elapsed ms of every refused save, reported beside `submitted_ms` and
    /// excluded from it (F-19, F-382).
    pub writer_refused_ms: Vec<f64>,
    /// Per submitted save, parallel to `submitted_ms` (task #196): a WAL checkpoint
    /// overlapped the save. Empty when the application did not report it (an older
    /// binary, or an index without a `WalHealth`): absence, not "false" everywhere.
    pub checkpoint_overlap: Vec<bool>,
    /// Per submitted save, parallel to `checkpoint_overlap` (task #199): `idle`,
    /// `size-bound`, `no`, or `yes` (overlapped, kind not seen). Empty when not reported.
    pub checkpoint_kind: Vec<String>,
    /// Per submitted save: an FTS5 merge commit landed inside the save's window.
    pub fts_merge: Vec<bool>,
    /// Per submitted save: a writer turn of 50 ms or more overlapped the window.
    pub slow_commit: Vec<bool>,
    /// Saves whose FTS5 segment probe failed (counted as no merge).
    pub flag_probe_failures: u64,
    /// Idle checkpoints deferred to work in flight, over the run.
    pub checkpointer_deferred_busy: u64,
    /// PASSIVE plus RESTART checkpoint runs, over the run.
    pub checkpointer_runs: u64,
    /// Idle wake-ups the frame floor skipped, over the run (task #199).
    pub checkpointer_floor_skips: u64,
}

impl SaveBenchmarkTrace {
    /// Parse one exit-report line's save-benchmark fields.
    ///
    /// Does not require or validate `StartupTrace`'s seven marks — see this
    /// type's own doc comment for why the two are kept apart.
    ///
    /// # Errors
    /// Any of the five fields is missing or is not the shape it claims —
    /// naming the field, the same discipline [`StartupTrace::parse`] follows.
    pub fn parse(line: &str) -> Result<SaveBenchmarkTrace, String> {
        let count = |key: &str| -> Result<usize, String> {
            number(line, key)
                .map(|n| n as usize)
                .ok_or_else(|| format!("the save benchmark report has no {key}: {line}"))
        };
        let counter = |key: &str| -> Result<u64, String> {
            number(line, key)
                .map(|n| n as u64)
                .ok_or_else(|| format!("the save benchmark report has no {key}: {line}"))
        };
        let array = |key: &str| -> Result<Vec<f64>, String> {
            number_array(line, key)
                .ok_or_else(|| format!("the save benchmark report has no {key}: {line}"))
        };
        let submitted = array("save_submitted_ms")?;
        let tags = |key: &str| -> Result<Vec<bool>, String> {
            let Some(flags) = bool_array(line, key)? else {
                return Ok(Vec::new());
            };
            if flags.len() == submitted.len() {
                Ok(flags)
            } else {
                Err(format!(
                    "the save benchmark report's {key} has {} entries for {} submitted saves",
                    flags.len(),
                    submitted.len()
                ))
            }
        };
        let kinds = match string_array(line, "save_checkpoint_kind")? {
            Some(kinds) if kinds.len() != submitted.len() => {
                return Err(format!(
                    "the save benchmark report's save_checkpoint_kind has {} entries for {} submitted saves",
                    kinds.len(),
                    submitted.len()
                ));
            }
            Some(kinds) => kinds,
            None => Vec::new(),
        };
        Ok(SaveBenchmarkTrace {
            attempted: count("save_attempted")?,
            submitted_ms: submitted.clone(),
            handle_missing: counter("save_handle_missing")?,
            writer_refused: counter("save_writer_refused")?,
            writer_refused_ms: array("save_writer_refused_ms")?,
            // The tags are diagnostic and optional: a report without them still
            // gates. A tag array of the wrong length is not: it would pair samples
            // with the wrong flags, so it is refused by name.
            checkpoint_overlap: tags("save_checkpoint_overlap")?,
            checkpoint_kind: kinds,
            fts_merge: tags("save_fts_merge")?,
            slow_commit: tags("save_slow_commit")?,
            flag_probe_failures: number(line, "save_flag_probe_failures").map_or(0, |n| n as u64),
            checkpointer_deferred_busy: number(line, "save_checkpointer_deferred_busy")
                .map_or(0, |n| n as u64),
            checkpointer_runs: number(line, "save_checkpointer_runs").map_or(0, |n| n as u64),
            checkpointer_floor_skips: number(line, "save_checkpointer_floor_skips")
                .map_or(0, |n| n as u64),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> String {
        r#"{"t0_to_window_visible_ms": 312.4, "t0_to_vault_usable_ms": 803.1,
            "t0_to_main_ms": 41.2, "t0_to_state_ready_ms": 45.0,
            "t0_to_runtime_ready_ms": 60.0, "t0_to_setup_done_ms": 90.0,
            "t0_to_window_created_ms": 96.8,
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

    // ---------------------------------------------------------------- F-411

    #[test]
    fn raf2_ms_and_the_frame_to_paint_gap_are_absent_when_the_app_never_sent_them() {
        // good() carries neither field, which is every trace taken before this task
        // and every build that predates the app-side wiring test-engineer asked
        // tauri-app-dev for. Absence must not be an error -- both are diagnostic.
        let t = StartupTrace::parse(&good()).expect("valid trace");
        assert_eq!(t.paint_evidence_ms, None);
        assert_eq!(t.raf2_ms, None);
        assert_eq!(t.frame_to_paint_gap_ms(), None);
    }

    #[test]
    fn the_frame_to_paint_gap_is_reported_once_both_operands_are_present() {
        let line = good().replace(
            "\"schema_version\": 1}",
            "\"paint_evidence_ms\": 184.0, \"raf2_ms\": 120.6, \"schema_version\": 1}",
        );
        let t = StartupTrace::parse(&line).expect("valid trace");
        assert_eq!(t.paint_evidence_ms, Some(184.0));
        assert_eq!(t.raf2_ms, Some(120.6));
        assert!((t.frame_to_paint_gap_ms().expect("both present") - 63.4).abs() < 0.001);
    }

    #[test]
    fn the_frame_to_paint_gap_is_absent_when_only_one_operand_is_present() {
        // paint_evidence_ms alone (task #100's actual traces, taken before raf2_ms
        // existed) must not synthesise a gap from a missing second operand.
        let line = good().replace(
            "\"schema_version\": 1}",
            "\"paint_evidence_ms\": 184.0, \"schema_version\": 1}",
        );
        let t = StartupTrace::parse(&line).expect("valid trace");
        assert_eq!(t.paint_evidence_ms, Some(184.0));
        assert_eq!(t.raf2_ms, None);
        assert_eq!(t.frame_to_paint_gap_ms(), None);
    }

    #[test]
    fn raf2_ms_is_not_required_and_does_not_gate_validation() {
        // F-411 is explicit: no assertion. A trace with every required mark but no
        // raf2_ms must still validate -- the whole point is that this is a column,
        // not a seventh rejection alongside the existing six.
        let line = good().replace(
            "\"schema_version\": 1}",
            "\"paint_evidence_ms\": 184.0, \"schema_version\": 1}",
        );
        StartupTrace::parse(&line)
            .expect("raf2_ms absence must not reject an otherwise-valid trace");
    }

    // ---------------------------------------------------------------- #141 / NFR-PRF-01

    fn with_tree(tree_fields: &str) -> String {
        good().replace(
            "\"schema_version\": 1}",
            &format!("{tree_fields} \"schema_version\": 1}}"),
        )
    }

    #[test]
    fn a_trace_without_a_tree_field_parses_as_before() {
        // Older builds: no `tree`, no first-screen mark. Not an error, and not a tree.
        let t = StartupTrace::parse(&good()).expect("valid trace");
        assert!(!t.tree);
        assert_eq!(t.tree_first_screen_ms, None);
    }

    #[test]
    fn a_tree_less_trace_fails_for_the_current_build_and_still_parses_for_an_old_one() {
        // The cold-start job parses with `parse_requiring_tree`. Mutant, run by hand:
        // make `parse_requiring_tree` call `parse_with(line, false)` and the error
        // assertion fails, which is the P1 build that stopped emitting `tree`.
        let err = StartupTrace::parse_requiring_tree(&good()).expect_err("no tree declared");
        assert!(err.contains("does not declare a tree"), "{err}");
        let declared = with_tree("\"tree\":true, \"t0_to_tree_first_screen_ms\": 790.5,");
        StartupTrace::parse_requiring_tree(&declared).expect("a declared tree passes");
        // `"tree":false` is not a declaration either.
        StartupTrace::parse_requiring_tree(&with_tree("\"tree\":false,"))
            .expect_err("false is not a tree");
        // Without the requirement the same tree-less line still reads.
        StartupTrace::parse(&good()).expect("an older build's trace parses");
    }

    #[test]
    fn a_declared_tree_carries_its_first_screen_number() {
        // Both spellings: the app writes compact JSON, hand-built fixtures spaced.
        for tree in ["\"tree\":true,", "\"tree\": true,"] {
            let line = with_tree(&format!("{tree} \"t0_to_tree_first_screen_ms\": 790.5,"));
            let t = StartupTrace::parse(&line).expect("valid trace");
            assert!(t.tree, "{tree}");
            assert_eq!(t.tree_first_screen_ms, Some(790.5));
        }
    }

    #[test]
    fn a_declared_tree_with_no_first_screen_is_an_incomplete_trace_not_a_pass() {
        // Mutant, run by hand: delete the `tree && is_none` check in `parse` and this
        // fails, because the trace then parses and the budget job would pass a usable
        // number that never waited on the tree.
        let err = StartupTrace::parse(&with_tree("\"tree\":true,")).expect_err("incomplete");
        assert!(err.contains("t0_to_tree_first_screen_ms"), "{err}");
        assert!(err.contains("incomplete"), "{err}");
        // `"tree":false` declares nothing, so it does not demand the field.
        StartupTrace::parse(&with_tree("\"tree\":false,")).expect("no tree declared");
    }

    #[test]
    fn a_real_trace_from_the_p1_build_parses_with_its_tree_term_binding() {
        // Captured 2026-11-12 from `serat --vault <9-note vault> --exit-after-ready` (release
        // build of 3972bf0) under Xvfb with software rendering, so the times are slow and
        // mean nothing; the *shape* is what this pins: the real app's compact JSON, `tree`
        // true, and the tree's first screen the binding term of usable.
        let line = r#"{"t0_to_window_visible_ms":8632.471092,"paint_evidence_ms":499.0,"raf2_ms":1586.0,"t0_to_vault_usable_ms":8712.396634,"t0_to_main_ms":1871.213577,"t0_to_state_ready_ms":1871.278607,"t0_to_runtime_ready_ms":2320.076207,"t0_to_setup_done_ms":2320.493865,"t0_to_window_created_ms":5450.86045,"t0_to_buffer_rendered_ms":8643.078711,"tree":true,"t0_to_tree_first_screen_ms":8712.396634,"t0_to_index_settled_ms":2321.006057,"t0_to_watcher_ms":2323.555551,"watcher":"registered","t0_to_walk_complete_ms":8632.774893,"t0_source":"harness","profile":"release","schema_version":12,"notes_walked":9,"vault":"v","last_open":null}"#;
        let t = StartupTrace::parse(line).expect("the real app's trace parses");
        assert!(t.tree);
        assert_eq!(t.tree_first_screen_ms, Some(8712.396634));
        assert_eq!(t.tree_first_screen_ms, Some(t.vault_usable_ms));
    }

    #[test]
    fn a_first_screen_after_usable_is_rejected_because_usable_is_the_latest_term() {
        let line = with_tree("\"tree\":true, \"t0_to_tree_first_screen_ms\": 900.0,");
        let err = StartupTrace::parse(&line).expect_err("803.1 usable < 900 tree");
        assert!(err.contains("did not wait for the tree"), "{err}");
        // Equal is the normal case (the tree was the last term).
        let line = with_tree("\"tree\":true, \"t0_to_tree_first_screen_ms\": 803.1,");
        StartupTrace::parse(&line).expect("tree is the last term");
    }

    // ---------------------------------------------------------------- #106 / F-381

    #[test]
    fn all_seven_marks_are_required_not_optional() {
        // ADR-0014 §Consequences: "a trace missing any of the seven marks is a
        // rejection, not an Option." The application side still carries the middle
        // three as Option<f64> (F-381) -- this is the harness half of that fix, and
        // it must name each one the same way a_missing_field_names_itself already
        // does for profile.
        for (field, needle) in [
            ("t0_to_state_ready_ms", "\"t0_to_state_ready_ms\": 45.0, "),
            (
                "t0_to_runtime_ready_ms",
                "\"t0_to_runtime_ready_ms\": 60.0, ",
            ),
            ("t0_to_setup_done_ms", "\"t0_to_setup_done_ms\": 90.0, "),
        ] {
            let line = good().replace(needle, "");
            let err = StartupTrace::parse(&line)
                .expect_err(&format!("a trace missing {field} must not pass"));
            assert!(err.contains(field), "{field}: {err}");
        }
    }

    #[test]
    fn the_seven_terms_are_computed_in_adr_0014_partition_order_and_sum_exactly() {
        // Pinned against hand arithmetic rather than only against vault_usable_ms,
        // so a bug that permuted two terms but left the sum invariant (which
        // telescoping makes easy to do by accident) would still be caught.
        let t = StartupTrace::parse(&good()).expect("valid trace");
        let terms = t.terms();
        // .abs() < 0.001 throughout rather than assert_eq!: these are subtractions
        // of decimal literals that do not all round-trip exactly through f64 (45.0
        // - 41.2 is 3.799999999999997, not 3.8), which is the same reason the sum
        // identity itself carries a tolerance rather than exact equality.
        assert!((terms.p_load_ms - 41.2).abs() < 0.001, "t0 -> main");
        assert!((terms.s_pre_ms - 3.8).abs() < 0.001, "main -> state_ready");
        assert!(
            (terms.p_builder_ms - 15.0).abs() < 0.001,
            "state_ready -> runtime_ready"
        );
        assert!(
            (terms.s_setup_ms - 30.0).abs() < 0.001,
            "runtime_ready -> setup_done"
        );
        assert!(
            (terms.p_webview_ms - 6.8).abs() < 0.001,
            "setup_done -> window_created"
        );
        assert!(
            (terms.s_shell_ms - 215.6).abs() < 0.001,
            "window_created -> window_visible"
        );
        assert!(
            (terms.s_settle_ms - 490.7).abs() < 0.001,
            "window_visible -> vault_usable"
        );
        assert!(
            (terms.sum_ms() - t.vault_usable_ms).abs() < 0.001,
            "ADR-0014 Decision 1: Sigma(terms) must equal t0_to_vault_usable_ms"
        );
        assert!(
            (terms.platform_term_ms() - 63.0).abs() < 0.001,
            "P = P_load + P_builder + P_webview"
        );
        assert!(
            (terms.serat_term_ms() - 740.1).abs() < 0.001,
            "S = S_pre + S_setup + S_shell + S_settle"
        );
        assert!(
            (terms.s_main_ms() - 33.8).abs() < 0.001,
            "S_main = S_pre + S_setup"
        );
        assert!(
            (terms.s_front_ms() - 706.3).abs() < 0.001,
            "S_front = S_shell + S_settle"
        );
        // The partition is exhaustive: platform + Serat must also equal the total,
        // independent of how sum_ms() itself is computed.
        assert!(
            (terms.platform_term_ms() + terms.serat_term_ms() - t.vault_usable_ms).abs() < 0.001
        );
    }

    #[test]
    fn terms_redistribution_between_adjacent_marks_is_invisible_to_the_sum_identity() {
        // F-386, demonstrated rather than only asserted in a doc comment. Moving
        // window_visible earlier by Delta is F-377's surviving mutation --
        // mark_first_paint hoisted to script start -- and ADR-0014 §Consequences
        // proves the sum identity structurally cannot catch it: Startup::vault_
        // usable() is painted.max(rendered).max(index).max(watcher), and painted
        // does not settle that maximum, so t_usable does not move. S_shell shrinks
        // by Delta and S_settle grows by exactly Delta; every sum stays invariant.
        //
        // This trace keeps window_created (96.8), main and the three new marks
        // fixed, and moves only window_visible from 312.4 to 146.8 -- a 50 ms gap
        // above window_created, comfortably clearing rejection 5's 5 ms floor (that
        // floor exists for a *different* mutation, F-377's mutation A: a single
        // rAF instead of a double, which collapses the gap to near zero) so this
        // test is isolated to what rejection 6 can and cannot see. vault_usable
        // (803.1) is untouched, exactly as F-386 says a real mutation would leave
        // it. If the sum identity could catch this mutation, this parse would fail
        // and the test below would need inverting -- it does not, which is the
        // point.
        let mutated = good().replace(
            "\"t0_to_window_visible_ms\": 312.4",
            "\"t0_to_window_visible_ms\": 146.8",
        );
        let t = StartupTrace::parse(&mutated)
            .expect("the sum identity cannot see an interior redistribution (F-386)");
        let terms = t.terms();
        assert!(
            (terms.s_shell_ms - 50.0).abs() < 0.001,
            "S_shell shrank by Delta"
        );
        assert!(
            (terms.s_settle_ms - 656.3).abs() < 0.001,
            "S_settle grew by exactly Delta -- the anomaly F-386 says is a diagnostic, not \
             a rejection"
        );
        assert!(
            (terms.sum_ms() - t.vault_usable_ms).abs() < 0.001,
            "the identity held throughout, which is F-386's whole claim"
        );
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
    fn an_empty_last_open_is_the_normal_case_now_and_parses_fine() {
        // Task #52: no note is a precondition for "vault usable", so the
        // harness no longer passes --open at all (task #54) and every real
        // trace it takes has an empty last_open. An earlier version of this
        // check rejected that as "rendered nothing" -- which would have failed
        // every cold-start run once the harness stopped opening a note.
        let line = good().replace("\"last_open\": \"_types/Project.md\", ", "");
        let trace = StartupTrace::parse(&line).expect("an empty last_open is not a failure");
        assert_eq!(trace.last_open, "");
    }

    #[test]
    fn a_missing_field_names_itself() {
        let line = good().replace("\"profile\": \"release\", ", "");
        let err = StartupTrace::parse(&line).expect_err("a missing field must not pass");
        assert!(err.contains("profile"), "{err}");
    }

    // ---------------------------------------------------------------- #94

    fn save_report(
        attempted: usize,
        submitted_ms: &str,
        handle_missing: u64,
        writer_refused: u64,
        writer_refused_ms: &str,
    ) -> String {
        // The real shape serde_json::to_string produces (`BenchmarkExitReport`,
        // crates/serat-app/src/lib.rs): no spaces, exercised here rather than the
        // spaced style `good()` uses above, so this test also stands as evidence
        // the reader tolerates compact JSON and not only the hand-spaced fixture
        // style `StartupTrace`'s own tests use.
        format!(
            "{{\"save_attempted\":{attempted},\"save_submitted_ms\":[{submitted_ms}],\
             \"save_handle_missing\":{handle_missing},\"save_writer_refused\":{writer_refused},\
             \"save_writer_refused_ms\":[{writer_refused_ms}]}}"
        )
    }

    #[test]
    fn an_honest_save_report_parses() {
        let line = save_report(3, "12.5,8.3,15.0", 0, 0, "");
        let trace = SaveBenchmarkTrace::parse(&line).expect("valid save report");
        assert_eq!(trace.attempted, 3);
        assert_eq!(trace.submitted_ms, vec![12.5, 8.3, 15.0]);
        assert_eq!(trace.handle_missing, 0);
        assert_eq!(trace.writer_refused, 0);
        assert!(trace.writer_refused_ms.is_empty());
    }

    #[test]
    fn a_report_with_a_refusal_parses_both_series_apart() {
        let line = save_report(4, "12.5,8.3,15.0", 0, 1, "0.4");
        let trace = SaveBenchmarkTrace::parse(&line).expect("valid save report");
        assert_eq!(trace.attempted, 4);
        assert_eq!(trace.submitted_ms, vec![12.5, 8.3, 15.0]);
        assert_eq!(trace.writer_refused, 1);
        assert_eq!(trace.writer_refused_ms, vec![0.4]);
    }

    #[test]
    fn an_untouched_run_reports_true_zeros_not_absence() {
        // BenchmarkExitReport's own doc: a run over an untouched vault reports
        // every field 0, not the absence of a measurement -- matched here.
        let line = save_report(0, "", 0, 0, "");
        let trace = SaveBenchmarkTrace::parse(&line).expect("an all-zero report is valid");
        assert_eq!(trace.attempted, 0);
        assert!(trace.submitted_ms.is_empty());
        assert_eq!(trace.handle_missing, 0);
    }

    #[test]
    fn the_per_save_tags_parse_when_present_and_are_absent_otherwise() {
        // Task #196, NFR-PRF-03.
        let plain = save_report(3, "12.5,8.3,15.0", 0, 0, "");
        let none = SaveBenchmarkTrace::parse(&plain).expect("untagged report");
        assert!(none.checkpoint_overlap.is_empty() && none.fts_merge.is_empty());
        let tagged = plain.replace(
            "\"save_writer_refused_ms\":[]",
            "\"save_writer_refused_ms\":[],\"save_checkpoint_overlap\":[false,true,false],\
             \"save_fts_merge\":[false,false,true],\"save_slow_commit\":[false,false,true],\
             \"save_flag_probe_failures\":2,\"save_checkpointer_deferred_busy\":5,\
             \"save_checkpointer_runs\":7,\"save_checkpointer_floor_skips\":3,\
             \"save_checkpoint_kind\":[\"no\",\"idle\",\"no\"]",
        );
        let t = SaveBenchmarkTrace::parse(&tagged).expect("tagged report");
        assert_eq!(t.checkpoint_overlap, vec![false, true, false]);
        assert_eq!(t.checkpoint_kind, vec!["no", "idle", "no"]);
        assert_eq!(t.checkpointer_floor_skips, 3);
        assert!(
            none.checkpoint_kind.is_empty(),
            "an older binary reports no kinds"
        );
        assert_eq!(t.fts_merge, vec![false, false, true]);
        assert_eq!(t.slow_commit, vec![false, false, true]);
        assert_eq!(
            (
                t.flag_probe_failures,
                t.checkpointer_deferred_busy,
                t.checkpointer_runs
            ),
            (2, 5, 7)
        );
    }

    #[test]
    fn a_tag_array_that_does_not_match_the_samples_is_refused_by_name() {
        // Pairing samples with the wrong flags would invent a finding.
        let line = save_report(3, "12.5,8.3,15.0", 0, 0, "").replace(
            "\"save_writer_refused_ms\":[]",
            "\"save_writer_refused_ms\":[],\"save_fts_merge\":[true]",
        );
        let err = SaveBenchmarkTrace::parse(&line).expect_err("a short tag array");
        assert!(err.contains("save_fts_merge"), "{err}");
        let err = bool_array("{\"x\":[true,maybe]}", "x").expect_err("a malformed entry");
        assert!(err.contains('x') && err.contains("maybe"), "{err}");
        assert_eq!(bool_array("{}", "x"), Ok(None));
        let line = save_report(3, "12.5,8.3,15.0", 0, 0, "").replace(
            "\"save_writer_refused_ms\":[]",
            "\"save_writer_refused_ms\":[],\"save_checkpoint_kind\":[\"idle\"]",
        );
        let err = SaveBenchmarkTrace::parse(&line).expect_err("a short kind array");
        assert!(err.contains("save_checkpoint_kind"), "{err}");
        assert_eq!(string_array("{}", "x"), Ok(None));
        assert!(string_array("{\"x\":[idle]}", "x").is_err());
    }

    #[test]
    fn each_missing_field_names_itself() {
        for key in [
            "save_attempted",
            "save_submitted_ms",
            "save_handle_missing",
            "save_writer_refused",
            "save_writer_refused_ms",
        ] {
            let line = save_report(1, "1.0", 0, 0, "").replace(&format!("\"{key}\":"), "\"x\":");
            let err = SaveBenchmarkTrace::parse(&line)
                .expect_err(&format!("a report missing {key} must not pass"));
            assert!(err.contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn number_array_parses_the_shapes_serde_json_actually_produces() {
        assert_eq!(number_array("{\"xs\":[]}", "xs"), Some(Vec::new()));
        assert_eq!(number_array("{\"xs\":[1.0]}", "xs"), Some(vec![1.0]));
        assert_eq!(
            number_array("{\"xs\":[1.0,2.5,3.0]}", "xs"),
            Some(vec![1.0, 2.5, 3.0])
        );
        // Trailing content after the array must not confuse the closing bracket
        // search -- a real report has fields on both sides of every array.
        assert_eq!(
            number_array("{\"a\":1,\"xs\":[4.0,5.0],\"b\":2}", "xs"),
            Some(vec![4.0, 5.0])
        );
        assert_eq!(number_array("{}", "xs"), None);
    }
}
