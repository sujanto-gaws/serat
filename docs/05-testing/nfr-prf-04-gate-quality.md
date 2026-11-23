# Does NFR-PRF-04 have a gate-quality measurement? (task #96)

**Status:** closed — gate-quality PASS, 27,693.5 ms · **Owner:** test-engineer · **Serves:**
NFR-PRF-04, C-06 · **Cites:** ADR-0013, F-327, F-375, task #83

## The question, precisely

C-06 says the performance budgets are binding and enforced in CI. NFR-PRF-04 (full rebuild,
20,000 notes, ≤ 30 s) has been *measured* several times over this spike. Measured is not the
same claim as *gate-quality*: ADR-0013's condition 2 requires a residual — here, a margin
between the reading and the limit — to be stated **against a stated noise floor**, and F-327
recorded that this harness's own noise on this machine is large enough to swallow the budget's
whole margin. This document asks the narrower question #96 was opened for: **is there, right
now, a reading of NFR-PRF-04 that this project could point to and say "that is the gate
number, and here is why it should be trusted"?**

## The evidence on record

Five reference-profile readings of `prf04-full-rebuild` against the canonical 20k vault, same
binary, same digest, taken across two sessions (F-327):

| Reading | Result against 30 s |
|---|---|
| 36,364 ms | FAIL |
| 37,596 ms | FAIL |
| 27,052 ms | PASS |
| 24,400 ms | PASS |
| 19,000 ms | PASS |

Spread across these five: `(37,596 − 19,000) / 19,000 ≈ 98%` — the readings do not merely have
noise, they disagree about the *verdict*. F-327's diagnosis, not repeated in full here: the
movement is between sessions, not within one (three runs in one sitting cluster inside 0.4% of
their own median), and it tracks the writer's own phase, not the vault warming in cache. F-327
named the cause it could not yet confirm: contention from other agents' concurrent `rustc`/
`link` processes on this shared machine.

Task #83 built the instrument that could confirm or rule that out — `Environment` now stamps
`cpu_load_percent` and `concurrent_build_processes` on every reading — and, taken with
`concurrent_build_processes: 0`, four same-session parse-phase readings landed at 3236–3637 ms
(median 3332 ms, 1.3% spread), far below the 31.3 s reading that had been the leading suspect
for the FAIL verdicts. That is evidence *for* the contention hypothesis. **It is not itself a
full-rebuild reading**: #83 measured the parse phase in isolation while writing the
instrument, not a complete `prf04-full-rebuild` run, and it predates this ruling.

## Applying ADR-0013's conditions

