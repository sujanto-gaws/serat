# ADR-0019: A budget is asserted where a gate number can be produced
**Status:** accepted · **Date:** 2026-11-23 · **Serves:** C-06, NFR-PRF-08, NFR-PRF-04, FR-VLT-04, NFR-PRF-01, NFR-PRF-03
**Honours:** ADR-0013 (when a merge may proceed with a budget red), ADR-0014 (cold start as two terms), ADR-0011 (the watcher path FR-VLT-04 measures), ADR-0008 and ADR-0020 (the save path the second amendment's phases split); RISK-012, RISK-013, RISK-017
**Source:** ruling requested by test-engineer in task #166, on the evidence in `docs/05-testing/nfr-prf-04-gate-quality.md` ("Task #166") and the FR-VLT-04 triage of task #165.

## Context

C-06 says the performance budgets are binding and "enforced in CI against the synthetic vault". NFR-PRF-08 says every budget is asserted in CI and a regression fails the build. Both were written on the premise that a CI runner produces a number that says something about the budget. For two budgets, hosted CI does not.

**The runner can never produce a gate number.** `Environment::gate_eligible()` (`bench/budget/src/env.rs`) is true on Windows only when the real-time scanner is on. All 36 hosted `windows-latest` runs on record (279 to 316) report 2 cores and the scanner OFF. So the harness's own rule says no figure from that runner is a gate number. That is true by construction, and it stays true for as long as the runner image ships that way. `reference-machine.md` said the opposite until task #166 read the logs.

**The runner's own term dominates the figure.** Hosted file I/O costs 15 to 25 times more per note than Linux: a batch of 100 notes takes 2.9 to 4.8 s there against 0.2 s on Linux. NFR-PRF-04 failed in all 20 runs before the #56 merge (`1ec7a9e`, WAL checkpointer off the save path; medians 51.3 to 131.8 s). It passed or was indeterminate in all 16 runs after it (15.9 to 30.2 s). Within a single run after the step, one binary's samples spanned 20.6 to 60.1 s (192 %). FR-VLT-04 passed 3 of 4 runs at 376 to 615 ms against 500 ms. In those runs the watcher delay held at a p95 of 172.9 to 173.6 ms, and the processing tail (p95 205 to 450 ms) moved between runs three minutes apart. In both budgets the runner's own spread is at least the budget's margin, so a hosted verdict is a coin toss.

**No ceiling is derivable yet.** The harness's two-threshold design (`budget-harness.md`) has a regression ceiling, `ci_limit_ms`. Under the straddle rule that ceiling would have to sit above 60.1 s for NFR-PRF-04, which is twice the product limit and above the smallest sample taken before #56. It would catch nothing.

Task #166 shipped a mechanism: `CiAssertion { Always, WhenGateEligible }` on `Budget`, with `Outcome::asserted` and a `REPORT-ONLY` verdict. That mechanism makes C-06's reading a design decision rather than a workflow detail. This ADR states the rule the mechanism implements, so the rule can be checked against the next budget instead of being argued again. It does not live in ADR-0011, which is the queue's drop policy. ADR-0013 is adjacent but separate: it answers *when a red may be merged past*, and this ADR answers *where a budget is asserted at all*.

**This interprets C-06; it does not amend it.** C-06 lives in CLAUDE.md and is the lead's to open, as it was for ADR-0013. In the PRD, C-06 reads "a feature that breaches a budget does not ship until it stops breaching it". That is a statement about shipping. "In CI" is the mechanism the charter chose for it, and this ADR says where that mechanism means something.

## Options considered

1. **Keep asserting the product limit on hosted Windows (`Always`, no ceiling).** This is the letter of C-06. It was rejected because a red there is not actionable and a green is not evidence. Twenty consecutive reds before #56 did not lead to the fix. The fix arrived for another reason, and the reds were explained after the fact. Goal 3 (correctness is provable, not argued) fails both ways. The cost is also borne by every correctness PR, which ADR-0013 would then have to excuse one at a time.
2. **A median-only regression ceiling of about 45 s on hosted Windows, with a change to `determinate()`.** This separates the two groups cleanly (largest post-step median 30.2 s, smallest pre-step median 51.3 s). It was rejected for now, for three reasons:
   - It changes the harness's no-tunable `determinate()` rule for one budget. That rule is the instrument ADR-0013's conditions 2 and 5 rest on.
   - It rests on one day and one image (`20261103`) of post-step data.
   - It has no FR-VLT-04 counterpart: four runs give no percentile to set a ceiling from.

   It stays available as the promotion path below, once there is enough data to derive the ceiling rather than choose it.
3. **Best-of-three re-runs.** Rejected. A p95 budget re-run until one attempt passes asserts the runner's best minute. It is the coin toss `determinate()` exists to refuse, and it still would not make the runner eligible.
4. **Assert where a gate number can be produced; report everywhere else; never stop measuring.** Chosen.

## Decision

**1. The rule.** A budget is enforced where a gate number can be produced. That happens in two places:
- on the reference machine (RM-1, `--profile reference`, scanner on per RISK-012) at each phase gate;
- in CI on any runner the harness rates gate eligible.

A CI job on a runner that cannot produce a gate number reports. It still measures, prints `REPORT-ONLY`, uploads its JSON with `"asserted": false`, and fails the build when the measurement cannot be taken: a missing app, a failed note-count guard, a failed phase-sum check or a digest mismatch. A slow number does not fail it. `REPORT-ONLY` is a verdict, not an error. It is never PASS or FAIL, because a verdict on a figure nothing asserts is F-443's untruth.

**2. Entry into the report-only class is earned per budget, on evidence. It is never the default.** A budget is declared `CiAssertion::WhenGateEligible` only when all three of the following hold, and each is recorded with the runs it came from:
- (a) the CI runner class that runs the job is not gate eligible by `Environment::gate_eligible()`;
- (b) the recorded hosted runs show that the runner's own run-to-run spread, or a term the product does not own (the runner's file I/O), is at least the budget's margin;
- (c) no `ci_limit_ms` ceiling can be derived from those runs.

