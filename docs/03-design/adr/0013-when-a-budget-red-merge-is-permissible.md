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
representative. **If #83 concludes the 31.3s reading was real rather than contention, #13's
residual must be re-examined against a green baseline** rather than treated as settled by
F-375. A budget-red merge is a deferral with a named creditor, not a discharge.

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

**test-engineer, consulted on whether conditions 1, 2 and 5 are checkable with the harness
as it stands** — sent at the time of writing; their answer belongs here and this section
will carry it verbatim rather than in summary. The conditions were drafted from F-375's
measurements, which are theirs, so a reading that they cannot be routinely satisfied is a
reason to amend the rule rather than to waive it.

**The lead ruled the shape** — narrow rather than general, conjunctive rather than a
balancing test, and written rather than left to precedent — on the ground that constraints
are theirs to open and that an implicit exception is the failure mode this project keeps
recording in other forms.

## Tested against

**One case, and it is the case that produced the rule rather than an independent one — which
is the weakness to state plainly.** Task #13 met all five: same-session A/B (main 54,826ms
vs branch 60,714ms, parse 31,257 vs 31,316ms, identical); residual +5.9s at n=1 against a
37.9% machine spread; baseline red for a named reason owned by #83; a correctness fix under
FR-NAV-04; and the measurement recorded with its conditions, including that two earlier
branch-only re-runs were INDETERMINATE with samples straddling the limit.

A rule derived from one case and tested against that same case has not yet been tested. The
case that would test it is **the next budget-red merge somebody proposes** — particularly
one where a condition is arguable rather than clearly met, since conditions 3 and 4 are
where this will be pushed. Whoever meets that case should record which condition was
hardest to satisfy, and whether the honest answer was to fail the rule or to widen it.
