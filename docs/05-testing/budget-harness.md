---
project: Serat
artifact: Budget harness and CI budget jobs
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-08, NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, US-35, C-06
---

# The Budget Harness

NFR-PRF-08: *"Every performance budget shall be asserted in continuous integration; a regression shall fail the build."* This is how. One job per budget, each measured against the benchmark vault, each recording the conditions the number was taken under.

**The harness is red today, deliberately.** `serat-index` has no rebuild entry point (task I4), no incremental save path (I5), and there is no application to start (D2, S1). Every job fails with a message naming the task that unblocks it. A budget job that *skips* when its subject is missing is worse than no job: the build goes green and the budget silently stops being asserted.

## Running one

```
cargo run --release -p budget --bin prf04-full-rebuild -- --profile reference
cargo run --release -p budget --bin prf03-save-to-index -- --profile reference
cargo run --release -p budget --bin prf01-cold-start   -- --profile reference
```

Each job generates the 20k vault if it is missing, **verifies it against its manifest before starting the clock**, runs the measurement, prints a report, writes `bench/out/budgets/<id>.json`, and exits non-zero if the budget was breached or could not be measured. Release build only — a budget measured in a debug build is not a measurement.

`--iterations N` overrides the count for a quick local check and prints a warning that the result is not a gate number.

## The budgets

| Job | Requirement | Statistic | n | Limit | Vault |
|---|---|---|---|---|---|
| `prf01-cold-start` | NFR-PRF-01 · window visible | p95 | 20 | 400 ms | 20k |
| `prf01-cold-start` | NFR-PRF-01 · vault usable | p95 | 20 | 1,000 ms | 20k |
| `prf03-save-to-index` | NFR-PRF-03 | p95 | 1,000 | 50 ms | 20k |
| `prf04-full-rebuild` | NFR-PRF-04 | wall clock | 3 | 30,000 ms | 20k |

The table lives in `bench/budget/src/lib.rs`, next to the requirement each number came from, and a unit test asserts every value against the PRD. NFR-PRF-08's acceptance criterion is `Inspect` — "CI configuration review confirms each budget has an asserting job" — and the `every-budget-has-a-job` CI job performs that review automatically, so a budget added to the PRD without a job fails the build instead of waiting to be noticed.

NFR-PRF-01 is one requirement with two marks on one timeline, so one job asserts both and reports both. "Cold start failed" without saying which half is a bug report nobody can act on.

### Two thresholds, not one

The product budget belongs to the reference machine (`reference-machine.md`). A CI runner is not that machine, and a GitHub-hosted Windows runner has Defender enabled where RM-1 does not. So each budget carries two numbers:

- `product_limit_ms` — the number in the PRD, asserted under `--profile reference`.
- `ci_limit_ms` — a regression ceiling, asserted under `--profile ci`. Its job is to catch a change that made things worse, not to certify the product.

`ci_limit_ms` is `None` for all four budgets, because there has been no CI run to set it from and a placeholder invented at a desk is worse than an honest red. Until it is set, the CI job asserts the product limit. **The ceilings are set from the first green CI run and recorded here with the run they came from** — not by lowering a bar to make a build pass.

### Every budget names the interval it times

Each budget carries an `interval` string saying exactly what is timed, printed in the report and recorded in the result JSON, and a test fails any budget that leaves it empty. This is not bureaucracy: P0 finding **F-03** is precisely what happens when nobody writes it down.

| Budget | Interval |
|---|---|
| NFR-PRF-01 window visible | process spawn to the first painted frame |
| NFR-PRF-01 vault usable | process spawn to the vault-usable mark, as task D2 defines it |
| NFR-PRF-03 | watcher event dequeued to the row being visible to a reader |
| NFR-PRF-04 | entry to `rebuild()` until it returns, writer drained and `ANALYZE` run |

**F-10 and NFR-PRF-04: the aggregate is asserted, the breakdown is recorded.** NFR-PRF-04 is a wall-clock requirement and the budget still asserts one number. But one number cannot answer architecture.md's **open design question 1** — *does the parse pool or the index writer dominate rebuild time?* — and §1.2 says the parse pool exists precisely because that assumption is unmeasured. Reported as a single figure, "the index writer dominates" and "reading 39.4 MiB off disk dominates" are indistinguishable, and they imply completely different fixes.

