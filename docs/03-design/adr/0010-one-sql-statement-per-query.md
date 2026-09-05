# ADR-0010: One SQL statement per query
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-STR-08, FR-STR-09, NFR-PRF-05
**Source:** Serat Design Description §11, decision ADR-10

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Multi-pass execution with intermediate results assembled in Rust.

## Decision
Every SQ query compiles to exactly one statement; the grammar forbids anything that could not.

## Consequences
The planner sees the whole query, cost is inspectable, and the query inspector can show a user the real statement. Costs expressiveness: two-hop traversals and recursive queries are simply not sayable, which is the intended trade.

## Objections recorded
None recorded at baseline.

## Tested against
**None yet, and this is the ADR most likely to need widening.** It is an absolute — the grammar forbids anything that could not compile to one statement — and P0 established that every absolute here has met a legitimate case its author did not have, three times, each time correctly resolved by widening the rule rather than contorting the code.

The case that would test this one is a real query from the owner's vault that is natural to ask and not sayable in one statement: a two-hop traversal, or a rollup over a rollup. Someone should go looking for it during P2's hand-translation of ten Dataview queries (charter, immediate move 3) rather than waiting for it to arrive from a user, because the cost of widening this decision rises sharply once SQ has a grammar, a conformance suite and queries written against it. While it is still prose, the correction is a conversation.

Two outcomes are acceptable and one is not. Confirming the trade — the query is genuinely better expressed another way, and the expressiveness cost is worth the cost being knowable — is a result. Widening the rule deliberately is a result. Discovering the case in P3 and adding a second execution path to accommodate it is the failure this field exists to prevent, because it would take with it the property the decision was made for: that the planner sees the whole query.