A budget with a ceiling is `Always`. This is already pinned by `a_when_gate_eligible_budget_has_no_ceiling_and_a_ceiling_means_always`. Today NFR-PRF-04 qualifies. FR-VLT-04 qualifies on the same criteria: (a) the same runner; (b) the watcher delay is fixed at about 173 ms, the processing tail swings 205 to 450 ms p95 between runs minutes apart, and the per-note I/O is 15 to 25 times Linux; (c) four runs give no percentile.

**3. A report-only budget must still be asserted in CI wherever a gate-eligible runner class can run it.** `ubuntu-latest` is gate eligible by the harness's rule. NFR-PRF-04 and FR-VLT-04 therefore each gain a job there, under `--profile ci`, which asserts the product limit. These jobs are a regression gate on Serat's own code: parse, writer and watcher processing. They do not certify the product, which is RM-1's job, and their names say so. A Linux figure is not RM-1's gate figure and is never reported as one. These jobs start as `Always`. If their first hosted runs meet point 2's three conditions, the class can change, but only on that evidence (point 5).

**4. Leaving the class.** A budget leaves the report-only class when either of the following becomes true. It then moves to `CiAssertion::Always` with a `ci_limit_ms`:
- (i) CI gains a gate-eligible Windows runner class, either a self-hosted reference runner or a larger hosted label with the scanner on, with at least 10 runs on which `determinate()` holds;
- (ii) at least 30 runs on hosted Windows after the step, over at least 3 days and 2 image versions, yield a ceiling of 1.5 times the 95th percentile of medians, recorded with the runs it came from.

These are test-engineer's criteria from #166, adopted unchanged. Option 2's `determinate()` change may be proposed then, against that data, as an amendment to this ADR.

**5. NFR-PRF-01 (Serat term) and NFR-PRF-03 stay `Always`.** They are not classed pre-emptively. They have no hosted figure yet, because #164 only just made them measurable. Classifying a budget before its first number is how the class becomes a place to park budgets, and point 2 exists to prevent that. If their first hosted runs come back red, the red is honest, and those runs are the evidence point 2 is judged on.

