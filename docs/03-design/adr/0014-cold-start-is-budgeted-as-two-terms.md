# ADR-0014: Cold start is budgeted as two terms, one of which is not ours

**Status:** accepted · **Date:** 2026-09-08 · **Serves:** NFR-PRF-01, C-06, C-07, NFR-PRF-08

**Amended by ADR-0015 §5 (2026-09-24, proposed).** The tree precondition of `t_usable` is now the front-end mark `tree_first_screen` (root listing applied and painted), replacing `buffer_rendered`, which P0 used because it had no tree. The seven-mark partition and every term boundary here are unchanged; `S_settle`'s end mark changes which precondition settles it. The 233.8 ms p95 in §Decision 7 was taken under the `buffer_rendered` precondition and is labelled so until the Serat term is re-measured on the first build that renders a tree.

**Accepted on task #100's figures.** All three consultations in §Objections are answered, and
the one input this ADR reserved the right to be wrong about — `S_front`'s spread, which
§Decision 4's allocation depends on — is now measured. **§Decision 5's rule has been applied
and produced a non-empty interval, so the split yields a binding budget rather than the
escalation its empty-interval clause provides for.** The numbers and their bounds are in
§Decision 7, added on acceptance.

**What is still true of every figure here: the basis travels with it, and two of the basis
clauses are new.** #100's figures were taken with the paint mark **unguarded** (F-397: task
#113 is not on main) and with the harness **not consuming** the paint evidence at all (F-403).
Neither invalidates the numbers; both are conditions a later reading has to match to be
comparable.

## Context

NFR-PRF-01 asks for an interactive window within 400 ms and a fully usable vault within
one second, at 20,000 notes with a warm index. Both marks are measured and both are
missed: `window_visible` p95 **1088 ms** against 400, `vault_usable` p95 **1094 ms**
against 1000 (S1's first measurement of the real shell, 20 spawns, release, warm vault,
scanner disabled, Windows/WebView2).

The miss is not distributed the way the requirement assumes. §3.4's phase table puts
Serat's own main-thread work at **4.7–5.6 ms** and `WebviewWindowBuilder::build` at
**753.7–865.4 ms** — the platform term alone exceeds the whole 400 ms window budget by
more than twice, with no code from this repository in it. §3.4's own diagnosis:

> the 619 ms is not a finding about the crate graph, the actor, or the schema. It is a
> finding that the requirement was written without a webview-startup term in it.

**Three different figures for the webview term appeared within a few lines here — 619 ms,
753.7–865.4 ms, and a ~966 ms platform term — and they were three different measurements,
not a drift.** F-86's 619 ms is the median of that phase on the run that first measured the
miss; 753.7–865.4 ms is the same phase on the later run with marks placed for the purpose, at
two notes and at twenty thousand; ~966 ms was the whole *platform term* at twenty thousand, so
it summed three phases rather than naming one. Stated because §Decision 6 of this ADR requires
the basis to travel with the figure, and this paragraph is the one place in it that quoted
numbers without one. F-94 and F-106 both landed on exactly this shape of error.

**Every figure in this section is from that phase table, whose basis turned out to be
unreconstructable (§Objections), so this section is the diagnosis and not the evidence.** The
~966 ms is no longer quoted as a value anywhere else in this ADR: §Decision 2 and §Consequences
now carry #100's attested platform term and name the ~966 only as the figure they withdrew.
And the figures that remain here remain because withdrawing them would erase the reasoning
that produced the split. They are quoted for shape: an 865 ms term beside a 5 ms term is
not a claim any statistic decides.

The owner ruled on 2026-09-08 that NFR-PRF-01 is re-baselined **with** the webview term
rather than by opening C-07 (Tauri v2 + Vue 3 are fixed), and that **the re-baseline is a
split, not a slackening**. This ADR fixes the shape of that split so product-analyst can
write a requirement against it at task #104, and so the budget harness can assert it.

It is expensive to reverse for three reasons, which is why it is an ADR and not a section:
a requirement will be baselined against it; CI jobs will assert against its term
boundaries; and it defers a C-07 question that has to stay visible after everyone who
deferred it has moved on.

**What this ADR does not do.** It does not choose the number. The figures above predate
task #98, which mounts Vue 3 and CodeMirror 6 into the shell those figures were taken
against, and task #100 re-measures cold start afterwards. **The structure is settled here
and the figures slot in.** Every figure quoted in this ADR is provisional in exactly that
way and says so where it appears.

## Options considered

