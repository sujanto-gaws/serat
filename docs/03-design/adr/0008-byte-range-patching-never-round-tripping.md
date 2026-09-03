# ADR-0008: Byte-range patching, never round-tripping
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-VIW-06, FR-VIW-08, NFR-MNT-03
**Source:** Serat Design Description §11, decision ADR-08

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Parse to an AST, modify, re-emit.

## Decision
A view-originated write replaces the bytes of one field. There is no serialiser in the write path.

## Consequences
Formatting is preserved exactly, so a save is not a diff and git history stays meaningful. Costs the byte-range bookkeeping through the whole indexer, and limits view edits to values — which is a constraint the product wanted anyway.

## Objections recorded
None recorded at baseline.
