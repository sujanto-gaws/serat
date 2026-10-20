# ADR-0015: The file tree arrives as complete directory listings, on a clock that starts at the painted window

**Status:** accepted — on PRD v1.6 (task #32, merged at `857af57`), which carries FR-VLT-01's acceptance criterion as §3 defines it; proposed until then, because §3 decided the PRD's open question on *complete* versus *first screen* and moved the clock start · **Date:** 2026-09-24 · **Serves:** FR-VLT-01, NFR-PRF-01, NFR-CMP-03, FR-VLT-04, US-01
**Honours:** ADR-0001 (the tree read is a read, never a writer request), ADR-0002 (the index is disposable, so the tree must stand without it), ADR-0011 (catch-up from the tree walk enters on the bulk side), ADR-0014 (the platform term is not ours)

## Context

FR-VLT-01: *given a folder of 20,000 .md files, when it is opened, then the file tree is populated and navigable within 2 seconds.* P0 measured the no-index path at **4180 ms median / 8099 ms p95 at 20k and 8666 / 13080 ms at 50k** (F-193; warm vault, scanner off, n=10 and n=8), roughly linear in vault size.

Three things are inside that number, and only one of them is the tree:

1. **The platform term.** F-193 ran from `t0` (process spawn) to `walk_complete`. ADR-0014 measured the platform term — loader, `tauri::Builder`, WebView2 — at **1117 ms median / 1687 ms p95** with no Serat code in it. On WebView2 that alone is 84% of FR-VLT-01's p95 budget before a directory is read.
2. **The canonicalising walk.** F-193 predates the lexical prefix strip; F-12 measured `canonicalize`-per-entry as 1.42 s of a 1.85 s isolated walk, and the in-situ walk at 3.1 s. The strip has since landed in `crates/serat-vault/src/walk.rs` (re-check: `grep -n 'relativise_walked' crates/serat-vault/src/walk.rs`). No in-situ figure has been taken since.
3. **The shape.** `Vault::walk` collects a sorted `Vec` and returns when the last file is found, so **the first row and the last row become visible at the same moment** (F-204). Whatever the constant, the time to show anything is proportional to the vault.

The first two are constant factors and the third is the one the task says cannot be fixed by a constant. PRD §Open questions (F-204, F-214) left open whether "populated and navigable" means *complete* or *first screen usable*; the bench (task #9) cannot be built until one precise definition exists, so this ADR states it.

It is an ADR because it is expensive to reverse on three counts: a budget job will assert the marks it defines; the IPC contract (task #7) will carry its wire shape; and it changes one of NFR-PRF-01's four `t_usable` preconditions, which ADR-0014's gate depends on.

## Options considered

1. **Keep the complete sorted walk, make it faster.** The strip is necessary and already landed. It leaves the shape: time-to-first-row is still time-to-last-row, so the design meets 20k by a margin that shrinks linearly and has no answer past it. Rejected as the design; kept as the implementation of the enumeration underneath option 5.
2. **Flat streaming walk in directory order, sorted client-side** (F-204's proposal). Removes the size-proportionality of the first row. But a directory's contents are not known to be complete until the whole walk ends, so the tree reorders under the user and "this folder is fully listed" has no moment — navigable and complete become unmeasurable. The global sort it needs is also unnecessary: a tree only ever needs **per-directory** order. Rejected in favour of its idea at a better grain.
3. **Lazy tree: list a directory only when the user expands it** (the VS Code model). Cheapest and size-independent. But the tree is never *populated* in the requirement's sense, folder counts and tree filtering become impossible, and it gives FR-VLT-01's bench nothing to time but one `read_dir`. Rejected as the primary; retained as the named trigger below if 50k proves too slow.
4. **Persist a tree cache in `.serat/`.** C-02 permits it (disposable), but it is the index by another name — the index already holds every path — and it would be a second derived store to keep in step. Rejected.
5. **Complete directory listings, breadth-first, from whichever source is ready — the index when it is `Ready`, a streaming walk when it is not — on a clock that starts when the vault is open in a painted window.** Chosen.

And on the clock:

6. **Measure from `t0`.** Would put ADR-0014's platform term inside FR-VLT-01's budget: 1687 ms of 2000 at p95 is spent by WebView2 before any Serat code matters, which makes the requirement a second, weaker cold-start budget rather than a tree budget. Rejected by ADR-0014 §Decision 2's attribution rule — a term is ours if a change confined to this repository can move it. The `t0`-based total is reported, unbudgeted, exactly as ADR-0014 reports its totals.

## Decision

### 1. The unit is a complete directory listing

The tree crosses from Rust to the front end as a sequence of `DirListing { dir, entries }` messages. Each is **the complete, final-order contents of one directory** — never a partial one — and replaces any earlier listing for the same `dir`. **An entry is a name and a kind (`note` | `asset` | `dir`), and nothing else.** Size and mtime are deliberately absent: the tree displays neither, and an entry that carried them would change on every save. Order is decided in Rust, once, per directory: directories first, then files; case-insensitive by name under Unicode simple case folding; ties broken by byte order. The front end never sorts.

**Three message types, and the cost of each is bounded by what changed.**

| Message | Sent for | Cost to the webview |
|---|---|---|
| `DirListing { dir, entries }` | Initial population; a catch-up listing that **differs** from what was sent (Rust keeps what it sent, so identical listings are never re-sent); a directory rename; the rescan that recovers from a watcher overflow (ADR-0011) | Proportional to that directory, and never triggered by a save. **A directory rename drops the old subtree** (one `TreeRemove` for the old name, which removes every descendant from the model) **and re-lists every descendant directory under the new name**, root of the renamed subtree first, so its cost is proportional to the subtree. |
| `TreeUpsert { dir, entry, after }` | A file or directory created, or the new name of a rename | One entry. `after` names the sibling it follows (`None` = first), so the front end inserts without comparing names and cannot disagree with Rust's collation |
| `TreeRemove { dir, name }` | A file or directory deleted, or the old name of a rename | One entry |

**Rust guarantees two ordering properties on the stream, and the front end relies on them** (#24's review). Both concern a live message that arrives before or after the listing of the directory it names, which can happen while the initial population or a catch-up walk is still in progress.

- **Any `DirListing` sent after a `TreeRemove` already reflects that remove.** A listing is built from the state at the time it is sent, never from a snapshot older than a remove already on the stream. So a removed entry cannot come back through a later listing.
- **Any `TreeUpsert` into a directory whose `DirListing` has not been sent yet is superseded by that listing.** When the listing arrives it contains the upserted entry (if the entry still exists), and it replaces whatever the front end held for that directory.

Given those two, the front end treats a `TreeRemove` for a directory it has no listing for as a no-op, and it may drop or keep an early `TreeUpsert`, because the listing that follows is authoritative either way. **These are serat-app's and serat-vault's obligations, not the front end's, and task #13 must test both in Rust**: a remove racing a pending listing, and an upsert racing a pending listing, each asserting the stream the front end would see.

**A content change to an existing file sends nothing at all.** Saving a note in a 5,000-entry flat folder therefore costs the tree zero messages, which is what design goal 2 and NFR-PRF-02 need from it. An earlier draft of this ADR re-sent the whole parent listing on every watcher event, and that was a keystroke-adjacent cost proportional to folder size (lead's review of task #2).

Listings travel on an **ordered, lossless per-session stream** (a Tauri `ipc::Channel` bound to the open-vault session), never on the throttled `index://` event bus: §3.4 already rules that throttled events are for invalidation, never for establishing that something has occurred, and a tree listing is data. The stream ends its initial population with `TreeComplete { entries, source }`, where `entries` is the count of file entries delivered and `source` is `index` or `walk`.

### 2. Two sources, one shape

- **Warm path (index `Ready`).** One prepared, fixed-shape read over `notes(path, kind)` on a read connection (ADR-0001: a read, not a request to the writer), grouped into listings by a pure function in `serat-core`. The root listing is sent first and alone, then the rest breadth-first. Directories that contain no indexed file — empty folders, or folders holding only ignored files — are absent until the walk arrives. **This is the requirement's scope, not a gap in it** (lead's ruling, taken to the PRD): FR-VLT-01's gated mark covers *every file the ignore rules admit and every folder that contains one*. Empty folders appear when the walk completes, and their arrival is reported, not gated.
- **No-index path (`Opening` that resolves to rebuild, `Rebuilding`, `Unavailable`).** A **breadth-first streaming tree walk** in `serat-vault`: each `read_dir` produces one listing, root first. It shares `walk`'s ignore rules, its symlink refusal (T-07) and its lexical prefix strip (F-12). `Vault::walk` — sorted `Vec`, used by the rebuild for rowid determinism — is unchanged. On the no-index path this is a second enumeration beside the rebuild's; that duplication is accepted and **is inside the measured number**, because the budget is taken with the rebuild running (§4).
- **Which one populates.** The index tree is sent only if no walk listing has been sent yet; otherwise the walk populates alone. `TreeComplete.source` records which.
- **The walk always runs, after the first paint, and is authoritative.** On the warm path its listings arrive after `TreeComplete{source: index}` and replace any that differ. Every path the walk finds that the index lacks, or the index holds that the walk did not find, is submitted to the index as catch-up on the **bulk, blocking side of ADR-0011** — the reconciliation-on-open that §3.4 records as missing (*"the open path still has no catch-up mechanism"*) gets its consumer here.
- **Live updates.** After the 150 ms coalescing window, a watcher event that creates or deletes a path sends one `TreeUpsert` or `TreeRemove`, and a modification sends nothing. The front end applies an upsert or remove as one keyed insertion into the virtualised list, never as a rebuild of the directory. FR-VLT-04's 500 ms applies.

Batching for the channel: the root listing alone; thereafter listings coalesced into one message per ≤ 256 KiB or ≤ 16 ms of walking, whichever comes first. The numbers are tuning, not contract.

### 3. What "populated and navigable" means, measurably

**Clock start `T_open` = the later of `window_visible` and the `open_vault` command returning.** On a cold start with `--vault` that is `window_visible` (ADR-0014's mark, unchanged); when a user opens a vault from a running window it is the command's return. Either way the platform term is outside the interval by construction.

Two front-end marks, stamped the way `window_visible` is: after the Vue commit that applied the listing, on the second `requestAnimationFrame`, reported by IPC and timed on the Rust monotonic clock that stamps every other mark.

| Mark | Fires when | Status |
|---|---|---|
| `tree_first_screen` | The root listing has been applied **and painted**, with the viewport's rows interactive. The observable event: the tree component's post-flush watcher (`flush: 'post'`) has run for the commit that rendered those rows. Vue binds a row's click and keydown handlers when it creates the element, so a row in the DOM after that flush is clickable and focusable (NFR-USE-01). The mark is then taken on the second `requestAnimationFrame`. | Reported at 20k and 50k; also NFR-PRF-01's tree precondition (§5). |
| `tree_complete` | The `TreeComplete` message has been applied **and painted**: every listing of the initial population is in the front-end model. | **Binding.** |

**FR-VLT-01 is met when `tree_complete − T_open` ≤ 2000 ms at p95 at 20,000 notes, on each of two arms separately, never averaged** (§3.4 already forbids averaging a warm and a cold measurement):

- **warm arm** — a valid index, `Ready` at open (pre-warmed as F-450 requires), `source = index`;
- **no-index arm** — `.serat/` removed before each spawn, so the open check returns `NoIndex` and **the rebuild runs concurrently with the tree walk**, `source = walk`. That concurrency is the product's behaviour on first open, and measuring the walk without it is F-203's category error (an isolated benchmark for a cost that is about contention).

The stricter reading — *complete* — is the one gated, because this design satisfies both readings and a gate on the weaker one would pass a tree that shows its first screen and then takes ten seconds to fill. *Navigable* is carried by `tree_first_screen`, which is expected to be constant in vault size and is reported at both sizes so that expectation can fail visibly.

**What `tree_complete` covers:** every file the ignore rules admit and every folder that contains one. An empty folder is not part of the gated mark: it appears when the walk completes, and that arrival is reported beside the verdict.

**Validity conditions a run must meet, or it is rejected rather than recorded:**

1. `TreeComplete.entries` and the front-end model's file-entry count at `tree_complete` both equal the **vaultgen manifest's `notes + assets`**. That is 20,000 + 200 = 20,200 on the 20k vault, the count the P0 walk returns (ADR-0011's worked shape; T-07's 20,200 files). The manifest counts exactly what the default ignore rules admit: `notes` excludes `.trash/`, the `.obsidian/` and `.trash/` files are counted separately as `ignored_files`, and the `.vaultgen` stamp is in `files` only (`bench/vaultgen/src/lib.rs`, `Manifest`). The count is taken from the manifest and never from the walk, because a walk that drops entries agrees with itself (F-19's shape).
2. `source` matches the arm.
3. `tree_first_screen ≤ tree_complete`, and both are after `T_open`.
4. The run records `cpu_load_percent`, `concurrent_build_processes` and `gate_eligible` like every other budget run.

Reported beside the verdict, gating nothing: `t0 → tree_complete` (what a user waits through), `tree_first_screen − T_open` at 20k and 50k, `tree_complete − T_open` at 50k, and **empty folders: their count, and their time of arrival after `tree_complete`** (from `TreeMessage::WalkSettled`).

### 4. The front end's obligations

The tree renders through a **virtualised list**: DOM rows proportional to the viewport, never to the vault. Building the model is linear in entries and happens off the paint by applying listings in the order they arrive. Without virtualisation `tree_complete` becomes a DOM-size measurement; this clause is what makes the front-end half of the number sub-linear.

A directory whose listing has not arrived renders as a folder row in a pending state and becomes expandable when its listing lands. No request/priority mechanism exists in P1 (see *Trigger to revisit*).

### 5. NFR-PRF-01's `t_usable` tree clause changes subject (amends ADR-0014)

§3.4 defines `t_usable` as the latest of four marks. One of them is *"the file tree having been delivered to the front end"*, which P0 satisfied with `mark_buffer_rendered` because P0 had no tree. From P1:

- **"Delivered" is the concrete front-end mark `tree_first_screen` (§3)**, not the Rust send. It is the root listing applied and painted, on either source. The front-end apply is chosen over the Rust send because §3.4's own rule is that usable means *the front end has it*, and a send can succeed into a webview that has not rendered it.
- **`buffer_rendered` leaves `t_usable`.** It was only ever the stand-in for a tree P0 did not have, and §3.4's ruling (*a note being open is not one of the four*) already says a restored buffer does not gate usability. It stays in the trace as a reported, unbudgeted mark.
- **Why the root and not the whole tree.** Waiting for the whole tree would make `S_settle` proportional to vault size, which §3.4's load-bearing property forbids. On the no-index path it would also put a rebuild-contended walk inside the Serat term's 350 ms. How big the root listing is depends on the vault's top level, not its note count. On the warm arm NFR-PRF-01 measures, it comes from one index read about 10 ms after the index is `Ready`.
- **This changes what NFR-PRF-01's binding term measures, so the benchmarks change with it (working agreement 4).** `prf01-serat-term` and `prf01-cold-start` must require `tree_first_screen` in the trace in place of `buffer_rendered` as the tree precondition of `vault_usable`, and the harness's check of which mark settles `t_usable` must be updated to match. The seven-mark partition and its terms are unchanged; only `S_settle`'s end-mark precondition moves. The Serat term is then re-measured on the first build that renders a tree (after tasks #13, #18 and #24), and the figure is compared against the 350 ms budget with its basis clauses — this is a test-engineer task (lead creates it). Until that re-measurement, NFR-PRF-01's attested 233.8 ms p95 is a figure for the *buffer-rendered* precondition and must be labelled that way.

## Consequences

**Positive.** The time to a usable first screen stops depending on vault size on both paths. Initial population and catch-up share one message and one replace rule. Live updates cost one entry each, and a save costs the tree nothing. The open path gains the catch-up reconciliation §3.4 recorded as absent, so a warm index that is stale in some rows is corrected in the session rather than trusted for its life. The FR-VLT-01 figure no longer carries the WebView2 term, so it measures something this repository can move.

**Negative.** Completion is still linear in vault size. The live-update path adds two message types beside the listing, and a front end that applies an upsert at the wrong `after` position would be wrong silently, so task #24 needs a test that replays a random create/delete sequence and compares the result against a fresh listing. The expectation, not a measurement, is that the strip plus the moved clock put the no-index arm at 20k well inside 2 s; at 50k it may not, and FR-VLT-01 says nothing about 50k, so that is reported rather than gated. The no-index path enumerates the vault twice (tree walk and rebuild walk). Empty directories are missing from a warm tree until the walk arrives.

**Risks.**
- The no-index arm's contention with the rebuild is unmeasured. **If `tree_complete` breaches, the remedy is to hold the rebuild's read-and-hash phase until `tree_complete`**, not to couple the rebuild to the tree walk. That deferral is not free: it adds the tree's completion time to the rebuild's wall clock, and NFR-PRF-04 sits at 27.7 s against 30 s (task #96). A deferral longer than about 2 s spends the whole margin, so it can only be adopted with an NFR-PRF-04 re-measurement beside it.
- `T_open = window_visible` relies on ADR-0014's paint mark being honest; every guard that mark carries (and every one it lacks, ADR-0014's table) is inherited here.
- A front-end mark is again the thing timed, and F-470 applies: a front-end change is only exercised after `npm run build` and `cargo build --release`.

**Triggers to revisit, and they are two different problems.** A `tree_complete` breach, or a result within 20% of 2000 ms at 20k on the no-index arm, is answered by the rebuild deferral above; `tree_prioritise` would not help it, because completion needs every directory listed. **Users in the P1 gate week waiting on an unexpanded folder** is a navigability problem, and it is answered by a `tree_prioritise(dir)` request that moves a pending directory to the front of the walk. That is option 3's idea inside option 5, not a replacement for it.

## Objections recorded

**rust-core-dev** (owns `serat-vault`, `serat-index`) and **test-engineer** (owns the bench, task #9) were consulted on the draft on 2026-09-24 with two specific questions each; the proposal and questions are in their inboxes. **No reply had arrived when this was committed, so their objections are absent, not waived** — and the places I expect them are named rather than left for them to find:

- *rust-core-dev*: whether the rebuild should consume the tree walk's output instead of enumerating separately. I think not — it would couple rebuild determinism (sorted rowids, FR-VLT-02) to a UI-ordered stream — but the duplicate enumeration is their cost to carry.
- *test-engineer*: whether the manifest count can be the guard's denominator when the ignore rules are the product's. **Answered from the code since, without waiting for them:** the manifest's `notes + assets` is exactly what the default ignore rules admit (§3, validity condition 1). Their objection, if they have one, is still wanted on the rest.

Replies will be recorded here, and if one changes the mechanism the ADR is amended before task #13 starts.

**lead** (review of task #2, 2026-09-24) raised the objection that changed the mechanism: re-sending a whole parent listing on every watcher event put a folder-sized IPC payload, and a folder-sized replace on the webview's main thread, behind every save. Upheld. Live updates became single-entry messages without size or mtime (§1). The same review moved the rebuild deferral ahead of `tree_prioritise` as the remedy for a completion breach, made `tree_first_screen` an observable event, pinned "delivered" to that front-end mark, and returned this ADR to *proposed* until the PRD carries its acceptance wording.

## Tested against

The F-193 numbers, decomposed: they include the platform term (1117 / 1687 ms, ADR-0014) and a pre-strip walk (3.1 s in situ, F-12), and those are the two terms this ADR removes from the gated quantity by clock and by landed code respectively. **Not yet tested against a post-strip in-situ measurement, which is the case that would falsify it** — the no-index arm at 20k with the rebuild running. Today's code has no tree to mark, so task #9's first run takes `walk_complete − window_visible` from the existing `--exit-after-walk` trace as the baseline — the post-strip in-situ walk F-204 said should be measured before anything was believed — and the budget job proper goes red until the two marks exist (tasks #13, #18 and #24).