1. **Slacken NFR-PRF-01 to the measured number** — 400 ms becomes 1100 ms, 1 s becomes
   1.2 s. Rejected. It converts a requirement into a description of the current build,
   which is the failure mode C-06 exists to prevent: the budget stops being able to fail,
   so it stops being able to catch a regression. It also books Serat's own work — ~140 ms
   when this option was weighed, composed from the phase table and since attested at
   **201.1 ms median / 233.8 ms p95** (#100) — into a number dominated by a platform term, so a doubling of the front end's startup
   cost would be inside the new budget on arrival.

2. **Open C-07 and re-decide the shell.** Rejected by the owner, and the reasons are on
   record rather than assumed: the miss is a fact about what Tauri v2 costs before any
   Serat code runs, the same webview term would have to be paid by any embedded-webview
   shell, and P0 has measured exactly one of the three webviews Tauri uses (F-96). A
   framework decision taken on one platform's number is the same class of error as the
   requirement that was written without the term in it.

3. **Budget only the marks we already have, and add a note that most of it is the
   platform.** Rejected. §3.4 has published a prose "Serat's own share" twice and been
   wrong both times — "roughly 130 ms of the 400", which matched no subset of the terms,
   and 28 ms, which added two intervals of which one was mostly Tauri's runtime. A note is
   what both of those were. Nothing that is not a measured interval survives contact with
   this document.

4. **Split `t_usable` into a partition of contiguous, mark-bounded intervals, budget the
   Serat terms, and record the platform term without a budget.** Chosen.

## Decision

### 1. The split is a partition, not an attribution

Every millisecond between `t0` and `t_usable` belongs to **exactly one** term, and each
term is the interval between two marks the application already emits
(`crates/serat-app/src/startup.rs`). The terms therefore sum to the mark by construction.

| Term | Interval (fields of `Trace`) | Owner |
|---|---|---|
| `P_load` | `t0` → `t0_to_main_ms` | platform — loader, DLLs, runtime init |
| `S_pre` | `t0_to_main_ms` → `t0_to_state_ready_ms` | **Serat** — arguments, settings TOML, `AppState` |
| `P_builder` | `t0_to_state_ready_ms` → `t0_to_runtime_ready_ms` | platform — `tauri::Builder` construction |
| `S_setup` | `t0_to_runtime_ready_ms` → `t0_to_setup_done_ms` | **Serat** — event sink, `open_vault`, thread spawns, boot info |
| `P_webview` | `t0_to_setup_done_ms` → `t0_to_window_created_ms` | platform — `WebviewWindowBuilder::build` |
| `S_shell` | `t0_to_window_created_ms` → `t0_to_window_visible_ms` | **Serat** — our bundle in their renderer, to the first composited frame |
| `S_settle` | `t0_to_window_visible_ms` → `t0_to_vault_usable_ms` | **Serat** — the remaining preconditions of `t_usable` |

**Platform term** `P = P_load + P_builder + P_webview`.
**Serat term** `S = S_pre + S_setup + S_shell + S_settle`, and `P + S = t_usable` exactly.

Being a partition is the whole of its integrity. Time cannot be moved out of the Serat
term without moving it into a named platform term at a named mark, and the identity
`Σ(terms) = t0_to_vault_usable_ms` is checkable on every single run — **and since task #106
landed it is checked: `bench/budget/src/trace.rs` requires all seven marks rather than
making four of them `Option`, and asserts the identity.** The paragraph below is kept in the
tense it was written in because it records why the property was unassertable and what that
cost; **what it must not be read as is the current state.** §Objections carries what the
assertion is actually worth, and it is less than this paragraph hoped: given the other
checks the identity is a telescoping sum, true of any seven numbers, so what it guards is
the harness's own field-to-term mapping rather than the application's marks. One-line
re-check: `grep -n 'fn sum_ms' bench/budget/src/trace.rs`. F-381 caught this
paragraph asserting the property in the present tense when the harness could not evaluate
it: the application stamps all seven marks and `bench/budget/src/trace.rs` parses four, so
three of the seven terms are computable and **the Serat term, which is the binding one, is
not** — two of its four sub-terms are unavailable. The property is a property of the
partition; being able to *evaluate* it is a property of the harness, and writing the second
in the present tense because the first is true is precisely the error this ADR spends its
Context section on. A decomposition that
did not have that property would be a fourth composed aggregate, and this section has a
record with those.

### 2. The attribution rule

**A term is Serat's if a change confined to this repository can move it. It is the
platform's if it cannot.**

Two boundaries are decided by that rule against the easier alternative, and both are
deliberate:

- **`S_shell` is Serat's**, though it executes inside WebView2's renderer. The renderer is
  theirs; what it renders is ours — bundle size, framework, mount work, first layout. Book
  it to the platform and a front-end regression becomes invisible to the budget, which is
  the flattering measurement this spike has now recorded going wrong three separate ways.
  `S_shell` is the term task #98 moves, and it is the reason the split has to exist before
  #98's number is taken rather than after.
- **`P_builder` is the platform's, with a caveat that is stated rather than rounded off.**
  `tauri::Builder`'s construction is platform code whose cost is partly a function of a
  Serat decision — the size of `generate_handler!` and the plugin set. It is the one term
  where a Serat regression could hide in the platform column. It is booked to the platform
  because a builder term of ~13 ms against a platform term of ~1117 ms is not where anything
  hides (#100's attested medians; the ~29 ms and ~966 ms this clause used to quote came from
  the phase table whose basis is unreconstructable, §Objections),
  and because a budget on an interval we cannot profile from inside is a budget nobody can
  act on. **It is watched rather than budgeted**, and if it ever moves materially with
  command count that is the signal to re-book it.

### 3. Which terms are binding, and which are recorded

- **The Serat term is binding.** It is what C-06 enforces for NFR-PRF-01 from the
  re-baseline forward.
- **The platform term is recorded with every cold-start figure and carries no budget**,
  because no change in this repository moves it. "Not ours to budget" is not the same
  claim as "irreducible", and this ADR makes only the first — see §Consequences.
- **The total is reported beside both**, with its original targets retained as the
  user-facing aspiration they always were, marked unmet on WebView2. Deleting the total
  would hide the only number that describes what a user actually waits through.

### 4. The Serat term is allocated between two sub-terms, in the design rather than in the PRD

`S_main = S_pre + S_setup` (Rust, main thread) and `S_front = S_shell + S_settle` (our
front end in their renderer) have different owners and different noise floors. `S_main` is
the one term §3.4 can measure cleanly — 5.2 / 5.6 ms against a p95 of 9.4 / 9.6, across a
vault ten thousand times larger. **This clause said *median* until F-452, and could not**:
the basis of those cells is unrecoverable beyond *a central statistic, median or mean
unknown* (§Objections), and naming one is the error §Decision 6 exists to prevent — found
in this ADR's own text, one clause after the rule. The attested replacement is #100's
`S_main` at **3.4 ms median / 4.0 ms p95**, which is a median because someone recorded that
it was. **And the across-a-vault reading is a bound rather than a demonstration** (F-452:
the table's columns are a blocked comparison), which costs this decision nothing — the
allocation rests on the two sub-terms having different owners and different noise floors,
not on either being flat in vault size. `S_front` sits at ~121–138 ms pre-#98, composed
from the same table and superseded by #100's 197.6 / 230.4 ms, and is where the editor
lands.

**One binding number in the PRD (the Serat term); its allocation between `S_main` and
`S_front` lives in architecture §3.4 and in the budget jobs.** A requirement with three
numbers in it is a requirement nobody quotes correctly; a single sum, though, lets a
100 ms front-end regression hide behind 5 ms of quiet Rust, so the allocation has to exist
somewhere — it just does not have to exist in the PRD.

### 5. How the number is chosen, since choosing it is not this ADR's job

The rule below is the deliverable, because it is what makes the outcome a split rather
than a slackening in a form that can be checked instead of asserted. Let `s95` be the
Serat term's p95 from task #100.

- **Ceiling: the Serat term budget must be ≤ 400 ms.** The original 400 governed a window
  that contained the platform term as well. Applying 400 to a strictly smaller quantity
  can never be a slackening, so 400 is the *ceiling* on the new number and never the
  number itself.
- **Floor: it must be ≥ `s95` × 1.3**, with the measured spread stated. §3.4's own lesson
  from the scale check is that a tolerance sized against the effect and never against the
  noise produces a job that goes red for no reason and is disabled by the third person it
  wakes.
- **If those bound an empty interval** — if `s95` × 1.3 exceeds 400 ms — **the split has
  failed to produce a binding budget, and that is the finding.** It is escalated, not
  resolved by widening the ceiling. Widening it is the slackening this ADR exists to
  prevent, arriving one step later.

### 6. Every figure carries its basis, and every bound is stated as a bound

§3.4: *"the basis belongs with the figure wherever it is quoted, because the figure without
it is what went wrong the first time."* For this split the basis is **six** clauses, and all
six are conditions **under which the number is better than a user's**:

1. **Warm vault.** Every cold-start figure this project has produced is a warm-vault
   figure (F-154): repeated spawns leave the vault in the page cache, a freshly generated
   vault is write-cached rather than cold, and dropping the Windows page cache needs
   privileges this machine does not have. NFR-PRF-01 cannot be validated against a cold
   vault here.
2. **Scanner disabled.** `gate_eligible: false`, RISK-012. The harness marks it and prints
   NOT A GATE NUMBER. It bites less on this path than on NFR-PRF-04 — §3.4's critical path
   does no per-file work by design — but it still bites on `P_load` and on everything
   after `t_usable`.
3. **One platform.** WebView2 only. WKWebView and WebKitGTK are unmeasured (F-96). The
   *partition and the attribution rule* are platform-independent; **every figure is
   Windows**, and the platform term above all is a WebView2 number.
4. **One machine, and the machine is part of the measurement** (F-327). Figures carry
   `cpu_load_percent` and `concurrent_build_processes` (#83) or they are not comparable
   with each other.
5. **Which build.** Figures quoted from §3.4's phase table predate task #98's front end.
   #100's figures are the post-#98 ones, and the two must never be mixed in one table.
6. **Which *front-end* build, which is a separate clause and was missing until F-470.**
   Every claim this ADR and §3.4 make about front-end behaviour cites `app/src/*`, and the
   binary does not read `app/src/*`. `tauri.conf.json` sets `frontendDist: "webview"` with
   **no `beforeBuildCommand`**, so `crates/serat-app/webview/` is compiled *into*
   `serat.exe` and `cargo build` never invokes vite. **A statement about the front end is
   therefore a statement about the last vite build, not about the source it cites**, and
   the two can disagree with nothing to say so. F-470 measured the gap rather than
   inferring it: a stub that never calls `mark_first_paint`, launched without a rebuild,
   produced **exit 0, a complete trace and `paint_evidence_ms` 352.0** — a trace
   indistinguishable from an honest one. After `cargo build --release`, the same stub gave
   **exit 2 and no trace**. So a front-end change needs `npm ci`, `npm run build` and
   `cargo build --release`, **in that order**, and a run whose pipeline was never shown to
   reach the binary is not evidence about the binary.

   **What this clause does *not* say, because guessing either way would be worse than the
   bound.** The real-window runs already on record — #116's CSP work (F-417), F-392's
   `boot.js` mutations — are **not** re-characterised as invalid here. Nobody recorded
   whether they were taken with the two-build discipline, and inventing either answer is
   the failure this ADR's §Decision 6 exists to prevent. What is recorded is that their
   basis is **unstated on this axis**, which is the honest status and is the same shape as
   the phase table's unreconstructable basis (F-452). The remedy is F-470's positive
   control — apply the stub, rebuild, confirm exit 2, restore — and it costs one rebuild.

Clauses 1 and 2 push the number the same way. **A re-baseline attested on this basis is a
bounded claim and must be written as one.** This bound is why NFR-PRF-01 was deliberately
*not* re-baselined earlier (F-96); the owner has ruled to proceed, so the bound is recorded
rather than resolved.

### 7. What #100 measured, and the budget the rule produces

Two independent batches of n=20, same binary, same 20k vault, same sitting,
`concurrent_build_processes: 0` throughout (task #100).

| Term | Batch 1 median / p95 | Batch 2 median / p95 |
|---|---|---|
| `S_main` = `S_pre` + `S_setup` | 3.4 / 4.6 ms | 3.4 / 4.0 ms |
| `S_front` = `S_shell` + `S_settle` | 176.4 / 213.9 ms | 197.6 / 230.4 ms |
| **`S` (the binding Serat term)** | — | **201.1 / 233.8 ms** |
| `P` (platform, unbudgeted) | — | 1117 / 1687 ms |

**§Decision 4's allocation survives and is confirmed rather than assumed.** `S_main` reads
3.4 ms median in *both* batches with p95 between 4.0 and 4.6 — quiet enough to budget tightly,
which is what §Decision 4 claimed of it from the old phase table whose basis turned out to be
unreconstructable. `S_front` moves 176→198 ms median and 214→230 ms p95 across batches: about
13% and 8%. That is the spread the allocation needed and it is small enough that the two
sub-terms want separate budgets, as §Decision 4 said.

**§Decision 5's rule, applied.** `s95` = **233.8 ms**. The floor is `s95 × 1.3` = **303.9 ms**;
the ceiling is **400 ms**. The interval **[304, 400] is non-empty**, with 96 ms of room.
**So the empty-interval clause does not fire and the split produces a binding budget** — which
was never guaranteed when the clause was written, and is the result that makes this a split
rather than a slackening in fact rather than in intention.

**Recommended to product-analyst for #104: 350 ms**, sitting above the floor with margin for a
tail three batches cannot bound (F-327) and well below the ceiling, so it remains a tightening
of the 400 ms that used to govern a strictly larger quantity. The number is theirs; the
interval is not.

**The strongest empirical result is not any of the above.** #100 reports the aggregate scalars
at roughly **97% within-batch spread and 19% cross-batch**, at zero build contention — while
`S` moves about 8% on p95 across batches. **The term this ADR made binding is measurable and
the term it declined to budget is not.** That is the case for the split arriving from the
instrument rather than from the argument, and it is a better defence of §Decision 3 than
§Decision 3 gave itself. It also means the aggregate scalars cannot serve as a gate in their
current form whatever budget is set on them, which is #101's problem and not this ADR's.

## Consequences

**Positive.** Serat's contribution to cold start acquires a budget that can actually fail:
**201.1 ms median / 233.8 ms p95** of measurable work (#100, attested; the ~140 ms written
here before #100 was composed from the phase table and carried that table's basis),
invisible inside a number dominated by a platform term, becomes the thing someone is held
to. The partition is checkable per run rather than
argued. The front end gets a budget before the editor lands in it rather than after. And
the platform cost stops being an excuse that has to be re-explained in every conversation,
because it has a name and a column.

**Negative.** NFR-PRF-01 stops being one number a user recognises. "Cold start under one
second" is a promise; "the Serat term under X ms" is an engineering budget, and no user
cares which term their wait belongs to. §Decision 3 keeps the total reported for exactly
that reason, but the requirement is genuinely less legible than it was, and that is a real
cost rather than a presentational one.

**Risks.**

- **The split can be used to stop looking.** A term with no budget attracts no attention,
  and `P_webview` is the largest single term in the wait by a wide margin — **725.3 ms
  median / 1199.1 ms p95**, against a whole Serat term of 201.1 / 233.8 (#100, attested).
  The *78% of the wait* this clause used to quote was a ratio composed from the phase
  table's cells and inherited its basis; the attested figures make the point without one.
  The mitigation is §Open design questions, not this
  ADR, and mitigations that live in a table are the kind this project has watched go stale.
- **"Not ours to budget" is one claim; "irreducible" is another, and only the first is
  established.** §3.4 says the platform term is *"a platform cost that no decision in this
  document moves"* — which is true and is not a proof that no decision could. Nothing has
  tested whether the WebView2 term responds to anything under our control. Writing the
  split without saying so would convert a measurement into a floor, which is the same
  error class as the requirement that was written without the term.
- **Nothing keeps the four intermediate marks present (F-381, and it is F-41's shape).**
  All four are `Option` on `Trace` and `Startup::trace()` never requires them, so a mark
  that stopped being emitted would fail no test — it would simply make the Serat term
  uncomputable, silently, in the direction of there being no number rather than a wrong
  one. The cheap fix is in the harness rather than the app: **a trace missing any of the
  seven marks is a rejection, not an `Option`**, which makes them required in the only
  place that has to read them. That belongs in task #106 beside the sum identity.
- **`S_front` has one post-mount reading and it supports the allocation, weakly.**
  vue-frontend-dev's #98 smoke test measured `window_created → window_visible` — which is
  `S_shell` — at **~176 ms**, against ~130 ms for the static shell it replaces. **Basis, and
  it is not a gate basis: debug build, one-note vault, an end-to-end WebView2 run rather
  than a unit test, offered as an order of magnitude and explicitly not as a finding.** It
  lands near 200 ms rather than near 600, so §Decision 4's split of the Serat term between
  `S_main` and `S_front` survives on the evidence available today and is not rewritten
  before #104. **It must be confirmed against #100's release-mode figure on the 20k vault
  before this ADR moves to accepted** — a debug build over one note is not the basis
  §Decision 6 asks to travel with a number, and quoting it as though it were is the exact
  error §Context is about.

  It carries a second use worth recording. **A ~176 ms gap is the shape a real paint delay
  produces and a hoisted mark would not**, so it is positive evidence that #98's marks are
  honest — and it is precisely the kind of evidence F-386 says the sum identity cannot
  supply. **It is not a substitute for #105's mechanism**: an argument from plausible
  magnitude is weaker than evidence that cannot be fabricated, and it degrades to nothing
  the moment anyone changes what the shell does before first paint. Until #105 lands it is
  the only empirical check on mark honesty that exists, which is a statement about how thin
  the coverage is rather than about how good this reading is.
- **The sum identity does not catch a misplaced mark, and cannot (F-386).** Asked deliberately
  rather than hoped: does the partition catch F-377's surviving mutation — `mark_first_paint`
  hoisted to script start — on its own? **No, and the reason is structural.**
  `Startup::vault_usable()` is `painted.max(rendered).max(index).max(watcher)`
  (`crates/serat-app/src/startup.rs:189`), and `painted → usable` is the smallest term
  there is — **1.1 ms median / 1.7 ms p95** (#100, attested; the 4.7–7.5 ms this clause
  used to quote was the phase table's last row read *across* its two columns, which F-452
  rules inadmissible) — so `painted` is **not** the term that settles the maximum. Moving it earlier
  by Δ therefore leaves `t_usable` untouched: `S_shell` shrinks by Δ, `S_settle` grows by
  exactly Δ, and every sum is invariant. **An identity over contiguous intervals says nothing about
  whether a mark fired at the right event.**

  **And it constrains less than the first version of this bullet claimed — test-engineer's
  correction at #106, and it is worth taking in full.** This said the identity "constrains
  data integrity — a missing mark, a clock step". It does not. Given that all seven marks
  are required and that monotonicity is checked separately, the identity is a **telescoping
  sum of consecutive differences, which equals the last minus the first for any seven
  numbers at all, ordered or not.** It therefore **cannot fail on real data**. What it
  guards is the harness's own field-to-term mapping — an edit that feeds the wrong field
  into a term, or adds an eighth term without updating the sum — and it was proven by
  deliberately breaking that mapping rather than by any honest run failing it. So
  §Decision 1 is right that the partition tiles, and wrong if it is read as saying the
  identity is what checks that on live data. Nothing else in the decision changes. Moving an interior boundary redistributes time between two
  adjacent terms and is exactly what the identity cannot see. **So task #105 does not get
  simpler**, and the post-composite evidence it is built around is doing work nothing else
  does.

  **What the partition does buy is a diagnostic, and it is a sharp one.** Under the
  mutation `S_settle` grows from an attested 1.1 / 1.7 ms (#100) to roughly Δ, bounded by
  the window-build-to-paint interval — F-86 put that at ~100 ms pre-#98 and #100 attests
  `S_shell` at 196.1 / 229.2 ms after it. **The multiplier is left unstated rather than
  recomputed**: the 13–21× first written here divided one unattested figure by another,
  and what the diagnostic needs is only that the anomaly is orders of magnitude in the
  smallest and most stable term in the table, in a quantity today's four-mark harness cannot compute at
  all. **It is not a rejection and must not be turned into one by bounding `S_settle`:**
  that term grows legitimately in P1 with a real file tree, and on Linux with one inotify
  watch per directory. A large `S_settle` means the paint fired well before the other three
  preconditions, which is suspicious and not provably wrong.
- **The paint mark that bounds `S_shell` has a guard that cannot fail (F-377).** A
  `mark_first_paint` hoisted to script start survives the existing substring assertion,
  and the harness's 5 ms paint-gap floor does not see it either — that floor is sized for
  a Rust-side rewire to window creation, and this mutation's gap is page-load-plus-script-
  execute, far above it. **Such a mark does not flatter the total, it empties `S_shell`**,
  which is the term this ADR hands the front end to own. Task #105's guard is therefore a
  precondition of this split being worth anything rather than a neighbouring concern.
- **`S_front` may be too noisy to budget.** It is measured inside a renderer we do not
  control and its spread is unknown at the time of writing. If #100 reports a spread that
  is a large fraction of the number, the allocation of §Decision 4 collapses into a single
  Serat-term budget, and `S_front` is watched rather than asserted.
- **The C-07 question is deferred, and a deferral outlives the people who took it.** See
  below; it is in §Open design questions for that reason.

### What P0 hands P1, stated plainly so the decision is visible rather than lost

**On WebView2, the 400 ms first-paint budget is unreachable regardless of what Serat
does.** The platform term at 20,000 notes is **1116.7 ms median and 1687.1 ms p95** with zero
Serat code in it — 2.8× the whole budget at the median and 4.2× at p95 (#100, attested,
post-#98; the ~966 ms this paragraph used to quote came from the phase table whose basis is
unreconstructable, §Objections). No allocation of the Serat term changes that.

P0 cannot distinguish the two futures that follow, and both are P1's:

- **If macOS (WKWebView) and Linux (WebKitGTK) carry a platform term of the same order,
  the requirement was wrong rather than the design** — which is the shape of the answer
  F-96 predicted — and this split is the permanent fix.
- **If they do not, the 400 ms is achievable on two platforms of three and Windows is the
  outlier.** That is a WebView2 question first: whether the term responds to a fixed-version
  runtime rather than evergreen, to bundle size, to navigation ordering, or to nothing at
  all. Only if it responds to nothing does it become a C-07 question — *and then it is one*,
  because C-07 fixes Tauri v2 and a platform on which a Must-priority requirement is
  structurally unmeetable is a constraint being tested rather than a budget being missed.

**Neither is being decided now, and neither may be quietly dropped.** What settles it is
measuring the same seven-term partition on the other two webviews. **#112 is a precondition of
that plan and was not visible when it was written:** #108 types the paint field
`number | null` while #105 takes a bare `f64`, so a genuine null produces **no trace at all**.
On any webview without Paint Timing, cold start therefore becomes **unmeasurable rather than
merely unguarded** — and "run the partition on WKWebView and WebKitGTK" assumes a trace can be
taken there. It does not block P0, since F-393 measured the entry landing on WebView2, but the
P1 plan above is contingent on #112 and should be read that way. That is one row in
§Open design questions and it should stay there until someone has run it.

## Objections recorded

**test-engineer answered, in two halves, because the question has two different kinds of
answer.** *Assertable with the harness as it stands: yes, as of task #106 (`bench/budget/
src/trace.rs`).* All seven marks are now required rather than three of them `Option` (F-381's
harness half), and a `Terms` type computes the seven-interval partition and exposes
`platform_term_ms()`/`serat_term_ms()`/`s_main_ms()`/`s_front_ms()` — nothing before #106
could produce any of the four sub-terms this ADR's §Decision 4 needs. The sum identity is
also asserted, with one caveat stated rather than left implicit: given the other checks it is
a tautology of how the terms are defined (a telescoping sum, true for any seven numbers) and
cannot fail on real data — what it guards is this module's own field-to-term mapping, proven
by deliberately breaking that mapping and watching it catch it, not by an honest run ever
failing it. *`S_front`'s noise floor: I cannot answer this, and saying so is the honest
answer.* No `S_front` figure exists anywhere in this repository yet — §3.4's phase table
predates #98, and #100 is the task that mounts Vue 3 and CodeMirror 6 and takes the first
real reading. A tolerance asserted against a number nobody has measured would be exactly
what F-327 and §3.4's own scale-check lesson warn against: a job sized to pass rather than to
mean anything, going red for no reason and getting disabled by the third person it wakes.
§Decision 5's floor (`s95 × 1.3`) already builds the right instrument for this — it takes the
spread as data rather than assuming one — so my answer to the second half is that the
question is #100's to answer, not mine to guess at now. *(Answered 2026-09-08.)*

**The inner-mark question is answered, and by a field added for another purpose.** This ADR
asked vue-frontend-dev whether `S_shell` needs a mark separating WebView2's navigation from
our mount, and offered three ways to take it. **The answer is the third — reported, not
budgeted.** #108's frontend evidence carries `nav_dom_content_loaded_ms`, which is
`PerformanceNavigationTiming`'s DOMContentLoaded and therefore fires when HTML parsing
completes, **before any script runs**. That is exactly the boundary the question was about:
`window_created → DOMContentLoaded` is navigation and parse, `DOMContentLoaded →
window_visible` is our mount. It is **diagnostic and carries no budget** — `S_shell` stays
whole and stays ours under §Decision 2, so a front-end regression still lands on a term
someone owns, and the split exists to say *which half* moved when it does.

**That field must never be mistaken for the paint anchor, and it was once.** DOMContentLoaded
fires before Vue mounts, so a mark gated on it is early regardless of when real content
appears — it cannot expose F-388's mutation and would re-open exactly the hole F-388
identified. The paint anchor is `first-contentful-paint`; DOMContentLoaded is the `S_shell`
split. Two fields, two purposes, and the failure of confusing them is silent.

**A decision was pre-committed here against `first-contentful-paint` failing to populate in
a real WebView2 window — vue-frontend-dev's flag, and F-107's shape, since a guard built on
an entry that never arrives rejects every honest mark instead of the dishonest one. F-393
closed it by measurement and the contingency did not fire:** the entry populates, at
`startTime` 184 ms against `domContentLoadedEventEnd` 129.8 ms, over the Chrome DevTools
Protocol against the real binary. **129.8 < 184 is the ordering the conjunction argument
depends on**, so F-388's rejection of DOMContentLoaded as the anchor is now confirmed by
measurement rather than by reading the spec. The pre-commitment is kept in this record
rather than deleted, because a contingency that did not fire is the cheapest kind to have
written and the record is what makes the next one worth writing.

**What that measurement also shows is a fragility nobody has yet had to think about, and it
is one line of HTML away.** F-393 found `#app` holding real rendered content when FCP fired
— so on today's shell FCP *is* our mount, because the document renders nothing before Vue
fills it. **That is a property of #98's current markup, not of the mechanism.** Add a
loading skeleton, a spinner, or a static titlebar to `index.html` and FCP moves ahead of the
mount, F-388's hole reopens, and **the guard keeps passing** — the change that breaks it is
a one-line improvement made by someone with no reason to read this ADR.

**The conjunction is what absorbs that, and this is the evidence it was not belt-and-braces.**
Its two halves cover two different futures: the paint entry catches a mark hoisted before any
paint (F-377's mutation B), and the post-mount frame gate catches a paint entry that fires on
content which is not ours (F-388). **Only the first is exercised today.** The second is
dormant and becomes load-bearing the moment anyone adds markup that paints before Vue — which
is precisely when nobody will be looking at it. Neither half is redundant, and dropping
either one later on the grounds that it never fires would be removing the half that is doing
its job by not needing to.

**vue-frontend-dev answered the fairness question, and answered it against their own
interest.** Their words: a budget on `S_shell` is *"fair, under the ADR's own attribution rule
— bundle size, mount strategy, and which CodeMirror extensions we load at start are all
changes confined to `app/` that measurably move `S_shell`, so it's ours even though part of
the interval's floor is WebView2's own navigation cost we can't move. That's exactly why an
inner mark is worth it: `S_shell` as defined conflates an unavoidable floor with our
controllable mount cost, and a persistently large floor would bury a real regression in
noise."* **That is a better argument for the inner mark than the one this ADR made**, which
reached the same place from fairness rather than from detection. They also declined a second
IPC round trip on the ground that it would perturb the interval it measures — right, and it
is why the evidence rides on the existing call.

**vue-frontend-dev, originally consulted because `S_shell` is theirs and #98 is inside it.** The
question put to them was whether a budget on an interval measured inside a renderer they do
not control is fair, and whether an inner mark is needed to separate WebView2's navigation
from our mount. **A mark added now is cheap and after #104 is not**, so their answer can
still change the partition. *(Consulted 2026-09-08; awaiting their answer.)*

**tauri-app-dev answered on `P_builder`, and the objection is a confirmation.** Their words:
watching rather than budgeting it *"is right for P0: `generate_handler!` currently lists 6
commands, and nothing about #105 or #98 grows that list, so there's no reason to expect it
to move this wave. If a P1 command surface grows materially I'll flag it rather than let it
hide in the platform column — agreed with your caveat as written."* §Decision 2's caveat
stands unchanged, now with a count attached to it.

**An objection raised *by* this ADR against #105's proposed mechanism (F-388), recorded here
because it bears directly on `S_shell`'s meaning.** tauri-app-dev proposed replacing `boot.js`'s
double `requestAnimationFrame` with a `PerformanceObserver` gate on the
`first-contentful-paint` entry, on the ground that a mark which cannot fire before a real
paint makes F-377's mutations structurally inapplicable rather than merely detected. **The
evidence half is right and the replacement half is not.** `first-contentful-paint` attests
that *something* painted, not that *our shell* painted, and the two coincide only while the
document renders nothing before the framework mounts. **They did not coincide in the shell this
objection was raised against, and by design:** `crates/serat-app/webview/index.html` — the
S1 placeholder, still on main as this is written — carries the entire shell as static
markup — wordmark, vault label, editor, status bar — because §3.4 requires the editor to
accept keystrokes from the first paint, *"a property of the document rather than a
promise"*, and that file instructs #98 to preserve it. So against that shell FCP fires on the static skeleton,
**before Vue and CodeMirror mount**, and a mark gated on FCP alone would move `t_visible`
earlier by the whole cost of the mount — which is `S_shell`, the term this ADR makes
binding. **That is mutation B's exact effect arriving legitimately, through the fix for
mutation B**, and F-386 says the sum identity would not see it.

**#98 resolved it the other way, and the objection stands anyway.** vue-frontend-dev did not
preserve the static markup: `app/`'s `index.html` is an empty `#app` div, so nothing paints
until Vue mounts and FCP cannot fire early *on that shell*. That is a better answer than the
one asked for — §3.4's property still holds, because the first paint now **is** the mounted
shell — and the hazard described above never materialised. **It also means the mechanism's
safety now rests on a markup decision rather than on the mechanism**, which is what the
paragraph on F-393 below is about.

**F-396 checked that against #98's actual tree and made the survival reason stronger than the
one given here.** `app/index.html` and the built output both carry an empty `<div id="app">`,
and `shell.css` paints only a background colour — which the Paint Timing spec excludes from
*contentful* paint by definition. **So on this tree FCP does attest that our shell rendered**,
and the hazard argued above is not merely unrealised, it is absent. What survives is a
different and better objection: **#105's guard lives in `crates/serat-app` and its soundness
depends on a property of `app/index.html` that nothing asserts** — that the document stays
contentless until Vue fills it. That is a cross-crate implicit dependency with no check
across the boundary, and it is a stronger reason to keep the conjunction than the one this
ADR originally gave, because it survives someone adding a splash screen where "FCP fires on
the skeleton" only described a tree that no longer exists.

**Ruling for #112, and it corrects the phrase "a guard split across crates" that this ADR
used a paragraph ago.** The defect is not that an assertion would live in another crate — it
is that the precondition is asserted **nowhere**. A check on `app/index.html` placed in `app/`
is correct placement, not a split. **Four properties had to hold and none of them had an enforcer** — the state at #109, and the
reason this table exists. That is a claim about a past tree, so it can only become history and
never false. **The table has since gained a row** — #112 made the absent-paint-entry case a
rejection in its own right — **and what is enforced now is the *Enforced as of* column's to
say, not this sentence's.**

**This sentence carried a count of the rows below it through two corrections, and product-analyst
argued it out rather than reporting it.** The first version left it reading *"four properties"*
over five rows (F-496). The second moved the number and added a second one — *five rows, two of
them enforced* — which is two live counts of a growing table where there had been one. **Their
argument for dropping both is that the guard a count buys depends on what it guards.** In the
PRD, a count over a bounds list is a real guard: a figure quoted elsewhere with three of its
four bounds looks complete, and the number is what makes that visibly wrong. **Here the reader
is looking straight at the table**, so the rows already say everything the count says, and all
the count can do is disagree with them. So it is gone, and the sentence keeps only the claim
that cannot decay:

| Property | Enforced as of `8a5c525` | By what |
|---|---|---|
| Two genuinely distinct frames occurred | **Yes, as a rejection** | `validate_paint_evidence` requires both finite and `raf2_ms > raf1_ms`, returning `PaintEvidenceRejected::frame_gate_failed` (#113, #115) |
| The paint entry is present at all | **Yes, as a typed rejection** | `Option<f64>` plus `PaintEvidenceRejected::missing_entry` — raised by our code rather than failing at Tauri's IPC layer before it runs (#112) |
| The paint value is the browser's entry, not a substitute clock read | **No — demoted to a diagnostic** | `frame_precedes_paint_diagnostic` prints and returns `Ok`. It is not a rejection because at a 200 ms tolerance it could not fire: the attack's own gap is bounded above by FCP, measured at 184 ms (F-406, F-393) |
| The frames were scheduled *after our mount* | **No, and nothing can.** | Not recoverable from these timestamps in Rust or anywhere else; frontend review only |
| `app/index.html` stays contentless, so the paint entry means *our* paint | **No** | Still nothing. `crates/serat-app/src/commands.rs:92` and `startup.rs:66` both say so in their own words: *"a property nothing in this crate"* enforces |

**The state this table recorded when it was written, kept because it is what the two-column
shape was for.** At #109 the conjunction was in force **nowhere**: `raf1`/`raf2` appeared in no
Rust file, `mark_first_paint` took `first_contentful_paint_ms: f64` alone, and the frontend had
been sending `raf1Ms`/`raf2Ms` since #108 with **Tauri silently dropping both**, because the
command had no parameter to receive them. **Both halves appeared to exist and nothing joined
them** — the most dangerous state a mechanism can be in, because every individual file reads as
though the work is done, and F-388's entire history is people reasoning correctly from a
plausible reading of one file. A single-column table headed *"Enforced by"* would have said
exactly that, in the document that makes the rule against it. Found by code-reviewer at #109.

**What changed, and it is not what the old column promised.** #112, #113, #114 and #115 landed
and the first two rows are now enforced as rejections. **The third was not enforced — it was
demoted**, on this ADR's own pre-committed condition, because F-393 measured FCP at 184 ms and
F-406 showed a 200 ms tolerance therefore cannot fire. **So "to be enforced by #112" resolved
to "yes, yes, no, and the reason for the no is a measurement"**, and a reader who took the
column as a plan would have concluded the opposite of the truth about the row that matters
most. Raised by product-analyst against PRD v1.3, checked against the tree rather than against
this register, and it is F-471's class in the document that hosts §Decision 6: **a column
headed with a task number is a promise with a date on it.** One-line re-check, which stays
true either way because it names the call rather than its verdict: `grep -n
'frame_precedes_paint_diagnostic' crates/serat-app/src/startup.rs`.

**Ruling (task #125): the guard is FCP-only, none of the three proposed tightenings is taken, and the one intervention with margin in it already exists and is set too low.**

tauri-app-dev verified against the body rather than the doc that `validate_paint_evidence`'s only rejection compares `raf1` and `raf2` **to each other** and never to the paint entry, so F-504's mutation — `fcp: 1, raf1: 0, raf2: 1` — is finite, strictly increasing and positive, and passes. They brought it here rather than patching it, which is why there is a ruling to make.

**First, the constraint that bounds every candidate and was not stated in any of them.** `mark_first_paint` receives three numbers from the webview. **No arithmetic relation among three caller-supplied numbers can be a guard against fabrication**, because the fabricator supplies all three: any rule expressible over `fcp`, `raf1` and `raf2` is satisfiable by choosing them to satisfy it. Everything in that family can catch *careless* fabrication — a hoisted mark, a stub, a constant — and nothing in it is a boundary. That is not an argument against a check; it is the label that has to travel with one, because F-107's whole class is guards trusted for something they never did.

**`raf1` strictly positive is rejected.** It kills `raf1: 0` and not `raf1: 1`, so it raises the cost of the mutation by one increment while reading as a fix. Its stated justification — a rAF timestamp of exactly zero is not reachable in a real page — is the *not reachable* reasoning F-406 was recorded for distrusting, and the proposer said so themselves before proposing it.

**Re-arming a paint-entry comparison is rejected, and their own data is why, in the direction they did not expect.** The honest `paint_evidence_ms − raf2_ms` band is **35.2–79.7 ms, median 50.2 over 48 traces** (#119), independently matching F-406's 51.4 / 56.7 / 63.4. F-504's fabrication produces a gap of **0** — so it fails on the **low** side, and a *lower* bound would catch what F-406's upper bound could not. It is still the wrong instrument: the margin between an honest floor of 35.2 ms and a threshold that must sit above 0 is under 2x, on a machine F-327 measures at 35% session-to-session drift. **That is a tolerance sized against the effect and never against the noise**, which is the defect F-406 exists to record, arriving with the sign flipped.

**Asserting `app/index.html` stays contentless is not an alternative to either and should be done anyway.** It closes F-396(b)'s dependency — the reason an FCP-only guard is sound at all — and it is static, deterministic, has no threshold and cannot fire on an honest run, which is what the other two candidates cannot say. **It does nothing about F-504**, because a fabricated call never consults the real paint entry, and treating the three as substitutes is the part of the framing to reject: one closes a live dependency, the others chase a fabrication.

**The intervention with real margin exists already and is set thirty-nine times too low.** `window_visible − window_created` is computed from **two host-stamped clocks** — the one quantity in this mechanism the webview cannot supply — and the budget job already rejects a trace where it is under **5 ms** (`MIN_PAINT_GAP_MS` in `bench/budget/src/trace.rs`, **enforced rather than documented — checked, because a floor that only a doc comment claims is the exact shape this ADR keeps recording**; one-line re-check: `grep -n MIN_PAINT_GAP_MS bench/budget/src/trace.rs`). Honest `S_shell` is **196.1 ms median / 229.2 p95** (#100), so a 5 ms floor catches only a mark rewired essentially *to* window creation, which is what §3.4 records it as being sized for. F-504's call arrives at module evaluation, comfortably past it. **Raising that floor is the change with room in it**, and the number it should be raised to is the one nobody has: `S_shell` under F-504's placement. **I am not setting a threshold I cannot derive** — that is the whole of F-406's lesson — so this is a measurement request rather than a table decision, and it belongs with whoever runs the mutation.

**Pre-committed, in the form that worked last time.** If the measurement shows a raised floor cannot separate honest placement from fabricated placement with margin against F-327's drift, **it stays a diagnostic and this table records that it cannot fire**, rather than being armed because arming feels safer. That is ADR-0014's own condition on F-406, applied before the data exists rather than after — and if it turns out unreachable, **that is this table changing and not a test failing.**

**The measurement was taken and the condition fired, by a wider margin than it asked for.** `S_shell` honest, n=53 this session: **min 125.0, p05 137.1, median 187.1, p95 317.4**. Fabricated, n=5 fresh: **min 104.6, median 126.7, max 132.7**. **Three of the five fabricated runs sit above the honest minimum**, so catching all five needs a floor above 132.7 — which rejects honest runs at 125.0 and sits inside the honest p05. The condition asked whether the two could be separated *with margin against drift*; they cannot be separated **at all**, before drift is considered. `MIN_PAINT_GAP_MS` stays at 5 ms, and this paragraph is the record that raising it was considered and refused on evidence. Positive control per F-470: `paint_evidence_ms: 1.0` in all five runs, bundle edited then `cargo build --release`, so the mutation demonstrably reached the binary.

**And the constraint above is wrong as I stated it — it reaches one level further, which tauri-app-dev established rather than reported.** I wrote that no arithmetic over three *caller-supplied* numbers can guard against fabrication, and offered `S_shell` as the escape because both its clocks are host-stamped. **The host stamps the arrival; the caller chooses when to arrive.** So `S_shell` is not caller-supplied and is still **caller-determined**, and a fabricator who wants to land inside the honest band simply delays the fake call. The sampled fabricated range is therefore not the quantity that matters — the *achievable* one is, and this run landed inside the honest band without trying to. **The correct statement is that nothing observable at `mark_first_paint` can discriminate, because everything available there is either supplied by the caller or timed by them.**

**That is why the static check is not merely also-worth-doing: it is the only member of the family that can work.** `app/index.html` staying contentless is a property of the source that no runtime caller participates in — it is checked without observing the call, which is exactly what escapes a constraint about what callers control. The three candidates were never three options; **one of them was outside the impossibility and the other two were inside it**, and that is the shape of the ruling rather than a preference among them.

**`MIN_PAINT_GAP_MS`'s own doc is already scoped correctly and should stay that way** — *"the gap below which `window_visible` is assumed to be **mismeasured**"*. Raising it would repurpose a mismeasurement detector as a fabrication guard it structurally cannot be, which is how a check comes to be trusted for something it never did.

**Bounds, and one is load-bearing.** One machine, n=5 fabricated, and a **reconstruction** of F-504's placement rather than their diff. The reconstruction landed **below** the honest band and F-504's landed **above** it. That is not a discrepancy to resolve — **it is the result**: the gap is placement-dependent and unbounded in both directions. A further run against F-504's exact placement was offered and **declined**, because one more sample of an unbounded quantity cannot bound it, and asking for it would be seeking a number that changes nothing.

**Which call takes the mark, since #125's failure message now sends readers here for the answer.** `mark_first_paint` is first-wins — `if marks.window_visible.is_some() { return Ok(false) }` — and #125 pins the call sites at exactly one. **With one caller, first-wins is not a rule; it is a description.** The question only becomes real when a second legitimate site is proposed, and this is the answer to have ready before that, rather than after.

**First-wins is not to be adopted as the tie-break by default, and the reason is what it would silently make the mark mean.** `window_visible` is supposed to name *the first composited frame of our shell*. Under two call sites, first-wins makes it name *whichever site executed earlier*, which is a property of code ordering rather than of painting — and the two coincide only while one site exists. **F-504 is that divergence arriving as an attack; a second honest call site is the same divergence arriving as a refactor.** The structure is identical and only the intent differs, which is precisely why the guard cannot tell them apart (F-505).

**So a second call site is not a tie to break. It is a change to what `S_shell` measures**, and it requires deciding which interval the term names before the site lands: the earliest composited frame of any shell content, or the frame that carries paint-entry evidence. Those are different quantities, both defensible, and **no runtime check can choose between them** — so the choice is this table's and has to be made in advance. Until it is, **what keeps the question from being answered by accident is a *pair* of checks, and neither half would do it alone** — which is a better description of their value than *counts the call sites*.

**The pair, and it is complementary by construction rather than by intention (F-510).** The Rust bundle-string count catches a literal hand-edited into the committed bundle, **which vitest never sees**; the front-end check catches the **wrapper route** — a second call written as `markFirstPaint({...})` — **which a string count cannot see**, and which was live and verified live by three parties independently before it was closed. **This paragraph credited the count alone when it was first written**, hours before that evidence existed, and the count by itself demonstrably left the route open with the literal count still reading 1 and the suite green. Crediting one half of a pair to a property only the pair has is how a check comes to be trusted for more than it does, and this ADR has spent its length on that shape.

**One thing recorded rather than blessed.** First-wins was never chosen as a policy; it is the shape of the `is_some()` early return, and F-504 is what turned an implementation detail into a load-bearing one. Writing that down is worth more than ratifying it, because the next person to read the early return will otherwise take it for a decision somebody made.

**Ruling (task #128): what the contentless check reads, what *contentless* means, and why it is not the family F-377 condemned.**

**It reads the built artefact, not the source.** `tauri.conf.json` sets `frontendDist: "webview"` with no `beforeBuildCommand`, so `crates/serat-app/webview/index.html` is compiled into `serat.exe` and `app/index.html` is a file the binary never opens (F-470). A check on the source would be checking a document that cannot paint. The two also differ **legitimately** — the source carries `<script type="module" src="/src/main.ts">` in the body and the built copy does not — so a source-side check would additionally have to model Vite's transform to know which differences are real. **The built copy is the ground truth and needs no model of the build.** A source-side check at authoring time gives **earlier feedback** and is not the guarantee; if only one is built, it is this one.

**That sentence used to call the source-side check *the second half of an F-510 pair*, and so does the stylesheet paragraph below — two different second halves, which is an ambiguity an implementer hit rather than a reader.** Worse than ambiguous: for *this* property the F-510 analogy does not hold. In F-510's case the two checks caught **genuinely different attacks** — a literal hand-edited into the bundle, and a call routed through the wrapper — each invisible to the other. **Contentlessness is not like that.** If the source gains a skeleton and nobody rebuilds, the shipped artefact is still contentless and **the property still holds**, because the binary embeds the built copy. So a source-side twin catches the same markup **earlier**, not *additionally*. **The stylesheet check is the real second half** — it closes a route the markup check structurally cannot see — and the source-side twin is a convenience whose value is the length of the feedback loop. Ranking them the same would have spent vue-frontend-dev's time on the weaker one first.

**Contentless is defined as an allowlist, not as an absence.** *Renders nothing before scripts run* is the property we want and is not statically decidable — CSS alone can paint a `::before`, a background, a sized empty box. So the check does not attempt it. **The body may contain the mount point and script elements and nothing else**; anything further fails. That is deliberately stricter than the property, and the strictness is the point: **widening it is then a decision somebody takes, in the same shape as ADR-0012's five-command grant**, which F-509 names as the model because it states a test a reader can resolve against a file rather than an outcome they must infer.
**Contentless is defined as an allowlist, not as an absence. The body may contain the mount point — `<div id="app"></div>` — and nothing else**; anything further fails.

**Comments in the body are not allowlisted either, and that is deliberate rather than an oversight** (tauri-app-dev, who decided it inside the ruling and flagged it rather than burying it). A comment cannot paint, so the *property* permits one — but the allowlist is deliberately stricter than the property precisely so that widening is a decision somebody takes, and carving out an exception for a case that has not arisen is the quiet widening that reasoning forbids. Verified: neither the built body nor the source body contains a comment today. If one ever appears legitimately, red is correct and the exception gets made with a real case behind it. **This is the same call as the script clause below, and it goes the same way for the same reason.**

**That clause first permitted script elements too, and it should not.** code-reviewer checked the shipped artefact and the body has **no script in it**: Vite emits the module script into `<head>`, so the permission was for something that is not there. **An allowlist that permits what the artefact does not contain is looser than the artefact for no benefit** — and if a future Vite does emit one into the body, red is the correct outcome and widening is then the decision this shape exists to force. Verified rather than reasoned: the built body inner is exactly `<div id="app"></div>`. That is deliberately stricter than the property, and the strictness is the point: **widening it is then a decision somebody takes, in the same shape as ADR-0012's five-command grant**, which F-509 names as the model because it states a test a reader can resolve against a file rather than an outcome they must infer.
**This paragraph first justified the allowlist by claiming the real property is not statically decidable, and gave three examples of which two were wrong.** A background **colour** and a sized empty box are *First* Paint, **not First *Contentful* Paint** — tauri-app-dev's correction, and it matters because a check written against *any styling* would have been wrong rather than merely strict. The property is about **contentful** paint, so the vectors are bounded rather than open: text nodes, image-bearing elements, background images, and CSS **generated content**. The first two the markup allowlist covers outright.
**Which leaves the stylesheet, and this ruling has to settle it rather than leave it implied.** `index.html` links `shell.css`, and a `::before { content: "Loading" }` would paint before any script runs **without touching the markup at all** — so a markup-only check is not the property, and would be trusted for it.

**And the ordering that makes that route live rather than theoretical is worth stating, because it is not obvious from either file** (code-reviewer): the stylesheet is a `<link>` in `<head>` and the bundle is a **module** script, therefore deferred — **so the stylesheet applies before `boot.js` executes**. A designer adding a skeleton in CSS gets contentful text painted before Vue mounts, with the body still exactly the mount point. **That is now the cheapest remaining route and the one taken innocently**, which is what makes the stylesheet half of the pair the one to build second rather than the one to defer. **The vector is bounded and small**: generated content or a background image can only paint pre-script on an element that exists pre-script, and there are three — `html`, `body`, `#app` — plus the universal selector. So the stylesheet half is **narrow and checkable**, not a CSS parser.
**It is the second half — the one that adds coverage rather than speed — and it landed *with* the first, so the condition attached to landing them apart never came due.** That condition was: until the stylesheet half lands, this table says the markup check is not the whole property, because the failure this ADR keeps recording is a half credited with the whole.

**Both halves are built (task #128) and each fails only on its own vector**: `<div>Loading…</div>` reddens the markup half and leaves the stylesheet half green; `body::after{content:"x"}` does the reverse. **That is the F-510 pair property demonstrated rather than asserted**, and it is worth more than the condition it made unnecessary — had only the markup half been built, the second row would have been green with a contentful skeleton painting. code-reviewer had already reproduced exactly that: `body::before{content:"Loading…"}` shipped in `shell.css` with the built body still exactly the mount point and the suite green. **The stylesheet half closes that route**, and the two vectors it covers — CSS generated content and background images — are the two the markup half structurally could not see. So each of the four *named* contentful-paint vectors is now checked by one half or the other.

**What remains is smaller and different, and it needs stating precisely so nobody reads *contentless is asserted* as *contentless is proven*.** The pair covers the vectors we **enumerated**; *renders nothing before scripts run* is still not statically decidable, so what is unchecked is **whatever we failed to enumerate** — a `list-style-image`, a `border-image`, an SVG arriving by a route neither half names. **F-396(b)'s dependency is checked against known routes rather than discharged**, which is a weaker claim than *closed* and a much stronger one than it had this morning.

**The ruling against pinning that residual was validated faster than expected.** The argument was *pin a boundary you mean to keep, not one you mean to remove, because closing it turns the pin red and red will read as a regression*. The gap closed within the hour — a negative control written when it was offered would already be red today, meaning *good news*, in a suite where red means regression everywhere else.

**And one hazard nobody anticipated, found by the implementer and worth carrying because it would have been silent**: the minifier rewrites `body::after` to **`body:after`** in the shipped sheet. The check matched anyway because it looks for the *declaration* rather than the selector — **but a stricter form matching on `::` would have missed the real artefact while passing its own unit tests**, which is F-470's shape inside a single file. Anyone tightening the selector matching has to read the built sheet, not the source. And the trap travels with the ruling rather than being rediscovered: **match `content` as a property, not as a substring** — `justify-content` is in `shell.css` today and has already cost two people a false positive. code-reviewer's matcher is the form to use, because it **anchors on position rather than enumerating exceptions**: `content` preceded by start-of-line, `;`, `{` or whitespace. That excludes `justify-content`, `align-content` and `place-content` without naming them, so it does not decay as CSS gains new `*-content` properties — **a blocklist would**, and a blocklist that decays silently is the shape this ADR keeps recording.

**Why this is not the structural-check family F-377 condemned, which is the objection to answer before it is raised.** F-377's bundle check asserted that a *behaviour* — a mark taken inside a double `requestAnimationFrame` — could be inferred from *text*, and it cannot: the substring survived every mutation that mattered. **This check asserts a property of the text itself.** What markup is present in a document is exactly the question text can answer, and it is the whole of the property, not a proxy for one. **A shallow check standing in for a semantic property is F-377; a shallow check asserting a shallow property is just a check** — and the reason this one is worth having at all is F-505: nothing observable at `mark_first_paint` can discriminate, so the only place left to stand is outside the call.

**It runs under `cargo test --workspace`, not in CI**, and that is a requirement rather than a convenience: CI produces no jobs at all (F-384), so a check that lives only there does not run. `crates/serat-app` already opens the committed bundle by literal name, so the check has a home that executes on every developer's machine today.

**Pre-committed, because this table has been wrong about a guard before.** If the check cannot be made to fail — if the build emits a body the check accepts whatever the source says — **then it is not a guard and this table records that**, rather than shipping a green assertion over an untested property. The mutation that must go red is a `<div>Loading…</div>` added to `app/index.html`, built, and the test run: **red is the requirement, and a green run there is the finding.**

**The third row is the boundary worth stating rather than papering over.** Whether the frames
were scheduled after Vue mounted is not recoverable from three numbers, so `serat-app` cannot
enforce it however the arithmetic is arranged. What it can enforce is the **internal
consistency of the evidence**, which rows 1 and 2 do. Claiming more than that for a runtime
guard is how a check comes to be trusted for something it never did.

**The fourth row gets a static check and deliberately not a runtime one.** A bound on
`raf2_ms − first_contentful_paint_ms` would technically detect a skeleton — a large gap means
the paint preceded the frames — but it is a magnitude argument, and this section has already
ruled that magnitude is weaker than evidence which cannot be fabricated. Worse, the threshold
would be indistinguishable from a slow mount. The property is static, so it is checked
statically.

**And row 2 lands only after its margin is measured.** `first_contentful_paint_ms <= raf2_ms`
catches the realistic mutation the others miss — reading the clock instead of the entry, which
lands after `raf2` — but its false-positive risk depends on when the frames are scheduled
relative to DOM insertion, and nobody has measured that margin. **It becomes a rejection if an
honest run shows the ordering holding with room, and a reported diagnostic if it does not.**
A guard that can fire on an honest run is worse than no guard, because it gets disabled
(F-107).

The resolution is **conjunction rather than replacement**: keep the gate that ties the mark
to our shell having rendered, and add the paint entry as the evidence that cannot be
fabricated. Each covers the other's blind spot — a frame gate alone is forgeable, which is
F-377; a paint entry alone is satisfiable by a skeleton, which is the above.

**tauri-app-dev on the basis of the phase-table figures: it cannot be reconstructed, and
that absence is the answer.** Asked twice, and the honest reply was that they have no record
of taking those measurements — the cold-start phase table predates or sits outside anything
the session that would have produced it can reach. **Treated as a finding rather than as a
gap to be filled with a plausible guess**, which is what §Decision 6 exists to prevent.

**What is known and what is not, kept separate because they are not the same.** §3.4 records
that the table is twenty spawns per figure, release build, scanner off. **What no longer
exists is whether each cell is a median or a p95, whether all seven rows came from one
session on one set of spawns or were composed across runs, and what the machine's contention
was** — the three things #83 made mandatory afterwards and F-327 showed are worth 35% of a
number.

**So the table may be quoted for its *shape* and not for its *values*, and this ADR quotes it
that way from here.** An 865 ms term beside a 5 ms term is not a claim any statistic decides;
which terms dominate, and therefore where the split's boundaries belong, survives the missing
basis entirely. **Every figure in this document is provisional against task #100 for exactly
this reason, and always was** — §Decision 5 sets the budget from `s95` measured after #98
rather than from anything above. The structure was written so the figures slot in, and this
is the first case where that mattered rather than being a precaution.

**What this must not become is a precedent for quoting them anyway.** These figures travelled
into three documents before anyone asked what they were. That they can no longer be attested,
and that nobody noticed until the question was put twice, is a stronger argument for
§Decision 6 than the rule's original justification was.

**Why they were the ones asked, and what is left of it.** tauri-app-dev was consulted on two
things: they measured the table this ADR partitions, and §Decision 2's caveat about
`generate_handler!` is a question only they have watched move. The first is answered above and
the answer is that it cannot be reconstructed. The second is now partly answered by something
other than a consultation — #100 measures `P_builder` at **13.3 ms median / 16.9 ms p95**, and
it stays watched rather than budgeted. *(Consulted 2026-09-08; answered on the basis question
2026-09-08, and this paragraph corrected 2026-09-09, where it had gone on reading "awaiting
their answer" beneath the answer itself.)*

**Architect's closure of the basis question, and it is a bound rather than a blank
(2026-09-09).** The conclusion above stands — the basis cannot be reconstructed from what was
recorded — and part of it can be recovered from arithmetic the table itself carries, which is
worth more to a later reader than the bare word *unreconstructable*. The two rows this ADR
calls `S_pre` and `S_setup` sum exactly to the `S_main` figure §3.4 records separately (4.7 +
0.5 = 5.2 at two notes; 5.0 + 0.5 ≈ 5.6 at twenty thousand), and that figure is stated beside a
**p95** of 9.4 and 9.6 — so those cells are a central statistic and p95 is what they are not.
The 20,000-note column's three platform rows compose to 965.9 ms, which is the **~966 ms this
ADR used to quote**: that figure had the table's basis and never a better one. **The honest
form of the basis is: a central statistic, median or mean unknown, over twenty spawns per
figure, release build, scanner off, machine state unrecorded, composition across runs not
excluded.**

**And one consequence that had not been drawn: the table may not be read across, either.** Its
two columns are a *blocked* two-vault comparison, which §3.4 later discredits for exactly this
comparison because the within-arm spread is about five times the effect. So neither the 753.7 →
865.4 ms growth nor the 4.7 → 5.0 ms flatness is evidence about vault size. The flat reading is
the load-bearing one — it is where "nothing on the critical path is proportional to the size of
the vault" gets its only direct measurement — and it is now carried as a bound rather than as a
demonstration until §3.4's proportionality check runs on the Serat term (task #118's ruling),
which puts it inside a paired interleaved design for the first time.

**The owner ruled the frame** — re-baseline with the webview term rather than open C-07,
and as a split rather than a slackening — on 2026-09-08. The grounds are quoted in
§Context. This ADR records the shape that ruling takes, not the ruling.

## Tested against

**Task #100, and it has now run — this field said "none yet" for a day after its own case
landed.** The partition was derived from §3.4's phase table, which is the same body of
measurement it is meant to organise — a rule derived from one case and checked against that
case has not been checked, which is exactly what ADR-0013's own Tested against field had to
admit about itself. #100 is the independent case, and it is independent in the way that
matters: a different build (post-#98), a different sitting, and a harness that computes the
terms rather than a reader who composes them.

Three specific things #100 would confirm or break, with what it returned:

1. **Does Σ(terms) equal `t0_to_vault_usable_ms` on every run?** **Yes, and the answer is
   worth less than the question assumed.** #106 asserts the identity, and test-engineer's
   objection above is the reason it proves less than it reads: with all seven marks required
   and monotonicity checked separately it is a telescoping sum, true of any seven numbers, so
   what it guards is the harness's field-to-term mapping — proven by breaking that mapping
   deliberately, not by an honest run ever failing it.
2. **Is `S_front` still ~138 ms with Vue 3 and CodeMirror 6 mounted, or does it move by an
   order?** **It moved, and not by an order: 197.6 ms median / 230.4 ms p95**, against
   `S_main`'s 3.4 / 4.0. So §Decision 4's allocation survives its own test, and it turns out to
   have been the load-bearing decision — `S_front` is 98% of the Serat term, so a single sum
   would have hidden the entire front end behind four milliseconds of quiet Rust.
3. **Does `s95` × 1.3 fit under 400 ms?** **Yes: 233.8 × 1.3 = 303.9, leaving [304, 400] and
   96 ms of room.** The empty-interval clause still has never fired, so it remains the clause
   most likely to be argued with when it does.

**A fourth thing #100 returned that this list did not ask for, and it is the strongest result
in the document** (§Decision 7, F-407): the aggregate scalars move ~19% between attested
batches while `S` moves ~8% on p95. The term this ADR made binding is measurable and the term
it declined to budget is not, which defends §Decision 3 better than §Decision 3 did.

**What is still untested here is the ratio, not the level.** §3.4's proportionality check has
never been run against the Serat term — task #118's ruling moves its subject there — so
"nothing on the critical path is proportional to vault size" remains supported only by a
blocked two-vault comparison in the phase table, which §Objections above now records as
inadmissible for that reading.

The second case that would test this — and the more valuable one — is **the first
platform that is not Windows**, because the attribution rule claims to be
platform-independent and has never been applied anywhere else.
