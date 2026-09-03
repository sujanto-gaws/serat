# ADR-0006: CodeMirror 6 with decorations
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-EDT-02, FR-EDT-10, FR-VLT-05
**Source:** Serat Design Description §11, decision ADR-06

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
ProseMirror or another document-model editor; a custom editor.

## Decision
Hybrid live preview built from CM6 decorations over a text document.

## Consequences
The document stays text, so external edits reconcile as diffs and no save re-serialises. Costs the hardest frontend work in the project; FR-EDT-10 keeps split preview as an explicit fallback so an overrun cannot block P1.

## Objections recorded
None recorded at baseline.
