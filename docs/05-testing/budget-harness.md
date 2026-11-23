---
project: Serat
artifact: Budget harness and CI budget jobs
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-08, NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, US-35, C-06
---

# The Budget Harness

NFR-PRF-08: *"Every performance budget shall be asserted in continuous integration; a regression shall fail the build."* This is how. One job per budget, each measured against the benchmark vault, each recording the conditions the number was taken under.

**A budget that cannot be measured fails; it never skips.** A budget job that skips when its subject is missing is worse than no job: the build goes green and the budget silently stops being asserted.

**Prerequisite for the three app-driving jobs (`prf01-serat-term`, `prf01-cold-start`, `prf03-save-to-index`, plus `prf01-cold-start-terms` locally):** the release application, built first with `cargo build --release -p serat-app` (per package, never `--workspace`: no `test-hooks`, ADR-0011). `cargo run -p budget` builds the harness only; it does not build the app. The bins look for `target/release/serat` (`serat.exe` on Windows) beside the workspace, or `$SERAT_BIN`. If it is absent the bin exits with code **3** (`EXIT_APP_ABSENT`) and one line, `budget: serat-app binary absent: looked for <path>; build it first ...`, and prints no verdict line. Exit 1 remains "breached, undecided, or a measurement failed after it started". In CI the `build-app` job builds it once and the three jobs download it into `target/release/` (task #164); on a reference machine build it before the run. Under a custom `CARGO_TARGET_DIR` set `SERAT_BIN`. On a headless Linux box the app needs a display (`xvfb-run`); Windows runners do not.

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

`ci_limit_ms` is `None` for every budget in the table above, because there has been no CI run to set it from and a placeholder invented at a desk is worse than an honest red. Until it is set, the CI job asserts the product limit. **The ceilings are set from the first green CI run and recorded here with the run they came from** — not by lowering a bar to make a build pass.

### Every budget names the interval it times

Each budget carries an `interval` string saying exactly what is timed, printed in the report and recorded in the result JSON, and a test fails any budget that leaves it empty. This is not bureaucracy: P0 finding **F-03** is precisely what happens when nobody writes it down.

| Budget | Interval |
|---|---|
| NFR-PRF-01 window visible | process spawn to the first **composited frame** of the shell (architecture §3.4 row 1) |
| NFR-PRF-01 vault usable | process spawn to the latest of: first paint, the tree's first screen (root listing applied and painted, ADR-0015 §3, from P1 task #141; P0 used the last-open buffer), index settled, watcher settled (§3.4 row 2) |
| NFR-PRF-03 | `apply()` submitted through `flush()` returning — Serat's own save path, no watcher, no debounce |
| NFR-PRF-04 | entry to `rebuild()` until it returns, writer drained and `ANALYZE` run Since task #154 `rebuild()` first takes the index's cross-process writer lock (`.serat/index.lock`: one `open` and one `flock`, microseconds, once per rebuild), and every index open does the same (the open-time and catch-up benches). Inside the interval, below any measurable share; the lock is why a bench run beside a running app on the same vault is now refused with `HeldByAnotherProcess` rather than rebuilt under it. |

**F-10 and NFR-PRF-04: the aggregate is asserted, the breakdown is recorded.** NFR-PRF-04 is a wall-clock requirement and the budget still asserts one number. But one number cannot answer architecture.md's **open design question 1** — *does the parse pool or the index writer dominate rebuild time?* — and §1.2 says the parse pool exists precisely because that assumption is unmeasured. Reported as a single figure, "the index writer dominates" and "reading 39.4 MiB off disk dominates" are indistinguishable, and they imply completely different fixes.

**"Producer waited" is not the writer's cost, and the label says so.** The phase is measured around `submit_blocking` plus the final drain, so it is how long the *producer was blocked*. The writer's own SQL runs concurrently on its own thread and is charged to nobody. The reading is therefore the opposite of the obvious one:

- near zero means the writer **kept up** — not that writing is cheap;
- large means the writer **is** the bottleneck, because the only way it grows is the queue being full.

A column called "write" holding a small number would settle open design question 1 backwards — someone would conclude parsing dominates when the writer does. The field is named `producer_wait`, the report prints `producer waited`, and a line under the breakdown carries the reading with the number. A test asserts both the label and the explanation are present, because the sentence is the load-bearing part.

The five phases will not sum to the wall clock, and the gap is the producer and the writer overlapping — the actor working, not the harness broken. The report says that too.

**F-47: `ANALYZE` must describe the finished index, and only this vault can show it.** `commit()` re-runs `ANALYZE` when a batch touches more than 5% of the notes. At test scale 5% is smaller than one batch, so the opportunistic path fires on the last batch every time and the rebuild's explicit `analyze()` is unobservable — which is why deleting it changed nothing in `serat-index`'s own tests. At 20,000 notes the threshold is 1,000 against a batch of 512, so the tail of `20000 % 512` = **32 notes** never crosses it. Remove or reorder the explicit call and the planner's statistics describe the index as it was 32 notes ago; architecture §4.3 records what follows — `ix_notes_kind` over `ix_notes_type`, and every typed query a full scan. That is the defect that otherwise surfaces as "queries are slow on my real vault but fine in the tests" three months into P2.

**This check has a precondition, and it refuses rather than passing when it is unmet.** Below roughly 10,000 notes the opportunistic 5% re-ANALYZE fires on the last batch every time, the explicit `analyze()` becomes unobservable, and a green result would mean nothing was tested rather than that nothing is wrong (F-128). So `check_analyze_freshness` returns an error on a vault that small: a guard that quietly stops guarding is worse than one that is absent, and this is the only place in the project where the defect is visible at all.

So after each rebuild the harness reads `sqlite_stat1` **out of the index itself** and asserts the row count it records for `notes` equals the vault's note count. Reading the database rather than trusting a number the rebuild reports is the same argument as recomputing the vault digest: an oracle the code under test does not supply.

**Links resolved are checked against the manifest too, at `edges + asset_embeds`.** An asset is a row in `notes` with `kind='asset'` (ADR-0005), so a rebuild resolves `![[report.pdf]]` like any other link — while the manifest's `link_degree.edges` counts only note-to-note edges. Rather than either side bending a number to match the other, the generator now records `content.asset_embeds` separately and the harness adds them: **the manifest is the oracle and the rebuild's count is the thing under test, so the oracle is the one that has to be derivable** (F-129). Adding the field changed no vault byte — the canonical digests are unchanged, because the manifest lives outside the vault.

A rebuild reporting fewer has dropped links, and one reporting *more* has counted rows touched rather than links resolved. That second case is not hypothetical: rust-core-dev hit it while writing the second pass, and a dangling link came back reported as resolved. It is the rebuild-backpressure shape one layer up — a wrong number that looks right.

**F-38: `write-FTS` must be non-zero.** `write_rows` and `write_fts` are only comparable across runs if both were measured on the bulk-load shape — triggers dropped, `blocks_fts` built once at the end. A rebuild that regressed to loading *through* the triggers would report `write_fts` at roughly zero with `write_rows` absorbing the cost: **the breakdown would look better while the total got worse**, and no wall-clock figure can see that. Given F-32 — about **6x**, 26,000 blocks at 2.1 s with the triggers live against 0.32 s dropped, independently reproduced at 5.8x with an identical resulting search index — it is the rebuild regression most worth catching, and it is the phase breakdown justifying its own existence.

So the job records five phases beside the total — **walk, read+hash, parse, producer waited, FTS build** — as milliseconds and as a share of the wall clock, in the report and in the result JSON. `write` is split on the architect's evidence: on the dropped-trigger load, 3.7 s of bulk row insert against 1.9 s of `blocks_fts` build at 259,971 blocks; with the triggers left live, the same shape cost 2.1 s at 26,000 blocks against 0.32 s dropped. Reported as one figure those are indistinguishable and the fixes differ, which is the same argument as F-10 itself, one level down. The walk enumerates without reading contents, so the split is real rather than notional. Phases are the per-phase median across runs, not the phases of the median run: one slow run should not decide where the time went.

The phases are allowed to sum to *less* than the wall clock. That is concurrency between the parse pool and the writer, not an error, and the report says so rather than leaving a reader to wonder — a sum that exceeded the total would be the thing worth investigating.

When T2 reports, this breakdown is not decoration. It is the evidence that closes the oldest open question in the design document.

**F-227, F-268 and NFR-PRF-03: the asserted series must be Serat's own save path, not a save observed through the watcher.** §3.1.1's save path has no watcher and no dequeue at all — the buffer is written, the in-memory bytes go straight to the parse pool, submit, visible — so neither of the two watcher-path readings below is an interval NFR-PRF-03 can mean:

- **End to end**, from the file being written *and re-detected by the watcher*. Includes the 150 ms per-path coalescing window of architecture §3.1 step 1, so it cannot fit inside 50 ms by construction — 150 > 50. FR-VLT-04's 500 ms, not this budget's.
- **Pipeline**, from the watcher event being dequeued. Still a watcher-path interval despite the name — this is F-268, one layer inward from F-227: the harness asserted this series for exactly the reason it must not, because the name reads like the pipeline's own cost when it is a watcher-triggered one.

The job measures **all three** — `save`, the direct path through `IndexHandle::apply()` then `writer().flush()`, plus the two above — and asserts only `save`, printing the watcher-path figures beside it as `reported, not asserted`. `check_save_path` refuses to assert anything whose `SaveTiming.path`/`SaveSamples.path` does not read `"save"` (F-229): a save timed on the watcher path looks exactly like a real number, and this is what keeps that from becoming NFR-PRF-03's gate figure by accident, on top of the fact that neither watcher-path interval is bounded the way `save` is.

F-03 itself — which of the two watcher-path readings FR-VLT-04 means — remains the architect's to close and product-analyst's to re-baseline at Q1; it no longer bears on this budget's asserted series.

### Cold start: four ways a trace is refused

D2 has landed, so NFR-PRF-01 is measurable. The app writes one line of JSON to `SERAT_STARTUP_TRACE` and exits; the harness stamps t0 immediately before `spawn()` and passes it in as `SERAT_STARTUP_T0_UNIX_NANOS`, so everything before `main` — loader, DLLs, webview init — is inside the number rather than outside it.

`budget::trace` refuses a trace rather than turning it into a number when:

1. **`t0_source` is not `harness`.** The app falls back to an `Instant` inside `main` when the variable is absent, and says so. That figure undercounts pre-main; it is a different measurement, not a slightly worse one.
2. **`profile` is not `release`.** A budget measured in a debug build is not a measurement. The mark is emitted in both builds so it is exercised locally; only the recording is refused.
3. **The marks are out of order.** `main` ≤ `window_created` ≤ `window_visible` ≤ `vault_usable`. Out of order means the wall clock moved under the run, and a sample that survived a clock step is not a sample.
4. **`window_visible - window_created < 5 ms`.** This is the blank-rectangle check. §3.4 measures the *painted shell*, not window creation, and the two differ by the cost of loading and compositing the webview bundle — far more than 5 ms. A paint mark rewired to fire at window creation collapses that gap to about zero, passes 400 ms, and ships a blank rectangle. The threshold is a mutation detector, not a performance floor; tauri-app-dev asked the harness to own it, and if it ever fires on an honest run that is a more interesting result than the job passing.

**No note is opened, and none is required.** Task #52 ruled that "vault usable" is architecture.md §3.4's four preconditions — index state known, file tree delivered, watcher settled, window painted — and none of them is "a note is rendered": `--open` is session restore for a user, not a readiness precondition, so the harness stopped passing it (task #54). The trace still carries `last_open` for a real user session that did restore one, but the harness's own cold-start runs report it empty, and an empty `last_open` is not treated as a failure.

The app has no Rust-side path that stamps the paint mark — it comes only from a double `requestAnimationFrame` after the shell's first render — so a shell that paints nothing never marks, the run times out, and the job goes red. There is no fast path for a blank rectangle.

**Two honesty notes for the gate report**, from tauri-app-dev: until S2 lands the webview is a placeholder page rather than the Vue app, so the P0 figure is the shell's cold start with a trivial front end and S2 will move it; and `--exit-after-ready` exits after the usable mark, so every iteration is genuinely cold.

### Every cold-start figure is a warm-vault figure

Stated in the report line itself, not only here. Each iteration spawns against the same vault, so after the first the page cache holds it and the walk is CPU-bound; a user's cold start reads a cold vault. The gap is unmeasured because dropping the Windows page cache needs privileges the measuring account lacks, and a freshly generated vault is write-cached rather than cold — measuring that way gave +42.9 ms against a **null control of +15.9 ms with the same binary on both arms**, so most of the apparent effect was the harness measuring itself. `reference-machine.md` records what would fix it. Until then this is a stated bound on what P0 established.

### Scale independence: §3.4's load-bearing claim, as a test

Architecture §3.4 states that **nothing on the critical path is proportional to vault size** — the main thread never opens the index, and the file tree renders from one query over `notes(path)` rather than from the walk. That is a claim, so it gets a test rather than a paragraph.

`prf01-cold-start` runs the same measurement against the 20k and the 50k vault and asserts that the compared mark's **p95** did not grow by more than **20%**. If the critical path were proportional to vault size, 2.5x the notes would show up as roughly 2.5x the time; 20% sits far below that, so the check fails on the thing it is for. `--skip-scale` omits it for a quick local run.

**Task #118 moved the subject to the Serat term, and architecture §3.4's ruling gives the five grounds.** The sharp one is sensitivity: at task #100's attested p95s the aggregate mark is 1575.6 ms against the Serat term's 233.8 ms, so at the 20% tolerance the aggregate cannot see a vault-proportional step under about **315 ms** where the Serat term catches one at about **47 ms**. The attribution argument survives even at zero noise — only a change confined to this repository can introduce a vault-proportional step, which is ADR-0014 §Decision 2's own test for what is ours, and asserting on the aggregate is asserting mostly on a term no change of ours can move. **Both aggregates' 20k-to-50k ratios are still measured and printed beside the verdict and assert nothing**, because contention is one channel of vault-size dependence that lands in `P_webview` and so outside `S`.

**Two things the moved check must say, and both are in the verdict line rather than in a later investigation.** The 20% tolerance was sized against the *effect* and never against the noise of the series it is asserted on, and the series has just changed — so each arm's own spread is reported beside the ratio, and a spread that is not comfortably inside the tolerance is a finding to escalate rather than a tolerance to widen (ADR-0014 §Decision 5's floor rule). And `S_settle` is inside the compared quantity, so **both arms must enter the batch in the same index state**: a `.serat/` present on one arm and absent on the other measures which fixture was warm rather than whether the critical path grew. `ColdStartVaultGuard` equalises *cleanup* and says nothing about the starting state, so that is a separate check, it refuses rather than caveats, and the verdict records which state was compared.

