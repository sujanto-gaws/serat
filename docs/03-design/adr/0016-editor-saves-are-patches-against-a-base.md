# ADR-0016: Editor saves are patches against a base, and external changes arrive in the same coordinates

**Status:** accepted — code-reviewer approved task #5 (merged at `d0d5300`); these nits folded in afterwards; §2 step 3, §3 and §4 amended from task #33's implementation on 2026-10-20 and after code-reviewer-2's review · **Date:** 2026-09-24 · **Serves:** FR-VLT-03, FR-VLT-05, NFR-REL-01, NFR-CMP-01, C-04, US-06, US-07
**Honours:** ADR-0006 (the document stays text), ADR-0008 (no serialiser in any write path — extended here from view writes to editor saves), ADR-0007 (the wire types are generated)

## Context

P0's `write_file` takes the whole document. That was acceptable for a text box, and `contracts/ipc.rs` already refuses the one way it could corrupt a file — a lossy UTF-8 read followed by a save. **P1 puts CodeMirror 6 behind it, and CodeMirror has a second way.** With no `lineSeparator` configured, CM6 splits lines on `\r\n`, `\n` and `\r` and joins them with `\n`. Saving `doc.toString()` therefore rewrites every line ending of a CRLF note (the 20k benchmark vault's manifest counts `notes_crlf: 399` and `notes_bom: 79`) and of any note with mixed endings, although the user changed one word. That is a re-serialisation, which C-04 forbids. It would pass every test that uses LF fixtures, and every Obsidian user on Windows would see it first as a whole-file git diff.

Setting `lineSeparator` to the file's dominant ending fixes pure-CRLF notes and still rewrites the minority endings of a mixed one. **The defect is not the separator. It is that a save sends a document instead of the edit.**

FR-VLT-05 needs the reverse direction in the same terms. An external change has to reach the buffer as a minimal edit: cursor at line 40 stays when line 5 changes, and undo still reaches earlier states (§3.3). It must also meet the case §3.3 does not cover — local edits that have not been saved yet — without discarding either side (NFR-REL-01).

It is an ADR because the IPC contract, the front end's buffer model and serat-vault's write path are all built on it.

## Options considered

1. **Whole-document save with `lineSeparator` detected per file.** Fixes pure CRLF, breaks mixed endings, and leaves "no byte outside the edit changes" as a property nobody can assert. Rejected.
2. **Whole-document save, with Rust re-inserting the original line endings by aligning lines.** A heuristic that reconstructs what the edit was from its result — a serialiser by another name. Rejected, on ADR-0008's own reasoning.
3. **Byte offsets from the front end.** The front end holds UTF-16 text and does not know the file's bytes (endings, BOM), so it would have to reproduce the file's encoding exactly to compute them. Rejected.
4. **Patches in line/column coordinates of the base, applied by Rust to the base bytes.** Chosen.
5. **For reconciliation: diff in the front end.** Rejected: the diff of a large note belongs off the webview's main thread (goal 2), and Rust already has both the base bytes and the disk bytes.

## Decision

### 1. One coordinate system, both directions