So the job records four phases beside the total — **walk, read+hash, parse, write** — as milliseconds and as a share of the wall clock, in the report and in the result JSON. The walk enumerates without reading contents, so the split is real rather than notional. Phases are the per-phase median across runs, not the phases of the median run: one slow run should not decide where the time went.

The phases are allowed to sum to *less* than the wall clock. That is concurrency between the parse pool and the writer, not an error, and the report says so rather than leaving a reader to wonder — a sum that exceeded the total would be the thing worth investigating.

When T2 reports, this breakdown is not decoration. It is the evidence that closes the oldest open question in the design document.

**F-03 and NFR-PRF-03.** "Save→index" has two defensible readings that differ by more than the budget itself:

- **End to end**, from the file being written. Includes the 150 ms per-path coalescing window of architecture §3.1 step 1, so it cannot fit inside 50 ms by construction — 150 > 50.
- **Pipeline**, from the watcher event being dequeued. The only reading the current design can satisfy.

The job measures **both** and asserts the pipeline interval, printing the end-to-end figure beside it as `F-03 … reported, not asserted`. That way the gate report states which number the 50 ms claim rests on instead of implying there was only ever one, and if the architect rules the other way the assertion moves one line. The seam asks I5 for a dequeue timestamp for exactly this reason.

This is a measurement decision, not a ruling: F-03 is the architect's to close and product-analyst's to re-baseline at Q1.

### p95 is nearest-rank

NFR-PRF-01 says "over 20 runs; p95 within budget", and at n = 20 the choice of percentile definition changes the answer. Nearest rank at q = 0.95, n = 20 is the 19th of 20 sorted samples: **one slow run out of twenty is inside the budget; two are not.** An interpolating percentile would let two through, which is not what the requirement means. A unit test asserts exactly that boundary.

## The gate number is the scanner-enabled number

A rebuild of 20,000 notes reads 20,000 files, so on Windows the on-access virus scanner moves the result more than most code changes will. RM-1 happens to have real-time protection disabled, which makes every number taken on it a best case no ordinary user will see.

So, per the lead's instruction for B2: **every filesystem-bound budget is measured both ways on RM-1, and the scanner-enabled figure is the one the charter's "< 30 s" gate is judged against.** A scanner-off number is a diagnostic, not a verdict. Real-time protection is never disabled to make a budget pass; if the honest number breaches, the gate is NO-GO and the architecture changes, which is what P0 is for.

That rule does not depend on anyone remembering it:

- the harness detects the scanner state and the active power plan itself, on every run;
- both are printed in the report and recorded in the result JSON;
- a scanner-disabled Windows run is marked `"gate_eligible": false` and prints a `NOT A GATE NUMBER` line, so a best case cannot be filed as the gate figure by omission;
- an *undetermined* scanner state is also not gate eligible — unknown is not the same as fine.

The harness never changes the scanner state. Toggling real-time protection is an administrator action with consequences outside this repository, and a benchmark tool that disables your antivirus to look good is not a thing this project should contain. Toggling is a manual step in the protocol in `reference-machine.md`.

Record the environment either side of a series:

```
cargo run --release -p budget --bin environment
```

It prints the scanner state, power plan and core count, and exits non-zero when the environment cannot produce a gate number — so a script can refuse to take the gate figure in the wrong conditions.

## What every result carries

A number without its conditions is not comparable with another number, so each result records:

- the vault's `tree_digest_sha256`, recomputed from disk, not copied from the manifest;
- the generator's `VAULT_FORMAT_VERSION` and the vault's note count;
- the machine profile, the limit that was applied, and the interval that was timed;
- the operating system, core count, real-time scanner state and power plan;
- whether the result is gate eligible;
- every raw sample, not just the statistic.

If the vault on disk does not match its manifest the job refuses to run and tells you to regenerate it. A number taken against a stale or hand-edited vault looks fine and means nothing, which is the worst kind of number to have.

## The seam

`bench/budget/src/subject.rs` is the only module in the harness that links a Serat crate. Three functions — `full_rebuild`, `save_to_index`, `cold_start` — and each carries, in a `const`, the exact entry point it needs. When task I4 lands, one function changes and no measurement code moves.

The entry points the harness is waiting on:

