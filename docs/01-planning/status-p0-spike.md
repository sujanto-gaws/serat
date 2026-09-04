---
project: Serat
artifact: P0 spike status
phase: 01-planning
owner: lead
status: interrupted — gate NOT reached
---

# P0 Spike — Status

> **P0 produced no gate numbers. The four budgets are exactly as unknown as they were at the start.**
> The design was reviewed and corrected, repeatedly and usefully — that is not the same as validated, and the most likely misreading of a report full of design findings is that it was.

**Gate (charter §Milestones):** full index of 20,000 notes < 30 s · save→index < 50 ms p95 · cold start < 1 s (window visible < 400 ms).

## Gate verdict: NOT REACHED — no budget number was ever taken

**Zero of the four gate numbers exist.** The spike ended when every teammate hit a session limit within six minutes of each other, with the measurement subjects still unlanded. This is not a NO-GO — a NO-GO is a measured miss, and would itself be a result. Nothing was measured, so nothing is known about whether the architecture meets its budgets.

What exists instead is a verification apparatus with no subject yet: the benchmark vault, the budget harness, the CI jobs, the startup-trace contract and the test cases are built and green, and each budget job fails loudly today naming the task that unblocks it. Three go green with one function body each — TC-020 when `rebuild` lands, TC-021 when `apply_and_wait` lands, TC-022 when the `serat` binary lands.

**The one measured figure bearing on a budget is not reassuring, and is not a verdict either:** `Vault::walk` costs ~2.0 s on the 20k vault (F-12), against FR-VLT-01's own 2 s budget on the no-index path. It is off the cold-start critical path per §3.4, and its fix (lexical prefix strip, ~50 ms) is written but deliberately unlanded, because working agreement 4 requires it to land with the bench that measures it.

## What P0 actually produced

**62 recorded findings** (`tasks-p0-spike.md`), every one measured or reproduced rather than argued — including several that correct earlier findings, and two that correct the lead. P0's stated purpose is to replace blueprint targets with real numbers; it did not do that. What it did instead, at a cost of one merge, was find a class of defect that would have made those numbers untrustworthy had they been taken.

### The four findings that would have corrupted the gate result

| # | Finding |
|---|---|
| F-19 | The rebuild dropped files under backpressure. It would not have crashed or logged — it would have indexed part of the vault, completed, and posted a **fast** NFR-PRF-04 number, which the gate report would have cited as evidence the architecture works. |
| F-25 | `.ceil()` to `.floor()` in the percentile is invisible at n=20 and n=1000 where the two coincide, and at n=3 turns NFR-PRF-04's median-of-three into a **best**-of-three. Nothing asserted the median at all. |
| F-26 | The vault digest gate proved self-consistency, not canonicality: a vault regenerated under a different seed passed straight through, because the manifest is regenerated alongside it. |
| F-27 | Six of eight mutations of the budget harness were invisible to all nine of its tests, including deleting the note-count guard that exists to catch F-19. |

Every one is invisible **in the direction that flatters the number**. Had the numbers been taken on schedule, they would have been taken with these in place.

### The pattern, which is the transferable result

**Six tests that could not fail, across three teammates** (F-13, F-28, F-35, F-50), every one found by mutation or boundary arithmetic and none by reading. Twice, the fix for one produced another that still could not fail — including one the lead merged (F-28) — and the sixth was the guard for F-19, the spike's headline defect. The sharpest statement of the pattern is its most frequent author's (F-58): *"I write the assertion correctly and then arrange the setup so the code path under test is never entered."* Reading the test tells you nothing, because the assertion looks right; the check is whether the setup reaches the path, not whether the assertion is correct. The generalisation (F-40): the useful question is *which of these guards can be removed without a test going red*, and the stronger answer is a structural guarantee no deletion can reach. Two mechanisms guarded F-19; one could be deleted silently, the other could not, and its author's own assessment was that the difference was luck as much as design.

### Design defects found by implementation

- **F-11** — the drop-latch-rescan recovery path re-entered the loss: capacity 8,192 against 20,200 sorted paths, FIFO, so the same prefix won every rescan and **12,008 files never reached the consumer after five rescans**.
- **F-31** — ADR-0011's own clear-on-completion wording still lost an overflow occurring *during* the rescan; found by its author reading the ADR against the first implementation to meet it. Recorded against the ADR, not the code.
- **F-32** — a bulk load with the FTS triggers live costs about **6x** on the phase that must fit inside a 30 s budget: 26,000 blocks at 2.1 s live against 0.32 s dropped, independently reproduced at 5.8x. *An earlier, larger figure was struck by its own author as an abandoned run rather than a measurement; the lead had repeated it twice before it was withdrawn.*
- **F-16** — the PRD's data dictionary contradicts the DDL; the DDL is correct, because a note-level FTS table cannot answer *which block* and so cannot serve a citation or a locator.
- **F-15, F-33** — a folder rename reached nothing; `write_atomic` silently widens the permissions of a note the user chmod'd 600 (design goal 1).

### Requirements results