**The arms are interleaved, not blocked, and that is what makes the claim decidable.** Measured as two blocks of twenty, the comparison could not resolve anything: the within-arm spread on the reference machine is about five times the effect being tested for, so each block carries whatever the machine was doing during it — which is how a 20,000-note vault came out *faster* than a 2-note one, and why the claim was briefly recorded as undecidable (F-109). It is not undecidable; the design was wrong. Alternating the arms spawn by spawn, and alternating which arm leads, puts slow drift on both and cancels it in the paired difference. Same number of spawns, strictly more information: at 30 pairs the paired 95% CI came out **[-123, +45] ms against a 179 ms tolerance**, which excludes an effect the size of the tolerance (F-123).

Three consequences, each of which the harness states rather than assumes:

- **The comparison is judged on p95 and reported with rank statistics.** The judgement compares the two p95s, because that is the statistic NFR-PRF-01 itself asserts. Beside it the report carries the paired **median** difference and a **sign test** — rank statistics, because these distributions are heavy-tailed and a mean under-detects: in the run behind F-141 the mean's confidence interval included zero while the sign test gave p ≈ 0.04 and the p95 ratio was 1.20. A mean would have called a real effect noise.
- **The tolerance stays at 20%.** Widening it would trade away the detection it was sized for. The instrument was the problem, not the threshold — and architect, who set it, said so themselves after measuring the noise they had not measured when they set it.

  It is worth knowing how little headroom that leaves for one specific regression. Removing the walk deferral costs a **p95 ratio of 1.20** — measured paired, after the same blocked-sampling error inflated the original estimate by about ten times. So this assertion would sit exactly on its threshold for that defect rather than clearly failing, which is a further argument for judging on the p95 the budget cares about rather than on a mean that would miss it entirely.
