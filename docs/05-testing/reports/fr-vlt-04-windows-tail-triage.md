---
project: Serat
artifact: FR-VLT-04 intermittent tail on windows-latest, triage (task #165)
phase: 05-testing
owner: rust-core-dev
serves: FR-VLT-04, NFR-PRF-08
cites: ADR-0001, ADR-0011, ADR-0015, RISK-013
---

# FR-VLT-04 on windows-latest: triage (task #165)

**Date:** 2026-11-23 · **Run by:** rust-core-dev · **Status:** triaged; attribution on Windows open until the instrumented re-run (task #167; no runner, RISK-013); FR-VLT-04 is report-only on hosted Windows (ruled in #166)

## Summary

FR-VLT-04 (external change visible in the index, 500 ms p95 over 100 single-note changes) passed in 3 of 4 runs on windows-latest (2 cores) and failed once. The failure and the passes differ only in the **processing** half; the design's fixed half is the same in every run.

| Run (head) | write to visible p95 / max | watcher delay p95 | processing median / p95 / max | batch-of-100 p95 | env line |
|---|---|---|---|---|---|
| 312 (`c24943a`, job 129346710239) | 417.0 / 545.1 ms | 172.9 ms | 8.2 / 256.7 / 377.0 ms | 5547.9 ms | 2 cores, cpu load 8%, 0 build processes |
| 315 (`cf93c32`, job 129357719989) | 376.1 / 1261.2 ms | 173.6 ms | 6.2 / 204.9 / 1095.5 ms | 7712.2 ms | 2 cores, cpu load 6%, 0 build processes |
| 316 (`6a608b8`, job 129359023563) FAIL | 615.3 / 792.0 ms | 172.9 ms | 9.3 / 450.5 / 626.4 ms | 7017.1 ms | 2 cores, cpu load 7%, 0 build processes |

The environment line is taken before the benchmark starts; it cannot see a neighbour that starts during the 3 minutes of the run. Run 311 (`9ce1c6c`, p95 422.1 ms) is quoted from the status record; its log was not kept. The three logged runs are the same image and job; runs 315 and 316 started 3 minutes apart and 312 about 35 minutes before them. `git diff --stat` over `crates/` and `bench/budget` is empty for `c24943a..6a608b8`, `cf93c32..6a608b8` and `c24943a..cf93c32`: the product code and the bench were identical in all three runs, so the 376 to 615 ms movement is not a code change.

## Latency budget (architecture 3.1.2, the watcher path)

| Stage | Where | Fixed or variable | Measured |
|---|---|---|---|
| notify delivery | `notify` backend (inotify, ReadDirectoryChangesW) | variable | inside the window figure |
| coalescing window | `COALESCE_WINDOW` 150 ms, `crates/serat-vault/src/watch.rs` (kept at 150 ms) | fixed by design | watcher delay median 151 ms Linux, 166 ms Windows |
| window tick | `TICK` 30 ms, same file | fixed granularity | included; Windows adds its 15.6 ms timer resolution |
| read, hash gate | `Reconciler::index_paths`, ADR-0015 stat before read | variable | sub-millisecond |
| parse | `CommonMarkParser` on the consumer thread | variable | 0.1-0.2 ms (p1-save-tail) |
| reader open | `Reconciler::reader()` (`crates/serat-index/src/external.rs:163`): one new connection per drain, four connection PRAGMAs plus `query_only`, cold page cache | variable | not measured in the product; the bench approximates it (a read-only approximation: new read-only connection, the same PRAGMAs, one row lookup), measured after the commit on a row already written so it warms nothing the next write touches: median 0.8 ms, p95 1.0 ms on Linux in the final placement (foreign CPU 0.02 cores, p95 single 179.9 ms in that run). An earlier placement, just before the write on the row that write targeted, read 0.9 / 1.2-1.3 ms and warmed that row's cache; it was moved so #167's runs are comparable with each other. Runs 311-316 carry no such line. Windows not measured: a credible (c) contributor (file open, `-shm` mapping, locks), to read from the next Windows log |
| index transaction | writer thread, `synchronous = NORMAL`, no fsync on commit; FTS5 merges run inside it | variable | 4-5 ms median; FTS5 merge adds about 6 ms mean when one coincides |
| flush barrier | consumer waits for the writer to commit | variable | included |
| event emit | serat-app `THROTTLE` 100 ms, outside the interval this bench measures | fixed | not measured |

The fixed part is the 150 ms window and the 30 ms tick. The CI median is 177 ms in all three runs, so about 320 ms is left for processing, and 5 or more of 100 iterations spend 200-600 ms there (processing p95 205-450 ms).

## What the CI logs say about the shape

Watcher delay p95 is 172.9 / 173.6 / 172.9 ms and max at most 180.6 ms in all three runs, **including the failing one**, while processing reached 626 ms (run 316) and 1095 ms (run 315). The thread that takes the path off the queue was never late; the stall is inside the processing path (file or `-shm` I/O, a Windows lock, an FTS5 merge, the writer thread's scheduling), not a starved timer. That is not the shape CPU contention produces on Linux (below), so the reading "runner noise" is not the best fit by itself: I/O-side stall on the runner's volume ranks at least equal to CPU noise, and the logs cannot separate them.

## Local reproduction (Linux container, 4 cores, release, `p1-external-change --vault 20k --single 100`)

The container is shared with other agents. Each row is one run; the foreign CPU is measured during the single series by the bench (everything busy except the process, in cores of the host's, as `p1-save-tail` does). `taskset -c 0,1` pins the bench to 2 cores. Controlled loaded rows add two `yes > /dev/null` loops pinned one per core.

| Condition | Foreign CPU (cores) | Single p95 / max | Processing median / p95 / max |
|---|---|---|---|
| 4 cores, no pinning (first run, load avg 0.6) | not measured then | 180.5 / 350.2 ms | 4.4 / 24.6 / 198.7 ms |
| pinned 2, settle 300 ms | 0.03 | 176.9 / 310.8 ms | 4.7 / 25.9 / 159.7 ms |
| pinned 2, settle 1500 ms | 0.61 | 179.7 / 326.6 ms | 5.4 / 28.2 / 175.7 ms |
| pinned 2, settle 300 ms, two `yes` loops | 2.51 | 200.3 / 485.7 ms | 8.1 / 37.7 / 334.9 ms |
| pinned 2, settle 1500 ms, two `yes` loops | 2.52 | 208.9 / 675.8 ms | 9.4 / 57.3 / 502.1 ms |
| pinned 2, settle 300 ms (instrumented bench; other load not controlled) | 1.21 | 210.3 / 357.5 ms | 4.8 / 27.7 / 206.6 ms |
| pinned 2, settle 300 ms (instrumented bench; other load not controlled) | 2.01 | 199.8 / 411.5 ms | 5.3 / 31.2 / 260.3 ms |

The earlier uncontrolled table (a foreign process at 125% CPU, p95 276-638 ms) is withdrawn as evidence: settle and load changed together there and it was one or two runs per cell. In the controlled rows saturating both pinned cores moves p95 by 25-30 ms and the max to 0.5-0.7 s, and the quiet-to-quiet differences (settle 300 against 1500) are within noise. CPU saturation alone reproduces the max seen in CI, not the CI p95 of 376-615 ms. No fully quiet window was available for the instrumented runs (foreign 1.2 and 2.0 cores from other tenants of the host), so the quiet claim rests on the first four rows.

## FTS5 merge and checkpoint coincidence (instrumented, two runs, pinned to 2 cores)

`p1-external-change` now reads the FTS5 segment count (the structure-record parser of `p1-save-tail`) before the write and after the commit and flags a merge when the count fell, and flags a checkpoint when one was running at either end or a PASSIVE or RESTART started or finished in between.

| Run | Iterations with a merge | Over 100 ms | Of those with a merge | Mean processing with / without merge | Iterations with a checkpoint | Of the over-100 ms with a checkpoint | Mean with / without checkpoint |
|---|---|---|---|---|---|---|---|
| foreign 1.21 cores | 43 of 100 | 1 | 1 | 11.7 / 8.8 ms | 15 | 0 of 1 | 8.1 / 10.4 ms |
| foreign 2.01 cores | 43 of 100 | 2 | 2 | 15.9 / 9.8 ms | 13 | 1 of 2 | 16.4 / 11.8 ms |

Shown: on Linux every iteration over 100 ms (3 of 3) coincided with a merge, which is what `p1-save-tail` found (45 of the 50 slowest saves); a merge costs about 3-6 ms on average; a checkpoint overlap does not predict the slow ones (1 of 3). Not shown: that either explains the Windows tail. With 43 of 100 iterations merging, a merge coincides with most slow iterations by chance as well as by cost (3 of 3 is weak evidence at this n), and the Windows 200-600 ms stalls are larger than any merge cost seen here (at most 15.9 ms mean, 260 ms max under load). The earlier claim that a merge costs "8-130 ms, not 200-600" is withdrawn: `p1-save-tail` reports 148.9 ms quiet and 257.6 ms loaded for its worst saves.

### Negative control and mutant for the overlap instrument

- **Control (checkpointer cannot fire).** `--quiesce-checkpointer` waits until no PASSIVE has run for 3 s (above the 2 s idle spacing), then `--single 8 --settle-ms 20` runs about 1.4 s of writes, shorter than the spacing, with commits closer together than the 250 ms idle gate. Two runs: the checkpointer line reads 0 PASSIVE runs, **checkpoint overlap 0 of 8**, while the FTS5 merge line still reports **2 of 8** iterations with a merge in both. The instrument reads 0 when there is nothing to overlap and still sees the merges.
- **Mutant (delta dropped).** With the `passive_runs + restart_runs` delta replaced by `false`, so only the in-progress flag at the write and at the commit remains, the default series (settle 300 ms, 100 iterations, 23 PASSIVE runs each) reads **3 and 1** iterations overlapped; restored, the same command reads **12 and 12** (23 runs each). The delta is what catches a 20-100 ms checkpoint that starts and ends inside one 175 ms iteration; without it the instrument misses 75-90% of the overlaps. It is not a total zero because a checkpoint caught live at a sample instant still counts. The mutant was restored and the restored binary re-run before commit.
- Side finding: with settle 20 ms and no quiescing, the first iteration of a short series overlapped a checkpoint twice in two runs and cost 182-190 ms processing against 8 ms; the same bench shape, a write landing on a checkpoint, is the only 100 ms-plus case a checkpoint explained in these runs.

## Candidates

- **(a) A fixed delay dominates: rejected.** A fixed delay explains the passes and the failure equally: window and tick give watcher delay p95 173 ms in all three runs. Shortening the window would buy headroom (50 ms per 50 ms) but changes the coalescing the window exists for (a 100-file `git checkout` becomes several rounds); that is a design ruling, and it was decided to keep 150 ms.
- **(b) Checkpoint or FTS merge coincidence: checkpoint not implicated on Linux; merge implicated on Linux as a small, frequent cost; Windows attribution pending the instrumented re-run.**
- **(c) Windows-only cost: cannot be excluded and ranks at least equal to (d).** The batch of 100 costs 2874 / 3247 / 4772 ms median on Windows (about 29-48 ms per note) against about 0.2 s on Linux (about 2 ms per note), so per-commit file work on the runner is an order of magnitude slower; the single-note tail is plausibly the same cost arriving unevenly. Unmeasured candidates: per-drain `reader()` open, `-shm` mapping and locking, the 15.6 ms scheduler quantum for handoffs between four threads. Defender's real-time scanner was reported OFF in all three runs, which removes the scanner but not other filter drivers.
- **(d) Runner noise: possible, not demonstrated.** The 376 to 615 ms movement between runs 3 minutes apart with an unchanged median fits shared-host variance. CPU contention reproduces the max but not the CI p95 and, by the unchanged watcher delay, did not starve the timer thread in CI.

## Verdict

Not a fixed delay; not the checkpointer on Linux; the CI shape is a processing-path stall whose cause (I/O on the runner volume, Windows locking, merge, or neighbour noise) the logs cannot separate. No product code changed and the window stays at 150 ms. The decision on what CI does with the number was taken in #166: FR-VLT-04 is report-only on hosted Windows under the NFR-PRF-04 rule; best-of-three was rejected.

## Change made

- `bench/budget/src/bin/p1-external-change.rs`: report-only instrumentation (checkpoint overlap including RESTART, FTS5 merge, reader-open approximation, foreign CPU during the single series, checkpointer counters), `--settle-ms` (default 300, above 0; the CI job does not pass it). Under `--assert` the bin prints `REPORT-ONLY` and exits 0 when `Environment::detect().gate_eligible()` is false and the measurement succeeded; it still exits 1 on a breach where the runner is eligible and on any failure to measure (exit 2). FR-VLT-04 is a bin-level `--assert`, not a `Budget`, so `CiAssertion::WhenGateEligible` is expressed as `budget::gate::report_only` / `passes_when_eligible`.
- `bench/budget/src/gate.rs`: those two functions and a test.
- `.github/workflows/budgets.yml`: the job name now says "report-only on hosted windows; asserted on the reference machine"; `bench/budget/src/lib.rs`: `P1_REPORT_ONLY_ON_HOSTED` and a two-way job-name test.

## Pending on Windows (closing step)

The best-of-three against report-only question is closed (#166 ruled report-only; best-of-three rejected). The Windows re-runs are task #167 (RISK-013): at least three runs of `p1-external-change` on windows-latest, read for the checkpoint overlap, FTS5 merge, reader-open and checkpointer lines. If slow iterations coincide with checkpoints, the idle spacing (2 s, architect ruling #59) is the lever; if with merges, the merge policy; if with neither and the reader-open approximation is small, the cause is the volume or the host. The reference-machine run (`docs/05-testing/reference-machine.md`) is where FR-VLT-04 is asserted.
