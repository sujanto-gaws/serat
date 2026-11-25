---
title: First hosted-runner execution since 2026-11-09 (budgets.yml runs 415 and 417)
status: final for the evidence held; two triages open for one more run
date: 2026-11-24
task: 167
serves: FR-VLT-04, NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-06, NFR-PRF-08, FR-VLT-01, US-35, C-06
adrs: ADR-0019, ADR-0014, ADR-0015
test-cases: TC-159
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
* **Harness (test-engineer, next dispatch):** per-launch evidence from `cold_start_once`: the launch index, the time of each launch and of its exit, and whether any `msedgewebview2.exe` from the previous launch is still alive at the start of the next (the process list, printed on failure). Then one dispatch that waits for the previous `msedgewebview2.exe` to exit between launches and gives each launch its own `WEBVIEW2_USER_DATA_FOLDER`, and one that sets `--enable-logging=stderr --v=0` and the occlusion-off flags; the WebView2 runtime version (`HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` `pv`) and the session (`query session`) printed in an `environment` step. The app's stderr is inherited by the bin (`Command::status()`), so all of it lands in the job log.

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
