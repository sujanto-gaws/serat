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
    interval: "apply() submitted through flush() returning -- Serat's own save path, \
               no watcher, no debounce (F-227, F-268; see check_save_path)",
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

/// Every P0 budget, so a CI configuration review (NFR-PRF-08's acceptance
/// criterion is `Inspect`) can check the list against the PRD rather than
/// against somebody's memory.
pub const P0_BUDGETS: &[&Budget] = &[
    &PRF01_WINDOW_VISIBLE,
    &PRF01_VAULT_USABLE,
    &PRF03_SAVE_TO_INDEX,
    &PRF04_FULL_REBUILD,
];

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

    /// Whether the budget held.
    pub fn passed(&self) -> bool {
        self.value_ms() <= self.limit_ms()
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
        let verdict = match (self.determinate(), self.passed()) {
            (false, _) => "INDETERMINATE",
            (true, true) => "PASS",
            (true, false) => "FAIL",
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
        let _ = writeln!(
            s,
            "  samples     n={}  min {:.1}  mean {:.1}  max {:.1} ms  spread {:.1}%",
            self.samples.len(),
            self.samples.min(),
            self.samples.mean(),
            self.samples.max(),
            self.spread_pct(),
        );
        if !self.determinate() {
            let _ = writeln!(
                s,
                "  undecided   the samples straddle the {:.1} ms limit -- this run observed \
                 both a pass and a fail, so it has not settled the question. Re-run with more \
                 iterations, or on a machine steady enough to answer it. Reported as a failure \
                 because a run that decided nothing must not go green.",
                self.limit_ms(),
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
        if self.budget.ci_limit_ms.is_none() && self.profile == Profile::Ci {
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
}

impl Measurement {
    /// A measurement with nothing but the asserted series.
    pub fn new(samples: Samples) -> Measurement {
        Measurement {
            samples,
            phases: None,
            reported: Vec::new(),
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

    /// Attach the phase breakdown.
    #[must_use]
    pub fn with_phases(mut self, phases: subject::RebuildPhases) -> Measurement {
        self.phases = Some(phases);
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
    format!(
        "usage: {id} [--profile reference|ci] [--iterations N] [--help]

  --profile <name>    Which machine profile's threshold applies.
                      One of: reference (default), ci.
  --iterations <N>    How many iterations to run. Default {iterations}, which is
                      what this budget's acceptance criterion names. Any other
                      value marks the result indicative: it is printed, and it
                      must not be recorded as a gate number.
  --help, -h          Print this and measure nothing.

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

/// Repository root, resolved at compile time from this crate's location.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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

/// Whether a save's timing came from Serat's own save path rather than the
/// watcher.
///
/// P0 findings **F-227, F-229, F-268**. A save routed through the watcher
/// carries FR-VLT-04's 500 ms budget and the 150 ms coalescing window;
/// NFR-PRF-03's 50 ms can only mean the direct path `apply()` then `flush()`
/// takes, since architecture §3.1.1/§3.2 mean no external change can reach
/// the index inside the coalescing window at all. A number measured on the
/// wrong path is not distinguishable from a real one by looking at it — that
/// is what makes this check load-bearing rather than decorative, and it is
/// exactly the shape F-268 found already inside the instrument meant to
/// prevent it.
///
/// # Errors
/// When `path` is not exactly `"save"`.
pub fn check_save_path(path: &str) -> Result<(), String> {
    if path == "save" {
        return Ok(());
    }
    Err(format!(
        "a save timing came from the {path:?} path, not \"save\". NFR-PRF-03 governs Serat's \
         own save path (apply() then flush()), not a save observed through the watcher, which \
         carries FR-VLT-04's 500 ms budget and the 150 ms coalescing window instead (F-227, \
         F-229, F-268). Reporting this number against NFR-PRF-03 would be wrong by \
         construction, not by a measurement error."
    ))
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
        // session, once through cold_start_once (fixed by CleanIndexOnDrop) and once
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
    let measurement = measure(&vault, parsed.iterations)?;
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
    match decide(budget, args, measure) {
        Ok(outcome) => {
            report(&outcome);
            // Indeterminate is not success. A run whose samples straddled the
            // limit saw the budget breached and would go green on the median.
            if outcome.passed() && outcome.determinate() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("FAIL  {} · {}\n  {e}", budget.requirement, budget.id);
            std::process::ExitCode::FAILURE
        }
    }
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
                ok &= outcome.passed() && outcome.determinate();
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

/// NFR-PRF-03's measurement: the save-path interval, asserted, beside the two
/// watcher-path intervals kept for FR-VLT-04 (F-227, F-268).
///
/// # Errors
/// Any failure of the save path, or [`check_save_path`] rejecting a save
/// measured on the watcher path rather than Serat's own.
pub fn measure_save_to_index(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<Measurement, String> {
    measure_save_to_index_with(vault, iterations, |v, i, it| {
        subject::save_to_index(v, i, it)
    })
}

/// As [`measure_save_to_index`], against an injected save path.
///
/// Split out so `check_save_path` refusing a watcher-path measurement can be
/// tested without the real seam existing yet (task I3, the last blocker
/// standing between `subject::save_to_index` and a real number now that I5
/// has landed) — the same reasoning as `measure_full_rebuild_with`, and
/// forced by the same lesson: F-268 was a
/// harness bug nothing exercised, because nothing could inject a `"watcher"`
/// tag to prove the guard fires on it.
///
/// # Errors
/// As [`measure_save_to_index`].
pub fn measure_save_to_index_with<F>(
    vault: &VaultUnderTest,
    iterations: usize,
    save_to_index: F,
) -> Result<Measurement, String>
where
    F: FnOnce(&Path, &Path, usize) -> Result<subject::SaveSamples, String>,
{
    // Outside the vault: the same fix as `measure_full_rebuild_with`, for the
    // same reason (F-372, task #78). No `{i}` suffix needed -- there is one
    // scratch directory for the whole call, not one per iteration -- but the
    // pid still guards against two concurrent `prf03` runs colliding.
    let index_dir = std::env::temp_dir().join(format!("serat-bench-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&index_dir);
    let samples = save_to_index(&vault.path, &index_dir, iterations)?;
    let _ = std::fs::remove_dir_all(&index_dir);
    check_save_path(samples.path)?;
    Ok(Measurement::new(samples.save)
        .reporting(
            "watcher-write-to-visible",
            "File written to row visible, via the watcher -- includes the 150 ms coalescing \
             window of architecture §3.1 step 1. FR-VLT-04's interval, not this budget's \
             (F-227).",
            samples.end_to_end,
        )
        .reporting(
            "watcher-dequeue-to-visible",
            "Watcher event dequeued to row visible. Still a watcher-path interval despite the \
             name -- not this budget's either (F-268).",
            samples.pipeline,
        ))
}

/// The window-painted mark at two vault sizes, measured in interleaved pairs.
#[derive(Clone, Debug)]
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
    small: &VaultUnderTest,
    large: &VaultUnderTest,
    pairs: usize,
    mut measure_one: F,
) -> Result<ScaleCheck, String>
where
    F: FnMut(&VaultUnderTest, usize) -> Result<f64, String>,
{
    // The vaults are arguments rather than fetched here, so a test can drive
    // this without generating 98 MiB and hashing it. `ensure_vault` has its own
    // tests, in tests/vault_gate.rs, against a real vault on disk.
    if pairs == 0 {
        return Err("a scale check needs at least one pair".to_string());
    }
    let mut small_ms = Vec::with_capacity(pairs);
    let mut large_ms = Vec::with_capacity(pairs);
    let mut diffs = Vec::with_capacity(pairs);

    for i in 0..pairs {
        // Alternate which arm goes first, so any within-pair ordering effect —
        // a warm page cache, a thermal ramp — falls on both arms equally.
        let (a, b) = if i % 2 == 0 {
            (small, large)
        } else {
            (large, small)
        };
        let first = measure_one(a, i)?;
        let second = measure_one(b, i)?;
        let (s_ms, l_ms) = if i % 2 == 0 {
            (first, second)
        } else {
            (second, first)
        };
        small_ms.push(s_ms);
        large_ms.push(l_ms);
        diffs.push(l_ms - s_ms);
    }

    let larger_slower = diffs.iter().filter(|d| **d > 0.0).count();
    Ok(ScaleCheck {
        small_ms: median_of(&small_ms),
        large_ms: median_of(&large_ms),
        small_p95_ms: percentile_of(&small_ms, 0.95),
        large_p95_ms: percentile_of(&large_ms, 0.95),
        pairs,
        median_diff_ms: median_of(&diffs),
        larger_slower,
        sign_test_p: sign_test_p(pairs, larger_slower),
    })
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
        "PASS  NFR-PRF-01 · scale independence\n  window painted p95 {:.1} ms at 20k, {:.1} ms \
         at 50k over {} interleaved pairs (median {:.1} / {:.1})\n  paired median difference \
         {:+.1} ms, larger vault slower in {}/{} pairs, sign test p = {:.3} — vault size does \
         not inflate the mark beyond the {:.0}% tolerance (§3.4). This bounds the effect; it \
         does not claim there is none.\n  caveat: every iteration reads a page-cached vault, so \
         no figure here is a cold-VAULT figure",
        check.small_p95_ms,
        check.large_p95_ms,
        check.pairs,
        check.small_ms,
        check.large_ms,
        check.median_diff_ms,
        check.larger_slower,
        check.pairs,
        check.sign_test_p,
        SCALE_TOLERANCE * 100.0
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
pub fn scale_verdict<V, F>(iterations: usize, vaults: V, measure: F) -> Result<String, String>
where
    V: FnOnce() -> Result<(VaultUnderTest, VaultUnderTest), String>,
    F: FnMut(&VaultUnderTest, usize) -> Result<f64, String>,
{
    let (small, large) = vaults()?;
    let check = measure_scale_independence(&small, &large, iterations, measure)?;
    judge_scale(&check)
}

/// Architecture §3.4's load-bearing claim, measured and judged.
///
/// Returns whether it held, having printed the verdict either way.
pub fn run_scale_check(iterations: usize) -> bool {
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
        let _small_guard = subject::ColdStartVaultGuard::watch(&small.path);
        let _large_guard = subject::ColdStartVaultGuard::watch(&large.path);
        scale_verdict(
            iterations,
            || Ok((small, large)),
            |vault, run| {
                subject::cold_start_once(&vault.path, run).map(|trace| trace.window_visible_ms)
            },
        )
    });
    match verdict {
        Ok(line) => {
            println!("{line}");
            true
        }
        Err(e) => {
            println!("FAIL  NFR-PRF-01 · scale independence\n  {e}");
            false
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
        };
        assert!(!over.passed());
        assert!(over.to_report().starts_with("FAIL"));
        assert!(over.to_json().contains("\"passed\": false"));
    }

    #[test]
    fn every_p0_budget_matches_the_number_in_the_prd() {
        // NFR-PRF-08's acceptance criterion is Inspect: "CI configuration review
        // confirms each budget has an asserting job". This is that review, as a
        // test, so the list cannot drift from the PRD unnoticed.
        assert_eq!(P0_BUDGETS.len(), 4);
        assert_eq!(PRF01_WINDOW_VISIBLE.product_limit_ms, 400.0);
        assert_eq!(PRF01_WINDOW_VISIBLE.iterations, 20);
        assert_eq!(PRF01_VAULT_USABLE.product_limit_ms, 1_000.0);
        assert_eq!(PRF01_VAULT_USABLE.iterations, 20);
        assert_eq!(PRF03_SAVE_TO_INDEX.product_limit_ms, 50.0);
        assert_eq!(PRF03_SAVE_TO_INDEX.iterations, 1_000);
        assert_eq!(PRF04_FULL_REBUILD.product_limit_ms, 30_000.0);
        for b in P0_BUDGETS {
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

    // ---------------------------------------------------- F-227/F-229/F-268

    #[test]
    fn check_save_path_accepts_only_save() {
        check_save_path("save").expect("the direct save path is accepted");

        let err = check_save_path("watcher").expect_err("a watcher-path save must be refused");
        assert!(err.contains("watcher"), "{err}");
        assert!(err.contains("NFR-PRF-03"), "{err}");
        assert!(err.contains("F-227"), "{err}");
        assert!(err.contains("F-229"), "{err}");
        assert!(err.contains("F-268"), "{err}");

        // SaveSamples::default() gives path: "" -- the guard must fail closed
        // on the default, not open.
        assert!(check_save_path("").is_err());
    }

    fn save_samples(path: &'static str) -> subject::SaveSamples {
        let mut save = Samples::new();
        save.push(Duration::from_millis(12));
        let mut end_to_end = Samples::new();
        end_to_end.push(Duration::from_millis(400));
        let mut pipeline = Samples::new();
        pipeline.push(Duration::from_millis(180));
        subject::SaveSamples {
            path,
            save,
            end_to_end,
            pipeline,
        }
    }

    #[test]
    fn measure_save_to_index_asserts_the_save_interval_and_reports_the_watcher_ones() {
        // F-268: this is the regression test for the defect the finding is
        // about. Before the fix this measurement asserted `pipeline` -- a
        // watcher-path interval -- and this test would have passed on the
        // wrong series, silently, the same way the real bug did.
        let m = measure_save_to_index_with(&a_vault(), 1, |_, _, _| Ok(save_samples("save")))
            .expect("a save-path measurement is accepted");
        assert_eq!(m.samples.values(), &[12.0]);
        assert_eq!(m.reported.len(), 2);
        let labels: Vec<&str> = m.reported.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"watcher-write-to-visible"), "{labels:?}");
        assert!(labels.contains(&"watcher-dequeue-to-visible"), "{labels:?}");
        // Neither watcher-path series may be silently dropped, or FR-VLT-04
        // loses the only measurement of the interval it actually governs.
        assert_eq!(
            m.reported
                .iter()
                .find(|r| r.label == "watcher-write-to-visible")
                .expect("present")
                .samples
                .values(),
            &[400.0]
        );
    }

    #[test]
    fn a_save_measured_on_the_watcher_path_is_refused_not_silently_asserted() {
        let err = measure_save_to_index_with(&a_vault(), 1, |_, _, _| Ok(save_samples("watcher")))
            .expect_err("a watcher-path measurement must not become an NFR-PRF-03 number");
        assert!(err.contains("watcher"), "{err}");
    }

    #[test]
    fn a_failed_save_measurement_propagates_rather_than_being_absorbed() {
        let err = measure_save_to_index_with(&a_vault(), 1, |_, _, _| Err("boom".into()))
            .expect_err("a failed save path is not a measurement");
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
        let check = measure_scale_independence(&small, &large, 4, |vault, _| {
            order.push(vault.notes);
            Ok(if vault.notes == 20_000 { 300.0 } else { 310.0 })
        })
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
        assert!(
            line.contains("does not claim there is none"),
            "the claim is a bound on the effect, not its absence: {line}"
        );

        // The paired difference is what survives machine drift: both arms move
        // together, and the difference does not.
        let mut i = 0;
        let drifting = measure_scale_independence(&small, &large, 6, |vault, _| {
            i += 1;
            // A ramp of 100 ms per spawn, on top of a flat 5 ms real effect.
            let drift = f64::from(i) * 100.0;
            Ok(drift + if vault.notes == 20_000 { 300.0 } else { 305.0 })
        })
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
        let tail_only = measure_scale_independence(&small, &large, 20, |vault, _| {
            if vault.notes == 20_000 {
                return Ok(300.0);
            }
            n += 1;
            // Four slow spawns in twenty: the median is untouched, the p95 is not.
            Ok(if n > 16 { 900.0 } else { 300.0 })
        })
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
        };
        let err = judge_scale(&grew).expect_err("2.5x must fail the job");
        assert!(err.contains("proportional to vault size"), "{err}");

        // Zero pairs is a failure, not an empty pass.
        let err = measure_scale_independence(&small, &large, 0, |_, _| Ok(300.0))
            .expect_err("no pairs is not a passing scale check");
        assert!(err.contains("at least one pair"), "{err}");

        // And a failure to measure propagates rather than being absorbed.
        let err = measure_scale_independence(&small, &large, 4, |_, _| Err("no app".into()))
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
        let line = scale_verdict(4, vaults, |_, _| Ok(300.0)).expect("a flat critical path passes");
        assert!(line.starts_with("PASS"), "{line}");
        assert!(line.contains("300.0 ms at 20k"), "{line}");

        let err = scale_verdict(4, vaults, |vault, _| {
            Ok(if vault.notes == 20_000 { 300.0 } else { 750.0 })
        })
        .expect_err("2.5x growth must not produce a PASS line");
        assert!(err.contains("proportional to vault size"), "{err}");

        // A vault that cannot be prepared is a failure, not a pass.
        let err = scale_verdict(4, || Err("no 50k vault".to_string()), |_, _| Ok(300.0))
            .expect_err("no vault, no verdict");
        assert_eq!(err, "no 50k vault");
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

        assert!(!P0_BUDGETS.is_empty());
        for budget in P0_BUDGETS {
            // NFR-PRF-01 is one requirement with two marks measured by one job.
            let job = match budget.id {
                "prf01-window-visible" | "prf01-vault-usable" => "prf01-cold-start",
                other => other,
            };
            assert!(
                text.contains(&format!("\n  {job}:")),
                "budget {} has no asserting job in budgets.yml (NFR-PRF-08)",
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
}
