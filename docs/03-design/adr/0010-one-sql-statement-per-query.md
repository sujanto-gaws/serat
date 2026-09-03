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