- **The claim is stated as a bound, not as an absence.** The report says "vault size does not inflate the mark beyond the tolerance … This bounds the effect; it does not claim there is none." The CI excludes an effect as large as the tolerance; it does not exclude a small real one, and the budget is about the former.

Blocked runs remain fine for the absolute budget p95s — a budget is a threshold and needs no pairing. Only the comparison between vault sizes does.

### F-99: which end of the save path, and nobody has the number

F-03 settled which *path* NFR-PRF-03 governs — Serat's own save, not the watcher path with its 150 ms coalescing window. **F-99 asks which *end* of that path, and the two differ by more than the budget:**

| End | What it is |
|---|---|
| submit → `flush()` returns | the index has committed; a reader opening a transaction now sees the change |
| submit → `index.changed` arrives | what the **UI** learns from, throttled to ten per second — so up to **100 ms** later, twice the entire 50 ms budget |

Only the second is what a person experiences. The harness measures both, asserts the drained one, and reports the notified one beside it — the same treatment F-03's two intervals get, for the same reason. Which end the requirement *means* is product-analyst's at Q1; making the ambiguity visible is the harness's job, and a passing 50 ms figure against the wrong end would describe nothing a user could notice.

**The seam asks for `submit()` then `flush()`, not a combined `apply_and_wait()`.** Architect's ruling, and the reason is better than the one I would have given: a barrier must stay visible where it is called. `submit(); flush();` shows the wait; `apply_and_wait()` is a verb that reads like a write and hides one, and it is a single copy-paste from the save path where the wait is forbidden by design goal 2. It is also a better seam — the two calls are separately timeable, so if a number ever looks wrong you can see whether the cost is in the submit or in the drain.

