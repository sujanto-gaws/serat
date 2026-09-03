# ADR-0007: Generated TypeScript from Rust
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** NFR-MNT-01
**Source:** Serat Design Description §11, decision ADR-07

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Hand-written types on both sides; a schema language as the source of truth.

## Decision
ts-rs generates the front-end types; the build fails if the committed output is stale.

## Consequences
The IPC boundary cannot drift silently, which matters more here than usual because it is the only boundary. Costs a build step and requires the Rust types to be the ones designed for the wire.

## Objections recorded
None recorded at baseline.
