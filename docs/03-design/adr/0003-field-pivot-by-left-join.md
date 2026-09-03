# ADR-0003: Field pivot by LEFT JOIN
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-STR-05, NFR-PRF-05
**Source:** Serat Design Description §11, decision ADR-03

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
A JSON1 column per note; a wide denormalised table per type; filtering the EAV table per reference.

## Decision
One LEFT JOIN onto fields per distinct referenced key, planned at compile time.

## Consequences
Field access is one indexed lookup per key per row. A query referencing twenty fields plans twenty joins, which is the shape worth having in the benchmark suite. JSON1 was rejected because it cannot use an index for a range scan.

## Objections recorded
None recorded at baseline.
