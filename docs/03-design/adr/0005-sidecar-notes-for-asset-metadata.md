# ADR-0005: Sidecar notes for asset metadata
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-BIN-02, FR-VLT-02
**Source:** Serat Design Description §11, decision ADR-05

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Metadata in the index only; extended attributes; one manifest per folder.

## Decision
A binary's frontmatter, tags, links and annotations live in a companion Markdown file.

## Consequences
Folder clutter, mitigated by pairing the two in the file browser. In exchange the metadata is portable, greppable, diffable and survives the application — which index-only metadata does not, xattrs do not survive git or sync, and a per-folder manifest turns every edit into a merge conflict.

## Objections recorded
None recorded at baseline.
