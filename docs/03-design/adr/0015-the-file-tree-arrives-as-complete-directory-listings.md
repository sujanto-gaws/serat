# ADR-0015: The file tree arrives as complete directory listings, on a clock that starts at the painted window

**Status:** accepted · **Date:** 2026-09-24 · **Serves:** FR-VLT-01, NFR-PRF-01, NFR-CMP-03, FR-VLT-04, US-01
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

The tree crosses from Rust to the front end as a sequence of `DirListing { dir, entries }` messages. Each is **the complete, final-order contents of one directory** — never a partial one — and replaces any earlier listing for the same `dir`. Entries carry name, kind (`note` | `asset` | `dir`), size and mtime. Order is decided in Rust, once, per directory: directories first, then files; case-insensitive by name under Unicode simple case folding; ties broken by byte order. The front end never sorts.

**There is one message type, and it serves initial population, catch-up and live updates.** A directory changed on disk is re-listed and its listing re-sent; the front end replaces it. No delta format exists, so no delta can be applied in the wrong order.

Listings travel on an **ordered, lossless per-session stream** (a Tauri `ipc::Channel` bound to the open-vault session), never on the throttled `index://` event bus: §3.4 already rules that throttled events are for invalidation, never for establishing that something has occurred, and a tree listing is data. The stream ends its initial population with `TreeComplete { entries, source }`, where `entries` is the count of file entries delivered and `source` is `index` or `walk`.

### 2. Two sources, one shape

- **Warm path (index `Ready`).** One prepared, fixed-shape read over `notes(path, kind)` on a read connection (ADR-0001: a read, not a request to the writer), grouped into listings by a pure function in `serat-core`. The root listing is sent first and alone, then the rest breadth-first. Directories that contain no indexed file are absent until the walk arrives — a missing empty folder for a moment, which is design goal 5 rather than a defect.
- **No-index path (`Opening` that resolves to rebuild, `Rebuilding`, `Unavailable`).** A **breadth-first streaming tree walk** in `serat-vault`: each `read_dir` produces one listing, root first. It shares `walk`'s ignore rules, its symlink refusal (T-07) and its lexical prefix strip (F-12). `Vault::walk` — sorted `Vec`, used by the rebuild for rowid determinism — is unchanged. On the no-index path this is a second enumeration beside the rebuild's; that duplication is accepted and **is inside the measured number**, because the budget is taken with the rebuild running (§4).
- **Which one populates.** The index tree is sent only if no walk listing has been sent yet; otherwise the walk populates alone. `TreeComplete.source` records which.
- **The walk always runs, after the first paint, and is authoritative.** On the warm path its listings arrive after `TreeComplete{source: index}` and replace any that differ. Every path the walk finds that the index lacks, or the index holds that the walk did not find, is submitted to the index as catch-up on the **bulk, blocking side of ADR-0011** — the reconciliation-on-open that §3.4 records as missing (*"the open path still has no catch-up mechanism"*) gets its consumer here.
- **Live updates.** A watcher event on a path re-lists its parent directory after the 150 ms coalescing window and sends the listing. FR-VLT-04's 500 ms applies.

Batching for the channel: the root listing alone; thereafter listings coalesced into one message per ≤ 256 KiB or ≤ 16 ms of walking, whichever comes first. The numbers are tuning, not contract.

### 3. What "populated and navigable" means, measurably

