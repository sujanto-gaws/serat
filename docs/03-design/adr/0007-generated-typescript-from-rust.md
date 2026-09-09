# ADR-0007: Generated TypeScript from Rust
**Status:** accepted (design baseline 1.0) · **Date:** 2026-08 · **Serves:** NFR-MNT-01
**Source:** Serat Design Description §11, decision ADR-07

## Context
One of ten decisions in the Serat design that would be expensive to reverse. See docs/03-design/architecture.md for the surrounding design.

## Options considered
Hand-written types on both sides; a schema language as the source of truth.

## Decision
ts-rs generates the front-end types; the committed output must match what the generator produces.

**This clause used to read *"the build fails if the committed output is stale"* and that was never true (F-509).** `#[ts(export)]` *writes* each type when the crate's tests run, so a stale committed file is **silently regenerated locally** and shows up as a **dirty git tree rather than a failed build**, and nothing in `.github/workflows/` diffs the regenerated output against what is committed. **So the property this ADR makes binding is currently held by whoever notices `git status`.** The mechanism is implemented; its enforcement is not. Rewritten to name **what must be true** rather than **what fails**, because the second is a claim about a guard and this document had no business making one it had not checked.

**What would enforce it** is a step that regenerates the types and diffs them: `TS_RS_EXPORT_DIR=… cargo test -p serat-contracts`, then compare against the ten committed files in `app/src/ipc/generated/`. **It is not task #110, and this paragraph said it was (F-511).** #110 diffs `crates/serat-app/webview` — **the vite bundle**: different generator, different trigger, different directory, and nothing in `.github/workflows/` or `bench/doclint/src/` references `ipc/generated` at all. **So CI returning buys the bundle check and leaves this clause exactly as unenforced as it is today**, which is the opposite of what naming #110 implied.

**And the correction is worth more than the fix, because I introduced this while removing a false claim about a guard.** The clause above used to assert a build failure that never existed; correcting it, I named a remedy that does not cover the property — **a second false statement about enforcement, written in the act of fixing the first.** F-511's finder caught it against their own prior assertion. The pattern is one this register now holds several times over, and the operative form is narrow enough to act on: **a remedy is a claim about a guard and has to be checked like one** — *which artefact, which generator, which trigger* — and pointing at a task number feels like evidence while supplying none.

Until such a step exists, this decision is **stated and unenforced**, and saying so is the difference between a gap and a surprise.

## Consequences
The IPC boundary cannot drift silently, which matters more here than usual because it is the only boundary. Costs a build step and requires the Rust types to be the ones designed for the wire.

**The first sentence is the aspiration and not the current state**, for the reason above: the boundary *can* drift silently today, and what stops it is a person reading `git status`. That is worth keeping visible here rather than only in §Decision, because *"cannot drift silently"* is the sentence someone will quote when they need to trust the boundary.

## Objections recorded
None at baseline. One has arrived since (F-509, task #102's per-ADR conformance pass), and it is about the form of the decision rather than its substance.

**ADR-0007's decision named an *outcome*; ADR-0012's named a *test*, and only one of them could be checked without inference.** Same document type, same author discipline, and the difference decided whether a reader could tell truth from intention. *"The build fails if the committed output is stale"* reads as a fact about the tree and is not checkable without going to look; *"the manifest names these five commands"* is a claim a reader can resolve against a file. **The lesson is not that this decision was wrong — the generator does work and the types do not drift by accident. It is that a decision phrased as an outcome cannot be audited, so it stays true-sounding for as long as nobody checks**, which in this case was from design baseline 1.0 until #102.