ADR-0013 was written for a different question — when a budget-red *merge* may proceed — but
its condition 2 is exactly this document's test, and its checkability was answered directly in
the ADR (test-engineer's consultation, landed `a88d008`): every `Outcome` this harness produces
already carries `spread_pct()`, a `determinate()`/`INDETERMINATE` verdict, and — since #83 — a
machine-load attestation in `Environment::summary()`. The instrument is not the gap. **The gap
is that none of the five readings above were taken with that instrument present.** They predate
`cpu_load_percent`/`concurrent_build_processes` entirely, so none of them can be classified,
after the fact, as an unloaded-machine reading or a contended one. A number cannot be
gate-quality retroactively; the attestation has to exist at the moment it is taken.

## Ruling (as it stood before the attested run below closed the question)

**No — at the time of writing, NFR-PRF-04 did not have a gate-quality measurement**, for a reason narrower
than "the noise is too large": the noise is *unattributed* on every reading taken so far,
because the instrument that could attribute it did not exist when they were taken. This is not
a restatement of F-327; it is F-327 plus #83, applied: the tool to tell a slow machine from a
busy one now exists and has not yet been pointed at a full `prf04-full-rebuild` run.

**What closes this.** One reference-profile `prf04-full-rebuild` run, taken under a declared
measurement window (CLAUDE.md's convention — one machine, one sitting, nobody else building),
with `concurrent_build_processes: 0` in its own `Environment` line. If that reading:

- **passes with `concurrent_build_processes: 0` and `determinate() == true`** (no sample
  straddled 30 s), it is gate-quality and becomes the number #101 reports for NFR-PRF-04. A
  single attested reading is stated as such — not as "the" number without qualification — with
  its `spread_pct()` quoted beside it, per ADR-0013 condition 2's own standard.
- **fails, or the machine cannot be gotten to `concurrent_build_processes: 0`**, that is itself
  the finding, and #101 reports NFR-PRF-04 as *unmeasured at gate quality*, distinct from
  *measured and failing* — the same distinction F-380 already draws for NFR-PRF-03 pre-#90, and
  the same discipline this project's findings register keeps insisting on rather than letting a
  number stand in for a verdict nobody actually reached.

**Not attempted here.** No new measurement is taken in this document. Task #96 is a ruling, not
a reading; the standing rule ("no gate number without a declared window", resume-list
scheduling constraint) applies, and test-engineer has requested the window from the lead.

## Number, taken 2026-09-08 under a declared measurement window

| Field | Value |
|---|---|
| Reading (median of 3, ms) | 27,693.5 |
| Samples (ms) | 26,858.4 / 27,693.5 / 27,713.7 |
| Limit (ms) | 30,000.0 |
| `concurrent_build_processes` | 0 |
| `cpu_load_percent` | 22% |
| `spread_pct()` (n=3) | 3.2% |
| `determinate()` | true (`max_ms` 27,713.7 ≤ `limit_ms` 30,000 — no sample straddled the limit) |
| Verdict | **PASS, gate-quality** |
| `gate_eligible` | false — real-time scanner off (RISK-012); best-case/comparable figure, not the charter-gate figure, same convention every reading in this project has used |
| Vault digest | `35fcb867…` — matches the canonical 20k manifest; not F-372-poisoned |
| Taken | window declared and held by the lead; every other teammate off the CPU for the duration |

**This closes the ruling above.** An attested run — `concurrent_build_processes: 0`, determinate,
3.2% spread, an order of magnitude tighter than the 98% spread across F-327's five unattributed
readings — is exactly what the ruling said would settle the question, and it settled it as a
pass: NFR-PRF-04 now has a gate-quality wall-clock measurement, 27,693.5 ms against a 30,000 ms
limit.

**One distinction for #103 and #101 to keep separate, at architect's request.** *"NFR-PRF-04 has
a gate-quality measurement"* (this document, a wall-clock PASS/FAIL) and *"parse is ~17% of the
rebuild"* (#83, a phase **share** within one session, 1.3% spread across four readings) are both
true and are not the same statistic. A share is computed within one run and tends to survive the
machine moving better than a wall clock does — this run's own phase table shows parse at 14% of
27,694 ms (3,942 ms), a different session from #83's and a different number, which is expected
and is not a disagreement between the two: shares are session-relative too (F-375's lesson,
carried). Neither figure supersedes the other; they answer different questions and #103 should
cite both without treating one as a refinement of the other.

**Corroborating evidence from the same day, from a different instrument.** While this window was
open, two wall-clock-deadline tests elsewhere in the suite — `a_save_through_write_file_reaches_
the_index_without_the_watcher` and `the_wiring_publishes_rebuilding_before_the_rebuild_starts_
and_ready_after` — failed under full-workspace-build contention and passed 1/1 in isolation. This
is the same underlying fact this ruling argues from F-327's historical 35% drift, showing up in a
different place: this machine's timings move enough under load to flip a deadline-sensitive
verdict, unprompted and on the same day, in code that has nothing to do with the benchmark
harness. It is not needed to carry this ruling — the attested run above already closes the
question on its own — but it is independent confirmation of the mechanism the ruling assumes,
and it is worth keeping on record because it is exactly the kind of evidence F-327 predicted
without anyone going looking for it.

## Task #145: code spans leave the scan (not a gate run)

**What changed.** The parser now excludes inline code spans from the tag and wikilink scan
(`crates/serat-parse`), and the exclusion test became a binary search over merged ranges. On
the 20k vault this **changes what is indexed**: tags and links inside code spans no longer
produce rows (about 687 spans' worth), so link and tag counts before and after differ. The
digest and the note count are unchanged, and the budget's note-count guard passed in every run.

**Before and after, `prf04-full-rebuild --profile reference` on the 20k vault** (digest
`35fcb867…`), three runs each of the median-of-3 job, alternated, same host, binaries built from
`271b066` (before: the parent of the first #145 commit) and from the #145 head. **Not attested:**
a shared 4-core Linux host at load average 13 to 15 throughout, `concurrent rustc/link processes
unknown`; these are indicative, not a gate-quality reading.

| Run | before: wall median (spread) | before: parse phase | after: wall median (spread) | after: parse phase |
|---|---|---|---|---|
| 1 | 14,768 ms (25.6%) | 6,521 ms | 13,930 ms (9.7%) | 6,406 ms |
| 2 | 14,602 ms (26.6%) | 6,035 ms | 17,617 ms (40.9%) | 8,229 ms |
| 3 | 15,716 ms (13.4%) | 6,876 ms | 16,929 ms (20.3%) | 6,856 ms |

Medians of the three: before 14,768 ms, after 16,929 ms; parse phase before 6,521 ms, after
6,856 ms. The run-to-run spread inside a single side (13,930 to 17,617 ms after; 14,602 to
15,716 ms before) is larger than the gap between sides, and run 2 after is a 41%-spread outlier
under load, so **no change is distinguishable from noise**. The reviewer's earlier loaded rounds
(parse 4,467 to 4,619 ms and 5,656 to 5,478 ms) point the same way, and a hostile note with
50,000 code spans parses in 124 ms, so the new exclusion is not quadratic. All six readings are
far under the 30,000 ms limit. An attested re-run belongs with the next quiet window.

## Task #166: runner class and gate eligibility on hosted Windows (2026-11-23)

**Ruling (home: ADR-0019; the evidence is here; evidence maps to ADR-0019 point 2, all under section (b) below: (a) scanner OFF in 36 of 36, (b) the spread and file-I/O bullets, (c) the no-derivable-ceiling bullet).** On a GitHub-hosted `windows-latest` runner NFR-PRF-04 is **report-only**: the job
measures, prints `REPORT-ONLY`, uploads the JSON and cannot fail the PR for a slow number. The
assertion is the reference machine's (`--profile reference`, at each gate) and, once #168 lands, ubuntu-latest's. A budget is
"enforced in CI" (C-06) only on a runner that can produce a gate number; this one cannot, so
the report says NFR-PRF-04 is **not CI-asserted** rather than counting a red job as enforcement.
It is mechanical, not a convention: `Budget::ci_assertion = CiAssertion::WhenGateEligible`
(`bench/budget/src/lib.rs`); under `--profile ci` the bin asserts only when
`Environment::gate_eligible()` is true (Linux, or Windows with the scanner on) and exits 0
with `REPORT-ONLY` otherwise. `--profile reference` always asserts, and so does a degraded run or a budget that has a `ci_limit_ms` (always asserted, never report-only). A measurement that cannot be
taken still exits 1; exit 3 (app absent) is unchanged; no new exit code was needed because
"report-only" is a verdict, not an error.

### (a) Regression or variance? Neither: a step down, at one merge

Every `prf04-full-rebuild` job log for runs 279 to 316 (36 jobs; runs 313 and 314 had no
PRF-04 job), fetched and tabulated by run, head, start time, region, runner image and phase:

| Group | Runs | Median of 3 | Producer waited on the writer | Verdicts |
|---|---|---|---|---|
| before `1ec7a9e` (#56 WAL checkpointer, page-cache sizes, merged 2026-11-09 13:46 UTC) | 279-298, 20 runs, 2026-10-20 18:00 to 2026-11-09 13:34 | 51.3 to 131.8 s | 38.2 to 101.8 s | 20 FAIL |
| from `1ec7a9e` | 299-316, 16 runs, 2026-11-09 13:55 to 18:43 | 15.9 to 30.2 s (median of medians 21.1 s) | 1.2 to 2.4 s | 14 PASS, 2 INDETERMINATE (299: one sample 33.9 s; 306: samples 20.6 / 28.4 / 60.1 s) |

- **It clusters by head, not by time.** Run 298 (13:34, 7b06763, image `20261103.298.1`, 55.2 s)
  and run 299 (13:55, 1ec7a9e, same image, 30.2 s, producer wait 1.6 s) are 21 minutes apart on the
  same image; all 19 runs of 2026-10-20/21 across seven regions and two image versions are slow. A
  runner-day effect would not line up with one merge, a region-independent step, and a phase
  that collapses.
- **The phase that moved is the writer's.** In the slow runs `read+hash` 1.4 to 3.9 s and `parse`
  2.1 to 4.2 s are the same as in the fast runs (parse 2.4 to 4.6 s); what differs is
  `producer waited` (the producer blocked on the writer): 38 to 102 s before, about 1.2 to 2.4 s after.
  #56 turned automatic WAL checkpointing off on the writer and set page-cache sizes. The
  coincidence is exact and the mechanism fits; it is **not an A/B** (a Linux A/B cannot reproduce
  the runner's file I/O), so it is attribution by merge boundary, stated as such.
- **No regression between `80123ce` (51.3 s) and `6a608b8` (17.8 s): the direction is an
  improvement of about 2.5 to 6x**, and 17.8 s is the faster end of a 15.9 to 30.2 s band.
- **Concurrency does not explain it:** `concurrent rustc/link processes 0` in 36 of 36; cpu load
  1 to 10% at start in 33 of 36 (71%, 41%, 24% in runs 283, 296, 311, two of them slow, one fast).
  The environment line is taken before the benchmark starts and cannot see a neighbour that
  starts later.
- **Noise is still there after the step, and large:** within one run on one binary, run 306's samples
  span 20.6 to 60.1 s (192%), run 316's 17.7 to 26.7 s (51%), run 299's 26.8 to 33.9 s. Post-#56
  spread: 0.6 to 192%. So the 30 s limit is decided at the edge of the runner's own noise.

### (b) Can a 2-core hosted Windows runner ever be gate eligible? No, by the harness's own rule

`Environment::gate_eligible()` on Windows requires the real-time scanner to be **on**. All 36
logs say `realtime scanner OFF` (and 2 cores). The harness never toggles the scanner
(RISK-012; budget-harness.md), so the hosted runner is ineligible for as long as it ships that
way, which is the prior fact `reference-machine.md` had wrong (it said hosted runners have
real-time protection enabled; corrected in this task). Eligibility is also not the only gap:

- The 30 s is the product limit; RM-1's only figure is 27.7 s (P0, scanner off); 7.9 to 10.8 s is the 4-core Linux container's. The hosted band after #56 is 15.9 to 30.2 s on 2 cores, i.e. the
  budget's margin on this runner is smaller than the runner's own run-to-run spread.
- Hosted file I/O is far slower per note (the #165 triage: batch-of-100 medians 2.9 to 4.8 s,
  about 29 to 48 ms per note against about 2 ms on Linux, with the watcher timer unstarved: delay
  p95 172.9 to 173.6 ms in all three runs while processing p95 went 205 to 450 ms). A rebuild
  writes 20,000 notes; the runner's disk, not the code, is the term that moves. "Scanner off is
  a best case, so a breach there is a real breach" (the old reading of `gate_eligible`) holds
  only if the runner is otherwise no slower than the reference machine; the logs and #165 refute that for
  I/O-bound budgets.
- **No ceiling is derivable now.** A `ci_limit_ms` regression ceiling (the harness's two-thresholds
  design) must exceed the largest sample under the harness's straddle rule, 60.1 s in the
  post-#56 band, which is 2x the product limit and above the pre-#56 minimum sample (48.7 s): it
  would catch nothing the 51 to 132 s step did not already show. A median-only rule at about 45 s
  would separate the two groups cleanly (post max median 30.2 s, pre min median 51.3 s) but
  changes the harness's no-tunable `determinate()` rule and rests on one day and one image
  (`20261103`) of post-step data. Not invented here.

### RM-1 is not a gate number either

RM-1 also runs with the scanner off, so the reference-machine assertion is itself a
`gate_eligible: false` figure (the P0 27,693.5 ms is the only RM-1 NFR-PRF-04 reading). **No gate
figure for NFR-PRF-04 exists anywhere today;** a Defender-on RM-1 run is owed (RISK-012).

### What the job still does, and what is pending

- The hosted job still runs on every PR and push, uploads the JSON (it now carries
  `"asserted": false` beside `"gated": true`), and is red when the rebuild cannot be measured
  (note-count guard, phase-sum check, vault digest). It is useful as a **large-effect detector**:
  it showed the #56 step.
- Not CI-asserted: NFR-PRF-04 on the current head. Asserted on the reference machine only: the
  P0 figure is 27,693.5 ms (scanner off, gate-quality), the P1 figure is unattested (p1-gate-report.md).
- **To promote to asserting in CI (flip to `CiAssertion::Always` with a `ci_limit_ms`)** one of:
  (1) a runner class that is gate eligible and steady: a self-hosted reference runner, or a larger hosted
  Windows runner label if the account can use one, with the scanner on, taking at least 10 runs whose
  `determinate()` is true; or (2) at least 30 post-#56 runs on hosted Windows over at least 3 days and 2 image versions,
  from which a ceiling is set at 1.5x the 95th percentile of medians, recorded here with the runs it came from.
  Both need runners to be scheduled at all (RISK-013); neither can be done from this repository.
- ADR-0019 also gives a `WhenGateEligible` budget an asserting job on ubuntu-latest (task #168, not part of this change).
- The same rule applies to NFR-PRF-01 (Serat term) and NFR-PRF-03 on hosted Windows if their first hosted numbers show the
  same shape; they have no hosted figure yet (the app was not built until #164), so
  they stay `CiAssertion::Always` and are decided on evidence.

### FR-VLT-04 on the same runner class (for #165; decided here, wired there)

The harness's criteria say the same about `p1-external-change` as about NFR-PRF-04: hosted
`windows-latest` is never gate eligible, and the #165 triage shows the term that moves is the
runner's file I/O, not Serat's timer (watcher delay p95 172.9 to 173.6 ms in all three runs while
processing p95 went 205 to 450 ms and max to 1,095 ms; batch-of-100 medians 2.9 to 4.8 s, 15 to 25x
Linux per note). **Ruling (ADR-0019 is its home; evidence here): FR-VLT-04's 500 ms p95 is report-only on hosted Windows and asserted on
the reference machine and on any gate-eligible runner**, the same rule and mechanism. **Best-of-three
is rejected:** a p95 budget re-run until one attempt passes asserts the runner's best minute, a
retry is exactly the coin toss `determinate()` exists to refuse, and it still would not make the
runner eligible. Wiring is in #165's revision, not this branch, because both edit the same bin
and `P1_GATED` test: when `!Environment::detect().gate_eligible()`, `p1-external-change --assert`
prints a `REPORT-ONLY` line and exits 0, the job name says "report-only" and names the reference
machine, and `P1_GATED` gains the same two-way name check. The other `--assert` bins
(`p1-tree-path` 36 to 106 ms against 2 s, `p1-switcher` 5 to 12 ms against 50 ms) have 10x or more
margin and stable figures on the runner; they keep failing a breach there until a hosted figure says otherwise.
PRD v1.9's "gated in CI" for FR-VLT-04 reads "asserted on RM-1 at the gate and on ubuntu-latest in CI (#168); report-only on hosted Windows (ADR-0019)", applied in PRD v1.12.

**The three Windows re-runs #165 needs** (overlap and checkpointer lines; this task's PRF-04
`REPORT-ONLY` lines) are owned by test-engineer as `[P1/test] Windows re-runs of the report-only
jobs when a runner is available` (task to be created by the lead) and are RISK-013's
named action: first runner window after the owner restores scheduling, dispatch `budgets.yml` three
times on the head, record the PRF-04 and FR-VLT-04 lines in this file.
