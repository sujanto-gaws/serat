# ADR-0002: Rebuild instead of migrate
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-VLT-02, NFR-REL-02, NFR-PRF-04
**Source:** Serat Design Description §11, decision ADR-02

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Versioned migrations, as any application with a real database would use.

## Decision
A breaking index schema change drops the database and rebuilds. No migration scripts.

## Consequences
An entire category of machinery and of partially-migrated failure states never exists. Requires the rebuild to stay fast, which NFR-PRF-04 already demands. The extraction cache is excluded and is migrated properly.

**What this ADR is operative through is the open check's trigger set**, and the set is narrower than it was. Architecture §3.4 lists six conditions under which an existing index is discarded and rebuilt; this decision supplies the third, `IndexVersion`, which is the whole of the upgrade path here. **One of the other five has been demoted and it is recorded in this ADR because the demotion changes what "rebuild instead of migrate" costs a user in practice.**

**A vault-identity mismatch is no longer a rebuild trigger on its own** (task #120, ruled in §3.4 under *Ruling (task #120)*; F-458, F-459). `index_meta.vault_root` was compared as a path string, so a mapped drive, a junction, a UNC form, a differently-cased root or a trailing separator each read as a different vault and bought a full rebuild — 28.6 s measured on the 20,200-note fixture, with no index for its duration. **The decision is that the comparison is canonicalised on both sides** — which is what `schema/001_init.sql:120` already asked for — **and that a surviving mismatch is recorded and reported rather than punished. Task #120 is the implementation and it is with rust-core-dev; as of `4259abe` the code still compares `to_string_lossy()` and still rebuilds.** The sentence is written in the voice of the decision on purpose, because a decision record that describes code it does not control is how this project's prose goes stale (F-368); one-line re-check, which stays useful either way because it names the comparison rather than its state: `grep -n 'VaultMoved' crates/serat-index/src/schema.rs`. The reasoning: `notes.path` is vault-relative (`schema/001_init.sql:134`), so a vault that moved carries an index in which every row is still right, and the open path falls through to ordinary reconciliation. **This does not weaken ADR-0002.** A schema change still drops the database and rebuilds; what stopped triggering a rebuild is a condition that was never about the schema and never about the index being wrong.

**Why it is written here rather than only in §3.4.** This ADR is what a reader consults to learn when an index is thrown away, and a trigger that quietly stops firing is exactly the kind of change that leaves a decision record describing a system that no longer exists (F-368). The ratification of #120 named this obligation — *ADR-0002 documents the trigger being demoted, so its text moves in the same commit as the code* — and the text did not move with it; this is that paragraph, one commit late.

## Objections recorded
None at baseline. Two have arrived since, and they are recorded here because a baseline stub that still says *none* three hundred commits later is telling a reader something false about how examined the decision is (task #120's sweep; the lead's ruling of 2026-09-09).

**The precondition this decision leans on is met with less margin than the sentence implies.** *"Requires the rebuild to stay fast, which NFR-PRF-04 already demands"* reads as though the requirement discharges the obligation. The attested figure is **27,693.5 ms against a 30,000 ms budget** at `concurrent_build_processes: 0` with a 3.2% in-run spread (#96) — about 8% of headroom, while F-327 records the same binary on the same vault moving **35% between sessions**. So the drift exceeds the margin: rebuild-instead-of-migrate is affordable on an attested machine and has not been shown affordable on an arbitrary one. This does not reverse the decision — the machinery a migration path would need is a real and permanent cost, and F-327's drift is a property of the measurement environment rather than of the rebuild — but *"NFR-PRF-04 already demands it"* is a smaller guarantee than it sounds and should not be quoted as though the question is closed.

**The trigger set is where this decision is felt, and one of its triggers was wrong for two hundred commits without anyone paying for it visibly.** F-459's identity check cost a full rebuild on a spelling difference — measured at **8 ms for a warm open against 21–29 s for the rebuild** (test-engineer, #94), three orders of magnitude decided by a comparison no user can see or influence. The objection this raises against ADR-0002 is not that rebuilding is wrong; it is that **a decision whose failure mode is *slow* rather than *unsafe* attracts no scrutiny until someone measures it**, and C-02 is what makes that trade acceptable rather than what makes it invisible.
