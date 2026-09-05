---
project: Serat
artifact: Design-document path check
phase: 05-testing
owner: test-engineer
serves: F-239, F-238, C-03, FR-STR-08, FR-STR-09, FR-STR-10, NFR-MNT-01
---

# Design-document path check

**P0 finding F-239.** `docs/03-design/architecture.md` names seven "repo-ready"
companion files by path, in the present tense — "the full contract is
`contracts/extractor.rs`" — and states that where this document and those
files disagree, the files are correct. Two of the seven exist. Five did not,
and there was no `grammar/` directory at all. This is a different failure
shape from a missing requirement ID (F-238): a missing ID reads as
*incomplete* and invites someone to finish the table; a citation to a file
that is not there reads as *complete* and stops anyone from looking, because
nothing about reading the sentence tells you the evidence it points at does
not exist.

## What the check does

`cargo run -p doclint --bin check-design-paths` scans every `.md` file under
`docs/03-design/` and `CLAUDE.md` (which carries the same canonical-documents
table exposure), extracts every repo-relative path they cite, and asserts each
one resolves under the repository root. A path that does not resolve fails the
run and is reported with every place it was cited, so the message says what to
fix rather than only that something is wrong.

Extraction is a handful of character-class rules over plain text (`bench/doclint/src/lib.rs`),
not a regex: a run of `[A-Za-z0-9/_.-]` counts as a citation candidate only if
its first `/`-segment is a known source directory (`docs`, `crates`, `bench`,
`app`, `schema`, `grammar`, `contracts`), it ends in a recognised extension,
and it contains no `..` (a range like `docs/03-design/adr/0001..0010` names ten
ADRs, not a file). Two example paths ADR-0005 uses to illustrate its sidecar
naming rule (`contracts/lease.pdf`, `contracts/lease.pdf.md`) happen to share
the `contracts/` prefix with the real design-file directory and are excluded by
name, with the citation that forced the exclusion recorded next to it.

## The `*(planned)*` marker

A path a document cites is either real (it must resolve today) or deliberately
planned for a later phase. Task #7 closed F-239 on the document side by
marking every cited-but-unwritten path `*(planned)*` in the traceability table,
and architecture.md's own §Traceability preamble defines the marker: *"A path
in the Where column marked `*(planned)*` names a repo-ready file that is not
written yet... Every unmarked path resolves today, and that is a property a
check can assert."* That sentence is this check's specification. A citation
followed by the marker (either `*(planned)*` or the row-scoped
`*(planned — the whole of this row)*`) is excused from resolving; an
unmarked one is not.

There is deliberately no second, separately-maintained allowlist file. A list
that can drift from the prose next to it is worse than none — F-239 was a
document disagreeing with itself undetected, and a shadow list would be the
same defect one layer down. If a document cites the same path in more than one
place (architecture.md's top-of-file summary cites all seven files in flowing
prose with no per-citation marker, relying on the traceability table's row to
carry it), the path counts as planned if *any* citation of it carries the
marker.

A path marked planned that now resolves on disk is reported as a note, not a
failure — the artefact landed, which is what "planned" was for — naming where
the now-stale marker was written so it can be dropped.

## Running it

```text
cargo run -p doclint --bin check-design-paths
cargo test -p doclint
```

Exit code 1 and a non-empty report mean at least one citation does not resolve
and is not marked planned. `cargo test -p doclint` runs in every
`cargo test --workspace`, including
[`the_real_design_documents_fail_on_exactly_f_239s_five_paths`](../../bench/doclint/src/lib.rs),
which proves the check can fail before trusting it (F-58, F-27): it runs the
real check against the real repository and requires the result to name
exactly F-239's five paths, not fewer (a regression in the extraction) and not
more (a false positive). That test is expected to start failing — in the sense
that its hard-coded expectation goes stale — the moment task #7's markers or
task #10's files land on the branch it runs against; updating it then is the
maintenance both mechanisms exist for.

## CI

`.github/workflows/design-docs.yml` runs `cargo test -p doclint` on every pull
request and push to `main`.