One submit per flush. `flush()` drains the whole queue, so submitting fifty and flushing once would be a throughput figure wearing a latency label, and NFR-PRF-03 is a p95 of individual saves.

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

## The five phases do not share a unit, and the labels are load-bearing

This is the part of the rebuild report most likely to be misread, so the report says it out loud:

| Phase | Unit |
|---|---|
| `walk` | **wall clock**, single-threaded |
| `read_hash` | **CPU time, summed across the parse pool** — can exceed the wall clock, and on eight cores should |
| `parse` | **CPU time, summed across the parse pool** — same |
| `producer waited` | **how long the producer was blocked**, plus the final drain. Not the writer's cost |
| `fts build` | **wall clock**, writer-side |

A reader adding them expecting a total is wrong twice over: once by a figure that is too small, and twice by two that are too large. None of them is a share of anything.

**Why the harness labels rather than normalises.** `parse` as CPU time read against `producer waited` is precisely what settles architecture.md's open design question 1:

- **parse-dominated** — `parse` several times the wall clock (the pool busy on every core), `producer waited` near zero (the writer never made anyone wait). The fix is pool sizing.
- **writer-dominated** — `parse` at or below the wall clock (cores idle, waiting on the queue), `producer waited` large. The fix is batching.

Those are opposite conclusions from the same two columns. Dividing the phases by the wall clock so they added up would destroy the one number P0 was built to produce. The parse pool is sized at cores minus one (§2.2), so `serat_index::parse_pool_threads()` belongs in the gate report beside the figures — a rebuild number without the pool width is not comparable between machines.

