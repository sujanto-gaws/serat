---
project: Serat
artifact: T2 — the four P0 gate benchmarks, reported together (task #101)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, US-35
cites: ADR-0001, ADR-0011, ADR-0013, ADR-0014, F-384, F-397, F-403, F-405, F-14, F-33, F-34, F-382, F-392, F-402, F-417, F-418, F-442, F-450, F-458, F-466, F-479, F-464, F-470, F-481, F-482, F-504
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

**One more bound, found after this figure was taken and not yet resolved (F-417, code-reviewer's #102 sweep,
reasoned rather than measured).** `S_shell` (196/229 ms) — the term ADR-0014 budgets and #104 set 350 ms against — may
have been measured against a shell that never applied CodeMirror's own stylesheet: `style-src 'self'` has no nonce,
and `style-mod` (CodeMirror's runtime styling) creates an inline `<style>` element, which that CSP refuses. If
confirmed, `S_shell` did not pay for work a styled editor would actually do. Task #116 verifies before fixing; this
report's `S_shell`/`S`/`S_front` figures should be read as provisional until it closes.

## NFR-PRF-03 — save → index < 50 ms p95 — **PASS at 4.306 ms, `gate_eligible: false`**

**Measured.** Task #95, fourth attempt, 2026-09-09, under the declared freeze against tree `c60dfdd` (`main` +
#119 `690b70c` + #94). Full report: `docs/05-testing/reports/nfr-prf-03-save-to-index.md`.

| Field | Value |
|---|---|
| p95 | **4.306 ms** over 1,000 saves · limit 50.0 ms (reference profile) · `degraded: null` |
| Replication | a second 1,000-save run immediately after gave **4.4 ms**; both are reported so that designating the first cannot be read as choosing the better |
| Distribution | min 1.481 · mean 3.002 · max 8.218 ms |
| Save-path census | **1000 attempted · 1000 submitted · 0 writer-refused · 0 handle-missing** — every attempted save reached the index writer, so the p95 is over the whole workload rather than over whichever fraction survived |
| Vault | canonical 20k, digest `35fcb867…bd393e`, verified by the job before measuring and intact after |
| Verdict | **PASS on the budget** — 11.6x of headroom, so F-327's 35% session-to-session movement cannot reach the verdict here the way it reached NFR-PRF-04's |
| **Binding caveat** | **`gate_eligible: false`** — real-time scanning is disabled on this machine (RISK-012), which the harness itself refuses to call the charter number. A scanner-on re-run is what makes this the gate figure; the headroom makes that likely to hold and *likely* is not *measured*. |
| Instrument | **Mutation-attested after the fact and passed** — task #122 against the composed tree `c60dfdd` rather than the branch tips: control 95/0, ten of eleven red, identical to `d30aa34`. The number was taken before that attestation existed and a later pass does not reorder those events, so the report keeps the bound as *discharged* rather than deleting it. The surviving eleventh is the known pre-warm SHOULD-FIX, and it is a SHOULD-FIX **only while `handle_missing` stays a hard zero**. |
| Charter re-run | Task **#123**, blocked on the repository owner's own machine setting. Enabling real-time protection is a security decision on their machine, **not a benchmark convenience**, and it is theirs to make. |

**The zero is not a measurement of F-382's window, and is not recorded as one.** On this tree a zero cannot
distinguish *the pre-warm worked* from *#119 closed the ordering window* from *both* — #119's own 35.4 µs / 191 µs
(F-462) stays the measurement of record. A residual would have been the more interesting result and would have gone
to #119's record; none appeared.

**This section previously carried five stale claims under a correct conclusion (F-466)** — *"the most dangerous form,
because the section reads as maintained."* They are corrected below with the anchor-plus-re-check convention F-442
asks for, and kept rather than deleted, because what the section used to say is why the number took four attempts.

| What this section used to say | What is true, and the one-line re-check |
|---|---|
| `save_to_index` *"calls `IndexHandle::apply()`/`flush()` directly against `serat-index` — it never enters `serat-app`"* | **False since #94 merged.** `subject::save_to_index_via_app` spawns the real compiled binary with `--bench-save-list`, so every sample is a real `AppState::write_file`: `write_atomic`, the re-read/hash, echo suppression and the handle slot all included. Re-check: `grep -n "bench-save-list" bench/budget/src/subject.rs` |
| Anchored at `bench/budget/src/subject.rs:517` | **The line number has moved and will move again.** Anchored by symbol from here on. Re-check: `grep -n "fn save_to_index_via_app" bench/budget/src/subject.rs` |
| Blocked on **#111**, *"pending"* | **#111 is MERGED.** `save_path_outcomes` is reachable by a budget job; F-402 and F-412 both closed with their mutations re-run red by code-reviewer. Re-check: the resume table's #111 row |
| Blocked on **F-402**, *"open"* — the ninth guard-that-cannot-fail | **Closed.** The outcome bucketing now distinguishes a refused save from a submitted one, and the p95 is computed over submitted saves alone. Re-check: `grep -n "writer_refused_ms" bench/budget/src/lib.rs` |
| The lead's third ruling set *"left this open: does NFR-PRF-03 mean the save interval measured however it can be reached, or Serat's own write path, end to end? Unresolved as of this writing."* | **Settled: Serat's own write path.** The requirement's words are *a note saved through Serat's own write path* in an open vault. That ruling is also what makes the pre-warm below legitimate rather than a slackening. |

**What the instrument now is, so the number arrives with its method attached rather than after it.**

| Property | State |
|---|---|
| Interval | `AppState::write_file`'s call into the index, timed from outside the process — Serat's own save path, no watcher, no debounce (architecture §3.1.1). |
| Setup | The vault is **pre-warmed** to a `Ready` index before the binary is spawned (`subject::prewarm_index`). NFR-PRF-03 governs a save in an *already-open* vault; racing a vault's first index build is a different question, and F-450 records that racing it was what produced the earlier 1000/1000. The pre-warm derives its root the same way `serat-app` does, because F-458 found that a differently-spelled path makes `open_or_rebuild` declare the index `VaultMoved` and rebuild — leaving the pre-warm **inert while looking like it worked**. |
| Conservation | `check_save_completeness` treats `handle_missing` as a **hard zero** and refuses the whole run if it is not, and separately requires `submitted + writer_refused` to account for every attempt — which also catches a fourth, unnamed way a save could vanish that neither counter was built to count. |
| Census, printed every run | `attempted · submitted · writer-refused · handle-missing`, **including when `handle_missing` is zero.** The zero is the point: a residual that only shows up as silence is indistinguishable from nobody having looked. |
| Degradation | ADR-0011 backpressure does not fail the run, it fails the **number**: a non-zero `writer_refused` sets `Measurement::degraded`, and a degraded outcome cannot report PASS however fast the saves that landed were. |
| What still bounds the figure | Reference machine only; warm vault by construction (above); `gate_eligible: false` whenever the real-time scanner is off (RISK-012). |

**Why it took four attempts, since that is the part worth carrying forward.** No instrument reached `serat-app`
(closed by #94); `save_path_outcomes` was unreachable by a budget job (closed by #111); and the harness was saving
into a cold vault's first rebuild (F-450) — closed by the pre-warm, **which was itself inert until F-458**, because
it passed a differently-spelled vault root and `open_or_rebuild` declared the index `VaultMoved` and rebuilt. Three
of the four failures were the instrument, not the code, and the third looked exactly like a product defect.

## NFR-PRF-08 — every budget has an asserting job; a breach fails CI — **cannot currently be satisfied at all**

This is a **Must** (C-06: "Performance budgets are binding and enforced in CI") and the honest state is not "unmeasured"
but **"unsatisfiable as things stand, for a reason unrelated to the code"**:

| Field | Value |
|---|---|
| Local budget jobs | All four exist and run locally (`bin/prf01-cold-start.rs`, `prf03-save-to-index.rs`, `prf04-full-rebuild.rs`, plus `bin/environment.rs`) and TC-023 (`docs/05-testing/reports/`) covers that every budget has a job. |
| CI enforcement | **`ci.yml` and `budgets.yml` cannot currently be scheduled at all (F-384).** Every run since commit `abc71409` (2026-09-06) has completed in 1-4 seconds with zero steps and no runner assigned, on both `push` and `pull_request`. The one job that does get a runner (`gate`, which has no toolchain to install) correctly reads five `needs.*.result` values that are all `failure` and exits 1 — the file parses and schedules; specific jobs within it do not get runners. Consistent with (not confirmed — the investigating token's scope returns 403 on the billing endpoint) exhausted included Actions minutes on this private repo, given `windows-latest` runners bill at 2x and this session pushed to `main` roughly every 3-8 minutes for over two hours. |
| What would close it | The repository owner (spending limit / payment method / minutes reset). **Nothing in `bench/` or `docs/05-testing/` can fix this** — recorded per F-384 rather than guessed past, because treating forty red checks as noise and continuing to push is the exact failure mode F-354 already named. |
| Verdict | **Not satisfiable by test-engineer's own work.** The jobs exist and are correct locally; the Must fails on infrastructure the code does not control. |

## C-01..C-07, per code-reviewer's #102 sweep against `main`

F-418 (code-reviewer, `df73b0d`): the sweep found **no BLOCKING breach**, and three of its results were not yet carried
anywhere this report's readers would find them. Stated here rather than left implicit, per that finding's own close.

| Constraint | Status | Basis |
|---|---|---|
| **C-01** files stay plain Markdown | Clean | `write_file` writes the buffer verbatim; nothing normalises line endings or adds a trailing newline. |
| **C-02** the index is disposable | Clean, positively evidenced | No longer only argued: #97's T1 deletes `.serat/` on real disk and asserts row-level equivalence through `open_or_rebuild`. Settings live in the platform config dir, not `.serat/`. |
| **C-03** no JavaScript in the query path | Vacuous, not satisfied | There is no `serat-sq` yet. A constraint with nothing to breach is not evidence the constraint holds once the crate exists. |
| **C-04** writes patch byte ranges | No breach, with a bound | Nothing re-serialises today because there is no parse→render round trip on the write path — P0 writes the whole buffer. The byte-range patcher itself is P2 scope. |
| **C-05** no AI write lands without diff review | Vacuous, not satisfied | There is no `serat-ai`/`serat-mcp` yet, same shape as C-03. |
| **C-06** performance budgets binding, enforced in CI | Not satisfied, on infrastructure | See the NFR-PRF-08 section above (F-384) — CI cannot schedule at all. `.github/workflows/budgets.yml`'s own header was stale (still said the jobs were "RED today, on purpose" because rebuild/save-path/the application did not exist — all three now do); corrected in this task alongside this report to say the real reason is F-384, not absent implementation. |
| **C-07** Tauri v2 + Vue 3 fixed | Now satisfied on both halves, **measured** | In a scratch worktree, `npm ci && npm run build` reproduces the committed `webview/` bundle **byte-identical** (`git diff` empty) — a dated observation (this session), not a standing property, and the local equivalent of #110's CI check, still blocked by F-384. Closes F-380's note that the Vue half was previously unverified. |

**F-379 carried as a trend to watch, not a breach.** *"No domain logic in `serat-app`"* has no mechanical guard, and two
tasks now add non-product code to that crate by design — #113's validation move and #111's `run_save_benchmark`/
`--bench-save-list`. Neither is a breach (instrumentation in the product is how NFR-PRF-01 is measurable at all), but
the trend has no guard and a clean sweep must not be read as an enforced one.

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
both cases only a manual run against the real WebView2 window caught what the suite could not. That observation is
unchanged and is the durable lesson; it is restated here because T2 is the document meant to carry it forward rather
than let it live only in the findings that produced it.

**This paragraph used to end "either defect is a closed finding rather than a live one", and F-504 falsified that
clause.** It is corrected rather than deleted, because the line is narrow in *both* directions and a reader who takes
away either "it was fine" or "the guard is broken" would have the wrong one.

| Claim | Status after #121's run against a real WebView2 window (F-504) |
|---|---|
| **Positive control, run first** | Stubbed `boot.js` → `cargo build --release` → **exit 2, no trace.** The pipeline demonstrably reaches the binary, so everything below is evidence rather than a stale-binary artefact (F-470). **Recorded as method, not as a result:** a mutation run without this control cannot tell a surviving mutation from a mutation that never arrived. |
| **Honest baseline** | exit 0, `paint_evidence_ms` 212.0, `raf2_ms` 163.2, raf2 preceding FCP by ~49 ms — inside F-406's measured 51–65 ms band. |
| **Mutation A** (single rAF, `raf2 == raf1`) | **Discharged.** RED, exit 2, no trace — **against the real guard**, which is what F-392 asked for and what F-464 found had never actually been done. |
| **Mutation B, canonical** (`fcp: 0`) | Rejected by `first_contentful_paint_ms <= 0.0`; the honest call then succeeds and the trace carries real numbers. **This is why the gap always looked covered.** |
| **Mutation B, sharp** (`fcp: 1, raf1: 0, raf2: 1`) | **SURVIVED — reproduced, not predicted.** exit 0, and the trace records `paint_evidence_ms: 1.0`, `raf2_ms: 1.0`: fabricated evidence accepted, written verbatim into the audit field, and the honest call silently no-opped by first-wins. F-479's pre-registered prediction is confirmed, so **F-481 and F-482 are empirical rather than predicted.** |

**What the surviving mutation does and does not establish, stated in both directions because the temptation runs both
ways.** It establishes that fabricated paint evidence is **accepted and audited** — the number in the report is the
number the fake supplied. It does **not** establish that any figure in this report was flattered: in that placement
`t0_to_window_visible_ms` came out **2687 ms against the honest 2458 ms — worse, not better**, because module
evaluation happens after window creation. **And nothing bounds it in our favour either**: the mark is stamped when
the call *arrives*, so a fake arriving earlier would move the number the other way. The honest summary is that the
mark is **unbounded in both directions**, not that it is safe.

Closing it is **task #125**, named rather than described so that F-471's mechanism can watch it: a fix summarised in
prose here would be a promise this document cannot keep, and the task number is the thing that can be checked as
open or closed without reading this paragraph at all.
## Verdict

**Not a clean GO, and the shape of the shortfall has changed: three of four budgets are now measured, and none of the
three that remain open is a code problem.**

- **NFR-PRF-04** — PASS, gate-quality (#96).
- **NFR-PRF-03** — **PASS at 4.306 ms against 50 ms**, 1,000 saves, every attempt accounted for (#95). `gate_eligible:
  false` on the scanner, so it is a comparable figure and not yet the charter one.
- **NFR-PRF-01** — fails its current asserted scalars, and the seven-term breakdown shows the failure is a platform
  term this project does not control rather than Serat code; ADR-0014's split, now `accepted`, budgets the term that
  is ours and comfortably clears it.
- **NFR-PRF-08** — a **Must** that cannot currently be satisfied at all, for a reason outside every team member's
  code (F-384, needs the repository owner).

**Two of the three open items are the same item wearing different clothes: the machine.** NFR-PRF-03's
`gate_eligible: false` and NFR-PRF-01's platform term are both "the number this project controls is fine and the
environment it is measured in is not the one the charter judges", and both are answered by re-measuring rather than
by changing code. NFR-PRF-08 is answered by nobody on this team.

**This document does not decide whether P0 proceeds** — that is #102's (code-reviewer's final sweep) and the owner's
call — **it states what is known, what is blocked, and why**, so that decision is made against evidence rather than a
placeholder.

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01 | Final reading, seven-term breakdown, F-397/F-403/F-405 caveats, ADR-0014 acceptance. |
| NFR-PRF-03 | Measured: 4.306 ms p95 over 1,000 saves (#95), with the census, the `gate_eligible: false` caveat, and an explicit refusal to read the zero `handle_missing` as F-382's window. Full report in `reports/nfr-prf-03-save-to-index.md`. |
| NFR-PRF-04 | Carries #96's gate-quality PASS forward without re-deriving it. |
| NFR-PRF-08 | States the Must is unsatisfiable on infrastructure grounds (F-384), distinct from unmeasured. |
| US-35 | "Stays fast as the vault grows" — all four numbers this project can currently produce, in one place. |