**Clock start `T_open` = the later of `window_visible` and the `open_vault` command returning.** On a cold start with `--vault` that is `window_visible` (ADR-0014's mark, unchanged); when a user opens a vault from a running window it is the command's return. Either way the platform term is outside the interval by construction.

Two front-end marks, stamped the way `window_visible` is: after the Vue commit that applied the listing, on the second `requestAnimationFrame`, reported by IPC and timed on the Rust monotonic clock that stamps every other mark.

| Mark | Fires when | Status |
|---|---|---|
| `tree_first_screen` | The root listing has been applied **and painted**, and the rows in the viewport accept click and keyboard focus (NFR-USE-01). | Reported at 20k and 50k. |
| `tree_complete` | The `TreeComplete` message has been applied **and painted**: every listing of the initial population is in the front-end model. | **Binding.** |

**FR-VLT-01 is met when `tree_complete − T_open` ≤ 2000 ms at p95 at 20,000 notes, on each of two arms separately, never averaged** (§3.4 already forbids averaging a warm and a cold measurement):

- **warm arm** — a valid index, `Ready` at open (pre-warmed as F-450 requires), `source = index`;
- **no-index arm** — `.serat/` removed before each spawn, so the open check returns `NoIndex` and **the rebuild runs concurrently with the tree walk**, `source = walk`. That concurrency is the product's behaviour on first open, and measuring the walk without it is F-203's category error (an isolated benchmark for a cost that is about contention).

The stricter reading — *complete* — is the one gated, because this design satisfies both readings and a gate on the weaker one would pass a tree that shows its first screen and then takes ten seconds to fill. *Navigable* is carried by `tree_first_screen`, which is expected to be constant in vault size and is reported at both sizes so that expectation can fail visibly.

**Validity conditions a run must meet, or it is rejected rather than recorded:**

1. `TreeComplete.entries` and the front-end model's file-entry count at `tree_complete` both equal the **vaultgen manifest's** count of indexable files (notes plus assets under the default ignore rules) — taken from the manifest, never from the walk, because a walk that drops entries agrees with itself (F-19's shape).
2. `source` matches the arm.
3. `tree_first_screen ≤ tree_complete`, and both are after `T_open`.
4. The run records `cpu_load_percent`, `concurrent_build_processes` and `gate_eligible` like every other budget run.

Reported beside the verdict, gating nothing: `t0 → tree_complete` (what a user waits through), `tree_first_screen − T_open` at 20k and 50k, and `tree_complete − T_open` at 50k.

### 4. The front end's obligations

The tree renders through a **virtualised list**: DOM rows proportional to the viewport, never to the vault. Building the model is linear in entries and happens off the paint by applying listings in the order they arrive. Without virtualisation `tree_complete` becomes a DOM-size measurement; this clause is what makes the front-end half of the number sub-linear.

A directory whose listing has not arrived renders as a folder row in a pending state and becomes expandable when its listing lands. No request/priority mechanism exists in P1 (see *Trigger to revisit*).

### 5. NFR-PRF-01's `t_usable` tree clause changes subject

§3.4 defines `t_usable` as the latest of four marks, one being *"the file tree having been delivered to the front end"*, which P0 satisfied with `mark_buffer_rendered` because P0 had no tree. **From P1 that clause is satisfied by the first listing — the root — delivered to the front end**, on either source. Waiting for the whole tree would make `S_settle` proportional to vault size, which §3.4's load-bearing property forbids, and on the no-index path would put a rebuild-contended walk inside the Serat term's 350 ms. Root-listing size is a property of the vault's top level, not of its note count.

## Consequences

**Positive.** The time to a usable first screen stops depending on vault size on both paths. Initial population, catch-up and live updates share one message and one replace rule. The open path gains the catch-up reconciliation §3.4 recorded as absent, so a warm index that is stale in some rows is corrected in the session rather than trusted for its life. The FR-VLT-01 figure no longer carries the WebView2 term, so it measures something this repository can move.

**Negative.** Completion is still linear in vault size. The expectation, not a measurement, is that the strip plus the moved clock put the no-index arm at 20k well inside 2 s; at 50k it may not, and FR-VLT-01 says nothing about 50k, so that is reported rather than gated. The no-index path enumerates the vault twice (tree walk and rebuild walk). Empty directories are missing from a warm tree until the walk arrives.

**Risks.**
- The no-index arm's contention with the rebuild is unmeasured. If it breaches, the first remedy is to delay the rebuild's read-and-hash phase behind `tree_complete`, not to couple the rebuild to the tree walk.
- `T_open = window_visible` relies on ADR-0014's paint mark being honest; every guard that mark carries (and every one it lacks, ADR-0014's table) is inherited here.
- A front-end mark is again the thing timed, and F-470 applies: a front-end change is only exercised after `npm run build` and `cargo build --release`.

**Trigger to revisit.** If `tree_complete` at 20k on the no-index arm is within 20% of 2000 ms, or users in the P1 gate week wait on an unexpanded folder, add a `tree_prioritise(dir)` request that moves a pending directory to the front of the walk — option 3's idea inside option 5, not a replacement for it.

## Objections recorded

**rust-core-dev** (owns `serat-vault`, `serat-index`) and **test-engineer** (owns the bench, task #9) were consulted on the draft on 2026-09-24 with two specific questions each; the proposal and questions are in their inboxes. **No reply had arrived when this was committed, so their objections are absent, not waived** — and the places I expect them are named rather than left for them to find:

- *rust-core-dev*: whether the rebuild should consume the tree walk's output instead of enumerating separately. I think not — it would couple rebuild determinism (sorted rowids, FR-VLT-02) to a UI-ordered stream — but the duplicate enumeration is their cost to carry.
- *test-engineer*: whether the manifest count can be the guard's denominator for the no-index arm when the ignore rules are the product's (the manifest must count what the product's default ignore rules admit, or the guard fails for a correct run).

Replies will be recorded here, and if one changes the mechanism the ADR is amended before task #13 starts.

## Tested against

The F-193 numbers, decomposed: they include the platform term (1117 / 1687 ms, ADR-0014) and a pre-strip walk (3.1 s in situ, F-12), and those are the two terms this ADR removes from the gated quantity by clock and by landed code respectively. **Not yet tested against a post-strip in-situ measurement, which is the case that would falsify it** — the no-index arm at 20k with the rebuild running. Today's code has no tree to mark, so task #9's first run takes `walk_complete − window_visible` from the existing `--exit-after-walk` trace as the baseline — the post-strip in-situ walk F-204 said should be measured before anything was believed — and the budget job proper goes red until task #13 emits the two marks.