| Budget | Needs | From |
|---|---|---|
| NFR-PRF-04 | `rebuild(source: &dyn NoteSource, parser: &dyn NoteParser, index_dir: &Path) -> Result<RebuildReport, IndexError>`, callable from a plain binary with no Tauri app running, returning only after the writer has drained — plus the four F-10 phase durations on `RebuildReport` | task I4, `serat-index` |
| NFR-PRF-03 | `open(vault, index_dir)` plus a submit-one-file call the harness can observe completing — the budget is save → *queryable*, so it must be able to see the point a reader would see the change — **and** the moment the watcher event left the queue, so F-03's two intervals can be reported separately | task I5, `serat-index` / `serat-vault` |
| NFR-PRF-01 | `SERAT_STARTUP_TRACE=<file>` writing `{"t0_to_window_visible_ms":…, "t0_to_vault_usable_ms":…}` and exiting; t0 is process spawn measured by the harness, not an `Instant::now()` inside `main` | task D2 (definition), task S1 (`serat-app`) |

**NFR-PRF-01 is blocked on D2, not on effort.** The architect has been asked to define t0, "window visible" (first painted frame, not window creation — measuring window creation would pass 400 ms and ship a blank rectangle) and "vault usable", plus which integrity check runs on the warm start path, since whatever it is sits inside the 1 s budget. The harness measures what D2 defines and does not guess.

## Why the harness may link the product

Architecture §2.1's dependency rule governs the *product* graph. A harness that may not link the thing it measures cannot measure anything. `crates/serat-core/tests/dependency_rule.rs` encodes the rule that actually applies, in the `BUDGET_MAY_LINK` allow-list: bench crates may depend on product crates; **nothing under `crates/` may depend on anything under `bench/`.** The direction of the arrow is the whole point, and it is asserted rather than remembered.

The rebuild signature is shaped by the same rule. A rebuild walks, parses and writes, and no product crate may depend on two of those — so the dependencies arrive as `serat-core` traits (`NoteSource`, `NoteParser`) that the harness constructs and hands in, which is the mechanism §2.1 prescribes. `NoteSource` is new in I4 and its home is architect's to ratify; the *shape* cannot change without a new crate and an ADR.

## Traceability

| Test ID | Verifies | Location | Status |
|---|---|---|---|
| TC-020 | NFR-PRF-04, NFR-PRF-08 | `bench/budget/src/bin/prf04-full-rebuild.rs` | harness landed, red pending I4 |
| TC-021 | NFR-PRF-03, NFR-PRF-08 | `bench/budget/src/bin/prf03-save-to-index.rs` | harness landed, red pending I5 |
| TC-022 | NFR-PRF-01, NFR-PRF-08 | `bench/budget/src/bin/prf01-cold-start.rs` | harness landed, red pending D2 + S1 |
| TC-023 | NFR-PRF-08, US-35 | `.github/workflows/budgets.yml` | landed |
| TC-026 | NFR-PRF-08 — the budget table matches the PRD | `bench/budget/src/lib.rs::every_p0_budget_matches_the_number_in_the_prd` | pass |
| TC-027 | NFR-PRF-01/03 — p95 is nearest-rank at n = 20 | `…::nearest_rank_p95_over_twenty_runs_allows_exactly_one_slow_run` | pass |
| TC-028 | NFR-PRF-08 — a breach fails, the boundary passes | `…::a_breach_is_a_failure_and_the_boundary_passes` | pass |
| TC-029 | NFR-PRF-08 — a missing subject fails loudly and names the task | `bench/budget/src/subject.rs::the_seam_fails_loudly_and_says_what_is_missing` | pass |
| TC-040 | NFR-PRF-04 — a scanner-disabled Windows run is not gate eligible | `bench/budget/src/env.rs::a_scanner_disabled_windows_run_is_not_gate_eligible` | pass |
| TC-041 | NFR-PRF-01/03/04 — the measurement environment is detected and recorded | `bench/budget/src/env.rs::detection_reports_something_about_this_machine` | pass |
| TC-042 | F-03 — every budget declares the interval it times | `bench/budget/src/lib.rs::every_p0_budget_matches_the_number_in_the_prd` | pass |
| TC-043 | F-10 — the rebuild seam asks for the four phases and they sum coherently | `bench/budget/src/subject.rs::the_seam_fails_loudly_and_says_what_is_missing` | pass |
