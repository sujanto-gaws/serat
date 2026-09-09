---
project: Serat
artifact: NFR-PRF-03 measurement, save → index at p95 (task #95)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-03, NFR-PRF-08, US-35
cites: ADR-0001, ADR-0011, F-327, F-382, F-435, F-448, F-450, F-458, F-462, F-484, RISK-012
---

# Save → index, measured (task #95)

**Status:** measured · **PASS on the budget, `gate_eligible: false` on the machine** · **instrument
mutation-attested after the fact by #122, passed** · **Run by:** test-engineer, 2026-09-09
**Tree:** `c60dfdd` — `main` with #119 merged (`690b70c`) and #94 merged. `target/release/serat.exe` and
`target/release/prf03-save-to-index.exe`, release profile, both built from that tree in one cargo process.
**Vault:** canonical 20k fixture, digest `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e`,
verified against `bench/vaults/vault-20k.manifest.json` by the job itself before measuring, and intact afterwards
(no `.serat/` left behind — F-372/F-401's guard held).

## The number

| Field | Run 1 — **the number of record** | Run 2 — replication |
|---|---|---|
| p95 | **4.306 ms** | 4.4 ms |
| Limit | 50.0 ms (reference profile) | 50.0 ms |
| Verdict | **PASS**, `degraded: null` | PASS |
| n | 1,000 | 1,000 |
| min / mean / max | 1.481 / 3.002 / 8.218 ms | 2.6 / 3.6 / 20.0 ms |
| Save-path census | **1000 attempted · 1000 submitted · 0 writer-refused · 0 handle-missing** | identical |
| `gate_eligible` | **false** | false |

`bench/out/budgets/prf03-save-to-index.json` carries run 2; run 1's JSON is the one quoted above, field by field.

**Run 1 is designated the number of record because it was the pre-registered run** — the banner said *"1,000 saves,
`--profile reference`, canonical 20k vault, pre-warmed"*, and that is run 1. Run 2 was taken immediately afterwards
as **replication, not selection**, and both are reported here precisely so that designating one cannot be mistaken
for choosing the better. They differ by 0.1 ms.

**Why one run would have been enough, and why two are better anyway.** F-327 records 35% session-to-session movement
on unchanged code, which is what made NFR-PRF-04's 19.0 s reading unusable against a 30 s limit. That argument does
not reach here: **4.3 ms against 50 ms is 11.6x of headroom**, so a 35% movement — or a 300% one — cannot change the
verdict. The replication is reported because a single reading is what this project has repeatedly regretted, not
because the margin needed it.

## What the zero means, and what it does not

The census reports `handle_missing: 0` on both runs, and **this report does not claim that as a measurement of
F-382's window.**

The lead's declaration is explicit and it is right: on this tree a zero cannot distinguish *the pre-warm worked* from
*#119 closed the ordering window* from *both at once*. #119 moved the terminal publish to after `into_handle()` fills
the slot, and #94 pre-warms the index to `Ready` before the binary is spawned. Either alone would plausibly produce
this zero. **F-382's window remains measured by #119's own 35.4 µs / 191 µs (F-462), and nothing here supersedes
that.**

What the zero *does* establish is narrower and still worth having: **every one of 1,000 attempted saves reached the
index writer**, so the p95 above is computed over the whole workload rather than over whichever fraction survived.
That is the property the conservation rule exists to guarantee, and it is now observed rather than assumed.

**A residual would have been the more interesting result.** Had a non-zero survived both the pre-warm and #119's
ordering fix, it would have been a third cause nobody has named, and the banner's instruction was that it must not
be filed under F-382 without a fresh diagnosis. None appeared, so nothing goes to #119's record from this run.

## What the number is a number *of*

| Property | State |
|---|---|
| Interval | `AppState::write_file`'s call into the index, timed from outside the process — Serat's own save path, no watcher, no debounce (architecture §3.1.1). Every sample is a real `write_atomic`, re-read/hash, echo suppression and handle-slot submission through the compiled binary, driven by `--bench-save-list`. |
| Setup | The vault is pre-warmed to a `Ready` index before the binary is spawned. NFR-PRF-03's words are *a note saved through Serat's own write path* in an **already-open** vault; racing a vault's first index build is a different question, and F-450 records that racing it was what produced the earlier 1000/1000. |
| Conservation | `handle_missing` is a hard zero — a non-zero refuses the whole run and produces no number at all. `submitted + writer_refused` must account for every attempt, which also catches a fourth, unnamed way a save could vanish. |
| Degradation | ADR-0011 backpressure fails the **number**, not the run: a non-zero `writer_refused` sets `degraded`, and `degraded` gates `passed()` (F-435, mutation-verified). It was zero here, so nothing was excluded from the series. |
| Short runs | A short series cannot post a gate number: `indicative` travels on `Outcome` (F-448) and folds into `degraded`. Both runs were the full 1,000. |

## Bounds on the figure — read these before quoting it

1. **`gate_eligible: false`.** Real-time virus scanning is disabled on this machine (RISK-012), so the harness's own
   environment gate refuses to call this the charter number: *"a best case no user will see."* **This is the binding
   caveat on the reading** and it is the same one NFR-PRF-04's gate-quality assessment carried. A scanner-on re-run
   is what turns 4.3 ms into the charter's figure; with 11.6x of headroom, the budget is unlikely to be at risk, but
   *unlikely* is not *measured* and this report does not claim it.
2. **Warm vault, by construction.** The pre-warm is the requirement's own premise, not a convenience — but it means
   this figure says nothing about a save made while a first index build is running. That case is design-correct
   behaviour (F-450) and is not what NFR-PRF-03 governs.
3. **Reference machine only.** No CI ceiling exists for this budget (`ci_limit_ms: None`), so a CI run today would
   assert the product limit on a slower runner. That is deliberate: a placeholder ceiling invented at a desk is worse
   than an honest red.
4. **One platform.** Windows, 16 cores, Energy Star power plan, `concurrent_build_processes: 0` under the declared
   freeze.
5. **The composed tree was not mutation-attested at measurement time — DISCHARGED by task #122, and kept rather than
   deleted.** At the time this figure was taken, code-reviewer had verified `690b70c` byte-identical to the approved
   `8460e32` and verified #94 would merge cleanly, but had not mutated `c60dfdd` itself; F-429 was satisfied on the
   merges and *inferred* on the composition, and the bound said that if the attestation failed, this number was void.
   **It passed** — #122, against `c60dfdd` rather than the branch tips: control 95/0, ten of eleven red, identical to
   `d30aa34`, D1/D2 red. **4.306 ms is not void.**

   The bound stays on this list because **it remains true of the figure**: this number was taken before its instrument
   was attested, and a later pass does not reorder those two events. Deleting it would drop a true bound and leave a
   reader unable to tell that the sequence was ever a risk.

   **The eleventh mutation survived and that is a known, already-recorded SHOULD-FIX, not a new hole.** It is the
   pre-warm survivor, and its status is conditional in a way worth repeating where the number lives: it is a
   SHOULD-FIX **only while `handle_missing` stays a hard zero.** Soften that check and it becomes blocking the same
   day, because the conservation rule is what makes the survivor harmless.

## Verdict

**NFR-PRF-03 PASS at 4.306 ms p95 against a 50 ms budget, over 1,000 saves through the real application, with every
attempted save accounted for — and `gate_eligible: false` until it is re-run with the scanner enabled.**

The fourth attempt is the first to produce a number. The three that did not were: no instrument reaching `serat-app`
(closed by #94), `save_path_outcomes` unreachable by a budget job (closed by #111), and a harness saving into a cold
vault's first rebuild (F-450, closed by the pre-warm — which was itself inert until F-458).
