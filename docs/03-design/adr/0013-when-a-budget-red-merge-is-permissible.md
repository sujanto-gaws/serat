# ADR-0013: When a merge may proceed with a performance budget red

**Status:** accepted · **Date:** 2026-09-07 · **Serves:** C-06, NFR-PRF-04, NFR-MNT-01

## Context

C-06 says the performance budgets are binding and enforced in CI. It does not say what
happens when a budget is red for a reason the change in front of you did not cause, and
task #13 met that case: a P1 correctness fix — every incremental save nulled all inbound
links to a note and nothing re-resolved them (F-244, FR-NAV-04) — landed while NFR-PRF-04
was breaching.

**An unwritten precedent is the worst available outcome, and that is why this exists.** It
will be cited — *"we merged #13 with NFR-PRF-04 red"* — and the conditions that earned it
will not travel with the citation, because a register row is read by whoever goes looking
for it while a constraint is read by everyone. An exception living only in F-375 makes
C-06 quietly weaker than it reads.

**This interprets C-06; it does not amend it.** Constraints live in CLAUDE.md and are the
lead's to open, and the lead opened this one and ruled that it be written narrowly. The
conditions below are deliberately hard to satisfy. They are *conjunctive*: meeting four of
five is not meeting the rule.

## Options considered

1. **Leave it case-by-case.** Rejected: it does not stay case-by-case. The first citation
   turns it into a precedent whose reasoning has been lost, and the constraint decays
   without anyone deciding that it should.
2. **Treat C-06 as absolute — no merge while any budget is red.** Rejected on the case
   itself. NFR-PRF-04 was red on `main` independently of #13, and parse alone at 31.3s
   already exceeded the whole 30s budget on that machine, so *no* writer-side change could
   have brought it green. Under this option a correctness bug stays in the tree,
   indefinitely, for a reason unrelated to it — enforcing C-06's letter against its
   purpose.
3. **A narrow, conjunctive exception with the conditions stated.** Chosen.

## Decision

A merge may proceed with a performance budget red **only when all five hold**:

