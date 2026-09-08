# Does NFR-PRF-04 have a gate-quality measurement? (task #96)

**Status:** ruling drafted, number pending · **Owner:** test-engineer · **Serves:** NFR-PRF-04,
C-06 · **Cites:** ADR-0013, F-327, F-375, task #83

## The question, precisely

C-06 says the performance budgets are binding and enforced in CI. NFR-PRF-04 (full rebuild,
20,000 notes, ≤ 30 s) has been *measured* several times over this spike. Measured is not the
same claim as *gate-quality*: ADR-0013's condition 2 requires a residual — here, a margin
between the reading and the limit — to be stated **against a stated noise floor**, and F-327
recorded that this harness's own noise on this machine is large enough to swallow the budget's
whole margin. This document asks the narrower question #96 was opened for: **is there, right
now, a reading of NFR-PRF-04 that this project could point to and say "that is the gate
number, and here is why it should be trusted"?**

## The evidence on record

Five reference-profile readings of `prf04-full-rebuild` against the canonical 20k vault, same
binary, same digest, taken across two sessions (F-327):

| Reading | Result against 30 s |
|---|---|
| 36,364 ms | FAIL |
| 37,596 ms | FAIL |
| 27,052 ms | PASS |
| 24,400 ms | PASS |
| 19,000 ms | PASS |

Spread across these five: `(37,596 − 19,000) / 19,000 ≈ 98%` — the readings do not merely have
noise, they disagree about the *verdict*. F-327's diagnosis, not repeated in full here: the
movement is between sessions, not within one (three runs in one sitting cluster inside 0.4% of
their own median), and it tracks the writer's own phase, not the vault warming in cache. F-327
named the cause it could not yet confirm: contention from other agents' concurrent `rustc`/
`link` processes on this shared machine.

Task #83 built the instrument that could confirm or rule that out — `Environment` now stamps
`cpu_load_percent` and `concurrent_build_processes` on every reading — and, taken with
`concurrent_build_processes: 0`, four same-session parse-phase readings landed at 3236–3637 ms
(median 3332 ms, 1.3% spread), far below the 31.3 s reading that had been the leading suspect
for the FAIL verdicts. That is evidence *for* the contention hypothesis. **It is not itself a
full-rebuild reading**: #83 measured the parse phase in isolation while writing the
instrument, not a complete `prf04-full-rebuild` run, and it predates this ruling.

## Applying ADR-0013's conditions

ADR-0013 was written for a different question — when a budget-red *merge* may proceed — but
its condition 2 is exactly this document's test, and its checkability was answered directly in
the ADR (test-engineer's consultation, landed `a88d008`): every `Outcome` this harness produces
already carries `spread_pct()`, a `determinate()`/`INDETERMINATE` verdict, and — since #83 — a
machine-load attestation in `Environment::summary()`. The instrument is not the gap. **The gap
is that none of the five readings above were taken with that instrument present.** They predate
`cpu_load_percent`/`concurrent_build_processes` entirely, so none of them can be classified,
after the fact, as an unloaded-machine reading or a contended one. A number cannot be
gate-quality retroactively; the attestation has to exist at the moment it is taken.

## Ruling

**No — NFR-PRF-04 does not currently have a gate-quality measurement**, for a reason narrower
than "the noise is too large": the noise is *unattributed* on every reading taken so far,
because the instrument that could attribute it did not exist when they were taken. This is not
a restatement of F-327; it is F-327 plus #83, applied: the tool to tell a slow machine from a
busy one now exists and has not yet been pointed at a full `prf04-full-rebuild` run.

**What closes this.** One reference-profile `prf04-full-rebuild` run, taken under a declared
measurement window (CLAUDE.md's convention — one machine, one sitting, nobody else building),
with `concurrent_build_processes: 0` in its own `Environment` line. If that reading:

- **passes with `concurrent_build_processes: 0` and `determinate() == true`** (no sample
  straddled 30 s), it is gate-quality and becomes the number #101 reports for NFR-PRF-04. A
  single attested reading is stated as such — not as "the" number without qualification — with
  its `spread_pct()` quoted beside it, per ADR-0013 condition 2's own standard.
- **fails, or the machine cannot be gotten to `concurrent_build_processes: 0`**, that is itself
  the finding, and #101 reports NFR-PRF-04 as *unmeasured at gate quality*, distinct from
  *measured and failing* — the same distinction F-380 already draws for NFR-PRF-03 pre-#90, and
  the same discipline this project's findings register keeps insisting on rather than letting a
  number stand in for a verdict nobody actually reached.

**Not attempted here.** No new measurement is taken in this document. Task #96 is a ruling, not
a reading; the standing rule ("no gate number without a declared window", resume-list
scheduling constraint) applies, and test-engineer has requested the window from the lead.

## Number, once taken

| Field | Value |
|---|---|
| Reading (ms) | *pending* |
| `concurrent_build_processes` | *pending* |
| `cpu_load_percent` | *pending* |
| `spread_pct()` (n=3) | *pending* |
| `determinate()` | *pending* |
| Verdict | *pending* |
| Taken | *pending* |
