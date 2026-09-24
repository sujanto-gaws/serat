# ADR-0016: Editor saves are patches against a base, and external changes arrive in the same coordinates

**Status:** proposed — accepted when review of task #5 is clean · **Date:** 2026-09-24 · **Serves:** FR-VLT-03, FR-VLT-05, NFR-REL-01, NFR-CMP-01, C-04, US-06, US-07
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

A position is `{ line, col }`: `line` is 0-based; `col` is in **UTF-16 code units** within that line, which is CodeMirror's native unit. Lines are split on `\r\n`, `\n` and a lone `\r`, exactly as CM6 splits them. The line terminator is not part of any line, so no position can fall inside a `\r\n`. A BOM is part of line 0's text, as it is in CM's document. **That holds only if the front end builds the document from `read_file`'s `String`, which keeps U+FEFF, and never runs the bytes through a default `TextDecoder`, which strips it.** If the BOM were stripped, every column on line 0 would be off by one. The shared table test therefore includes a BOM case. A file that is not valid UTF-8 is never opened for editing (`IpcError::NotUtf8`, already in the contract), so every column maps to exactly one byte offset.

The mapping from `{line, col}` to a byte offset in the base bytes is **one pure function in serat-vault**, beside `write_atomic`, and both directions use it.

### 2. Saves are patch sets

The front end keeps, per open note, the **base** — the `Text` as last loaded or saved, and its hash — and the **ChangeSet composed from every document change since the base**. A save sends `[{ from, to, insert }]` in base coordinates, with `base_hash`. serat-vault then:

1. re-reads the file and refuses with `StaleHash` if its hash is not `base_hash` — the optimistic lock of §6.1;
2. maps each range onto byte offsets and checks the ranges are ordered and disjoint;
3. converts every `\n` in `insert` to the note's line ending, taken as its first terminator (`\n` when the file has none);
4. splices and writes the temporary file, then **re-hashes the target immediately before the rename** and refuses with `StaleHash` (removing the temporary file) if it moved;
5. renames (FR-VLT-03) and returns the new hash.

**The remaining window is stated as a bound rather than hidden.** An external write between step 4's re-hash and the rename is overwritten. The window is one hash of the current file plus one `rename` system call. For a note of the benchmark vault's 2 KiB mean that is well under a millisecond, and it is proportional to file size. Closing it entirely needs a lock no other editor honours. The loss is not silent, though: the other writer's bytes are gone from disk, but the watcher reports our rename, and nothing that writes into the vault from outside Serat is protected by any lock anyway. #23's hostile test writes externally between the step-1 check and the rename, and asserts the step-4 refusal.

**Ordering between saves and external changes.** Tauri orders neither command responses against events nor events against responses. So the front end applies these rules, and nothing else:

- An `ExternalChange` is applied **only when its `base_hash` equals the buffer's current base hash**.
- **While a save is in flight, an arriving `ExternalChange` is held**, not applied. When the save resolves, the held change is applied if its `base_hash` now equals the base, and dropped otherwise.
- **Any other mismatch drops the change and requests a fresh diff** against the current base (`request_external_diff(path, base_hash)`). serat-app answers with an `ExternalChange` computed from the disk bytes of that moment, or with nothing when disk equals the base.
- **A save refused with `StaleHash` leaves the buffer dirty and is never retried blindly.** The front end waits for, or requests, the `ExternalChange` that explains the refusal, and the outcomes of §4 decide what happens next.

#23 tests both arrival orders (response before event, and event before response) and the external write between check and rename.

**Bytes outside the patched ranges are copied, never re-encoded — by construction rather than by care.** That is the invariant, and the property test in *Tested against* item 1 checks it. On success the base becomes the document as it was when the save was taken; changes typed while the save was in flight stay composed against that new base.

`write_file` keeps exactly one job: **creating** a note (`base_hash: None`). The editor never calls it for an existing file. Saves are triggered by 1 s of idle, by blur, by closing the note and by the save command. None of them runs on the keystroke path (goal 2): the save is an asynchronous IPC call, and the keystroke never waits for it.

### 3. External changes are edit scripts in base coordinates

serat-app records the base bytes and hash of every note open in an editor. When the watcher path reports a changed path (after the hash gate), and that path is open, and the new hash differs from the buffer's base hash, serat-app reads the disk bytes. serat-vault computes a line diff with character refinement inside changed lines, under a 50 ms deadline that falls back to the line diff. serat-app emits `ExternalChange { path, base_hash, new_hash, edits }` with `edits` in the base's coordinates. A disk hash equal to the base hash — Serat's own save, if echo suppression missed it — produces no event, so reconciliation needs no echo logic of its own.

**If the edits cover more than half the file**, one whole-range edit is sent: position is meaningless at that point (§3.3's fallback, kept).

### 4. The front end applies one of three outcomes

| Buffer state | Outcome |
|---|---|
| No unsaved changes | Apply `edits` as one transaction marked `Transaction.addToHistory.of(false)`. The selection and the undo history are mapped through it, so the cursor on line 40 stays put and undo reaches every earlier local state. Undo does not revert the external edit: undoing someone else's change in your buffer would write it away at the next save. The base becomes the disk version. |
| Unsaved changes whose base ranges are **disjoint** from every external edit (ranges that touch count as overlapping) | Map the external changes through the local ones and apply them the same way. The base becomes the disk version, and the local ChangeSet is rebased over the external one. The buffer stays dirty, and the next save patches against the new hash. |
| Unsaved changes that **overlap** | **Conflict.** Apply nothing and keep the buffer as the user left it. Show a non-modal notice (§9.1: no modal on the write path) with three actions. *Keep mine* **re-bases the buffer onto the disk version**: the base `Text` becomes the disk text, the base hash becomes the disk hash, and the local ChangeSet is **recomputed as the diff from the disk text to the buffer**. The next save then patches disk into exactly what the user sees. Adopting the disk hash while keeping a ChangeSet computed against the old base would splice old-base coordinates into new bytes and corrupt the file. That was the first draft's rule, and review caught it. #23's hostile test asserts that the disk bytes equal the buffer, byte for byte, after a *Keep mine* save. *Take theirs* replaces the buffer in one **history-recorded** transaction, so undo brings back what was typed. *Compare* opens a diff. Nothing is discarded silently in any branch. |

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
3. **Not yet tested:** a note with a lone `\r` line ending, the only splitting rule CM6 and a naive Rust `lines()` disagree on. It is named here so that the shared table test includes it.
