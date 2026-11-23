---
project: Serat
artifact: FR-VLT-04 intermittent tail on windows-latest, triage (task #165)
phase: 05-testing
owner: rust-core-dev
serves: FR-VLT-04, NFR-PRF-08
cites: ADR-0001, ADR-0011, ADR-0015, RISK-013
---

# FR-VLT-04 on windows-latest: triage (task #165)

**Date:** 2026-11-23 · **Run by:** rust-core-dev · **Status:** triaged, Windows confirmation pending (no runner, RISK-013)

## Summary

FR-VLT-04 (external change visible in the index, 500 ms p95 over 100 single-note changes) passed in 3 of 4 runs on windows-latest (2 cores) and failed once. The failure and the passes differ only in the **processing** half; the design's fixed half is the same in every run.

| Run (head) | write to visible p95 / max | watcher delay p95 | processing median / p95 / max | batch-of-100 p95 | env line |
|---|---|---|---|---|---|
| 312 (`c24943a`, job 129346710239) | 417.0 / 545.1 ms | 172.9 ms | 8.2 / 256.7 / 377.0 ms | 5547.9 ms | 2 cores, cpu load 8%, 0 build processes |
| 315 (`cf93c32`, job 129357719989) | 376.1 / 1261.2 ms | 173.6 ms | 6.2 / 204.9 / 1095.5 ms | 7712.2 ms | 2 cores, cpu load 6%, 0 build processes |
| 316 (`6a608b8`, job 129359023563) FAIL | 615.3 / 792.0 ms | 172.9 ms | 9.3 / 450.5 / 626.4 ms | 7017.1 ms | 2 cores, cpu load 7%, 0 build processes |

The environment line is taken before the benchmark starts; it cannot see a neighbour that starts during the 3 minutes of the run.

## Latency budget (architecture 3.1.2, the watcher path)

| Stage | Where | Fixed or variable | Measured |
|---|---|---|---|
| notify delivery | `notify` backend (inotify, ReadDirectoryChangesW) | variable, sub-millisecond when scheduled | inside the window figure |
| coalescing window | `COALESCE_WINDOW` 150 ms, `crates/serat-vault/src/watch.rs` | fixed by design | watcher delay median 151 ms Linux, 166 ms Windows |
| window tick | `TICK` 30 ms, same file | fixed granularity | included; Windows adds the 15.6 ms timer resolution |
| read, hash gate | `Reconciler::index_paths`, ADR-0015 stat before read | variable | sub-millisecond |
| parse | `CommonMarkParser` on the consumer thread | variable | 0.1-0.2 ms (p1-save-tail) |
| reader open | `Reconciler::reader()` opens one connection per call | variable | milliseconds |
| index transaction | writer thread, `synchronous = NORMAL`, no fsync on commit | variable | 4-5 ms median |
| flush barrier | consumer waits for the writer to commit | variable | included |
| event emit | serat-app `THROTTLE` 100 ms, not in this measurement (the bench stops at "committed") | fixed, outside the 500 ms interval measured here | not measured |

There is no fixed delay beyond the 150 ms window and the 30 ms tick, and the window is the design's coalescing (the 100-file batch collapses into one round because of it). Median on Windows is 177 ms in all three runs: the 500 ms budget leaves about 320 ms for the processing tail, and 5 to 8 of 100 iterations spend 200-600 ms there.

## Local reproduction (Linux container, 4 cores, release, `p1-external-change --vault 20k --single 100`)

Another agent's build or test run shared the container; the load average and the top foreign process are recorded with each figure. `taskset -c 0,1` mimics 2 cores.

| Condition | Load avg at start | Single p95 / max | Processing median / p95 / max |
|---|---|---|---|
| 4 cores, quiet | 0.60 | 180.5 / 350.2 ms | 4.4 / 24.6 / 198.7 ms |
| 2 cores pinned, quiet | 0.90 to 1.3 | 176.2 / 375.7 ms and 177.1 / 311.1 ms | 4.4-4.6 / 22-25 / 159-224 ms |
| 2 cores pinned, foreign process at 125% CPU | 1.9 | 276.4 / 731.0 ms | 5.2 / 31.6 / 579.8 ms |
| 2 cores pinned, settle 1500 ms, foreign process 11-125% | 1.7 to 3.0 | 581.3 / 1914.7 ms and 637.5 / 935.7 ms | 6.9 / 332.7 / 1691.1 ms and 30.4 / 297.9 / 519.9 ms |
| 2 cores pinned, settle 1500 ms, foreign process 74% (quiet after) | 1.7 | 183.1 / 499.5 ms | 5.1 / 26.8 / 327.9 ms |

Quiet, the Linux figure is the 150 ms window plus about 5 ms: p95 about 180 ms, the 190 ms of the status table. Under foreign CPU load on the same two cores the processing tail appears in the way CI shows (median unchanged near 5 ms, p95 tens to hundreds of ms, max over a second) and the watcher delay itself grows (p95 262 ms in one contended run), which is a starved thread, not a longer window.

## Candidates

- **(a) A fixed delay dominates: rejected.** Window 150 ms and tick 30 ms only; watcher delay p95 is 173 ms in all three CI runs, passing and failing alike. Shortening the window would buy headroom (each 50 ms is 50 ms) but changes the coalescing the window exists for (a 100-file `git checkout` becomes several rounds); that is a design ruling, not a fix, and the tail it would not remove is 200-450 ms.
- **(b) Checkpoint or FTS merge coincidence: not supported.** The bench now reports checkpoint overlap per iteration. Quiet, 12-14 of 100 iterations overlap a checkpoint (the bench's 300 ms settle just exceeds the 250 ms idle gate, so a checkpoint starts about 50 ms before the next write every 2 s) and those cost about 2x more on average (22-24 ms against 8-9 ms), but with a quiet machine at most two iterations exceed 100 ms, and in the contended and settle-1500 runs most slow iterations (14 of 18, 26 of 32) had no checkpoint overlapping and the mean with overlap was lower than without in two runs. A PASSIVE checkpoint takes no lock the writer needs; no RESTART ran (0 of 28 to 87 runs). FTS merges are real tail on the save path (`docs/05-testing/p1-save-tail-2026-11.md`, 45 of 50 slowest saves) but a one-note commit costs 8-130 ms there, not 200-600 ms, and CI max of 1.1 s is beyond it. Not the cause of the runs seen; it is a quantified contributor to the 20-30 ms Linux p95.
- **(c) Windows-only cost not showable on Linux: cannot be excluded.** The batch-of-100 costs 3-5 s median on Windows against 0.2 s on Linux (10-30 ms per note against 2), so file I/O on the runner's volume is an order of magnitude slower per commit; the single-note tail is plausibly the same cost arriving unevenly. Windows-only branches in the path (ReadDirectoryChangesW, mandatory file locking on `-shm` and the WAL, Defender) cannot be exercised here.
- **(d) Runner noise: the best fit.** Same head family, same image, same 2 cores, and the result moves from 376 to 615 ms between runs 3 minutes apart (jobs started 18:40 and 18:43) with an unchanged median. Linux reproduces the shape with foreign CPU load and does not without it. The pre-run environment line (cpu load 6-8%, 0 build processes) shows a quiet start, not a quiet run.

## Verdict

(d) with (c) not excluded. No code path was found to fix: no fixed delay to shorten, no checkpoint or merge dependence, nothing the product does that the quiet figure fails to explain. No product code changed.

## Change made

`bench/budget/src/bin/p1-external-change.rs`, report-only, gating unchanged: per iteration set a "checkpoint overlap" line (iterations overlapped, iterations over 100 ms and how many of those overlapped, mean processing with and without overlap), a "checkpointer" line (PASSIVE runs, incomplete, RESTART, longest, mean), and `--settle-ms` (default 300, the CI job does not pass it). The next Windows run answers (b) against (c) and (d) from its own log.

## Pending on Windows (closing step)

Re-run `budgets.yml` `p1-external-change` on windows-latest when a runner is available (RISK-013) at least three times and read the new overlap and checkpointer lines. If every slow iteration is outside a checkpoint and the median stays near 177 ms, the tail is the runner (record as (d); the options are then a rerun-best-of-three rule or report-only on the hosted Windows runner as for NFR-PRF-04, a lead and architect decision, not taken here). If slow iterations coincide with checkpoints, the idle spacing (2 s, architect ruling #59) is the lever, to be raised with the architect with the Windows numbers. The reference-machine run (`docs/05-testing/reference-machine.md`) remains the binding measurement.
