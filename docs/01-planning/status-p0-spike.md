---
project: Serat
artifact: P0 spike status
phase: 01-planning
owner: lead
status: interrupted — gate NOT reached
---

# P0 Spike — Status

> **One of the four gate numbers now exists, and it misses: cold start is 940-1146 ms p95 against a 400 ms budget — 2.4-2.9x over — of which Serat's own main-thread work is 5.2 ms, about 0.5% (F-86, F-130). The other three budgets remain unmeasured.**
> The design was reviewed and corrected, repeatedly and usefully — that is not the same as validated, and the most likely misreading of a report full of design findings is that it was.
> The measurement apparatus is now well guarded against flattering itself, which is also not the same as the numbers being good (F-71). The first real figure is still the first real figure.

**Gate (charter §Milestones):** full index of 20,000 notes < 30 s · save→index < 50 ms p95 · cold start < 1 s (window visible < 400 ms).

## Gate verdict: NOT REACHED — four statements, not one

**This section is deliberately before the findings.** A reader who meets a hundred-odd findings first will have formed a view by the time they reach the verdict, and the view formed from a hundred design corrections is "this was validated". It was not.

**The largest fact first, because the good news is the part a reader will generalise from.**

1. **THREE OF THE FOUR BUDGETS ARE UNMEASURED** — full rebuild, save-to-index, and the 50k scale run. Every code path they need now exists, so they are blocked on merge and measurement rather than on code; that is not the same as being measured.
2. **§1.2's three structural assumptions remain UNTESTED.** Nothing measured says whether the parse pool or the index writer dominates a rebuild, whether write contention is the bottleneck, or whether brute-force vector search scales. **P1 must not proceed as though P0 validated the three structures it was built to test.**
3. **Cold start is MEASURED AND MISSED** — `t0 → window visible` **1.9x to 2.9x over the 400 ms budget** across runs, on one platform, best case (scanner off, so `gate_eligible: false`), with the cause attributed and **mostly outside our control**: Serat's own main-thread work is **4.8–5.6 ms, under 1%** of the time to first paint, while **596–865 ms of the median sits inside `WebviewWindowBuilder::build`** — WebView2 environment and controller creation, before any Serat code that matters. Report ranges, not points: successive runs of the same branch on the same machine give 760, 810 and 940–1146 ms to paint (F-138). Every version of the conclusion survives.
4. **§3.4's design property is MEASURED AND HOLDS, on both halves — and that is one property, on one platform, on one machine.** *Main-thread work is constant in vault size:* 5.2 ms at 2 notes against 5.6 ms at 20,000, a ten-thousand-fold difference on the one term not dominated by a platform cost (F-130, F-132). *Wall clock does not grow with vault size beyond the tolerance:* 30 **interleaved** pairs, paired-difference 95% CI **[-123.1, +45.4] ms against a 179.5 ms tolerance**, the interval excluding the tolerance (F-141). This was recorded as *unfalsifiable* an hour earlier and that was wrong — **the obstacle was blocked sampling, not the iteration count**, and within-arm spread of 830 ms against the effect is why no quantity of blocked samples decides it while thirty interleaved pairs do. **The claim is a bound, not an absence:** a small real effect is not excluded and nothing in the design needs it to be.

**The one extrapolation in this report (F-143):** the case for keeping the walk deferral — a ~50 ms walk at 20,000 becoming ~125 ms at 50,000 and half a second at 200,000 — is **arithmetic from a single measured point. Nobody has run 50,000 notes at all**, though the generator has had the preset since B1. It is the weakest claim in a document where everything else is measured, and it was raised by the person the ruling had favoured.

§3.4 budgets that whole phase at 400 ms *including* our own work, which the measurement shows is not available to us. The question this hands Q1 is not whether Serat can be made faster but **whether 400 ms is achievable on Tauri v2 at all** — a question about C-07, not about the implementation. NFR-PRF-01 was deliberately **not** re-baselined on it: one platform is not enough when WebView2 is one of three webviews (F-96).

**The failure mode to avoid when reading this document:** the 619 ms is not a finding about the crate graph, the actor, or the schema. **It is a finding that the requirement was written without a webview-startup term in it.**

**Three of the four gate numbers do not exist.** The spike had earlier stalled when every teammate hit a session limit within six minutes of each other, with the measurement subjects still unlanded. This is not a NO-GO — a NO-GO is a measured miss, and would itself be a result. Nothing was measured, so nothing is known about whether the architecture meets its budgets.

**Every code path P0 needs now exists** — `rebuild` (I4), `apply` and `flush` (I5), the shell binary (S1) and `open_or_rebuild` (I6) — so the three unmeasured budgets are blocked on merge order and measurement rather than on code. **NFR-REL-02's acceptance criterion is exercised**: truncate the index by one page, start, detect, rebuild. One caveat its author volunteered rather than letting the branch read as covering it: I6's tests run against a fake vault, so the classification and ordering are proven and real-disk behaviour is not — and **the 4 KB mid-file case that passes all six stamp steps is not covered, because the `IndexFault` classifier that would catch it is mandatory rather than defence in depth and is not built** (F-136, P1).

What the harness gives: the benchmark vault, the budget harness, the CI jobs, the startup-trace contract and the test cases are built and green, and each budget job fails loudly today naming the task that unblocks it. Three go green with one function body each — TC-020 when `rebuild` lands, TC-021 when `apply_and_wait` lands, TC-022 when the `serat` binary lands.

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

**A count of findings is not a judgement about quality, and this table should not be read as one (F-65).** Three of the four are attributed to one reviewer against one harness. That harness was *well built*: its seam guard, interval guard and digest gate all worked, and every invisible mutation was in the layer around them. Its reviewer's own assessment — offered unprompted — is that it was **untested in the places tests are hardest to write, which is a different and more common problem** than being badly built.

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

**On main (51 commits, three merges):** the Cargo workspace and a mechanically-enforced dependency rule; `serat-core`; the deterministic 20k/50k benchmark vault generator (`bench/vaultgen`, one command, byte-identical from a fixed seed); the budget harness and CI budget jobs (`bench/budget`, task B2); the compiled IPC contract and the `serat-contracts` crate that keeps it compiled (task D3 + review fixes); **serat-vault** — walk, watcher with the 150 ms coalescing window, the two-door bounded queue, blake3 hash gate, atomic temp-and-rename writes, echo suppression wired end to end (task I2, approved after two rounds with seven of seven mutations biting); `schema/001_init.sql`; the findings register; `coding-standards.md` (worktree workflow), `dev-setup.md`, `benchmark-vault.md`, `reference-machine.md` (RM-1), `budget-harness.md`, `test-plan.md` (TC-001..TC-029).

**Unmerged.** The design stack is a **single tip**, `task/f43-truncation-page-threshold` — §3.4, ADR-0011 (amended), §2.1's F-04 ruling (corrected per F-61), §2.2's barrier correction, ADR-0005's pin, F-43's one-page truncation rule. The nine earlier design branches are its ancestors: merging the tip takes them all, and they are listed separately only because they still exist as refs. Then `task/i4-serat-index-actor` (bulk-load sequence in `rebuild()`, two-pass link resolution, F-10's phase durations); **`task/i5-incremental-save-path`** — in progress, and the single task standing between this spike and its first budget number; `task/b3-ci-workflow`; `task/b4-close-f25-f26-f27` (**WIP** — the harness fixes that must land before any number is taken); `task/s1-tauri-shell` (rebasing onto main, needs the F-46 `t_usable` gate closed).

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