**6. FR-VLT-04's wiring spec in `nfr-prf-04-gate-quality.md` stands as written:**
- `p1-external-change --assert` prints `REPORT-ONLY` and exits 0 when the runner is not gate eligible;
- the job name says "report-only" and names the reference machine;
- `P1_GATED` gains the same two-way name check;
- best-of-three is rejected.

**`COALESCE_WINDOW` stays at 150 ms.** The window is the design's coalescing: it is why a 100-file `git checkout` arrives as one round and not several. The #165 triage showed it identical in passing and failing runs, so the tail a shorter window would not remove is 200 to 450 ms. Shortening a product constant to buy headroom on a runner that cannot produce a gate number would tune the product to the runner. That is the inverse of goal 3. The window's value is decided by the open question on sync tools (architecture §Open design questions), not by CI headroom. On a quiet machine FR-VLT-04 reads about 180 ms p95, which leaves about 320 ms of margin.

**7. While a budget is report-only on hosted Windows, a PR that changes code on its measured interval carries a gate-eligible figure.** The interval is named by `Budget::interval`. The figure is the point-3 job's line or a `--profile reference` run on any gate-eligible machine, quoted in the PR description. This is CLAUDE.md working agreement 4 applied to the class, and code-reviewer checks it.

**8. What a gate report states.** A gate report has two columns per budget, and they are never merged:
- **CI-asserted:** a job asserted this budget on the gate head and was green, with the runner named.
- **Gate figure:** a determinate, gate-eligible `--profile reference` figure on RM-1, taken on the gate head.

A report-only job counts as **not CI-asserted**. A budget is **enforced for the gate** only if it has a gate figure. A CI assertion is the regression half of C-06 and cannot substitute for the gate figure. A gate figure from a machine other than RM-1 can be cited as supporting evidence but is not the gate figure.

