# ADR-0004: Extraction in a child process
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-BIN-07, NFR-SEC-03, NFR-REL-03
**Source:** Serat Design Description §11, decision ADR-04

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
In-process with catch_unwind; a WASM sandbox.

## Decision
Each extraction runs in a short-lived child with time and memory caps and no network.

## Consequences
A crash or a hang costs one file. catch_unwind was rejected because it does not contain memory corruption or an infinite loop; WASM was rejected because the mature PDF and OOXML libraries do not target it.

## Objections recorded
None recorded at baseline.
