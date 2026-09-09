# ADR-0001: Single-writer actor for the index
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** NFR-PRF-03, NFR-REL-01
**Source:** Serat Design Description §11, decision ADR-01

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
A shared connection behind a mutex; a connection per thread with SQLite's own locking.

## Decision
One thread owns the only write connection; all mutations arrive on a channel. Reads use a separate pool.

## Consequences
Lock ordering disappears as a concern. Backpressure becomes visible as queue depth. Write throughput is bounded by one thread — acceptable, because SQLite in WAL admits one writer anyway.

## Objections recorded
None at baseline. One has arrived since, and it is against the *justification* rather than the decision (architecture §1.2, closed at task #103; recorded here on the lead's ruling of 2026-09-09, because a stub still reading *none* misdescribes how examined a decision is).

**The assumption §1.2 nominated for this ADR is not the one the actor rests on, and as stated it cannot be tested here.** *"Write contention is the bottleneck"* is a claim about what happens when writers contend, and this decision permits **exactly one writer by construction** — there is no contention to observe, so no P0 measurement could confirm or refute it. A structure that eliminates a phenomenon cannot also be evidence about it.

**And the actor never rested on it.** §Consequences above argues from failure modes — lock ordering, backpressure, one queue to reason about — and says outright that a shared connection behind a mutex has *the same throughput*. So this decision would stand unchanged if contention turned out to be irrelevant, and §1.2 attributed to it a dependency it does not have.

**What P0 did establish is narrower and real:** the writer was the dominant phase — `producer_wait` at 45–51% of wall clock — and its cost was **internal rather than structural**, roughly a million redundant statement prepares per rebuild (F-324). `prepare_cached` took the rebuild from 27.1 s to 17.5 s. That is evidence the **actor** was not the problem, and no evidence at all about the counterfactual it was nominated to answer.