**The P1 gate, applied (task #166's question 4).**

| Budget | CI-asserted | Gate figure (RM-1, scanner on) |
|---|---|---|
| FR-VLT-01, NFR-PRF-06 | `Always` on hosted Windows | Owed. |
| NFR-PRF-01 (Serat term), NFR-PRF-03 | NFR-PRF-03: `Always` on hosted Windows. NFR-PRF-01: none (report-only; point 3 exception, amendment 2026-11-25) | Owed. NFR-PRF-03's 17.01 ms p95 on the Linux container is attested supporting evidence only. |
| NFR-PRF-04, FR-VLT-04 (once #165's wiring lands) | Report-only on hosted Windows; asserted on `ubuntu-latest` once point 3's jobs land | Owed. The 7.9 to 10.8 s rebuild figures are the Linux container's; RM-1's only figure is the P0 27.7 s, scanner off, which is not a gate number. |
| NFR-PRF-02 | Report-only, headless, already | Owed: the WebView2 run. |

Until runners schedule (RISK-013), no budget is CI-asserted on the gate head. **So the P1 gate can count a budget as enforced under C-06 only after its RM-1 scanner-on run, and today that is none of them.** The gate report says so in those words. It does not count a green report-only job, or a Linux figure, as enforcement.

## Consequences

**Positive.**
- C-06 is honoured where honouring it is possible, and the places where it is not are named, mechanical and visible in the job name.
- A red on hosted Windows for these budgets now means "could not measure", which is actionable.
- The class has entry criteria, exit criteria and a test pinning both directions, so it is hard to enter by accident and possible to leave.
- The Linux assertion (point 3) puts a real build-failing regression gate back on the code these budgets time.

**Negative.**
- Between gates, a Windows-only regression in a report-only budget does not fail a PR. The #56-shaped writer stall would have shown up only as a number in a log. Point 7 and the report-only job (still a large-effect detector: it showed the #56 step) are the mitigations, and neither is a build failure.
- The gate now depends on someone running RM-1 with the scanner on at every gate. That cost was always owed by RISK-012, and this ADR makes it unavoidable.

**Risks.**
- The class is the obvious place to put any budget that is red in CI. Point 2's three recorded conditions are the defence, and code-reviewer should refuse a `WhenGateEligible` with no runs cited.
- `gate_eligible()` treats Linux and macOS as eligible unconditionally. That is right for the scanner question it was written for, but it makes point 3's Linux jobs gate-eligible by definition rather than on evidence. If hosted Linux turns out to be noisy for FR-VLT-04 (the triage reproduced the tail on 2 contended cores), point 2 applies to that runner too.

## Objections recorded

**test-engineer** proposed the mechanism and the FR-VLT-04 wiring, and both are adopted unchanged. Their argument against option 2 (one day, one image, a change to `determinate()`) is upheld, and their promotion criteria are adopted unchanged as point 4. They left open whether C-06's letter accepts the reference machine. This ADR answers yes for the gate and no for regressions, and that second half is why point 3 exists.

**rust-core-dev** (#165 triage) said a shorter `COALESCE_WINDOW` "would buy headroom but changes the coalescing" and called that "a design ruling, not a fix". The ruling is to keep 150 ms, for the reasons in point 6.

**Not yet put to a teammate:** point 3 (the Linux asserting jobs) and point 7 (the PR obligation) are the architect's additions in this ruling, and test-engineer should object if hosted Linux cannot carry the 20k vault inside the job's time limit. **The lead** owns C-06. If the lead reads "enforced in CI" as admitting no gate-only budgets, option 2 becomes the fallback, at the costs stated above.

## Tested against

There are two cases, and both produced the rule, which is the weakness to state:
- NFR-PRF-04's 36 hosted runs, which meet point 2 clearly;
- FR-VLT-04's 4 runs, which meet it on thinner data.

The cases that would test it are still to come. One is the first hosted numbers for NFR-PRF-01 and NFR-PRF-03 (point 5): whoever reads them records whether point 2's conditions were met or merely wished for. The other is the first hosted run of point 3's Linux jobs.

## Amendment 2026-11-25 (task #190, proposed by test-engineer, ratified by the architect 2026-11-25)

**Point 5 is superseded for NFR-PRF-01 (Serat term); NFR-PRF-03 stays `Always`.** Point 5 left the first hosted numbers to decide. They arrived in run 427 (main `83b616d`, the first hang-free run after #186): Serat term p95 546.3 ms of 350, n=20, min 237.5, max 711.6, spread 199.6%, scanner OFF, 4 cores, INDETERMINATE. Point 2 judged on that run: (a) not gate eligible, met; (b) the runner's own spread (samples 237.5 to 711.6 ms straddle the 350 ms limit, mean 309.2; platform term p95 709.7) exceeds the margin, met on one run, which is thin; (c) no ceiling is derivable from one run (point 4(ii) wants 30), met. NFR-PRF-01's `prf01-serat-term` job is `CiAssertion::WhenGateEligible` from #190, with its name saying "report-only on hosted windows; asserted on the reference machine".

**Point 3 has a named exception.** A report-only budget gets an asserting CI job on a gate-eligible runner class only if one can run it. Today none can run NFR-PRF-01's Serat term. A gate-eligible Windows runner class (point 4(i)) ends the exception, and the budget then leaves `NO_LINUX_TWIN`: the app-driving jobs build and launch `serat.exe` on windows-latest only, PRD v1.2 bounds the 350 ms to Windows/WebView2 (F-96), and the Linux figure (RM-2) is 1,310.4 ms. The exception is the closed set `NO_LINUX_TWIN` in `bench/budget/src/lib.rs`. A budget in it has no CI job that asserts its product limit, so its gate-report CI-asserted column is "none" and the RM-1 scanner-on run is its only assertion; this is weaker than NFR-PRF-04's position, and ADR-0019 point 7's PR obligation is its mitigation. Entry on one run is provisional: if either of the next two hang-free hosted runs is determinate, point 2(b) was not met and the budget returns to `Always`. Otherwise it leaves only by point 4.

**Unchanged:** the product limit, the frame guard, `determinate()` (no iteration count settles a series whose minimum is below and whose tail is above the limit), point 4's exit criteria, point 8. The job also fails on a hung launch or a frame-gate refusal past #186's retries (point 1's list). In point 8's table, NFR-PRF-01's CI-asserted cell now reads 'none (report-only; point 3 exception)'. NFR-PRF-03 stays `Always`. Whether a budget with no CI assertion of its limit satisfies C-06 is the lead's reading (Objections recorded).

**Objections recorded (amendment).** The lead's reading of C-06: it is NOT satisfied in CI for NFR-PRF-01 while no CI job asserts its limit. The budget counts as enforced only by an RM-1 scanner-on reference run, which is still owed. The P1 gate report lists this as an open gate item, not a pass, until that run exists or a gate-eligible Windows runner class asserts the limit.

## Amendment 2026-11-26 (task #201, architect)

**Point 5, NFR-PRF-03: one checkpoint-overlapped outlier is excused.** NFR-PRF-03 stays `Always`; its all-saves p95 at 50 ms stays asserted (C-06). Evidence: 8 of 14 hosted runs INDETERMINATE, each with exactly one save over 50 ms at p95 13 to 16 ms; of three tagged, two overlapped a checkpoint (442, 447) and one nothing (445); Linux 5 of 6 (#196).

**Rule, `prf03-save-to-index` only.** A straddling series is determinate when exactly one save exceeds the limit, it overlapped a checkpoint (any kind), and the overlapped class has n ≥ 20; the verdict is then the all-saves p95. The class's nearest-rank p95 is the reported bound this guarantees, not a further condition: with one save over, n ≥ 20 puts rank ⌈0.95n⌉ ≤ n − 1 on a save within the limit, and n ≤ 19 puts it at n, the outlier itself, so a save cannot excuse itself (the code checks both; they never disagree). Anything else stays INDETERMINATE, including one unflagged save (one tagged run in seven): nothing attributes it (goal 3). One is the observed maximum; raising it is an amendment.

**Rejected.** More iterations: at about one outlier per 1,000 saves a larger n straddles more often (Linux 4 of 4 at 2,000 to 3,000). Recurrence within N runs: outliers recur in 3 of 7 tagged runs, so no N separates noise from regression. No change: red in 8 of 14 runs at a quarter of the limit.

**Objections recorded.** test-engineer (#196 item 2) proposed judging without the overlapped class; not adopted: it drops 235 of 1,000 saves (run 447) and could hide a class-wide regression. Not yet put to test-engineer (#205).

## Second amendment 2026-11-26 (task #211, architect; implementation task #213)

**Point 5, NFR-PRF-03: one over-limit save carried by the vault's file I/O is excused, beside the checkpoint excusal above.** NFR-PRF-03 stays `Always`. Its all-saves p95 at 50 ms is still the asserted figure (C-06, NFR-PRF-08). This supersedes the first amendment's "including one unflagged save ... nothing attributes it" for one case only: a save whose phase breakdown (TC-184) puts its time in the vault's file I/O.

**Evidence.** Three hosted `windows-latest` runs carried the phase arrays (4 cores, scanner OFF, `--profile ci`, 1,000 saves each; ci-windows report item (a-5)):
- 453 (`e1c25dc`): PASS, p95 16.0 ms, max 28.4 ms. Phase p95: sync 14.99 ms; every other phase 0.55 ms or less.
- 454 (`192af09`): INDETERMINATE, p95 15.3 ms. One save over the limit, 106.4 ms, of which sync was 105.4 ms (99.1%).
- 455 (`402811a`): INDETERMINATE, p95 15.1 ms. One save over the limit, 134.8 ms: write 43.6 ms, sync 89.4 ms, rename 0.7 ms. That is 66.8% in sync plus rename and 99.3% in write plus sync plus rename.

Both outliers have `checkpoint=no merge=no`. In both, hash, load, parse, submit and other add up to 1.0 ms or less. None of the three runs had two saves over. Linux agrees (10,000 saves): the over-limit tail is sync up to 93.1 ms and rename up to 37.8 ms, and every other phase is 6 ms or less.

**Why `write` is in the class.** The class was named "sync plus rename" when the conditions were set (#207, ruled before any hosted phase array existed). Run 455 fell between the two outcomes that ruling defined. It missed (a)'s 90% sync-plus-rename share. It also met none of (c)'s triggers: it was not mostly CPU phases or other, and it did not recur within its run. A rule has to decide that gap. The decision rests on what the `write` phase contains, not on which answer passes 455.
- **What `write` contains** (`atomic::write_exclusive` up to its `Write` mark): a directory check, a permissions stat, `create_new` of the temp file (NTFS: an MFT record, a directory-index insert, a `$LogFile` record) and `write_all` of the note's bytes into the cache. It contains no Serat lock (the open-vault and per-path lock waits are in `hash`) and no Serat computation (the bytes, the hash and the temp name exist before it starts). Only filesystem syscalls on the vault run in it. The same is true of `sync` and `rename`, and of no other phase.
- **How the 43.6 ms fits that.** A create can wait on NTFS metadata while the volume is flushing. A cached write can be throttled when dirty pages pass the cache manager's limit. Both are the disk. The scanner, which #207 named as `write`'s other explanation and a reason for (c), was OFF on this runner by its own machine line. The CPU work in `write` is a copy of a few kilobytes. Run 455's save also spent 89.4 ms in `sync`, while its CPU phases ran at their usual cost (hash 0.1, load 0.2, parse 0.7 ms). So in that window the volume stalled, not the thread.
- **What would tell the disk from a descheduled runner.** Descheduling would inflate whichever phase happened to be running, so CPU phases would sometimes carry the time. They are under a tenth of a typical save (hash, load, parse and submit p95s of 0.00 to 0.53 ms against a 15 ms sync p95), so something like one outlier in ten or more would be CPU-carried. A disk stall lands only in write, sync or rename. Watching for CPU-carried outliers is what (c)'s trigger already does, and what the remainder bound below enforces sample by sample. A split of `write` into create and `write_all` would separate NTFS metadata waits from cache throttling. It is not needed for this rule and is not ruled.

So the attributed class is **the vault-I/O phases: write, sync and rename**. Effect on the evidence: 454 is excused (106.1 of 106.4 ms, 99.7%) and 455 is excused (133.7 of 134.8 ms on the one-decimal figures; 99.3% as reported). Had the class been sync plus rename only, 454 would be excused and 455 would stay INDETERMINATE.

**Rule, `prf03-save-to-index` only.** A straddling series is determinate when all of the following hold. The verdict is then the all-saves p95, which is the asserted figure.
1. **One outlier in total.** Exactly one save is over the limit. The cap is shared with the checkpoint excusal: a series has at most one excused save, whichever attribution excuses it. That save is excused by the checkpoint rule above, or, failing that, by this one. The checkpoint rule is tried first, so a save that qualifies under both is reported as checkpoint-excused.
2. **Phases are present.** The save has the eight phase values (TC-184). With no phases, for example from an older binary, there is no vault-I/O excusal.
3. **Share.** `(write + sync + rename) / ms ≥ 0.90`, where `ms` is the save's elapsed time. `other` and every unmarked microsecond count against the share. So do hash (lock waits), load, parse and submit.
4. **Remainder.** `ms − (write + sync + rename)` is no more than the all-saves p95. In words: the save's non-I/O part is no slower than an ordinary whole save. For an outlier just over 50 ms the share is the binding bound. For a large outlier the remainder is: a 500 ms save at 90% would otherwise hide 50 ms of parse.
5. **Series size.** The series has n ≥ 20. This is the analogue of the class rule; this attribution has no class. With n ≥ 20 and one save over, rank ⌈0.95n⌉ ≤ n − 1 falls on a save within the limit. So the excused save is never the asserted figure, and a PASS means the all-saves p95 is within the limit. The maximum of the other n − 1 saves is within the limit, because exactly one is over.

The 0.90 was set by the #207 ruling before any hosted array existed, and it is kept. The remainder bound is derived from the series itself and adds no constant.

**Guards kept from the #207 ruling.**
- (i) **The per-save fsync count is pinned by a test.** The temp file is synced exactly once, in `sync`. On unix the directory is synced exactly once, in `rename`; on Windows it is not synced at all. No fsync runs in `write`. Without this pin, "carried by vault I/O" would quietly mean something else after a change to `atomic.rs`. The test records order only and changes no byte written (C-04, ADR-0008). The attribution must not land without this test.
- (ii) **The cap is shared** (rule 1).
- (iii) **`other` counts against the share** (rule 3).

**The slow-commit tag is kept as reported-only and printed as saturated.** It was set on 999 to 1,000 of 1,000 saves in all three runs, and on about 99% on Linux. It measures a writer turn, and under the benchmark's back-to-back burst a turn lasts while the queue is non-empty. A lower or higher threshold would still be saturated. A tag that discriminates would be a per-commit duration, which is a new measurement in serat-index and is not ruled here. Dropping the tag would change the exit-line contract in serat-app and remove the one signal for a merge that is still running at the end of the window. Instead, any `tag split` line whose without-class has fewer than 20 saves is printed as saturated ("discriminates nothing on this run"). No verdict rule reads any tag except the checkpoint flag.

**Rejected.**
- **Sync plus rename only.** This would excuse a disk stall that lands in `fsync` and refuse the same stall when it lands in `CreateFile`. The phase boundary is a mark the code chose, not a line between Serat's code and the disk.
- **Every phase, i.e. "one outlier excused".** That is no attribution at all. A CPU-carried outlier (parse, hash's lock wait) is exactly what goal 3 must not excuse.
- **Option (c)** (RISK-017's "tails do not hold" accepted until a gate-eligible runner). None of its triggers fired.
- **Option (b)** (profile changes). It does not remove a disk stall.
- **More iterations.** Rejected in the first amendment for the same reason.
- **Dropping the slow-commit tag, or a new threshold for it.** See above.

**Consequences.**
- Positive: on this evidence, every recorded hosted INDETERMINATE since the phase arrays (454, 455) becomes a PASS with its attribution printed. The verdict is still the all-saves p95. A regression in Serat's code shows up as a p95 shift, a second outlier, or an outlier carried by CPU phases, and all three still fail or stay INDETERMINATE.
- Negative: an excused save is now one the vault's disk carried, not one the index caused. A regression that adds I/O to every save is caught by the p95 and by the fsync pin. A regression that adds rare I/O to one save in a thousand would be excused once per run. On RM-1 with the scanner on, a scanner stall in `write` or `rename` is excused under the same cap. The gate figure is still the all-saves p95 (point 8), so this changes determinacy, not the number.
- Risk: the 90% is a threshold, not a derivation. Raising the cap or adding a phase is an amendment.

**What a hosted run must show (after #213 lands).**
- Every excused save names its attribution, and every vault-I/O excusal prints its share and remainder.
- The all-saves p95 stays in the 13 to 16 ms range recorded so far.
- The rule is reopened toward (c), by amendment, if either of these happens:
  - an over-limit save is mostly in hash, load, parse, submit or other;
  - two or more saves are over the limit in one run, in more than one run out of ten.

**Objections recorded.**
- **test-engineer**, in #207's recommendation, proposed sync plus rename and read "`write` carrying it" as the scanner, so as support for (c). Not adopted, for two reasons: the scanner was OFF on these runs, and 455 is carried by `sync`, not `write`. Not yet put to test-engineer: #213 is that exchange. If test-engineer objects, the fallback is sync plus rename, at the cost that 455's shape stays INDETERMINATE.
- **rust-core-dev** owns serat-vault, where the fsync pin lives. Its review of the pin is part of #213.
- RISK-017's impact text ("INDETERMINATE when ... the outlier is unflagged") becomes partly stale when #213 lands. That text is product-analyst's and the lead's to change.