- **F-03 — NFR-PRF-03 was never unsatisfiable.** Two paths were conflated: a save made in Serat has no watcher and no debounce (the 50 ms budget); an external change carries the 150 ms window and is FR-VLT-04's 500 ms. §3.2 already *required* the save path, because echo suppression drops the watcher event for Serat's own write — an index fed only by the watcher would never index a note saved in Serat at all. A missing paragraph, not a missing 100 ms. **A wording clarification for Q1, not a re-baseline.**
- **F-18** — a workaround §4.4 assumed P2 would need is unnecessary; the partial field indexes are selected unaided.

### Assumptions and risks

- **A-06 / RISK-002 — the Rust is not the risk (F-07).** serat-vault compiled first try, 35 of 40 tests passed on the first run, two clippy findings across two tasks, rusqlite built SQLite unattended. All three real costs were coordination: a workspace-wide gate red because another teammate's crate was mid-edit; the repository appearing mid-task; untracked `bench/` breaking `cargo metadata` in every worktree at once. **The risk is that the workspace is one shared object with many owners, and that cost scales with teammates rather than with lines of code.** RISK-002 should be re-rated on this evidence, not closed.
- **RISK-012 (new, mitigated in code)** — the reference machine runs with its real-time scanner disabled, so its figures are a Windows best case. The harness now detects the scanner and power plan on every run, marks a scanner-disabled *or undetermined* environment `gate_eligible: false`, and never toggles the scanner itself.
- **§1.2's three assumptions are untouched (F-09).** Nothing measured confirms or contradicts the single-writer assumption; parse-pool-versus-writer is open until I5 and T2; vector search is P4. **Nothing may be re-baselined on P0's evidence to date.**

## Artifacts

**On main (37 commits, two merges):** the Cargo workspace and a mechanically-enforced dependency rule; `serat-core`; the deterministic 20k/50k benchmark vault generator (`bench/vaultgen`, one command, byte-identical from a fixed seed); the budget harness and CI budget jobs (`bench/budget`, task B2); the compiled IPC contract and the `serat-contracts` crate that keeps it compiled (task D3 + review fixes); `schema/001_init.sql`; the findings register; `coding-standards.md` (worktree workflow), `dev-setup.md`, `benchmark-vault.md`, `reference-machine.md` (RM-1), `budget-harness.md`, `test-plan.md` (TC-001..TC-029).

**Unmerged.** The design stack is now a **single tip**, `task/f43-truncation-page-threshold` — §3.4, ADR-0011 (amended), §2.1's F-04 ruling (corrected per F-61), §2.2's barrier correction, ADR-0005's pin, F-43's one-page truncation rule. Then `task/i2-serat-vault-watcher` (both blockers fixed; the reviewer's mutation now fails two tests where all 55 passed); `task/i4-serat-index-actor` (bulk-load sequence in `rebuild()`, two-pass link resolution, F-10's phase durations); `task/b3-ci-workflow`; `task/b4-close-f25-f26-f27` (**WIP** — the harness fixes that must land before any number is taken); `task/s1-tauri-shell` (**WIP** — compiles, trace marks unproven, needs the F-46 `t_usable` gate).

Three branches are lead-committed WIP: their authors' sessions ended mid-task with the work uncommitted in shared worktrees. Each commit message states exactly what is unfinished. **None may be merged as-is.**

## To resume, in order

1. **`task/b4-close-f25-f26-f27` (test-engineer).** Nothing else matters until the harness cannot flatter a number: F-25, F-26, F-27, plus F-38 (assert `write-FTS` non-zero, or a rebuild regressing to load through the triggers reads as an improvement). The author had just begun mutating their own guards.
2. **`task/i4-serat-index-actor` (rust-core-dev).** Author's last words: *the primitive is guarded now, but the rebuild's use of it still isn't — mutation 1 still passes.* Then the F-35 sweep of serat-index.
3. **Merge the design stack** at `9e20472`, then `task/d3-review-fixes`, then I2's revision, then I4. **I2 must merge before S1**, which has it merged in.
4. **`task/s1-tauri-shell`** — rebase onto `d3-review-fixes` first; two breaking contract changes landed under it.
5. **Then the numbers:** TC-020, TC-021, TC-022, T1, T2 — and D4 and Q1, which wait on T2.

## Open, owed, unresolved

F-12 (walk fix, held for its bench) · F-29b (architect to confirm the widened `NoteSource` surface) · F-31 (generation counter — implemented, unreviewed) · F-33, F-34 (`write_atomic` permissions and parent fsync) · F-35 (sweep unfinished) · F-38 · code-reviewer's D3 findings, sent to teammates but not recorded here.

**Process deviations, recorded rather than smoothed:** the lead cherry-picked `8c5730f` to unblock three teammates, carrying `NoteSource` and `LoadedNote` onto main inside a commit whose message describes only a test fix — provenance is F-30, and the author later confirmed they would have preferred it separate. The lead also committed three teammates' work directly: twice because it sat uncommitted in a shared working copy across repeated requests to land it, and once because the authors' sessions ended mid-task.
