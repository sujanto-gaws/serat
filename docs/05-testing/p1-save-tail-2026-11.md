# P1 hub-note save tail, after the checkpoint spacing (task #62), November 2026

Serves NFR-PRF-03 (save to index, 50 ms p95) · NFR-PRF-08 (budgets asserted) · ADR-0011. Owner: test-engineer. Harness: `p1-save-tail` (task #53), at integration head `6857025`; attested run 3 at `6655230`.

## Verdict: ATTESTED (run 3, 2026-11-23, head `6655230`). NFR-PRF-03 PASS: p95 17.01 ms against 50 ms.

Run 3 printed `attested 0`: `concurrent_build_processes (rustc, cc1, cc1plus, ld, lld, rust-lld, collect2, link; 102 samples, one per 10 saves plus start/end): min 0 · max 0 · samples with any: 0 · attested 0`. The build was finished and `pgrep -af "rustc|cc1|ld|cargo|node"` showed only a kernel thread before the start; no other agent, gate or build ran for the whole 1013 s. Runs 1 and 2 (below) did not attest and stay diagnostic only; they are kept as the record of why the quiet window was needed.

Command, all runs: `p1-save-tail --vault 20k --iterations 1000 --gap-ms 1000` (release, 4 cores, linux, 1000 saves of `areas/Retention 2.md`, in-degree 679, warm-up 5, product checkpointer: idle 250 ms, size 4,000 frames, idle spacing 2 s). Vault `vault-20k`, seed 20260903, tree digest `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e`. Raw output is not committed (convention of the earlier runs: lines quoted here).

Comparison with the pre-spacing attested 23.3 ms p95 (source report not found, see `docs/01-planning/status-p1-editor.md`): the post-spacing attested p95 is **17.01 ms** (p50 11.54, max 148.86), so the spacing does not regress the tail. The pre-spacing figure stays unsourced; this run is the first sourced attested figure.

| | Run 1 (builds from the start) | Run 2 (started after a quiet window) | **Run 3 (attested)** |
|---|---|---|---|
| Attestation line | `max 12 · samples with any: 87 of 102 · NOT 0, the run is not quiet` | `max 12 · samples with any: 49 of 102 · NOT 0, the run is not quiet` | `max 0 · samples with any: 0 · attested 0` |
| Load average 1/5/15 at start | 10.24 11.45 12.42 | 4.02 10.56 12.46 | 0.61 0.95 0.61 |
| Load average at end | 17.40 11.88 10.95 | 5.44 8.11 9.52 | 0.02 0.07 0.21 |

Run 3 extras: wal file 16,920,872 B at start, 16,777,216 B at end; statements per transaction min 130, median 164, max 4243; FTS5 segments 9 before, 2 to 30 during, merges (count fell) on 874 saves; of the 50 saves above p95, 45 had an FTS5 merge, 0 a checkpoint, mean foreign CPU 0.5%. The worst save (#9, 148.9 ms, 133.6 ms of statements) is the 4,043-frame transaction at the start, at 0.2% foreign CPU: a genuine first-merge cost, not interference.

## Per-phase table (ms, nearest rank over 1000 saves)

| Phase | Run 1 p50 | Run 1 p95 | Run 1 max | Run 2 p50 | Run 2 p95 | Run 2 max | Run 3 p50 | **Run 3 p95** | Run 3 max |
|---|---|---|---|---|---|---|---|---|---|
| **total (apply..flush)** | 22.24 | **42.04** | 257.63 | 21.64 | **44.45** | 240.25 | 11.54 | **17.01** | 148.86 |
| read | 0.04 | 0.07 | 3.79 | 0.04 | 0.07 | 1.31 | 0.07 | 0.10 | 2.26 |
| parse | 0.12 | 0.18 | 12.51 | 0.12 | 0.18 | 3.83 | 0.12 | 0.17 | 2.07 |
| submit | 0.01 | 5.80 | 13.77 | 0.01 | 6.53 | 13.46 | 0.02 | 0.03 | 0.09 |
| wake (queue + BEGIN) | 0.03 | 2.33 | 23.56 | 0.03 | 3.18 | 27.82 | 0.10 | 0.17 | 4.86 |
| txn statements | 15.09 | 27.74 | 239.11 | 14.87 | 27.72 | 219.92 | 8.63 | 12.64 | 133.62 |
| COMMIT (excl. checkpoint) | 4.61 | 15.14 | 85.53 | 3.19 | 15.75 | 96.72 | 2.36 | 3.42 | 89.25 |
| checkpoint (wal hook) | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 |
| post-commit | 0.13 | 5.25 | 30.34 | 0.13 | 5.67 | 30.12 | 0.15 | 0.24 | 2.28 |

Against the 50 ms p95: runs 1 and 2 (42.04 and 44.45 ms) are under it while loaded, a diagnostic reading only. **Run 3, attested, is 17.01 ms: NFR-PRF-03 passes** with 33 ms of headroom. The loaded runs add about 10 ms to p50 and 25 to 28 ms to p95, almost all in the transaction statements and COMMIT.

## Checkpointer (its own thread, ADR-0011)

| | Run 1 | Run 2 | Run 3 (attested) |
|---|---|---|---|
| PASSIVE runs | 501 | 501 | 501 |
| Incomplete / RESTART | 0 / 0 | 0 / 0 | 0 / 0 |
| Checkpoints per minute | 29.3 | 29.3 | 29.7 |
| Frames per checkpoint (mean) | 1371 | 1371 | 1376 |
| Longest / mean checkpoint | 155.0 / 29.4 ms | 164.1 / 30.4 ms | 187.4 / 23.7 ms |
| Saves with a checkpoint in progress at their start or end | 1 of 1000 | 2 of 1000 | 0 of 1000 |
| Checkpoints run by the hook (on the save path) | 0 | 0 | 0 |
| Index WAL pinned | no | no | no |

The structural result is stable across both runs and load-independent: **no checkpoint lands on the save path (hook checkpoint 0.00 ms, 0 hook checkpoints)**, and at most 2 of 1000 saves overlapped one. The tail that remains is in `txn statements` and `COMMIT` (the 50 slowest saves have a mean excess of +14 ms and +24 ms in those two phases in run 1), with 43 of the 50 coinciding with an FTS5 merge, and foreign CPU averaging 96% on those saves, which is the other agents' builds. The worst save of run 1 (#9, 257.6 ms) is 239 ms of statements on a 4,041-frame transaction at 94% foreign CPU.

## Closure of #62

Closed by run 3 (`attested 0`, head `6655230`, 2026-11-23). The tail on a quiet machine is the FTS5 merge and its COMMIT (45 of the 50 saves above p95 coincide with a segment-count fall), not a checkpoint: none lands on the save path in any run. Not re-run on the reference machine (`docs/05-testing/reference-machine.md`); this container has 4 cores, as in runs 1 and 2.
