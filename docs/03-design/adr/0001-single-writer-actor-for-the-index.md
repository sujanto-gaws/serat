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
None recorded at baseline.
