---
project: Serat
artifact: Bridge-side save_patch timed as shipped, with a per-phase breakdown (task #172, closing T3 of #147)
phase: 05-testing
owner: tauri-app-dev (with test-engineer)
serves: NFR-PRF-03, NFR-PRF-02, US-01
cites: architecture.md 9.3, ADR-0008, ADR-0019, C-04, task #147, #61
---

# Test Report: `save_patch` through the IPC bridge, release profile, per phase (task #172)

**Date:** 2026-11-24 · **Branch:** `task/p1-172-bridge-release-timing`, from `a27de35`.

**Read this first.** Every figure is a **Linux-container figure** (4 CPUs, Linux 6.18 container), **supporting evidence, not a gate number** (ADR-0019). **No budget attaches to it.** NFR-PRF-03's 50 ms is the save-to-index path, attested at 17.01 ms p95 on the benchmark vault's hub note; this is the *bridge-side cost of the `save_patch` command on a 5,000-line note*, a different quantity on a different note. NFR-PRF-02 asks about the keystroke, and the save runs off the webview's main thread. Nothing here is a PASS or a FAIL of either.

## Summary

| | |
|---|---|
| What was asked | Build the bridge in release, put per-phase timing inside `save_patch`, take the shipped-profile figure #147 could not, and say whether the second `apply_edits` plus `hash_bytes` in `editor.rs` is more than a quarter of the call |
| Release `save_patch` | **48.8 ms p50, 68.8 ms p95, 82.0 ms max** (n = 60, load 0.14 before and 0.08 after; client wall 51.3 / 71.2 / 86.3 ms) |
| Dev profile, for continuity | 339.4 / 437.7 / 457.4 ms (n = 60; #147: 319.9 / 375.8 / 453.2 in its probe). One stray single-core process was running, load 1.0, so this is looser than the release run |
| Where the time is | **`submit_to_index` is 42.7 ms of the 48.8 ms median (87.5%)**, in the dev profile 324 of 339 ms (95.5%). It is the index hand-off's synchronous read, hash and **parse of the note on the command thread**. A standalone parse of the same note takes 30.9 ms p50. It is not the write, not the hash, not the second apply |
| Second `apply_edits` + `hash_bytes` | **1.28 ms p50 of 48.8 = 2.6%** (1.32 ms with the base advance, 2.7%). **The quarter threshold is not crossed**: no follow-up for it |
| `Vault::write_patch` as a whole | 4.6 ms p50 (9.4%), of which the durable temp-file write 1.75 ms, apply 1.23 ms, rename plus directory fsync 1.08 ms, and four hashes 0.38 ms |
| Correction to #147 | #147 attributed the 7x opt-level scaling to "probably the hash and the edit application". The phases say otherwise: apply is 5.3 ms in dev and 1.2 in release, hash 0.2 and 0.1; the 7x is the note being parsed inside `submit_to_index` (324 ms to 42.7 ms), and #147's claim that the call "ends at submit, 0.03 ms p95" is true of the `p1-save-tail` `submit` phase and not of `AppState::save_patch`'s `submit_to_index`, which is that phase plus the read and the parse that precede it |

## Method

### The build

```
cd <checkout>
CARGO_TARGET_DIR=<dir> cargo build --release --features bench-bridge --example editor_ipc_bridge -p serat-app
node bench/editor-latency/record/save-patch-probe.mjs <dir>/release/examples/editor_ipc_bridge 60 1000
```

The profile is the workspace's own `release` (no custom profile; the workspace sets none), so it is what the shipped binary is built with. The bridge was unbuildable in release because `serat-app`'s `[dev-dependencies]` enable `test-hooks` on serat-index and serat-vault for every example, and `lib.rs` const-asserts that away outside `debug_assertions`.

**Option chosen: a `bench-bridge` feature on `serat-app` (the narrow form of option a).**

- Option (b), a bench crate depending on `serat-app`, is **not available**: architecture 2.1 says "nothing depends on serat-app", `crates/serat-core/tests/dependency_rule.rs::nothing_depends_on_serat_app` enforces it, and the bench tier's allowance (`BUDGET_MAY_LINK`) deliberately omits it. It would need an ADR.
- Option (c), keeping the dev-dependencies off the examples, is not expressible: Cargo builds `[dev-dependencies]` with their features for examples as for tests, and per-target selection does not exist for them.
- A custom profile alone cannot work, because `cfg!(debug_assertions)` is the only profile-derived condition the assertion can see; setting `debug-assertions = true` in a bench profile would pass the guard but measure a build the product never ships (checks on, `debug_assert!` live).
- The feature changes the assertion to `!TEST_HOOKS || cfg!(debug_assertions) || cfg!(feature = "bench-bridge")` and adds one compensating control: **`src/main.rs` carries `compile_error!` under the same feature**, so the `serat` binary (the only shipped artifact) cannot be built with it, in any profile. `cargo build --example` builds the library and the example, not the binary, so the bridge builds and the application does not. A plain `cargo build --release -p serat-app` is unchanged: the guard is exactly as strong as before for everything that ships.

What the release bridge still carries: the `test-hooks` code of serat-vault (`Vault::with_platform`, one refusal-table seam) and serat-index (the read path's progress-handler seam, a checkpoint observer), because the dev-dependencies still switch it on. None of it is on the `save_patch` path, which uses neither. This is the one remaining difference from the shipped binary, and it is stated rather than argued away.

### The phase timing

`serat_vault::phases` (new, `crates/serat-vault/src/phases.rs`): `begin()`, `mark(name)`, `finish() -> Vec<Phase>`, thread-local. A phase is named at its end and measures the time since the previous mark, so the phases partition the call. Behind the `phase-timing` feature of serat-vault (off by default; `serat-app`'s `bench-bridge` and its dev-dependencies turn it on). **Off, every function is an empty `#[inline(always)]` body and `finish` returns an empty `Vec` with no allocation**; on, a mark records only between `begin` and `finish` on the calling thread, so an instrumented build nobody observes pays one thread-local test. Eleven marks sit in `Vault::write_patch_core` and seven in `Editor::save_patch` and `AppState::save_patch`, each between two statements that already existed: no byte is read, hashed or written differently (C-04, ADR-0008; the write-back property and hostile tests are unchanged and green). The only code motion is in `Editor::save_patch`, where the `if let ... && let ... &&` chain became nested `if let`s so a mark can sit between the second apply and the second hash; the conditions and their order are the same. The bridge calls `begin` before and `finish` after `AppState::save_patch` and reports each phase in `/stats` as `save_patch.phase.NN.<name>`; a refused save's partial phases are dropped.

### The probe

`bench/editor-latency/record/save-patch-probe.mjs` (committed): a fresh 5,000-line synthetic note in the temp directory (the same generator as #147's 60-save probe), the bridge opens the vault, the note is opened with `note_opened`, then **60 one-character `save_patch` patches at line 2500, one per second** (the editor's autosave cadence), each on the hash the previous returned. It is #147's probe without `strace`. Report-only; it fails only if a save is refused. n = 60, not the n = 40 of #147's Playwright arm.

### Environment

| | |
|---|---|
| Machine | 4 CPUs, Linux 6.18 container |
| Load average | release run: 0.14 (1 min) before, 0.08 after. Dev run: 1.01 before, 1.56 after, with one stray single-core busy process of the agent's own that could not be stopped; the dev figure is for continuity only |
| Build | `rustc 1.92`, release as above; dev = `cargo build --example editor_ipc_bridge -p serat-app` |

## Results

`save_patch`, bridge-side milliseconds, n = 60:

| Build | p50 | p95 | max |
|---|---|---|---|
| **release (`--release --features bench-bridge`)** | **48.80** | **68.79** | **81.97** |
| dev (CI's build) | 339.42 | 437.69 | 457.43 |
| for reference, #147's 60-save probe, dev | 319.9 | 375.8 | 453.2 |
| for reference, #147, `opt-level=3` with debug assertions, Playwright arm n = 40 | 38.7 | 59.7 | 62.0 |

Per phase, release build, ms (the sum of the phase medians is 48.65 against a call median of 48.80; the medians of a sum are not the sum of the medians, so the columns are for reading shares, not for adding):

| # | Phase (ends at) | p50 | p95 | max | Share of p50 | Dev p50 |
|---|---|---|---|---|---|---|
| 00 | `app.open_lock` | 0.01 | 0.01 | 0.02 | 0.0% | 0.05 |
| 01 | `app.prepare` (args, edits, held-base lookup) | 0.01 | 0.01 | 0.01 | 0.0% | 0.05 |
| 02 | `vault.resolve` | 0.02 | 0.02 | 0.05 | 0.0% | 0.02 |
| 03 | `vault.path_lock` | 0.00 | 0.00 | 0.01 | 0.0% | 0.02 |
| 04 | `vault.read_base` | 0.13 | 0.18 | 0.50 | 0.3% | 0.13 |
| 05 | `vault.hash_before` | 0.10 | 0.17 | 0.20 | 0.2% | 0.19 |
| 06 | `vault.apply_edits` | 1.23 | 2.11 | 3.89 | 2.5% | 5.31 |
| 07 | `vault.hash_after` | 0.10 | 0.16 | 0.18 | 0.2% | 0.17 |
| 08 | `vault.stat_permissions` | 0.01 | 0.01 | 0.02 | 0.0% | 0.01 |
| 09 | `vault.write_temp_fsync` (create, write, fsync, mtime) | 1.75 | 2.46 | 7.46 | 3.6% | 1.79 |
| 10 | `vault.reread` (the re-read before the rename) | 0.09 | 0.24 | 0.51 | 0.2% | 0.10 |
| 11 | `vault.hash_reread` | 0.09 | 0.14 | 0.22 | 0.2% | 0.17 |
| 12 | `vault.rename_syncdir` (rename, directory fsync) | 1.08 | 1.44 | 3.55 | 2.2% | 1.13 |
| 13 | `app.write_patch_return` | 0.00 | 0.01 | 0.03 | 0.0% | 0.02 |
| 14 | `app.apply_edits_again` | 1.19 | 1.91 | 2.08 | 2.4% | 5.53 |
| 15 | `app.hash_again` | 0.09 | 0.14 | 0.16 | 0.2% | 0.16 |
| 16 | `app.advance_base` | 0.04 | 0.08 | 0.12 | 0.1% | 0.06 |
| 17 | `app.submit_to_index` | **42.71** | **60.68** | **67.32** | **87.5%** | **324.00** |

Grouped, release p50: `Vault::write_patch` (02 to 12) 4.6 ms, 9.4%; the second apply and hash (14, 15) 1.28 ms, 2.6%; `submit_to_index` (17) 42.7 ms, 87.5%; everything else under 0.1 ms. The four hashes cost 0.10 + 0.10 + 0.09 + 0.09 = 0.38 ms: **hashing is not a meaningful part of the call** at 5,000 lines. The write's durable half (09 plus 12) is 2.8 ms, which agrees with #147's strace figures (fsync p50 1.3 ms, rename 0.6 ms) allowing for a second directory fsync.

### What phase 17 is

`AppState::save_patch` ends with `submit_to_index`, which calls `IndexHandle::apply` (`crates/serat-index/src/incremental.rs:398`): it **loads the note again** (`source.load`: read and hash), **parses it** (`build_op`: `parser.parse` on the whole note), then submits the op to the writer's queue and records the echo. The parse runs on the command thread that called `save_patch`, not on the index writer. I did not instrument inside serat-index (it depends only on serat-core and cannot see `serat_vault::phases`), so phase 17 is not split. A standalone `CommonMarkParser::parse` of the same 5,000-line note (release, 60 repetitions, a throwaway example not committed) measured 30.85 / 44.87 / 83.41 ms and 31.59 / 52.53 / 65.94 ms p50 / p95 / max on two runs; that is about 72% of phase 17's median. The other roughly 11 ms (read, hash, `build_op`'s conversion, the queue send, `echo.record_echo`, the `activity().touch()`, in a process that is also running the index writer) is **not attributed**.

## What this does and does not say

- It says the shipped-profile `save_patch` on a 5,000-line note is about 49 ms p50 and 69 ms p95 in this container, and where it goes.
- It says #147's hypothesis (hash and apply) was wrong, and its statement that "none of the phases is inside the call" needs the exception that the `p1-save-tail` read and parse phases are inside it, run synchronously on the command thread.
- It does **not** say NFR-PRF-03 is met or missed. NFR-PRF-03's attested 17.01 ms p95 is the in-process hub-note save-to-index path on a small note, where `p1-save-tail` measured read 0.07 ms and parse 0.18 ms p95; here the note is 5,000 lines and the parse alone is about 31 ms. Whether NFR-PRF-03 is meant to hold for a note that size is a question for the requirement's owner, not something this report answers; **I have not run `p1-save-tail` on a 5,000-line note and no figure here is that quantity**.
- It does not say the editor is slow. The save runs off the webview main thread (`save_patch_off_thread`); #147 and #61 found the real arm no worse than the stub arm in frames. A 49 ms command on a worker thread is not a keystroke cost, though it is the interval in which a further save on the same note waits on `open_lock`.
- It is not WebView2, not Tauri's transport, not the reference machine (RM-1), and the bridge's loopback HTTP and JSON are outside the bridge-side figure (client wall is 2.5 ms higher at p50).
- The dev-profile figure rode a load of about 1 (one busy core of four).
- The release bridge carries the `test-hooks` code of the two crates, off the save path.

## Tests

| What | Where |
|---|---|
| phase recorder: order, charged-to-its-own-phase and sum, nothing outside `begin`..`finish`, `begin` discards a stale run, per-thread (5 tests, with the feature) | `crates/serat-vault/src/phases.rs` `tests_on` |
| the disabled build records nothing and `PHASE_TIMING` is false (1 test, without the feature: run by `cargo test -p serat-vault`; under `--workspace` the dev-dependencies turn the feature on, so this one is skipped there and the five above run) | `phases.rs` `tests_off` |
| `save_patch` reports its 16 phases in order, sum within 10 ms of the wall time around the call, the file bytes and hash are what they were; without a held base there is no second apply or hash; a save with no `begin` leaves nothing for `finish` | `crates/serat-app/src/editor.rs` (`a_timed_save_...`, `a_save_with_no_held_base_...`) |
| the release guard: `bench-bridge` is not a default feature and enables nothing but the inert timing; only dev-dependencies enable `test-hooks` (2 always-on tests); and, `#[ignore]`d for their release-profile `cargo check` cost and run in CI's `serat-app-release-guard` job: the shipped build compiles, the guard fires for either crate's `test-hooks`, the `serat` binary refuses `bench-bridge`, the example accepts it with its dev-dependencies' test-hooks on (4 tests) | `crates/serat-app/tests/release_guard.rs`, `.github/workflows/budgets.yml` |

**Shown able to fail** (each mutant reverted): dropping `app.hash_again`, and renaming `app.apply_edits_again`, each turn the ordering test red; not advancing the recorder's stamp turns the sum test red (in serat-vault with the feature, and through serat-app); making the disabled `finish` return a phase turns `tests_off` red; removing the `compile_error!` from `main.rs` turns the binary-refusal test red; removing `bench-bridge` from the library guard turns the example-accepts test red; deleting the guard turns the guard-fires test red. All six release-guard tests pass unmutated (86.8 s).

## Unresolved

1. **Phase 17 is not split** (about 31 of 42.7 ms is the parse; about 11 ms unattributed). Splitting it needs a timing seam in serat-index, which cannot use `serat_vault::phases` (it depends only on serat-core). Owner: rust-core-dev if wanted.
2. **A `save_patch` on a 5,000-line note parses the whole note on the command thread before returning**, about 31 ms in release here. Whether to move the parse after the return (the writer thread, as the `p1-save-tail` model assumes) is a design question for the architect and rust-core-dev; it does not change what is written to disk (C-04). It is **not** the second apply: that is 2.6% of the call.
3. NFR-PRF-03 on a note this size is unmeasured (see above).
4. The reference-machine run on WebView2 is still owed for NFR-PRF-02.
