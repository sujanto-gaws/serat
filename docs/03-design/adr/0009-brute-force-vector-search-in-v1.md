# ADR-0009: Brute-force vector search in v1
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-AIX-03, NFR-PRF-07
**Source:** Serat Design Description §11, decision ADR-09

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
An HNSW or IVF index.

## Decision
sqlite-vec exact search over all chunks; no approximate index.

## Consequences
No recall loss, no index build step, no parameters to tune. Valid to roughly 200,000 chunks — about 50,000 notes, which is the stated scale target. Revisit only if the ceiling moves, and record the measurement when it does.

## Objections recorded
None recorded at baseline.
