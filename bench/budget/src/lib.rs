//! The budget harness: one asserting job per performance budget.
//!
//! **Serves NFR-PRF-08** ("every performance budget shall be asserted in
//! continuous integration; a regression shall fail the build"), **US-35** and
//! **C-06**. The budgets themselves are NFR-PRF-01, NFR-PRF-03 and NFR-PRF-04
//! in P0; NFR-PRF-05 and NFR-PRF-09 join in P2.
//!
//! Three rules are encoded here rather than left to discipline:
//!
//! 1. **A measurement carries its conditions.** Every result records the vault's
//!    tree digest, the generator's format version and the machine profile. A
//!    number without those is not comparable with another number.
//! 2. **The vault is verified before it is measured.** The harness recomputes
//!    the tree digest and refuses to run against a vault that does not match its
//!    manifest, because a stale or hand-edited vault produces a number that
//!    looks fine and means nothing.
//! 3. **A threshold is never weakened to make a build pass.** If a budget is
//!    wrong it is re-baselined in the PRD with the measurement attached (task
//!    Q1), not edited here.

pub mod env;
pub mod gate;
pub mod subject;
pub mod trace;

use env::Environment;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which statistic the requirement's acceptance criterion names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Statistic {
    /// The 95th percentile, nearest-rank. NFR-PRF-01 and NFR-PRF-03.
    P95,
    /// The median wall-clock figure across runs. NFR-PRF-04's acceptance
    /// criterion says "wall-clock within budget" without naming a count; one run
    /// of a thirty-second operation on a laptop is a coin toss, so the harness
    /// takes three and reports the middle one.
    WallClock,
}

impl Statistic {
    /// The machine-readable name, for the result JSON.
    pub fn label(self) -> &'static str {
        match self {
            Statistic::P95 => "p95",
            Statistic::WallClock => "wall_clock_median",
        }
    }

    /// What a reader of the gate report needs to see, including the sample
    /// count, because "wall clock = 27431 ms" over three runs reads as one run.
    /// That exact line goes into the charter gate report (F-25).
    pub fn describe(self, n: usize) -> String {
        match self {
            Statistic::P95 => format!("p95 over {n} runs"),
            Statistic::WallClock => format!("median of {n} wall-clock runs"),
        }
    }
}

/// Which machine the threshold applies to.
///
/// The product budget belongs to the reference machine (see
/// `docs/05-testing/reference-machine.md`). A CI runner is not that machine and
/// never will be, so CI asserts a *regression ceiling*: a separate number whose
/// job is to catch a change that made things worse, not to certify the product.
/// Keeping them in one column would mean either a green CI that proves nothing
/// or a red CI that everyone learns to ignore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// The reference machine. Asserts the number in the PRD.
    Reference,
    /// A CI runner. Asserts the regression ceiling.
    Ci,
}

impl Profile {
    /// Parse `--profile`.
    pub fn parse(s: &str) -> Result<Profile, String> {
        match s {
            "reference" | "ref" => Ok(Profile::Reference),
            "ci" => Ok(Profile::Ci),
            other => Err(format!("unknown profile {other}; expected reference or ci")),
        }
    }

    /// How it prints.
    pub fn label(self) -> &'static str {
        match self {
            Profile::Reference => "reference",
            Profile::Ci => "ci",
        }
    }
}

/// Whether a `--profile ci` run asserts a budget on a runner that cannot produce a gate
/// number (ADR-0019, task #166, NFR-PRF-04, NFR-PRF-08, C-06, RISK-012).
///
/// C-06 says a budget is enforced in CI. A hosted `windows-latest` runner is 2 cores with
/// the real-time scanner off in every one of the 36 runs on record, so
/// [`env::Environment::gate_eligible`] is false there by construction and the figure it
/// produces is printed `NOT A GATE NUMBER`. For a budget whose product limit is the
/// reference machine's number and which has no regression ceiling (`ci_limit_ms: None`),
/// asserting that limit on such a runner asserts nothing about the product: 36 runs of
/// `prf04-full-rebuild` read medians of 15.9 s to 131.8 s (a code step at #56 explains
/// most of that), and one run's own three samples, same binary, same minutes, spanned
/// 20.6 s to 60.1 s (docs/05-testing/nfr-prf-04-gate-quality.md, "Task #166"). Failing a
/// PR on that is a red nobody can act on, and a green would not mean the budget held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiAssertion {
    /// `--profile ci` asserts whatever the environment says. The default, and the only
    /// value for a budget that has a `ci_limit_ms` regression ceiling.
    Always,
    /// `--profile ci` asserts only when the environment is gate eligible; on any other
    /// runner the figure is measured, printed and written as `REPORT-ONLY` and cannot
    /// fail the build. `--profile reference` is unaffected: a person chose that machine,
    /// so it always asserts. The assertion lives on the reference machine, and the job
    /// name says so (pinned by `a_report_only_budget_says_so_in_its_job_name`).
    WhenGateEligible,
}

/// One budget from NFR-PRF.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Stable job id, e.g. `prf04-full-rebuild`.
    pub id: &'static str,
    /// The requirement it asserts.
    pub requirement: &'static str,
    /// The requirement's own words, so a failure message says what is wrong
    /// rather than only that something is.
    pub statement: &'static str,
    /// The statistic the acceptance criterion names.
    pub statistic: Statistic,
    /// Iterations the acceptance criterion names.
    pub iterations: usize,
    /// The limit on the reference machine, in milliseconds. This is the number
    /// in the PRD.
    pub product_limit_ms: f64,
    /// The regression ceiling in CI, in milliseconds.
    ///
    /// `None` until the first CI run exists to set it from. Until then the CI
    /// job asserts the product limit, which will be red on a slower runner —
    /// deliberately, because a placeholder ceiling invented at a desk is worse
    /// than an honest red.
    pub ci_limit_ms: Option<f64>,
    /// Which preset vault it is measured against.
    pub vault: &'static str,
    /// **Exactly which interval is timed.**
    ///
    /// Named on the budget rather than left to the job, because P0 finding F-03
    /// showed that "save→index" has two defensible readings that differ by more
    /// than the budget itself: end-to-end from the file being written (which
    /// includes the 150 ms per-path coalescing window of architecture §3.1
    /// step 1, and so cannot fit 50 ms by construction) and the pipeline's own
    /// cost from the event being dequeued. A number reported without saying
    /// which one it is is not a measurement, it is a rumour.
    pub interval: &'static str,
    /// Whether a breach of this budget fails the build (NFR-PRF-08, C-06), or
    /// whether the figure is measured and printed and gates nothing.
    ///
    /// **Task #118, F-443, F-407.** The PRD is the source of truth for what is
    /// gated, and PRD v1.2 gates only the Serat term for NFR-PRF-01; the two
    /// pre-split aggregates are reported. That disagreement used to live in
    /// the difference between a doc and a job, which is exactly how F-443 was
    /// found. It is a field now, so the two cannot drift again without a line
    /// of code changing, and [`P0_BUDGETS`] is checked against it.
    ///
    /// A `false` here is **not** a weakened budget. The limit stays on the
    /// struct and stays in the report, because the pre-split figures are still
    /// the ones a reader compares a cold start against; what is removed is the
    /// assertion, and only because F-407 measured the aggregate moving ~19%
    /// batch-to-batch, which cannot decide a pass at any threshold.
    pub gated: bool,
    /// Whether a `--profile ci` run on a runner that is not gate eligible asserts this
    /// budget or only reports it (task #166). See [`CiAssertion`].
    pub ci_assertion: CiAssertion,
}

impl Budget {
    /// The limit that applies under `profile`.
    pub fn limit_ms(&self, profile: Profile) -> f64 {
        match profile {
            Profile::Reference => self.product_limit_ms,
            Profile::Ci => self.ci_limit_ms.unwrap_or(self.product_limit_ms),
        }
    }
}

/// NFR-PRF-01. Two marks on one timeline, so it is two budgets sharing a job.
pub const PRF01_WINDOW_VISIBLE: Budget = Budget {
    id: "prf01-window-visible",
    requirement: "NFR-PRF-01",
    statement: "The application shall present an interactive window within 400ms",
    statistic: Statistic::P95,
    iterations: 20,
    product_limit_ms: 400.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "process spawn to the first painted frame",
    gated: false,
    ci_assertion: CiAssertion::Always,
};

/// NFR-PRF-01, the second mark.
pub const PRF01_VAULT_USABLE: Budget = Budget {
    id: "prf01-vault-usable",
    requirement: "NFR-PRF-01",
    statement: "…and a fully usable vault within 1 second, at 20,000 notes with a warm index",
    statistic: Statistic::P95,
    iterations: 20,
    product_limit_ms: 1_000.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "process spawn to the vault-usable mark, as task D2 defines it",
    gated: false,
    ci_assertion: CiAssertion::Always,
};

/// NFR-PRF-01, re-baselined (task #104, ADR-0014 §Decision 3): the binding
/// quantity from the split forward is the Serat term, not either of the two
/// legacy marks above. [`measure_serat_term`] keeps `PRF01_WINDOW_VISIBLE` and
/// `PRF01_VAULT_USABLE`'s intervals alive as reported, unasserted series on
/// this same measurement — F-407 found the aggregate moves ~19%
/// batch-to-batch against ~8% for the Serat term, so it cannot decide a pass
/// at any budget and gating it manufactures a red job that eventually gets
/// disabled, taking the honest reporting with it.
///
/// **Task #107 is additive only.** `prf01-cold-start` still asserts the two
/// legacy marks above; demoting them to reported-only is task #118, tracked
/// separately because F-443 found it is a real gap between what the PRD says
/// is gated and what CI currently gates, and because whether §3.4's
/// scale-check moves to this job or stays with the legacy one is architect's
/// call, not bench/'s.
pub const PRF01_SERAT_TERM: Budget = Budget {
    id: "prf01-serat-term",
    requirement: "NFR-PRF-01",
    statement: "Of the interval from process start to an interactive window and a fully usable \
                vault, the portion Serat's own code is responsible for -- main-thread setup \
                plus the front end's mount, first composited frame and settle -- shall not \
                exceed 350ms at p95",
    statistic: Statistic::P95,
    iterations: 20,
    product_limit_ms: 350.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "S_pre + S_setup + S_shell + S_settle (ADR-0014 Decision 1's seven-mark \
               partition, Decision 3's binding sum) -- the platform term and the two legacy \
               marks are reported beside it, never asserted (F-407)",
    gated: true,
    // Task #190, ADR-0019 point 2 (recorded in ci-windows-first-run-2026-11.md, "Run 427"):
    // hosted windows-latest is not gate eligible (scanner off, 4 cores here, 2 elsewhere),
    // one hang-free run of 20 launches read p95 546.3 ms against 350 with samples 237.5 to
    // 711.6 ms (spread 199.6%), and no `ci_limit_ms` can be derived from one run. No Linux
    // twin exists either (`NO_LINUX_TWIN`): the 350 ms is bounded to Windows/WebView2 (PRD
    // v1.2, F-96) and RM-2's Linux figure was 1,310.4 ms. The assertion is the reference
    // machine's (`--profile reference`); this job still fails when a launch hangs, the app is
    // absent or the frame gate refuses past its retries, because that is a measurement that
    // could not be taken. The frame guard and the product limit are unchanged.
    ci_assertion: CiAssertion::WhenGateEligible,
};

/// NFR-PRF-03.
pub const PRF03_SAVE_TO_INDEX: Budget = Budget {
    id: "prf03-save-to-index",
    requirement: "NFR-PRF-03",
    statement: "A single note save shall be reflected in the index within 50ms at p95",
    statistic: Statistic::P95,
    iterations: 1_000,
    product_limit_ms: 50.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "AppState::write_file's call into the index, timed from the outside -- Serat's \
               own save path, no watcher, no debounce (architecture §3.1.1; \
               see check_save_completeness)",
    gated: true,
    ci_assertion: CiAssertion::Always,
};

/// NFR-PRF-04.
pub const PRF04_FULL_REBUILD: Budget = Budget {
    id: "prf04-full-rebuild",
    requirement: "NFR-PRF-04",
    statement: "A full index rebuild of 20,000 Markdown notes shall complete within 30 seconds",
    statistic: Statistic::WallClock,
    iterations: 3,
    product_limit_ms: 30_000.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "entry to rebuild() until it returns, writer drained and ANALYZE run",
    gated: true,
    // Task #166: hosted windows-latest is never gate eligible, and one run's three
    // samples spanned 20.6 s to 60.1 s; the assertion is the reference machine's and CI
    // reports (docs/05-testing/nfr-prf-04-gate-quality.md).
    ci_assertion: CiAssertion::WhenGateEligible,
};

/// The share by which the window-painted mark may grow between the 20k and the
/// 50k vault before the critical path is judged to depend on vault size.
///
/// Architecture §3.4's load-bearing property is that **nothing on the critical
/// path is proportional to vault size**: the main thread never opens the index,
/// and the file tree renders from a query rather than from the walk. If that
/// were false, 2.5x the notes would show up as roughly 2.5x the time. 20% sits
/// far below that and comfortably above run-to-run noise, so the check fails on
/// the thing it is for and not on a warm afternoon.
///
/// This is the assertion that makes §3.4 a claim rather than an intention, and
/// it is the reason the cold-start job runs against two vaults.
pub const SCALE_TOLERANCE: f64 = 0.20;

/// Judge whether the critical path grew with vault size.
///
/// # Errors
/// A sentence naming both figures when it did.
pub fn assert_scale_independent(small_ms: f64, large_ms: f64) -> Result<(), String> {
    let ceiling = small_ms * (1.0 + SCALE_TOLERANCE);
    if large_ms <= ceiling {
        return Ok(());
    }
    Err(format!(
        "window painted at 50,000 notes is {large_ms:.1} ms against {small_ms:.1} ms at 20,000          — {:.0}% growth, over the {:.0}% tolerance. Architecture §3.4 says nothing on the          critical path is proportional to vault size; something on it now is.",
        (large_ms / small_ms - 1.0) * 100.0,
        SCALE_TOLERANCE * 100.0
    ))
}

/// Every P0 budget CI gates, so a configuration review (NFR-PRF-08's
/// acceptance criterion is `Inspect`) can check the list against the PRD
/// rather than against somebody's memory.
///
/// **Every entry below gates; the two pre-split aggregates do not and are in
/// [`PRF01_REPORTED`].** Stated as "every entry" rather than as a number
/// (F-496): a count over a list that grows is maintained by whoever changes
/// the list, who is exactly the person not reading the paragraph above it.
/// The PRD's re-baseline (task #104) gates only `PRF01_SERAT_TERM` for
/// NFR-PRF-01; `PRF01_WINDOW_VISIBLE` and `PRF01_VAULT_USABLE` are measured
/// and reported by the same job and gate nothing (F-443 was the gap between
/// those two facts; F-407 is why the aggregates cannot decide a pass). They
/// are in [`PRF01_REPORTED`], not gone -- **#118 removed the assertion, not
/// the figure.**
///
/// The split is checked mechanically rather than kept by hand: see
/// `the_gated_list_and_the_reported_list_partition_every_budget_by_its_own_flag`.
pub const P0_BUDGETS: &[&Budget] = &[&PRF01_SERAT_TERM, &PRF03_SAVE_TO_INDEX, &PRF04_FULL_REBUILD];

/// Every P0 figure that is measured and printed and gates nothing (task #118).
///
/// This list exists so "reported" is a place rather than an absence. A figure
/// that is merely dropped from [`P0_BUDGETS`] stops being enumerable, stops
/// being testable, and is one refactor away from silently ceasing to be
/// measured -- which is the failure #118 exists to avoid, not to cause.
pub const PRF01_REPORTED: &[&Budget] = &[&PRF01_WINDOW_VISIBLE, &PRF01_VAULT_USABLE];

/// A benchmark vault the harness knows how to produce and how to recognise.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    /// `smoke`, `20k`, `50k`.
    pub name: &'static str,
    /// Total `.md` files.
    pub notes: usize,
    /// Binaries under `assets/`.
    pub assets: usize,
    /// Location, relative to the repository root.
    pub dir: &'static str,
    /// **The canonical tree digest**, at `VAULT_FORMAT_VERSION` 1 and the
    /// canonical seed.
    ///
    /// Pinned here rather than only in CI because of P0 finding F-26. Verifying
    /// the vault against its own manifest proves self-consistency and nothing
    /// else: regenerate with `--seed 424242` and the manifest is regenerated
    /// alongside it, so the two agree perfectly about a vault nobody meant to
    /// measure. `vaultgen` prints a warning in that case and the harness never
    /// sees it. This constant is what turns "the vault matches its manifest"
    /// into "the vault is the one every recorded number was taken against".
    pub digest: &'static str,
}

/// The three vaults, with their canonical digests.
///
/// If a change to `vaultgen` changes these, `VAULT_FORMAT_VERSION` is bumped in
/// the same commit and every recorded benchmark number is re-taken. A number
/// from format version 1 and a number from format version 2 are not comparable.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "smoke",
        notes: 400,
        assets: 24,
        dir: "bench/vaults/vault-smoke",
        digest: "c461c84347186fa66ff0be205d2a2e2aab1ed5bd9f281c2fba8e8b746bd9f3e9",
    },
    Preset {
        name: "20k",
        notes: 20_000,
        assets: 200,
        dir: "bench/vaults/vault-20k",
        digest: "35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e",
    },
    Preset {
        name: "50k",
        notes: 50_000,
        assets: 500,
        dir: "bench/vaults/vault-50k",
        digest: "42fb99b2c7fcef77c26d6a578d63efd4169f68236c4a4b066608d3e0f0276aa1",
    },
];

/// Look a preset up by name.
///
/// # Errors
/// An unknown name, listing the ones that exist.
pub fn preset(name: &str) -> Result<&'static Preset, String> {
    PRESETS.iter().find(|p| p.name == name).ok_or_else(|| {
        format!(
            "unknown preset {name}; expected one of {}",
            PRESETS
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// Timings, in milliseconds.
#[derive(Clone, Debug, Default)]
pub struct Samples {
    values: Vec<f64>,
}

impl Samples {
    /// Empty.
    pub fn new() -> Samples {
        Samples { values: Vec::new() }
    }

    /// Record one measurement.
    pub fn push(&mut self, d: Duration) {
        self.values.push(d.as_secs_f64() * 1000.0);
    }

    /// How many.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The raw values, in collection order.
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Nearest-rank percentile: the smallest value at or above which `q` of the
    /// samples fall.
    ///
    /// Stated explicitly because "p95" is ambiguous and the choice changes the
    /// answer at n = 20 — the exact size NFR-PRF-01 specifies. Nearest rank at
    /// q = 0.95, n = 20 is the 19th of 20 sorted samples, so one slow run out of
    /// twenty is inside the budget and two are not. Interpolating would let two
    /// slow runs pass, which is not what the requirement means.
    pub fn percentile(&self, q: f64) -> f64 {
        assert!(!self.values.is_empty(), "no samples");
        let mut sorted = self.values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let rank = (q * sorted.len() as f64).ceil().max(1.0) as usize;
        sorted[rank.min(sorted.len()) - 1]
    }

    /// Smallest sample.
    pub fn min(&self) -> f64 {
        self.values.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// Largest sample.
    pub fn max(&self) -> f64 {
        self.values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Arithmetic mean.
    pub fn mean(&self) -> f64 {
        self.values.iter().sum::<f64>() / self.values.len() as f64
    }

    /// The median. Nearest rank at q = 0.5, so at n = 3 it is the middle
    /// sample and emphatically not the fastest.
    pub fn median(&self) -> f64 {
        self.percentile(0.50)
    }

    /// The statistic this budget is judged on.
    pub fn statistic(&self, s: Statistic) -> f64 {
        match s {
            Statistic::P95 => self.percentile(0.95),
            Statistic::WallClock => self.median(),
        }
    }
}

/// What the vault was when the number was taken.
#[derive(Clone, Debug)]
pub struct VaultUnderTest {
    /// Where it is.
    pub path: PathBuf,
    /// Its recomputed tree digest.
    pub digest: String,
    /// The generator format version that produced it.
    pub format_version: u32,
    /// Notes it contains.
    pub notes: usize,
    /// Note-to-note link edges the generator produced, from the manifest's
    /// `link_degree.edges`. The oracle for `RebuildReport.links_resolved`.
    pub note_links: usize,
    /// Binary asset files it contains, from the manifest's `counts.assets`.
    ///
    /// Needed because `RebuildReport::notes` counts every row in `notes` --
    /// markdown notes and assets alike, `kind='asset'` per ADR-0005 -- so a
    /// correct rebuild of this vault reports `notes + assets`, not `notes`
    /// alone (F-247, test-engineer's). The 20k preset is 20,000 notes and 200
    /// assets; an oracle of 20,000 rejects every correct run.
    pub assets: usize,
    /// `![[binary]]` embeds, from the manifest's `content.asset_embeds`.
    ///
    /// An asset is a row in `notes` with `kind='asset'` (ADR-0005), so a rebuild
    /// resolves `![[report.pdf]]` like any other link — while `link_degree.edges`
    /// counts note-to-note edges only. The oracle is therefore
    /// `note_links + asset_embeds + asset_md_links`, or a correct rebuild is
    /// refused for resolving more than the manifest admits to (F-319).
    pub asset_embeds: usize,
    /// `[title](assets/binary.pdf)` links, from `content.asset_md_links`.
    ///
    /// The other way a vault references a binary. Counted apart because it is a
    /// different construct with the same consequence — the parser emits a
    /// `kind='md'` link and it resolves to the asset row (F-320).
    pub asset_md_links: usize,
}

/// A series the job measured but does not assert.
///
/// NFR-PRF-03's end-to-end interval is the case that forced this: it is real,
/// it is worth reporting, and printing it outside the report block put an
/// unasserted 812 ms above a line reading `PASS` (P0 finding F-28). Carried on
/// the [`Outcome`] and rendered in one place, so its label and its verdict
/// cannot drift apart.
#[derive(Clone, Debug)]
pub struct ReportedSeries {
    /// Short name, e.g. `end-to-end`.
    pub label: String,
    /// Why it is here and why it is not the budget figure.
    pub note: String,
    /// The timings.
    pub samples: Samples,
}

/// What became of every save a `--bench-save-list` run attempted.
///
/// Task **#94**, finding **F-450**, and the lead's binding condition on
/// `subject::prewarm_index`: *the run must report `handle_missing`, and any
/// residual must be reported, never absorbed.* Pre-warming the index is a
/// change that makes a failing measurement pass, so the census it makes
/// passable is stated on the face of the result rather than left to be
/// inferred from the absence of an error.
///
/// `handle_missing` is a hard zero — [`check_save_completeness`] refuses the
/// run before an [`Outcome`] exists if it is not — so every census that
/// reaches a report carries a zero there. **That zero is the point.** It is
/// #119's answer arriving from the other direction, and printing it is what
/// distinguishes "the pre-warm closed the window" from "nobody looked".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveCensus {
    /// Saves the run was asked to make.
    pub attempted: usize,
    /// Saves that reached the index writer, and so are in the asserted series.
    pub submitted: usize,
    /// Saves ADR-0011 backpressure refused. Non-zero is a real NFR-PRF-03
    /// finding and already sets [`Outcome::degraded`].
    pub writer_refused: u64,
    /// Saves that found no index handle — F-382's window.
    pub handle_missing: u64,
}

/// A finished measurement.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// Which budget.
    pub budget: Budget,
    /// Which machine profile the limit came from.
    pub profile: Profile,
    /// The vault it was measured against.
    pub vault: VaultUnderTest,
    /// What the machine was doing while the number was taken.
    pub environment: Environment,
    /// The timings the budget is asserted against.
    pub samples: Samples,
    /// Series measured and reported but **not** asserted, rendered inside the
    /// report block so they cannot be read as the budget figure (F-28).
    pub reported: Vec<ReportedSeries>,
    /// Where the time went, for budgets that can say. P0 finding F-10.
    ///
    /// The budget still asserts the aggregate — NFR-PRF-04 is a wall-clock
    /// requirement. This is reported beside it, because a single number cannot
    /// answer architecture.md's open design question 1, and that question is
    /// what §1.2 says the parse pool exists to settle.
    pub phases: Option<subject::RebuildPhases>,
    /// Why this measurement, though real, must not be reported as a pass no
    /// matter what `samples` says (task #94, F-382/F-421).
    ///
    /// Distinct from a measurement error (an `Err` from the `measure`
    /// closure, which produces no `Outcome` at all — "failing the run") and
    /// from an ordinary breach (`value_ms() > limit_ms()`, which needs no
    /// help from this field to fail). This is for a run that is real and
    /// honestly measured, and still must not pass: NFR-PRF-03's own p95 over
    /// `save_submitted_ms` alone cannot see that some fraction of the 1,000
    /// attempted saves never reached the index (ADR-0011 backpressure,
    /// reported here rather than folded silently into a fast-looking
    /// series). "Fail the number, not the run."
    pub degraded: Option<String>,
    /// The save path's own census, for the budget that has one (NFR-PRF-03).
    ///
    /// `None` for every other budget: nothing else in P0 attempts a countable
    /// unit of work that can fail without failing the run.
    pub save_census: Option<SaveCensus>,
}

impl Outcome {
    /// The judged value, in milliseconds.
    pub fn value_ms(&self) -> f64 {
        self.samples.statistic(self.budget.statistic)
    }

    /// The limit that applies, in milliseconds.
    pub fn limit_ms(&self) -> f64 {
        self.budget.limit_ms(self.profile)
    }

    /// Whether a breach of this figure fails the build (task #118).
    ///
    /// Carried from [`Budget::gated`] rather than decided here, so the one
    /// place that answers "is this a gate" is the budget the PRD describes.
    pub fn gates(&self) -> bool {
        self.budget.gated
    }

    /// Whether a breach of this figure fails the build **in this run** (task #166).
    ///
    /// [`Outcome::gates`] says what the PRD gates; this says whether this run, on this
    /// machine, is one that may assert it. They differ for exactly one case: a budget
    /// declared [`CiAssertion::WhenGateEligible`], run under `--profile ci` on an
    /// environment that is not gate eligible. The reference profile always asserts.
    pub fn asserted(&self) -> bool {
        self.gates() && !self.report_only_here()
    }

    /// A gated budget that this run only reports, because the runner cannot produce a
    /// gate number (task #166). The one place that rule is written.
    pub fn report_only_here(&self) -> bool {
        self.gates()
            && self.profile == Profile::Ci
            && self.budget.ci_assertion == CiAssertion::WhenGateEligible
            // A budget with a regression ceiling is always asserted (ADR-0019).
            && self.budget.ci_limit_ms.is_none()
            // A degraded run (backpressure, indicative iterations) fails the number even
            // here: report-only waives the runner's slowness, not a defective measurement.
            && self.degraded.is_none()
            && !self.gate_eligible()
    }

    /// Whether the budget held.
    ///
    /// `degraded` gates this before the timing comparison runs: a run that
    /// did not reach the index for every attempted save is not a pass no
    /// matter how fast the saves that did land were.
    ///
    /// **Still computed for an ungated figure**, and deliberately: the
    /// arithmetic is meaningful whether or not anything is asserted on it, and
    /// a reader comparing a cold start against the pre-split 400 ms wants that
    /// comparison. What [`Outcome::gates`] decides is whether the comparison
    /// may fail a build, not whether it may be made.
    pub fn passed(&self) -> bool {
        self.degraded.is_none() && self.value_ms() <= self.limit_ms()
    }

    /// Whether this number may be recorded as the gate figure.
    ///
    /// Passing and being gate eligible are different questions. A rebuild
    /// measured with the on-access virus scanner disabled can pass 30 seconds
    /// and still not be the number the charter gate is judged on, because no
    /// user runs that machine. See `env::Environment::gate_eligible`.
    pub fn gate_eligible(&self) -> bool {
        self.environment.gate_eligible()
    }

    /// How far the samples of this one run disagree, as a percentage of the
    /// fastest.
    ///
    /// Printed on every result because it is the only in-run evidence of how
    /// steady the machine was, and F-327 is about a number that was tight,
    /// confident and nine seconds wrong.
    #[must_use]
    pub fn spread_pct(&self) -> f64 {
        let min = self.samples.min();
        if min <= 0.0 {
            return 0.0;
        }
        (self.samples.max() - min) / min * 100.0
    }

    /// Whether this run actually settled the question it was asked.
    ///
    /// **The rule has no tunable constant in it, deliberately.** If every sample
    /// is under the limit the answer is pass; if every sample is over it the
    /// answer is fail; and if the samples straddle the limit then **this run
    /// observed both outcomes** and reporting either one as the verdict is a
    /// coin toss dressed as a measurement. The median can sit comfortably below
    /// a limit that a third of the samples exceeded.
    ///
    /// Chosen after measuring the alternative and rejecting it. A calibration
    /// yardstick -- fixed work, pinned reference, refuse when the machine is off
    /// its reference speed -- is the obvious fix for F-327 and does not survive
    /// contact with this machine: a fixed 3-5 second storage workload varied by
    /// 25-108% within a single run, so the yardstick was noisier than the drift
    /// it was meant to detect (F-332). Nothing inside one process can attest a
    /// machine that unsteady. What a run *can* say honestly is whether its own
    /// samples agreed, and that is what this reports.
    #[must_use]
    pub fn determinate(&self) -> bool {
        let limit = self.limit_ms();
        self.samples.max() <= limit || self.samples.min() > limit
    }

    /// The phase breakdown must not claim more time than the clock measured.
    ///
    /// The report already tells a reader that a sum *below* the wall clock is
    /// concurrency between the parse pool and the writer rather than an error.
    /// It named the anomaly and did not check it (P0 finding F-29). Since the
    /// phases are the whole content of open design question 1, a breakdown that
    /// double-counted would be reported as fact and would send the architect
    /// the wrong way.
    ///
    /// The 5% tolerance is for timer granularity across five separately
    /// measured spans, not for slop.
    ///
    /// # Errors
    /// When the phases sum to more than the wall clock plus tolerance.
    pub fn check_phases(&self) -> Result<(), String> {
        let Some(phases) = self.phases else {
            return Ok(());
        };
        check_bulk_load_shape(&phases)?;
        let sum_ms = phases.sum().as_secs_f64() * 1000.0;
        let total_ms = self.value_ms();
        if sum_ms <= total_ms * 1.05 {
            return Ok(());
        }
        Err(format!(
            "the phase breakdown sums to {sum_ms:.0} ms against a {total_ms:.0} ms wall clock. \
             A sum below the total is concurrency; a sum above it means a phase is counted \
             twice, and these numbers answer open design question 1 — they must not be \
             reported as fact."
        ))
    }

    /// Human-readable report. Printed by the job, pasted into a gate report.
    pub fn to_report(&self) -> String {
        let mut s = String::new();
        // An ungated figure never prints PASS or FAIL. Task #118: those two
        // words are a verdict, and a verdict on a series nothing asserts is
        // F-443's untruth in the opposite direction -- a reader who sees FAIL
        // beside a green build learns to ignore the word.
        let verdict = match (self.gates(), self.determinate(), self.passed()) {
            (false, _, _) => "REPORTED",
            // Task #166: gated by the PRD, not assertable on this runner. Never PASS or
            // FAIL: a verdict on a figure nothing asserts is F-443's untruth again.
            (true, _, _) if self.report_only_here() => "REPORT-ONLY",
            (true, false, _) => "INDETERMINATE",
            (true, true, true) => "PASS",
            (true, true, false) => "FAIL",
        };
        let _ = writeln!(
            s,
            "{verdict}  {} · {}",
            self.budget.requirement, self.budget.id
        );
        let _ = writeln!(s, "  budget      {}", self.budget.statement);
        let _ = writeln!(s, "  interval    {}", self.budget.interval);
        let _ = writeln!(
            s,
            "  measured    {} = {:.1} ms   limit {:.1} ms ({} profile)",
            self.budget.statistic.describe(self.samples.len()),
            self.value_ms(),
            self.limit_ms(),
            self.profile.label()
        );
        if !self.gates() {
            let _ = writeln!(
                s,
                "  not a gate  PRD v1.2 reports this figure and does not gate it (task #118,                  F-443). The {:.1} ms limit above is the pre-split reference figure, printed                  because it is what a reader compares a cold start against -- it decides                  nothing here, and {} is the NFR-PRF-01 budget that does (F-407: this series                  moves ~19% batch-to-batch against ~8% for the Serat term, so it cannot decide                  a pass at any threshold).",
                self.limit_ms(),
                PRF01_SERAT_TERM.id
            );
        }
        if self.report_only_here() {
            let _ = writeln!(
                s,
                "  not asserted  this runner cannot produce a gate number ({}) and the CI profile \
                 has no regression ceiling, so the {:.1} ms product limit is compared and printed \
                 but cannot fail the build (task #166). It is asserted on the reference machine \
                 (--profile reference); a breach there is a NO-GO.",
                self.environment
                    .ineligibility_reason()
                    .unwrap_or("not gate eligible"),
                self.limit_ms()
            );
        }
        let _ = writeln!(
            s,
            "  samples     n={}  min {:.1}  mean {:.1}  max {:.1} ms  spread {:.1}%",
            self.samples.len(),
            self.samples.min(),
            self.samples.mean(),
            self.samples.max(),
            self.spread_pct(),
        );
        if !self.determinate() && !self.report_only_here() {
            let _ = writeln!(
                s,
                "  undecided   the samples straddle the {:.1} ms limit -- this run observed \
                 both a pass and a fail, so it has not settled the question. Re-run with more \
                 iterations, or on a machine steady enough to answer it. Reported as a failure \
                 because a run that decided nothing must not go green.",
                self.limit_ms(),
            );
        }
        if let Some(reason) = &self.degraded {
            let _ = writeln!(s, "  DEGRADED    {reason}");
        }
        if let Some(c) = self.save_census {
            // Printed whether or not anything went wrong, and `handle_missing`
            // is printed even at zero: the pre-warm this census exists to
            // police (task #94, F-450) is a setup change that makes a failing
            // measurement pass, and a residual that is never shown is a
            // residual that was absorbed.
            let _ = writeln!(
                s,
                "  save path   {} attempted · {} submitted · {} writer-refused · {} handle-missing",
                c.attempted, c.submitted, c.writer_refused, c.handle_missing
            );
        }
        let _ = writeln!(
            s,
            "  vault       {} ({} notes, generator v{})",
            self.vault.path.display(),
            self.vault.notes,
            self.vault.format_version
        );
        for extra in &self.reported {
            if extra.samples.is_empty() {
                continue;
            }
            let _ = writeln!(
                s,
                "  {:<11} p95 {:.1} ms over {} samples — reported, not asserted. {}",
                extra.label,
                extra.samples.percentile(0.95),
                extra.samples.len(),
                extra.note
            );
        }
        let _ = writeln!(s, "  digest      {}", self.vault.digest);
        let _ = writeln!(s, "  machine     {}", self.environment.summary());
        if let Some(p) = self.phases {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            let total = self.value_ms().max(f64::MIN_POSITIVE);
            let share = |d: std::time::Duration| ms(d) / total * 100.0;
            let _ = writeln!(
                s,
                "  phases      walk {:.0} ms ({:.0}%) · read+hash {:.0} ms ({:.0}%) · parse {:.0} ms ({:.0}%) · producer waited {:.0} ms ({:.0}%) · fts build {:.0} ms ({:.0}%)",
                ms(p.walk),
                share(p.walk),
                ms(p.read_hash),
                share(p.read_hash),
                ms(p.parse),
                share(p.parse),
                ms(p.producer_wait),
                share(p.producer_wait),
                ms(p.write_fts),
                share(p.write_fts)
            );
            let _ = writeln!(
                s,
                "  reading     \"producer waited\" is how long the producer was BLOCKED on the writer, not what the writer cost — near zero means the writer kept up, large means it is the bottleneck"
            );
            let _ = writeln!(
                s,
                "  F-10        phases sum to {:.0} ms against a {:.0} ms wall clock; a sum below the total is concurrency, not error",
                ms(p.sum()),
                total
            );
        }
        if let Some(reason) = self.environment.ineligibility_reason() {
            let _ = writeln!(s, "  NOT A GATE NUMBER: {reason}");
        }
        if self.budget.ci_limit_ms.is_none()
            && self.profile == Profile::Ci
            && !self.report_only_here()
        {
            let _ = writeln!(
                s,
                "  note        no CI regression ceiling is set yet, so the product limit is \
                 being asserted on a runner that is not the reference machine"
            );
        }
        s
    }

    /// Machine-readable result, for trend tracking across runs.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "{{");
        let _ = writeln!(s, "  \"budget_id\": \"{}\",", self.budget.id);
        let _ = writeln!(s, "  \"requirement\": \"{}\",", self.budget.requirement);
        let _ = writeln!(s, "  \"interval\": \"{}\",", self.budget.interval);
        let _ = writeln!(s, "  \"profile\": \"{}\",", self.profile.label());
        let _ = writeln!(s, "  \"statistic\": \"{}\",", self.budget.statistic.label());
        let _ = writeln!(s, "  \"value_ms\": {:.3},", self.value_ms());
        let _ = writeln!(s, "  \"limit_ms\": {:.3},", self.limit_ms());
        let _ = writeln!(s, "  \"passed\": {},", self.passed());
        let _ = writeln!(
            s,
            "  \"degraded\": {},",
            match &self.degraded {
                // Debug-formatting a String gives JSON-legal quoting (escapes
                // any embedded quote or backslash) without a dependency.
                Some(reason) => format!("{reason:?}"),
                None => "null".to_string(),
            }
        );
        // Task #118: `passed` alone cannot say whether anything depended on
        // it. A trend reader joining these files needs to know which rows were
        // gates when they were written, because that changes.
        let _ = writeln!(s, "  \"gated\": {},", self.gates());
        // Task #166: whether THIS run could fail a build. `gated` is the budget's flag and
        // does not change with the runner; a trend reader needs both.
        let _ = writeln!(s, "  \"asserted\": {},", self.asserted());
        let _ = writeln!(s, "  \"samples_n\": {},", self.samples.len());
        let _ = writeln!(s, "  \"min_ms\": {:.3},", self.samples.min());
        let _ = writeln!(s, "  \"mean_ms\": {:.3},", self.samples.mean());
        let _ = writeln!(s, "  \"max_ms\": {:.3},", self.samples.max());
        let _ = writeln!(s, "  \"vault_digest_sha256\": \"{}\",", self.vault.digest);
        let _ = writeln!(
            s,
            "  \"vault_format_version\": {},",
            self.vault.format_version
        );
        let _ = writeln!(s, "  \"vault_notes\": {},", self.vault.notes);
        let _ = writeln!(s, "  \"os\": \"{}\",", self.environment.os.label());
        let _ = writeln!(s, "  \"cores\": {},", self.environment.cores);
        let _ = writeln!(
            s,
            "  \"realtime_scanner\": {},",
            match self.environment.realtime_scanner {
                Some(true) => "true",
                Some(false) => "false",
                None => "null",
            }
        );
        let _ = writeln!(
            s,
            "  \"power_plan\": \"{}\",",
            self.environment.power_plan.as_deref().unwrap_or("unknown")
        );
        let _ = writeln!(s, "  \"gate_eligible\": {},", self.gate_eligible());
        if let Some(p) = self.phases {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            let _ = writeln!(s, "  \"phases_ms\": {{");
            let _ = writeln!(s, "    \"walk\": {:.3},", ms(p.walk));
            let _ = writeln!(s, "    \"read_hash\": {:.3},", ms(p.read_hash));
            let _ = writeln!(s, "    \"parse\": {:.3},", ms(p.parse));
            let _ = writeln!(s, "    \"producer_wait\": {:.3},", ms(p.producer_wait));
            let _ = writeln!(s, "    \"write_fts\": {:.3}", ms(p.write_fts));
            let _ = writeln!(s, "  }},");
        }
        if let Some(c) = self.save_census {
            let _ = writeln!(s, "  \"save_path_census\": {{");
            let _ = writeln!(s, "    \"attempted\": {},", c.attempted);
            let _ = writeln!(s, "    \"submitted\": {},", c.submitted);
            let _ = writeln!(s, "    \"writer_refused\": {},", c.writer_refused);
            let _ = writeln!(s, "    \"handle_missing\": {}", c.handle_missing);
            let _ = writeln!(s, "  }},");
        }
        if !self.reported.is_empty() {
            let _ = writeln!(s, "  \"reported_not_asserted\": {{");
            let last = self.reported.len() - 1;
            for (i, extra) in self.reported.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "    \"{}\": {{ \"p95_ms\": {:.3}, \"samples_n\": {} }}{}",
                    extra.label,
                    extra.samples.percentile(0.95),
                    extra.samples.len(),
                    if i == last { "" } else { "," }
                );
            }
            let _ = writeln!(s, "  }},");
        }
        let _ = write!(s, "  \"samples_ms\": [");
        for (i, v) in self.samples.values().iter().enumerate() {
            if i > 0 {
                let _ = write!(s, ", ");
            }
            let _ = write!(s, "{v:.3}");
        }
        let _ = writeln!(s, "]");
        let _ = writeln!(s, "}}");
        s
    }
}

