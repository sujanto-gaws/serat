---
project: Serat
artifact: NFR-PRF-01 re-measurement, real frontend mounted (task #100)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-01, US-35
cites: ADR-0014, F-397, F-403, F-154, RISK-012, F-327
---

# Cold start, re-measured with Vue 3 + CodeMirror 6 mounted (task #100)

**Status:** measured, both marks miss the current budget · **Run by:** test-engineer, 2026-09-08
**Vault:** canonical 20k fixture, digest `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e` (matches
`bench/vaults/vault-20k.manifest.json` on every run below — not F-372/F-401-poisoned) · **Binary:** `target/release/serat.exe`,
release profile, `.worktrees/test` at `5b36c31` (main `37b47ff` merged, plus this task's own commits)

## What changed since the last NFR-PRF-01 reading

The last recorded NFR-PRF-01 number (resume-list "Where the four gate numbers stand at resume") was taken against
`crates/serat-app/webview/` as a **static placeholder shell** — no Vue, no CodeMirror. #98/#108/#105/#112/#114 mounted
the real frontend and gave the two startup marks a guard that can fail (still incomplete — see Standing caveats below).
This is the first cold-start reading against the real editor.

## Procedure and environment

Followed `docs/05-testing/reference-machine.md`'s protocol and the lead's task brief exactly:

1. **Checked `concurrent_build_processes` before starting: 0.** Confirmed again after every batch (three checks total,
   all 0). No build/rustc/link process was ever observed running (`tasklist` cross-check, first check).
2. **Vault gate checked before and after every batch.** `bench/vaults/vault-20k/.serat/` was absent before the first
   batch, absent again after every subsequent batch, and `cargo test --release -p budget --test vault_gate` —
   `the_canonical_vault_passes_the_gate` — passed both before the session's first run and after its last. The F-401
   hazard the brief called out did not bite.
3. **Resolved a real duplication before measuring** (see "F-401 guard duplication" below) — the worktree's own
   uncommitted work and `origin/main`'s `abfb3d8` had independently written two different fixes for the same hazard.
   Neither was safe to run un-reconciled: one of them (`CleanIndexOnDrop`, on `main`, marked `UNVERIFIED` at commit
   time) deletes `.serat/` unconditionally on drop, which would destroy a legitimately pre-existing index. Resolved in
   favour of the guard that checks first. `cargo test --release -p budget`: 64 passed (lib) + 6 passed (`vault_gate.rs`),
   0 failed, before any cold-start run.
4. **Three batches of `n=20` reference-profile runs**, same binary, same vault, same sitting:
   - Batch 1 — a purpose-built instrument (`prf01-cold-start-terms`, added this session) that calls
     `subject::cold_start_traces` to keep every validated `StartupTrace` rather than reducing to the two gate scalars.
   - Batch 2 — the same instrument, extended to also report the two ADR-0014 headline totals (`S`, `P`) and dump raw
     per-run terms. This is the batch the seven-term table below is drawn from.
   - Batch 3 — the **official gate binary**, `prf01-cold-start --profile reference`, unmodified. This is the batch
     with the recorded PASS/FAIL verdict and the digest-stamped result files.

## Why a new instrument, and what it does and does not change

`prf01-cold-start` (the gate binary) reduces every run straight to `window_visible_ms`/`vault_usable_ms` — the two
scalars it asserts — and discards the other five ADR-0014 marks. The brief asked for all seven terms at median and
p95 plus `S_main`/`S_front`, which that reduction cannot answer after the fact. Added `subject::cold_start_traces`
(same guarded batch, same `--exit-after-ready` spawn, same six-rejection validation in `budget::trace` as
`cold_start`) and a new non-gate binary, `bin/prf01-cold-start-terms.rs`, that reads the full `Terms` breakdown from
each trace. **It asserts nothing and cannot fail a build** — `prf01-cold-start` remains the only budget job with a
verdict. Also added `StartupTrace::paint_evidence_ms: Option<f64>` (parsed, not required, no new rejection) so the
frontend's paint-evidence value could be reported rather than silently discarded — see the F-397/F-403 caveat below
for what this does and does not mean.

## ADR-0014's seven-term partition, median and p95 (batch 2, n=20)

| Term | Boundary | Median (ms) | p95 (ms) |
|---|---|---:|---:|
| `P_load` | t0 → main (platform: loader, DLLs) | 317.8 | 657.3 |
| `S_pre` | main → state_ready (Serat: args, settings, `AppState`) | 3.1 | 3.8 |
| `P_builder` | state_ready → runtime_ready (platform: `tauri::Builder`) | 13.3 | 16.9 |
| `S_setup` | runtime_ready → setup_done (Serat: event sink, `open_vault`, threads) | 0.3 | 0.7 |
| `P_webview` | setup_done → window_created (platform: `WebviewWindowBuilder::build`) | 725.3 | 1199.1 |
| `S_shell` | window_created → window_visible (Serat: our bundle → first composited frame) | 196.1 | 229.2 |
| `S_settle` | window_visible → vault_usable (Serat: remaining `t_usable` preconditions) | 1.1 | 1.7 |

**Sub-terms and headline totals (ADR-0014 Decisions 3–4):**

| Quantity | Median (ms) | p95 (ms) |
|---|---:|---:|
| `S_main` = `S_pre` + `S_setup` | 3.4 | 4.0 |
| `S_front` = `S_shell` + `S_settle` | 197.6 | 230.4 |
| **`S`** = `S_main` + `S_front` (binding Serat term, C-06) | **201.1** | **233.8** |
| `P` = `P_load` + `P_builder` + `P_webview` (platform, recorded, no budget) | 1116.7 | 1687.1 |

Batch 1's independent reading of the same seven terms, same vault, same binary, ~90 seconds earlier in the same
sitting, for comparison: `P_load` 320.2/740.1, `S_pre` 3.0/4.2, `P_builder` 12.5/17.1, `S_setup` 0.3/0.4, `P_webview`
685.5/930.9, `S_shell` 175.5/213.0, `S_settle` 1.1/1.5, `S_main` 3.4/4.6, `S_front` 176.4/213.9 (median/p95 ms). The two
readings agree closely on `S_main` and reasonably on `S_front`/`S_shell` (176–198 ms median, 213–230 ms p95); `P_load`
and especially `P_webview` move more between batches (658–1199 ms p95 on `P_webview` alone) — consistent with F-327's
recorded noise being a platform/OS-scheduling effect more than a Serat-code effect, and this is the first time that
distinction has been visible term-by-term rather than only in the two aggregate scalars.

**`S_front`'s spread is the input ADR-0014 was waiting on to leave `proposed`.** Both batches: 176–198 ms median,
213–230 ms p95 — a ~13% median-to-median spread and ~8% on p95, tight enough by this project's own standard (compare
F-327's 35–98% spread on the aggregate scalars) to be usable as the ADR's basis. Reported to architect directly per
the task brief; not itself a re-baseline decision, which is #104's.

## The two asserted marks, batch 3 (official `prf01-cold-start`, canonical)

| Budget | Statistic | Value (ms) | Limit (ms) | Verdict |
|---|---|---:|---:|---|
| `window_visible` (NFR-PRF-01, first painted frame) | p95, n=20 | 1575.6 | 400.0 | **FAIL** |
| `vault_usable` (NFR-PRF-01, vault usable) | p95, n=20 | 1577.2 | 1000.0 | **INDETERMINATE** — samples straddle the limit; reported as a failure per the harness's own rule that an undecided run must not go green |

Sample detail, from the result files (`bench/out/budgets/prf01-{window-visible,vault-usable}.json`):

- `window_visible`: n=20, min 812.6 ms, median 1267.2 ms (computed from the recorded samples), mean 1298.5 ms,
  max 1603.3 ms, p95 1575.6 ms, spread 97.3% (harness-reported).
- `vault_usable`: n=20, min 814.2 ms, median 1268.4 ms, mean 1299.6 ms, max 1604.2 ms, p95 1577.2 ms, spread 97.0%.
- `machine`: windows · 16 cores · realtime scanner OFF · power plan Energy Star · cpu load 1% ·
  `concurrent_build_processes: 0`.

**Cross-batch range, not a single point.** The same binary, same vault, three consecutive `n=20` batches in one
sitting gave `window_visible` p95 of 1643.0 ms (batch 1), 1876.5 ms (batch 2), 1575.6 ms (batch 3) — an 1576–1877 ms
range, ~19% batch-to-batch spread on top of ~97% *within*-batch spread on every batch. `vault_usable` moves with it
(1644.0 / 1878.5 / 1577.2 ms). Reporting the official batch's number as the record (it is the one with the
digest-stamped result file and the only one that ran through the unmodified gate binary) but the range is the honest
statement of what this session measured, per F-138/F-327's standing lesson: a number without the sessions around it
is not reproducible from what's recorded.

## Comparison against the pre-#98 placeholder shell

| Mark | Placeholder shell (pre-#98) | Real frontend, this session (batch 3) | Limit |
|---|---:|---:|---:|
| `vault_usable` p95 | 1094 ms | 1577.2 ms | 1000 ms |
| `window_visible` p95 | 1088 ms | 1575.6 ms | 400 ms |

**Worse on both marks, by roughly 45%.** This is the honest cost of mounting CodeMirror 6 that the owner's ruling
asked this task to surface rather than avoid. It is not entirely (or even mostly) attributable to `S_front`/`S_shell`
alone, though — see below.

**What the seven-term breakdown adds to that comparison, and the caveat it forces.** `S_front` (the term the CodeMirror
mount actually lives in) is 197–230 ms median/p95 — real, but on its own nowhere near large enough to account for a
~480–490 ms p95 regression on the aggregate marks. `P_load` and `P_webview` — both platform terms, unrelated to
CodeMirror — are the larger and noisier movers (`P_webview` alone: 658–1199 ms p95 across the two term-level batches).
**This session cannot separate "the real bundle is heavier to load into WebView2" from "this machine's ambient load
moved between the placeholder-shell session and this one."** ADR-0014 §Decision 3 excludes the platform term from any
budget for exactly this reason — it is recorded, not judged — but it means the regression against the placeholder
figure above should not be read as "CodeMirror mounting costs ~480 ms." What can be stated with the term-level
evidence behind it: `S` (the binding Serat term) is 201/234 ms median/p95, comfortably inside even the tighter of the
two legacy scalar budgets on its own, and the aggregate miss is dominated by platform terms this repository does not
control on this webview (F-96).

## Standing bounds (both push the same way — toward the number understating what a user sees)

- **Warm vault (F-154).** The Windows page cache cannot be dropped on this machine; every cold start after the first
  in a batch reads a warm vault. `docs/05-testing/reference-machine.md` §"No cold-start figure on this machine is a
  cold-vault figure" is unchanged by this session — still unmeasured, still narrower than a user's actual first
  launch.
- **`gate_eligible: false`.** Real-time scanner is off on this machine (RISK-012); every figure above is the
  best-case, scanner-disabled reading, not the charter-gate figure. Stated in every result file
  (`"gate_eligible": false`) and in every `environment` stamp this session took.

## Three things the brief asked to be stated plainly

**The mark is not yet guarded (F-397).** `mark_first_paint` (`crates/serat-app/src/commands.rs:104`) is the only
call site that can set `paint_evidence_ms`, but #113 — a test asserting that call site passes the real evidence
rather than a placeholder — is written but not on `main`. Confirmed by inspection this session: nothing in
`commands.rs` or its test module currently pins the argument that call site is given. So every figure in this report
is a real measurement of an **unguarded** mark; a future one-line change to that call site would not be caught by
anything in the suite today, and this report is not evidence that it would be.

**The conjunction is enforced nowhere (F-403).** `raf1_ms`/`raf2_ms` — the two `requestAnimationFrame` timestamps
`app/src/startup.ts` computes — reach no Rust parameter on `main`; only `paint_evidence_ms` (the
`first-contentful-paint` entry) crosses the IPC boundary. `budget::trace::StartupTrace::validate` does not consume
`paint_evidence_ms` at all (confirmed by reading `trace.rs`'s six rejections before adding the field this session) —
this report's new `paint_evidence_ms` field is parsed and displayed, not validated. That is sound today only because
`app/index.html` is contentless before Vue mounts, so nothing can paint early and pass — but nothing in this harness
asserts that property, so it is a fact about the current build, not a guarantee.

**`paint_evidence_ms`, reported for the first time.** Present on 20/20 runs in both term-level batches:
batch 1 median 192.0 ms / p95 228.0 ms; batch 2 median 212.0 ms / p95 244.0 ms. These track `S_shell`
closely (batch 1: `S_shell` 175.5/213.0 vs evidence 192.0/228.0; batch 2: `S_shell` 196.1/229.2 vs evidence
212.0/244.0), which is the expected relationship — `first-contentful-paint` fires inside the same interval
`window_created → window_visible` measures — and is a mild positive signal that the evidence is measuring what it
claims to, but per the two caveats above this is an observation, not a passing test.

## F-401 guard duplication, found and resolved before measuring

`.worktrees/test` carried an uncommitted `ColdStartVaultGuard` (drafted in an earlier session under the freeze,
never compiled) at the same time `origin/main` had independently landed `CleanIndexOnDrop` at `abfb3d8`, itself
committed `UNVERIFIED` under a freeze. Same shape as the #97/#105 collisions the lead's rulings describe: two fixes
for one hazard, written from views of the file that disagreed, neither ever run. Resolved in favour of
`ColdStartVaultGuard` — `CleanIndexOnDrop`'s `Drop` deletes `<vault>/.serat` unconditionally, which would destroy a
`.serat/` that predates the batch for a reason the guard cannot see, the exact hazard its own doc comment names one
paragraph before doing it anyway. `ColdStartVaultGuard` records whether `.serat/` existed at `watch()` time and only
removes what the batch itself created; it ships with four tests (own-creation cleanup, pre-existing left alone,
cleanup on early return via `?`, ordering of the pre-existing check) that now compile and pass. Filed as
**F-NEXT/f401-guard-duplicated** for the lead to number.

## What #101/#104 should carry forward

- **NFR-PRF-01 misses on both marks** against the current 400 ms / 1000 ms budget, on the real frontend, on the
  scanner-disabled/warm-vault best case. Not gate-quality by ADR-0013's standard — no attribution instrument
  (`concurrent_build_processes`) contradicts a load explanation here the way it did for NFR-PRF-04 (#96); this
  session's own three-batch spread (1576–1877 ms p95, `concurrent_build_processes: 0` throughout) suggests the noise
  is not simply "another process was building," which was NFR-PRF-04's answer, and is unresolved.
- **`S` (the binding Serat term under ADR-0014 §Decision 3) is 201/234 ms median/p95** — inside both legacy scalar
  budgets on its own. The aggregate miss is dominated by platform terms (`P_load`, `P_webview`) this project does
  not control on WebView2 (F-96, one of three webviews).
- **`S_front`'s spread (176–198 ms median, 213–230 ms p95 across two batches) is the input ADR-0014 needed to move
  from `proposed` to `accepted`.** Reported to architect directly.
- Every figure here is warm-vault, scanner-disabled, and rests on an unguarded mark (F-397) whose conjunction is
  unenforced (F-403). None of that is new to this session, but all of it now has a concrete reading behind it rather
  than only a placeholder-shell reading.

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01 | The re-baseline's first post-mount reading: both marks, all seven ADR-0014 terms, `S`/`P` totals. |
| US-35 | "Stays fast as the vault grows" — measured against the 20k fixture, warm-vault caveat stated. |
| ADR-0014 | Supplies the `S_front` spread the ADR's `accepted` transition needed; both caveats (F-397, F-403) restated per §Consequences. |
