---
project: Serat
artifact: T2 — the four P0 gate benchmarks, reported together (task #101)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, US-35
cites: ADR-0001, ADR-0013, ADR-0014, F-384, F-397, F-403, F-405, F-14, F-33, F-34, F-392, F-402
---

# T2 — the four P0 gate benchmarks, reported together

**Status:** two numbers final, two unmeasured for reasons stated below · **Run by:** test-engineer
**This document started before all four inputs existed** (lead's instruction, #101's own history): NFR-PRF-04 and
NFR-PRF-01 are final and will not move; NFR-PRF-03 and NFR-PRF-08 are drafted with exactly what would fill them,
rather than estimated. TC-020..TC-023 (`docs/05-testing/test-plan.md`) are the test cases this document closes out.

## Read the terms before the verdict

NFR-PRF-01 below fails its asserted scalars. Reported first, because it is the fact that makes the failure legible
rather than a verdict that reads as "the implementation is slow," which the measurement underneath it does not say:

**The binding Serat term (`S`, ADR-0014 §Decision 3) is 201.1 ms median / 233.8 ms p95 — inside the current 400 ms
budget with 96 ms of room even against ADR-0014's own recommended tighter split (see NFR-PRF-01 below).** The
platform term (`P`, unbudgeted, WebView2-specific) is 1116.7 ms median / 1687.1 ms p95 and is what drags the
aggregate scalars over their limits. Serat's own contribution to cold start is not what is failing; a webview
constructor this project does not control is.

## NFR-PRF-04 — full rebuild, 20,000 notes < 30 s — **PASS, gate-quality, final**

| Field | Value |
|---|---|
| Reading (median of 3, ms) | 27,693.5 |
| Samples (ms) | 26,858.4 / 27,693.5 / 27,713.7 |
| Limit (ms) | 30,000.0 |
| `concurrent_build_processes` | 0 |
| `spread_pct()` (n=3) | 3.2% |
| `determinate()` | true |
| Verdict | **PASS, gate-quality** |
| `gate_eligible` | false — real-time scanner off (RISK-012); best-case/comparable figure |
| Vault digest | `35fcb867…` — matches the canonical 20k manifest |

Full derivation: `docs/05-testing/nfr-prf-04-gate-quality.md` (task #96). Not repeated here beyond the table; that
document is the source and this one only carries the number forward. This is the only one of the four budgets with
an attributable noise floor (build contention, ruled out by attestation) — see the NFR-PRF-01 note on F-405 below for
why that same explanation does not transfer to the other three.

## NFR-PRF-01 — window visible < 400 ms, vault usable < 1 s, p95 over 20 runs — **FAIL / INDETERMINATE, final**

Full derivation: `docs/05-testing/reports/nfr-prf-01-cold-start-post-codemirror.md` (task #100). Summary:

| Budget | Statistic | Value (ms) | Limit (ms) | Verdict |
|---|---|---:|---:|---|
| `window_visible` | p95, n=20 | 1575.6 | 400.0 | **FAIL** |
| `vault_usable` | p95, n=20 | 1577.2 | 1000.0 | **INDETERMINATE** (samples straddle the limit) |

**ADR-0014's seven-term partition** (batch 2 of #100's session, n=20, median/p95 ms):

| Term | Median | p95 |
|---|---:|---:|
| `P_load` (platform) | 317.8 | 657.3 |
| `S_pre` (Serat) | 3.1 | 3.8 |
| `P_builder` (platform) | 13.3 | 16.9 |
| `S_setup` (Serat) | 0.3 | 0.7 |
| `P_webview` (platform) | 725.3 | 1199.1 |
| `S_shell` (Serat) | 196.1 | 229.2 |
| `S_settle` (Serat) | 1.1 | 1.7 |
| **`S` = S_pre+S_setup+S_shell+S_settle (binding)** | **201.1** | **233.8** |
| `P` = P_load+P_builder+P_webview (unbudgeted) | 1116.7 | 1687.1 |

**ADR-0014 moved to `accepted` on these figures** (§Decision 7). `S`'s p95 (233.8 ms) clears even the ADR's own
recommended tighter split — floor `s95 × 1.3` = 303.9 ms, ceiling 400 ms, interval [304, 400] non-empty — with 96 ms
of room. §Decision 7 recommends **350 ms** to product-analyst for #104's new `S` budget; the number is theirs to set,
the interval is the ADR's.

**F-405 — the batch-to-batch spread, stated rather than smoothed over.** Three `n=20` batches, same binary, same
vault, same sitting, `concurrent_build_processes: 0` throughout, gave `window_visible` p95 of **1643.0 / 1876.5 /
1575.6 ms** — ~19% batch-to-batch, on top of ~97% within-batch spread. **#96's finding that NFR-PRF-04's historical
35% drift was contention, resolved to 3.2% under attestation, does not transfer here.** This machine was attested
quiet for every #100 batch and the cold-start figure still moved ~19%. The cause is unidentified and recorded as
open (F-405). **Do not read NFR-PRF-04's clean number as evidence this harness is generally stable — the two budgets
have different noise characters, and only one of them is currently understood.**

**Two caveats on the mark itself, both load-bearing for how much this number can be trusted:**

- **F-397 — the mark was unguarded when this was measured.** `mark_first_paint`'s call site
  (`crates/serat-app/src/commands.rs:104`) has no test pinning that it is passed real evidence rather than a
  placeholder; #113 (that test) is not on `main`. Every figure above is a real measurement of an unguarded mark.
- **F-403 — the conjunction is enforced nowhere.** `raf1_ms`/`raf2_ms` reach no Rust parameter; only
  `paint_evidence_ms` (the browser's `first-contentful-paint` entry) crosses the IPC boundary, and
  `budget::trace::StartupTrace::validate` does not consume it either — it is parsed and reported (added this
  session) but not required or checked. Sound today only because `app/index.html` is contentless before Vue mounts,
  which nothing asserts.

**Standing bounds, unchanged from #100's report:** warm vault (F-154, page cache cannot be dropped on this machine);
`gate_eligible: false` (real-time scanner off, RISK-012).

## NFR-PRF-03 — save → index < 50 ms p95 — **unmeasured, blank by design**

Not estimated, per the lead's instruction. What would fill this row:

| Field | What is needed |
|---|---|
| Instrument | `bench/budget/src/bin/prf03-save-to-index.rs` exists and is wired to `IndexHandle::apply()`/`flush()` (TC-021), but `bench/budget/src/subject.rs:517`'s `save_to_index` calls those directly against `serat-index` — it never enters `serat-app`, so it never exercises `AppState::write_file`, `write_atomic`, the re-read/hash, echo suppression or the handle slot. The lead's third ruling set left this open: does NFR-PRF-03 mean "the save interval measured however it can be reached" or "Serat's own write path, end to end"? Unresolved as of this writing. |
| Blocker | **#111** (`tauri-app-dev-2`, pending) — makes `save_path_outcomes` reachable by a budget job. #94 (test-engineer, this task's own #101 sibling) cannot assert the real save path without it: *"#94 has nothing to assert against without it"* is the resume table's own words. #94 → #95 (take and record the number) → this row. |
| A second, independent reason nothing here can pass yet | **F-402** — the ninth guard-that-cannot-fail on record (see below). `run_save_benchmark`'s outcome bucketing does not distinguish a refused save from a submitted one in the series NFR-PRF-03's p95 is asserted against; replacing the bucketing with an unconditional push gives **94 passed, 0 failed — identical to control.** A number taken through the current instrument, even once #111 unblocks it, would not yet be trustworthy until F-402 is fixed. |
| Verdict | **Cannot be reported.** Blocked on #111 (implementation) and F-402 (instrument correctness), both open and both outside this task's scope to close. |

## NFR-PRF-08 — every budget has an asserting job; a breach fails CI — **cannot currently be satisfied at all**

This is a **Must** (C-06: "Performance budgets are binding and enforced in CI") and the honest state is not "unmeasured"
but **"unsatisfiable as things stand, for a reason unrelated to the code"**:

| Field | Value |
|---|---|
| Local budget jobs | All four exist and run locally (`bin/prf01-cold-start.rs`, `prf03-save-to-index.rs`, `prf04-full-rebuild.rs`, plus `bin/environment.rs`) and TC-023 (`docs/05-testing/reports/`) covers that every budget has a job. |
| CI enforcement | **`ci.yml` and `budgets.yml` cannot currently be scheduled at all (F-384).** Every run since commit `abc71409` (2026-09-06) has completed in 1-4 seconds with zero steps and no runner assigned, on both `push` and `pull_request`. The one job that does get a runner (`gate`, which has no toolchain to install) correctly reads five `needs.*.result` values that are all `failure` and exits 1 — the file parses and schedules; specific jobs within it do not get runners. Consistent with (not confirmed — the investigating token's scope returns 403 on the billing endpoint) exhausted included Actions minutes on this private repo, given `windows-latest` runners bill at 2x and this session pushed to `main` roughly every 3-8 minutes for over two hours. |
| What would close it | The repository owner (spending limit / payment method / minutes reset). **Nothing in `bench/` or `docs/05-testing/` can fix this** — recorded per F-384 rather than guessed past, because treating forty red checks as noise and continuing to push is the exact failure mode F-354 already named. |
| Verdict | **Not satisfiable by test-engineer's own work.** The jobs exist and are correct locally; the Must fails on infrastructure the code does not control. |

## Findings carried, not adopted, into P0 scope

Open against `rust-core-dev` from before this wave, listed so this report does not read as though P0 left nothing
open:

- **F-14** — Threat T-06 (binary-write guard) has no test asserting it, though the guard itself is correct on `main`.
- **F-33** — `write_atomic` uses default file-create permissions, silently widening a chmod'd file's permissions on
  replace.
- **F-34** — `write_atomic` does not `fsync` the parent directory; a rename may not survive a power cut on POSIX.

None of the three is a P0 acceptance-criterion breach (FR-VLT-03's stated criterion — wholly old or wholly new — still
holds in every case); each is the gap between that criterion and what a careful user would additionally assume.

## Two lessons from this wave, both cross-cutting rather than specific to one budget

**Nine guards-that-cannot-fail are on record this wave, and the last three form one chain rather than nine unrelated
slips.** F-392 was found inside the fix for F-377; F-397 was found inside #105, which was itself F-392's fix; F-402
was found inside #111, the task written to close a different finding (F-382) by the same author who found F-402.
**Each fix's own review is where the next unfailable guard was hiding**, not a fresh area of the code — a repeatable
blind spot in how a fix for one guard is checked, not nine independent lapses.

**A green unit suite is not evidence a mechanism works.** Twenty-seven tests stayed green through two separate rounds
of the paint-mark guard being broken against the real application (F-377's original mutation, then F-392's), and in
both cases only a manual run against the real WebView2 window caught what the suite could not. This project's own
recorded discipline — mutate a guard before trusting it — held here exactly where it mattered and is the reason
either defect is a closed finding rather than a live one; it is restated here because T2 is the document meant to
carry it forward rather than let it live only in the two findings that produced it.

## Verdict

**Not a clean GO.** Two of four budgets are final and one of those two (NFR-PRF-01) fails its current asserted
scalars, though the seven-term breakdown shows the failure is a platform-term problem this project does not control
rather than a Serat-code problem, and ADR-0014's split — now `accepted` — gives product-analyst a budget on the term
that is ours to control and comfortably clears it. The other two budgets are not measured: NFR-PRF-03 is blocked on
work outside this task (#111) and an instrument defect found and recorded but not yet fixed (F-402); NFR-PRF-08, a
Must, cannot currently be satisfied at all for a reason outside any team member's code (F-384, needs the repository
owner). **This document does not decide whether P0 proceeds — that is #102's (code-reviewer's final sweep) and the
owner's call — it states what is known, what is blocked, and why, so that decision is made against evidence rather
than a placeholder.**

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01 | Final reading, seven-term breakdown, F-397/F-403/F-405 caveats, ADR-0014 acceptance. |
| NFR-PRF-03 | States precisely what blocks it (#111, F-402) rather than estimating. |
| NFR-PRF-04 | Carries #96's gate-quality PASS forward without re-deriving it. |
| NFR-PRF-08 | States the Must is unsatisfiable on infrastructure grounds (F-384), distinct from unmeasured. |
| US-35 | "Stays fast as the vault grows" — all four numbers this project can currently produce, in one place. |
