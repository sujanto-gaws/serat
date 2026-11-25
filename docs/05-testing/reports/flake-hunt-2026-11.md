---
project: Serat
artifact: Flake hunt, 2026-11-24 sightings in serat-index and serat-app (task #182)
phase: 05-testing
owner: test-engineer
serves: NFR-REL-01, NFR-MNT-01, US-07
cites: architecture.md 10.3, ADR-0001, task #99, task #131, task #154, task #35, task #144, task #153, task #175, task #176
---

# Flake hunt: two one-off failures seen 2026-11-24 (task #182)

**Date:** 2026-11-25 · **Branch:** `task/p1-182-flake-hunt`, from `fe85178`. Linux container, 4 CPUs, debug builds, Rust 1.92. Rule honoured: no assertion is decided by wall-clock timing (#99, #131).

## Summary

| | |
|---|---|
| Sighting 1 (serat-index, name not captured) | **Two different tests fail under load, both reproduced.** (a) `checkpoint::tests::it_fires_once_the_writer_has_been_idle_and_not_before`: wall-clock assertion; fixed. (b) `a_second_open_beside_a_live_writer_is_refused_and_leaves_the_first_intact` in `tests/writer_lock.rs`: a spawned child process holds a copy of another test's `flock`; fixed. Which of the two was the 2026-11-24 failure is not knowable (the name was not captured); (a) is the likelier fit for "right after a recompile", (b) for the suspect the report named. Both are real. |
| Sighting 2 (`a_deleted_refused_file_leaves_the_list_only_at_the_next_walk`) | **Not reproduced by load alone (0 in 20).** Root cause found by reading and then forced: the watcher records a refused name on its *delete* event, a little after the `rm`, and the test did not wait for it. With the watcher armed and the test thread paused 400 ms after the second walk, the old test fails 3 of 3 at the sighting's assertion. Fixed by waiting on what the assertion depends on. |
| Product findings | One, low priority: the watcher records a refusal for a delete (`crates/serat-vault/src/watch.rs`, `Accepted::Refused`). Pinned by an `#[ignore]`d failing test. Task text below. One hazard to note: serat-app's `mkfifo` test spawns a process inside the same test binary that holds index locks (same mechanism as 1b). |
| Product code changed | None. |

## What ran

Two pinned CPU loops (`taskset -c 0` and `taskset -c 1`, `while :; do :; done`, PIDs 8468 and 8469) for the whole hunt, on top of the lead's and #179's builds, which is what was wanted. Both loops were killed by PID afterwards and checked gone (no `while :; do :; done` process left).

| Run | Count | Result |
|---|---|---|
| `cargo test -p serat-index --no-fail-fast`, default features, baseline code | 20 | **18 passed, 2 failed** (run 1: `checkpoint::tests::it_fires_once_the_writer_has_been_idle_and_not_before`, `checkpoint.rs:693`, `left: 1 right: 0`; run 8: `a_second_open_beside_a_live_writer_is_refused_and_leaves_the_first_intact`, `writer_lock.rs:78`, `HeldByAnotherProcess`) |
| `cargo test -p serat-app --lib --no-fail-fast`, baseline code | 20 | **20 passed, 0 failed** (433 tests each) |
| Targeted: the `writer_lock` test binary alone, baseline, two copies at once under the loops | 60 | **3 failed**, all `reopen after the holder closed: HeldByAnotherProcess` |
| Targeted: the same binary after the fix, same conditions | 60 | **0 failed** |
| "Right after recompile" (`touch crates/serat-index/src/lib.rs`, rebuild, run at once), run on the **fixed** tree | 5 | **5 passed, 0 failed** (every run rebuilt serat-index and its 20-odd test binaries first) |
| `cargo test -p serat-app --lib`, fixed tree, under the loops | 10 | **10 passed, 0 failed** |

Whether a recompile beside the run matters: nothing in either failure depends on a just-built binary. Both are load effects (the recompile is one way to make load), so the five recompile runs are reported for completeness and are not a separate condition that was found to bite.

## Sighting 1a: the idle-gate test decided by how long a loop of saves took

`crates/serat-index/src/checkpoint.rs`, `it_fires_once_the_writer_has_been_idle_and_not_before`. It saved every 30 ms for a second against a 300 ms idle gate and asserted that no checkpoint ran. One stall of the whole process longer than 300 ms between two saves (a loaded runner, a recompile on the same cores) lets the gate honestly expire, so the checkpointer is right and the test is wrong. Caught under load: `assertion left == right failed: a checkpoint ran while commits were arriving inside the idle gate, left: 1, right: 0`.

**Fix.** The test is split along what it asserted.
* `the_idle_gate_is_measured_from_the_last_commit_and_each_commit_moves_it`: the "not before" half, on the scheduler's own `State` with instants the test makes up: nothing is due until a commit; the gate is one `idle` after the last commit; ten commits 30 ms apart each move it; the pinned `RESTART` is gated the same way and only while unspent. No clock decides it.
* `it_fires_once_the_writer_has_been_idle`: the "fires" half on the live actor, an event wait on the checkpoint counter (the 30 s is a hang bound, not a decision).
* `idle_checkpoints_are_spaced_and_a_trailing_one_fires_after_typing_stops` had the same defect in its ceiling (`passive_runs <= 4` after a loop meant to last 2 s; a stall that stretches the loop legitimately allows a fifth). The ceiling is now `elapsed / spacing + 1` from the time the loop actually ran.

**Mutants (all red, then restored).** Gate ignores `idle` (`passive_at` returns `last_commit`): the pure test, the live test and `the_writer_does_not_auto_checkpoint` fail. Spacing removed from `passive_at`: the spacing test fails. The run loop stops moving `last_commit` on a commit: the live test fails. **Not covered by a time-free test:** that the *run loop* moves the gate on a commit is only exercised by the live test's firing, not by a "not before" assertion, because a negative claim needs time to pass. The pure test covers the formula the loop uses.

## Sighting 1b: a child process keeps another test's lock alive

`crates/serat-index/tests/writer_lock.rs`. The test closes its writer with `first.close()` and immediately reopens (`writer_lock.rs:78`), expecting the lock free. It is not, when another test in the same binary, `two_processes_a_second_process_is_refused_by_a_live_writer`, is at that moment inside `Command::spawn`.

Cause: `flock` belongs to the open file description, not the descriptor. Between `fork` and `exec` the child holds a copy of every descriptor the parent has open, including the lock file another test just "closed"; the lock lives until the child execs. Under load that window is tens of milliseconds. `lock.rs` is correct and the lock's release order (`_lock` declared last) is correct.

Proved outside the repo with a 30-line program (`pre_exec` parks the child before `exec`): after the parent closes its locked file, re-locking gives `WouldBlock` while the child is pre-exec and succeeds after it execs. The test hit it in 3 of 60 baseline runs of the binary and 0 of 60 after the fix; the same message each time.

**Fix.** `SPAWN_FENCE: RwLock<()>` in the test file. The two tests that spawn (`two_processes_a_second_process_is_refused_by_a_live_writer`, and the `--ignored` hammer) take it exclusively; the three tests that open writers take it shared. No writer is open anywhere in the process while a child is created. Product behaviour of the lock is unchanged.

**Mutant.** The fence's sole job is ordering, so its mutant is the baseline: removing it is the 3 in 60. 0 in 60 with it is weak by itself (a 5 percent rate gives zero in 60 about 5 percent of the time); the pre-exec program is the stronger evidence of cause.

**Hazard noted, not fixed.** `crates/serat-app/src/asset/tests.rs:796` spawns `mkfifo` inside the serat-app lib test binary, in which other tests hold and release index locks (`held_lock_tests.rs` asserts on exactly that). It has not failed in 20 baseline runs plus 10 after; it is the same hazard and the same fence would close it. Left for whoever next sees `HeldByAnotherProcess` there.

## Sighting 2: `a_deleted_refused_file_leaves_the_list_only_at_the_next_walk`

The test (now `state.rs:3821`; the sighting's 3721 was the same assertion on the older tree): write a file whose name is not valid UTF-8, open, `vault.walk()`, assert one refused name; delete the file; assert the list still has it ("not re-walked by a read"); `vault.walk()`; assert the list is empty and the status count is 0.

Who else writes the refusal log (`RefusalLog`, `crates/serat-vault/src/refusal.rs`):
1. **Any walk**, by `replace_scope`: the open's rebuild (the fixture has no index, so the rebuild's `NoteSource::entries` is `Vault::walk`, on the index actor's thread, `lib.rs:1658`), the tree's walk, the open's own catch-up trigger (which stands aside over a rebuilt index).
2. **The watcher**, `watch.rs:243`: `Accepted::Refused(refusal) => vault.refusals().record(refusal)`, for **every** event naming the path, a delete included, the moment its loop handles the event (no coalescing window for refusals). It is async: inotify, the notify thread, a channel, the loop (30 ms tick).

The window: `rm`; second walk clears the log; then the watcher handles the delete and records the name again, before the final `is_empty()`. Normally that handling lands after the test has finished, or before the walk; under load the test thread is paused between the walk and the assertion for as long as the watcher takes. Not reproduced by load alone in 20 runs (the window is a few microseconds wide and the watcher is armed only part-way through the test body). Forced: with the watcher armed first and the test thread paused 400 ms after the second walk, the old test fails 3 of 3 at the sighting's assertion (`assertion failed: state.refused_names().is_empty()`).

This is neither the #144 throttle nor `re_emit`: those publish the status; they do not write the list the assertion reads. They are not in this race.

**Fix (test only).** The assertion depends on two things being finished, so the test waits on what each publishes:
* `settled_state` (Ready, the tree walk settled, which also means the watcher is registered) replaces `state`: the open's walks are over and the watcher is armed *before* the delete, so the delete event definitely exists to be waited for.
* After the `rm`, a valid-named file is written and the test waits for its **row in the index** (`wait_for_row`). Events reach the watcher loop in the order the kernel made them, so once the later create has been handled and indexed, the delete of the refused name has been handled and nothing more is coming for it. Then the second walk and the assertions, unchanged in meaning.

**Mutants.** (1) The fixed test with a 400 ms pause after the second walk: passes 5 of 5 (the old test fails 3 of 3). (2) `replace_scope` that never clears (`if found.is_empty() { return; }`): the final assertion goes red at `state.rs:3858`.

## Product finding to file (not fixed here)

**Task: `[P1/impl] Watcher: a delete event must not record a refused name`.**
* Owner: rust-core-dev (`serat-vault`), with architect for the ruling. IDs: FR-VLT-01, NFR-REL-01, US-07; honours design goal 5 and the contract text that the list "may name a file that is already gone until the walk clears the log" (that sentence permits staleness left behind by a missed delete, not a delete *adding* a name the log did not hold).
* Reproduction: `cargo test -p serat-vault --lib the_delete_of_a_refused_name_does_not_add_it_to_the_log -- --ignored`. A file with an invalid-UTF-8 name exists before the watcher starts; the watcher starts; the file is deleted; `refusals()` then holds the name (`file name is not valid UTF-8`).
* Acceptance: that test passes with `#[ignore]` removed; `a_refused_name_is_recorded_not_queued` still passes (a *create* is recorded); a rename away from a refused name is judged by the architect (the old name is a delete event, the new one may or may not be refusable). When it lands, the second half of the sighting-2 test's wait (the sentinel row) can stay as it is; it is harmless.
* Effect today: the "N names not shown" count is one too high after a refused file is deleted, until the next whole walk. Low.

## Not reproduced

* Sighting 2 by load alone: 0 in 20 baseline runs (433 tests each) and 0 in the fixed runs. Reproduced only by forcing the window.
* The 2026-11-24 serat-index run's identity: two independent failures reproduce; which one it was cannot be known.

## Files

`crates/serat-index/src/checkpoint.rs`, `crates/serat-index/tests/writer_lock.rs`, `crates/serat-app/src/state.rs`, `crates/serat-vault/src/watch.rs` (the ignored pin). TC-164 in `docs/05-testing/test-plan.md` (TC-163 is reserved by #179).