`SaveTiming`'s three phases (NFR-PRF-03) *do* sum, because one save is single-threaded. That inconsistency between the two structures is deliberate and worth knowing rather than discovering.

## Which of these guards can be removed without a test going red

The harness exists to stop a number being believed when it should not be, so it is the component where a test that cannot fail does the most damage. Reading a test does not tell you whether it can fail — the assertion looks right either way. The only way to find out is to change the code and watch.

`bench/budget` is therefore swept by mutation: apply one change to a scratch copy, run the suite, record whether anything went red. **The current sweep is 27 mutations, all caught.** It is not a fixture in the repository, because a mutation harness that runs in CI becomes a thing to satisfy rather than a thing to learn from; it is a technique, and the record of what it found belongs in the gate report.

**A mutation only tells you something if you did not write the test with that mutation in mind.** Without that rule the technique produces a number that certifies itself. It is why the first round below is reported and then discarded.

The first two rounds are the useful part, because they failed:

| Round | Result |
|---|---|
| Round 1 — 16 mutations I wrote tests for | 16 caught. Self-confirming: I authored both sides. |
| Round 2 — 10 more, aimed at code I had **not** written a test for | **5 survived.** |

The five survivors, and what each says:

| Mutation | Why it survived | Closed by |
|---|---|---|
| `check_note_count(..)?` dropped from the rebuild loop | The guard was covered; the function that *calls* it was not. F-19's defence could be deleted silently. | `measure_full_rebuild_with`, so the loop is testable through an injected rebuild |
| `Statistic::P95` 0.95 → 0.90 | At n = 20 nearest rank moves from the 19th sample to the 18th, and nothing ever looked at the 18th. | A 20-sample case where the two differ |
| `ensure_vault` trusts the manifest instead of recomputing the digest | `check_vault`'s three refusals were covered; the wiring that feeds them was not. | `verify_vault_at`, exercised against a real tampered vault in `tests/vault_gate.rs` |
| `write_result` writes nothing | Nothing read the file back. | `write_result_to`, and a test that reads it back |
| `Outcome::gate_eligible` always true | Tested on `Environment`, never through the `Outcome` that carries it into a report. | A scanner-off outcome asserting `NOT A GATE NUMBER` reaches the report |

