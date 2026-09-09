---
project: Serat
artifact: Design-document consistency checks
phase: 05-testing
owner: test-engineer
serves: F-239, F-238, F-303, F-346, F-348, C-03, FR-STR-08, FR-STR-09, FR-STR-10, NFR-MNT-01
---

# Design-document consistency checks

One crate (`bench/doclint`), and every check in it is here: F-239 (below) is
about a document
citing evidence that is not there; F-303 is about two documents making a claim
that depends on each other and nothing checking the join; F-496 (task #126) is
about a count written above a list that stops matching it; F-346/F-348 (at the
end of this file) is about a table that stops rendering as a table while every
word in it is still true. All three are mechanical for the same reason —
none of these failures is visible from inside one document read alone, or
even from a careful read of the raw source in the F-346/F-348 case.

## F-239: does every cited path resolve?

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

Exit code 1 and a non-empty report mean at least one citation does not resolve
and is not marked planned. `cargo test -p doclint` runs in every
`cargo test --workspace`, including
[`the_real_design_documents_have_no_unmarked_missing_paths`](../../bench/doclint/src/lib.rs),
which asserts the property against the real repository: every cited path
resolves or is marked planned, plus a floor on how many citations were found
at all (F-27's lesson — "0 missing" and "checked nothing" are the same
one-line PASS unless the count is in it). The can-it-fail proof (F-58, F-27)
lives on synthetic fixtures instead, deliberately: a real-tree test asserting
F-239's exact five missing paths was tried first and broke the moment task #7
marked them planned (F-255, F-265) — proof-of-failure belongs on a fixture
that does not depend on the repository staying broken.

## F-303: does the PRD's Must list agree with architecture.md's accounting?

architecture.md's §Traceability preamble makes a claim about a file it does
not live in: *"Every requirement of priority Must in the baseline is
accounted for."* The baseline is `docs/02-requirements/prd.md`, and the two
documents have different owners who cannot see each other's edits — NFR-SEC-06
was allocated in the PRD at v1.1 and the claim went stale the moment
product-analyst committed.

`cargo run -p doclint --bin check-prd-join` extracts every Must from the PRD's
requirement-definition tables (scoped to exclude the PRD's *own* traceability
appendix, which has the identical row shape and would otherwise double-count),
extracts every requirement ID cited across architecture.md's five accounting
places — the main Traceability table, §10.1, §10.2, §10.4, and the "Musts with
no design element yet" subsection, which counts as accounting for a Must by
naming the gap plainly — and asserts the two sets match. It reports both
directions: a Must with no citation anywhere, and a citation naming an ID the
PRD no longer declares at any priority.

**What it does not cover.** It checks that IDs are *accounted for*, not that
the design element cited is real or adequate — F-239 already showed a table
citing five files that did not exist, and this check would have called that
row accounted for too. A green join is not a green design review.

`bench/doclint/src/prd_join.rs::the_real_documents_join_cleanly` asserts the
join holds today (87 Musts, all accounted for) as a property, not a pinned
count, since both documents are expected to keep changing together. The
can-it-fail proof is a synthetic PRD and a synthetic architecture.md with one
Must deliberately absent from every accounting place (F-58, F-27, F-304).

## F-346/F-348: does every table render as a table?

**P0 finding F-346/F-348 (task #49, #51).** Task #49 found the threat model's
STRIDE table broken below T-05 for seven commits across six passes, two of
them edits to that exact section: a stray unescaped `|` inside a cell —
`[[Target|Alias]]`, `file_name = target || '.md'`, a shell `||` — splits a GFM
table row at the pipe, silently truncating everything after it and pushing
every following cell one column left. Neither F-239 nor F-303 would ever have
caught this: every cited path still resolved and every cited ID was still in
the raw text, because both checks read the source, not the rendering. The
table simply stopped showing its content where a reader would look, and stayed
that way through two edits of the surrounding prose.

`cargo run -p doclint --bin check-table-structure` scans every `.md` file
under `docs/` and checks two properties of every table it finds, neither about
content:

1. Every row's unescaped-pipe count matches its header's. `\|` does not count
   — GFM splits on a raw `|` even inside inline code, so an escaped one is the
   only way to put a literal pipe in a cell.
2. No line starting with `|` sits outside a table: a header row with no
   delimiter row beneath it, or a row-shaped line with prose immediately above
   and no blank line to separate them, is not a table to a GFM renderer even
   though it reads as one on a naive glance at the source.

**What it does not cover.** It checks that a table renders as *a* table — the
right number of cells, in the right place — not that its *content* is
correct. A column count that has always matched proves nothing about whether
the header names the right thing; that is F-303's and a human reviewer's job,
not this check's.

`bench/doclint/src/table_structure.rs::the_check_can_fail_and_the_real_docs_tree_is_clean`
asserts the property holds today against the real tree (every table renders),
plus a floor on how many markdown files were scanned (F-27's lesson again).
The can-it-fail proofs are all synthetic fixtures — the stray-pipe shape task
#49 found, the shell-`||` shape, an orphan header, prose ending a table with no
blank line — for the same reason F-239's and F-303's do: a real-tree
proof-of-failure breaks the day someone fixes the thing it was pinned to
(F-255).

## Running it

```text
cargo run -p doclint --bin check-design-paths
cargo run -p doclint --bin check-prd-join
cargo run -p doclint --bin check-table-structure
cargo test -p doclint
```

## CI

`.github/workflows/design-docs.yml` runs `cargo test -p doclint` (covering
every check in the crate, since they are one crate) on every pull request and
push to `main`, and prints each binary's report so a failure names which check
failed. **Counted as "every check" rather than as a number, per F-496** — this
sentence said "three checks", and the crate has grown twice since.