A position is `{ line, col }`: `line` is 0-based; `col` is in **UTF-16 code units** within that line, which is CodeMirror's native unit. Lines are split on `\r\n`, `\n` and a lone `\r`, exactly as CM6 splits them. The line terminator is not part of any line, so no position can fall inside a `\r\n`. **Every line separator — `\r\n`, `\n` or a lone `\r` — occupies exactly one offset in CM6's `Text` model** (`@codemirror/state`'s public `Text.length` and `Text.line(n).from` count it once), which is what makes `{line, col}` independent of separator width. **This holds only while no `EditorState.lineSeparator` facet is configured.** With the facet set, CM6 splits on that one string alone, and a lone `\r` or a `\n` in a CRLF note becomes line content rather than a separator. The front end's patch builder (`patches.ts` in the editor module, on `task/p1-21-editor`) relies on the facet being absent, so configuring it anywhere is a change to this ADR. Assuming a CRLF costs two offsets puts every column off by one on a mixed-ending file; the first tests written against this rule did exactly that (vue-frontend-dev, task #21). A BOM is part of line 0's text, as it is in CM's document. **That holds only if the front end builds the document from `read_file`'s `String`, which keeps U+FEFF, and never runs the bytes through a default `TextDecoder`, which strips it.** If the BOM were stripped, every column on line 0 would be off by one. The shared table test therefore includes a BOM case. A file that is not valid UTF-8 is never opened for editing (`IpcError::NotUtf8`, already in the contract), so every column maps to exactly one byte offset.

The mapping from `{line, col}` to a byte offset in the base bytes is **one pure function in serat-vault**, beside `write_atomic`, and both directions use it.

### 2. Saves are patch sets

The front end keeps, per open note, the **base** — the `Text` as last loaded or saved, and its hash — and the **ChangeSet composed from every document change since the base**. A save sends `[{ from, to, insert }]` in base coordinates, with `base_hash`. serat-vault then:

1. re-reads the file and refuses with `StaleHash` if its hash is not `base_hash` — the optimistic lock of §6.1;
2. maps each range onto byte offsets and checks the ranges are ordered and disjoint;
3. converts every separator in `insert` to the note's line ending, writes `\r\n` instead where that ending would fuse with a neighbouring `\r`, and refuses a deletion that would fuse two endings (the rules are below);
4. splices and writes the temporary file, then **re-hashes the target immediately before the rename** and refuses with `StaleHash` (removing the temporary file) if it moved;
5. renames (FR-VLT-03) and returns the new hash.

**The note's line ending is its most common terminator in the base bytes of this save, with ties going to whichever kind appears first. A lone `\r` is the ending only when it is the note's only kind. A note with no separator at all uses `\n`.** Every separator in `insert` (`\n`, `\r\n` or a lone `\r`) becomes that ending. The rule is stateless: it is read from the base at every save, and nothing about it is remembered in memory or in `.serat/`, which goal 1 and C-02 rule out for something written into the file. Two consequences are accepted.

- **A stray `\r` does not set the ending.** Under an earlier draft's "first terminator" rule, one lone `\r` near the top of an LF note made every new line a lone `\r`, which `git diff`, `grep`, `wc` and most editors misread. Majority makes the stray the exception it already is (code-reviewer-2's review of this amendment, ruled by lead).
- **After a save removes the last separator of a CRLF note, later inserts use `\n`.** The file then has no ending left to preserve, and a one-line file carries no ending for a user to lose. If the user wants CRLF back, their tool's normalisation restores it, and Serat preserves it from then on.

**Two endings can fuse in a mixed-ending note, and the two ways it can happen are handled differently.** A lone `\r` followed directly by a `\n` reads as one `\r\n`, which swallows a line.

- **The insert form is avoided by choosing what Serat writes.** With `\n` as the ending, an inserted separator that would land directly after a lone `\r` in the base (an insert at the start of the line that a lone `\r` ends, beginning with a separator) is written as `\r\n` instead. In base `a\rb\nc\n` the ending is `\n`, and pressing Enter at the start of `b`'s line (inserting `\n` at line 1, column 0) writes `a\r\r\nb\nc\n`, not `a\r\nb\nc\n`. `\r\n` cannot fuse with a neighbour on either side, because CM6 reads `\r\r\n` as `\r` then `\r\n`, and `\r\n\n` as `\r\n` then `\n`. The separator bytes in an insert are Serat's own, so this is choosing what is written into the user's range, not re-encoding anything outside it, and ADR-0008 holds. Under the majority rule this is the only insert that can fuse: `\r\n` as the ending never fuses, and `\r` is the ending only when the note contains no `\n`.
- **The deletion form is refused.** In base `a\rx\nc`, deleting `x` would leave `a\r\nc`, where the lone `\r` and the `\n` fuse. Fixing that would mean changing a byte outside the user's range, which ADR-0008 forbids. So serat-vault refuses with `VaultError::LineEndingsWouldFuse`, which reaches the front end as **`IpcError::LineEndingsWouldFuse { path, line }`**. That is a distinct variant (`contracts/ipc.rs`), because `InvalidPatch` means a client bug that is only logged. This refusal is a legitimate edit that has to be explained to the user. The file is not touched. **The front end shows a clear error naming the line**: the save was refused because the note mixes line-ending styles there. The buffer stays dirty and intact, and the user can edit that line differently or normalise the note's endings in another tool. The save is never retried automatically, because it would be refused again.
- **Both are backstopped by a line-count check.** The patched text must have exactly as many lines as the base, plus the lines the edits add, minus the lines they remove, or the save is refused. The check is sound, as code-reviewer-2 argued. No position falls inside a `\r\n`, so a splice can never split a separator. Every separator an insert brings is counted. So the only way the count can differ is two separators becoming one, and a fuse can only lower the count. **This can only happen in a mixed-ending note.** With one kind of ending, every inserted separator is that kind, so a `\r` and a `\n` from different separators never meet.

**The remaining window is stated as a bound rather than hidden.** An external write between step 4's re-hash and the rename is overwritten. The window is one hash of the current file plus one `rename` system call. For a note of the benchmark vault's 2 KiB mean that is well under a millisecond, and it is proportional to file size. Closing it entirely needs a lock no other editor honours. **The loss in that window is silent.** The other writer's bytes are gone from disk, and nothing reports it: the watcher sees only our rename, which carries our own hash. What bounds the loss is the size of the window, not any detection. The upgrade path, if #23 shows the window matters, is an atomic exchange that fails when the target has moved: `renameat2(RENAME_EXCHANGE)` on Linux, `renamex_np(RENAME_SWAP)` on macOS and `ReplaceFileW` on Windows. Each can detect the change after swapping and restore the other writer's bytes. #23's hostile test writes externally between the step-1 check and the rename, and asserts the step-4 refusal.

**Ordering between saves and external changes.** Tauri orders neither command responses against events nor events against responses. So the front end applies these rules, and nothing else:

- An `ExternalChange` is applied **only when its `base_hash` equals the buffer's current base hash**.
- **While a save is in flight, an arriving `ExternalChange` is held**, not applied. When the save resolves, the held change is applied if its `base_hash` now equals the base; otherwise it is dropped **and `request_external_diff` is called**.
- **Any other mismatch drops the change and requests a fresh diff** against the current base (`request_external_diff(path, base_hash)`). serat-app answers with an `ExternalChange` computed from the disk bytes of that moment, or with nothing when disk equals the base.
- **A save refused with `StaleHash` leaves the buffer dirty and is never retried blindly.** The front end **always calls `request_external_diff`** — it does not wait for an event that may already have been dropped — and the outcomes of §4 decide what happens next.
- **At most one save per note is in flight.** A save triggered during another save does not start; triggers coalesce into one save issued when the in-flight save resolves, carrying every change made meanwhile against the base that save produced.
- **When serat-app no longer holds the bytes for the requested `base_hash`** (the note was closed and reopened, or a newer base replaced it), `request_external_diff` returns an **`IpcError::StaleHash` carrying the current disk hash**, never a guess. The front end treats it as an overlapping change, which is the conflict path of §4, so the user chooses and nothing is merged on stale coordinates.

#23 tests both arrival orders (response before event, and event before response) and the external write between check and rename.

**Bytes outside the patched ranges are copied, never re-encoded — by construction rather than by care.** That is the invariant, and the property test in *Tested against* item 1 checks it. On success the base becomes the document as it was when the save was taken; changes typed while the save was in flight stay composed against that new base.

`write_file` keeps exactly one job: **creating** a note (`base_hash: None`). The editor never calls it for an existing file. Saves are triggered by 1 s of idle, by blur, by closing the note and by the save command. None of them runs on the keystroke path (goal 2): the save is an asynchronous IPC call, and the keystroke never waits for it.

### 3. External changes are edit scripts in base coordinates

serat-app records the base bytes and hash of every note open in an editor. When the watcher path reports a changed path (after the hash gate), and that path is open, and the new hash differs from the buffer's base hash, serat-app reads the disk bytes. serat-vault computes the edit script: a Myers line diff, with each replacement hunk refined to the characters that differ (common prefix and suffix trimmed), under a **50 ms deadline**. **If the deadline passes**, the changed middle becomes one hunk, after trimming the common leading and trailing lines, and that hunk is refined the same way. serat-app emits `ExternalChange { path, base_hash, new_hash, edits }` with `edits` in the base's coordinates. A disk hash equal to the base hash — Serat's own save, if echo suppression missed it — produces no event, so reconciliation needs no echo logic of its own.

**The diff compares the two texts in CodeMirror's line model.** Lines are split on `\r\n`, `\n` and a lone `\r`, terminators excluded, and every `insert` is `\n`-joined. A difference that is only a line ending therefore produces no edit, because the editor's document cannot represent one. **So a disk change that only flips CRLF↔LF yields `edits = []` with a new `new_hash`.** It is still emitted, and the front end moves its base hash to `new_hash` without touching the buffer. The base `Text` is already equal to the disk text in the line model. The next save patches the new bytes and keeps their new endings outside the patched ranges.

**If the edits cover more than half the file**, one whole-document edit is sent, because position is meaningless at that point (§3.3's fallback, kept). "More than half" is measured as `max(bytes removed, bytes added) × 2 > max(base size, other size)`. Removed and added are summed over the hunks' whole lines, each counted with one terminator, before character refinement. Taking the larger of the two sides means a note rewritten from scratch is whole-document whichever version is shorter.

### 4. The front end applies one of three outcomes

| Buffer state | Outcome |
|---|---|
| No unsaved changes | Apply `edits` as one transaction marked `Transaction.addToHistory.of(false)`. The selection and the undo history are mapped through it, so the cursor on line 40 stays put and undo reaches every earlier local state. Undo does not revert the external edit: undoing someone else's change in your buffer would write it away at the next save. The base becomes the disk version. |
| Unsaved changes whose base ranges are **disjoint** from every external edit (ranges that touch count as overlapping) | Map the external changes through the local ones and apply them the same way. The base becomes the disk version, and the local ChangeSet is rebased over the external one. The buffer stays dirty, and the next save patches against the new hash. |
| Unsaved changes that **overlap** | **Conflict.** Apply nothing and keep the buffer as the user left it. Show a non-modal notice (§9.1: no modal on the write path) with three actions. *Keep mine* **re-bases the buffer onto the disk version**: the base `Text` becomes the disk text, the base hash becomes the disk hash, and the local ChangeSet is **recomputed as the diff from the disk text to the buffer**, computed in Rust by serat-vault's edit-script function (the same one §3 uses), not in the webview. The front end sends the buffer text once, on that explicit action, through `rebase_buffer` (`contracts/ipc.rs`). The next save then patches disk into exactly what the user sees. Adopting the disk hash while keeping a ChangeSet computed against the old base would splice old-base coordinates into new bytes and corrupt the file. That was the first draft's rule, and review caught it. #23's hostile test asserts that the disk bytes equal the buffer after a *Keep mine* save, **modulo line endings**: the same lines and the same characters, byte for byte, with each line ending as the disk version had it, and new lines in the note's ending. That is preserved by design, not a gap. The edit script compares in the line model, so the endings of lines the user did not touch are the disk's, and step 3 gives new lines the note's ending. *Take theirs* replaces the buffer in one **history-recorded** transaction, so undo brings back what was typed. *Compare* opens a diff. Nothing is discarded silently in any branch. |

A note deleted on disk while open keeps its buffer and shows a notice; the save command then recreates the file through `write_file` with `base_hash: None`.

## Consequences

**Positive.** C-04 holds for the editor by construction, and it holds for CRLF, mixed endings and BOMs without a single special case. Saves and reconciliation share one coordinate system and one mapping function, so they cannot disagree about where a position is. The stale-hash guard now covers editor saves as well as view writes. A concurrent external edit either merges or becomes a visible conflict — it never becomes a lost update.

**Negative.** The front end must keep a base `Text` and a composed ChangeSet per open note. CM's `Text` is a persistent tree, so the base is shared structure rather than a copy. Composing is linear in the number of changes rather than in the size of the document, but it runs on every transaction, and the NFR-PRF-02 harness has to see it. The disjointness rule refuses some merges a character-level three-way merge would accept. That is conservative on purpose, and it is the side of the trade NFR-REL-01 wants.

**Risks.** The line-splitting rule must match CM6's exactly. If it drifts, a patch lands on the wrong line. The rule therefore lives in one Rust function with a table test shared with the front end: the same fixture strings, the same expected `{line, col}` → byte offsets, run by `cargo test` and by `npm test`.

## Objections recorded

**vue-frontend-dev** (owns the editor) was consulted on 2026-09-24 with two questions: whether the composed ChangeSet is affordable on the keystroke path for a 5,000-line note, and whether `addToHistory(false)` with history mapping yields FR-VLT-05's behaviour. **No reply had arrived when this was committed, so the objection is absent, not waived.** The place I expect it: the cost of `compose` on every transaction. If NFR-PRF-02's harness shows it, the fallback keeps the per-transaction ChangeSets in an array and composes them only at save time — the same result, with the cost moved off the keystroke. The ADR does not depend on which of the two is used.

**lead** (review of task #5, 2026-09-24) raised three blocking objections, all upheld. First, *Keep mine* as first drafted corrupted the file, because it adopted the new hash over a ChangeSet in old coordinates; it now re-bases (§4). Second, nothing ordered a save in flight against an `ExternalChange`; §2 now carries the ordering rule. Third, rendered links could navigate the app window to a remote origin; that is architecture §9.3 and T-09. The same review added the re-hash before rename, the BOM rule and the manifest citation.

## Tested against

1. **The 399 CRLF notes and 79 BOM notes of the 20k benchmark vault.** They are the case that found the defect, and they become a test: for every note in the vault, apply a random patch set through the save path and assert that every byte outside the patched ranges is unchanged (the NFR-MNT-03 property, extended to the editor). Owned by test-engineer's task #12 corpus.
2. **FR-VLT-05's acceptance case** — cursor at line 40, external edit at line 5 — in both the clean and the dirty-but-disjoint states, plus one overlapping edit that must produce a conflict and lose nothing. Task #23's hostile test.
3. **A note with a lone `\r` line ending** — the only splitting rule CM6 and a naive Rust `lines()` disagree on. **Now covered on the front-end side** by app/src/editor/patches.test.ts *(planned)* on `task/p1-21-editor` (`b87755d`), together with the one-offset-per-separator rule above. The Rust half of the shared table test must carry the same case.
4. **Patch application, as property and hostile tests**: crates/serat-vault/src/patch_props.rs *(planned)*, which arrives with task #33 (`task/p1-33-patch-apply` @ `7d39829`, in review). At that commit it covers bytes outside the patched ranges staying unchanged, the lone-`\r` ending, a CRLF+BOM note across many saves, and the stale-base and crash-mid-write cases. The edit script's own tests sit beside it in `diff.rs`: endings-only changes give `edits = []`, and the half-file fallback. **`7d39829` predates this amendment, so #33 owes five changes:**
   - the majority ending rule, replacing first-terminator;
   - the insert form written as `\r\n`, replacing that commit's refusal of it, with its test changed to assert the written bytes;
   - `VaultError::LineEndingsWouldFuse` mapped to `IpcError::LineEndingsWouldFuse`;
   - a test of the deletion form (`a\rx\nc` with `x` deleted);
   - a test of the 50 ms deadline's one-hunk fallback.