/// What one budget's measurement produced.
///
/// One asserted series, optionally a phase breakdown, and any number of series
/// that are reported but not asserted. Every job now returns this, which is
/// what let `prf01-cold-start` stop reimplementing argument parsing and lose
/// two of the guards in the process (P0 finding F-27).
#[derive(Clone, Debug, Default)]
pub struct Measurement {
    /// The series the budget is judged on.
    pub samples: Samples,
    /// Where the time went, for budgets that can say (F-10).
    pub phases: Option<subject::RebuildPhases>,
    /// Series reported beside it.
    pub reported: Vec<ReportedSeries>,
    /// Carried straight through to [`Outcome::degraded`] by [`outcome_from`] —
    /// see that field's doc for what it means and why it is not the same
    /// thing as this function returning `Err`.
    pub degraded: Option<String>,
    /// Carried straight through to [`Outcome::save_census`] by [`outcome_from`].
    pub save_census: Option<SaveCensus>,
}

impl Measurement {
    /// A measurement with nothing but the asserted series.
    pub fn new(samples: Samples) -> Measurement {
        Measurement {
            samples,
            phases: None,
            reported: Vec::new(),
            degraded: None,
            save_census: None,
        }
    }

    /// Add a series that is reported but not asserted.
    #[must_use]
    pub fn reporting(mut self, label: &str, note: &str, samples: Samples) -> Measurement {
        self.reported.push(ReportedSeries {
            label: label.to_string(),
            note: note.to_string(),
            samples,
        });
        self
    }

    /// Mark this measurement as real but not a pass, for a reason `samples`
    /// alone cannot express (task #94; see [`Outcome::degraded`]).
    #[must_use]
    pub fn degraded(mut self, reason: impl Into<String>) -> Measurement {
        self.degraded = Some(reason.into());
        self
    }

    /// Attach the phase breakdown.
    #[must_use]
    pub fn with_phases(mut self, phases: subject::RebuildPhases) -> Measurement {
        self.phases = Some(phases);
        self
    }

    /// Attach the save-path census (task #94, F-450).
    #[must_use]
    pub fn counting(mut self, census: SaveCensus) -> Measurement {
        self.save_census = Some(census);
        self
    }
}

/// The command-line options every budget job accepts.
#[derive(Clone, Copy, Debug)]
pub struct JobArgs {
    /// Which machine profile's threshold applies.
    pub profile: Profile,
    /// How many iterations to run.
    pub iterations: usize,
    /// Whether `iterations` differs from the acceptance criterion's count, in
    /// which case the result is indicative and must not be recorded.
    pub indicative: bool,
    /// The caller asked what the flags are, so the job prints them and measures
    /// nothing. A run that took 37 seconds to answer `--help` was the symptom
    /// that started F-323.
    pub help: bool,
}

/// What the flags are, for `--help` and for the refusal message.
#[must_use]
pub fn usage(budget: &Budget) -> String {
    let (flag_synopsis, extra) = if budget.id == PRF01_WINDOW_VISIBLE.id {
        (
            " [--skip-scale]",
            "  --skip-scale        Report the two figures only; skip the 20k-vs-50k scale
                      assertion (architecture 3.4). Asserts nothing: not for a gating job.
",
        )
    } else {
        ("", "")
    };
    format!(
        "usage: {id} [--profile reference|ci] [--iterations N] [--root DIR]{flag_synopsis} [--help]

  --profile <name>    Which machine profile's threshold applies.
                      One of: reference (default), ci.
  --iterations <N>    How many iterations to run. Default {iterations}, which is
                      what this budget's acceptance criterion names. Any other
                      value marks the result indicative: it is printed, and it
                      must not be recorded as a gate number.
  --root <DIR>        Repository root (also SERAT_REPO_ROOT). Default: the
                      [workspace] above the working directory.
{extra}  --help, -h          Print this and measure nothing.

Both value flags also accept the --flag=value form.

Anything else is refused rather than ignored. A job that skipped a flag it did
not recognise would take a DEFAULT measurement and report it as a configured
one -- `--profile=ci` silently measured against the reference threshold, and a
mistyped `--iteration 1` silently ran the full count (F-323).",
        id = budget.id,
        iterations = budget.iterations,
    )
}

/// Parse the arguments every job shares.
///
/// Every argument is either recognised or refused. The previous version scanned
/// for `--profile` and `--iterations` by exact string and **discarded everything
/// else without comment**, which is the wrong default for a harness whose whole
/// purpose is that a number can be attributed (F-26, NFR-MNT-02). Three shapes it
/// accepted and silently mishandled: `--help` ran the full 37-second job;
/// `--profile=ci` is not the string `--profile`, so it was dropped and the run
/// used the reference threshold; and a mistyped `--iteration 1` ran the default
/// count. Each produced a real measurement, correctly reported as whatever it
/// actually did -- so the report was honest and the operator's intent was gone
/// (F-323).
///
/// A repeated flag is refused rather than resolved. First-wins and last-wins are
/// both defensible and neither is guessable from the command line.
///
/// # Errors
/// An unknown argument, a flag without its value, a value the flag cannot take,
/// or the same flag twice -- each as a sentence, with the usage text.
pub fn parse_args(budget: &Budget, args: &[String]) -> Result<JobArgs, String> {
    let mut profile: Option<Profile> = None;
    let mut iterations: Option<usize> = None;
    let mut help = false;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        // `--flag=value` splits here so that both forms reach the same code. The
        // equals form is what most tools accept, which is exactly why silently
        // ignoring it was dangerous rather than merely unhelpful.
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (arg, None),
        };

        match name {
            "--help" | "-h" => {
                help = true;
                i += 1;
            }
            // Read by `repo_root()` from the process arguments (shared `reporoot`
            // rule); here it is only checked and skipped so it is not "unknown".
            "--root" => {
                if inline.as_deref().is_some_and(str::is_empty)
                    || (inline.is_none() && args.get(i + 1).is_none())
                {
                    return Err(format!(
                        "--root needs a directory

{}",
                        usage(budget)
                    ));
                }
                i += if inline.is_some() { 1 } else { 2 };
            }
            "--profile" | "--iterations" => {
                let value = match inline {
                    Some(value) => {
                        i += 1;
                        value
                    }
                    None => {
                        let value = args
                            .get(i + 1)
                            .cloned()
                            .ok_or_else(|| format!("{name} needs a value\n\n{}", usage(budget)))?;
                        i += 2;
                        value
                    }
                };
                if name == "--profile" {
                    if profile.is_some() {
                        return Err(format!(
                            "--profile given more than once; which one did you mean?\n\n{}",
                            usage(budget)
                        ));
                    }
                    profile = Some(Profile::parse(&value)?);
                } else {
                    if iterations.is_some() {
                        return Err(format!(
                            "--iterations given more than once; which one did you mean?\n\n{}",
                            usage(budget)
                        ));
                    }
                    iterations = Some(value.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(
                        || {
                            format!(
                                "--iterations needs a positive integer, not {value:?}\n\n{}",
                                usage(budget)
                            )
                        },
                    )?);
                }
            }
            other => {
                return Err(format!("unknown argument {other:?}\n\n{}", usage(budget)));
            }
        }
    }

    let iterations = iterations.unwrap_or(budget.iterations);
    Ok(JobArgs {
        profile: profile.unwrap_or(Profile::Reference),
        iterations,
        indicative: iterations != budget.iterations,
        help,
    })
}

/// Repository root, resolved at run time (tasks #45, #54) by the shared `reporoot`
/// crate: `--root`, `SERAT_REPO_ROOT`, a `[workspace]` manifest above the working
/// directory, then the compile-time location. [`parse_args`] has already rejected
/// a `--root` with no value, so the error arm is unreachable from a bin.
///
/// # Panics
/// On a `--root` with no value, when called without going through [`parse_args`].
pub fn repo_root() -> PathBuf {
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    let r = reporoot::from_process(&reporoot::compiled_root(env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("{e}"));
    // Results are written under this root (bench/out/budgets) and the vault is read
    // from it, so the one place the rule is applied also says which rule won, once,
    // on stderr (the compiled-in fallback is the silent-wrong-tree case).
    ANNOUNCED.call_once(|| eprintln!("budget: {}", r.report_line()));
    r.path
}

/// Make sure the preset vault exists, generating it if it does not, then verify
/// that what is on disk is what the manifest says it is.
///
/// A cross-process lock guards the check-then-generate window (task #70):
/// `cargo test --workspace` runs every budget test binary as a
/// separate process, several of which call this for the same preset, and the
/// `.vaultgen` stamp is written only once generation finishes -- seconds,
/// for the 20k preset. Without the lock, a second process can find the
/// directory non-empty and stampless mid-generation by the first and hit
/// vaultgen's own refusal to touch an un-stamped, non-empty directory,
/// rather than the number-or-error a caller is waiting for. Same shape as
/// task #28's `FAIL_NEXT_COMMIT` race, one level up: shared mutable state on
/// a fixed path, concurrent execution, passing in isolation and failing
/// under load.
///
/// # Errors
/// If generation fails, if the manifest is missing, if the vault on disk does
/// not match its manifest, or if another process holds the lock past the
/// timeout below.
pub fn ensure_vault(name: &str) -> Result<VaultUnderTest, String> {
    let preset = preset(name)?;
    let (notes, assets) = (preset.notes, preset.assets);
    let path = repo_root().join(preset.dir);
    let manifest_path = vaultgen::manifest_path_for(&path);

    if !path.join(".vaultgen").exists() || !manifest_path.exists() {
        let _lock = VaultGenerationLock::acquire(&path)?;
        // Re-check now that the lock is held: the process that held it before
        // us may have just finished generating, in which case there is
        // nothing left to do and no reason to regenerate.
        if !path.join(".vaultgen").exists() || !manifest_path.exists() {
            eprintln!("budget: generating {name} vault at {}", path.display());
            let opts = vaultgen::Options {
                seed: vaultgen::DEFAULT_SEED,
                notes,
                assets,
                out: path.clone(),
                manifest: None,
                jobs: None,
                force: true,
                quiet: true,
            };
            vaultgen::generate(&opts).map_err(|e| format!("generating the vault: {e}"))?;
        }
    }

    verify_vault_at(preset, &path)
}

/// A cross-process advisory lock over one vault preset's generation.
///
/// Backed by atomic file creation rather than an OS file lock: `create_new`
/// failing when the file already exists is the one filesystem operation both
/// Windows and POSIX guarantee is atomic, and this crate otherwise has no
/// reason to add a locking dependency for one narrow race. A process that
/// finds the lock held polls rather than blocking indefinitely and gives up
/// well past the slowest real generation this project has measured (the 20k
/// preset, under nine seconds), so a genuinely stuck lock still surfaces as
/// an error naming the file to remove rather than a silent hang.
struct VaultGenerationLock {
    lock_path: PathBuf,
}

impl VaultGenerationLock {
    fn acquire(vault_path: &Path) -> Result<Self, String> {
        let lock_path = vault_path.with_extension("vaultgen-lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { lock_path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::time::Instant::now() >= deadline {
                        return Err(format!(
                            "another process has held the vault-generation lock at {} for over \
                             120s -- if the process that created it is gone, remove the file by \
                             hand",
                            lock_path.display()
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => return Err(format!("creating lock file {}: {e}", lock_path.display())),
            }
        }
    }
}

impl Drop for VaultGenerationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// Verify a vault that already exists, wherever it is.
///
/// Split from [`ensure_vault`] so the verification can be exercised against a
/// real vault in a temporary directory. The digest is over paths relative to the
/// vault root, so it does not depend on where the vault lives -- which is what
/// makes this testable at all.
///
/// # Errors
/// A missing or unreadable manifest, or any of [`check_vault`]'s refusals.
pub fn verify_vault_at(preset: &Preset, path: &Path) -> Result<VaultUnderTest, String> {
    // F-372 / task #78, checked first and reported on its own: a scratch
    // directory a killed benchmark left behind reads, downstream, as a tree-
    // digest mismatch -- which is what "the vault does not match its
    // manifest" *actually means* here, and it is not what it sounds like. That
    // message names corruption and offers regeneration; the true cause is one
    // stray directory and the true fix is `rm -rf` on it. Checking here, ahead
    // of the digest comparison, means the caller is told the cheap fix instead
    // of reasoning backward from a hash mismatch to a directory listing --
    // which is exactly the four runs it cost the first time (F-371/F-372).
    let stray = stray_scratch_entries(path);
    if !stray.is_empty() {
        return Err(format!(
            "found {} at the vault's top level, which a killed benchmark run leaves behind \
             (F-372, task #78) -- this is almost certainly the whole problem, not vault \
             corruption. Delete it and re-run before doing anything more expensive:\n    {}\n  \
             vault {}",
            stray.join(", "),
            stray
                .iter()
                .map(|name| format!("rm -rf {}", path.join(name).display()))
                .collect::<Vec<_>>()
                .join("\n    "),
            path.display()
        ));
    }

    let manifest_path = vaultgen::manifest_path_for(path);
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {e}", manifest_path.display()))?;
    let expected = json_string(&manifest, "tree_digest_sha256")
        .ok_or_else(|| format!("{} has no tree digest", manifest_path.display()))?;
    let format_version = json_number(&manifest, "format_version").unwrap_or(0.0) as u32;
    let notes_in_manifest = json_number(&manifest, "notes").unwrap_or(0.0) as usize;
    let note_links = json_number(&manifest, "edges").unwrap_or(0.0) as usize;
    // Default 0 rather than failing, so a manifest generated before these
    // counts existed still runs -- and then under-reports the oracle, which
    // shows up as "resolved more than the manifest admits to" rather than as a
    // silent pass. A stale fixture should be visible, not fatal (F-319).
    let asset_embeds = json_number(&manifest, "asset_embeds").unwrap_or(0.0) as usize;
    let asset_md_links = json_number(&manifest, "asset_md_links").unwrap_or(0.0) as usize;

    let actual = tree_digest(path)?;
    check_vault(
        preset,
        &actual,
        &expected,
        format_version,
        notes_in_manifest,
    )
    .map_err(|e| format!("{e}\n  vault {}", path.display()))?;

    Ok(VaultUnderTest {
        path: path.to_path_buf(),
        digest: actual,
        format_version,
        notes: notes_in_manifest,
        note_links,
        assets: preset.assets,
        asset_embeds,
        asset_md_links,
    })
}

/// The three things that must be true of a vault before a clock starts.
///
/// Split out of [`ensure_vault`] so it can be tested without generating 39 MiB
/// (P0 finding F-26): the on-disk bytes match the manifest, the manifest is the
/// *canonical* one, and it holds the number of notes the preset promises.
///
/// # Errors
/// A sentence naming which of the three failed and how to fix it.
pub fn check_vault(
    preset: &Preset,
    on_disk: &str,
    in_manifest: &str,
    format_version: u32,
    notes_in_manifest: usize,
) -> Result<(), String> {
    let regenerate = format!(
        "Regenerate it: cargo run --release -p vaultgen -- --preset {} --force",
        preset.name
    );
    if on_disk != in_manifest {
        return Err(format!(
            "the vault does not match its manifest\n  manifest {in_manifest}\n  on disk  \
             {on_disk}\n{regenerate}"
        ));
    }
    if on_disk != preset.digest {
        return Err(format!(
            "this is not the canonical {} vault\n  canonical {}\n  on disk   {on_disk}\n\
             It matches its own manifest, so it was generated coherently — with a different \
             seed, a different note count, or a different generator version. A number taken \
             against it is not comparable with any recorded number.\n{regenerate}",
            preset.name, preset.digest
        ));
    }
    if format_version != vaultgen::VAULT_FORMAT_VERSION {
        return Err(format!(
            "the vault was written by generator format version {format_version}; this build \
             expects {}. Numbers from two format versions are not comparable.\n{regenerate}",
            vaultgen::VAULT_FORMAT_VERSION
        ));
    }
    if notes_in_manifest != preset.notes {
        return Err(format!(
            "the manifest reports {notes_in_manifest} notes; the {} preset is {} \
             notes\n{regenerate}",
            preset.name, preset.notes
        ));
    }
    Ok(())
}

/// Whether the rebuild still used the bulk-load shape (P0 finding F-38).
///
/// `write_rows` and `write_fts` are only comparable across runs if both were
/// measured on the same shape: FTS triggers dropped, `blocks_fts` built once at
/// the end. If a rebuild ever regressed to loading *through* the triggers,
/// `write_fts` would read as roughly zero and `write_rows` would absorb the
/// whole cost — **the breakdown would look better while the total got worse**,
/// and nothing in a wall-clock figure can see that.
///
/// It is worth a check rather than a comment because of what the regression
/// costs: with the FTS triggers live a bulk load costs about **6x** on the
/// phase that has to fit inside a 30 s budget — 26,000 blocks at 2.1 s live
/// against 0.32 s dropped, independently reproduced at 5.8x with an identical
/// resulting search index (F-32). This is also the phase breakdown justifying its own
/// existence — the aggregate cannot distinguish these two worlds.
///
/// # Errors
/// When a rebuild that did real work reports no FTS build at all.
pub fn check_bulk_load_shape(phases: &subject::RebuildPhases) -> Result<(), String> {
    // Gated on `parse` rather than on the producer's wait: a fast writer means
    // the producer never blocks, so `producer_wait` is legitimately near zero on
    // exactly the healthy runs this check must still cover. `parse` is non-zero
    // whenever the rebuild did any work at all.
    if phases.parse.is_zero() {
        return Ok(());
    }
    if phases.write_fts.is_zero() {
        return Err(
            "the rebuild wrote rows but reports no FTS build time. blocks_fts is built once at \
             the end of the bulk load, so a zero here means the load went through the FTS \
             triggers instead — which shows as a better-looking breakdown and a worse total. \
             Loading through the triggers cost about 6x when it was measured: 26,000 blocks at \
             2.1 s live against 0.32 s dropped (F-32)."
                .to_string(),
        );
    }
    Ok(())
}

/// Whether `ANALYZE` described the finished index rather than an earlier one.
///
/// P0 finding **F-47**. `sqlite_stat1` is read from the index directly, so this
/// is an independent oracle rather than a number the rebuild reports about
/// itself. `sqlite_stat1` describes the whole `notes` table, so `expected` must
/// be `vault.notes + vault.assets`, the same correction [`check_note_count`]
/// needs and for the same reason (ADR-0005: an asset is a row in `notes` too).
///
/// # Errors
/// When the recorded row count is not the vault's total row count.
pub fn check_analyze_freshness(recorded: usize, expected: usize) -> Result<(), String> {
    // The precondition, stated rather than assumed (P0 finding F-128). This is
    // the only place in the project where the defect is visible at all: below
    // roughly 10,000 notes the opportunistic 5% re-ANALYZE fires on the last
    // batch every time, the explicit analyze() becomes unobservable, and this
    // check silently stops guarding while still passing. A green result here on
    // a small vault means nothing was tested, not that nothing is wrong.
    if expected < 10_000 {
        return Err(format!(
            "the ANALYZE check needs a vault of at least 10,000 notes to mean anything and this \
             one has {expected}. Below that the opportunistic 5% re-ANALYZE fires every batch \
             and the explicit call cannot be observed. Refusing rather than passing, because a \
             guard that quietly stops guarding is worse than one that is absent."
        ));
    }
    if recorded == expected {
        return Ok(());
    }
    Err(format!(
        "ANALYZE recorded {recorded} rows for `notes`; the vault holds {expected}. The \
         statistics describe an earlier index, so the planner picks ix_notes_kind over \
         ix_notes_type and every typed query degrades to a full scan (architecture §4.3). At \
         20,000 notes the opportunistic 5% re-ANALYZE never fires for the tail batch of 32, so \
         the explicit analyze() is the only thing that makes this true — and this is the only \
         scale at which its absence is visible."
    ))
}

/// Whether every link the vault contains was resolved.
///
/// The expected count is `note_links + asset_embeds`: the manifest records the
/// note-to-note edges it generated and, separately, the `![[binary]]` embeds,
/// and an asset is a row in `notes` so a rebuild resolves both (F-129). A
/// rebuild reporting fewer has dropped links; one reporting more has counted
/// rows touched rather than links resolved — the bug rust-core-dev hit while
/// writing the second pass, which is the rebuild-backpressure shape one layer
/// up.
///
/// # Errors
/// When the counts disagree.
pub fn check_links_resolved(reported: usize, expected: usize) -> Result<(), String> {
    if reported == expected {
        return Ok(());
    }
    let direction = if reported < expected {
        "fewer, so links were dropped"
    } else {
        "more, so something other than resolved links is being counted"
    };
    Err(format!(
        "the rebuild resolved {reported} note-to-note links; the vault manifest generated \
         {expected}. That is {direction}."
    ))
}

/// Whether a rebuild indexed the whole vault.
///
/// A fast number over a partially indexed vault is the shape of a false pass,
/// and a wall clock cannot see it (P0 finding F-19). Extracted from the job so
/// that deleting it fails a test rather than passing silently (F-27).
///
/// `expected` must be `vault.notes + vault.assets`, not `vault.notes` alone:
/// `RebuildReport::notes` counts every row in the `notes` table, and an asset
/// is a row there too (`kind='asset'`, ADR-0005). Passing `vault.notes` on its
/// own rejects every correct rebuild of a vault that has any assets at all —
/// the 20k preset always would have, at 20,000 against 20,200.
///
/// # Errors
/// When the counts disagree.
pub fn check_note_count(reported: usize, expected: usize) -> Result<(), String> {
    if reported == expected {
        return Ok(());
    }
    Err(format!(
        "the rebuild reported {reported} notes, the vault manifest says {expected}. A fast \
         number over a partial index is not a passing budget."
    ))
}

/// Whether a `--bench-save-list` run's own counters add up — checked against
/// the harness's own count of what it attempted rather than trusted from the
/// app's arithmetic, the same "recompute, don't trust" rule [`tree_digest`]
/// and [`subject::stat1_note_rows`] already follow.
///
/// P0 findings **F-382, F-390, F-421**, task **#94**. The two ways an
/// attempted save can fail to reach the index want different treatment,
/// which is why this is one check rather than "the degraded count is zero":
///
/// * `handle_missing` is a **hard zero**, checked here. F-382's legitimate
///   empty-handle race is a narrow window at `open_vault` — a save landing
///   before the handle exists — and seeing it recur across a run this long
///   means something else is wrong. Non-zero here refuses the whole run: no
///   number is produced, because none of what it attempted can be trusted to
///   mean what the series claims.
///
///   **Read this before softening it (F-499, F-475).** Task #122's attestation
///   of the tree behind NFR-PRF-03's 4.306 ms left **one mutation surviving**,
///   and it was accepted as a SHOULD-FIX rather than a blocker **only because
///   this check is a hard zero.** The strictness here is what makes that
///   survivor harmless: a run that would expose it cannot produce a number at
///   all. Relax this to a threshold, a warning, or a `degraded` reason and
///   **the survivor becomes blocking the same day**, and the standing gate
///   number loses the argument that admitted it. The conditional is written
///   here rather than only in the register and the report because the person
///   who would soften a check is reading the check, not the finding — the
///   register is a record, not a tripwire.
/// * `writer_refused` is **not** checked here. ADR-0011's sanctioned
///   backpressure may legitimately fire, and ruling on a non-zero value is
///   [`measure_save_to_index_with`]'s job, not this one's — ADR-0011
///   backpressure is a real, reportable NFR-PRF-03 finding, not evidence
///   this harness malfunctioned, and refusing the run over it here would be
///   exactly the "fail the run instead of the number" mistake this function
///   exists to avoid.
///
/// What this function alone checks is the **denominator**: that `submitted`
/// plus `writer_refused` accounts for everything `--bench-save-list`
/// attempted. That is deliberately a different check from "the two named
/// degraded counters are zero" — it also catches a fourth, unnamed way a
/// save could vanish that neither counter was built to count, which neither
/// `handle_missing == 0` nor `writer_refused == 0` alone would notice.
///
/// # Errors
/// `handle_missing` is non-zero, or the counted outcomes do not sum to what
/// was attempted.
pub fn check_save_completeness(trace: &trace::SaveBenchmarkTrace) -> Result<(), String> {
    if trace.handle_missing != 0 {
        return Err(format!(
            "{} of {} attempted saves found no index handle. F-382's legitimate race is a \
             narrow window at open; seeing it recur across a run this long means an instrument \
             problem, not a number to report.",
            trace.handle_missing, trace.attempted
        ));
    }
    let accounted = trace.submitted_ms.len() as u64 + trace.writer_refused;
    if accounted != trace.attempted as u64 {
        return Err(format!(
            "{} attempted saves but only {accounted} are accounted for -- {} submitted, {} \
             refused. Something vanished in a way neither named counter caught.",
            trace.attempted,
            trace.submitted_ms.len(),
            trace.writer_refused
        ));
    }
    // F-445's first SHOULD-FIX: a run this counters agree on is still one with
    // no p95 to report if nothing was ever submitted -- every attempt landed in
    // writer_refused. Left unhandled, that reaches Samples::percentile with an
    // empty series and panics on its own "no samples" assertion (outcome_from's
    // guard catches an empty Measurement generically, but the specific reason
    // -- every save refused -- is exactly the message a reader most needs and
    // the one a generic panic or a generic "no samples" Err both discard). A
    // named refusal here is the last point before that reason is lost.
    if trace.submitted_ms.is_empty() && trace.attempted > 0 {
        return Err(format!(
            "all {} attempted saves were refused by the writer (ADR-0011 backpressure) -- there \
             is no p95 to compute over zero submitted saves. This is a finding about the queue \
             being saturated well beyond a single save's workload, not a shorter series to \
             quietly accept.",
            trace.attempted
        ));
    }
    Ok(())
}

/// Recompute the vault's tree digest from what is on disk, exactly as the
/// generator computes it: SHA-256 over `path\0filehash\n` in sorted path order.
///
/// # Errors
/// Any filesystem failure while walking.
pub fn tree_digest(root: &Path) -> Result<String, String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(root, root, &mut files).map_err(|e| format!("walking {}: {e}", root.display()))?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut tree = vaultgen::hash::Sha256::new();
    for (rel, abs) in &files {
        let bytes = std::fs::read(abs).map_err(|e| format!("reading {}: {e}", abs.display()))?;
        tree.update(rel.as_bytes());
        tree.update(b"\0");
        tree.update(&vaultgen::hash::Sha256::digest(&bytes));
        tree.update(b"\n");
    }
    Ok(vaultgen::hash::hex(&tree.finish()))
}

/// Top-level entries a killed benchmark run leaves inside a fixture vault
/// (F-372, task #78).
///
/// `measure_full_rebuild_with` and `measure_save_to_index_with` used to build
/// their scratch indexes at `vault.path.join(".serat-bench-<n>")` and
/// `vault.path.join(".serat-bench")` -- both *inside* the fixture. A run that
/// completes cleans up; one that is killed does not, and what it leaves moves
/// the vault's tree digest for every caller after it, silently, until someone
/// reads `verify_vault_at`'s error as corruption and regenerates 20,000 notes
/// to fix one stray directory. Both call sites now build in `temp_dir()`
/// instead, so this should find nothing on a tree this crate produced itself
/// -- but it stays, because the failure mode it caught once was exactly a
/// change that "should" have been unreachable, and a one-`read_dir` check
/// costs nothing next to what reading the wrong error cost.
fn stray_scratch_entries(path: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path) else {
        // Whatever is wrong with `path` here is `verify_vault_at`'s to report,
        // not this function's -- it has nothing more specific to add.
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with(".serat-bench"))
        .collect()
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        // C-02: the index is disposable, and its own existence is not vault content.
        // vaultgen never writes .serat/ -- it is entirely the product's own doing, the
        // instant anything opens the vault for real -- so it never enters the digest
        // this compares against, and this recompute must not treat its mere presence
        // as a difference either. F-401: this bit the shared 20k vault twice in one
        // session, once through cold_start_once (fixed by ColdStartVaultGuard) and once
        // through some other real invocation of the app against it directly, which no
        // per-caller cleanup can fully guard against -- excluding it here does, for
        // every caller, present and future, without relying on anyone remembering to
        // clean up after themselves. Root only (`vault_root.join(".serat")`,
        // crates/serat-app/src/index.rs) -- a nested directory that happens to share
        // the name is real vault content and must still be seen.
        if dir == root
            && path.is_dir()
            && path.file_name().and_then(|n| n.to_str()) == Some(".serat")
        {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Write the result where the gate report and any trend tooling will find it.
///
/// # Errors
/// Any filesystem failure.
pub fn write_result(outcome: &Outcome) -> std::io::Result<PathBuf> {
    write_result_to(&repo_root().join("bench/out/budgets"), outcome)
}

/// As [`write_result`], into a directory of the caller's choosing.
///
/// # Errors
/// Any filesystem failure.
pub fn write_result_to(dir: &Path, outcome: &Outcome) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", outcome.budget.id));
    std::fs::write(&path, outcome.to_json())?;
    Ok(path)
}

// A manifest reader rather than a JSON parser: the manifest is written by
// `vaultgen`, its shape is fixed, and pulling in serde to read three fields
// would be the tail wagging the dog.
fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}

