---
project: Serat
artifact: S_shell under a fabricated mark_first_paint placement (task #127)
phase: 05-testing
owner: tauri-app-dev
serves: NFR-PRF-01, NFR-PRF-08, US-35
cites: ADR-0014, F-327, F-406, F-470, F-477, F-504, F-505
---

# `S_shell` under a fabricated placement (task #127)

**Status:** measured · **Run by:** tauri-app-dev, 2026-09-09
**Verdict against the pre-committed condition:** a raised `MIN_PAINT_GAP_MS` **does not separate**
honest from fabricated placement. The distributions **overlap**, before F-327's drift is considered.

## The question, and why it was the only one worth asking

F-505 established that **no arithmetic over the three webview-supplied numbers**
(`firstContentfulPaintMs`, `raf1Ms`, `raf2Ms`) can guard against fabrication: the fabricator supplies
all three, so any rule expressible over them is satisfiable by choosing them to satisfy it.

That left one candidate with apparent margin. `S_shell` = `window_visible − window_created` is computed
from **two host-stamped clocks**, the only quantity in this mechanism the webview does not supply, and
`bench/budget/src/trace.rs`'s `MIN_PAINT_GAP_MS` already rejects a trace below **5.0 ms**. The missing
number was `S_shell` under a *fabricated* placement — which decides whether raising that floor buys
anything.

**Architect pre-committed the outcome before the data existed:** if a raised floor cannot separate the
two with margin, the check **stays as it is** and ADR-0014's table records that it cannot fire. It is
not armed because arming feels safer. That is F-406's lesson applied in advance rather than after.

## Procedure

Followed `docs/05-testing/paint-guard-real-window-runs.md`, which is the mechanics authority.

1. **Mutation** — F-504's shape, `firstContentfulPaintMs: 1, raf1Ms: 0, raf2Ms: 1`, as a second literal
   invocation appended to `crates/serat-app/webview/boot.js`.
2. **Two-build order (F-470)** — bundle edited, then `cargo build --release -p serat-app`. Without the
   rebuild the mutation does not reach the binary and the run comes back clean; that hazard is why the
   next step is not optional.
3. **Positive control** — `paint_evidence_ms: 1.0` and `raf2_ms: 1.0` in **all five** runs, so the
   fabricated call demonstrably took the mark and the mutation demonstrably reached the binary. (Noted
   in passing: #125's new call-site count assertion caught this mutation before it was bypassed to run
   the measurement.)
4. **Teardown (F-477)** — bundle restored, rebuilt, honest run confirms `paint_evidence_ms: 372.0`;
   `git status --porcelain` empty; `git diff --name-only origin/main...HEAD -- crates/serat-app/webview/`
   empty on every branch.

**Vault:** a generated 4,000-note scratch vault, **not** the canonical 20k fixture — F-401's hazard, and
this measurement has no need of the canonical vault's size or digest.
**Binary:** `target/release/serat.exe`, release profile, `.worktrees/tauri-app`.

## Results

`S_shell` = `t0_to_window_visible_ms − t0_to_window_created_ms`, milliseconds.

| | n | min | p05 | median | p95 | max |
|---|---|---|---|---|---|---|
| **honest** (this session's accumulated traces) | 53 | **125.0** | 137.1 | 187.1 | 317.4 | 326.7 |
| **fabricated** (fresh runs, this task) | 5 | 104.6 | — | 126.7 | — | **132.7** |

Fabricated, per run: 104.6, 107.8, 126.7, 127.1, 132.7.

**They overlap.** Three of the five fabricated runs sit **above** the honest minimum of 125.0. Catching
all five needs a floor above 132.7 ms, which rejects honest runs at 125.0 and sits inside the honest p05
of 137.1. There is no value of `MIN_PAINT_GAP_MS` that admits the honest distribution and refuses the
fabricated one.

The pre-committed condition was *cannot separate **with margin** against F-327's 35% session drift*. The
measured result is stronger and simpler: it does not separate them **at all**, with the drift term never
reached.

## Why no number would have helped — the structural half

Architect's constraint holds one level further out than it was stated, and `S_shell` is **inside** it.

The rule is that a relation over caller-**supplied** numbers cannot be a boundary. `S_shell` looked like
the escape because both its clocks are host-stamped. But **the host stamps the *arrival*; the caller
chooses *when to arrive*.** `S_shell` is therefore not caller-supplied and is still **caller-determined**
— a fabricator who wants to land inside the honest band simply delays the fake call.

So the sampled fabricated range is not the relevant quantity; the **achievable** range is. This run
demonstrates a placement already inside the honest band **without having been chosen to be**.

`MIN_PAINT_GAP_MS`'s own doc scopes it correctly today — *"the gap below which `window_visible` is
assumed to be **mismeasured**"*. Raising it would repurpose a mismeasurement detector as a fabrication
guard that it structurally cannot be, and would read as protection while buying nothing.

## Standing caveats

- **This is a reconstruction of F-504's placement, not their diff** — a synchronous literal appended at
  bundle end. **It landed *below* honest; F-504's landed *above*** (2687 ms against 2458 ms aggregate).
  **That disagreement is a result, not a discrepancy to resolve:** the gap is placement-dependent and
  unbounded in both directions, which the lead established for the mark and which this measures for the
  floor. Measuring F-504's exact placement is one further run if the table wants it.
- **n = 5 fabricated, one machine, one build.** The overlap conclusion does not rest on the sample size:
  it needs one fabricated run above the honest minimum and there are three, and the structural argument
  above makes the sampled distribution secondary in any case.
- **`concurrent_build_processes` was not recorded for these runs.** They were sequential and followed
  their builds, but this is not a gate number and was not taken under the reference-machine protocol. It
  is a comparison of two placements on one machine, and should not be quoted as an `S_shell` figure for
  any budget.
