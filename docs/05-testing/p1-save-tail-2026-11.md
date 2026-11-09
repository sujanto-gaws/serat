# P1 hub-note save tail, after the checkpoint spacing (task #62), November 2026

Serves NFR-PRF-03 (save to index, 50 ms p95) · NFR-PRF-08 (budgets asserted) · ADR-0011. Owner: test-engineer. Harness: `p1-save-tail` (task #53), at integration head `6857025`.

## Verdict: NOT ATTESTED. These figures are diagnostic only.

The instruction was to report `attested 0` or say it did not. **It did not, on either of two attempts.** Two other agents were building throughout (`concurrent_build_processes` is sampled from `/proc` by the harness itself). The second attempt waited for four consecutive quiet polls (about 17 minutes) before it started, and builds began again during the run. The pre-spacing 23.3 ms p95 (attested quiet) therefore has **no quiet post-spacing figure to compare with yet**; #62 stays open until a run prints `attested 0`.

Command, both runs: `p1-save-tail --vault 20k --iterations 1000 --gap-ms 1000` (release, 4 cores, linux, 1000 saves of `areas/Retention 2.md`, in-degree 679, warm-up 5, product checkpointer: idle 250 ms, size 4,000 frames, idle spacing 2 s).

| | Run 1 (builds from the start) | Run 2 (started after a quiet window) |
|---|---|---|
| Attestation line | `max 12 · samples with any: 87 of 102 · NOT 0, the run is not quiet` | `max 12 · samples with any: 49 of 102 · NOT 0, the run is not quiet` |
| Load average 1/5/15 at start | 10.24 11.45 12.42 | 4.02 10.56 12.46 |
| Load average at end | 17.40 11.88 10.95 | 5.44 8.11 9.52 |

## Per-phase table (ms, nearest rank over 1000 saves)

| Phase | Run 1 p50 | Run 1 p95 | Run 1 max | Run 2 p50 | Run 2 p95 | Run 2 max |
|---|---|---|---|---|---|---|
| **total (apply..flush)** | 22.24 | **42.04** | 257.63 | 21.64 | **44.45** | 240.25 |
| read | 0.04 | 0.07 | 3.79 | 0.04 | 0.07 | 1.31 |
| parse | 0.12 | 0.18 | 12.51 | 0.12 | 0.18 | 3.83 |
| submit | 0.01 | 5.80 | 13.77 | 0.01 | 6.53 | 13.46 |
| wake (queue + BEGIN) | 0.03 | 2.33 | 23.56 | 0.03 | 3.18 | 27.82 |
| txn statements | 15.09 | 27.74 | 239.11 | 14.87 | 27.72 | 219.92 |
| COMMIT (excl. checkpoint) | 4.61 | 15.14 | 85.53 | 3.19 | 15.75 | 96.72 |
| checkpoint (wal hook) | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 |
| post-commit | 0.13 | 5.25 | 30.34 | 0.13 | 5.67 | 30.12 |

Against the 50 ms p95: both runs are under it (42.04 and 44.45 ms) **while loaded**, which is a diagnostic reading and not a pass: a gate figure needs `attested 0`. Against the pre-spacing attested 23.3 ms p95: not comparable, because these are loaded and that was quiet. The p50 (about 22 ms) is the nearer comparison to it and is not an equivalent statistic.

## Checkpointer (its own thread, ADR-0011)

| | Run 1 | Run 2 |
|---|---|---|
| PASSIVE runs | 501 | 501 |
| Incomplete / RESTART | 0 / 0 | 0 / 0 |
| Checkpoints per minute | 29.3 | 29.3 |
| Frames per checkpoint (mean) | 1371 | 1371 |
| Longest / mean checkpoint | 155.0 / 29.4 ms | 164.1 / 30.4 ms |
| Saves with a checkpoint in progress at their start or end | 1 of 1000 | 2 of 1000 |
| Checkpoints run by the hook (on the save path) | 0 | 0 |
| Index WAL pinned | no | no |

The structural result is stable across both runs and load-independent: **no checkpoint lands on the save path (hook checkpoint 0.00 ms, 0 hook checkpoints)**, and at most 2 of 1000 saves overlapped one. The tail that remains is in `txn statements` and `COMMIT` (the 50 slowest saves have a mean excess of +14 ms and +24 ms in those two phases in run 1), with 43 of the 50 coinciding with an FTS5 merge, and foreign CPU averaging 96% on those saves, which is the other agents' builds. The worst save of run 1 (#9, 257.6 ms) is 239 ms of statements on a 4,041-frame transaction at 94% foreign CPU.

## To close #62

Re-run on a window where `ps` shows no `rustc`/`cc1`/`ld` for the whole 17 minutes (a dedicated runner or the reference machine, `docs/05-testing/reference-machine.md`), and keep the run only if it prints `attested 0`.
