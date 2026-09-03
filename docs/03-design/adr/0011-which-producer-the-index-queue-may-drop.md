# ADR-0011: Which producer the index queue may drop
**Status:** accepted · **Date:** 2026-09-04 · **Serves:** FR-VLT-04, NFR-PRF-03, NFR-REL-01
**Source:** finding F-06, raised by rust-core-dev while implementing I2; sharpened by code-reviewer's F-11. Candidate text proposed by rust-core-dev and rewritten here.

## Context

Architecture §3.1.2 step 2 says the work queue "deduplicates by path and is bounded". It does not say what happens when the bound is reached, and the queue could not be implemented without deciding — so the decision was made in code, which is the wrong place for it. What happens on overflow determines whether an external change can be lost, and that is FR-VLT-04.

ADR-0001 is silent on it too. That silence is the actual gap: "bounded" reads as a statement about memory when it is a statement about correctness.

Two producers feed that queue, and they are not alike:

- **The watcher's coalescing thread.** It is draining the operating system's event buffer. It cannot be made to wait.
- **The bulk producer** — the initial index, a full rebuild, the catch-up scan at startup, and the rescan that recovers from an overflow. It is a loop over a `Vec` and can be made to wait at no cost.

The first version of this decision treated them as one producer, and that is what made it wrong.

## Options considered

1. **Block every producer until space frees.** The textbook answer, and correct for the bulk producer. Fatal for the watcher: blocking the coalescer does not stop events arriving, it stops us draining the OS event buffer, so the platform drops notifications instead — and neither `ReadDirectoryChangesW` nor inotify will tell you afterwards which ones. A visible, recoverable overflow becomes silent divergence between the index and the disk, which is the one failure C-02 cannot absorb: every other index defect is repaired by a rebuild, and this one is not detectable in the first place.
2. **Grow the queue without bound.** A `git checkout` of a large repository, or a sync tool catching up after a week offline, then costs memory proportional to the burst. "Bounded" exists to prevent exactly this.
3. **Drop from every producer, latch a flag, and rescan.** What was implemented first, and it does not work — see the consequence F-11 measured, below.
4. **Drop the watcher, block the bulk producer.** Chosen.

## Decision

**The watcher may be dropped. The bulk producer may not.**

When the queue is full and the producer is the watcher, the push is refused, an `overflowed` flag latches, and the consumer's contract on observing it is to walk the vault and reconcile. Overflow costs a full walk; it never costs a missed change.

When the queue is full and the producer is a bulk enumeration, the push blocks until there is room, or the walk hands its `Vec` to the consumer directly. Blocking is safe here precisely because the walk is not the coalescer, so the argument that justifies dropping the watcher does not apply to it.

The flag is latched rather than counted, because the response to one drop and to four thousand is identical. It is cleared **on completion of the rescan, not on read**, so a consumer that observes the flag and then dies mid-walk re-triggers the rescan instead of forgetting it.

Clear-on-completion needs one more turn of the screw, because the obvious implementation of it is also wrong: an overflow that happens *during* the rescan must not be cleared by that rescan finishing, or the walk that was supposed to recover the drops swallows a newer set of them. So the latch carries a generation counter. The consumer reads the generation when it starts walking, and at the end clears the flag only if the generation is unchanged; if it moved, the flag stays set and the consumer walks again. The counter is not a count of dropped paths — the response to one drop and to four thousand is still identical — it exists solely so that "the state I recovered from" and "the state now" can be told apart.

The default bound is 8,192 distinct paths.

## Consequences

Positive: the only path that can lose an event is the one that has a recovery mechanism, and the recovery mechanism no longer runs through the queue that lost it.

Negative: correctness now depends on the rescan actually happening. The risk moves from "events are lost" to "the flag is mishandled", which is a better place for it — a latched flag and a completion-clear are testable, and a dropped inotify event is not.

The risk this ADR exists to close was measured, not imagined. With every producer dropping, capacity 8,192, a 20,200-file vault, `walk()` returning sorted paths and a FIFO queue, the same 8,192 paths win the slots on every rescan: **12,008 files had never once reached the consumer after five rescans.** A stable loop rather than a slow one. It also meant the *first* index of a 20k vault silently dropped about 12,000 paths unless the consumer outpaced the walk, which a cold start cannot promise. That is the defect option 3 produces, and it is invisible without a test that counts what arrived.

The asymmetry then turned up a second time, independently, before this ADR was written. F-19: the *rebuild* dropped files under the same policy, and would have reported a fast benchmark number for a partly indexed vault — the same root cause in a second place, one queue policy serving two producers with opposite recovery properties. It was fixed the way this ADR requires, with a blocking submit kept deliberately off the `IndexWriter` trait so the save path cannot reach for it. Two independent occurrences in one spike is the evidence that the asymmetry is the decision and the policy is only its consequence.

This sharpens ADR-0001 rather than amending it: the single-writer actor's queue is not a uniform pipe, and which side of it a producer sits on is a property of the producer, not of the queue.

## Objections recorded

rust-core-dev proposed the drop-latch-rescan policy and implemented it in I2, and their reasoning against blocking the watcher is adopted here unchanged; the clear-on-completion refinement is also theirs, raised in the same message that proposed the ADR. code-reviewer's objection — that routing the bulk producer through the dropping queue makes the recovery path re-enter the loss — is the reason this ADR is about the asymmetry rather than about the policy, and it is upheld in full.

The lead's framing is recorded because it changed the shape of the decision: what needed recording was never *whether* dropping is allowed, but *which producer* may be dropped. An ADR that had answered the first question would have blessed the defect.