The pattern in four of the five is the same and worth naming: **the guard was tested and its caller was not.** A unit test on a refusal proves the refusal works, not that anything asks it. Three of the five were only closable by testing at the level the guard actually runs — which is why `tests/vault_gate.rs` generates a real vault, tampers with one byte, and checks the gate refuses it.

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
| NFR-PRF-04 | `rebuild(source: &dyn NoteSource, parser: &dyn NoteParser, index_dir: &Path) -> Result<RebuildReport, IndexError>`, callable from a plain binary with no Tauri app running, returning only after the writer has drained — plus the F-10 phase durations, `notes` (Markdown notes only, excluding assets) and `links_resolved` (links whose `dst_note` resolved) on `RebuildReport` | task I4, `serat-index` |
| NFR-PRF-03 | `IndexHandle::open(vault_root, index_dir)`, then `apply(&dyn NoteSource, &dyn NoteParser, &dyn EchoGuard, &VaultPath) -> SaveTiming` submitted directly (no watcher) followed by `writer().flush()` — `SaveTiming.path` must read `"save"`, refused otherwise (F-229, `check_save_path`) — **plus** a way to observe the watcher-path readings (last dequeue, row visible) so F-227/F-268's two intervals stay reported, not asserted | task I5, `serat-index` |
| NFR-PRF-01 | `SERAT_STARTUP_TRACE=<file>` writing `{"t0_to_window_visible_ms":…, "t0_to_vault_usable_ms":…}` and exiting; t0 is process spawn measured by the harness, not an `Instant::now()` inside `main` | task D2 (definition), task S1 (`serat-app`) |

