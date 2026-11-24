---
project: Serat
artifact: Linux desktop reference run, gaws-dev-05, 2026-11-24 (task #177)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, US-35, C-06
cites: ADR-0019, ADR-0014, RISK-012, RISK-013, RISK-016, F-96
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

**What this is, plainly.** PRD v1.2 re-baselined the 350 ms as a split, and bounded that re-baseline to Windows/WebView2 (F-96): "WKWebView and WebKitGTK are unmeasured. If their platform terms are of the same order, the requirement was wrong rather than the design". This run is the first figure from webkit2gtk. It is a Linux finding, not yet attributed, produced on a machine the harness rates gate eligible, so it is an asserted FAIL there. It is not a verdict on the Windows budget, whose gate figure (RM-1, scanner on) is owed and whose last reading was 233.8 ms p95 on WebView2 at P0.

**What the file shows and what it does not.**
- The result is far outside the budget, not marginal: the smallest of the 20 samples (1,046.6 ms) is about three times the limit.
- Within this machine, the platform term (503.1 ms p95) is well below the Serat term (1,310.4 ms p95), so the failure is not explained by webview creation. The Serat term is Serat's own four marks (`S_pre`, `S_setup`, `S_shell`, `S_settle`), so the PRD's same-order-platform-terms clause does not excuse it. No other machine's figure is used for this.
- The file carries the sum of the Serat term only, not its four marks (`S_pre`, `S_setup`, `S_shell`, `S_settle`), so it cannot say which mark carries the 1.3 s. That is the next question and needs the per-mark trace the job already collects.
- The build profile is recorded by construction: the harness refuses a startup trace whose profile is not `release` (`trace.rs`; the harness drives `target/release/serat`; TC-045), so a debug build is ruled out. What blocks attribution is that the JSON carries the Serat-term sum and not its marks. The bin that prints `S_pre`, `S_setup`, `S_shell` and `S_settle` per trace is `prf01-cold-start-terms`; running it on RM-2 is the next step.
- tree-first-screen and vault-usable are identical (1,796.493 ms), and that is explained: `tree_first_screen` is an independent mark, and `vault_usable` is the latest of paint, tree first screen, index settled and watcher (`trace.rs`). Equal p95s mean the tree's first screen was the last mark in the samples that set them. The same equality appears in the harness's own fixture.

**Recommendation to the owner (a decision, not a re-baseline).** Does NFR-PRF-01 bind Linux in P1? The PRD's scope says no: the 350 ms is bounded to Windows/WebView2, and the "one platform" bound states that other webviews are unmeasured and that, if their platform terms are of the same order, "the requirement was wrong rather than the design". The PRD does not yet say what P1 does when a measured non-Windows figure fails. That is a decision with a risk attached, so it is recorded as **RISK-016** in `docs/01-planning/risk-register.md`, owned by the owner pending the decision. If the owner wants the requirement stated per platform, that is a product-analyst task, and this report proposes it, to be filed by the lead: **"[P1/spec] State whether NFR-PRF-01 binds Linux/webkit2gtk or only Windows/WebView2; do not re-baseline without a per-mark trace (serves NFR-PRF-01, F-96, ADR-0014)"**. Nothing here re-baselines anything: no target in the PRD, the harness or CI changes with this report.

**Do not conclude** that the Windows budget is wrong, or that the Windows Serat term has regressed. Nothing in this run measures Windows.

## What #167 still needs

Task #167 (Windows re-runs of the report-only jobs) is not closed by this run. It still needs: a Windows machine with real-time protection on (RM-1 or a hosted runner with the scanner on) running NFR-PRF-04, FR-VLT-04, NFR-PRF-01 and NFR-PRF-03 with `--profile reference` on the gate head, the `environment` stamp before and after, and the Windows scanner-on figure for each. The gate report's "Gate figure (RM-1, scanner on)" column stays Owed for every row.

## Owed by RM-2 itself

1. `environment` before and after, so that RM-2's model, CPU, RAM, storage, kernel, toolchain, webkit2gtk version, and concurrent load are recorded rather than owed.
2. `p1-external-change` (FR-VLT-04) on RM-2.
3. `p1-save-tail` on RM-2, for an attested NFR-PRF-03 figure.
4. A longer prf04 series (n of at least 5, with the first run reported separately) if RM-2 is to be quoted for NFR-PRF-04 beyond "inside 30 s".
5. A per-mark NFR-PRF-01 trace (`prf01-cold-start-terms`) to attribute the Serat term; and the section 3.4 scale assertion, which is absent from the committed results.

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01 | Records the first webkit2gtk Serat-term figure, as an asserted FAIL on Linux, not yet attributed, bounded by PRD v1.2 and F-96; RISK-016 holds the decision |
| NFR-PRF-03 | Records p95 20.247 ms of 50 on a Linux desktop, 1,000 distinct notes (stratified) |
| NFR-PRF-04 | Records median 9,152.9 ms of 30,000 on a Linux desktop, with phases |
| NFR-PRF-08 | The five jobs ran under `--profile reference` and asserted where the harness asserts; none is a CI run |
| US-35 | Supporting evidence on a second machine for "stay fast as the vault grows" |
| C-06 | The asserted figures are on a named machine (RM-2) with the vault digest; the gate figure stays RM-1's (ADR-0019 §8) |