fn json_number(text: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The whole of a budget job except printing and exiting.
///
/// Split out of [`run_job`] because a function returning an opaque `ExitCode`
/// cannot be tested, and P0 finding F-27 was four mutations that no test
/// noticed — including a job swallowing the seam error to post a fabricated
/// number, and the deletion of the note-count guard. Every one of those is now
/// a `Result` a test can inspect.
///
/// # Errors
/// A malformed argument, an unusable vault, a failed measurement, a measurement
/// that produced no samples, or a phase breakdown that does not add up.
pub fn decide<F>(budget: &Budget, args: &[String], measure: F) -> Result<Outcome, String>
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<Measurement, String>,
{
    let parsed = parse_args(budget, args)?;
    if parsed.indicative {
        eprintln!(
            "budget: running {} iterations, not the {} the acceptance criterion names; this \
             result is indicative only and must not be recorded as a gate number",
            parsed.iterations, budget.iterations
        );
    }
    let vault = ensure_vault(budget.vault)?;
    decide_with_vault(budget, parsed, vault, measure)
}

/// The measure-and-judge half of [`decide`], taking a vault rather than
/// acquiring one.
///
/// Split out for task #70: a test exercising only "does `decide` propagate a
/// measurement error rather than swallowing it" does not need a real 20,000-
/// note vault on disk to do that, and `decide`'s own shape made getting one
/// unavoidable -- `ensure_vault` ran before the measurement closure got a
/// chance to fail, so a test passing an always-`Err` closure still paid for
/// (and could race on) real vault generation. `cargo test --workspace` runs
/// every budget test binary as a separate process, several of which reach
/// `ensure_vault` for the same 20k preset; the generation stamp is written
/// only at the end, so a process finding the directory non-empty and
/// stampless mid-generation by another process hit vaultgen's own
/// non-empty-directory refusal instead of the error the test was checking
/// for. Same shape as task #28's race, one level up: shared mutable state on
/// a fixed path, concurrent execution, passes in isolation and fails under
/// load. This makes the failure structurally impossible for tests that have
/// no reason to touch disk at all, rather than merely rarer.
///
/// # Errors
/// A failed measurement, a measurement that produced no samples, or a phase
/// breakdown that does not add up.
pub fn decide_with_vault<F>(
    budget: &Budget,
    parsed: JobArgs,
    vault: VaultUnderTest,
    measure: F,
) -> Result<Outcome, String>
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<Measurement, String>,
{
    let mut measurement = measure(&vault, parsed.iterations)?;
    // F-445's second SHOULD-FIX: `decide`'s own eprintln! says this result must
    // not be recorded as a gate number, and stops there -- the `Outcome` (and
    // its `to_json()`, which is what a trend tool or a pasted report actually
    // reads back) carried no trace of it. `degraded` is the carrier task #94
    // built for exactly this shape ("real measurement, must not read as a
    // pass"), so an indicative run rides the same field a backpressure finding
    // does, rather than a warning that only ever reached stderr.
    if parsed.indicative {
        let note = format!(
            "running {} iterations, not the {} the acceptance criterion names; this result is \
             indicative only and must not be recorded as a gate number",
            parsed.iterations, budget.iterations
        );
        measurement.degraded = Some(match measurement.degraded.take() {
            Some(existing) => format!("{note}; also: {existing}"),
            None => note,
        });
    }
    outcome_from(budget, parsed.profile, vault, measurement)
}

/// Build the outcome, refusing a measurement that cannot mean anything.
///
/// # Errors
/// No samples, or phases that sum to more than the wall clock.
pub fn outcome_from(
    budget: &Budget,
    profile: Profile,
    vault: VaultUnderTest,
    measurement: Measurement,
) -> Result<Outcome, String> {
    if measurement.samples.is_empty() {
        return Err("the measurement produced no samples".to_string());
    }
    let outcome = Outcome {
        budget: *budget,
        profile,
        vault,
        environment: Environment::detect(),
        samples: measurement.samples,
        reported: measurement.reported,
        phases: measurement.phases,
        degraded: measurement.degraded,
        save_census: measurement.save_census,
    };
    outcome.check_phases()?;
    Ok(outcome)
}

/// Run one budget end to end and exit with the right code.
///
/// 0 if the budget held, 1 if it was breached or the measurement could not be
/// taken. A measurement that cannot be taken is a failure, not a skip — a
/// budget job that silently passes when the thing it measures does not exist is
/// worse than no job at all.
pub fn run_job<F>(budget: &Budget, args: &[String], measure: F) -> std::process::ExitCode
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<Measurement, String>,
{
    // Parsed here as well as inside `decide`, so `--help` prints without opening a
    // vault or taking a measurement. Parsing is pure and costs nothing; the
    // alternative is a third return shape on `decide` for the one case that
    // measures nothing.
    match parse_args(budget, args) {
        Ok(parsed) if parsed.help => {
            println!("{}", usage(budget));
            return std::process::ExitCode::SUCCESS;
        }
        Ok(_) => {}
        Err(e) => {
            // Not a FAIL of the budget: nothing was measured. Printing this as a
            // budget failure would put a breach in the log that never happened.
            eprintln!("budget: {e}");
            return std::process::ExitCode::FAILURE;
        }
    }
    // The verdict step is `job_exit`'s, not this function's (task #124,
    // F-500). What used to be here was the single conditional that decides
    // SUCCESS vs FAILURE for every gated budget, and changing it to `if true`
    // -- so no budget could ever fail CI -- left the whole suite green. It was
    // correct; nothing would have noticed it breaking. There is no decision
    // left in this function to mutate.
    job_exit(budget, decide(budget, args, measure))
}

/// Turn a finished (or failed) measurement into this process's exit code.
///
/// **Task #124, finding F-500 — this exists to be callable from a test.**
/// The conditional inside it is the whole of C-06's enforcement for every
/// gated budget: it decides `ExitCode::FAILURE` for NFR-PRF-01's Serat term,
/// NFR-PRF-03 and NFR-PRF-04 alike. While it lived inline in [`run_job`],
/// mutating it to `if true` left the suite entirely green — **F-27's own
/// finding, on the same function, still open years of findings later.**
///
/// Split out rather than tested through `run_job` because `run_job` acquires
/// the 20,000-note fixture before it reaches the decision, and a test that
/// must generate 20,000 notes to check a boolean is a test nobody runs.
///
/// An `Err` here is a measurement that could not be *taken*, which fails on
/// its own account and never reaches [`enforce`]: "reported" means a number is
/// not asserted, not that its absence is acceptable.
#[must_use]
pub fn job_exit(budget: &Budget, result: Result<Outcome, String>) -> std::process::ExitCode {
    match result {
        Ok(outcome) => {
            report(&outcome);
            // One rule, one place (F-485). This used to be a second copy of
            // the decision that did not consult `gates()` at all, so a
            // budget's report and its enforcement could disagree.
            if enforce(&outcome) {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(e) => {
            // Task #164: an unbuilt application is its own exit code and prints
            // no verdict-shaped line (nothing was measured).
            if let Some(code) = subject::absent_app_exit(&e) {
                return code;
            }
            eprintln!("FAIL  {} · {}\n  {e}", budget.requirement, budget.id);
            std::process::ExitCode::FAILURE
        }
    }
}

/// `prf01-cold-start`'s whole composition: report the two demoted aggregates,
/// then assert §3.4's scale independence.
///
/// **Task #124, finding F-501 — F-81 reopened one level further out.** F-81
/// was `run_scale_check` being droppable with nothing going red; the remedy
/// moved the measurement and the judgement into this library, and
/// [`scale_verdict`]'s own doc records the lesson — *"the pieces were tested
/// and the composition was not."* **The next level up was still unpinned:**
/// deleting the `run_scale_check` call from the binary's `main` left every
/// test green, and the comment beside that call claimed the opposite. So the
/// composition moves here too, and the binary keeps nothing a test cannot
/// reach.
///
/// See [`cold_start_job_with`] for the seam the tests drive.
#[must_use]
pub fn cold_start_job(args: &[String]) -> std::process::ExitCode {
    cold_start_job_with(
        args,
        |a| {
            run_paired_job(
                &PRF01_WINDOW_VISIBLE,
                &PRF01_VAULT_USABLE,
                a,
                measure_cold_start,
            )
        },
        run_scale_check,
    )
}

/// The one flag `prf01-cold-start` takes beyond the common set (task #187).
pub const SKIP_SCALE_FLAG: &str = "--skip-scale";

/// [`cold_start_job`] with both halves injected, so a test can assert the
/// composition without a 20,000-note fixture or a compiled `serat.exe`.
///
/// The order is the contract and is asserted by the tests: the reported pair
/// runs first; the scale check runs **only** if that step could be taken at
/// all and `--skip-scale` was not passed; and a failed scale check fails the
/// job even though the two figures it ran beside gate nothing (task #118).
#[must_use]
pub fn cold_start_job_with<R, S>(args: &[String], reported: R, scale: S) -> std::process::ExitCode
where
    R: FnOnce(&[String]) -> std::process::ExitCode,
    S: FnOnce(usize) -> std::process::ExitCode,
{
    // Task #187: `--skip-scale` is this job's own flag and the common parser (rightly)
    // refuses what it does not know, so it is stripped here, before that parse. It was
    // documented, honoured below, and unreachable: `reported(args)` got it and refused.
    let skip_scale = args.iter().any(|a| a == SKIP_SCALE_FLAG);
    let common: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != SKIP_SCALE_FLAG)
        .cloned()
        .collect();
    let reported = reported(&common);
    if reported != std::process::ExitCode::SUCCESS || skip_scale {
        return reported;
    }
    scale(PRF01_WINDOW_VISIBLE.iterations)
}

/// **The one place a measured outcome becomes a build result.**
///
/// F-485, following F-480. Before this, the rule lived twice — `run_job` had
/// its own `if outcome.passed() && outcome.determinate()` and `run_paired_job`
/// had a second copy behind an `if outcome.gates()` — and the two could
/// disagree without anything going red. **They did disagree**: `gates()` was
/// consulted in the verdict word, the report body and the JSON key, and *not*
/// in `run_job`'s decision, which is the path every gated budget actually
/// takes. That was fail-closed and therefore safe (setting `gated: false` on a
/// real gate could not disarm it), and it still meant a budget could **print
/// "not a gate" and fail CI anyway** — a figure whose report contradicts its
/// enforcement.
///
/// So the rule is written once, here, and both runners call it. The three
/// inputs are exactly the three things that decide it and there is no fourth:
///
/// * **`gated`** — from [`Budget::gated`], which is what the PRD says. An
///   ungated figure is measured, validated, printed and written to its JSON,
///   and cannot fail a build.
/// * **`passed`** — the timing comparison, with `degraded` already folded in
///   (F-435: *fail the number, not the run*).
/// * **`determinate`** — a run whose samples straddled the limit observed both
///   outcomes and settled nothing; reporting either as the verdict is a coin
///   toss dressed as a measurement.
///
/// A measurement that could not be *taken* never reaches this function: that
/// is an `Err` at the call site and fails on its own. "Reported" means the
/// number is not asserted, not that its absence is acceptable.
#[must_use]
pub fn enforce(outcome: &Outcome) -> bool {
    // `asserted()`, not `gates()`: task #166 added the one runner-dependent case, and it
    // is derived from the budget's own `ci_assertion` field and the environment, so this
    // stays one decision in one place (F-485).
    if !outcome.asserted() {
        return true;
    }
    outcome.passed() && outcome.determinate()
}

/// The body of every budget binary's `main`, with comments and whitespace
/// stripped — the last link in the chain a unit test can otherwise not reach.
///
/// **Task #124. This is where the regress stops, and it stops by argument
/// rather than by execution.** F-501 is F-69 one step further out: pin a
/// predicate and the line calling it is unpinned; pin that line and the
/// function calling it is unpinned; move the composition into the library and
/// **`main` calling the composition is still unpinned** — measured, not
/// assumed: replacing `prf01-cold-start`'s `main` body with
/// `ExitCode::SUCCESS` leaves the whole suite green.
///
/// No unit test can call a `main`. So the property asserted instead is that
/// **there is nothing in a `main` worth testing**: each one is a single
/// delegation to a library entry point. That is weaker than executing it and
/// it is honest about being weaker — what it buys is that the unpinned
/// remainder is one line long and visible in a diff, rather than a
/// composition that can quietly lose a step.
///
/// # Errors
/// The `bin/` directory or one of its files cannot be read.
pub fn budget_binary_main_bodies() -> Result<Vec<(String, String)>, String> {
    let dir = repo_root().join("bench/budget/src/bin");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading {}: {e}", dir.display()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let Some(at) = text.find("fn main(") else {
            continue;
        };
        let Some(open) = text[at..].find('{') else {
            continue;
        };
        let body: String = text[at + open + 1..]
            .lines()
            .map(str::trim)
            .take_while(|l| *l != "}")
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .collect::<Vec<_>>()
            .join(" ");
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        out.push((name, body));
    }
    out.sort();
    Ok(out)
}

/// Every budget id that a job enforcing through [`enforce`] can breach a build
/// with — read off the binaries rather than restated (F-485).
///
/// The gap F-485 names is not that enforcement is wrong today; it is that
/// **nothing pinned the wiring**. `PRF01_REPORTED`'s entries are ungated, and
/// a sixth budget added to that list but wired to a `run_job`-calling binary
/// would print *"not a gate"* in its own report and still fail CI on a breach.
/// The partition test pins flag against list; this pins **list against the
/// binaries that actually run**, which is the half that was missing.
///
/// # Errors
/// The `bin/` directory cannot be read.
pub fn budgets_enforced_by_a_binary() -> Result<Vec<String>, String> {
    let dir = repo_root().join("bench/budget/src/bin");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading {}: {e}", dir.display()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        // `run_paired_job` contains `run_job` as a substring, so the paired
        // form has to be excluded explicitly rather than matched loosely --
        // exactly the kind of near-miss F-30 recorded when a grep decided a
        // check's coverage.
        let calls_run_job = text.contains("run_job(") && !text.contains("run_paired_job(");
        if !calls_run_job {
            continue;
        }
        for budget in P0_BUDGETS.iter().chain(PRF01_REPORTED) {
            // `prf03-save-to-index` -> `PRF03_SAVE_TO_INDEX`, which is how
            // every one of these constants is named. Derived rather than
            // listed, so a new budget cannot be missed by a table nobody
            // updated.
            let konst = budget.id.to_uppercase().replace('-', "_");
            if text.contains(&konst) {
                ids.push(budget.id.to_string());
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// Run two budgets that share one measurement, as NFR-PRF-01 does.
///
/// Both are reported; the job fails if either is breached. "Cold start failed"
/// without saying which half is a bug report nobody can act on.
pub fn run_paired_job<F>(
    first: &Budget,
    second: &Budget,
    args: &[String],
    measure: F,
) -> std::process::ExitCode
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<(Measurement, Measurement), String>,
{
    let parsed = match parse_args(first, args) {
        Ok(p) if p.help => {
            println!("{}", usage(first));
            return std::process::ExitCode::SUCCESS;
        }
        Ok(p) => p,
        Err(e) => {
            eprintln!("budget: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let vault = match ensure_vault(first.vault) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL  {} · {}\n  {e}", first.requirement, first.id);
            return std::process::ExitCode::FAILURE;
        }
    };
    let (a, b) = match measure(&vault, parsed.iterations) {
        Ok(pair) => pair,
        Err(e) => {
            if let Some(code) = subject::absent_app_exit(&e) {
                return code;
            }
            eprintln!(
                "FAIL  {} · {} and {}\n  {e}",
                first.requirement, first.id, second.id
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut ok = true;
    for (budget, measurement) in [(first, a), (second, b)] {
        match outcome_from(budget, parsed.profile, vault.clone(), measurement) {
            Ok(outcome) => {
                report(&outcome);
                // The same `enforce` `run_job` calls -- not a second copy of
                // the rule that happens to agree with it (F-485).
                ok &= enforce(&outcome);
            }
            Err(e) => {
                eprintln!("FAIL  {} · {}\n  {e}", budget.requirement, budget.id);
                ok = false;
            }
        }
    }
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// The median of a list of durations.
///
/// One definition, used by the phase breakdown and agreeing with
/// `Samples::median()` by construction — P0 finding F-25 noted two definitions
/// of median in the crate with nothing holding them together.
pub fn median_duration(mut xs: Vec<Duration>) -> Duration {
    if xs.is_empty() {
        return Duration::ZERO;
    }
    xs.sort_unstable();
    // Nearest rank at q = 0.5, matching Samples::percentile exactly: at n = 3
    // the middle sample, never the fastest.
    let rank = ((0.5 * xs.len() as f64).ceil().max(1.0) as usize).min(xs.len());
    xs[rank - 1]
}

/// NFR-PRF-04's measurement: `iterations` rebuilds into a fresh index each time.
///
/// Lives in the library rather than in the binary because P0 finding F-27 was
/// four mutations no test noticed, and every one of them was in a job binary
/// that nothing covered. The logic under test is now the logic that runs.
///
/// # Errors
/// Any rebuild failure, or a rebuild whose note count disagrees with the vault
/// manifest.
pub fn measure_full_rebuild(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<Measurement, String> {
    measure_full_rebuild_with(
        vault,
        iterations,
        subject::full_rebuild,
        subject::stat1_note_rows,
    )
}

/// As [`measure_full_rebuild`], against an injected rebuild.
///
/// The loop — fresh index per iteration, the note-count guard, the per-phase
/// median — is the part worth testing, and it cannot be tested through the real
/// seam because the real seam does not exist yet. A mutation sweep found the
/// note-count guard survived precisely because nothing exercised this function.
///
/// # Errors
/// As [`measure_full_rebuild`].
pub fn measure_full_rebuild_with<F, A>(
    vault: &VaultUnderTest,
    iterations: usize,
    mut rebuild: F,
    mut analyze: A,
) -> Result<Measurement, String>
where
    F: FnMut(&Path, &Path) -> Result<subject::RebuildTiming, String>,
    A: FnMut(&Path) -> Result<usize, String>,
{
    let mut samples = Samples::new();
    let mut runs: Vec<subject::RebuildTiming> = Vec::with_capacity(iterations);
    for i in 0..iterations {
        // A fresh index directory per iteration: a rebuild over a warm index is
        // not a rebuild, and reusing the directory would measure the second run
        // of something the requirement describes as the first.
        //
        // Outside the vault, deliberately (F-372, task #78 -- the same lesson
        // task #68's `measure_the_open_check_on_the_20k_vault` learned first,
        // one file over in subject.rs). This used to be
        // `vault.path.join(format!(".serat-bench-{i}"))`: a run that died
        // between here and the cleanup below left `.serat-bench-0/` sitting
        // inside the fixture vault, which moved its tree digest and failed
        // every canonicality check after it -- for every caller, not just this
        // one -- with a message that named neither the cause nor the one-line
        // fix. `temp_dir()` carries no such obligation: nothing this loop does
        // can poison the vault, cleanup or no cleanup.
        let index_dir =
            std::env::temp_dir().join(format!("serat-bench-rebuild-{}-{i}", std::process::id()));
        let _ = std::fs::remove_dir_all(&index_dir);
        let timing = rebuild(&vault.path, &index_dir)?;
        // Read before the directory goes away. F-47 is only visible here.
        let recorded = analyze(&index_dir);
        // Still removed before the checks below, so a slow disk doesn't carry
        // 20,000 notes' worth of index across iterations -- but no longer load-
        // bearing for the vault's own integrity the way it was at F-311.
        let _ = std::fs::remove_dir_all(&index_dir);
        // F-247, test-engineer's: `RebuildReport::notes` counts every row in
        // `notes` -- markdown notes AND assets alike, `kind='asset'` per
        // ADR-0005 -- so the oracle has to be the same sum, or a correct rebuild
        // of any vault containing assets is rejected as a partial index every
        // time. Confirmed live: the first wired run reported 20,200 against an
        // oracle of 20,000 and was refused.
        check_note_count(timing.notes, vault.notes + vault.assets)?;
        check_links_resolved(
            timing.links_resolved,
            vault.note_links + vault.asset_embeds + vault.asset_md_links,
        )?;
        // The SAME sum, for the same reason: `sqlite_stat1`'s row count for
        // `notes` covers assets too, because they live in that table. F-247
        // corrected the note-count oracle above and left this one on `notes`
        // alone, and the unit test's stub matched the wrong number, so the
        // suite stayed green until the first real run (F-321).
        check_analyze_freshness(recorded?, vault.notes + vault.assets)?;
        samples.push(timing.total);
        runs.push(timing);
    }
    // Per-phase median across runs, not the phases of the median run: one slow
    // run should not decide where the time went.
    let phases = subject::RebuildPhases {
        walk: median_duration(runs.iter().map(|r| r.phases.walk).collect()),
        read_hash: median_duration(runs.iter().map(|r| r.phases.read_hash).collect()),
        parse: median_duration(runs.iter().map(|r| r.phases.parse).collect()),
        producer_wait: median_duration(runs.iter().map(|r| r.phases.producer_wait).collect()),
        write_fts: median_duration(runs.iter().map(|r| r.phases.write_fts).collect()),
    };
    Ok(Measurement::new(samples).with_phases(phases))
}

/// NFR-PRF-03's measurement: 1,000 distinct saves through the real
/// application (task #94, following #111).
///
/// # Errors
/// The application could not be driven, [`check_save_completeness`] refused
/// the run (`handle_missing` non-zero, or the counted outcomes did not sum to
/// what was attempted), or the measurement produced no samples.
pub fn measure_save_to_index(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<Measurement, String> {
    measure_save_to_index_with(vault, iterations, subject::save_to_index_via_app)
}

/// As [`measure_save_to_index`], against an injected save run.
///
/// Split out for the same reason [`measure_full_rebuild_with`] is: the loop
/// worth testing here — the completeness check, and the writer-refused
/// bookkeeping that turns a non-zero count into a [`Measurement::degraded`]
/// reason rather than a silent pass — cannot be exercised through the real
/// seam without a compiled `serat.exe` and a real vault on disk.
///
/// # Errors
/// As [`measure_save_to_index`].
pub fn measure_save_to_index_with<F>(
    vault: &VaultUnderTest,
    iterations: usize,
    save_to_index: F,
) -> Result<Measurement, String>
where
    F: FnOnce(&Path, usize) -> Result<trace::SaveBenchmarkTrace, String>,
{
    let trace = save_to_index(&vault.path, iterations)?;
    check_save_completeness(&trace)?;

    let mut samples = Samples::new();
    for ms in &trace.submitted_ms {
        samples.push(Duration::from_secs_f64(ms / 1000.0));
    }
    // The census goes on before any branch below can return: the lead's
    // condition on `prewarm_index` is that `handle_missing` is reported on
    // every run, not only on the runs that went wrong (task #94, F-450).
    // `check_save_completeness` above has already refused a non-zero
    // `handle_missing`, so what this carries is the zero that says the
    // pre-warm closed the window -- #119's answer from the other direction.
    let mut measurement = Measurement::new(samples).counting(SaveCensus {
        attempted: trace.attempted,
        submitted: trace.submitted_ms.len(),
        writer_refused: trace.writer_refused,
        handle_missing: trace.handle_missing,
    });

    if trace.writer_refused > 0 {
        let mut refused = Samples::new();
        for ms in &trace.writer_refused_ms {
            refused.push(Duration::from_secs_f64(ms / 1000.0));
        }
        measurement = measurement
            .reporting(
                "writer-refused",
                "ADR-0011's sanctioned backpressure -- excluded from the asserted p95 because a \
                 refused save's near-instant timing would deflate the series rather than \
                 represent it (F-19, F-382).",
                refused,
            )
            .degraded(format!(
                "{} of {} attempted saves were refused by the writer (ADR-0011 backpressure). \
                 NFR-PRF-03's own stated workload of {} saves is enough to make the writer \
                 refuse on this vault -- a finding about the queue, not about this harness \
                 (F-421) -- and it fails this run's number regardless of what the {} landed \
                 saves' own p95 says.",
                trace.writer_refused,
                trace.attempted,
                trace.attempted,
                trace.submitted_ms.len()
            ));
    }

    Ok(measurement)
}

/// One spawn's contribution to the scale check (task #118).
///
/// Three series come off the same trace, and which one the check *asserts*
/// moved in #118: architecture §3.4's ruling puts the assertion on the Serat
/// term and keeps the two pre-split aggregates measured and printed. Carrying
/// all three from one spawn is what makes that possible without spawning
/// twice -- the aggregates are a second reduction of the same batch, never a
/// second measurement (F-446's rule, applied here).
#[derive(Clone, Copy, Debug)]
pub struct ScaleSample {
    /// ADR-0014 §Decision 3's binding sum, `S_pre + S_setup + S_shell +
    /// S_settle`. **The asserted series since task #118.**
    pub serat_term_ms: f64,
    /// The pre-split window-painted aggregate. Reported, never asserted.
    pub window_visible_ms: f64,
    /// The pre-split vault-usable aggregate. Reported, never asserted.
    pub vault_usable_ms: f64,
}

impl ScaleSample {
    /// A sample whose three series are the same number.
    ///
    /// For tests about the pairing, the drift cancellation or the judgement --
    /// none of which are about *which* series is the subject. A test that is
    /// about the subject sets the three fields apart, and one of those is
    /// below.
    #[must_use]
    pub fn flat(ms: f64) -> ScaleSample {
        ScaleSample {
            serat_term_ms: ms,
            window_visible_ms: ms,
            vault_usable_ms: ms,
        }
    }
}

/// A 20k-vs-50k ratio that is measured and printed and never asserted
/// (task #118).
///
/// §3.4's ruling keeps the aggregates in the output because contention is one
/// channel of vault-size dependence that lands in `P_webview`, which `S`
/// excludes by construction -- so dropping them would lose a signal, while
/// asserting them is what F-407 says cannot decide a pass.
#[derive(Clone, Copy, Debug)]
pub struct ScaleRatio {
    /// What it is, e.g. `window painted`.
    pub label: &'static str,
    /// p95 at the 20k vault.
    pub small_p95_ms: f64,
    /// p95 at the 50k vault.
    pub large_p95_ms: f64,
}

impl ScaleRatio {
    /// Growth from the small arm to the large one, as a percentage.
    ///
    /// Zero rather than infinity when the small arm is zero: this figure is
    /// printed, never compared, so a degenerate input must not panic or emit
    /// `inf` into a report someone pastes into a gate document.
    #[must_use]
    pub fn growth_pct(&self) -> f64 {
        if self.small_p95_ms <= 0.0 {
            return 0.0;
        }
        (self.large_p95_ms / self.small_p95_ms - 1.0) * 100.0
    }
}

/// What index state both arms of a scale check started from (task #118).
///
/// §3.4's ruling makes this a precondition rather than a detail: `S_settle` is
/// inside the compared quantity, so a `.serat/` present on one arm and absent
/// on the other measures fixture preparation rather than code. The existing
/// [`subject::ColdStartVaultGuard`] equalises *cleanup* and says nothing about
/// the starting state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexStartState {
    /// Neither vault had a `.serat/`. The first spawn on each arm starts a
    /// rebuild that outlives the process it was spawned in (F-450), so the
    /// arms are comparable to each other and no figure here is a warm one.
    ColdBoth,
    /// Both vaults had a `.serat/` before the first spawn.
    WarmBoth,
}

impl IndexStartState {
    /// How it prints in the verdict line.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            IndexStartState::ColdBoth => {
                "both arms started COLD (no .serat/): every arm pays a rebuild that outlives \
                 its spawn (F-450), so this compares two cold starts and not two warm ones"
            }
            IndexStartState::WarmBoth => "both arms started warm (.serat/ present)",
        }
    }
}

/// Check that both arms enter the batch in the same index state, and say which.
///
/// §3.4's task #118 ruling, as a refusal rather than a caveat: an unequal
/// start does not weaken the comparison, it replaces it with a measurement of
/// which fixture happened to be warm.
///
/// # Errors
/// The two arms disagree.
pub fn equal_index_start(small: &Path, large: &Path) -> Result<IndexStartState, String> {
    let small_warm = small.join(".serat").exists();
    let large_warm = large.join(".serat").exists();
    match (small_warm, large_warm) {
        (true, true) => Ok(IndexStartState::WarmBoth),
        (false, false) => Ok(IndexStartState::ColdBoth),
        _ => Err(format!(
            "the 20k arm started {} and the 50k arm started {}. S_settle is inside the \
             compared quantity, so unequal starting index state measures which fixture was \
             warm rather than whether the critical path grew with the vault (architecture \
             §3.4, task #118). Delete both .serat/ directories, or pre-build both, and re-run.",
            if small_warm { "warm" } else { "cold" },
            if large_warm { "warm" } else { "cold" },
        )),
    }
}

/// The asserted mark at two vault sizes, measured in interleaved pairs.
#[derive(Clone, Copy, Debug)]
pub struct ScaleCheck {
    /// Median at the 20k vault.
    pub small_ms: f64,
    /// Median at the 50k vault.
    pub large_ms: f64,
    /// p95 at the 20k vault. **This is what the judgement compares**, because
    /// it is what the budget itself asserts.
    pub small_p95_ms: f64,
    /// p95 at the 50k vault.
    pub large_p95_ms: f64,
    /// How many pairs were taken.
    pub pairs: usize,
    /// **Median** of `large - small` within each pair.
    ///
    /// A rank statistic rather than a mean, because these distributions are
    /// heavy-tailed and a mean under-detects: in the run that produced F-141,
    /// the mean's confidence interval included zero while the sign test gave
    /// p ≈ 0.04 and the p95 ratio was 1.20. A mean would have called a real
    /// effect noise.
    pub median_diff_ms: f64,
    /// How many pairs had the larger vault slower.
    pub larger_slower: usize,
    /// Two-sided sign-test p for that count against a fair coin.
    pub sign_test_p: f64,
    /// What index state both arms entered with (task #118's precondition).
    pub index_start: IndexStartState,
    /// The 20k arm's own spread, `(max - min) / min`, as a percentage.
    ///
    /// §3.4's ruling requires this beside the verdict rather than in a later
    /// investigation: the 20% tolerance was sized against the *effect* and
    /// never against the noise of the series it is asserted on, and the
    /// subject has just changed. If the spread is not comfortably inside the
    /// tolerance, that is the finding, and it is escalated rather than
    /// repaired by widening (ADR-0014 §Decision 5's floor rule).
    pub small_spread_pct: f64,
    /// The 50k arm's own spread, by the same definition.
    pub large_spread_pct: f64,
    /// The pre-split window-painted ratio. Printed, never asserted.
    pub window_visible: ScaleRatio,
    /// The pre-split vault-usable ratio. Printed, never asserted.
    pub vault_usable: ScaleRatio,
}

/// Two-sided sign test: the probability of a split at least this lopsided if
/// the two arms were exchangeable.
///
/// Exact rather than normal-approximated, since the harness runs tens of pairs
/// rather than hundreds.
pub fn sign_test_p(pairs: usize, successes: usize) -> f64 {
    if pairs == 0 {
        return 1.0;
    }
    let n = pairs;
    let k = successes.min(n - successes);
    // Sum the two tails: P(X <= k) + P(X >= n-k), which for a symmetric
    // binomial is 2 * P(X <= k), capped at 1.
    let mut tail = 0.0f64;
    let mut coeff = 1.0f64; // C(n, 0)
    for i in 0..=k {
        if i > 0 {
            coeff = coeff * ((n - i + 1) as f64) / (i as f64);
        }
        tail += coeff;
    }
    let total = 2.0f64.powi(n as i32);
    ((2.0 * tail) / total).min(1.0)
}

/// Measure the window-painted mark at both vault sizes, **interleaved**.
///
/// Lifted out of `prf01-cold-start.rs` because P0 finding **F-81** was
/// `assert_scale_independent` being well tested while its only caller — a
/// function living in a binary — could be short-circuited with nothing going
/// red. The guard was tested and nothing asked it.
///
/// The sampling is interleaved rather than blocked because of **F-109/F-123**.
/// The within-arm spread on the reference machine is about five times the
/// effect being tested for, so two blocks of twenty are contaminated by
/// whatever the machine was doing during each block — which is how a
/// 20,000-note vault measured *faster* than a 2-note one. Alternating the two
/// arms spawn by spawn, and alternating which goes first, puts slow drift on
/// both arms so it cancels in the paired difference. Same number of spawns,
/// strictly more information: at 30 pairs the paired 95% CI came out
/// [-123, +45] ms against a 179 ms tolerance, which decides the claim that
/// blocked sampling could not.
///
/// Medians rather than p95 for the ratio: a p95 over twenty samples is a noisier
/// estimator than the quantity it estimates. The *budget* stays a p95, because
/// that is what NFR-PRF-01 says; only this comparison uses the median.
///
/// # Errors
/// Any failure to take a measurement.
pub fn measure_scale_independence<F>(
    small_vault: &VaultUnderTest,
    large_vault: &VaultUnderTest,
    pairs: usize,
    index_start: IndexStartState,
    mut measure_one: F,
) -> Result<ScaleCheck, String>
where
    F: FnMut(&VaultUnderTest, usize) -> Result<ScaleSample, String>,
{
    // The vaults are arguments rather than fetched here, so a test can drive
    // this without generating 98 MiB and hashing it. `ensure_vault` has its own
    // tests, in tests/vault_gate.rs, against a real vault on disk.
    if pairs == 0 {
        return Err("a scale check needs at least one pair".to_string());
    }
    let mut small = Vec::with_capacity(pairs);
    let mut large = Vec::with_capacity(pairs);
    let mut diffs = Vec::with_capacity(pairs);

    for i in 0..pairs {
        // Alternate which arm goes first, so any within-pair ordering effect —
        // a warm page cache, a thermal ramp — falls on both arms equally.
        let (a, b) = if i % 2 == 0 {
            (small_vault, large_vault)
        } else {
            (large_vault, small_vault)
        };
        let first = measure_one(a, i)?;
        let second = measure_one(b, i)?;
        let (s, l) = if i % 2 == 0 {
            (first, second)
        } else {
            (second, first)
        };
        // Task #118: the paired difference is taken on the **asserted** series
        // only. The aggregates are reduced to a ratio of p95s below and never
        // enter the sign test, because a signal nothing asserts must not be
        // able to move a verdict by the back door.
        diffs.push(l.serat_term_ms - s.serat_term_ms);
        small.push(s);
        large.push(l);
    }

    let pick =
        |xs: &[ScaleSample], f: fn(&ScaleSample) -> f64| -> Vec<f64> { xs.iter().map(f).collect() };
    let small_serat = pick(&small, |x| x.serat_term_ms);
    let large_serat = pick(&large, |x| x.serat_term_ms);

    let larger_slower = diffs.iter().filter(|d| **d > 0.0).count();
    Ok(ScaleCheck {
        small_ms: median_of(&small_serat),
        large_ms: median_of(&large_serat),
        small_p95_ms: percentile_of(&small_serat, 0.95),
        large_p95_ms: percentile_of(&large_serat, 0.95),
        pairs,
        median_diff_ms: median_of(&diffs),
        larger_slower,
        sign_test_p: sign_test_p(pairs, larger_slower),
        index_start,
        small_spread_pct: spread_pct_of(&small_serat),
        large_spread_pct: spread_pct_of(&large_serat),
        window_visible: ScaleRatio {
            label: "window painted",
            small_p95_ms: percentile_of(&pick(&small, |x| x.window_visible_ms), 0.95),
            large_p95_ms: percentile_of(&pick(&large, |x| x.window_visible_ms), 0.95),
        },
        vault_usable: ScaleRatio {
            label: "vault usable",
            small_p95_ms: percentile_of(&pick(&small, |x| x.vault_usable_ms), 0.95),
            large_p95_ms: percentile_of(&pick(&large, |x| x.vault_usable_ms), 0.95),
        },
    })
}

/// `(max - min) / min` as a percentage, the same definition
/// [`Outcome::spread_pct`] uses, so the two cannot drift apart.
fn spread_pct_of(xs: &[f64]) -> f64 {
    let min = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !min.is_finite() || min <= 0.0 {
        return 0.0;
    }
    (max - min) / min * 100.0
}

/// Nearest-rank percentile of a slice of milliseconds, matching
/// `Samples::percentile` exactly so the two cannot drift apart.
fn percentile_of(xs: &[f64], q: f64) -> f64 {
    let mut sorted = xs.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((q * sorted.len() as f64).ceil().max(1.0) as usize).min(sorted.len());
    sorted[rank - 1]
}

/// Nearest-rank median, matching `Samples::median`.
fn median_of(xs: &[f64]) -> f64 {
    percentile_of(xs, 0.50)
}

/// Turn a scale measurement into the line a gate report carries.
///
/// # Errors
/// When the critical path grew with the vault.
pub fn judge_scale(check: &ScaleCheck) -> Result<String, String> {
    // The comparison is on p95, because that is the statistic the budget
    // asserts. The median and the sign test are reported beside it: they are
    // the sensitive detectors, and the p95 is the one that means something
    // against NFR-PRF-01. Judging on the mean would under-detect on these
    // heavy-tailed distributions, and judging on the median alone would assert
    // something the budget does not care about.
    assert_scale_independent(check.small_p95_ms, check.large_p95_ms)?;
    Ok(format!(
        "PASS  NFR-PRF-01 · scale independence\n  subject     the Serat term (ADR-0014 \
         §Decision 3's S_pre+S_setup+S_shell+S_settle) — task #118 moved the assertion here \
         from window_visible_ms; §3.4 has the five grounds\n  Serat term p95 {:.1} ms at 20k, \
         {:.1} ms at 50k over {} interleaved pairs (median {:.1} / {:.1})\n  paired median \
         difference {:+.1} ms, larger vault slower in {}/{} pairs, sign test p = {:.3} — vault \
         size does not inflate the mark beyond the {:.0}% tolerance (§3.4). This bounds the \
         effect; it does not claim there is none.\n  spread      the asserted series moved \
         {:.1}% within the 20k arm and {:.1}% within the 50k arm against a {:.0}% tolerance. \
         §3.4 requires this beside the verdict because the tolerance was sized against the \
         effect and never against this series' noise; a spread not comfortably inside it is a \
         finding to escalate, not a tolerance to widen.\n  reported    {} p95 {:.1} → {:.1} ms \
         ({:+.1}%) · {} p95 {:.1} → {:.1} ms ({:+.1}%) — measured, printed, asserted on \
         nothing (F-407, task #118)\n  index state {}\n  caveat: every iteration reads a \
         page-cached vault, so no figure here is a cold-VAULT figure",
        check.small_p95_ms,
        check.large_p95_ms,
        check.pairs,
        check.small_ms,
        check.large_ms,
        check.median_diff_ms,
        check.larger_slower,
        check.pairs,
        check.sign_test_p,
        SCALE_TOLERANCE * 100.0,
        check.small_spread_pct,
        check.large_spread_pct,
        SCALE_TOLERANCE * 100.0,
        check.window_visible.label,
        check.window_visible.small_p95_ms,
        check.window_visible.large_p95_ms,
        check.window_visible.growth_pct(),
        check.vault_usable.label,
        check.vault_usable.small_p95_ms,
        check.vault_usable.large_p95_ms,
        check.vault_usable.growth_pct(),
        check.index_start.describe(),
    ))
}

/// Measure both vaults and judge the result, as one composable step.
///
/// Split from [`run_scale_check`] because a mutation that stopped
/// [`judge_scale`] being called at all survived the sweep: the pieces were
/// tested and the composition was not. This is F-69 one level further out than
/// F-81 found it, and the same fix applies — everything but the I/O and the
/// printing lives where a test can drive it.
///
/// # Errors
/// A vault that cannot be prepared, a measurement that fails, or a critical
/// path that grew with the vault.
pub fn scale_verdict<V, F>(
    iterations: usize,
    index_start: IndexStartState,
    vaults: V,
    measure: F,
) -> Result<String, String>
where
    V: FnOnce() -> Result<(VaultUnderTest, VaultUnderTest), String>,
    F: FnMut(&VaultUnderTest, usize) -> Result<ScaleSample, String>,
{
    let (small, large) = vaults()?;
    let check = measure_scale_independence(&small, &large, iterations, index_start, measure)?;
    judge_scale(&check)
}

/// Architecture §3.4's load-bearing claim, measured and judged.
///
/// Returns SUCCESS if it held (having printed the verdict either way), FAILURE if it
/// did not, or `EXIT_APP_ABSENT` if the application is not built (no verdict printed).
pub fn run_scale_check(iterations: usize) -> std::process::ExitCode {
    // F-401: this check calls `cold_start_once` directly, spawn by spawn,
    // interleaved across both vaults (the doc comment on `cold_start_once`
    // explains why) -- so no single call is "the batch" the way it is inside
    // `cold_start`. The batch is the whole check, both vaults, every pair, and
    // the guards below are taken before the first spawn and held across all of
    // them, dropping only once `scale_verdict` has returned. Vaults are
    // resolved before `scale_verdict` runs specifically so the guards can be
    // taken on their paths first; `ensure_vault` errors surface through the
    // same `?` a bare fetch would have used.
    let vaults = (|| -> Result<(VaultUnderTest, VaultUnderTest), String> {
        Ok((ensure_vault("20k")?, ensure_vault("50k")?))
    })();
    let verdict = vaults.and_then(|(small, large)| {
        // Task #118's precondition, read before the guards and before the
        // first spawn: `S_settle` is inside the asserted quantity, so the two
        // arms must enter in the same index state and the run must say which
        // (§3.4). Checked here rather than inside `measure_scale_independence`
        // because it is a fact about the fixtures on disk, and that function
        // deliberately takes vaults it never touches.
        let index_start = equal_index_start(&small.path, &large.path)?;
        let _small_guard = subject::ColdStartVaultGuard::watch(&small.path);
        let _large_guard = subject::ColdStartVaultGuard::watch(&large.path);
        scale_verdict(
            iterations,
            index_start,
            || Ok((small, large)),
            |vault, run| {
                // Task #118: three series off one spawn. The Serat term is
                // asserted; the two aggregates are reported beside it. Same
                // trace, same validation, no second measurement (F-446).
                subject::cold_start_once(&vault.path, run).map(|trace| {
                    let terms = trace.terms();
                    ScaleSample {
                        serat_term_ms: terms.serat_term_ms(),
                        window_visible_ms: trace.window_visible_ms,
                        vault_usable_ms: trace.vault_usable_ms,
                    }
                })
            },
        )
    });
    match verdict {
        Ok(line) => {
            println!("{line}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            // An unbuilt application passes through as its own exit code (3).
            if let Some(code) = subject::absent_app_exit(&e) {
                return code;
            }
            println!("FAIL  NFR-PRF-01 · scale independence\n  {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// NFR-PRF-01's measurement: two marks from one set of cold starts.
///
/// # Errors
/// Any failure to start the application or to validate a trace.
pub fn measure_cold_start(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<(Measurement, Measurement), String> {
    let (visible, usable) = subject::cold_start(&vault.path, iterations)?;
    Ok((Measurement::new(visible), Measurement::new(usable)))
}

/// NFR-PRF-01's binding measurement since the #104 re-baseline (task #107):
/// the Serat term alone is asserted; the platform term and both legacy marks
/// are reported beside it and never asserted (F-407 — the aggregate moves
/// ~19% batch-to-batch against ~8% for the Serat term, so it cannot decide a
/// pass at any budget).
///
/// Reuses [`subject::cold_start_traces`] — the same guarded batch, the same
/// `--exit-after-ready` spawn, the same six-rejection validation in
/// [`trace`] that [`measure_cold_start`] and `prf01-cold-start-terms`
/// (task #100's diagnostic) already read — so this is a second reduction of
/// the same data, not a second measurement method.
///
/// # Errors
/// As [`measure_cold_start`].
pub fn measure_serat_term(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<Measurement, String> {
    measure_serat_term_with(vault, iterations, subject::cold_start_traces)
}

/// As [`measure_serat_term`], against an injected batch of traces.
///
/// Split out for the same reason [`measure_save_to_index_with`] is: the
/// reduction from seven marks to four reported series is the logic worth
/// testing, and exercising it through the real seam needs a compiled
/// `serat.exe` and a real vault on disk.
///
/// # Errors
/// As [`measure_serat_term`].
pub fn measure_serat_term_with<F>(
    vault: &VaultUnderTest,
    iterations: usize,
    cold_start_traces: F,
) -> Result<Measurement, String>
where
    F: FnOnce(&Path, usize) -> Result<Vec<trace::StartupTrace>, String>,
{
    let traces = cold_start_traces(&vault.path, iterations)?;

    let mut serat_term = Samples::new();
    let mut platform_term = Samples::new();
    let mut window_visible = Samples::new();
    let mut vault_usable = Samples::new();
    let mut tree_first_screen = Samples::new();
    for t in &traces {
        let terms = t.terms();
        if let Some(ms) = t.tree_first_screen_ms {
            tree_first_screen.push(Duration::from_secs_f64(ms / 1000.0));
        }
        serat_term.push(Duration::from_secs_f64(terms.serat_term_ms() / 1000.0));
        platform_term.push(Duration::from_secs_f64(terms.platform_term_ms() / 1000.0));
        window_visible.push(t.window_visible());
        vault_usable.push(t.vault_usable());
    }

    let measurement = Measurement::new(serat_term)
        .reporting(
            "platform-term",
            "P_load + P_builder + P_webview -- unbudgeted; no change confined to this \
             repository moves it (ADR-0014 Decision 3).",
            platform_term,
        )
        .reporting(
            "window-visible (legacy total)",
            "The pre-split aggregate PRF01_WINDOW_VISIBLE names. Reported, not gated here \
             (F-407: moves ~19% batch-to-batch against ~8% for the Serat term, so it cannot \
             decide a pass at any budget). Still asserted by prf01-cold-start until task #118.",
            window_visible,
        )
        .reporting(
            "vault-usable (legacy total)",
            "The pre-split aggregate PRF01_VAULT_USABLE names. Reported, not gated here -- \
             same reason as window-visible, and the same #118 caveat.",
            vault_usable,
        );
    // Task #141: the tree term of t_usable (ADR-0015 section 5), reported beside it. Only
    // builds that declare a tree carry it; a build that declares one and omits the number
    // never reaches here (the trace is refused at parse).
    Ok(if tree_first_screen.is_empty() {
        measurement
    } else {
        measurement.reporting(
            "tree-first-screen",
            "t0 to the root listing applied and painted (ADR-0015 section 3): the tree term \
             of vault-usable, which is the latest of paint, tree first screen, index settled \
             and watcher. Reported, not gated on its own (NFR-PRF-01 budgets the total).",
            tree_first_screen,
        )
    })
}

/// Print an outcome and write its result file.
pub fn report(outcome: &Outcome) {
    print!("{}", outcome.to_report());
    match write_result(outcome) {
        Ok(p) => println!("  result      {}", p.display()),
        Err(e) => eprintln!("budget: could not write the result file: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(ms: &[f64]) -> Samples {
        let mut s = Samples::new();
        for v in ms {
            s.push(Duration::from_secs_f64(v / 1000.0));
        }
        s
    }

    #[test]
    fn nearest_rank_p95_over_twenty_runs_allows_exactly_one_slow_run() {
        // NFR-PRF-01 says "over 20 runs; p95 within budget". With nearest rank
        // that is the 19th of 20 sorted samples: one outlier passes, two do not.
        // An interpolating percentile would let two through, which is not what
        // the requirement means, so the choice is asserted rather than assumed.
        let mut ms = vec![100.0; 19];
        ms.push(9_999.0);
        assert_eq!(samples(&ms).percentile(0.95), 100.0);

        let mut ms = vec![100.0; 18];
        ms.push(9_999.0);
        ms.push(9_999.0);
        assert_eq!(samples(&ms).percentile(0.95), 9_999.0);
    }

    #[test]
    fn percentiles_are_order_independent() {
        let a = samples(&[5.0, 1.0, 4.0, 2.0, 3.0]);
        let b = samples(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(a.percentile(0.5), b.percentile(0.5));
        assert_eq!(a.percentile(0.95), b.percentile(0.95));
        assert_eq!(a.max(), 5.0);
        assert_eq!(a.min(), 1.0);
        assert_eq!(a.mean(), 3.0);
    }

    #[test]
    fn a_breach_is_a_failure_and_the_boundary_passes() {
        let vault = VaultUnderTest {
            path: PathBuf::from("x"),
            digest: "d".into(),
            format_version: 1,
            notes: 20_000,
            note_links: 216_971,
            assets: 200,
            asset_embeds: 2_004,
            asset_md_links: 1_392,
        };
        let at_limit = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: vault.clone(),
            environment: Environment::default(),
            samples: samples(&[30_000.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(
            at_limit.passed(),
            "a value exactly at the limit is within it"
        );

        let over = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault,
            environment: Environment::default(),
            samples: samples(&[30_001.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(!over.passed());
        assert!(over.to_report().starts_with("FAIL"));
        assert!(over.to_json().contains("\"passed\": false"));
    }

    // --------------------------------------------------------------- #94

    #[test]
    fn a_degraded_outcome_fails_regardless_of_the_timing() {
        // "fail the number, not the run" (task #94): a real, honestly measured
        // Outcome whose own p95 clears the budget by a wide margin must still
        // report FAIL when `degraded` is set -- that is the whole point of the
        // field existing rather than folding this into `samples`.
        let outcome = Outcome {
            budget: PRF03_SAVE_TO_INDEX,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[1.0, 2.0, 3.0]),
            reported: Vec::new(),
            phases: None,
            degraded: Some("3 of 1000 attempted saves were refused by the writer".to_string()),
            save_census: None,
        };
        assert!(
            outcome.value_ms() <= outcome.limit_ms(),
            "the timing alone would pass -- that is what makes this a meaningful test"
        );
        assert!(!outcome.passed(), "degraded must override a passing timing");
        assert!(
            outcome.to_report().starts_with("FAIL"),
            "{}",
            outcome.to_report()
        );
        assert!(
            outcome.to_report().contains("DEGRADED"),
            "the reason must be visible in the pasted report: {}",
            outcome.to_report()
        );
        assert!(
            outcome
                .to_report()
                .contains("3 of 1000 attempted saves were refused"),
            "{}",
            outcome.to_report()
        );
        assert!(
            outcome
                .to_json()
                .contains("\"degraded\": \"3 of 1000 attempted saves were refused by the writer\""),
            "{}",
            outcome.to_json()
        );
        assert!(
            outcome.to_json().contains("\"passed\": false"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn an_undegraded_outcome_reports_a_null_reason() {
        let outcome = Outcome {
            budget: PRF03_SAVE_TO_INDEX,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[1.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(
            !outcome.to_report().contains("DEGRADED"),
            "{}",
            outcome.to_report()
        );
        assert!(
            outcome.to_json().contains("\"degraded\": null"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn every_p0_budget_constant_is_pinned_against_drift_within_this_file() {
        // **F-447: this test does not read the PRD, and its old name said it
        // did.** Nothing under `bench/budget/src/` opens
        // `docs/02-requirements/prd.md`. What it pins is the budget constants
        // against silent drift *within this crate* -- real value, and why the
        // 350 -> 3500 mutation went red -- but one edit touching both the
        // constant and the literal below passes it, which is F-443's failure
        // mode exactly ("the re-baseline landed in the PRD, not the code").
        // Renamed to what it does rather than left promising what it does not;
        // `bench/doclint`'s `check-prd-join` is the mechanism that would
        // genuinely close the gap, and extending it is the other legitimate
        // fix.
        //
        // NFR-PRF-08's acceptance criterion is Inspect: "CI configuration
        // review confirms each budget has an asserting job". This is that
        // review, as a test, for the half of it that lives in this file.
        // **Three gated entries since task #118**, not five: PRD v1.2 gates
        // only the Serat term for NFR-PRF-01, so the two pre-split aggregates
        // moved to `PRF01_REPORTED`. Their limits are still asserted here,
        // because a reported figure whose reference number could drift
        // unnoticed is worth no more than an ungated one.
        assert_eq!(P0_BUDGETS.len(), 3);
        assert_eq!(PRF01_REPORTED.len(), 2);
        assert_eq!(PRF01_WINDOW_VISIBLE.product_limit_ms, 400.0);
        assert_eq!(PRF01_WINDOW_VISIBLE.iterations, 20);
        assert_eq!(PRF01_VAULT_USABLE.product_limit_ms, 1_000.0);
        assert_eq!(PRF01_VAULT_USABLE.iterations, 20);
        // ADR-0014's derived interval is [s95*1.3, 400ms] = [304, 400]; 350 sits
        // inside it with room on both sides (task #104's own arithmetic).
        assert_eq!(PRF01_SERAT_TERM.product_limit_ms, 350.0);
        assert_eq!(PRF01_SERAT_TERM.iterations, 20);
        assert_eq!(PRF03_SAVE_TO_INDEX.product_limit_ms, 50.0);
        assert_eq!(PRF03_SAVE_TO_INDEX.iterations, 1_000);
        assert_eq!(PRF04_FULL_REBUILD.product_limit_ms, 30_000.0);
        for b in P0_BUDGETS.iter().chain(PRF01_REPORTED) {
            assert!(b.requirement.starts_with("NFR-PRF-"));
            assert_eq!(b.vault, "20k", "{} measures the wrong vault", b.id);
            assert!(
                !b.interval.is_empty(),
                "{} does not say which interval it times, and P0 finding F-03 is what happens when nobody writes that down",
                b.id
            );
        }
    }

    #[test]
    fn no_budget_binary_holds_logic_a_test_cannot_reach() {
        // **Task #124, and the honest end of F-501's regress.** Pinning
        // `enforce` left its caller unpinned; pinning the caller left the
        // composition unpinned; moving the composition into the library left
        // `main` calling it unpinned -- verified, not assumed: gutting
        // `prf01-cold-start`'s `main` to `ExitCode::SUCCESS` leaves the suite
        // green. No unit test can call a `main`, so the property asserted here
        // is that **there is nothing in one worth testing**.
        //
        // Weaker than executing it, and deliberately so. What it buys is that
        // whatever stays unpinned is one line long and shows up in a diff.
        let all = budget_binary_main_bodies().expect("bin/ is readable");
        assert!(
            all.len() >= 5,
            "found {} binaries; a check that found none would pass vacuously, which is F-30's              own lesson about a grep deciding coverage: {all:?}",
            all.len()
        );

        // **Which binaries this governs, derived rather than listed.** The
        // property is about paths that decide a BUILD RESULT, so the subjects
        // are the binaries calling a job entry point -- `run_job`,
        // `run_paired_job` or `cold_start_job`. `environment` and
        // `prf01-cold-start-terms` are diagnostics: they print, they assert no
        // budget, and there is no verdict in them to lose. A hand-kept
        // exemption list would be the second list that drifts from the first,
        // which is the failure doclint's own doc warns about, so the
        // discriminator is read off the source.
        let bodies: Vec<&(String, String)> = all
            .iter()
            .filter(|(_, body)| {
                body.contains("run_job(")
                    || body.contains("run_paired_job(")
                    || body.contains("cold_start_job(")
            })
            .collect();
        assert!(
            bodies.len() >= 4,
            "only {} binaries run a budget job; all four gate jobs must be here or this check              governs less than it looks: {bodies:?}",
            bodies.len()
        );
        for (name, body) in &bodies {
            // One statement, and it hands off to the library. `let` bindings
            // for argv are the only plumbing allowed; anything else is a
            // decision living where no test can drive it.
            let statements = body.matches(';').count();
            assert!(
                statements <= 1,
                "{name}'s main has {statements} statements. A main that does anything is a main \
                 nobody can test (F-501): move the composition into the library and leave one \
                 delegation here. Body: {body}"
            );
            assert!(
                body.contains("budget::") || body.contains("run_job") || body.contains("job("),
                "{name}'s main does not delegate to a library entry point, so whatever it does \
                 is untestable by construction. Body: {body}"
            );
        }

        // The specific one F-501 names, by name, because the general property
        // above would still hold if this binary delegated somewhere else.
        let cold_start = bodies
            .iter()
            .find(|(n, _)| n == "prf01-cold-start.rs")
            .expect("prf01-cold-start.rs exists");
        assert!(
            cold_start.1.contains("cold_start_job"),
            "prf01-cold-start's main must call the composition that runs the scale check -- \
             F-81 was this call being droppable with nothing going red, and F-501 is that same \
             call one level further out. Body: {}",
            cold_start.1
        );
    }

    #[test]
    fn a_breached_gated_budget_exits_failure_through_the_real_verdict_step() {
        // **Task #124, F-500.** `enforce` had a truth table and the line that
        // CALLS it did not: changing `run_job`'s verdict to `if true` -- so no
        // budget could ever fail CI -- left the whole suite green on both
        // trees. Testing the predicate is not testing the decision. This
        // drives the decision itself and asserts the process exit code, which
        // is the thing CI reads.
        let outcome = |budget: Budget, values: &[f64]| Outcome {
            budget,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(values),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };

        assert_eq!(
            job_exit(
                &PRF04_FULL_REBUILD,
                Ok(outcome(PRF04_FULL_REBUILD, &[40_000.0, 41_000.0]))
            ),
            std::process::ExitCode::FAILURE,
            "a gated budget over its limit must exit non-zero -- this is the whole of C-06's \
             enforcement and F-500 is what happens when nothing asserts it"
        );
        assert_eq!(
            job_exit(
                &PRF04_FULL_REBUILD,
                Ok(outcome(PRF04_FULL_REBUILD, &[10_000.0, 11_000.0]))
            ),
            std::process::ExitCode::SUCCESS
        );
        assert_eq!(
            job_exit(
                &PRF04_FULL_REBUILD,
                Ok(outcome(PRF04_FULL_REBUILD, &[10_000.0, 40_000.0]))
            ),
            std::process::ExitCode::FAILURE,
            "a straddling run settled nothing and must not go green"
        );
        // The demoted figures keep their side of #118: reported, never fatal.
        assert_eq!(
            job_exit(
                &PRF01_WINDOW_VISIBLE,
                Ok(outcome(PRF01_WINDOW_VISIBLE, &[1_575.6, 1_580.0]))
            ),
            std::process::ExitCode::SUCCESS,
            "an ungated figure over its reference limit must not fail a build"
        );
        // And a measurement that could not be TAKEN fails on its own account,
        // never reaching `enforce`.
        assert_eq!(
            job_exit(
                &PRF04_FULL_REBUILD,
                Err("no application to start".to_string())
            ),
            std::process::ExitCode::FAILURE,
            "reported means a number is not asserted, not that its absence is acceptable"
        );
    }

    #[test]
    fn the_app_driving_jobs_build_or_fetch_the_binary_before_they_measure() {
        // Task #164 (NFR-PRF-01, NFR-PRF-03, NFR-PRF-08, C-06): runs 279-316 failed
        // three jobs with "no application to start" because nothing built
        // target/release/serat.exe. Pin the wiring: each job that drives the app
        // needs `build-app`, downloads its artifact BEFORE the measure step, and
        // build-app builds `-p serat-app` (never --workspace: test-hooks).
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));
        let job_text = |id: &str| -> String {
            let start = text
                .find(&format!("\n  {id}:\n"))
                .unwrap_or_else(|| panic!("job {id} missing from budgets.yml"));
            let rest = &text[start + 1..];
            let end = rest
                .match_indices("\n  ")
                .find(|(i, _)| {
                    rest[i + 3..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
                })
                .map_or(rest.len(), |(i, _)| i);
            rest[..end].to_string()
        };
        // A skipped required check counts as passing under branch protection, so a
        // failed build-app (which skips the three) must still turn the PR red: the
        // `budgets-gate` job runs always and fails unless every gating job succeeded.
        let gate = job_text("budgets-gate");
        assert!(gate.contains("if: always()"), "{gate}");
        for job in [
            "serat-app-release-guard",
            "vault",
            "build-app",
            "prf04-full-rebuild",
            "prf03-save-to-index",
            "prf01-cold-start",
            "prf01-serat-term",
            "p1-read-paths",
            "p1-tree-path",
            "p1-external-change",
            "p1-switcher",
            "every-budget-has-a-job",
        ] {
            assert!(
                gate.contains(&format!("needs.{job}.result")),
                "budgets-gate does not check {job}"
            );
            assert!(gate.contains(job), "budgets-gate does not need {job}");
        }
        let build = job_text("build-app");
        assert!(
            build.contains("cargo build --release -p serat-app"),
            "{build}"
        );
        assert!(!build.contains("--workspace"), "{build}");
        assert!(!build.contains("--features"), "{build}");
        for (job, bin) in [
            ("prf03-save-to-index", "prf03-save-to-index"),
            ("prf01-cold-start", "prf01-cold-start"),
            ("prf01-serat-term", "prf01-serat-term"),
        ] {
            let body = job_text(job);
            assert!(body.contains("build-app"), "{job} does not need build-app");
            let download = body
                .find("download-artifact")
                .unwrap_or_else(|| panic!("{job} never downloads the app binary"));
            let measure = body
                .find(&format!("--bin {bin} "))
                .unwrap_or_else(|| panic!("{job} does not run {bin}"));
            assert!(
                download < measure,
                "{job} measures before it has the binary"
            );
        }
    }

    #[test]
    fn a_missing_application_exits_with_its_own_code_through_every_job_shape() {
        // Task #164, NFR-PRF-08: an unbuilt app is neither a pass nor a breach.
        use std::process::ExitCode;
        let absent = format!("{}looked for target/release/serat", subject::APP_ABSENT_TAG);
        let want = ExitCode::from(subject::EXIT_APP_ABSENT);
        assert_eq!(
            job_exit(&PRF03_SAVE_TO_INDEX, Err(absent.clone())),
            want,
            "run_job's path (prf03, prf01-serat-term)"
        );
        assert_eq!(
            job_exit(&PRF03_SAVE_TO_INDEX, Err("spawning x: denied".into())),
            ExitCode::FAILURE,
            "any other measurement error stays 1"
        );
        let a = absent.clone();
        let code = cold_start_job_with(
            &[],
            |_| job_exit(&PRF01_SERAT_TERM, Err(a)),
            |_| panic!("the scale check must not run when the app is absent"),
        );
        assert_eq!(code, want, "the cold-start composition keeps the code");
        // And the scale check's own absent code is not flattened to 1.
        let code = cold_start_job_with(&[], |_| ExitCode::SUCCESS, |_| want);
        assert_eq!(code, want, "a scale check that finds no app exits 3, not 1");
    }

    #[test]
    fn the_cold_start_job_runs_the_scale_check_and_fails_when_it_fails() {
        // **Task #124, F-501: F-81 reopened one level further out.** Deleting
        // `run_scale_check` from the binary's `main` left every test green,
        // while the comment beside that call claimed the composition was
        // pinned. The composition is in the library now and this drives it.
        use std::cell::Cell;
        use std::process::ExitCode;

        // 1. The scale check is CALLED, and its failure fails the job -- even
        //    though the two figures reported beside it gate nothing (#118).
        let called = Cell::new(false);
        let code = cold_start_job_with(
            &[],
            |_| ExitCode::SUCCESS,
            |iterations| {
                called.set(true);
                assert_eq!(
                    iterations, PRF01_WINDOW_VISIBLE.iterations,
                    "the scale check must run the same number of pairs the budget names"
                );
                ExitCode::FAILURE
            },
        );
        assert!(
            called.get(),
            "the scale check was never called -- F-81, again"
        );
        assert_eq!(
            code,
            ExitCode::FAILURE,
            "a critical path that grew with the vault must fail the job"
        );

        // 2. And its success is the job's success.
        let ok = cold_start_job_with(&[], |_| ExitCode::SUCCESS, |_| ExitCode::SUCCESS);
        assert_eq!(ok, ExitCode::SUCCESS);

        // 3. `--skip-scale` skips it, which is what makes that flag honest --
        //    and means a --skip-scale run asserts nothing at all.
        let skipped = Cell::new(false);
        let code = cold_start_job_with(
            &["--skip-scale".to_string()],
            |_| ExitCode::SUCCESS,
            |_| {
                skipped.set(true);
                ExitCode::FAILURE
            },
        );
        assert!(!skipped.get(), "--skip-scale must not run the scale check");
        assert_eq!(code, ExitCode::SUCCESS);

        // 4. A reporting step that could not be taken short-circuits: there is
        //    no point measuring scale independence against a vault that would
        //    not open, and the scale check must not paper over it.
        let reached = Cell::new(false);
        let code = cold_start_job_with(
            &[],
            |_| ExitCode::FAILURE,
            |_| {
                reached.set(true);
                ExitCode::SUCCESS
            },
        );
        assert!(
            !reached.get(),
            "the scale check must not run after the measurement step failed"
        );
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn enforce_is_the_whole_truth_table_and_the_only_place_the_rule_lives() {
        // F-485, following F-480. The rule used to be written twice and the
        // two copies disagreed -- `run_job`'s decision never consulted
        // `gates()` at all. This is the exhaustive table for the one copy that
        // is left: three inputs, eight rows, no fourth input.
        //
        // Both mutations that motivated it die here: `if false` in place of
        // the gated branch (rows 1-4 go wrong) and dropping the `gates()`
        // check so an ungated breach fails a build (rows 5-8 go wrong).
        let outcome = |budget: Budget, values: &[f64]| Outcome {
            budget,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(values),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        // PRF04's limit is 30,000 ms; PRF01_WINDOW_VISIBLE's is 400 ms.
        let gated_pass = outcome(PRF04_FULL_REBUILD, &[10_000.0, 11_000.0]);
        let gated_breach = outcome(PRF04_FULL_REBUILD, &[40_000.0, 41_000.0]);
        let gated_straddle = outcome(PRF04_FULL_REBUILD, &[10_000.0, 40_000.0]);
        let mut gated_degraded = outcome(PRF04_FULL_REBUILD, &[10_000.0, 11_000.0]);
        gated_degraded.degraded = Some("backpressure".to_string());

        let reported_pass = outcome(PRF01_WINDOW_VISIBLE, &[100.0, 200.0]);
        let reported_breach = outcome(PRF01_WINDOW_VISIBLE, &[1_575.6, 1_580.0]);
        let reported_straddle = outcome(PRF01_WINDOW_VISIBLE, &[100.0, 1_580.0]);
        let mut reported_degraded = outcome(PRF01_WINDOW_VISIBLE, &[100.0, 200.0]);
        reported_degraded.degraded = Some("backpressure".to_string());

        // Gated: only a determinate, undegraded pass may go green.
        assert!(enforce(&gated_pass), "a gated budget that held must pass");
        assert!(
            !enforce(&gated_breach),
            "a gated breach must fail the build"
        );
        assert!(
            !enforce(&gated_straddle),
            "a run that observed both a pass and a fail settled nothing and must not go green"
        );
        assert!(
            !enforce(&gated_degraded),
            "F-435: degraded fails the number, and the number is what the build asserts"
        );

        // Ungated: measured, printed, and unable to fail a build -- whatever
        // the arithmetic says. This is the half #118 exists for.
        assert!(enforce(&reported_pass));
        assert!(
            enforce(&reported_breach),
            "a reported figure over its reference limit must not fail a build"
        );
        assert!(enforce(&reported_straddle));
        assert!(enforce(&reported_degraded));

        // And the report agrees with the enforcement in both directions --
        // F-485's actual complaint was a figure whose words and whose exit
        // code could say different things.
        assert!(reported_breach.to_report().starts_with("REPORTED"));
        assert!(gated_breach.to_report().starts_with("FAIL"));
    }

    /// An outcome of `budget` under `profile` on `environment`, for the runner-class tests.
    fn outcome_on(
        budget: Budget,
        profile: Profile,
        environment: env::Environment,
        values: &[f64],
    ) -> Outcome {
        Outcome {
            budget,
            profile,
            vault: a_vault(),
            environment,
            samples: samples(values),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        }
    }

    /// What the hosted `windows-latest` runner reported in all 36 runs on record
    /// (runs 279-316): 2 cores, real-time scanner OFF, High performance.
    fn hosted_windows() -> env::Environment {
        env::Environment {
            os: env::Os::Windows,
            cores: 2,
            realtime_scanner: Some(false),
            power_plan: Some("High performance".into()),
            cpu_load_percent: Some(9),
            concurrent_build_processes: Some(0),
        }
    }

    fn linux() -> env::Environment {
        env::Environment {
            os: env::Os::Linux,
            cores: 4,
            realtime_scanner: None,
            power_plan: None,
            cpu_load_percent: None,
            concurrent_build_processes: None,
        }
    }

    #[test]
    fn a_ci_run_on_a_runner_that_cannot_produce_a_gate_number_reports_a_ci_only_budget() {
        // Task #166 (NFR-PRF-04, NFR-PRF-08, C-06, RISK-012). Run 298's median, 55,224.6 ms,
        // is over the 30 s limit; on the hosted runner it is printed and cannot fail.
        let slow = [52_403.7, 55_224.6, 101_426.5];
        let hosted = outcome_on(PRF04_FULL_REBUILD, Profile::Ci, hosted_windows(), &slow);
        assert!(hosted.gates(), "the PRD still gates NFR-PRF-04");
        assert!(!hosted.gate_eligible());
        assert!(hosted.report_only_here() && !hosted.asserted());
        assert!(
            enforce(&hosted),
            "a breach on a non-eligible runner must not fail CI"
        );
        let report = hosted.to_report();
        assert!(report.starts_with("REPORT-ONLY"), "{report}");
        assert!(report.contains("NOT A GATE NUMBER"), "{report}");
        assert!(report.contains("not asserted"), "{report}");
        assert!(
            !report.contains("no CI regression ceiling is set yet"),
            "{report}"
        );
        assert!(hosted.to_json().contains("\"asserted\": false"));
        assert!(hosted.to_json().contains("\"gated\": true"));
        // A straddle is not "undecided" when nothing is asserted either.
        let straddle = outcome_on(
            PRF04_FULL_REBUILD,
            Profile::Ci,
            hosted_windows(),
            &[20_596.8, 28_394.0, 60_147.4],
        );
        assert!(enforce(&straddle));
        assert!(!straddle.to_report().contains("undecided"));
        assert!(straddle.to_report().starts_with("REPORT-ONLY"));
    }

    #[test]
    fn the_same_budget_is_still_asserted_wherever_a_gate_number_can_be_produced() {
        // The rule removes the assertion from one runner class and nowhere else. Each row
        // is a different way "gate only when eligible" could be over-applied.
        let breach = [40_000.0, 41_000.0, 42_000.0];
        // An eligible CI runner (Linux has no scanner question) asserts and fails.
        let on_linux = outcome_on(PRF04_FULL_REBUILD, Profile::Ci, linux(), &breach);
        assert!(on_linux.asserted() && !enforce(&on_linux));
        assert!(on_linux.to_report().starts_with("FAIL"));
        assert!(on_linux.to_json().contains("\"asserted\": true"));
        // Windows with the scanner ON is eligible: asserts.
        let scanner_on = env::Environment {
            realtime_scanner: Some(true),
            ..hosted_windows()
        };
        let on_win = outcome_on(PRF04_FULL_REBUILD, Profile::Ci, scanner_on, &breach);
        assert!(on_win.asserted() && !enforce(&on_win));
        // The reference profile asserts on a scanner-off machine: RM-1 is that machine, and
        // a person chose it. (Its figure is still labelled NOT A GATE NUMBER; that is the
        // separate RISK-012 label and is unchanged.)
        let on_rm1 = outcome_on(
            PRF04_FULL_REBUILD,
            Profile::Reference,
            hosted_windows(),
            &breach,
        );
        assert!(on_rm1.asserted() && !enforce(&on_rm1));
        // An undetermined platform is not eligible, so it reports under ci rather than
        // asserting on a number nobody can attribute.
        let unknown = outcome_on(
            PRF04_FULL_REBUILD,
            Profile::Ci,
            env::Environment::default(),
            &breach,
        );
        assert!(!unknown.asserted() && enforce(&unknown));
        // A budget that did not opt in is untouched on the hosted runner.
        let prf03 = outcome_on(PRF03_SAVE_TO_INDEX, Profile::Ci, hosted_windows(), &[80.0]);
        assert_eq!(prf03.budget.ci_assertion, CiAssertion::Always);
        assert!(prf03.asserted() && !enforce(&prf03));
        // Report-only waives the runner's slowness and nothing else. An unmeasurable run
        // (Err) reaches job_exit and fails; a degraded Ok outcome is NOT report-only (it
        // falls into the asserted path and fails), so it cannot hide behind the runner.
        let mut degraded = outcome_on(
            PRF04_FULL_REBUILD,
            Profile::Ci,
            hosted_windows(),
            &[10_000.0],
        );
        degraded.degraded = Some("backpressure".into());
        assert!(!degraded.report_only_here() && !enforce(&degraded));
        assert!(degraded.to_report().contains("DEGRADED"));
        assert_eq!(
            job_exit(&PRF04_FULL_REBUILD, Err("the vault would not open".into())),
            std::process::ExitCode::FAILURE
        );
    }

    #[test]
    fn a_report_only_outcome_exits_zero_and_an_asserted_breach_exits_one() {
        // `job_exit` is the real exit decision (F-500). Both rows go through it so the exit
        // code cannot disagree with the verdict word.
        let breach = [52_403.7, 55_224.6, 101_426.5];
        let reported = outcome_on(PRF04_FULL_REBUILD, Profile::Ci, hosted_windows(), &breach);
        assert_eq!(
            job_exit(&PRF04_FULL_REBUILD, Ok(reported)),
            std::process::ExitCode::SUCCESS
        );
        let asserted = outcome_on(PRF04_FULL_REBUILD, Profile::Ci, linux(), &breach);
        assert_eq!(
            job_exit(&PRF04_FULL_REBUILD, Ok(asserted)),
            std::process::ExitCode::FAILURE
        );
    }

    #[test]
    fn a_when_gate_eligible_budget_has_no_ceiling_and_a_ceiling_means_always() {
        // The two thresholds are one design (budget-harness.md, "Two thresholds, not
        // one"): a regression ceiling is what CI asserts on a runner that is not the
        // reference machine, so a budget that has one asserts it wherever it runs. Setting
        // `ci_limit_ms` without flipping this field would leave a ceiling nothing asserts.
        for b in P0_BUDGETS.iter().chain(PRF01_REPORTED) {
            if b.ci_limit_ms.is_some() {
                assert_eq!(
                    b.ci_assertion,
                    CiAssertion::Always,
                    "{} has a CI ceiling, so CI must assert it",
                    b.id
                );
            }
        }
    }

    #[test]
    fn a_ceiling_is_asserted_even_on_a_report_only_budget() {
        // ADR-0019: report_only_here must not survive a future ci_limit_ms.
        let mut b = PRF04_FULL_REBUILD;
        b.ci_limit_ms = Some(60_000.0);
        let o = outcome_on(b, Profile::Ci, hosted_windows(), &[70_000.0, 71_000.0]);
        assert!(!o.report_only_here() && o.asserted() && !enforce(&o));
    }

    /// The text of job `id` in workflow `text`: from its two-space key to the next one.
    fn workflow_job(text: &str, id: &str) -> String {
        let head = format!("\n  {id}:\n");
        let start = text
            .find(&head)
            .unwrap_or_else(|| panic!("job {id} missing from budgets.yml"));
        let rest = &text[start + 1..];
        let end = rest
            .match_indices("\n  ")
            .find(|(i, _)| {
                rest[i + 3..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
            })
            .map_or(rest.len(), |(i, _)| i);
        rest[..end].to_string()
    }

    /// The `name:` of a job, as the PR's check list shows it.
    fn workflow_job_name(job: &str) -> String {
        job.lines()
            .find_map(|l| l.trim_start().strip_prefix("name:"))
            .map(|n| n.trim().trim_matches('"').to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn a_report_only_budget_says_so_in_its_job_name() {
        // NFR-PRF-08's acceptance criterion is "each budget has an asserting job". For a
        // budget declared `WhenGateEligible` the job reports on a hosted runner and the
        // assertion is the reference machine's (task #166). That is a different category,
        // and it must be visible where a reviewer reads it: in the name of the check on the
        // PR. Both directions, so a job cannot claim to report while the harness asserts,
        // or assert while its name says it reports.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));
        let mut report_only = 0;
        for b in P0_BUDGETS {
            let job = workflow_job(&text, b.id);
            let name = workflow_job_name(&job);
            let says = name.contains("report-only");
            assert_eq!(
                says,
                b.ci_assertion == CiAssertion::WhenGateEligible,
                "{}: job name {name:?} and Budget::ci_assertion disagree about whether CI \
                 asserts this budget (task #166)",
                b.id
            );
            if b.ci_assertion == CiAssertion::WhenGateEligible {
                report_only += 1;
                assert!(
                    name.contains("reference machine"),
                    "{}: a report-only job must name where the budget IS asserted: {name:?}",
                    b.id
                );
                // Still measured on every run, under the CI profile (the one that reports),
                // never masked: a failed measurement must still fail the job.
                assert!(
                    job.contains(&format!("--bin {} -- --profile ci", b.id)),
                    "{}: the report-only job must run the bin under --profile ci:\n{job}",
                    b.id
                );
                assert!(
                    !job.contains("continue-on-error") && !job.contains("|| true"),
                    "{}: report-only is the harness's decision, not a swallowed exit code",
                    b.id
                );
            }
        }
        assert!(
            report_only >= 1,
            "no budget is declared WhenGateEligible: this test would pass vacuously"
        );
    }

    #[test]
    fn every_budget_a_run_job_binary_drives_is_a_gated_one() {
        // **F-485's real gap.** The partition test below pins the `gated` flag
        // against list membership. Nothing pinned that the binaries calling
        // `run_job` -- the path that can fail a build -- drive exactly the
        // gated set. A sixth budget added to PRF01_REPORTED with
        // `gated: false` and wired to a run_job binary would print "not a
        // gate" in its own report and still fail CI on a breach: report and
        // enforcement in contradiction, with every existing test green.
        //
        // Read off the binaries rather than restated, so it cannot drift.
        let enforced = budgets_enforced_by_a_binary().expect("bin/ is readable");
        assert!(
            !enforced.is_empty(),
            "no binary was found to call run_job -- this check would pass vacuously, which is \
             F-27's lesson about a review that reviews nothing"
        );
        for id in &enforced {
            let budget = P0_BUDGETS
                .iter()
                .chain(PRF01_REPORTED)
                .find(|b| b.id == id)
                .unwrap_or_else(|| {
                    panic!("{id} is driven by a binary but is not a declared budget")
                });
            assert!(
                budget.gated,
                "{id} is driven by a run_job binary, so a breach of it fails the build -- but it \
                 is flagged ungated and its own report will say \"not a gate\". Report and \
                 enforcement must not be able to disagree (F-485)."
            );
        }
        let mut gated: Vec<String> = P0_BUDGETS.iter().map(|b| b.id.to_string()).collect();
        gated.sort_unstable();
        assert_eq!(
            enforced, gated,
            "the budgets a run_job binary drives and the budgets flagged gated must be the same \
             set, in both directions: a gated budget with no such binary is a budget CI cannot \
             breach on, which is NFR-PRF-08's own acceptance criterion unmet"
        );
    }

    #[test]
    fn the_gated_list_and_the_reported_list_partition_every_budget_by_its_own_flag() {
        // Task #118's invariant, and the reason `gated` is a field rather than
        // a convention. F-443 was found because "what the PRD gates" and "what
        // CI gates" were two facts kept in two documents by hand, and they
        // disagreed for a wave without anyone noticing. Keeping the same
        // disagreement possible between a `bool` and a list would be that
        // defect one layer down, so the two are checked against each other:
        // every entry in P0_BUDGETS gates, every entry in PRF01_REPORTED does
        // not, no id is in both, and every budget this file declares is in
        // exactly one of them -- so a sixth budget added to neither list fails
        // here rather than being silently unmeasured.
        for b in P0_BUDGETS {
            assert!(b.gated, "{} is in P0_BUDGETS but does not gate", b.id);
        }
        for b in PRF01_REPORTED {
            assert!(!b.gated, "{} is in PRF01_REPORTED but gates", b.id);
        }
        let mut ids: Vec<&str> = P0_BUDGETS
            .iter()
            .chain(PRF01_REPORTED)
            .map(|b| b.id)
            .collect();
        let declared = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(declared, ids.len(), "a budget is in both lists: {ids:?}");
        assert_eq!(
            ids,
            vec![
                "prf01-serat-term",
                "prf01-vault-usable",
                "prf01-window-visible",
                "prf03-save-to-index",
                "prf04-full-rebuild",
            ],
            "every budget this file declares must be in exactly one list"
        );
    }

    #[test]
    fn a_critical_path_that_grows_with_the_vault_is_caught() {
        // Architecture §3.4's load-bearing property, as a test. If the main
        // thread ever opened the index, or the tree rendered from the walk,
        // 2.5x the notes would show up here.
        assert!(assert_scale_independent(300.0, 300.0).is_ok());
        assert!(
            assert_scale_independent(300.0, 360.0).is_ok(),
            "exactly at tolerance"
        );
        let err = assert_scale_independent(300.0, 361.0).expect_err("over tolerance");
        assert!(err.contains("proportional to vault size"), "{err}");
        // The shape a proportional path would actually produce.
        let err = assert_scale_independent(300.0, 750.0).expect_err("2.5x must fail");
        assert!(err.contains("150%"), "{err}");
    }

    fn a_vault() -> VaultUnderTest {
        VaultUnderTest {
            path: PathBuf::from("x"),
            digest: "d".into(),
            format_version: vaultgen::VAULT_FORMAT_VERSION,
            notes: 20_000,
            note_links: 216_971,
            assets: 200,
            asset_embeds: 2_004,
            asset_md_links: 1_392,
        }
    }

    // ---------------------------------------------------------------- F-25

    #[test]
    fn the_median_of_three_is_the_middle_run_and_not_the_fastest() {
        // The mutation this exists for: ceil() -> floor() in percentile().
        // At n = 20 and n = 1000 the two coincide, so every earlier test was
        // blind to it. At n = 3, floor(0.5 * 3) = 1 gives sorted[0] — the
        // FASTEST of three — where nearest rank gives the middle one. One
        // character turns NFR-PRF-04's median-of-three into a best-of-three.
        let s = samples(&[10.0, 30.0, 20.0]);
        assert_eq!(s.percentile(0.50), 20.0, "median of three is the middle");
        assert_ne!(
            s.percentile(0.50),
            s.min(),
            "and emphatically not the fastest"
        );
        assert_eq!(s.median(), 20.0);

        // Even n, so nobody can quietly average.
        assert_eq!(samples(&[10.0, 20.0, 30.0, 40.0]).percentile(0.50), 20.0);
    }

    #[test]
    fn the_wall_clock_statistic_is_the_median() {
        // Nothing asserted this: WallClock => percentile(0.01) left every test
        // green, so a budget could have been judged on its fastest run.
        let s = samples(&[10.0, 30.0, 20.0]);
        assert_eq!(s.statistic(Statistic::WallClock), 20.0);
        assert_eq!(s.statistic(Statistic::P95), 30.0);

        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[10_000.0, 40_000.0, 20_000.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert_eq!(outcome.value_ms(), 20_000.0, "the middle run decides");
        assert!(outcome.passed());
    }

    #[test]
    fn the_report_says_the_number_is_a_median_of_three() {
        // This exact line goes into the charter gate report, and "wall clock =
        // 27431 ms" over three runs reads as one run.
        assert_eq!(
            Statistic::WallClock.describe(3),
            "median of 3 wall-clock runs"
        );
        assert_eq!(Statistic::P95.describe(20), "p95 over 20 runs");

        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[10_000.0, 40_000.0, 20_000.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(
            outcome.to_report().contains("median of 3 wall-clock runs"),
            "{}",
            outcome.to_report()
        );
    }

    #[test]
    fn the_two_medians_in_the_crate_agree() {
        // Samples::median and median_duration are separate implementations of
        // the same idea; nothing held them together before.
        for n in 1..=8usize {
            let ms: Vec<f64> = (0..n).map(|i| (i as f64 + 1.0) * 10.0).collect();
            let durations: Vec<Duration> = ms
                .iter()
                .map(|v| Duration::from_secs_f64(v / 1000.0))
                .collect();
            let a = samples(&ms).median();
            let b = median_duration(durations).as_secs_f64() * 1000.0;
            assert!((a - b).abs() < 1e-9, "n={n}: {a} vs {b}");
        }
    }

    // ---------------------------------------------------------------- F-26

    #[test]
    fn a_coherent_vault_generated_from_the_wrong_seed_is_refused() {
        // The finding: regenerate with --seed 424242 and the manifest is
        // regenerated alongside it, so vault and manifest agree perfectly about
        // a vault nobody meant to measure. Self-consistency is not canonicality.
        let p = preset("20k").expect("20k exists");
        let wrong = "da0d24b738bbc20657752c8960002b2eed2adbd3c891c4767cf902fd1303e0e4";

        check_vault(
            p,
            p.digest,
            p.digest,
            vaultgen::VAULT_FORMAT_VERSION,
            p.notes,
        )
        .expect("the canonical vault passes");

        let err = check_vault(p, wrong, wrong, vaultgen::VAULT_FORMAT_VERSION, p.notes)
            .expect_err("a coherent but non-canonical vault must be refused");
        assert!(err.contains("not the canonical"), "{err}");
        assert!(err.contains("--preset 20k --force"), "{err}");
    }

    #[test]
    fn a_vault_that_disagrees_with_its_manifest_is_refused() {
        let p = preset("20k").expect("20k exists");
        let err = check_vault(p, "aaa", p.digest, vaultgen::VAULT_FORMAT_VERSION, p.notes)
            .expect_err("hand-edited vault");
        assert!(err.contains("does not match its manifest"), "{err}");
    }

    // ---------------------------------------------------------------- F-372 / #78

    #[test]
    fn a_stray_scratch_directory_is_named_rather_than_read_as_corruption() {
        // The reproduction, standing in for what killing `prf04` actually did:
        // a directory that was never part of the vault, sitting at its top
        // level. verify_vault_at must catch this BEFORE it reaches the digest
        // comparison and reports something that reads as corruption.
        let dir = std::env::temp_dir().join(format!(
            "serat-budget-test-stray-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".serat-bench-0")).expect("scratch dir");
        std::fs::write(dir.join(".serat-bench-0").join("index.db"), b"leftover")
            .expect("leftover file");
        std::fs::write(dir.join(".vaultgen"), b"stamp").expect("stamp");

        let p = preset("20k").expect("20k exists");
        let err = verify_vault_at(p, &dir).expect_err("a stray scratch dir must be refused");

        assert!(
            err.contains(".serat-bench-0"),
            "must name the directory: {err}"
        );
        assert!(
            err.contains("killed benchmark"),
            "must say what leaves it behind: {err}"
        );
        assert!(
            err.contains("rm -rf"),
            "must offer the cheap repair, not just the expensive one: {err}"
        );
        // The whole point: this must not be reachable through the "does not
        // match its manifest" / "not the canonical vault" messages, which name
        // regeneration as the fix and say nothing about the real cause.
        assert!(
            !err.contains("does not match its manifest") && !err.contains("Regenerate it"),
            "a stray scratch directory is not corruption and must not read as any: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_vault_with_no_stray_directory_is_not_flagged_by_the_new_check() {
        // The negative case for the test above: an ordinary manifest mismatch
        // (no `.serat-bench*` present) must still reach check_vault's own
        // message unchanged, not the stray-directory branch.
        let dir = std::env::temp_dir().join(format!(
            "serat-budget-test-clean-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("vault dir");
        std::fs::write(dir.join(".vaultgen"), b"stamp").expect("stamp");
        std::fs::write(
            vaultgen::manifest_path_for(&dir),
            format!(
                r#"{{"format_version": {}, "tree_digest_sha256": "not-the-real-digest", "counts": {{"notes": 1, "assets": 0}}}}"#,
                vaultgen::VAULT_FORMAT_VERSION
            ),
        )
        .expect("manifest");

        let p = preset("20k").expect("20k exists");
        let err = verify_vault_at(p, &dir).expect_err("digest deliberately wrong");
        assert!(err.contains("does not match its manifest"), "{err}");
        assert!(!err.contains(".serat-bench"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stray_scratch_entries_ignores_ordinary_vault_contents() {
        let dir = std::env::temp_dir().join(format!(
            "serat-budget-test-ordinary-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notes")).expect("notes dir");
        std::fs::write(dir.join(".vaultgen"), b"stamp").expect("stamp");
        std::fs::write(dir.join("notes").join("a.md"), b"# a").expect("note");

        assert!(
            stray_scratch_entries(&dir).is_empty(),
            "an ordinary vault tree must not be flagged"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tree_digest_ignores_a_top_level_serat_directory_but_not_real_content() {
        // F-401: cold_start_once spawns the real app directly against a vault, and
        // the app's own index at <vault>/.serat -- correct, ordinary production
        // behaviour -- is not vault content and must not move the digest this test
        // is named after. Bit the shared 20k vault twice in one session before this
        // landed; the exclusion is scoped to the root only, so a real edit two lines
        // down still must not be masked by it.
        let dir = std::env::temp_dir().join(format!(
            "serat-budget-test-serat-exclusion-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notes")).expect("notes dir");
        std::fs::write(dir.join("notes").join("a.md"), b"# a").expect("note");

        let before = tree_digest(&dir).expect("digest of a clean tree");

        std::fs::create_dir_all(dir.join(".serat")).expect(".serat dir");
        std::fs::write(dir.join(".serat").join("index.db"), b"not vault content")
            .expect("index file");
        assert_eq!(
            tree_digest(&dir).expect("digest with .serat present"),
            before,
            "a real .serat/ index must not move the digest"
        );

        // The exclusion must not swallow a genuine change made at the same time.
        std::fs::write(dir.join("notes").join("a.md"), b"# a, edited").expect("edit");
        assert_ne!(
            tree_digest(&dir).expect("digest after a real edit"),
            before,
            "a real content change must still move the digest, .serat/ notwithstanding"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_wrong_note_count_or_format_version_is_refused() {
        let p = preset("20k").expect("20k exists");
        let err = check_vault(p, p.digest, p.digest, vaultgen::VAULT_FORMAT_VERSION, 400)
            .expect_err("wrong note count");
        assert!(err.contains("400 notes"), "{err}");

        let err = check_vault(p, p.digest, p.digest, 99, p.notes).expect_err("wrong version");
        assert!(err.contains("format version 99"), "{err}");
    }

    #[test]
    fn every_preset_is_pinned_to_a_digest() {
        assert_eq!(PRESETS.len(), 3);
        for p in PRESETS {
            assert_eq!(p.digest.len(), 64, "{} has no canonical digest", p.name);
            assert!(
                p.digest.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                p.name
            );
            assert!(p.notes >= 400, "{}", p.name);
        }
        assert!(preset("nope").is_err());
        // The 20k preset is the one every P0 budget measures.
        assert_eq!(preset("20k").unwrap().notes, 20_000);
    }

    // ---------------------------------------------------------------- F-27

    #[test]
    fn a_job_cannot_swallow_the_seam_error_and_post_a_number() {
        // The mutation: `subject::full_rebuild(..)?` becomes
        // `.unwrap_or_default()`, and a fabricated 1 ms number passes.
        // `decide_with_vault` returns the error rather than an Outcome, so
        // there is nothing to swallow that a test cannot see.
        //
        // Deliberately `decide_with_vault` with `a_vault()`, not `decide`
        // with a real preset (task #70): this test checks error propagation
        // and has no reason to generate or even open a real 20,000-note
        // vault to do that. Going through `decide` here used to mean this
        // test raced every other budget test binary over the same 20k
        // fixture path under `cargo test --workspace`'s concurrent
        // processes -- passing in isolation, failing under load, the same
        // shape as task #28's race one level up.
        let parsed = JobArgs {
            profile: Profile::Reference,
            iterations: PRF04_FULL_REBUILD.iterations,
            indicative: false,
            help: false,
        };
        let err = decide_with_vault(&PRF04_FULL_REBUILD, parsed, a_vault(), |_, _| {
            Err("serat-index exposes no rebuild entry point yet — task I4".to_string())
        })
        .expect_err("a failed measurement is not a passing budget");
        assert!(err.contains("task I4"), "{err}");
    }

    // --------------------------------------------------------------- F-445

    #[test]
    fn an_indicative_run_carries_the_warning_into_degraded_not_only_stderr() {
        // F-445's second SHOULD-FIX. Before this fix `decide`'s eprintln! was
        // the only record of "must not be recorded as a gate number" -- it
        // never reached the Outcome, so a fast in-budget reading from a short
        // local run could still print PASS and a JSON result with no trace of
        // the warning that made it meaningless.
        let parsed = JobArgs {
            profile: Profile::Reference,
            iterations: 1,
            indicative: true,
            help: false,
        };
        let outcome = decide_with_vault(&PRF04_FULL_REBUILD, parsed, a_vault(), |_, _| {
            let mut s = Samples::new();
            s.push(Duration::from_millis(100));
            Ok(Measurement::new(s))
        })
        .expect("an indicative run with real samples is still a real Outcome");

        assert!(
            outcome.value_ms() <= outcome.limit_ms(),
            "the timing alone would pass -- that is what makes this test meaningful"
        );
        let reason = outcome
            .degraded
            .as_ref()
            .expect("indicative must set degraded, not only print to stderr");
        assert!(reason.contains("indicative"), "{reason}");
        assert!(
            !outcome.passed(),
            "an indicative reading must never report PASS"
        );
        assert!(
            outcome.to_json().contains("\"degraded\":"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn an_indicative_run_combines_with_an_existing_degraded_reason_rather_than_overwriting_it() {
        let parsed = JobArgs {
            profile: Profile::Reference,
            iterations: 1,
            indicative: true,
            help: false,
        };
        let outcome = decide_with_vault(&PRF04_FULL_REBUILD, parsed, a_vault(), |_, _| {
            let mut s = Samples::new();
            s.push(Duration::from_millis(100));
            Ok(Measurement::new(s).degraded("a pre-existing finding"))
        })
        .expect("a degraded measurement is still a real Outcome");

        let reason = outcome.degraded.expect("degraded must still be set");
        assert!(reason.contains("indicative"), "{reason}");
        assert!(
            reason.contains("a pre-existing finding"),
            "neither reason may silently replace the other: {reason}"
        );
    }

    #[test]
    fn the_vault_generation_lock_excludes_a_second_holder_and_releases_on_drop() {
        // Task #70's other half: ensure_vault's check-then-generate window is
        // now guarded by VaultGenerationLock, and this proves the lock itself
        // does what it claims rather than trusting create_new's semantics by
        // reading them. Real mutual exclusion, not a mock: the second
        // acquire attempt genuinely fails while the first is held, on the
        // same lock file two processes would use.
        let dir =
            std::env::temp_dir().join(format!("serat-vault-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_file(dir.with_extension("vaultgen-lock"));

        let held = VaultGenerationLock::acquire(&dir).expect("first acquire must succeed");
        assert!(
            dir.with_extension("vaultgen-lock").exists(),
            "the lock file must exist on disk while held -- that is the whole \
             mechanism a second process checks"
        );

        // A second acquire on the same path must not succeed while the first
        // is still held: prove it can fail (F-58, F-27) by giving it a short
        // deadline rather than waiting out the real 120s timeout.
        let second = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dir.with_extension("vaultgen-lock"))
                {
                    Ok(_) => return true, // acquired -- the exclusion failed
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => return false, // correctly excluded
                }
            }
        })
        .join()
        .expect("the checking thread must not panic");
        assert!(
            !second,
            "a second holder acquired the lock while the first was still held"
        );

        let lock_path = held.lock_path.clone();
        drop(held);
        assert!(
            !lock_path.exists(),
            "dropping the lock must remove the file, or the next real process to want \
             this vault waits out the full 120s timeout for nothing"
        );
    }

    #[test]
    fn no_samples_is_a_failure_and_not_an_empty_pass() {
        let err = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::default(),
        )
        .expect_err("zero samples cannot be a budget");
        assert!(err.contains("no samples"), "{err}");
    }

    #[test]
    fn a_partially_indexed_vault_cannot_post_a_fast_number() {
        // F-19's guard, extracted from the job so that deleting it fails here.
        check_note_count(20_000, 20_000).expect("a complete rebuild");
        let err = check_note_count(10_000, 20_000).expect_err("half a vault");
        assert!(err.contains("10000 notes"), "{err}");
        assert!(err.contains("not a passing budget"), "{err}");
    }

    #[test]
    fn the_phase_breakdown_is_printed_and_recorded() {
        // The mutation: never print the phases and never write them to JSON.
        // They are the whole content of open design question 1.
        let phases = subject::RebuildPhases {
            walk: Duration::from_millis(500),
            read_hash: Duration::from_millis(3_000),
            parse: Duration::from_millis(6_000),
            producer_wait: Duration::from_millis(3_700),
            write_fts: Duration::from_millis(1_900),
        };
        let outcome = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(phases),
        )
        .expect("phases sum below the wall clock");

        let report = outcome.to_report();
        for needle in ["walk", "read+hash", "parse", "producer waited", "fts build"] {
            assert!(report.contains(needle), "report lacks {needle}: {report}");
        }
        let json = outcome.to_json();
        for needle in [
            "\"walk\"",
            "\"read_hash\"",
            "\"parse\"",
            "\"producer_wait\"",
            "\"write_fts\"",
        ] {
            assert!(json.contains(needle), "json lacks {needle}: {json}");
        }
    }

    #[test]
    fn a_rebuild_that_loaded_through_the_fts_triggers_is_caught() {
        // F-38. The regression is invisible in the total: write-FTS collapses to
        // zero, write-rows absorbs the cost, and the breakdown reads as if the
        // FTS build got free while the wall clock quietly got worse.
        let bulk = subject::RebuildPhases {
            walk: Duration::from_millis(500),
            read_hash: Duration::from_millis(3_000),
            parse: Duration::from_millis(6_000),
            producer_wait: Duration::from_millis(3_700),
            write_fts: Duration::from_millis(1_900),
        };
        check_bulk_load_shape(&bulk).expect("the bulk shape is what we expect");

        let through_triggers = subject::RebuildPhases {
            write_fts: Duration::ZERO,
            producer_wait: Duration::from_millis(5_600),
            ..bulk
        };
        let err = check_bulk_load_shape(&through_triggers).expect_err("triggers live");
        assert!(err.contains("FTS triggers"), "{err}");

        // And it does not fire on a breakdown where nothing was written at all,
        // which is a different fault with a different message.
        check_bulk_load_shape(&subject::RebuildPhases::default()).expect("nothing written");

        // It is wired into the outcome, not merely available.
        let err = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(through_triggers),
        )
        .expect_err("an outcome must not carry a trigger-loaded breakdown");
        assert!(err.contains("FTS triggers"), "{err}");
    }

    #[test]
    fn a_phase_breakdown_that_double_counts_is_refused() {
        // The report says a sum below the wall clock is concurrency. A sum
        // above it means a phase is counted twice, and these numbers answer
        // open design question 1 — reporting them as fact would send architect
        // the wrong way.
        let phases = subject::RebuildPhases {
            walk: Duration::from_millis(10_000),
            read_hash: Duration::from_millis(10_000),
            parse: Duration::from_millis(10_000),
            producer_wait: Duration::from_millis(10_000),
            write_fts: Duration::from_millis(10_000),
        };
        let err = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(phases),
        )
        .expect_err("50 s of phases against a 20 s wall clock");
        assert!(err.contains("counted"), "{err}");
    }

    #[test]
    fn an_unasserted_series_is_reported_inside_the_block_and_labelled() {
        // F-28: printing it outside put an unasserted 812 ms above a line
        // reading PASS.
        let outcome = outcome_from(
            &PRF03_SAVE_TO_INDEX,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[31.2])).reporting(
                "F-03",
                "the external-change path, which is FR-VLT-04's",
                samples(&[812.4]),
            ),
        )
        .expect("valid");
        let report = outcome.to_report();
        let verdict = report.find("PASS").expect("a verdict");
        let extra = report.find("812.4").expect("the reported series");
        assert!(
            extra > verdict,
            "the unasserted number must sit below the verdict:\n{report}"
        );
        assert!(report.contains("reported, not asserted"), "{report}");
        assert!(
            outcome.to_json().contains("reported_not_asserted"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn root_is_accepted_with_a_value_and_refused_without_one() {
        // Task #54: shared with doclint/vaultgen through `reporoot`; `repo_root()`
        // reads the value, `parse_args` only guards against a bare flag.
        for args in [&["--root", "/x"][..], &["--root=/x"][..]] {
            let a: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            assert!(parse_args(&PRF04_FULL_REBUILD, &a).is_ok(), "{args:?}");
        }
        for args in [&["--root"][..], &["--root="][..]] {
            let a: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            let err = parse_args(&PRF04_FULL_REBUILD, &a).expect_err("bare --root");
            assert!(err.contains("--root needs a directory"), "{err}");
        }
    }

    #[test]
    fn argument_parsing_rejects_what_it_cannot_honour() {
        let ok =
            parse_args(&PRF04_FULL_REBUILD, &["--profile".into(), "ci".into()]).expect("valid");
        assert_eq!(ok.profile, Profile::Ci);
        assert_eq!(ok.iterations, PRF04_FULL_REBUILD.iterations);
        assert!(!ok.indicative);

        let fewer =
            parse_args(&PRF04_FULL_REBUILD, &["--iterations".into(), "1".into()]).expect("valid");
        assert!(fewer.indicative, "a short run must be marked indicative");

        assert!(parse_args(&PRF04_FULL_REBUILD, &["--profile".into()]).is_err());
        assert!(parse_args(&PRF04_FULL_REBUILD, &["--iterations".into(), "0".into()]).is_err());
        assert!(parse_args(&PRF04_FULL_REBUILD, &["--profile".into(), "laptop".into()]).is_err());
    }

    // --------------------------------------------------------------- F-332

    #[test]
    fn a_run_whose_samples_straddle_the_limit_has_not_decided_anything() {
        let outcome = |values: &[f64]| Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(values),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };

        // Every sample under the limit: the run agreed with itself.
        let clear_pass = outcome(&[29_000.0, 29_500.0, 29_900.0]);
        assert!(clear_pass.determinate());
        assert!(clear_pass.passed());
        assert!(clear_pass.to_report().starts_with("PASS"));

        // Every sample over it: also decided, the other way.
        let clear_fail = outcome(&[30_100.0, 31_000.0, 32_000.0]);
        assert!(clear_fail.determinate());
        assert!(!clear_fail.passed());
        assert!(clear_fail.to_report().starts_with("FAIL"));

        // The case this exists for. One sample of three breached 30 s, and the
        // MEDIAN is comfortably under it -- so `passed()` says true and the job
        // would have gone green over a run that watched the budget break.
        let straddling = outcome(&[28_000.0, 29_000.0, 31_000.0]);
        assert!(
            straddling.passed(),
            "the median really is under the limit; that is the trap, not a typo"
        );
        assert!(
            !straddling.determinate(),
            "a run that observed both a pass and a fail has not settled the question"
        );
        let report = straddling.to_report();
        assert!(report.starts_with("INDETERMINATE"), "{report}");
        assert!(report.contains("straddle"), "{report}");

        // The boundary is inclusive on the passing side, matching `passed()`.
        let exactly_at = outcome(&[30_000.0, 30_000.0, 30_000.0]);
        assert!(exactly_at.determinate());
        assert!(exactly_at.passed());

        // And the spread is reported whether or not it decided anything, because
        // it is the only in-run evidence of how steady the machine was.
        assert!(
            (straddling.spread_pct() - 10.714).abs() < 0.01,
            "{}",
            straddling.spread_pct()
        );
        assert!(report.contains("spread"), "{report}");
    }

    // --------------------------------------------------------------- F-323

    #[test]
    fn an_argument_the_job_does_not_know_is_refused_rather_than_ignored() {
        // Each of these produced a real, correctly-reported measurement before,
        // against settings the operator did not ask for. The report was honest
        // about what it did; the intent was silently gone.
        let refuse = |args: &[&str]| -> String {
            let owned: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            parse_args(&PRF04_FULL_REBUILD, &owned).expect_err(&format!("{args:?} must be refused"))
        };

        // `--help` used to run the full 37-second rebuild. It is now a flag.
        for flag in ["--help", "-h"] {
            let parsed = parse_args(&PRF04_FULL_REBUILD, &[flag.to_string()])
                .unwrap_or_else(|e| panic!("{flag} must parse: {e}"));
            assert!(parsed.help, "{flag} must ask for usage");
        }

        // The equals form is not the string `--profile`, so it was dropped and
        // the run used the reference threshold while the operator had asked for
        // ci. It is now the same flag.
        let eq = parse_args(&PRF04_FULL_REBUILD, &["--profile=ci".into()]).expect("equals form");
        assert_eq!(eq.profile, Profile::Ci);
        let eq = parse_args(&PRF04_FULL_REBUILD, &["--iterations=1".into()]).expect("equals form");
        assert_eq!(eq.iterations, 1);
        assert!(eq.indicative, "a short run stays indicative in either form");

        // A typo is the case that matters most: it looks like it configured the
        // run and did not. The message must name the argument, or the reader is
        // told only that something was wrong.
        let err = refuse(&["--profil", "ci"]);
        assert!(err.contains("--profil"), "{err}");
        assert!(
            err.contains("--profile"),
            "the usage text must show the real flag: {err}"
        );
        assert!(refuse(&["--iteration", "1"]).contains("--iteration"));

        // A bare value with no flag, and a flag with no value.
        assert!(refuse(&["20k"]).contains("20k"));
        assert!(refuse(&["--profile"]).contains("needs a value"));
        assert!(refuse(&["--iterations"]).contains("needs a value"));

        // A bad value reaches its flag's own error in both forms.
        assert!(refuse(&["--profile", "laptop"]).contains("laptop"));
        assert!(refuse(&["--profile=laptop"]).contains("laptop"));
        assert!(refuse(&["--iterations", "0"]).contains("positive integer"));
        assert!(refuse(&["--iterations=x"]).contains("positive integer"));

        // Repeated flags are refused rather than resolved: first-wins and
        // last-wins are both defensible and neither is guessable.
        assert!(refuse(&["--profile", "ci", "--profile", "reference"]).contains("more than once"));
        assert!(refuse(&["--iterations", "1", "--iterations=2"]).contains("more than once"));

        // And the combination CI actually runs still works.
        let ci = parse_args(&PRF04_FULL_REBUILD, &["--profile".into(), "ci".into()])
            .expect("the form .github/workflows/budgets.yml uses");
        assert_eq!(ci.profile, Profile::Ci);
        assert!(!ci.help);
        assert!(!ci.indicative);
    }

    // ---------------------------------------------------------------- F-30

    // ------------------------------------------- F-382, F-390, F-421, #94

    fn save_trace(
        attempted: usize,
        submitted_ms: &[f64],
        handle_missing: u64,
        writer_refused: u64,
        writer_refused_ms: &[f64],
    ) -> trace::SaveBenchmarkTrace {
        trace::SaveBenchmarkTrace {
            attempted,
            submitted_ms: submitted_ms.to_vec(),
            handle_missing,
            writer_refused,
            writer_refused_ms: writer_refused_ms.to_vec(),
        }
    }

    #[test]
    fn check_save_completeness_accepts_a_fully_submitted_run() {
        check_save_completeness(&save_trace(3, &[12.0, 8.0, 15.0], 0, 0, &[]))
            .expect("every attempted save landed; nothing to refuse");
    }

    #[test]
    fn check_save_completeness_is_a_hard_zero_on_handle_missing() {
        // **This test is the tripwire, and that is deliberate (F-499).** The
        // hard zero cannot be softened without editing this assertion, so
        // whoever softens it arrives here -- which is the one place a warning
        // is certain to be read. What they need to know before they do:
        //
        // #122's attestation of the tree behind NFR-PRF-03's 4.306 ms left one
        // mutation surviving, and it was accepted as a SHOULD-FIX rather than
        // a blocker ONLY because this check is a hard zero (F-475). Loosen it
        // and that survivor becomes blocking the same day, and the standing
        // gate number loses the argument that admitted it. The trade is not
        // "this check is strict" against "this check is convenient"; it is
        // this check's strictness against a gate number's validity.
        let err = check_save_completeness(&save_trace(3, &[12.0, 8.0], 1, 0, &[]))
            .expect_err("a non-zero handle_missing must refuse the run");
        assert!(err.contains("1 of 3"), "{err}");
        assert!(err.contains("F-382"), "{err}");
    }

    #[test]
    fn check_save_completeness_accepts_a_non_zero_writer_refused() {
        // The other half of the split this check exists to make: writer_refused
        // is ADR-0011's sanctioned backpressure, not this function's business --
        // ruling on it belongs to measure_save_to_index_with, tested below.
        check_save_completeness(&save_trace(3, &[12.0, 8.0], 0, 1, &[0.4]))
            .expect("writer_refused alone must not refuse the run");
    }

    #[test]
    fn check_save_completeness_catches_a_fourth_unnamed_way_to_vanish() {
        // 3 attempted, only 2 accounted for by submitted+refused -- neither
        // named counter caught it, which is exactly the "denominator, not a
        // symptom" case this function exists for.
        let err = check_save_completeness(&save_trace(3, &[12.0], 0, 0, &[]))
            .expect_err("an unaccounted-for save must refuse the run");
        assert!(err.contains("3 attempted"), "{err}");
        assert!(err.contains("1 submitted, 0 refused"), "{err}");
    }

    #[test]
    fn check_save_completeness_refuses_an_all_refused_run_by_name_not_by_a_generic_panic() {
        // F-445's first SHOULD-FIX. Conservation alone passes here -- 0
        // submitted + 3 refused == 3 attempted -- and without this branch that
        // Ok would have reached Samples::percentile with an empty asserted
        // series, whose own "no samples" assertion panics rather than naming
        // the actual reason (every save was refused by the writer). The
        // specific message is the one worth reading; a panic or a generic
        // empty-measurement Err both discard it.
        let err = check_save_completeness(&save_trace(3, &[], 0, 3, &[0.1, 0.2, 0.3]))
            .expect_err("an all-refused run has no p95 to compute and must say so by name");
        assert!(err.contains("all 3 attempted saves were refused"), "{err}");
        assert!(err.contains("ADR-0011"), "{err}");
    }

    #[test]
    fn measure_save_to_index_propagates_the_specific_all_refused_reason() {
        let err = measure_save_to_index_with(&a_vault(), 3, |_, _| {
            Ok(save_trace(3, &[], 0, 3, &[0.1, 0.2, 0.3]))
        })
        .expect_err("an all-refused run must not silently produce an empty-samples measurement");
        assert!(err.contains("all 3 attempted saves were refused"), "{err}");
    }

    #[test]
    fn measure_save_to_index_asserts_only_the_submitted_series() {
        let m = measure_save_to_index_with(&a_vault(), 3, |_, _| {
            Ok(save_trace(3, &[12.0, 8.0, 15.0], 0, 0, &[]))
        })
        .expect("a fully submitted run is accepted");
        assert_eq!(m.samples.values(), &[12.0, 8.0, 15.0]);
        assert!(
            m.reported.is_empty(),
            "nothing to report when nothing degraded"
        );
        assert!(m.degraded.is_none());
    }

    #[test]
    fn a_clean_save_run_still_states_its_handle_missing_count() {
        // The lead's binding condition on task #94's prewarm_index (F-450):
        // the run must REPORT handle_missing, not merely refuse when it is
        // non-zero. Pre-warming the index is a setup change that turns a
        // failing measurement into a passing one, and this project's four
        // worst defects were all instruments that flattered the figure. A
        // zero that is printed is evidence the window closed; a zero that is
        // only implied by the absence of an error is indistinguishable from
        // nobody having looked.
        let m = measure_save_to_index_with(&a_vault(), 3, |_, _| {
            Ok(save_trace(3, &[12.0, 8.0, 15.0], 0, 0, &[]))
        })
        .expect("a fully submitted run is accepted");
        let census = m
            .save_census
            .expect("every save measurement carries its census, clean or not");
        assert_eq!(
            census,
            SaveCensus {
                attempted: 3,
                submitted: 3,
                writer_refused: 0,
                handle_missing: 0,
            }
        );

        let outcome = outcome_from(&PRF03_SAVE_TO_INDEX, Profile::Reference, a_vault(), m)
            .expect("a clean run produces an outcome");
        let report = outcome.to_report();
        assert!(
            report.contains("0 handle-missing"),
            "the zero must appear on the face of the report: {report}"
        );
        assert!(report.contains("3 attempted"), "{report}");
        let json = outcome.to_json();
        assert!(json.contains("\"handle_missing\": 0"), "{json}");
        assert!(json.contains("\"save_path_census\""), "{json}");
    }

    #[test]
    fn a_degraded_save_run_carries_the_refusal_count_into_the_census_too() {
        // The census is the denominator's full statement, so a run that
        // degrades on backpressure must still say how the attempts split.
        let m = measure_save_to_index_with(&a_vault(), 3, |_, _| {
            Ok(save_trace(3, &[12.0, 8.0], 0, 1, &[0.4]))
        })
        .expect("writer_refused alone must not fail the run");
        assert_eq!(
            m.save_census
                .expect("census travels with a degraded run too"),
            SaveCensus {
                attempted: 3,
                submitted: 2,
                writer_refused: 1,
                handle_missing: 0,
            }
        );
    }

    #[test]
    fn a_budget_with_no_countable_unit_of_work_carries_no_census() {
        // The other half: nothing but NFR-PRF-03 has a census, and an empty
        // one must not print as a row of zeroes that reads like a measured
        // result. A rebuild does not attempt saves.
        let outcome = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[19_000.0])),
        )
        .expect("a rebuild measurement is an outcome");
        assert!(outcome.save_census.is_none());
        assert!(
            !outcome.to_report().contains("save path"),
            "{}",
            outcome.to_report()
        );
        assert!(!outcome.to_json().contains("save_path_census"));
    }

    #[test]
    fn measure_save_to_index_refuses_a_run_with_a_missing_handle() {
        let err = measure_save_to_index_with(&a_vault(), 3, |_, _| {
            Ok(save_trace(3, &[12.0, 8.0], 1, 0, &[]))
        })
        .expect_err("handle_missing is a hard zero");
        assert!(err.contains("F-382"), "{err}");
    }

    #[test]
    fn measure_save_to_index_reports_and_degrades_a_run_with_a_refusal() {
        // "fail the number rather than the run" (F-421): the run is not an
        // Err -- a real Measurement comes back, with writer-refused reported
        // beside the submitted series -- but it must be marked degraded, so
        // an otherwise-fast p95 cannot pass as though 1,000 saves reached the
        // index when fewer did.
        let m = measure_save_to_index_with(&a_vault(), 3, |_, _| {
            Ok(save_trace(3, &[12.0, 8.0], 0, 1, &[0.4]))
        })
        .expect("writer_refused alone must not fail the run");
        assert_eq!(
            m.samples.values(),
            &[12.0, 8.0],
            "only submitted saves feed the p95"
        );
        assert_eq!(m.reported.len(), 1);
        assert_eq!(m.reported[0].label, "writer-refused");
        assert_eq!(m.reported[0].samples.values(), &[0.4]);
        let reason = m.degraded.expect("a refusal must set degraded");
        assert!(reason.contains("1 of 3"), "{reason}");
        assert!(reason.contains("F-421"), "{reason}");
    }

    #[test]
    fn a_failed_save_measurement_propagates_rather_than_being_absorbed() {
        let err = measure_save_to_index_with(&a_vault(), 1, |_, _| Err("boom".into()))
            .expect_err("a failed save path is not a measurement");
        assert_eq!(err, "boom");
    }

    // --------------------------------------------------------------- #107

    /// A directly-constructed trace, bypassing `StartupTrace::parse` and its
    /// six rejections -- this only exercises `measure_serat_term_with`'s own
    /// reduction, not parsing, so the marks only need to be internally
    /// consistent for `Terms::from_marks`, not a JSON line.
    fn fake_trace(
        main: f64,
        state_ready: f64,
        runtime_ready: f64,
        setup_done: f64,
        window_created: f64,
        window_visible: f64,
        vault_usable: f64,
    ) -> trace::StartupTrace {
        trace::StartupTrace {
            main_ms: main,
            state_ready_ms: state_ready,
            runtime_ready_ms: runtime_ready,
            setup_done_ms: setup_done,
            window_created_ms: window_created,
            window_visible_ms: window_visible,
            vault_usable_ms: vault_usable,
            t0_source: "harness".to_string(),
            profile: "release".to_string(),
            vault: "20k".to_string(),
            last_open: String::new(),
            paint_evidence_ms: None,
            raf2_ms: None,
            tree: false,
            tree_first_screen_ms: None,
        }
    }

    #[test]
    fn the_tree_first_screen_number_is_reported_in_the_row_when_the_build_declares_a_tree() {
        // NFR-PRF-01, task #141. Mutant, run by hand: drop the `tree_first_screen`
        // push in `measure_serat_term_with` and the series is absent.
        let mut a = fake_trace(10.0, 15.0, 20.0, 30.0, 40.0, 50.0, 70.0);
        a.tree = true;
        a.tree_first_screen_ms = Some(66.0);
        let mut b = fake_trace(20.0, 30.0, 40.0, 60.0, 80.0, 100.0, 140.0);
        b.tree = true;
        b.tree_first_screen_ms = Some(131.0);
        let m = measure_serat_term_with(&a_vault(), 2, |_, _| Ok(vec![a.clone(), b.clone()]))
            .expect("accepted");
        let series = m
            .reported
            .iter()
            .find(|r| r.label == "tree-first-screen")
            .expect("the tree series is reported");
        assert_eq!(series.samples.values(), &[66.0, 131.0]);
        // And the asserted series is untouched by it.
        assert_eq!(m.samples.values(), &[45.0, 90.0]);
    }

    #[test]
    fn measure_serat_term_asserts_the_serat_term_and_reports_three_other_series() {
        // main=10, state_ready=15 (S_pre=5), runtime_ready=20 (P_builder=5),
        // setup_done=30 (S_setup=10), window_created=40 (P_webview=10),
        // window_visible=50 (S_shell=10), vault_usable=70 (S_settle=20).
        // Serat term = 5+10+10+20 = 45; platform term = 10+5+10 = 25; sum = 70.
        let a = fake_trace(10.0, 15.0, 20.0, 30.0, 40.0, 50.0, 70.0);
        // A second trace, scaled up, so the reduction is seen to run per-trace
        // rather than reusing the first one's numbers.
        let b = fake_trace(20.0, 30.0, 40.0, 60.0, 80.0, 100.0, 140.0);

        let m = measure_serat_term_with(&a_vault(), 2, |_, _| Ok(vec![a.clone(), b.clone()]))
            .expect("two consistent traces are accepted");

        assert_eq!(
            m.samples.values(),
            &[45.0, 90.0],
            "the asserted series is the Serat term"
        );
        assert_eq!(m.reported.len(), 3);
        let labels: Vec<&str> = m.reported.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"platform-term"), "{labels:?}");
        assert!(
            labels.contains(&"window-visible (legacy total)"),
            "{labels:?}"
        );
        assert!(
            labels.contains(&"vault-usable (legacy total)"),
            "{labels:?}"
        );
        assert_eq!(
            m.reported
                .iter()
                .find(|r| r.label == "platform-term")
                .expect("present")
                .samples
                .values(),
            &[25.0, 50.0]
        );
        assert_eq!(
            m.reported
                .iter()
                .find(|r| r.label == "window-visible (legacy total)")
                .expect("present")
                .samples
                .values(),
            &[50.0, 100.0]
        );
        assert_eq!(
            m.reported
                .iter()
                .find(|r| r.label == "vault-usable (legacy total)")
                .expect("present")
                .samples
                .values(),
            &[70.0, 140.0]
        );
    }

    #[test]
    fn a_failed_serat_term_measurement_propagates_rather_than_being_absorbed() {
        let err = measure_serat_term_with(&a_vault(), 1, |_, _| Err("boom".into()))
            .expect_err("a failed cold-start batch is not a measurement");
        assert_eq!(err, "boom");
    }

    // ------------------------------------------------ closing the survivors

    #[test]
    fn the_rebuild_loop_refuses_a_partial_index_and_takes_per_phase_medians() {
        // The mutation sweep found `check_note_count(..)?` could be dropped from
        // measure_full_rebuild with every test still green, because nothing
        // exercised the loop. It does now, through an injected rebuild.
        fn timing(total_ms: u64, notes: usize, parse_ms: u64) -> subject::RebuildTiming {
            subject::RebuildTiming {
                total: Duration::from_millis(total_ms),
                notes,
                // 220,367: what a CORRECT rebuild resolves — the manifest's
                // 216,971 note edges plus the 3,396 asset references the
                // parser extracts and `link_degree` does not count (F-319).
                links_resolved: 220_367,
                phases: subject::RebuildPhases {
                    walk: Duration::from_millis(1),
                    read_hash: Duration::from_millis(2),
                    parse: Duration::from_millis(parse_ms),
                    producer_wait: Duration::from_millis(4),
                    write_fts: Duration::from_millis(5),
                },
            }
        }

        // a_vault() is 20,000 notes and 200 assets; RebuildReport::notes and
        // sqlite_stat1 both count the whole `notes` table, so the oracle every
        // one of these closures is judged against is 20,200, not 20,000
        // (found the hard way while wiring full_rebuild(), see check_note_count).

        // A rebuild that indexed half the vault must not post a fast number.
        let err = measure_full_rebuild_with(
            &a_vault(),
            1,
            |_, _| Ok(timing(1, 10_000, 3)),
            |_| Ok(20_200),
        )
        .expect_err("half a vault is not a passing budget");
        assert!(err.contains("not a passing budget"), "{err}");

        // A failing rebuild propagates rather than being absorbed.
        let err =
            measure_full_rebuild_with(&a_vault(), 1, |_, _| Err("boom".into()), |_| Ok(20_200))
                .expect_err("a failed rebuild is not a measurement");
        assert_eq!(err, "boom");

        // Three good runs: the phase reported is the per-phase median, not the
        // phase of the median run and not the slowest.
        let mut n = 0;
        let m = measure_full_rebuild_with(
            &a_vault(),
            3,
            |_, _| {
                n += 1;
                // 20,200: what a CORRECT rebuild of this vault reports, notes plus
                // assets alike (F-247). 20,000 is the partial-index case below.
                Ok(timing(1_000 * n, 20_200, [30, 10, 20][(n - 1) as usize]))
            },
            |_| Ok(20_200),
        )
        .expect("three complete rebuilds");

        // And stale statistics stop the run, at the level the check runs.
        let err = measure_full_rebuild_with(
            &a_vault(),
            1,
            |_, _| Ok(timing(1, 20_200, 3)),
            |_| Ok(19_968),
        )
        .expect_err("32 rows short of the finished index");
        assert!(err.contains("ix_notes_type"), "{err}");

        // As does a link count that does not match the vault.
        let err = measure_full_rebuild_with(
            &a_vault(),
            1,
            |_, _| {
                let mut t = timing(1, 20_200, 3);
                t.links_resolved = 200_000;
                Ok(t)
            },
            |_| Ok(20_200),
        )
        .expect_err("links dropped");
        assert!(err.contains("dropped"), "{err}");
        assert_eq!(m.samples.len(), 3);
        assert_eq!(m.samples.median(), 2_000.0);
        let phases = m.phases.expect("phases");
        assert_eq!(
            phases.parse,
            Duration::from_millis(20),
            "median parse phase"
        );
    }

    #[test]
    fn the_scale_check_interleaves_the_arms_and_rejects_a_path_that_grew() {
        // F-81: assert_scale_independent was well tested and its only caller
        // could be short-circuited with nothing going red. F-109/F-123: blocked
        // sampling could not decide the claim at all, so the arms interleave.
        let small = a_vault();
        let mut large = a_vault();
        large.notes = 50_000;

        // The arms must alternate, and which one goes first must alternate too.
        let mut order: Vec<usize> = Vec::new();
        let check = measure_scale_independence(
            &small,
            &large,
            4,
            IndexStartState::WarmBoth,
            |vault: &VaultUnderTest, _| {
                order.push(vault.notes);
                Ok(ScaleSample::flat(if vault.notes == 20_000 {
                    300.0
                } else {
                    310.0
                }))
            },
        )
        .expect("four pairs measured");
        assert_eq!(
            order,
            vec![
                20_000, 50_000, 50_000, 20_000, 20_000, 50_000, 50_000, 20_000
            ],
            "the arms must interleave, and the leading arm must alternate — blocked \
             sampling is what made this claim undecidable"
        );
        assert_eq!(check.pairs, 4);
        assert_eq!(check.small_ms, 300.0);
        assert_eq!(check.large_ms, 310.0);
        assert!(
            (check.median_diff_ms - 10.0).abs() < 1e-9,
            "paired median difference"
        );
        assert_eq!(
            check.larger_slower, 4,
            "the larger vault was slower every time"
        );
        assert!(
            check.sign_test_p < 0.13,
            "4/4 one way: p = {}",
            check.sign_test_p
        );

        let line = judge_scale(&check).expect("3% growth is within tolerance");
        assert!(line.contains("PASS"), "{line}");
        assert!(line.contains("interleaved pairs"), "{line}");
        // Task #118: the verdict must name the series it is asserting on, or
        // the change of subject is invisible to everyone reading the output.
        assert!(line.contains("Serat term p95"), "{line}");
        assert!(
            line.contains("does not claim there is none"),
            "the claim is a bound on the effect, not its absence: {line}"
        );

        // The paired difference is what survives machine drift: both arms move
        // together, and the difference does not.
        let mut i = 0;
        let drifting = measure_scale_independence(
            &small,
            &large,
            6,
            IndexStartState::WarmBoth,
            |vault: &VaultUnderTest, _| {
                i += 1;
                // A ramp of 100 ms per spawn, on top of a flat 5 ms real effect.
                let drift = f64::from(i) * 100.0;
                Ok(ScaleSample::flat(
                    drift + if vault.notes == 20_000 { 300.0 } else { 305.0 },
                ))
            },
        )
        .expect("six pairs");
        assert!(
            drifting.median_diff_ms.abs() < 106.0,
            "drift must not read as an effect: {:?}",
            drifting
        );

        // A tail-only regression: the median is flat and the p95 is not. The
        // judgement must fail. This is the case tauri-app-dev warned about --
        // the real walk-deferral effect was in the tail, with a p95 ratio of
        // 1.20 while the median barely moved -- and a judgement on the median
        // or the mean would call it noise.
        let mut n = 0;
        let tail_only = measure_scale_independence(
            &small,
            &large,
            20,
            IndexStartState::WarmBoth,
            |vault: &VaultUnderTest, _| {
                if vault.notes == 20_000 {
                    return Ok(ScaleSample::flat(300.0));
                }
                n += 1;
                // Four slow spawns in twenty: the median is untouched, the p95 is not.
                Ok(ScaleSample::flat(if n > 16 { 900.0 } else { 300.0 }))
            },
        )
        .expect("twenty pairs");
        assert_eq!(tail_only.large_ms, 300.0, "the median is flat");
        assert_eq!(tail_only.small_p95_ms, 300.0);
        assert_eq!(tail_only.large_p95_ms, 900.0, "the tail is not");
        let err = judge_scale(&tail_only)
            .expect_err("a tail-only regression must fail; the budget is a p95");
        assert!(err.contains("proportional to vault size"), "{err}");

        // The shape a path proportional to vault size would produce.
        let grew = ScaleCheck {
            small_ms: 300.0,
            large_ms: 750.0,
            small_p95_ms: 300.0,
            large_p95_ms: 750.0,
            pairs: 30,
            median_diff_ms: 450.0,
            larger_slower: 30,
            sign_test_p: 0.0,
            index_start: IndexStartState::WarmBoth,
            small_spread_pct: 0.0,
            large_spread_pct: 0.0,
            window_visible: ScaleRatio {
                label: "window painted",
                small_p95_ms: 300.0,
                large_p95_ms: 750.0,
            },
            vault_usable: ScaleRatio {
                label: "vault usable",
                small_p95_ms: 300.0,
                large_p95_ms: 750.0,
            },
        };
        let err = judge_scale(&grew).expect_err("2.5x must fail the job");
        assert!(err.contains("proportional to vault size"), "{err}");

        // Zero pairs is a failure, not an empty pass.
        let err =
            measure_scale_independence(&small, &large, 0, IndexStartState::WarmBoth, |_, _| {
                Ok(ScaleSample::flat(300.0))
            })
            .expect_err("no pairs is not a passing scale check");
        assert!(err.contains("at least one pair"), "{err}");

        // And a failure to measure propagates rather than being absorbed.
        let err =
            measure_scale_independence(&small, &large, 4, IndexStartState::WarmBoth, |_, _| {
                Err("no app".into())
            })
            .expect_err("a failed measurement is not a passing scale check");
        assert_eq!(err, "no app");

        // The composition, because a mutation that stopped judge_scale being
        // called at all survived when only the pieces were tested.
        // The two vaults must be distinguishable, or a closure keyed on call
        // order rather than on the vault would pass for the wrong reason -- as
        // this test did until the interleaving made it visible.
        let vaults = || {
            let mut fifty = a_vault();
            fifty.notes = 50_000;
            Ok((a_vault(), fifty))
        };
        let line = scale_verdict(4, IndexStartState::WarmBoth, vaults, |_, _| {
            Ok(ScaleSample::flat(300.0))
        })
        .expect("a flat critical path passes");
        assert!(line.starts_with("PASS"), "{line}");
        assert!(line.contains("300.0 ms at 20k"), "{line}");

        let err = scale_verdict(
            4,
            IndexStartState::WarmBoth,
            vaults,
            |vault: &VaultUnderTest, _| {
                Ok(ScaleSample::flat(if vault.notes == 20_000 {
                    300.0
                } else {
                    750.0
                }))
            },
        )
        .expect_err("2.5x growth must not produce a PASS line");
        assert!(err.contains("proportional to vault size"), "{err}");

        // A vault that cannot be prepared is a failure, not a pass.
        let err = scale_verdict(
            4,
            IndexStartState::WarmBoth,
            || Err("no 50k vault".to_string()),
            |_, _| Ok(ScaleSample::flat(300.0)),
        )
        .expect_err("no vault, no verdict");
        assert_eq!(err, "no 50k vault");
    }

    #[test]
    fn the_scale_check_asserts_the_serat_term_and_only_reports_the_aggregates() {
        // Task #118, architecture §3.4's ruling. This is the test the change
        // exists for, and it is written so that it fails if the subject is put
        // back: the aggregates carry a 2.5x step that would fail the tolerance
        // outright, while the asserted Serat term is flat. A check still
        // judging window_visible_ms cannot pass this.
        let small = a_vault();
        let mut large = a_vault();
        large.notes = 50_000;

        let check = measure_scale_independence(
            &small,
            &large,
            4,
            IndexStartState::WarmBoth,
            |vault: &VaultUnderTest, _| {
                let big = vault.notes == 50_000;
                Ok(ScaleSample {
                    // Flat: nothing of ours grew with the vault.
                    serat_term_ms: 200.0,
                    // 2.5x: the platform term did, which is not ours to gate.
                    window_visible_ms: if big { 3_750.0 } else { 1_500.0 },
                    vault_usable_ms: if big { 4_000.0 } else { 1_600.0 },
                })
            },
        )
        .expect("four pairs measured");

        assert_eq!(
            (check.small_p95_ms, check.large_p95_ms),
            (200.0, 200.0),
            "the asserted series must be the Serat term, not either aggregate"
        );
        assert_eq!(check.median_diff_ms, 0.0);
        assert_eq!(
            check.larger_slower, 0,
            "the sign test must see the asserted series only -- an aggregate that grew must \
             not be able to move the verdict by the back door"
        );

        let line = judge_scale(&check)
            .expect("a flat Serat term passes however the platform term behaved");
        assert!(line.starts_with("PASS"), "{line}");
        assert!(line.contains("Serat term p95 200.0 ms at 20k"), "{line}");
        // And the aggregates are still in the output, with their growth, doing
        // nothing to the verdict. "Remove only the assertion" is only true if
        // the figure survives.
        assert!(
            line.contains("window painted p95 1500.0 → 3750.0 ms (+150.0%)"),
            "{line}"
        );
        assert!(
            line.contains("vault usable p95 1600.0 → 4000.0 ms (+150.0%)"),
            "{line}"
        );
        assert!(line.contains("asserted on"), "{line}");
    }

    #[test]
    fn the_scale_check_reports_its_own_spread_and_the_index_state_both_arms_started_from() {
        // Two of §3.4's requirements on the moved check, and both are about
        // what the verdict must SAY rather than what it computes: the
        // tolerance was never sized against this series' noise, and S_settle
        // is inside the compared quantity so the starting index state decides
        // what was compared.
        let small = a_vault();
        let mut large = a_vault();
        large.notes = 50_000;

        let mut n = 0;
        let check = measure_scale_independence(
            &small,
            &large,
            4,
            IndexStartState::ColdBoth,
            |_vault: &VaultUnderTest, _| {
                n += 1;
                // 200, 210, 220, 230 … within each arm: a 15% spread against a
                // 20% tolerance, which is exactly the uncomfortable case the
                // ruling wants visible rather than buried.
                Ok(ScaleSample::flat(200.0 + f64::from(n % 4) * 10.0))
            },
        )
        .expect("four pairs measured");

        assert!(
            check.small_spread_pct > 0.0 && check.large_spread_pct > 0.0,
            "a series that moved must report a non-zero spread: {check:?}"
        );
        let line = judge_scale(&check).expect("within tolerance");
        assert!(line.contains("spread"), "{line}");
        assert!(
            line.contains("finding to escalate, not a tolerance to widen"),
            "the spread must arrive with what to do about it: {line}"
        );
        assert!(
            line.contains("both arms started COLD"),
            "the run must record which index state it compared: {line}"
        );
    }

    #[test]
    fn a_scale_check_whose_arms_start_in_different_index_states_is_refused() {
        // §3.4's precondition for the moved subject. ColdStartVaultGuard
        // equalises cleanup and says nothing about the starting state, so this
        // is a separate check and it refuses rather than caveats: an unequal
        // start does not weaken the comparison, it replaces it with a
        // measurement of which fixture happened to be warm.
        let dir = std::env::temp_dir().join(format!(
            "serat-budget-test-index-start-{}-{}",
            std::process::id(),
            line!()
        ));
        let small = dir.join("small");
        let large = dir.join("large");
        std::fs::create_dir_all(&small).expect("small arm");
        std::fs::create_dir_all(&large).expect("large arm");

        assert_eq!(
            equal_index_start(&small, &large).expect("neither arm has an index"),
            IndexStartState::ColdBoth
        );

        std::fs::create_dir_all(small.join(".serat")).expect("warm the small arm only");
        let err = equal_index_start(&small, &large)
            .expect_err("one warm arm and one cold arm is not a comparison");
        assert!(err.contains("20k arm started warm"), "{err}");
        assert!(err.contains("50k arm started cold"), "{err}");
        assert!(err.contains("S_settle"), "{err}");

        std::fs::create_dir_all(large.join(".serat")).expect("warm the large arm too");
        assert_eq!(
            equal_index_start(&small, &large).expect("both arms warm"),
            IndexStartState::WarmBoth
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reported_figure_prints_no_verdict_and_cannot_fail_a_build() {
        // Task #118's whole point, at the one seam where "demoted" could
        // silently mean "still gating": an ungated Outcome over a breached
        // limit. It must still be a real, complete report -- value, limit,
        // samples, JSON -- and it must not print PASS or FAIL, because a
        // verdict on a series nothing asserts teaches a reader to ignore the
        // word.
        let breached = Outcome {
            budget: PRF01_WINDOW_VISIBLE,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[1_575.6, 1_580.0, 1_590.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(!breached.gates(), "PRD v1.2 does not gate this figure");
        assert!(
            !breached.passed(),
            "the arithmetic is still done: 1575.6 ms is over 400 ms"
        );
        let report = breached.to_report();
        assert!(
            report.starts_with("REPORTED"),
            "an ungated figure must not print a verdict: {report}"
        );
        assert!(!report.contains("FAIL"), "{report}");
        assert!(report.contains("not a gate"), "{report}");
        assert!(
            report.contains(PRF01_SERAT_TERM.id),
            "the report must name the budget that does gate NFR-PRF-01: {report}"
        );
        // The figure itself is still there. Demoted, not deleted.
        assert!(report.contains("1575.6"), "{report}");
        assert!(breached.to_json().contains("\"gated\": false"));
        assert!(breached.to_json().contains("\"passed\": false"));

        // And a gated budget is unaffected: it still says FAIL.
        let gated = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[40_000.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(gated.gates());
        assert!(gated.to_report().starts_with("FAIL"));
        assert!(gated.to_json().contains("\"gated\": true"));
    }

    #[test]
    fn stale_analyze_statistics_are_caught() {
        // F-47: the only scale at which this is visible is the real vault.
        // 20000 % 512 = 32, and 32 never crosses the 5% (1,000-note) threshold.
        check_analyze_freshness(20_000, 20_000).expect("statistics describe the finished index");
        let err = check_analyze_freshness(19_968, 20_000).expect_err("32 notes short");
        assert!(err.contains("ix_notes_type"), "{err}");
        assert!(err.contains("full scan"), "{err}");
    }

    #[test]
    fn the_analyze_check_refuses_a_vault_too_small_to_show_the_defect() {
        // F-128. Below ~10,000 notes the opportunistic re-ANALYZE fires every
        // batch, so a green result would mean nothing was tested. Refusing beats
        // passing: a guard that quietly stops guarding is worse than none.
        let err = check_analyze_freshness(400, 400).expect_err("the smoke vault cannot show it");
        assert!(err.contains("at least 10,000 notes"), "{err}");
        check_analyze_freshness(20_000, 20_000).expect("the 20k vault can");
    }

    #[test]
    fn dropped_or_over_counted_links_are_caught() {
        check_links_resolved(218_975, 218_975).expect("every link resolved");
        let dropped = check_links_resolved(200_000, 218_975).expect_err("links lost");
        assert!(dropped.contains("dropped"), "{dropped}");
        // The bug rust-core-dev hit: rows touched counted as links resolved.
        let over = check_links_resolved(230_000, 218_975).expect_err("over-counted");
        assert!(over.contains("other than resolved links"), "{over}");
    }

    #[test]
    fn the_producer_wait_phase_is_labelled_for_what_it_measures() {
        // A column called "write" holding a small number would settle open
        // design question 1 backwards: near zero means the writer KEPT UP.
        let outcome = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(subject::RebuildPhases {
                walk: Duration::from_millis(500),
                read_hash: Duration::from_millis(3_000),
                parse: Duration::from_millis(6_000),
                producer_wait: Duration::from_millis(40),
                write_fts: Duration::from_millis(1_900),
            }),
        )
        .expect("valid");
        let report = outcome.to_report();
        assert!(report.contains("producer waited"), "{report}");
        assert!(
            report.contains("BLOCKED on the writer"),
            "the reading must travel with the number: {report}"
        );
        // And it must be findable: a reader scanning the block for the
        // explanation looks for a labelled line, not for a sentence.
        assert!(
            report
                .lines()
                .any(|l| l.starts_with("  reading ") && l.contains("BLOCKED on the writer")),
            "{report}"
        );
        assert!(!report.contains("write rows"), "{report}");
    }

    #[test]
    fn the_fts_check_still_covers_a_run_where_the_writer_kept_up() {
        // Gating F-38 on the producer's wait would have skipped the check on
        // exactly the healthy runs it must cover, since a fast writer means the
        // producer never blocks.
        let fast_writer_no_fts = subject::RebuildPhases {
            walk: Duration::from_millis(500),
            read_hash: Duration::from_millis(3_000),
            parse: Duration::from_millis(6_000),
            producer_wait: Duration::ZERO,
            write_fts: Duration::ZERO,
        };
        let err = check_bulk_load_shape(&fast_writer_no_fts).expect_err("still checked");
        assert!(err.contains("FTS triggers"), "{err}");
    }

    #[test]
    fn the_p95_statistic_is_the_ninety_fifth_percentile() {
        // 0.95 -> 0.90 survived the first sweep: at n = 20 nearest rank moves
        // from the 19th sample to the 18th, and nothing looked at the 18th.
        let mut ms = vec![100.0; 18];
        ms.push(500.0);
        ms.push(9_999.0);
        let s = samples(&ms);
        assert_eq!(s.statistic(Statistic::P95), 500.0, "the 19th of 20 sorted");
        assert_ne!(s.statistic(Statistic::P95), s.percentile(0.90));
    }

    #[test]
    fn a_result_file_is_actually_written_and_holds_the_numbers() {
        // "write_result writes nothing" survived: nothing read the file back.
        let dir = std::env::temp_dir().join(format!("budget-result-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[19_000.0, 21_000.0, 20_000.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        let path = write_result_to(&dir, &outcome).expect("write");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(
            written.contains("\"budget_id\": \"prf04-full-rebuild\""),
            "{written}"
        );
        assert!(written.contains("\"value_ms\": 20000.000"), "{written}");
        assert!(
            written.contains("\"samples_ms\": [19000.000, 21000.000, 20000.000]"),
            "{written}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_outcome_taken_in_the_wrong_environment_is_not_a_gate_number() {
        // "gate_eligible always true" survived: the flag was tested on
        // Environment and never through the Outcome that carries it into a
        // report.
        let scanner_off = env::Environment {
            os: env::Os::Windows,
            cores: 16,
            realtime_scanner: Some(false),
            power_plan: Some("Energy Star".into()),
            cpu_load_percent: None,
            concurrent_build_processes: None,
        };
        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: scanner_off,
            samples: samples(&[20_000.0]),
            reported: Vec::new(),
            phases: None,
            degraded: None,
            save_census: None,
        };
        assert!(outcome.passed(), "20 s is inside 30 s");
        assert!(
            !outcome.gate_eligible(),
            "but a scanner-off run is not the number the charter gate is judged on"
        );
        assert!(
            outcome.to_report().contains("NOT A GATE NUMBER"),
            "{}",
            outcome.to_report()
        );
        assert!(
            outcome.to_json().contains("\"gate_eligible\": false"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn every_budget_has_a_job_in_the_ci_workflow() {
        // NFR-PRF-08's acceptance criterion is Inspect. This was a shell loop
        // over a grep, and a grep that matches nothing runs zero iterations and
        // exits 0 — the job whose purpose is catching an unasserted budget was
        // itself unasserted. Iterating P0_BUDGETS instead cannot pass vacuously.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));

        // Task #118: the two pre-split aggregates are no longer in
        // P0_BUDGETS, so this no longer maps them onto prf01-cold-start. They
        // are still measured -- by that same job, which is now a reporting job
        // with the scale check as its only assertion -- and the test that they
        // stay measured is the one below, not this one. NFR-PRF-08 is about
        // budgets having asserting jobs, and they are no longer budgets.
        assert!(!P0_BUDGETS.is_empty());
        for budget in P0_BUDGETS {
            assert!(
                text.contains(&format!("\n  {}:", budget.id)),
                "budget {} has no asserting job in budgets.yml (NFR-PRF-08)",
                budget.id
            );
        }
    }

    /// The P1 budgets whose gate is a bin's `--assert` rather than a `Budget` run through
    /// `run_job`: (workflow job id, bin, requirement). A job that quietly lost its
    /// `--assert` would go green on every number, which is the F-30 failure again.
    const P1_GATED: &[(&str, &str, &str)] = &[
        ("p1-tree-path", "p1-tree-path", "FR-VLT-01"),
        ("p1-external-change", "p1-external-change", "FR-VLT-04"),
        ("p1-switcher", "p1-switcher", "NFR-PRF-06"),
    ];

    /// The P1 jobs whose bin prints REPORT-ONLY and exits 0 on a runner that is not gate
    /// eligible (task #165, ruled in #166). Their job name must say so and name the
    /// reference machine; every other P1 job's name must not claim it.
    const P1_REPORT_ONLY_ON_HOSTED: &[&str] = &["FR-VLT-04"];

    /// Whether the run line at `lines[idx]` keeps the exit status of the bin it runs. A line
    /// with a pipe (`bin | tee out`) returns tee's status under a bare `run:` on Linux
    /// (`bash -e`, no pipefail), so a breach would read green; it is safe only if its step
    /// says `shell: bash` (GitHub then adds `-o pipefail`) or `set -o pipefail`.
    fn run_line_keeps_exit_status(lines: &[&str], idx: usize) -> bool {
        let code = |l: &str| l.split('#').next().unwrap_or("").to_owned();
        if !code(lines[idx]).contains('|') {
            return true;
        }
        let is_step = |l: &str| l.trim_start().starts_with("- ");
        let mut from = idx;
        while from > 0 && !is_step(lines[from]) {
            from -= 1;
        }
        let mut to = idx + 1;
        while to < lines.len() && !is_step(lines[to]) {
            to += 1;
        }
        lines[from..to].iter().any(|l| {
            let c = code(l);
            c.trim() == "shell: bash" || c.contains("set -o pipefail")
        })
    }

    /// Whether workflow `text` has a job `job` that really gates on `bin`: a run line
    /// invoking `--bin <bin> ` with `--assert` before any `#`, no `||` on that line (it
    /// would swallow the exit code), and no `continue-on-error:` anywhere in the job.
    fn job_asserts(text: &str, job: &str, bin: &str) -> bool {
        let head = format!("\n  {job}:");
        let Some(start) = text.find(&head) else {
            return false;
        };
        // The job ends at the next key indented exactly two spaces.
        let mut body: Vec<&str> = Vec::new();
        for l in text[start + head.len()..].lines() {
            let key = l.starts_with("  ")
                && !l.starts_with("   ")
                && !l.trim().is_empty()
                && !l.trim_start().starts_with('#');
            if key {
                break;
            }
            body.push(l);
        }
        let code = |l: &str| l.split('#').next().unwrap_or("").to_owned();
        if body.iter().any(|l| code(l).contains("continue-on-error:")) {
            return false;
        }
        body.iter().enumerate().any(|(i, l)| {
            let c = code(l);
            c.contains(&format!("--bin {bin} "))
                && c.contains("--assert")
                && !c.contains("||")
                && run_line_keeps_exit_status(&body, i)
        })
    }

    #[test]
    fn every_p1_gated_budget_has_a_job_whose_run_line_asserts() {
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));
        for (job, bin, requirement) in P1_GATED {
            assert!(
                job_asserts(&text, job, bin),
                "{requirement}: job {job} has no un-swallowed run line invoking `--bin {bin}` with --assert (NFR-PRF-08)"
            );
        }
    }

    #[test]
    fn a_p1_job_says_report_only_in_its_name_exactly_when_its_bin_does() {
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));
        let mut report_only = 0;
        for (job, _bin, requirement) in P1_GATED {
            let name = workflow_job_name(&workflow_job(&text, job));
            let declared = P1_REPORT_ONLY_ON_HOSTED.contains(requirement);
            assert_eq!(
                name.contains("report-only"),
                declared,
                "{requirement}: job name {name:?} and P1_REPORT_ONLY_ON_HOSTED disagree (task #166)"
            );
            if declared {
                report_only += 1;
                assert!(
                    name.contains("reference machine"),
                    "{requirement}: a report-only job must name where it is asserted: {name:?}"
                );
            }
        }
        assert!(report_only >= 1, "no P1 job is report-only: vacuous");
    }

    /// What is wrong with the asserting Linux twin `<id>-linux` of a budget whose
    /// Windows job is report-only (ADR-0019 point 3, task #168), or `None` if nothing is.
    /// `flag` is what makes the bin assert: `--profile ci` for a `Budget` (the harness
    /// asserts when `gate_eligible()`), `--assert` for a P1 bin. Pure, so the mutants below
    /// can prove each clause bites.
    fn linux_twin_defect(text: &str, id: &str, bin: &str, flag: &str) -> Option<String> {
        let twin = format!("{id}-linux");
        let head = format!("\n  {twin}:\n");
        if !text.contains(&head) {
            return Some(format!("{twin}: no asserting ubuntu-latest job exists"));
        }
        let job = workflow_job(text, &twin);
        let name = workflow_job_name(&job);
        if name.contains("report-only") {
            return Some(format!(
                "{twin}: an asserting job must not say report-only: {name:?}"
            ));
        }
        if !name.contains("ubuntu-latest") || !name.contains("RM-1") {
            return Some(format!(
                "{twin}: the name must say where it asserts and that the gate figure is RM-1's: {name:?}"
            ));
        }
        if !job.lines().any(|l| l.trim() == "runs-on: ubuntu-latest") {
            return Some(format!(
                "{twin}: must run on ubuntu-latest (gate_eligible is true there)"
            ));
        }
        let code = |l: &str| l.split('#').next().unwrap_or("").to_owned();
        if job.lines().any(|l| code(l).contains("continue-on-error:")) {
            return Some(format!("{twin}: continue-on-error would fail open"));
        }
        let lines: Vec<&str> = job.lines().collect();
        let drives = lines.iter().enumerate().any(|(i, l)| {
            let c = code(l);
            c.contains(&format!("--bin {bin} "))
                && c.contains(flag)
                && !c.contains("--profile reference")
                && !c.contains("||")
                && run_line_keeps_exit_status(&lines, i)
        });
        if !drives {
            return Some(format!(
                "{twin}: no un-swallowed run line invoking `--bin {bin}` with `{flag}`"
            ));
        }
        let gate = workflow_job(text, "budgets-gate");
        if !gate.contains(&format!("\n      - {twin}\n"))
            || !gate.contains(&format!("check {twin} \"${{{{ needs.{twin}.result }}}}\""))
        {
            return Some(format!(
                "{twin}: missing from budgets-gate's needs or its check list"
            ));
        }
        None
    }

    /// Report-only budgets that have no asserting Linux twin, each with the reason ADR-0019
    /// point 3 ("wherever a gate-eligible runner class can run it") does not apply (task
    /// #190). Closed on purpose: an entry needs a reason, and
    /// `no_linux_twin_is_the_one_windows_only_budget_and_has_no_twin_job` pins the set.
    const NO_LINUX_TWIN: &[(&str, &str)] = &[(
        "prf01-serat-term",
        "NFR-PRF-01's 350 ms is bounded to Windows/WebView2 (PRD v1.2, F-96); the app-driving \
         jobs build and launch serat.exe on windows-latest only, and the Linux figure (RM-2, \
         1,310.4 ms) is a different platform's number that would fail a 350 ms assertion",
    )];

    /// The (job id, bin, flag) of every report-only-on-hosted-Windows budget: the P0 budgets
    /// declared `WhenGateEligible` and the P1 bins in `P1_REPORT_ONLY_ON_HOSTED`.
    fn report_only_budgets_with_twins() -> Vec<(String, String, &'static str)> {
        let mut v: Vec<(String, String, &'static str)> = P0_BUDGETS
            .iter()
            .filter(|b| b.ci_assertion == CiAssertion::WhenGateEligible)
            .filter(|b| !NO_LINUX_TWIN.iter().any(|(id, _)| *id == b.id))
            .map(|b| (b.id.to_string(), b.id.to_string(), "--profile ci"))
            .collect();
        v.extend(
            P1_GATED
                .iter()
                .filter(|(_, _, req)| P1_REPORT_ONLY_ON_HOSTED.contains(req))
                .map(|(job, bin, _)| (job.to_string(), bin.to_string(), "--assert")),
        );
        v
    }

    #[test]
    fn a_report_only_budget_is_asserted_on_a_gate_eligible_linux_job() {
        // ADR-0019 point 3 / NFR-PRF-08 / C-06: where the hosted Windows job reports, a job
        // on ubuntu-latest (which `gate_eligible()` rates eligible) must assert, so a
        // regression still fails the build. A budget therefore has one report-only Windows
        // job and one asserting Linux job; the name check applies to each job on its own.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));
        let twins = report_only_budgets_with_twins();
        assert!(
            twins.len() >= 2,
            "NFR-PRF-04 and FR-VLT-04 are the report-only budgets: this would pass vacuously"
        );
        for (id, bin, flag) in &twins {
            if let Some(defect) = linux_twin_defect(&text, id, bin, flag) {
                panic!("{defect} (ADR-0019 point 3)");
            }
        }
        // The harness side of the same claim: Linux is rated eligible, and on it the
        // report-only budget asserts and the profile's limit is the product limit.
        assert!(linux().gate_eligible());
        assert_eq!(PRF04_FULL_REBUILD.ci_limit_ms, None);
        assert_eq!(
            PRF04_FULL_REBUILD.limit_ms(Profile::Ci),
            PRF04_FULL_REBUILD.product_limit_ms
        );
    }

    #[test]
    fn the_linux_twin_check_is_shown_able_to_fail() {
        // Each clause has a mutant that must be reported, or the check proves nothing.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let good = std::fs::read_to_string(&workflow).expect("budgets.yml");
        let (id, bin, flag) = ("prf04-full-rebuild", "prf04-full-rebuild", "--profile ci");
        assert_eq!(linux_twin_defect(&good, id, bin, flag), None);
        let (pid, pbin, pflag) = ("p1-external-change", "p1-external-change", "--assert");
        assert_eq!(linux_twin_defect(&good, pid, pbin, pflag), None);

        // Replace `from` with `to` inside one job only, so a mutant cannot hit a twin.
        let in_job = |twin: &str, from: &str, to: &str| {
            let job = workflow_job(&good, twin);
            assert!(job.contains(from), "{twin}: {from:?} not found");
            good.replace(&job, &job.replace(from, to))
        };
        let n = workflow_job_name(&workflow_job(&good, "prf04-full-rebuild-linux"));
        let prf = "prf04-full-rebuild-linux";
        let ext = "p1-external-change-linux";
        // (what, defect message it must produce, mutated workflow, which twin it is read as)
        type Twin<'a> = (&'a str, &'a str, &'a str);
        let mutants: Vec<(&str, &str, String, Twin)> = vec![
            ("name says report-only", "must not say report-only",
             in_job(prf, &n, &format!("{n} (report-only)")), (id, bin, flag)),
            ("name loses RM-1", "gate figure is RM-1's",
             in_job(prf, "the gate figure is RM-1's", "a regression gate"), (id, bin, flag)),
            ("runs on windows", "must run on ubuntu-latest",
             in_job(prf, "runs-on: ubuntu-latest", "runs-on: windows-latest"), (id, bin, flag)),
            ("--profile reference instead of ci", "no un-swallowed run line",
             in_job(prf, "-- --profile ci", "-- --profile reference"), (id, bin, flag)),
            ("swallows the exit code", "no un-swallowed run line",
             in_job(prf, "-- --profile ci", "-- --profile ci || true"), (id, bin, flag)),
            ("continue-on-error", "continue-on-error",
             in_job(prf, "    needs: vault\n", "    needs: vault\n    continue-on-error: true\n"), (id, bin, flag)),
            ("left out of budgets-gate needs", "missing from budgets-gate",
             good.replace("      - prf04-full-rebuild-linux\n", ""), (id, bin, flag)),
            ("left out of budgets-gate checks", "missing from budgets-gate",
             good.replace("          check prf04-full-rebuild-linux \"${{ needs.prf04-full-rebuild-linux.result }}\"\n", ""), (id, bin, flag)),
            ("job renamed away", "no asserting ubuntu-latest job exists",
             good.replace("\n  prf04-full-rebuild-linux:\n", "\n  prf04-full-rebuild-lnx:\n"), (id, bin, flag)),
            ("--assert dropped (FR-VLT-04)", "no un-swallowed run line",
             in_job(ext, "--batches 20 --assert |", "--batches 20 |"), (pid, pbin, pflag)),
            ("continue-on-error (FR-VLT-04)", "continue-on-error",
             in_job(ext, "    needs: vault\n", "    needs: vault\n    continue-on-error: true\n"), (pid, pbin, pflag)),
            // The #168 review's blocker: `bin | tee` under a bare `run:` on Linux has no
            // pipefail, so tee's status hides a breach. Only the asserting step is mutated.
            ("no shell: bash on the piped assert step (FR-VLT-04)", "no un-swallowed run line",
             in_job(ext, "      - name: Measure and assert\n        shell: bash\n", "      - name: Measure and assert\n"), (pid, pbin, pflag)),
        ];
        assert_eq!(mutants.len(), 12);
        for (what, expect, text, (i, b, f)) in &mutants {
            assert_ne!(
                text, &good,
                "the mutant {what:?} did not change the workflow"
            );
            let defect = linux_twin_defect(text, i, b, f)
                .unwrap_or_else(|| panic!("mutant {what:?} was not caught"));
            assert!(
                defect.contains(expect),
                "mutant {what:?} was caught by the wrong clause: {defect:?}, wanted {expect:?}"
            );
        }
    }

    #[test]
    fn no_linux_twin_is_the_one_windows_only_budget_and_has_no_twin_job() {
        // NFR-PRF-01, NFR-PRF-08, C-06, ADR-0019 points 2 and 3, task #190. The exemption
        // is for a budget that cannot run on Linux by design; it must name a real
        // WhenGateEligible budget, say why, and not coexist with a twin job (a stale
        // exemption would hide a twin that nothing checks).
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow).expect("budgets.yml");
        let ids: Vec<&str> = NO_LINUX_TWIN.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["prf01-serat-term"], "the exemption set is closed");
        for (id, why) in NO_LINUX_TWIN {
            let b = P0_BUDGETS
                .iter()
                .find(|b| b.id == *id)
                .unwrap_or_else(|| panic!("{id} is not a P0 budget"));
            assert_eq!(b.ci_assertion, CiAssertion::WhenGateEligible, "{id}");
            assert!(why.contains("Windows") && why.len() > 60, "{id}: {why:?}");
            assert!(
                !text.contains(&format!("\n  {id}-linux:\n")),
                "{id} has a Linux job, so it must leave NO_LINUX_TWIN and get the twin checks"
            );
        }
        // Every other WhenGateEligible P0 budget is checked for a twin, so the exemption
        // cannot swallow one.
        let checked: Vec<String> = report_only_budgets_with_twins()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert!(
            checked.contains(&"prf04-full-rebuild".to_string()),
            "{checked:?}"
        );
        assert!(
            !checked.contains(&"prf01-serat-term".to_string()),
            "{checked:?}"
        );
    }

    #[test]
    fn the_serat_term_is_report_only_on_hosted_windows_and_asserts_where_a_gate_number_exists() {
        // NFR-PRF-01, NFR-PRF-08, US-35, C-06, ADR-0019 point 1 and 8, task #190. Run 427's
        // figures: n=20, min 237.5, max 711.6, p95 546.3 against 350 on a 4-core scanner-off
        // runner. Same samples, three environments.
        let mut v: Vec<f64> = vec![237.5; 3];
        v.extend([
            260.0, 270.0, 280.0, 290.0, 295.0, 300.0, 305.0, 310.0, 315.0, 320.0,
        ]);
        v.extend([330.0, 340.0, 380.0, 450.0, 546.3, 711.6]);
        assert_eq!(v.len(), 19);
        v.push(520.0);
        let hosted = outcome_on(PRF01_SERAT_TERM, Profile::Ci, hosted_windows(), &v);
        assert!(!hosted.gate_eligible() && hosted.report_only_here() && !hosted.asserted());
        assert!(!hosted.determinate(), "the samples straddle, and say so");
        assert!(
            hosted.to_report().starts_with("REPORT-ONLY"),
            "{}",
            hosted.to_report()
        );
        assert!(
            enforce(&hosted),
            "a slow hosted figure must not fail the build"
        );
        // The product limit is unchanged and the same samples assert on RM-1.
        assert_eq!(PRF01_SERAT_TERM.product_limit_ms, 350.0);
        assert_eq!(PRF01_SERAT_TERM.ci_limit_ms, None);
        let rm1 = outcome_on(PRF01_SERAT_TERM, Profile::Reference, hosted_windows(), &v);
        assert!(rm1.asserted() && !rm1.report_only_here());
        assert!(
            rm1.to_report().starts_with("INDETERMINATE"),
            "{}",
            rm1.to_report()
        );
        assert!(
            !enforce(&rm1),
            "on the reference profile a straddle still fails"
        );
        // And on a gate-eligible runner under ci the straddle is still a failure.
        let eligible = outcome_on(PRF01_SERAT_TERM, Profile::Ci, linux(), &v);
        assert!(eligible.asserted() && !enforce(&eligible));
        assert_eq!(
            job_exit(&PRF01_SERAT_TERM, Ok(hosted)),
            std::process::ExitCode::SUCCESS
        );
        assert_eq!(
            job_exit(&PRF01_SERAT_TERM, Ok(eligible)),
            std::process::ExitCode::FAILURE
        );
        // A degraded measurement is not waived by report-only.
        let mut degraded = outcome_on(PRF01_SERAT_TERM, Profile::Ci, hosted_windows(), &v);
        degraded.degraded = Some("a launch was refused".into());
        assert!(!degraded.report_only_here() && !enforce(&degraded));
    }

    #[test]
    fn the_report_only_windows_job_and_the_asserting_linux_job_each_keep_their_own_name() {
        // The two-way name check applies per job: the Windows job says report-only and
        // names the reference machine; its Linux twin does not say report-only.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow).expect("budgets.yml");
        for (id, _, _) in report_only_budgets_with_twins() {
            let win = workflow_job_name(&workflow_job(&text, &id));
            let lin = workflow_job_name(&workflow_job(&text, &format!("{id}-linux")));
            assert!(
                win.contains("report-only") && win.contains("reference machine"),
                "{win:?}"
            );
            assert!(!lin.contains("report-only"), "{lin:?}");
            assert_ne!(win, lin, "{id}: two jobs must not share a check name");
        }
    }

    #[test]
    fn job_asserts_accepts_the_good_job_and_rejects_each_sabotage() {
        let wrap = |run: &str, extra: &str| {
            format!(
                "jobs:\n  p1-x:\n    steps:\n      - name: m\n{extra}        run: {run}\n  next:\n    steps: []\n"
            )
        };
        let good = "cargo run --release -p budget --bin p1-x -- --vault 20k --assert";
        assert!(job_asserts(&wrap(good, ""), "p1-x", "p1-x"));
        // --assert hidden behind a comment.
        let commented = "cargo run -p budget --bin p1-x -- --vault 20k # --assert";
        assert!(!job_asserts(&wrap(commented, ""), "p1-x", "p1-x"));
        // The step may not fail open.
        assert!(!job_asserts(
            &wrap(good, "        continue-on-error: true\n"),
            "p1-x",
            "p1-x"
        ));
        // The exit code swallowed on the run line.
        let swallowed = format!("{good} || true");
        assert!(!job_asserts(&wrap(&swallowed, ""), "p1-x", "p1-x"));
        // A pipe under a bare `run:` loses the bin's exit status; `shell: bash` keeps it.
        let piped = format!("{good} | tee out.txt");
        assert!(!job_asserts(&wrap(&piped, ""), "p1-x", "p1-x"));
        assert!(job_asserts(
            &wrap(&piped, "        shell: bash\n"),
            "p1-x",
            "p1-x"
        ));
        assert!(job_asserts(
            &wrap(&format!("set -o pipefail; {piped}"), ""),
            "p1-x",
            "p1-x"
        ));
        // A continue-on-error in a different job is not this job's problem.
        let other = format!("{}  other:\n    continue-on-error: true\n", wrap(good, ""));
        assert!(job_asserts(&other, "p1-x", "p1-x"));
        assert!(!job_asserts(&wrap(good, ""), "missing", "p1-x"));
    }

    #[test]
    fn the_demoted_aggregates_are_still_measured_by_a_job_that_ci_runs() {
        // The other half of #118, and the half that is easy to lose: "remove
        // only the assertion" is true only while something still takes the
        // measurement. A figure dropped from P0_BUDGETS with no job left
        // running it is not demoted, it is deleted -- and it would look
        // identical to every other test in this file.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));
        assert!(
            text.contains("\n  prf01-cold-start:"),
            "the job that measures the demoted aggregates is not in budgets.yml"
        );
        assert!(!PRF01_REPORTED.is_empty());
        for budget in PRF01_REPORTED {
            assert!(
                !budget.gated,
                "{} is listed as reported but still gates",
                budget.id
            );
        }
    }

    #[test]
    fn the_manifest_reader_finds_what_it_needs() {
        let manifest = r#"{
  "format_version": 1,
  "seed": 20260903,
  "tree_digest_sha256": "abc123",
  "counts": { "notes": 20000, "assets": 200 }
}"#;
        assert_eq!(
            json_string(manifest, "tree_digest_sha256").as_deref(),
            Some("abc123")
        );
        assert_eq!(json_number(manifest, "format_version"), Some(1.0));
        assert_eq!(json_number(manifest, "notes"), Some(20_000.0));
        assert_eq!(json_string(manifest, "nope"), None);
    }

    /// Every `--flag` the usage text documents: the ones in the synopsis line
    /// (`[--flag ...]`) and the ones that open an entry in the flag list. Prose that
    /// merely mentions a flag (the `--iteration` typo it warns about) is not documentation.
    fn usage_flags(text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut lines = text.lines();
        let synopsis = lines.next().unwrap_or_default();
        let entries = lines
            .filter(|l| l.starts_with("  --"))
            .map(|l| l.trim_start().split("  ").next().unwrap_or_default());
        for chunk in std::iter::once(synopsis).chain(entries) {
            for word in chunk.split(|c: char| !(c == '-' || c.is_ascii_alphanumeric())) {
                if word.starts_with("--") && word.len() > 2 && !out.iter().any(|f| f == word) {
                    out.push(word.to_owned());
                }
            }
        }
        out
    }

    /// NFR-PRF-01, NFR-PRF-08, US-35 (task #187): a flag a bin documents is a flag its
    /// run accepts. The usage text is the documentation; each `--flag` in it, for every
    /// budget the common parser serves, goes through the real entry point -- the
    /// cold-start composition for its own budget -- and none may be refused.
    #[test]
    fn every_documented_flag_is_accepted_by_the_run_that_documents_it() {
        use std::process::ExitCode;
        let budgets = [
            &PRF01_WINDOW_VISIBLE,
            &PRF01_VAULT_USABLE,
            &PRF01_SERAT_TERM,
            &PRF03_SAVE_TO_INDEX,
            &PRF04_FULL_REBUILD,
        ];
        for budget in budgets {
            let flags = usage_flags(&usage(budget));
            assert!(
                flags.iter().any(|f| f == "--profile"),
                "{}: flag extraction found nothing",
                budget.id
            );
            for flag in flags {
                let args: Vec<String> = match flag.as_str() {
                    "--profile" => vec![flag.clone(), "ci".into()],
                    "--iterations" => vec![flag.clone(), "3".into()],
                    "--root" => vec![flag.clone(), "/x".into()],
                    _ => vec![flag.clone()],
                };
                // Only the cold-start job strips a flag of its own, so only its budget goes
                // through that composition; every other budget reaches the parser directly,
                // so a `--skip-scale` documented for one of them would be refused.
                let parsed = if budget.id == PRF01_WINDOW_VISIBLE.id {
                    let seen = std::cell::RefCell::new(None);
                    let _ = cold_start_job_with(
                        &args,
                        |a| {
                            *seen.borrow_mut() = Some(parse_args(budget, a));
                            ExitCode::FAILURE
                        },
                        |_| ExitCode::FAILURE,
                    );
                    seen.into_inner().expect("reported step ran")
                } else {
                    parse_args(budget, &args)
                };
                assert!(
                    parsed.is_ok(),
                    "{}: documented flag {flag} refused: {:?}",
                    budget.id,
                    parsed.err()
                );
            }
        }
    }

    /// Task #187: the flag reaches the job and not the common parser, which still
    /// refuses an unknown argument; and the usage text names it for cold-start only.
    #[test]
    fn skip_scale_is_stripped_before_the_common_parse_and_is_documented() {
        use std::process::ExitCode;
        let args: Vec<String> = ["--profile", "ci", "--skip-scale"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        // Raw, the common parser refuses it (the defect).
        assert!(parse_args(&PRF01_WINDOW_VISIBLE, &args).is_err());
        // Through the composition, it never arrives.
        let handed = std::cell::RefCell::new(Vec::new());
        let code = cold_start_job_with(
            &args,
            |a| {
                *handed.borrow_mut() = a.to_vec();
                ExitCode::SUCCESS
            },
            |_| ExitCode::FAILURE,
        );
        assert_eq!(code, ExitCode::SUCCESS, "scale check must be skipped");
        assert_eq!(*handed.borrow(), vec!["--profile".to_string(), "ci".into()]);
        assert!(parse_args(&PRF01_WINDOW_VISIBLE, &handed.borrow()).is_ok());
        // Without the flag the scale check runs.
        let ran = std::cell::Cell::new(false);
        let _ = cold_start_job_with(
            &["--profile".to_string(), "ci".into()],
            |_| ExitCode::SUCCESS,
            |_| {
                ran.set(true);
                ExitCode::SUCCESS
            },
        );
        assert!(ran.get(), "scale check must run without --skip-scale");
        // Usage text matches the flag, on cold-start only; another typo is still refused.
        assert!(usage(&PRF01_WINDOW_VISIBLE).contains("--skip-scale"));
        assert!(!usage(&PRF04_FULL_REBUILD).contains("--skip-scale"));
        assert!(parse_args(&PRF04_FULL_REBUILD, &["--skip-scale".to_string()]).is_err());
    }
}
