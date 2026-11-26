---
project: Serat
artifact: Linux desktop reference run, gaws-dev-05, 2026-11-24 (task #177)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, US-35, C-06
cites: ADR-0019, ADR-0014, ADR-0015, RISK-012, RISK-013, RISK-016, F-96, task #218
---

# Linux desktop reference run, gaws-dev-05, 2026-11-24

**Date:** 2026-11-24 · **Run by:** the repository owner (raw JSON, commit `4244c14`) · **Analysed by:** test-engineer, task #177 · **Machine:** RM-2 in `docs/05-testing/reference-machine.md` · **Raw data:** `docs/05-testing/reports/reference-runs/gaws-dev-05-2026-11/`

**Rule for this document:** every number below is copied from those five files or computed from them, and says which. Nothing is compared across machines as a verdict. Where this report sets a figure beside one from the Linux container or from RM-1, the machines differ and the comparison is context.

## Verdicts

| Budget | Verdict on RM-2 (Linux desktop, gate eligible, asserted) | What it is not |
|---|---|---|
| NFR-PRF-04 full rebuild, 20k, 30 s | **PASS**: median 9,152.9 ms of 30,000 (n=3) | Not the RM-1 gate figure; not a Windows figure |
| NFR-PRF-03 save to index, 50 ms p95 | **PASS**: p95 20.247 ms of 50 (n=1000) | Not the RM-1 gate figure |
| NFR-PRF-01 cold start, Serat term, 350 ms p95 | **FAIL**: p95 1,310.4 ms of 350 (n=20) | Not a verdict on the Windows budget: PRD v1.2 bounds the 350 ms to Windows/WebView2 (F-96) |

## Method

**What was run.** The owner ran four budget bins with `--profile reference` on a Linux desktop named gaws-dev-05, 4 cores, on 2026-11-24, against the 20,000-note vault with `vault_digest_sha256` `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e` (format version 1; identical in all five files, so the number and the vault travel together). Five result files, from three bins (`prf01-serat-term` writes the first, `prf01-cold-start` writes the next two, `prf03-save-to-index` and `prf04-full-rebuild` one each). `prf01-cold-start` also runs the section 3.4 20k-versus-50k scale assertion unless `--skip-scale` is given; no scale result is committed, so it is absent or was skipped, and this report has no scale figure:

| File | Budget | Job |
|---|---|---|
| `prf01-serat-term.json` | NFR-PRF-01 | Serat term, the asserted quantity (ADR-0014 Decision 3), with the platform term and the legacy totals reported beside it |
| `prf01-vault-usable.json` | NFR-PRF-01 | process spawn to vault-usable, the legacy 1,000 ms total |
| `prf01-window-visible.json` | NFR-PRF-01 | process spawn to first painted frame, the legacy 400 ms total |
| `prf03-save-to-index.json` | NFR-PRF-03 | `AppState::write_file`'s call into the index, 1,000 distinct notes, a stratified sample (at most one fifth from hub directories) |
| `prf04-full-rebuild.json` | NFR-PRF-04 | `rebuild()` entry to return, writer drained and ANALYZE run |

All five record `os: linux`, `cores: 4`, `realtime_scanner: null`, `power_plan: unknown`, `gate_eligible: true`.

**What was not run.**
- `environment`, before and after the series. There is no stamp of the machine's model, CPU, RAM, storage, kernel, toolchain, webkit2gtk version, or whether anything else was running. The result files carry only the five fields above.
- `p1-external-change` (FR-VLT-04), so RM-2 has no FR-VLT-04 figure.
- `p1-save-tail`, so RM-2 has no `attested` figure for NFR-PRF-03 and no per-phase save-tail breakdown.

**Consequence of the missing stamps.** The run is gate eligible by the harness's rule, which for Linux is unconditional, and that is all it is. It is **not attested** in the sense `p1-gate-report.md` uses (no recorded absence of concurrent builds), so a reader cannot tell a quiet desktop from a loaded one. The build profile is not in question for NFR-PRF-01: the harness rejects any startup trace whose profile is not `release` (`bench/budget/src/trace.rs`), so a prf01 JSON exists only if all 20 traces came from a release build. The first prf04 sample (12,718.8 ms) is 39.0% above the median, which is the shape of a cold page cache, and the files do not say whether the series discarded a warm-up run (`reference-machine.md` protocol step 6). None of this changes a number; it limits what a number can be used for.

**Sample sizes against the requirements.**
- NFR-PRF-01 asks for p95 over 20 runs on the reference machine. n=20 meets the count, with the caveat above that a warm-up discard is not recorded. A p95 over 20 samples is close to the second-largest sample; here 1,310.4 ms, the sample before the 1,419.1 ms maximum.
- NFR-PRF-04 was run with n=3 and reports the median. The P0 rebuild figure was also a median of 3, and the budget is stated as a wall-clock, so n=3 is consistent with the precedent. The spread is large (min 9,105.6, max 12,718.8; max minus min is 39.5% of the median), and three samples cannot say whether 12.7 s is a cold first run or a tail. It leaves 17.3 s of margin to the limit either way.
- NFR-PRF-03 was run with n=1,000 and a closed census (1,000 attempted, 1,000 submitted, 0 writer-refused, 0 handle-missing), as the requirement's acceptance criterion asks.

## Results

| Budget | Statistic | Value (ms) | Limit (ms) | n | Passed | gate_eligible | Asserted | Min / mean / max (ms) |
|---|---|---|---|---|---|---|---|---|
| NFR-PRF-01 `prf01-serat-term` | p95 | 1,310.429 | 350 | 20 | **false** | true | **true** | 1,046.636 / 1,167.405 / 1,419.093 |
| NFR-PRF-01 `prf01-window-visible` | p95 | 1,603.401 | 400 | 20 | false | true | false | 1,228.869 / 1,394.275 / 1,656.640 |
| NFR-PRF-01 `prf01-vault-usable` | p95 | 1,897.888 | 1,000 | 20 | false | true | false | 1,453.005 / 1,637.731 / 1,908.294 |
| NFR-PRF-03 `prf03-save-to-index` | p95 | 20.247 | 50 | 1,000 | **true** | true | **true** | 4.981 / 11.726 / 111.502 |
| NFR-PRF-04 `prf04-full-rebuild` | median of wall clock | 9,152.892 | 30,000 | 3 | **true** | true | **true** | 9,105.566 / 10,325.752 / 12,718.797 |

`degraded` is null in all five. The two legacy NFR-PRF-01 jobs are not asserted (`gated: false`, `asserted: false`) because PRD v1.2 and ADR-0014 retain them as reported figures; their `passed: false` is the harness's arithmetic against the old limits, not a verdict.

**NFR-PRF-01 reported beside the Serat term** (from `prf01-serat-term.json`, `reported_not_asserted`, each p95 over 20 samples):

| Term | p95 (ms) |
|---|---|
| Platform term | 503.086 |
| window-visible (legacy total) | 1,556.105 |
| vault-usable (legacy total) | 1,796.493 |
| tree-first-screen | 1,796.493 |

The legacy totals read lower in this file than in the two separate legacy jobs (1,556.1 against 1,603.4; 1,796.5 against 1,897.9): they are different series of 20 spawns. Both pairs are RM-2's and neither is preferred.

**NFR-PRF-04 phases** (`phases_ms`: the median of each phase across the runs, not the phases of the median run; `bench/budget/src/lib.rs`):

| Phase | ms |
|---|---|
| walk | 73.309 |
| read_hash | 474.200 |
| parse | 2,904.121 |
| producer_wait | 1,718.102 |
| write_fts | 1,514.910 |

The five phases sum to 6,684.6 ms, below the median of 9,152.9 ms. That gap is expected: the producer and the writer overlap, and the bootstrap is untimed (`rebuild.rs`; `budget-harness.md`). `producer_wait` is the time the producer was blocked on the writer, so 1,718 ms of it says the writer did not keep up for that long, and does not say what writing cost.

## Findings

### (a) NFR-PRF-04: PASS, asserted, on Linux

Median 9,152.9 ms of 30,000 ms, n=3 (min 9,105.6, max 12,718.8). The largest phase is parse at 2,904 ms, then producer_wait at 1,718 ms and write_fts at 1,515 ms; walk (73 ms) and read_hash (474 ms) are small.

Context, each from a different machine and not a comparison:
- The 4-core Linux container (shared with other agents' builds) read 7.9 to 10.8 s. RM-2's 9.15 s median falls inside that range, as does its minimum of 9.11 s; its 12.7 s first sample falls outside it.
- RM-1 (Windows, scanner off, P0) read 27,693.5 ms median of 3, which is not a gate number. That is 3.0 times RM-2's median. The difference mixes the operating system, the storage, the file-read cost (RM-1's reads run through NTFS with a different per-file cost) and the core count (16 threads against 4), so it says nothing about either machine alone.

What it supports: Serat's own parse and writer path rebuilds 20,000 notes in about 9 s on a Linux desktop with a margin of 3.3 times. What it does not support: any claim about Windows with the scanner on, which is the budget's binding condition (RISK-012). The RM-1 scanner-on figure is owed.

### (b) NFR-PRF-03: PASS, asserted, on Linux

p95 20.247 ms of 50 ms over 1,000 saves of distinct notes (stratified, at most one fifth from hub directories) (mean 11.726 ms, min 4.981, max 111.502). The census closes: 1,000 attempted, 1,000 submitted, 0 writer-refused, 0 handle-missing, so the p95 is over the whole workload.

Context only: the attested 17.01 ms p95 on the Linux container (`p1-save-tail-2026-11.md`, `attested 0`) is a different quantity. `p1-save-tail` saves one note 1,000 times (the highest in-degree note, in-degree 679, in-process apply to flush); `prf03-save-to-index` saves 1,000 distinct notes drawn across the vault. The two are not compared and neither ranks the other. RM-2's run is not attested (no `p1-save-tail`, no environment stamp), so the 20.247 ms is a figure with unknown concurrent load; the hub-note save on RM-2 itself is owed.

The PRD's v1.13 note holds: the 50 ms binds the hub-note class and the 20k vault's own distribution, and the 5,000-line class is a reported scenario with no figure. RM-2 gives none for it.

### (c) NFR-PRF-01: FAIL, asserted, on Linux, and not a verdict on the Windows budget

The Serat term read p95 1,310.4 ms against 350 ms (n=20; min 1,046.6, mean 1,167.4, max 1,419.1). The harness asserted it on a gate-eligible machine and the run FAILED. Beside it, reported and not asserted: platform term 503.1 ms, window-visible 1,556.1, vault-usable 1,796.5 and tree-first-screen 1,796.5 ms.

**What this is, plainly.** PRD v1.2 re-baselined the 350 ms as a split, and bounded that re-baseline to Windows/WebView2 (F-96): "WKWebView and WebKitGTK are unmeasured. If their platform terms are of the same order, the requirement was wrong rather than the design". This run is the first figure from webkit2gtk. It is a Linux finding (attributed 2026-11-26, #218, in the section below), produced on a machine the harness rates gate eligible, so it is an asserted FAIL there. It is not a verdict on the Windows budget, whose gate figure (RM-1, scanner on) is owed and whose last reading was 233.8 ms p95 on WebView2 at P0.

**What the file shows and what it does not.**
- The result is far outside the budget, not marginal: the smallest of the 20 samples (1,046.6 ms) is about three times the limit.
- Within this machine, the platform term (503.1 ms p95) is well below the Serat term (1,310.4 ms p95), so the failure is not explained by webview creation. The Serat term is Serat's own four marks (`S_pre`, `S_setup`, `S_shell`, `S_settle`), so the PRD's same-order-platform-terms clause does not excuse it. No other machine's figure is used for this.
- The file carries the sum of the Serat term only, not its four marks (`S_pre`, `S_setup`, `S_shell`, `S_settle`), so it cannot say which mark carries the 1.3 s. That question is answered by the per-mark trace of 2026-11-26 (section "Per-mark attribution, 2026-11-26" below).
- The build profile is recorded by construction: the harness refuses a startup trace whose profile is not `release` (`trace.rs`; the harness drives `target/release/serat`; TC-045), so a debug build is ruled out. What blocks attribution is that the JSON carries the Serat-term sum and not its marks. The bin that prints `S_pre`, `S_setup`, `S_shell` and `S_settle` per trace is `prf01-cold-start-terms`; it was run on RM-2 on 2026-11-26 (task #218), and the result is in the section "Per-mark attribution, 2026-11-26" below.
- tree-first-screen and vault-usable are identical (1,796.493 ms), and that is explained: `tree_first_screen` is an independent mark, and `vault_usable` is the latest of paint, tree first screen, index settled and watcher (`trace.rs`). Equal p95s mean the tree's first screen was the last mark in the samples that set them. The same equality appears in the harness's own fixture.

**Recommendation to the owner (a decision, not a re-baseline).** Does NFR-PRF-01 bind Linux in P1? The PRD's scope says no: the 350 ms is bounded to Windows/WebView2, and the "one platform" bound states that other webviews are unmeasured and that, if their platform terms are of the same order, "the requirement was wrong rather than the design". The PRD does not yet say what P1 does when a measured non-Windows figure fails. That is a decision with a risk attached, so it is recorded as **RISK-016** in `docs/01-planning/risk-register.md`, owned by the owner pending the decision. If the owner wants the requirement stated per platform, that is a product-analyst task, and this report proposes it, to be filed by the lead: **"[P1/spec] State whether NFR-PRF-01 binds Linux/webkit2gtk or only Windows/WebView2; do not re-baseline without a per-mark trace (serves NFR-PRF-01, F-96, ADR-0014)"**. Nothing here re-baselines anything: no target in the PRD, the harness or CI changes with this report.

**Do not conclude** that the Windows budget is wrong, or that the Windows Serat term has regressed. Nothing in this run measures Windows.

## Per-mark attribution, 2026-11-26

**Source:** `docs/05-testing/reports/reference-runs/gaws-dev-05-2026-11/prf01-cold-start-terms.jsonl` (the owner's run of `prf01-cold-start-terms` on gaws-dev-05, release binary, 20k vault, 20 iterations, committed `ef835ca`; analysed by test-engineer, task #218). Percentiles use the harness's nearest-rank convention (`Samples::percentile`: rank = ceil(q n), so the median of 20 is the 10th smallest and the p95 the 19th). Nearest-rank medians sit a little off an interpolated reading, for example `s_settle` 228.2 ms here against 229.6 interpolated, and the frame-to-paint gap median is -61.0 ms rather than -53.5. Every figure is computed from the file; none is copied from another run.

### Term definitions (ADR-0014 section Decision 1, `crates/serat-app/src/startup.rs` marks)

| Term | Interval | Owner and content |
|---|---|---|
| `p_load` | `t0` to main | platform: loader, shared libraries, runtime init |
| `s_pre` | main to state ready | Serat, Rust: arguments, settings TOML, `AppState` |
| `p_builder` | state ready to runtime ready | platform: `tauri::Builder` construction |
| `s_setup` | runtime ready to setup done | Serat, Rust: event sink, `open_vault`, thread spawns, boot info |
| `p_webview` | setup done to window created | platform: `WebviewWindowBuilder::build` |
| `s_shell` | window created to `window_visible` | Serat, front end: our bundle loaded and booted inside the webview, to the first composited frame (the double-rAF paint mark) |
| `s_settle` | `window_visible` to `vault_usable` | Serat, front end: the rest of the usable preconditions, here always `tree_first_screen` (root listing applied and painted, ADR-0015 section 5) |

`s_main = s_pre + s_setup` (Rust, main thread); `s_front = s_shell + s_settle` (our front end in the webview); `serat_term = s_pre + s_setup + s_shell + s_settle`; `platform_term = p_load + p_builder + p_webview`; the seven terms sum to `vault_usable` (ADR-0014).

### Table (n=20 per row, milliseconds)

| Term | Median | p95 | Min | Max | Owner |
|---|---|---|---|---|---|
| `p_load` | 54.2 | 63.5 | 49.3 | 90.7 | platform |
| `s_pre` | 0.1 | 0.1 | 0.1 | 0.1 | Serat Rust |
| `p_builder` | 99.9 | 106.5 | 94.3 | 108.9 | platform |
| `s_setup` | 0.3 | 0.4 | 0.3 | 0.6 | Serat Rust |
| `p_webview` | 287.9 | 356.0 | 246.9 | 775.3 | platform |
| `s_shell` | 885.3 | 958.2 | 771.8 | 1,469.7 | Serat front end |
| `s_settle` | 228.2 | 271.6 | 181.7 | 335.1 | Serat front end |
| `s_main` | 0.4 | 0.6 | 0.4 | 0.6 | Serat Rust |
| `s_front` | 1,130.3 | 1,293.4 | 976.0 | 1,669.2 | Serat front end |
| **`serat_term`** | **1,130.8** | **1,293.8** | 976.4 | 1,669.8 | Serat |
| `platform_term` | 443.3 | 508.4 | 402.5 | 964.3 | platform |
| `window_visible` | 1,328.1 | 1,423.8 | 1,241.2 | 2,434.6 | total |
| `vault_usable` | 1,572.1 | 1,759.0 | 1,467.1 | 2,634.1 | total |
| `tree_first_screen` | 1,572.1 | 1,759.0 | 1,467.1 | 2,634.1 | total |
| `paint_evidence` (webview clock) | 617.0 | 678.0 | 486.0 | 827.0 | diagnostic |
| `raf2` (webview clock) | 678.0 | 717.0 | 548.0 | 917.0 | diagnostic |
| `frame_to_paint_gap` (`paint_evidence - raf2`) | -61.0 | -37.0 | -90.0 | -30.0 | diagnostic |

`vault_usable` equals `tree_first_screen` in all 20 rows (largest difference 0.0 ms), so the tree's first screen was always the last precondition and `s_settle` is exactly the time from the first paint to the tree's first screen.

### Consistency with RM-2's 2026-11-24 run

| Figure | RM-2 2026-11-24 (n=20) | This run (n=20) | Difference |
|---|---|---|---|
| Serat term p95 | 1,310.4 | 1,293.8 | -1.3% |
| Platform term p95 | 503.1 | 508.4 | +1.1% |
| `window_visible` p95 | 1,556.1 | 1,423.8 | -8.5% |
| `vault_usable` p95 | 1,796.5 | 1,759.0 | -2.1% |

The two runs agree within about 2% on the Serat and platform terms and within 9% on `window_visible`; they are different samples of the same machine and build lineage, so the earlier figure is reproduced, not contradicted. (This run's smallest Serat term, 976.4 ms, is below RM-2's 1,046.6; the two runs are not the same iterations and the means were not compared.) The tail is one iteration, described below.

### Attribution

- **The Linux Serat term is carried by two front-end marks.** `s_shell` is 885.3 of 1,130.8 ms at the median (78.3%) and `s_settle` 228.2 ms (20.2%); together 98.5%. The Rust-side terms are `s_main` 0.4 ms, three orders of magnitude below the term (0.04%). Rust is not where the 1.1 s is.
- **So the term is the webkit2gtk front-end bootstrap, not Serat's Rust.** `s_shell` is our bundle's load, parse, execute and mount in the webview to the first composited frame; `s_settle` is the further time until the file tree's first screen is applied and painted. The platform term (443.3 ms median) is a separate, smaller figure and this run does not blame it: it is below the Serat term, and the Serat term's own Rust part is under a millisecond.
- **Sizing.** `s_shell` alone (885.3 ms median, 771.8 minimum) is 2.2 to 2.5 times the whole 350 ms budget, so removing `s_settle` entirely would not bring Linux inside it, and neither would any Rust change.
- **What the diagnostics add, and their limit.** `paint_evidence` and `raf2` are stamped on the webview's navigation-origin clock, not the harness `t0` clock, so they must not be subtracted from `t0_to_*` marks (trace.rs, F-411). Read as a magnitude only: the second rAF fires 678.0 ms after navigation start, against an `s_shell` of 885.3 ms from window creation, so roughly 200 ms of `s_shell` is not on the webview's own timeline (time between window creation and navigation start, or mark delivery over IPC). This file cannot say which. The frame-to-paint gap is negative in all 20 rows (median -61.0, range -90.0 to -30.0): `paint_evidence` precedes the second rAF, which is the ordering `trace.rs` records (F-411, honest magnitude about 51 to 65 ms), and the evidence is present in every row. It is a diagnostic with no assertion, so this is consistent with the paint mark not being hoisted, not proof of it.

### What the P2 fix task should target (a recommendation; scope is the architect's)

From the term definitions, the first measurement is a split of `s_shell` (885 ms) and then of `s_settle` (228 ms), because neither is yet divided finely enough to name a single cause:

1. **Split `s_shell` before changing anything.** ADR-0014's own consultation (vue-frontend-dev) already asked whether `S_shell` needs a mark separating the webview's navigation from our mount; on this machine that question is now worth about 200 ms by the figures above. Candidate marks: navigation start, `boot.js`/entry-module fetch start and end, script evaluation end, Vue `app.mount` return, first contentful paint. The existing `paint_evidence` is the last of these; the first four do not exist.
2. **Bundle size, parse and execute on webkit2gtk.** The shell includes the whole front-end bundle's load and evaluation, which at P1 includes CodeMirror 6 (the earlier Windows measurement after it was added is in `nfr-prf-01-cold-start-post-codemirror.md`; whether it sits in the entry chunk was not checked here). The candidates are route-level or dynamic-import splitting so the first paint needs only the shell, and a cheaper evaluate path (no eager editor construction before first paint). The Windows figure for the same bundle is about 200 ms median at P0, so the multiplier is a webkit2gtk and machine effect to be explained, not assumed.
3. **Does the first paint wait on the vault boot?** If mounting waits for the boot-info IPC or the vault open before rendering anything, `s_shell` contains work that belongs after the paint. The shell should paint from static content and let the boot info arrive after, which moves time from `s_shell` into `s_settle` and does not change `vault_usable` (ADR-0014 section Consequences, the redistribution caveat F-386): so the fix is judged on `vault_usable` and the Serat term, never on `s_shell` alone.
4. **The settle definition and the tree.** `s_settle` of 228 ms is entirely the tree's first screen. Two things to check: whether the root listing request is issued only after the paint (it could be issued at boot, in parallel with the bundle), and whether the 20k-vault root listing's size, serialisation or first-screen render dominates. The `tree_first_screen` definition (ADR-0015 section 5) is not a candidate for change: weakening a precondition to meet a budget is what ADR-0014 forbids.
5. **Acceptance:** the Serat term p95 on gaws-dev-05 as reported (not gating, F-96), with the per-mark trace re-run and the same table committed, plus the Windows figure on RM-1 to confirm nothing regressed there.

### The slow iteration

Run 0 is the one outlier: `serat_term` 1,669.8 ms (the maximum), `p_webview` 775.3 ms (2.7 times the 287.9 median), `s_shell` 1,469.7 ms and `window_visible` 2,434.6 ms. It is the first of the 20, which fits a cold page cache or first webview profile creation, but this file cannot show that. The other 19 have `s_shell` between 771.8 and 958.2 ms. Because n=20 and nearest-rank p95 is the 19th value, run 0 (the maximum) is excluded from every p95 above, and it did not move any median materially; no figure was recomputed without it.

### Environment and status of the figure

- **Quietness is unattested.** No `environment` before or after was committed with this trace, as for RM-2's 2026-11-24 files, so concurrent load, power state and the webkit2gtk version are unrecorded. Treat the figures as a reading from a desktop that was probably idle, not as an attested quiet run.
- **Not a gate figure.** It is Linux/webkit2gtk. PRD v1.2 bounds the 350 ms to Windows/WebView2 (F-96) and PRD v1.15 (#212) records the Linux figure as reported, not gating, attribution owed. This section is that attribution. It does not re-baseline anything and says nothing about Windows.
- The run is not asserted by the harness: `prf01-cold-start-terms` asserts nothing and exits 0 on a clean run (its own module doc).

## What #167 still needs

Task #167 (Windows re-runs of the report-only jobs) is not closed by this run. It still needs: a Windows machine with real-time protection on (RM-1 or a hosted runner with the scanner on) running NFR-PRF-04, FR-VLT-04, NFR-PRF-01 and NFR-PRF-03 with `--profile reference` on the gate head, the `environment` stamp before and after, and the Windows scanner-on figure for each. The gate report's "Gate figure (RM-1, scanner on)" column stays Owed for every row.

## Owed by RM-2 itself

1. `environment` before and after, so that RM-2's model, CPU, RAM, storage, kernel, toolchain, webkit2gtk version, and concurrent load are recorded rather than owed.
2. `p1-external-change` (FR-VLT-04) on RM-2.
3. `p1-save-tail` on RM-2, for an attested NFR-PRF-03 figure.
4. A longer prf04 series (n of at least 5, with the first run reported separately) if RM-2 is to be quoted for NFR-PRF-04 beyond "inside 30 s".
5. The section 3.4 scale assertion, which is absent from the committed results. (The per-mark NFR-PRF-01 trace that was owed here arrived 2026-11-26, #218; see "Per-mark attribution, 2026-11-26". Its `environment` before and after is still owed, under item 1.)

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01 | Records the first webkit2gtk Serat-term figure, as an asserted FAIL on Linux, bounded by PRD v1.2 and F-96; attributed on 2026-11-26 (#218) to the front-end shell, `s_shell` 885.3 ms median of a 1,130.8 ms term; RISK-016 holds the decision |
| NFR-PRF-03 | Records p95 20.247 ms of 50 on a Linux desktop, 1,000 distinct notes (stratified) |
| NFR-PRF-04 | Records median 9,152.9 ms of 30,000 on a Linux desktop, with phases |
| NFR-PRF-08 | The five jobs ran under `--profile reference` and asserted where the harness asserts; none is a CI run |
| US-35 | Supporting evidence on a second machine for "stay fast as the vault grows" |
| C-06 | The asserted figures are on a named machine (RM-2) with the vault digest; the gate figure stays RM-1's (ADR-0019 §8) |
