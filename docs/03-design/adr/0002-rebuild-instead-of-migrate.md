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

## Objections recorded
None recorded at baseline.