**NFR-PRF-01 is no longer blocked on definition, only on S1.** D2 answered all three marks, and the warm-start integrity check is the stamp — `user_version`, `index_meta`, `count(*) notes` — measured at **5-8 ms** (task #68; it read 2.4 ms before I6 added the vault_root step) against 1150 ms for `quick_check` and 1310 ms for `integrity_check`, so it fits inside the budget with three orders of magnitude to spare. `quick_check` runs in the background only when a `-wal` file at open says the previous session ended badly.

Explicitly **off** the critical path, so a job that waits for them is measuring NFR-PRF-04 with extra steps: the catch-up walk, `ANALYZE`, the FTS integrity check, `quick_check`, extraction and embedding. The cold-start job stops at the usable mark and does not wait for indexing to finish.

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
| TC-026 | NFR-PRF-08 — the budget constants are pinned against drift **within this crate**; nothing here reads the PRD (F-447) | `bench/budget/src/lib.rs::every_p0_budget_constant_is_pinned_against_drift_within_this_file` | pass |
| TC-027 | NFR-PRF-01/03 — p95 is nearest-rank at n = 20 | `…::nearest_rank_p95_over_twenty_runs_allows_exactly_one_slow_run` | pass |
| TC-028 | NFR-PRF-08 — a breach fails, the boundary passes | `…::a_breach_is_a_failure_and_the_boundary_passes` | pass |
| TC-029 | NFR-PRF-08 — a missing subject fails loudly and names the task | `bench/budget/src/subject.rs::the_seam_fails_loudly_and_says_what_is_missing` | pass |
| TC-040 | NFR-PRF-04 — a scanner-disabled Windows run is not gate eligible | `bench/budget/src/env.rs::a_scanner_disabled_windows_run_is_not_gate_eligible` | pass |
| TC-041 | NFR-PRF-01/03/04 — the measurement environment is detected and recorded | `bench/budget/src/env.rs::detection_reports_something_about_this_machine` | pass |
| TC-042 | F-03 — every budget declares the interval it times | `bench/budget/src/lib.rs::every_p0_budget_constant_is_pinned_against_drift_within_this_file` | pass |
| TC-043 | F-10 — the rebuild seam asks for the phases and they sum coherently | `bench/budget/src/subject.rs::the_seam_fails_loudly_and_says_what_is_missing` | pass |
| TC-044 | NFR-PRF-01 — a trace that undercounts pre-`main` time is refused | `bench/budget/src/trace.rs::a_trace_that_undercounts_pre_main_is_refused` | pass |
| TC-045 | NFR-PRF-01 — a debug build cannot produce a budget number | `…::a_debug_build_cannot_produce_a_budget_number` | pass |
| TC-046 | NFR-PRF-01 — a paint mark wired to window creation is caught | `…::a_paint_mark_wired_to_window_creation_is_caught` | pass |
| TC-047 | NFR-PRF-01 — marks out of order mean the clock moved, and the sample is refused | `…::marks_out_of_order_mean_the_clock_moved` | pass |
| TC-048 | NFR-PRF-01, arch §3.4 — a critical path that grows with the vault is caught | `bench/budget/src/lib.rs::a_critical_path_that_grows_with_the_vault_is_caught` | pass |
| TC-070 | F-25 — the median of three is the middle run, not the fastest | `bench/budget/src/lib.rs::the_median_of_three_is_the_middle_run_and_not_the_fastest` | pass |
| TC-071 | F-25 — `WallClock` is the median, and the report says so | `…::the_wall_clock_statistic_is_the_median`, `…::the_report_says_the_number_is_a_median_of_three` | pass |
| TC-072 | F-25 — the crate's two definitions of median agree | `…::the_two_medians_in_the_crate_agree` | pass |
| TC-073 | F-25 — `P95` is the 95th percentile and not the 90th | `…::the_p95_statistic_is_the_ninety_fifth_percentile` | pass |
| TC-074 | F-26 — a coherent vault from another seed is refused | `…::a_coherent_vault_generated_from_the_wrong_seed_is_refused`, `tests/vault_gate.rs` | pass |
| TC-075 | F-26 — a hand-edited vault, a wrong note count, a missing manifest | `tests/vault_gate.rs` | pass |
| TC-076 | F-27 — a job cannot swallow the seam error and post a number | `…::a_job_cannot_swallow_the_seam_error_and_post_a_number` | pass |
| TC-077 | F-27 — no samples is a failure, not an empty pass | `…::no_samples_is_a_failure_and_not_an_empty_pass` | pass |
| TC-078 | F-19, F-27 — the rebuild loop refuses a partial index and takes per-phase medians | `…::the_rebuild_loop_refuses_a_partial_index_and_takes_per_phase_medians` | pass |
| TC-079 | F-27 — the phase breakdown is printed and recorded | `…::the_phase_breakdown_is_printed_and_recorded` | pass |
| TC-080 | F-29 — a phase breakdown that double-counts is refused | `…::a_phase_breakdown_that_double_counts_is_refused` | pass |
| TC-081 | F-30 — every budget has a job, iterating the budgets rather than a grep | `…::every_budget_has_a_job_in_the_ci_workflow` | pass |
| TC-082 | F-38 — a rebuild that loaded through the FTS triggers is caught | `…::a_rebuild_that_loaded_through_the_fts_triggers_is_caught` | pass |
| TC-083 | F-27 — a result file is written and holds the numbers | `…::a_result_file_is_actually_written_and_holds_the_numbers` | pass |
| TC-084 | RISK-012 — an outcome taken in the wrong environment is not a gate number | `…::an_outcome_taken_in_the_wrong_environment_is_not_a_gate_number` | pass |
| TC-090 | F-47 — stale `ANALYZE` statistics are caught | `…::stale_analyze_statistics_are_caught` | pass |
| TC-091 | F-49 — dropped or over-counted resolved links are caught | `…::dropped_or_over_counted_links_are_caught` | pass |
| TC-092 | F-46 — the producer-wait phase is labelled for what it measures | `…::the_producer_wait_phase_is_labelled_for_what_it_measures` | pass |
| TC-093 | F-38 — the FTS check still covers a run where the writer kept up | `…::the_fts_check_still_covers_a_run_where_the_writer_kept_up` | pass |
| TC-109 | F-81, F-109, F-123 — the scale check interleaves the arms, cancels drift, and rejects a path that grew | `…::the_scale_check_interleaves_the_arms_and_rejects_a_path_that_grew` | pass |
| TC-110 | F-128 — the ANALYZE check refuses a vault too small to show the defect | `…::the_analyze_check_refuses_a_vault_too_small_to_show_the_defect` | pass |
| TC-118 | #118, F-443, F-407 — the gated list and the reported list partition every budget by its own `gated` flag | `…::the_gated_list_and_the_reported_list_partition_every_budget_by_its_own_flag` | pass |
| TC-119 | #118 — a reported figure prints `REPORTED`, never a verdict, and cannot fail a build | `…::a_reported_figure_prints_no_verdict_and_cannot_fail_a_build` | pass |
| TC-120 | #118, §3.4's ruling — the scale check asserts the Serat term and only reports the aggregates | `…::the_scale_check_asserts_the_serat_term_and_only_reports_the_aggregates` | pass |
| TC-121 | #118, §3.4's ruling — the moved check reports its own spread and the index state both arms started from | `…::the_scale_check_reports_its_own_spread_and_the_index_state_both_arms_started_from` | pass |
| TC-122 | #118, §3.4's precondition — arms starting in different index states are refused, not caveated | `…::a_scale_check_whose_arms_start_in_different_index_states_is_refused` | pass |
| TC-123 | #118 — the demoted aggregates are still measured by a job CI runs | `…::the_demoted_aggregates_are_still_measured_by_a_job_that_ci_runs` | pass |
| TC-111 | S1, task #52 — an empty `last_open` is the normal case (no note is a precondition) and parses fine | `bench/budget/src/trace.rs::an_empty_last_open_is_the_normal_case_now_and_parses_fine` | pass |

## `quick_check` on the 20k index (task #69; NFR-REL-01, NFR-REL-02)

`serat_index::quick_check(index_dir)` runs `PRAGMA quick_check` over its own read-only
connection, off the open path (architecture §3.4 recorded 1,150 ms for the pragma on this
fixture). Measured by `subject::tests::measure_quick_check_on_the_20k_index_with_the_checkpointer_running`
(`cargo test -p budget --lib measure_quick_check -- --ignored --nocapture`): the 20k index
built by `open_or_rebuild`, then a live `IndexHandle` open (writer and WAL checkpointer
running), five checks in a row, warm OS cache, host load about 1 on 4 cores, 2026-11-22:
**922, 955, 957, 967, 1,006 ms (median 957 ms)**. Indicative, not an attested gate run, and
nothing gates on it: the check is background and on demand, so it never sits on the
NFR-PRF-01 window. It takes no writer lock (task #154) and holds no read transaction
beyond the one call.

