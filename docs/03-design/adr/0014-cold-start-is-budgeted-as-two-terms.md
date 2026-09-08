# ADR-0014: Cold start is budgeted as two terms, one of which is not ours

**Status:** proposed · **Date:** 2026-09-08 · **Serves:** NFR-PRF-01, C-06, C-07, NFR-PRF-08

**Read this before citing it: the three consultations in §Objections were still outstanding
when this was written, and if it merged in that state they were outstanding at merge.** The
shape below is settled; two of the answers can still move §Decision 4's allocation and the
placement of one boundary. A reader who skims to §Decision should know that much without
reaching the bottom.

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

**Three different figures for the webview term appear within a few lines here — 619 ms,
753.7–865.4 ms, and the ~966 ms platform term in §Consequences — and they are three
different measurements, not a drift.** F-86's 619 ms is the median of that phase on the run
that first measured the miss; 753.7–865.4 ms is the same phase on the later run with marks
placed for the purpose, at two notes and at twenty thousand; ~966 ms is the whole *platform
term* at twenty thousand, so it sums three phases rather than naming one. Stated because
§Decision 6 of this ADR requires the basis to travel with the figure, and this paragraph is
the one place in it that quoted numbers without one. F-94 and F-106 both landed on exactly
this shape of error.

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
   so it stops being able to catch a regression. It also books ~140 ms of Serat's own work
   into a number dominated by a platform term, so a doubling of the front end's startup
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
`Σ(terms) = t0_to_vault_usable_ms` is checkable on every single run — **checkable in
principle today and in fact only after task #106**, and the tense matters. F-381 caught this
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
  because ~29 ms of builder against a ~966 ms platform term is not where anything hides,
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
the one term §3.4 can measure cleanly — 5.2 / 5.6 ms median, 9.4 / 9.6 p95, across a vault
ten thousand times larger. `S_front` sits at ~121–138 ms pre-#98 and is where the editor
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
it is what went wrong the first time."* For this split the basis is five clauses, and all
five are conditions **under which the number is better than a user's**:

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

Clauses 1 and 2 push the number the same way. **A re-baseline attested on this basis is a
bounded claim and must be written as one.** This bound is why NFR-PRF-01 was deliberately
*not* re-baselined earlier (F-96); the owner has ruled to proceed, so the bound is recorded
rather than resolved.

## Consequences

**Positive.** Serat's contribution to cold start acquires a budget that can actually fail:
~140 ms of measurable work, currently invisible inside a number dominated by a platform
term, becomes the thing someone is held to. The partition is checkable per run rather than
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
  and `P_webview` is 78% of the wait. The mitigation is §Open design questions, not this
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
  (`crates/serat-app/src/startup.rs:189`), and §3.4 measures `painted → usable` at
  4.7–7.5 ms, so `painted` is **not** the term that settles the maximum. Moving it earlier
  by Δ therefore leaves `t_usable` untouched: `S_shell` shrinks by Δ, `S_settle` grows by
  exactly Δ, and every sum is invariant. **An identity over contiguous intervals constrains
  data integrity — a missing mark, a clock step — and says nothing about whether a mark
  fired at the right event.** Moving an interior boundary redistributes time between two
  adjacent terms and is exactly what the identity cannot see. **So task #105 does not get
  simpler**, and the post-composite evidence it is built around is doing work nothing else
  does.

  **What the partition does buy is a diagnostic, and it is a sharp one.** Under the
  mutation `S_settle` grows from a measured 4.7–7.5 ms to roughly Δ, bounded by F-86's
  ~100 ms from window build to first painted frame — a 13–21× anomaly in the smallest and
  most stable term in the table, in a quantity today's four-mark harness cannot compute at
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
does.** The platform term at 20,000 notes is ~966 ms with zero Serat code in it — 2.4×
the whole budget. No allocation of the Serat term changes that.

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
measuring the same seven-term partition on the other two webviews. That is one row in
§Open design questions and it should stay there until someone has run it.

## Objections recorded

**test-engineer, consulted on whether the split is assertable with the harness as it
stands, and on `S_front`'s noise floor.** Their answer belongs here verbatim rather than
in summary, because the conditions in §Decision 5 are written against measurements that are
theirs. The question put to them was specifically whether a budget on `S_front` is
measurable at a useful tolerance, since a budget whose noise exceeds its own margin is not
a gate (F-327) — and if it is not, §Decision 4's allocation is the part that gives way.
*(Consulted 2026-09-08; awaiting their answer, which lands here before this ADR moves to
accepted.)*

**vue-frontend-dev, consulted because `S_shell` is theirs and #98 is inside it.** The
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
document renders nothing before the framework mounts. **They already do not coincide here
and by design:** `crates/serat-app/webview/index.html` carries the entire shell as static
markup — wordmark, vault label, editor, status bar — because §3.4 requires the editor to
accept keystrokes from the first paint, *"a property of the document rather than a
promise"*, and that file instructs #98 to preserve it. So FCP fires on the static skeleton,
**before Vue and CodeMirror mount**, and a mark gated on FCP alone would move `t_visible`
earlier by the whole cost of the mount — which is `S_shell`, the term this ADR makes
binding. **That is mutation B's exact effect arriving legitimately, through the fix for
mutation B**, and F-386 says the sum identity would not see it.

The resolution is **conjunction rather than replacement**: keep the gate that ties the mark
to our shell having rendered, and add the paint entry as the evidence that cannot be
fabricated. Each covers the other's blind spot — a frame gate alone is forgeable, which is
F-377; a paint entry alone is satisfiable by a skeleton, which is the above.

**tauri-app-dev, still awaited on the basis of the phase-table figures.** They measured the table this ADR partitions, and §Decision 2's caveat
about `generate_handler!` is a question only they have watched move. *(Consulted
2026-09-08; awaiting their answer.)*

**The owner ruled the frame** — re-baseline with the webview term rather than open C-07,
and as a split rather than a slackening — on 2026-09-08. The grounds are quoted in
§Context. This ADR records the shape that ruling takes, not the ruling.

## Tested against

**None yet, and the case that would test it is task #100.** The partition is derived from
§3.4's phase table, which is the same body of measurement it is meant to organise — a rule
derived from one case and checked against that case has not been checked, which is exactly
what ADR-0013's own Tested against field had to admit about itself.

Three specific things #100 will either confirm or break, and whoever runs it should record
which:

1. **Does Σ(terms) equal `t0_to_vault_usable_ms` on every run?** If it does not, a mark is
   missing or a boundary is wrong, and §Decision 1's central property does not hold.
2. **Is `S_front` still ~138 ms with Vue 3 and CodeMirror 6 mounted, or does it move by an
   order?** The split's value rests on the Serat term being small enough to budget usefully
   and large enough to be worth budgeting. #98 is the first change that tests both.
3. **Does `s95` × 1.3 fit under 400 ms?** §Decision 5's empty-interval clause has never
   fired and is the clause most likely to be argued with when it does.

The second case that would test this — and the more valuable one — is **the first
platform that is not Windows**, because the attribution rule claims to be
platform-independent and has never been applied anywhere else.
