---
title: First hosted-runner execution since 2026-11-09 (budgets.yml runs 415 and 417)
status: final for the evidence held; two triages open for one more run
date: 2026-11-24
task: 167
serves: FR-VLT-04, NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-06, NFR-PRF-08, FR-VLT-01, US-35, C-06
adrs: ADR-0019, ADR-0014, ADR-0015
test-cases: TC-159, TC-165, TC-168, TC-173, TC-174
---

# First hosted-runner execution since 2026-11-09

Task #167 (test-engineer). Test case TC-159 (TC-158 is held by task #176, in flight).

## What ran

| Run | Head | Trigger | When (UTC, 2026-11-24) | Result |
|---|---|---|---|---|
| 415 attempt 2 | `87b4fed` | workflow_dispatch | 13:05-13:31 | 13 of 17 jobs green, 4 failed (3 budget jobs and `budgets-gate`) |
| 417 | `9ba7cb5` | push | 13:34-13:42 (Linux editor-latency jobs still running at 13:44, past their subject steps) | the same 3 budget jobs red and `budgets-gate` red; 13 green and 1 NFR-PRF-02 Linux job unfinished when checked |
| 416 | `12542f1` | push | in progress when written | not read |

Machine line, both runs, every windows job: `windows · 4 cores · realtime scanner OFF · power plan High performance · cpu load 3 to 4%`, **gate eligible false**. In run 316 (2026-11-09, `6a608b8`) the image reported `windows-2025`, 2 cores. The runner class changed, so Windows figures here are not comparable with 2026-11 run-316 figures. **No hosted figure below is a gate number** (ADR-0019 section 8): the CI-asserted column and the RM-1 gate column stay separate.

The two runs reproduce each other on two heads and two runner instances. For FR-VLT-04 the failure is deterministic (the same rejection, 190 s and 185 s after the rebuild). **For NFR-PRF-01 it is not**: the failing launch index differs (below), so the same code path painted and exited on earlier launches in the same job.

## Per-job verdicts

| Job | Runner | Budget | Verdict |
|---|---|---|---|
| vault | | NFR-MNT-02 | green |
| build-app | windows-latest | serat-app release binary, 6 min | green |
| serat-app-release-guard (incl. bench-bridge, phase-timing-off) | | NFR-PRF-03 | green |
| editor-latency (both headless Linux jobs) | ubuntu-latest | NFR-PRF-02 | green after the #147 stub repair (supporting evidence only, not a gate number) |
| every-budget-has-a-job | | NFR-PRF-08 | green |
| p1-switcher | windows-latest | NFR-PRF-06, 20k gates | green |
| p1-read-paths | windows-latest | reported | green |
| p1-tree-path | windows-latest | FR-VLT-01 | green |
| prf04-full-rebuild | windows-latest | NFR-PRF-04, report-only | green (report-only: not CI-asserted) |
| prf04-full-rebuild-linux | ubuntu-latest | NFR-PRF-04, asserted | green |
| p1-external-change-linux | ubuntu-latest | FR-VLT-04, asserted | green |
| prf03-save-to-index | windows-latest | NFR-PRF-03 | green on both heads. Run 417, job 138484368970, verbatim: `PASS NFR-PRF-03 · prf03-save-to-index; ... p95 over 1000 runs = 1.4 ms, limit 50.0 ms (ci profile); samples n=1000 min 0.7 mean 1.3 max 34.5 ms; save path 1000 attempted · 1000 submitted · 0 writer-refused · 0 handle-missing; vault 20000 notes, digest 35fcb867...; windows · 4 cores · scanner OFF · cpu load 4%; NOT A GATE NUMBER (scanner off)`. Run 415: PASS, figure in artifact `prf03-save-to-index` |
| **p1-external-change** | windows-latest | FR-VLT-04, report-only | **RED** on runs 415 and 417, failure to measure (exit 2); **green from run 418** (see Resolution under Triage 1) |
| **prf01-cold-start** | windows-latest | NFR-PRF-01 | **RED**, no first paint in 60 s |
| **prf01-serat-term** | windows-latest | NFR-PRF-01 | **RED**, same |
| budgets-gate | | all | red, because of the three above |

**What the green NFR-PRF-03 figure is.** `prf03-save-to-index` times `AppState::write_file`'s call into the index from outside, over the stratified 1,000-note sample. That is the hand-off. It is not the apply-to-flush hub-note interval that `p1-save-tail` measures (17.01 ms p95, attested, 2026-11-23). 1.4 ms p95 therefore compares with nothing and is recorded as what it is: the first hosted-Windows NFR-PRF-03 figure with the real binary, a floor under the product's cost. The note in the job output is correct: no CI regression ceiling is set, so the product limit of 50 ms is asserted on a runner that is not the reference machine.

## Triage 1 — FR-VLT-04 on windows-latest: "REJECTED: only 1 of 100 edits were indexed in 10 s"

### The message names the batch shape, not the gated series

The error text is `only {upserted} of {count} edits were indexed in 10 s` and `count` is 1 for a single-note iteration and 100 for a batch iteration (`p1-external-change.rs`, `Run::iteration`). "of 100" is therefore **the batch**. The 100 gated single-note iterations ran first and each completed inside 10 s. The bin prints nothing until both series are done, so that gated series' figure was discarded with the rejection. Consequences for the hypotheses in the task text:

* (a) the #154 cross-process guard refusing the bench's writer: ruled out for this failure. The same `IndexHandle` committed 100 single-note upserts one iteration earlier, and the batch's first round also committed (1 upsert).
* (b) the catch-up stamp or clean-close marker making the watcher treat edits as seen: ruled out for the same reason; the watcher and consumer delivered 100 single edits and one path of the batch.
* (d) edits before the watcher is armed: ruled out. The bin starts the watcher, sleeps 1 s, then runs 100 single edits through it before the batch.
* (c) the notify backend on the new image: not excluded for the batch shape (below), but it is not a "watcher never delivers" case.

### What is left, ranked

