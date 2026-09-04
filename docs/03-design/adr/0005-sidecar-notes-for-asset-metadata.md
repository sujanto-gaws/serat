# ADR-0005: Sidecar notes for asset metadata
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** FR-BIN-02, FR-VLT-02
**Source:** Serat Design Description §11, decision ADR-05

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Metadata in the index only; extended attributes; one manifest per folder.

## Decision
A binary's frontmatter, tags, links and annotations live in a companion Markdown file.

**The name is the binary's full filename with `.md` appended** — `contracts/lease.pdf` pairs with `contracts/lease.pdf.md`, in the same directory, always. Pinned 2026-09-04 at test-engineer's request (finding F-23), having previously been left to convention.

Replacing the extension rather than appending to it — `lease.md` — was rejected because it collides. `lease.pdf` and `lease.png` would share one sidecar, and worse, a vault that already contains a genuine note called `lease.md` would have it silently adopted as an asset's metadata. Appending cannot collide: the only file that can claim `lease.pdf.md` is a note someone deliberately named that.

The sidecar is an ordinary note (C-01). It is indexed as one, it is editable as one, and nothing about the pairing is encoded anywhere but in the name — which is what makes the metadata survive the application (FR-VLT-02).

## Consequences
Folder clutter, mitigated by pairing the two in the file browser. In exchange the metadata is portable, greppable, diffable and survives the application — which index-only metadata does not, xattrs do not survive git or sync, and a per-folder manifest turns every edit into a merge conflict.

## Objections recorded
None recorded at baseline.