1. **The breach is not caused by the change**, established by a **same-session A/B** —
   baseline and branch measured minutes apart on one machine — and never by comparing
   against a figure recorded in another sitting. F-375 is why: the same 20k fixture read
   parse at 4.1s in one session and 31.3s in another, and the *share* moved 3.4x with
   nothing in the tree moving (see §Open design questions, and #84).
2. **The change's own residual is at or below the noise floor, and the noise floor is
   stated rather than assumed.** For #13: +5.9s at n=1 where the harness scored the machine
   at 37.9% spread.
3. **The baseline is red on its own, for a reason that is named and owned by a live task.**
   For #13: parse alone at 31.3s exceeded the whole budget, raised as #83. **"The budget
   was already red" is not sufficient on its own** — without this clause C-06 decays to
   nothing the first time a budget goes red and stays red, so the baseline breach must be
   *owned* and not merely observed.
4. **The change is a correctness fix**, not a feature or an optimisation. A feature can
   wait for a green budget; a correctness bug costs something every day it stays.
5. **The measurement is recorded with its conditions** — machine state, iteration count,
   and whether the harness called the run INDETERMINATE — so a later reader can tell what
   was actually established rather than what was concluded.

**What the merge then owes.** Condition 3 names a task; that task settling changes what has
been established. #13 merged on the understanding that #83 settles which parse figure is
representative, and this ADR first wrote the trigger as: *if #83 concludes the 31.3s reading
was real rather than contention, #13's residual must be re-examined against a green
baseline.* A budget-red merge is a deferral with a named creditor, not a discharge.

**That trigger did not fire, the deferral survived anyway, and the ADR was wrong about how.**
#83 concluded the opposite — parse reads a 3332ms median at `concurrent_build_processes = 0`,
1.3% spread — so the named condition was not met and #13 stays merged on the grounds it was
merged on. But #83's finding undermines the *instrument* rather than the verdict: the A/B that
cleared #13 was itself taken on a machine now known to be heavily contended. Both halves ran
under that contention, which is exactly what makes a same-session A/B admissible, and it is
still not evidence about a quiet machine. **The direction is not neutral.** Under contention
`producer waited` was 1–3%, because CPU starvation made the producer the constraint and the
writer never became one; a low-contention run showed it at 10,496ms, 32%. #13's cost is
writer-side, so it was measured under precisely the conditions least able to see it, and a
quiet-machine A/B could show the residual **larger**. Task #87 carries the deferral now.

**The lesson for this rule, and it is the first thing its Tested against field asked for.**
The trigger was written as *one task's conclusion* rather than as the condition that
conclusion was standing in for. What condition 2 actually needs is a residual measured on an
**attested** machine; #83 concluding "contention" satisfies the letter of the trigger while
leaving that need unmet. **A creditor should be named by the condition it discharges, not by
the task expected to discharge it** — tasks close, get superseded, and conclude things their
author did not anticipate, and a deferral pinned to one of them can look settled while the
question it stood for is still open.

## Consequences

**Positive.** The exception is explicit, so it can be met or failed rather than argued.
Condition 1 forces a same-session A/B, which is the measurement discipline F-327 has been
arguing for and which produced the only attributable figure in #13's case. Condition 3 makes
a standing red impossible to use as permanent cover.

**Negative.** It is more work than citing a precedent, deliberately. A same-session A/B on
the 20k fixture is not cheap, and conditions 2 and 5 require the harness to report a spread
and an indeterminacy verdict rather than a bare number.

**Risks.** Condition 4's "correctness fix" is the softest of the five and the one most
likely to be stretched; if it starts covering optimisations described as correctness, that
is the signal to tighten it. And the rule is only as good as the harness's noise floor: if
that number is wrong, condition 2 passes things it should not.

## Objections recorded

**test-engineer answered, split by condition because they are not equally checkable.**
*Condition 2 (residual stated against a stated noise floor): yes, on every run, with no
work needed.* `Outcome::spread_pct()` and `Outcome::determinate()` (→ `INDETERMINATE` in
`to_report()`) are computed and printed unconditionally, not opted into — every result this
harness has ever produced already carries the evidence condition 2 asks for; #13's citation
of "37.9% spread" and "two earlier branch-only re-runs were INDETERMINATE" was reading fields
that exist on every `Outcome`, not a special measurement taken for the occasion. *Condition 5
(recorded with machine state and iteration count): yes, and stronger since #83.* `Outcome`
already carries `Environment` and profile on every report; #83 added `cpu_load_percent` and
`concurrent_build_processes` to that same `summary()` line, so a same-session A/B taken today
states not just that the machine agreed with itself but whether anything else was building on
it — the exact gap #13's own citation could not close (§What the merge then owes: "#83's
finding undermines the *instrument* rather than the verdict"). *Condition 1 (same-session
A/B): not checkable by the harness at all, and it should not be read as a gap in the harness — it
is a property of how two jobs were run, not of what either one measured.* `Outcome` carries no
timestamp; nothing links two separate `to_report()` calls into one A/B except a human or agent
running both within one sitting and saying so in the prose that cites them, the way #13's
commit message did. A tool cannot attest its own proximity to another tool's earlier run — the
same reason F-332's calibration yardstick was tried and rejected for measuring the *machine's*
drift is why nothing here could measure the *run's* adjacency either. **So the rule needs no
amendment on checkability**: two of its three harness-facing conditions are already satisfied
by fields every run produces, unconditionally, and the third was never a harness question. What
condition 1 actually needs — recorded because #96 will need it too — is the discipline
CLAUDE.md's measurement-window convention already supplies: one machine, one sitting, nobody
else building.

**The lead ruled the shape** — narrow rather than general, conjunctive rather than a
balancing test, and written rather than left to precedent — on the ground that constraints
are theirs to open and that an implicit exception is the failure mode this project keeps
recording in other forms.

## Tested against

**One case, and it is the case that produced the rule rather than an independent one — which
is the weakness to state plainly. It has since been tested once, sooner than expected and not
by a merge: #83's conclusion exercised the deferral clause and found the trigger too narrow
(see §What the merge then owes). The five conditions themselves are still untested.** Task #13 met all five: same-session A/B (main 54,826ms
vs branch 60,714ms, parse 31,257 vs 31,316ms, identical); residual +5.9s at n=1 against a
37.9% machine spread; baseline red for a named reason owned by #83; a correctness fix under
FR-NAV-04; and the measurement recorded with its conditions, including that two earlier
branch-only re-runs were INDETERMINATE with samples straddling the limit.

A rule derived from one case and tested against that same case has not yet been tested. The
case that would test it is **the next budget-red merge somebody proposes** — particularly
one where a condition is arguable rather than clearly met, since conditions 3 and 4 are
where this will be pushed. Whoever meets that case should record which condition was
hardest to satisfy, and whether the honest answer was to fail the rule or to widen it.