1. **The batch needed more than 10 s on this runner (harness limit too tight, runner artefact).** In run 316 (job 129359023563, `6a608b8`) the batch took **7,017 ms p95 and 7,065 ms max** (median 4,773 ms, processing to committed median 4,621 ms) on the 2-core image, and in run 315 (`cf93c32`, job 129357719989) 7,712 ms p95 and 8,091 ms max, against the 10 s rejection limit: 19 to 29 % margin on a shape the PRD does not gate ("a file is modified", FR-VLT-04; the batch is "reported, not gated"). The per-note cost on hosted Windows is file I/O (#165 measured 29 to 48 ms per note against 2 ms on Linux, 100 notes is 3 to 5 s before the index writer). A different disk or antivirus configuration on the new image moves that by tens of per cent. The shape of the failure fits: the first round upserts one path (the watcher's first flush), the second round, 99 paths in one `index_paths` call, does not return in time. The same bin on the Linux container passes with the batch at 1.7 s (debug build, `--single 10 --batches 2 --assert`).
2. **The watcher lost events (a ReadDirectoryChangesW buffer overflow under 100 back-to-back write and rename pairs) and the bench, unlike the app, never acts on `RescanRequired`.** `Watcher` raises `VaultEvent::RescanRequired` on `need_rescan()`; the bench's consumer never calls `take_events()`, so a loss would leave edits unindexed for ever. If this is the cause it is a **harness gap, and a question for rust-core-dev** whether the app's consumer does rescan on it (it is specified to; this bin cannot show it). It is less likely than 1 because the batch worked on 2026-11-09 with the same watcher code and the same 100-file shape.
3. **A product slowdown in the batch path between `6a608b8` and `87b4fed`.** `git log 6a608b8..87b4fed` over serat-index, serat-vault, serat-app and bench/budget touches the watcher (refusal log on_change, rescan backstop), the writer lock (#154, #155), the open check (#69), but not `Reconciler::index_paths` or the work queue, and the Linux asserting twin is green on both heads. Nothing in the diff is a candidate on its own; it cannot be excluded without a per-round log from Windows.

**Timing evidence (hedged).** From the end of the rebuild to the rejection took 190 s in run 415 (13:10:40.7 to 13:13:51.0) and 185 s in run 417 (13:39:23.0 to 13:42:27.9). The Linux single series alone took about 47 s, so on Windows the 100 singles plus several batch iterations (each up to 8 s) most likely completed before one stalled. That supports hypothesis 1 (a slow batch iteration crossing 10 s) over "the watcher never delivers"; it does not prove it.

**Verdict on current evidence: most likely a harness / runner artefact on an ungated shape, discarding a gated result; hypotheses 2 and 3 are not excluded. Not shown to be a product regression; FR-VLT-04 (a Must) is not shown to be breached on Windows or anywhere.** What would settle it: the per-round line below on a Windows run. If the rounds show the second round committing at 10 to 20 s, it is hypothesis 1; if `watcher rescan RAISED`, hypothesis 2 (and a product question); if the queue depth is 99 and no round returns, the consumer is blocked (hypothesis 3, a product finding: file it).

### What changed in the harness (bench/budget only, no product code)

* `p1-external-change`: the batch limit is 60 s (single stays 10 s). A batch that does not complete in 60 s is still a rejection.
* A stalled iteration now prints `only N of M edits were indexed in T s; consumer rounds K [n upserted taken X ms committed Y ms; ...]; watcher rescan RAISED|not raised; queue depth Q`.
* The single series prints `single series done: p95 ...` to stderr before the batch runs, so a batch failure cannot take the gated figure with it.
* The bin prints `watcher armed S s before the first edit; backend ReadDirectoryChangesW|inotify|FSEvents`.
* Unit tests for the stall line and the two limits (TC-159). Locally on Linux (debug): `--vault 20k --single 10 --batches 2 --assert` passes, single p95 264.5 ms, batch 1.7 s.

### Resolution (run 418, task #180)

Run 418 on `3e35ce2`, the first head carrying the #167 diagnostics, and run 419 on `e9b6576` (job 138509620838, success) read FR-VLT-04 on windows-latest with the batch limit at 60 s. The job is **green** in both. No stall line was printed: no batch iteration hit 60 s. Runner: windows-latest, 4 cores, realtime scanner off, power plan High performance, `gate eligible false`; the single series is report-only.

| | Run 418 (`3e35ce2`) | Run 419 (`e9b6576`) | Run 316 (`6a608b8`, windows-2025, 2 cores) | Linux twin |
|---|---|---|---|---|
| Single write to visible, p95 / max (n=100) | **412.7 / 617.4 ms** | 396.4 / 834.1 ms | 615.3 / 792.0 ms | about 181 ms p95 locally (#165); asserted, green in CI |
| Watcher delay, p95 | 172.4 ms | 171.0 ms | about 173 ms | the 150 ms window plus delivery |
| Processing to committed, p95 / max | 247.2 / 455.2 ms | 229.6 / 669.3 ms | | about 5 ms |
| Singles over 100 ms processing, with a checkpoint overlapping | 16 of 17 | 11 of 11 | | |
| Mean processing with / without checkpoint overlap | 189.6 / 12.0 ms | 205.7 / 10.1 ms | | |
| Batch of 100, median / p95 / max (n=20, ungated) | 3,265 / 3,973 / 5,333 ms | 3,282 / 3,816 / 4,314 ms | 4,773 / 7,017 / 7,065 ms | 1.7 s (debug, #167) |
| Batch iterations overlapping a checkpoint | 19 of 20 | 20 of 20 | | |
| Checkpointer, PASSIVE runs, longest / mean | 49, 5,799.8 / 1,691.3 ms | 52, 4,741.5 / 1,493.5 ms | | |

Both heads read the same way: 0 incomplete and 0 RESTART checkpoints. The FTS5 merge split is not a discriminator (run 418: with / without a merge 40.9 / 58.3 ms on singles). The reader open and row lookup is about 1 ms, so the visibility read is not the cost.

**Verdict: not reproduced; no product regression shown.** The rejection of runs 415 and 417 ("only 1 of 100 edits were indexed in 10 s") did not recur on two runs: all 100 singles and all 20 batch iterations completed in both, and the batch max was 5.3 s (run 418) and 4.3 s (run 419) against the old 10 s limit. No batch iteration reached 10 s, so the 60 s limit is not what turned the job green; the failure simply did not recur. Runs 415 and 417 printed no batch times (unknown), and they were 4-core runners too. Triage 1's own test for hypothesis 1 (a second round committing at 10 to 20 s) was not met. **Hypothesis 1 (a batch iteration slower than 10 s on hosted Windows I/O, probably when a checkpoint overlaps) remains the most likely but is unconfirmed.** Where the watcher is concerned: it delivered every edit in both runs. Hypotheses 2 (lost events, `RescanRequired`) and 3 (a blocked consumer) are not supported: watcher delay p95 stayed within the 150 ms window plus 28 ms (max 194.1 ms) and no iteration stalled. The batch shape's cost coincides with the WAL checkpoint overlap on hosted Windows I/O (a correlation of means, not an attribution). The single-note p95 is within 500 ms even on this runner (412.7 and 396.4 ms). That is a report-only figure on a runner that is not gate eligible (RISK-012): it is not a gate number and not CI-asserted. The asserting twin is the Linux job (`p1-external-change-linux`, green).

**What remains open.**

* (a) The checkpoint overlap is the dominant cost of a 100-note external batch (19 of 20 iterations in run 418, 20 of 20 in run 419) and of the slow single iterations (16 of 17, 11 of 11). Processing is about 10 to 12 ms without an overlap and 190 to 206 ms with one. That is a product observation against the checkpointer policy (`checkpoint.rs`: 250 ms idle gate and 4,000-frame bound from #56, 2 s idle spacing from the #59 architect ruling) under hosted Windows I/O. It is worth a rust-core-dev look and is **not a FR-VLT-04 breach**. Task text for the lead is under "Proposed tasks" below (item 5).
* (a-1) **Task #183: candidate remedy landed, unproven on Windows; the next hosted run decides (TC-174).** The idle path of the checkpointer now yields to work in flight: it does not start while the writer is inside a turn or a reconcile round is running (`WalHealth::busy_guard`), it re-arms the idle gate and counts the deferral; the size bound is not deferred, durability and `synchronous` are unchanged, the #59 spacing is unchanged. **FR-VLT-04 line, the p95 the job asserts (single write to visible, 20k vault, release, Linux, this container, ADR-0019 supporting only):** before 181.9 ms (n=100) and 172.3 ms (n=500, instrumented build); after 171.6 ms and 173.6 ms (n=500 each); budget 500 ms. The bench prints `checkpoint overlap p95 processing: with .. (n=..), without .. (n=..), ratio ..x` and the checkpointer line carries the deferral count, so the next hosted run measures the acceptance (with-overlap p95 within 2x of without). **Linux evidence, supporting only (ADR-0019):** the 190 ms was not reproduced as a checkpoint cost. On the 20k vault, release, 500 singles: 58 and 57 overlapped iterations; p95 with / without 29.5 / 16.2 ms (1.82x) and 26.3 / 20.0 ms (1.32x) after, 35.6 / 17.1 ms (2.08x) before; the 100-iteration runs read 7.02x before on n=13 because one iteration held an FTS5 merge commit (apply phase 189 ms of CPU, 28 MB read, 14 MB written, the same 1 in 200 to 500 tail that occurs with the checkpointer switched off). PASSIVE runs on Linux last 10 to 25 ms, so a commit rarely lands inside one. The hosted Windows runs differ in kind: runs last 1.5 s on average and up to 5.8 s, so the checkpoint is in flight for most iterations and "overlap" is nearly the whole sample; the mechanism there (fsync contention with the writer's page writes) could not be observed here and is **not verified**. Deferral cannot interrupt a run already started and the bench's cadence (a write 300 ms after the commit, the gate at 250 ms) starts the run before the next event exists, so it may not move the Windows figure; if the next hosted run shows it does not, the remaining lever is the gate or the run length (an architect matter, #56/#59), not this change.
* (b) The single max is 617.4 ms and 834.1 ms, over 500 ms. FR-VLT-04 gates p95, not max; the tail is probably the same overlap, but the log does not attribute the max (1 of the 17 slow singles in run 418 had no overlap).
* (c) Scanner-on Windows is still unmeasured; no budget has that figure.
* (d) Run 316's p95 of 615.3 ms was on windows-2025 with 2 cores; 4 cores may account for part of the 615 to 413 ms difference. The two runs do not separate runner from code.

**Still red on hosted Windows, stated plainly:** `prf01-cold-start` and `prf01-serat-term` (NFR-PRF-01, intermittent, tasks #178 and #179), and `budgets-gate` as a consequence. FR-VLT-04's windows job is not among the red jobs from run 418 on.

## Triage 2 — NFR-PRF-01 on windows-latest: no `window_visible` in 60 s

Observed in the full job logs (reviewer's copies of runs 415 and 417): the bench launches the release `serat.exe` repeatedly; the run index starts at 0 and advances only after a trace with `window_visible` parses. **Earlier launches in the same job painted and exited.** Each left one `Failed to unregister class Chrome_WidgetWin_0. Error = 1412` line, about 1 s apart, which is the teardown of a *successful* launch; the failing launch then waited 60 s and printed `serat: no startup trace after 60s (timed out): no window_visible mark ...`, exit 2.

| Job | Run | Failing launch index | Earlier launches that succeeded |
|---|---|---|---|
| prf01-cold-start | 415 (`87b4fed`) | run 1 | 1 (run 0) |
| prf01-cold-start | 417 (`9ba7cb5`) | run 2 | 2 |
| prf01-serat-term | 415 | run 6 | 6 |
| prf01-serat-term | 417 | run 12 | 12 |

So the failure is **not deterministic inside a job**: the same binary, vault and session painted in 1, 2, 6 and 12 launches and then hung once. The jobs fail by design on the first such launch. Error 1412 (`ERROR_CLASS_HAS_WINDOWS`) is Chromium's teardown message for a launch that ended; it says nothing about the failing one, which printed nothing of its own. No other `serat:` line appears.

### What the code settles

* **(d) the "No vault open" shell: ruled out by code.** `lib.rs` computes `launch_vault = boot.vault.or_else(launch_last_vault)`, and `launch_last_vault` returns `None` when `boot.vault.is_some()` or `exit_after_ready` is set (`state.rs`, tests around line 6143 pin "`--vault` wins, and a measurement opens only what it was told to"). `cli.rs` rejects `--exit-after-ready` with no `--vault`. A failed open exits 2 with its own message before the window is built. So #151 did not drop the bench's flag. Also, `Startup::trace` checks `window_visible` first, so the message does not say whether vault usable would have been reached: only that the first paint mark is absent.
* **(c) the mark wired to an event that never fires under `--exit-after-ready`: ruled out.** `window_visible` is set in one place only, `mark_first_paint`, a command the page's JS calls from `app/src/startup.ts` after a double `requestAnimationFrame` and a buffered `PerformanceObserver` for `first-contentful-paint`. Nothing in Rust stamps it, and `--exit-after-ready` does not change that path (it only starts the waiting thread). The window is built visible (`WebviewWindowBuilder` has no `visible(false)`), `use_https_scheme(false)` is pinned.
* **What the page needs for the mark:** a frame actually produced by Chromium (so rAF fires and FCP is recorded), the IPC call allowed by the capability, and `validate_paint_evidence` accepting it. The evidence guard rejects with a typed error that is **returned to the page and not printed**; only the accepted path can print a diagnostic. A rejected call and a page that never ran look the same in the job log. That is the gap in the evidence.

### Ranked hypotheses (re-ranked on the run indices)

"The hosted session paints no frames" is **refuted as stated**: it painted on 21 launches across the four jobs. A page that deterministically fails on WebView2 is refuted for the same reason. What is left is a launch that hangs at random within a job:

1. **An intermittently rejected or missing paint mark (a product-side or product-adjacent finding).** The page's `mark_first_paint` call can be refused by `validate_paint_evidence` (missing FCP entry, `raf2 <= raf1`), and the refusal is returned to the page and **never printed**. A race between FCP availability, the double rAF and the observer would hit some launches and not others. Evidence needed: the printed marks snapshot and every rejection (product task 1, filed as #178).
2. **The previous launch's `msedgewebview2` processes still exiting or holding the shared user-data folder when the next launch starts.** Every launch uses the app's default WebView2 data folder (`cold_start_once` sets no per-launch folder), and the bench starts the next process as soon as the last returns. A browser process still tearing down can make the new launch's WebView2 attach to it, or wait. Fits a random failure on launch 1 to 12. Evidence needed: wait for the previous `msedgewebview2.exe` to exit between launches, or a per-run `WEBVIEW2_USER_DATA_FOLDER`, on one dispatch: if the hang disappears, this is it, and it is a harness defect (the bench, not the product).
3. **Intermittent occlusion or focus: the window is not visible to Chromium for that launch**, so rAF and FCP do not fire. A runner-session artefact. Evidence needed: the same dispatch with the occlusion-off flags `--disable-backgrounding-occluded-windows --disable-renderer-backgrounding --disable-features=CalculateNativeWinOcclusion` in `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`.
4. **A slow WebView2 first-run exceeding 60 s.** Now unlikely: launch 0 painted in every job, and a first run is the slow one.

**Product or runner?** Undetermined, and the two are not separable from these logs: 1 is product-adjacent (the guard may be rejecting honest paints on a rare ordering), 2 is a harness defect, 3 is a runner artefact. Whether it also happens on RM-1 is not known from these logs. The job fails by design on a launch that did not paint, and that is correct; what is missing is the evidence to attribute it.

### What the next run must log

Not done in this task because it is product code (`serat-app`, not mine to change) or a workflow dispatch that needs the owner's runner:

* **Product (tauri-app-dev):** at the `no startup trace` exit print which marks are set (window created, setup done, paint attempts and their rejections, buffer rendered, tree first screen), and `eprintln!` every `PaintEvidenceRejected`. Without it a rejected call and a dead page are indistinguishable.
  * **Delivered by #178:** under `SERAT_STARTUP_TRACE` the app prints `serat: webview runtime <version>` (from the watchdog thread, after the paint or the deadline), `serat: paint evidence rejected at <ms> ms: <reason>` per rejected `mark_first_paint`, and, 55 s in (`SERAT_STARTUP_DEADLINE_MS` overrides), if `window_visible` is unmarked, `serat: startup marks after <ms> ms: set=[...] missing=[...]` naming each mark. The bench needed no change: `Command::status()` inherits stderr into the job log. Not delivered: the count of calls that never arrived (a dead page prints no rejection line; the snapshot with `window_visible` missing and no rejection line is that signal).
  * **Dispatched by #179:** the harness half below (per-launch process list, per-launch user-data folder, occlusion-off flags), read against these three lines to choose between hypotheses 1 to 4.
* **Harness (test-engineer, next dispatch):** per-launch evidence from `cold_start_once`: the launch index, the time of each launch and of its exit, and whether any `msedgewebview2.exe` from the previous launch is still alive at the start of the next (the process list, printed on failure). Then one dispatch that waits for the previous `msedgewebview2.exe` to exit between launches and gives each launch its own `WEBVIEW2_USER_DATA_FOLDER`, and one that sets `--enable-logging=stderr --v=0` and the occlusion-off flags; the WebView2 runtime version (`HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` `pv`) and the session (`query session`) printed in an `environment` step. The app's stderr is inherited by the bin (`Command::status()`), so all of it lands in the job log.

### Diagnostic dispatch (task #179)

Serves NFR-PRF-01, NFR-PRF-08, US-35, C-06; honours ADR-0019, ADR-0014. Evidence is two sources: the first budgets run on main to carry #178's lines (run 45532921895, head `fe85178`, 2026-11-25), and one `workflow_dispatch` with `prf01_diagnostic=true` on the task branch (run 45580019582, head `14c4cd6`). A bin run ends at the first hang, so the unit of evidence is "launches until the first hang", and the samples are small.

**Run on main, `fe85178` (the #178 lines, per prf01 job).**

| Job | Launch that hung | Marks snapshot at 55 s | Rejection line before it | Webview runtime |
|---|---|---|---|---|
| prf01-cold-start | run 6 (launches 0-5 painted) | set=[main, state_ready, runtime_ready, setup_done, window_created, buffer_rendered, tree_first_screen, index_settled, watcher, walk_complete] missing=[window_visible, vault_usable] | at 859 ms: `mark_first_paint refused: the second requestAnimationFrame timestamp (188.296) did not follow the first (188.296)` | 157.0.4341.46 |
| prf01-serat-term | run 1 (launch 0 painted) | the same set and the same two missing | at 977 ms: the same refusal, (183.596) and (183.596) | 157.0.4341.46 |

**This settles hypothesis 1 and the shape of the failure.** In both jobs the shell was fully built (`window_created`, `buffer_rendered`, the tree, the index and the watcher all marked), the page did call `mark_first_paint`, and `validate_paint_evidence` refused it because the two `requestAnimationFrame` callbacks carried **the same timestamp** (`raf2 <= raf1`, `crates/serat-app/src/startup.rs`, `frame_gate_failed`). The page (`app/src/startup.ts`, `schedulePaintMark`) calls once, stores `paint mark failed` in a status line and returns `null`; nothing retries, so `window_visible` is never marked and the launch waits out the 60 s. The runtime is WebView2 157.0.4341.46 on every launch (the version query ran on the watchdog thread, after the paint or the deadline, so it was not inside `S_setup` and the "optional refinement" is not needed: the version line was printed on every launch and whether it disturbs the Serat term is not measured: the bins stop at the first hang and print no Serat-term figure). A hung launch is therefore **a one-shot refusal of an honest-looking paint, not a page that never ran, not a missing mark and not a window that was never built**.

**Dispatch, run 45580019582 (`14c4cd6`).** The harness gave each launch its own `WEBVIEW2_USER_DATA_FOLDER` (the log line shows the folder was created, so the loader honoured the variable with no app change) and waited for `msedgewebview2.exe` to exit after each launch (every wait ended quiet in 100 to 1,100 ms; no timeout). Three bin runs per arm. The `prf01-cold-start` arms **measured nothing**: the workflow passed `--skip-scale`, which `prf01-cold-start.rs` documents (lines 34-52) and `cold_start_job_with` honours, but `reported(args)` hands the full list to the parser, which rejects it (`lib.rs:1225`), so the documented flag can never work. Both cold-start arms exited at argument parsing. A harness defect, filed as #187 and fixed there: `cold_start_job_with` now strips `--skip-scale` before the common parse, the usage text documents it, and a test asserts every documented flag of every common-parser budget is accepted. The workflow had stopped passing it (`e275d00`); the `prf01_diagnostic` cold-start arm passes it again. Not re-dispatched, see below. Only `prf01-serat-term` produced launches:

| Arm | Bin run | Launches that painted before the hang | Hung launch, snapshot and rejection |
|---|---|---|---|
| folder | 1 | 9 (0-8) | launch 9: rejection at 3,098 ms, raf (953.896) = (953.896); snapshot: all marks but `window_visible` and `vault_usable` |
| folder | 2 | 3 (0-2) | launch 3: rejection at 1,541 ms, (900.696) = (900.696); same snapshot |
| folder | 3 | 13 (0-12) | launch 13: rejection at 1,633 ms, (1008.43) = (1008.43); same snapshot |
| folder+flags | 1 | 4 (0-3) | launch 4: rejection at 1,604 ms, (763.896) = (763.896); same snapshot |
| folder+flags | 2 | 0 | launch 0: rejection at 1,874 ms, (1377.364) = (1377.364); same snapshot |
| folder+flags | 3 | 0 | launch 0: rejection at 7,296 ms, (6504.764) = (6504.764); same snapshot |

(`folder+flags` = `--enable-logging --v=1 --disable-features=CalculateNativeWinOcclusion` in `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`; the first dispatch's `--enable-logging` wrote no `chrome_debug.log`, so `e275d00` changes it to `=stderr`.)

**Did the arms change the hang rate?** No. Baseline, runs 415/417/418/419 (four jobs, four hangs; launch indices 1, 2, 6, 12): 4 hangs in 25 launches (16%); with main run 45532921895's 2 hangs in 9 launches, 6 in 34 (18%). Per-launch folder with exit wait: 3 hangs in 28 launches (11%), at launch indices 9, 3, 13. Folder plus flags: 3 hangs in 7 launches (43%), all with the same rejection. Every hang in every arm and on main carries the same two-equal-timestamps refusal and the same snapshot. With n of 3 to 4 hangs per arm none of these differences is distinguishable from chance, and the flags arm is not better: the occlusion feature being off did not remove the equal timestamps (assuming the flag reached the runtime; the stderr log that would show it was not captured, `e275d00` adds it). **Hypothesis 2 (shared profile or a browser process still exiting) is ruled out as the cause**: the hang persists with a private profile and with no `msedgewebview2.exe` alive at the next launch. **Hypothesis 3 (occlusion) is not supported**: the flag arm hung at the same or a higher rate. Hypothesis 4 stays refuted.

**Decision.** A rejection line is present on every hang, so this is the **product finding**, not a runner artefact and not a harness defect. The measurement is not wrong to fail: the app did not mark the paint. It is also not a slow shell. The page asks once; Chromium on this runner intermittently hands both nested `requestAnimationFrame` callbacks the same frame time (a nested `requestAnimationFrame` should be delivered a frame later); the guard is right that two callbacks with one timestamp are not two composited frames, but a refusal that the page never retries turns a transient into a permanent no-paint. Filed as #186:

> **`[P1/impl] Retry the paint mark when the frame gate refuses equal requestAnimationFrame timestamps`**, owner tauri-app-dev. IDs NFR-PRF-01, NFR-PRF-08, US-35; ADR-0014 (the paint mark is the only source of `window_visible`; nothing else may call `mark_first_paint`) and the evidence rules in `startup.rs` (F-377, F-396). Reproduction: any `prf01-serat-term` run on windows-latest with the app built from `fe85178`: about 1 launch in 6 to 10 prints `serat: paint evidence rejected at <ms> ms: ... the second requestAnimationFrame timestamp (N) did not follow the first (N)` and then, 55 s in, the marks snapshot with only `window_visible` and `vault_usable` missing (runs 45532921895 and 45580019582, three of three arms). Acceptance: when `markFirstPaint` is refused for the frame gate, `schedulePaintMark` runs a fresh double `requestAnimationFrame` and calls once more, a bounded number of times (for example 5, then stops and records the refusal as now); the guard in `validate_paint_evidence` is **not** weakened (equal or decreasing timestamps still refuse, and the F-377 and F-396 tests stay), and `mark_first_paint` is still the only source of `window_visible` and still first-wins; a vitest drives `schedulePaintMark` with a mocked `requestAnimationFrame` that returns equal timestamps once and increasing ones after, and asserts one successful mark and two calls, plus a mutant that retries without a fresh double rAF goes red; the retry is covered by an assertion that a refusal for any other reason (missing FCP entry, implausible value) is **not** retried. Then re-run `prf01-serat-term` on windows-latest several times: the hang should vanish. Honest limit: why Chromium repeats the frame time here is not known from these logs.

Fix: #186 (this branch); acceptance: ≥3 hang-free prf01 runs on windows-latest, owed.

Open question for the architect, not answered here: why Chromium on this runner delivers the two chained `requestAnimationFrame` callbacks with one timestamp, and whether ADR-0014's "painted frame" bound needs a note that two frames may share a timestamp (the retry accepts the page's second pair; the guard still refuses equal ones).

**ADR-0019 recommendation (for lead and architect).** Keep NFR-PRF-01 on hosted windows-latest as it is (a measurement that fails when the app does not paint) and do **not** make it report-only: the hang is a product defect the job found, and hiding it would delete the only CI signal that shows it. Fix the product task above first; if the hangs persist after it, re-rank. The per-launch data folder should **not** become the bench default: it did not change the outcome, and a persistent profile is what a user's cold start has. The bounded exit wait is kept on by default (it only waits while a `msedgewebview2.exe` exists, logged, at most 10 s once per process, and sits outside the measured interval since t0 is stamped at spawn). Recorded as a recommendation; no ADR is edited here.

**What remains unknown.** Why the two callbacks share a timestamp (a Chromium or compositor behaviour on a hosted, headless-adjacent session; the WebView2 stderr log with `--v=1` was not captured and `e275d00` adds it); whether RM-1 shows it; the cold-start job's own hang rate with these arms (that arm did not run; one more dispatch of `prf01_diagnostic=true` on this branch is owed and needs the lead's go-ahead, since the brief allowed one dispatch). One dispatch is a small sample: 3 hangs in 28 launches and 3 in 7 cannot rank arms against each other; the finding rests on the identical signature of all ten hangs, not on a rate.

## Run 427: #186's first hang-free acceptance run, and the ruling for the hosted serat-term job (task #190, 2026-11-25)

Test case TC-165 (1 of 3 owed), TC-168. Run 427 is budgets.yml on main `83b616d`, the first run after #186 merged (https://github.com/sujanto-gaws/serat/actions/runs/45598311891). Figures below are copied from the job logs by the lead; the raw JSON (artifact `prf01-serat-term`) was not read.

| Item | Value |
|---|---|
| Job | "NFR-PRF-01 · Serat term < 350 ms p95" (139151909216); scale-independence job `prf01-cold-start` (139151909180) passed |
| Launches | 20 of 20 exited cleanly, no hang. Launch 13 took one frame-gate refusal at 671 ms ("the second requestAnimationFrame timestamp (178.83) did not follow the first (178.83)") and completed: the #186 retry working in the field, once |
| Verdict | INDETERMINATE. Serat term p95 **546.3 ms** against 350.0 (ci profile); n=20, min 237.5, mean 309.2, max 711.6, spread 199.6%. "The samples straddle the limit ... Reported as a failure because a run that decided nothing must not go green" |
| Reported beside it | platform term p95 709.7; window-visible p95 948.2; vault-usable 1,161.2; tree-first-screen 1,161.2 |
| Machine | windows, 4 cores, scanner OFF, High performance, cpu 9%: `gate_eligible` false, "NOT A GATE NUMBER"; "no CI regression ceiling is set yet, so the product limit is being asserted on a runner that is not the reference machine" |

**TC-165 status: 1 hang-free run of 3 owed.** Before #186 the baseline was 6 hangs in 34 launches (18%); this run is 0 in 21 attempts (20 launches plus the one refused-then-retried paint). One run cannot show the hang is gone (at 18% per launch, 20 clean launches has probability about 0.82^20 = 2%, so the run is real evidence, but the hang rate was never constant across runs: 4 hangs in 25 in runs 415 to 419). Two more hang-free runs close TC-165.

**Beside RM-2.** RM-2 (Linux desktop gaws-dev-05, 2026-11-24) read Serat term p95 1,310.4 ms, platform term 503.1 ms (RISK-016). Run 427 reads 546.3 ms and 709.7 ms on a different platform, runner and code state (#186 and later). Per the RM-2 rule neither is compared with the other or re-baselines the other; both are above 350 ms and neither is the gate figure. Run 427 is a second data point for RISK-016, a hosted Windows one, not a Windows verdict on the product. P0's Windows figure (RM-1, scanner off, 233.8 ms p95) remains the only Windows number near the limit and is not a gate number either.

### The ruling (ADR-0019 applied to the hosted `prf01-serat-term` job)

**Options.**

(b) **A higher iteration count cannot settle it.** `determinate()` is a range test: a run is determinate only when every sample is at or below the limit or every sample is above it. Here the minimum (237.5 ms) is below 350 and the mean (309.2) is below 350, so "all above" is impossible; "all at or below" needs the 19th and 20th values (546 and 712) to disappear. More iterations only widen the range (the maximum of a sample can only grow). No n settles 546 against 350 on this runner, and a looser rule would be the `determinate()` change ADR-0019 option 2 rejected. Rejected.

(a) **A CI regression ceiling cannot be derived yet.** ADR-0019 point 4(ii) asks for at least 30 runs over at least 3 days and 2 image versions, with a ceiling of 1.5 times the 95th percentile of medians. There is one hang-free run (runs 415 to 419 have hangs and no figure). A ceiling picked from one run is a number chosen, not derived; with a 200% spread it would sit above 546 ms, more than 1.5 times the product limit, and could not tell a real regression from a runner draw. It stays the promotion path: once the 30 runs exist, `ci_limit_ms` is set to 1.5 times the p95 of the per-run medians, recorded with the runs it came from, and the budget returns to `Always` (point 4). The baseline is refreshed on a runner image change, by re-deriving from at least 10 runs after it. Not done now.

(c) **Report-only on hosted Windows, asserted on the reference machine. Chosen.** Applying ADR-0019 point 2:
- (a) the runner is not gate eligible: scanner OFF, in this run and in every hosted run on record. Met.
- (b) the runner's own spread is at least the budget's margin: 199.6% inside one run, platform term p95 709.7 ms (a term the product does not own, and it moves with the runner: window-visible p95 948.2). The samples (237.5 to 711.6 ms) straddle the 350 ms limit around a mean of 309.2. Met on one run of 20 samples. **This is thin evidence** (FR-VLT-04 entered the class on four runs, NFR-PRF-04 on 36); it is recorded as such. Run 427's p95 is 196.3 ms over the limit, but point 2(b) asks for run-to-run spread and one run shows only in-run spread. Check: if either of the next two hang-free hosted runs is determinate, point 2(b) was not met and the budget returns to `Always`. Otherwise it leaves only by point 4.
- (c) no `ci_limit_ms` is derivable from the recorded runs: one run. Met.

**There is no Linux twin to assert on, so point 3 does not apply** ("wherever a gate-eligible runner class can run it"). The app-driving budgets build and launch `serat.exe` on `windows-latest` only (`build-app`, `prf01-serat-term`, `prf01-cold-start`, `prf03-save-to-index`; the ubuntu jobs are `prf04-full-rebuild-linux`, `p1-external-change-linux` and the headless NFR-PRF-02 editor jobs, none of which drives the shell). A Linux desktop does run the bin (RM-2), but PRD v1.2 bounds the 350 ms to Windows/WebView2 (F-96) and RM-2's Linux figure was 1,310.4 ms, so a hosted-Linux assertion at 350 would fail on a platform the budget does not cover and would assert nothing about the product. The exemption is recorded in `NO_LINUX_TWIN` in `bench/budget/src/lib.rs` and pinned by a test (TC-168).

**NFR-PRF-08.** "Every budget has an asserting job": for NFR-PRF-01 the asserting run is `--profile reference` on RM-1 with the scanner on at each gate (ADR-0019 point 1), and the CI job is its measurement twin on the hosted runner, named "report-only on hosted windows; asserted on the reference machine". As for NFR-PRF-04 in #166, the `every-budget-has-a-job` test finds the job, checks it runs the bin under `--profile ci` with no `continue-on-error` and no `|| true`, and checks the two-way name rule (`a_report_only_budget_says_so_in_its_job_name`); `budgets-gate` still needs and checks the job, so a failed measurement still turns the PR red. The difference from NFR-PRF-04 is that no CI job asserts the product limit at all: **NFR-PRF-01 now has no CI-asserted column entry**, and PRD NFR-PRF-08's "a regression fails the build" is weaker for it than for any other budget. The regression half is carried by (i) the job failing on a hang, an absent app, a refused frame past the retries or a degraded series, (ii) the `prf01-cold-start` scale-independence assertion (20k vs 50k Serat term within 20%), still asserted on windows-latest, and (iii) ADR-0019 point 7's obligation on PRs that touch the startup path: quote a `--profile reference` figure from a gate-eligible machine (there is no point-3 job for this budget). Lead's reading of C-06: it is NOT satisfied in CI for NFR-PRF-01 while no CI job asserts its limit; the budget counts as enforced only by an RM-1 scanner-on reference run, which is still owed, and the P1 gate report lists it as an open gate item until that run exists or a gate-eligible Windows runner class asserts it. That weakness is stated, not hidden; the way out is point 4's 30-run ceiling.

**What is not weakened.** The product limit stays 350.0 ms; the frame guard in `validate_paint_evidence` is untouched (the page's retry from #186 is the only change to the paint path); `determinate()` is unchanged, so the same samples on `--profile reference`, or on any gate-eligible runner, still print INDETERMINATE and fail (`the_serat_term_is_report_only_on_hosted_windows_and_asserts_where_a_gate_number_exists`). A degraded series is still not report-only. The earlier recommendation in Triage 2 (do not make NFR-PRF-01 report-only while the hang is an unexplained product defect) is superseded for the slowness only: it was about hiding the hang, and the hang still fails the job.

**Not verified.** No Windows run of the changed workflow; the job name and the `WhenGateEligible` branch are pinned by unit tests on synthetic outcomes, not by a hosted run. The raw samples of run 427 were not read, so the in-run median and the p95 interval are not given. Whether the 20k-versus-50k scale check on `prf01-cold-start` is a tight enough detector for a startup-path regression is not re-examined here (it passed).

## Runs 431 and 432: exit 2 after a refusal, a slow window, and one prf03 straddle (task #192, 2026-11-25)

Test case TC-173. Figures are copied from the CI logs by the lead; nothing here was run on Windows.

### (a) Exit code 2 half a second after a frame-gate refusal: root cause found

Evidence: run 431 (main `5de8807`) `prf01-cold-start` launch 3, run 432 (main `697c457`) `prf01-cold-start` launch 1 and `prf01-serat-term` launch 1. Each shows `paint evidence rejected at 448 / 634 / 662 ms`, then about 70 ms later `serat: no startup trace after 60s (usable): no vault_usable mark: the webview never reported the buffer rendered ...`, then exit 2. Run 427 launch 13 took the same refusal and exited cleanly.

Mechanism (all in `crates/serat-app`):

1. The page's #186 retry sends a second `mark_first_paint`, which succeeds. `window_visible` is now set. The tree, index and watcher marks were evidently already set by then (the failing exit follows the retry by 50 to 70 ms, and the paint is what woke the thread), so the paint is the **last** term of `Marks::vault_usable` (`startup.rs`) and `vault_usable` becomes `Some` at that instant.
2. The benchmark exit thread (`lib.rs` `spawn_benchmark_exit`) was blocked in `Startup::wait_for_usable`; the paint's `notify_all` woke it. It immediately called `startup.trace()`.
3. `trace()` still requires `buffer_rendered` (kept in the trace although ADR-0015 §5 removed it from `vault_usable`). That mark comes from `mark_vault_usable`, which the page (`app/src/startup.ts` `hydrateBuffer`) sends only after `await paintMark`, that is after the paint reply has crossed the IPC. So the buffer mark is one round trip behind, and `trace()` returned `TraceIncomplete { missing: "vault_usable", because: "... buffer ..." }`, printed with the label "usable" because `arrived` was true, and the thread called `exit_now(2)`.
4. Without a retry the paint normally lands before the tree, index and watcher settle, so usable is completed by a later mark and the buffer mark has long arrived. The retry moves the paint to the end, which is why only launches with a refusal are hit, and why run 427 launch 13 (the same refusal) passed: it presumably won the race (inferred, not logged). This is a race the #178/#141 changes created (the buffer left `vault_usable`, the wait did not follow it), exposed by #186.
5. The "after 60s" is `{timeout:?}`, the configured `--ready-timeout-ms` (60 s), not elapsed time. The wait took 50 to 70 ms. The message was truthful about nothing but the limit.

Candidates ruled out by the code: a deadline from a wrong origin (the watchdog `SERAT_STARTUP_DEADLINE_MS` only prints a snapshot and never exits; it did not fire); the harness closing the window on seeing the refusal (the exit comes from `exit_now(2)` inside the process, after `trace()`); a stale env deadline; #188 and #190 (no path in them touches this wait). The 13.8 s "FAILED after" of one launch was not explained from the code and is not attributed.

Fix: `Startup::wait_for_trace_terms` waits for usable **and** the buffer mark; the exit thread uses it; the failure message prints the waited milliseconds and the limit. `vault_usable` and `t_usable` are unchanged (the buffer is still not a term), and the frame guard is untouched.

### (b) A launch with no window for 55 s (run 432 `prf01-serat-term` launch 0)

`startup marks after 55029 ms: set=[main, state_ready, runtime_ready, setup_done, index_settled, watcher] missing=[window_created, ...]`, then "exited cleanly after 79210 ms". The exit was clean and wrote a trace, so the window **was** created, between 55 and 79 s: the main thread was somewhere between `setup_done` and `window_created` for 55 to 79 s. Inside `WebviewWindowBuilder::build()` (which creates the WebView2 environment and controller) is inferred; no log shows it. `window_created` is marked on the line after `build()` returns, and the benchmark exit thread is spawned after it, which is why nothing else ran. Hypotheses, most likely first:

1. First launch of `msedgewebview2.exe` in the job (it was launch 0) paying a cold start of the runtime: Defender or another scanner reading the runtime, a runner-image change, or WebView2 first-run profile creation. It is launch 0, which fits; launches 1 to 19 were fine.
2. WebView2 environment creation waiting on the per-launch `WEBVIEW2_USER_DATA_FOLDER` (folder creation, a lock from an earlier launch's exiting browser process). Less likely at launch 0 (nothing to lock on), likelier at later launches.
3. The main thread blocked between `setup_done` and `build()` (the settings read in `settings_geometry` is a default, so nothing does I/O there). Least likely.

The diagnostic that decides it is now emitted under `SERAT_STARTUP_TRACE`: `serat: main window build entering at N ms (webview data folder: ...)` before `build()` and `... returned at N ms ...` after. If `entering` is present and `returned` late, it is 1 or 2 (then compare launch index and whether `msedgewebview2.exe` was already running); if `entering` is absent, it is 3. No product behaviour changes. The 55 to 79 s launch was accepted by the harness as a sample, which is a measurement problem for the serat-term job (a single sample of about 60 s inside n=20); whether the harness should fail a launch whose window took more than a bound is for test-engineer.

### (c) NFR-PRF-03 `prf03-save-to-index` INDETERMINATE in run 432

p95 16.2 ms, min 0.9, mean 4.2, max 67.5 ms against 50 (ci profile), n=1000, 1000 submitted, scanner off, 4 cores. Run 431 passed. The verdict is INDETERMINATE only because `determinate()` is `max <= limit || min > limit` (`bench/budget/src/lib.rs`): one sample over 50 ms makes the series straddle. The p95 is a third of the limit.

Is the 67.5 ms sample the #183 WAL-checkpoint overlap shape? Plausibly, on the evidence available but unproven: the figures (a mean of 4.2 ms with a single tail sample 16 times that) are the shape #165/#183 found for saves that overlap a WAL checkpoint or an FTS merge on this runner, and the hand-off interval is the one prf03 measures. The raw samples and the overlap counters were not read; the prf03 job does not report checkpoint overlap the way `p1-external-change` does. So: consistent with, not shown to be.

Ruling under ADR-0019: **prf03 on hosted Windows stays `Always`.** Point 2 admits a budget to the report-only class only when (a) the runner is not gate eligible, (b) its own spread is at least the budget's margin, and (c) no ceiling is derivable. (b) is not met: the p95 sits at a third of the limit, and a single sample of 67.5 ms in 1,000 is not a spread the size of the margin; it has passed in most hosted runs, and the point-5 wording (no classing before the numbers show it) still holds. This is not the #190 situation, where the whole series straddled around the limit. The assertion is not changed in this task.

If it recurs (a second straddle in the next three runs): (1) have the prf03 job print the per-sample overlap with checkpoints and the offending sample's position (a diagnostic, like #165's for FR-VLT-04), (2) if the tail samples line up with checkpoints, re-judge point 2 for NFR-PRF-03 and consider whether `determinate()` should judge the p95 for a series this long, which is test-engineer's and the architect's call (it needs an ADR-0019 amendment), and (3) only then reclassify.

## Proposed tasks (for the lead to create; no product code was changed here)

1. **`[P1/impl] Startup trace names the marks it has when the paint mark never arrives`**, owner tauri-app-dev. IDs NFR-PRF-01, NFR-PRF-08, US-35; ADR-0014. Acceptance: on `--exit-after-ready` timeout the stderr line lists each mark set or absent and the count and text of rejected `mark_first_paint` calls; a rejected call prints one `serat:` line; unit tests drive `AppState::mark_first_paint` with rejected evidence and a timeout with partial marks; no change to which evidence is accepted.
2. **`[P1/test] NFR-PRF-01 per-launch evidence and isolation dispatches on windows-latest`**, owner test-engineer, after task 1 lands. IDs NFR-PRF-01, NFR-PRF-08, C-06, ADR-0019. Acceptance: `cold_start_once` logs launch index, times and any surviving `msedgewebview2.exe` on failure; a `workflow_dispatch` input waits for browser-process exit between launches and uses a per-launch user-data folder; another sets the WebView2 logging and occlusion flags; one run each; the report says which of hypotheses 1 to 4 holds. Depends on a runner.
3. **No ADR-0019 decision is proposed yet.** Whether NFR-PRF-01's hosted job can stay an `Always` gate on windows-latest depends on which hypothesis holds; if 2, the fix is the bench's. Decide after task 2's evidence.
4. (Done: task #180, see Resolution under Triage 1.) **`[P1/test] FR-VLT-04 windows batch: re-read after the diagnostic`**, owner test-engineer. Acceptance: on the next dispatch the per-round line and the single-series p95 are recorded; if it shows a rescan raised or a blocked consumer, a product task for rust-core-dev is filed with the round line as reproduction.
5. **`[P1/impl] External-change processing under a WAL checkpoint overlap`**, owner rust-core-dev. IDs FR-VLT-04; NFR-PRF-03 secondary (must not regress the save path); ADR-0001 (the checkpointer is not a writer of content) and architecture §2.2 and §3.1.1 (where `checkpoint.rs` cites). Evidence: run 418 and 419 FR-VLT-04 windows logs (processing 189.6 / 205.7 ms with an overlapping checkpoint against 12.0 / 10.1 ms without; 100-note batch overlapped in 19 and 20 of 20 iterations, median 3.3 s). Scope: the bench gains a p95-by-overlap split line (today it prints means and counts only, and run 419's batch "without" mean is 0.0 because 20 of 20 iterations overlapped). Acceptance: external-change processing p95 with a checkpoint overlap is within 2x of without, with at least 20 iterations in each class, measured on the Linux twin (the bench forces an overlap, for example by running the checkpointer back to back during the series); no change to durability (checkpoint mode, frame bound and synchronous setting are unchanged in effect). Judgement: worth a look, not urgent; the budget is met without it.

## RISK-013

Updated in `docs/01-planning/risk-register.md`: runners executed again 2026-11-24 (the owner fixed the cause). The risk is narrowed, not closed: the gate job is red, the three-dispatch recording owed to `nfr-prf-04-gate-quality.md` is not complete, and no hosted number is a gate number.

## Could not determine

* The windows single-series figure on `87b4fed` and `9ba7cb5` (discarded by the rejection; the job log has only the copy and rebuild lines).
* Per-round timings, rescan state and queue depth of the failed batch.
* The run 415 prf03 and prf04 figures (not fetched), and run 416.
* Any of the NFR-PRF-01 marks present at the timeout; the WebView2 runtime version on the runner.
