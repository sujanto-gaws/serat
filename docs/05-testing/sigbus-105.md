# SIGBUS in the serat-app lib test binary: pressure hypothesis (task #105b), November 2026

Serves NFR-REL-01 (no crash path in the shipped test gate) · NFR-PRF-08 (budget jobs are part of the gate and must not destabilise it) · NFR-MNT-01 (the suite is trustworthy). Owner: test-engineer.

## Verdict: REPRODUCED under a concurrent build in the same target dir (1 of 5, round 2 of experiment 2). NOT reproduced by data pressure alone (experiment 1: 10 of 10 clean; experiment 2: 2 concurrent 3 GB writes clean).

The fault is not in the executable image. It is a SQLite WAL `-shm` page fault, at test 245 of 353 (`state::tests::a_benchmark_run_never_takes_the_note_from_the_settings_file` was the next test): the same neighbourhood as "test 238 of 336" in the lead's three crashes. See Experiment 2. The concurrent build is the trigger observed; whether it acts through CPU, memory or disk contention rather than through the relink itself is not separated (no control run), so the process rule below is the conservative fix.

## Experiment 1 (first report): data pressure only

Summary kept below; details in the Environment, Method and Results sections that follow.

## Environment

- Host: Linux 6.18, 16,086 MB RAM, no swap, 4 cores, `/dev/vda` 252 G thin-provisioned (`df` 258,020 MB size; 9,987 MB available at start).
- Toolchain: `rustup run 1.92 cargo`; lib test binary `.../te105b-target/debug/deps/serat_app-e6366411e7419a68`, built at `origin/claude/great-mccarthy-jywddn`. It holds 353 tests passing + 6 ignored, not the 336 of the lead's runs (the checkout is newer), so "test 238" is not directly comparable.
- `ulimit -c unlimited` was set in the driver script. `/proc/sys/kernel/core_pattern` is `core` (a plain file in the cwd of the crashing process; no systemd-coredump, no apport). I did not change it. No `core*` file appeared in the worktree. `dmesg` is readable but holds only boot lines and an `idle-reclaim ... drop_caches: 1` line from the VM agent; no SIGBUS or segfault line.
- Because the host's idle-reclaim agent issues `drop_caches`, the page cache figures below are not monotonic.

## Method

Driver: a bash script (scratchpad, not committed) that for i in 1..5 ran `cargo test -p budget` (generates and indexes the 20k vault under `bench/vaults`, includes `frontmatter_lists`), then immediately ran the lib binary directly. Then for i in 1..5 it wrote `dd if=/dev/zero bs=1M count=5000` into the scratchpad, removed it, then ran the binary. `free -m` and `df -m /` were sampled between steps.

Deviation from the brief: the brief asked for 8 GB with at least 4 GB free. Only about 10 GB was free at the start and the scratch target dir (9 GB) and growing vault caches consumed more per iteration, so I used 5000 MB. Even so, **the disk reached 100% (1 MB available) while the dd file existed in runs d1, d2 and d3**; the file was removed before the binary ran, so the binary never ran on a full disk. The 4 GB headroom rule was not held during the dd step. This is itself the useful finding about the CI host: `df` available fell about 3.8 GB for a 5.0 GB write because of thin provisioning, so `df` headroom is unreliable.

## Results

| Run | Step before | Step before time | `free` used/free/cache (MB) after step | Binary exit | Tests |
|---|---|---|---|---|---|
| b1 | `cargo test -p budget` | 174 s | 3830 / 4515 / 8327 | 0 | 353 pass |
| b2 | `cargo test -p budget` | 129 s | 2491 / 5132 / 9049 | **101** (test failure, not a signal) | 352 pass, 1 fail |
| b3 | `cargo test -p budget` | 182 s | 3355 / 3128 / 10189 | 0 | 353 pass |
| b4 | `cargo test -p budget` | 199 s | 4190 / 3344 / 9138 | 0 | 353 pass |
| b5 | `cargo test -p budget` | 206 s | 2568 / 3481 / 10623 | 0 | 353 pass |
| d1 | 5000 MB dd, removed | n/a | 2580 / 1647 / 12204 (at full disk) | 0 | 353 pass |
| d2 | same | n/a | 2012 / 1918 / 12501 | 0 | 353 pass |
| d3 | same | n/a | 1743 / 2041 / 12648 | 0 | 353 pass |
| d4 | same | n/a | 2204 / 2327 / 11900 | 0 | 353 pass |
| d5 | same | n/a | 1950 / 2609 / 11871 | 0 | 353 pass |

All five `cargo test -p budget` invocations exited 0. Binary wall time 8.3 to 10.2 s. Disk available fell from 9,987 MB to 3,908 MB across the five budget runs (build artefacts and vault caches); the vault itself under `bench/vaults` is 90 MB.

### Incidental finding: a different flake (run b2)

`state::tests::vault_opened_precedes_every_event_carrying_the_new_id_and_the_old_id_never_follows_it` failed once in 10 runs at `crates/serat-app/src/state.rs:5247`: "no index://changed for B in the run, so the ordering of it was not tested". The event list shows four `index://status` per vault and no `index://changed`. This is a timing dependence (the test requires an event that did not arrive within its window), unrelated to SIGBUS (it is an ordinary panic, exit 101). Failure came right after a 129 s budget run (page cache 9 GB), so load may widen it. This is a task for the owner (tauri-app-dev): wait on the event with a deadline rather than a fixed run, or fail loudly as a precondition. Reproduction: run the binary repeatedly under CPU or IO load; 1 failure in 10 here.

## SQLite mmap

No `PRAGMA mmap_size` is set anywhere in `crates/` or `bench/` (grep for `mmap`: only a read-back of the pragma in `bench/budget/src/bin/p1-search-gap.rs`). SQLite's default is 0, so database pages are read with `pread`, not mapped. What SQLite does map regardless is the WAL `-shm` file (always, in WAL mode). A SIGBUS there needs the backing file truncated or its blocks unavailable (ENOSPC on a sparse or thin-provisioned page) at fault time. The executable image is the other mapped file: a SIGBUS in it occurs if the binary is rewritten or truncated while running (for instance a concurrent `cargo build` replacing `target/debug/deps/serat_app-<hash>`), or on I/O error in the backing store. I did not verify how cargo replaces the file, but a second cargo invocation on the same target dir during the run is the cheapest explanation that matches "only in the full run".

## What this does not test

1. Memory pressure concurrent with the run (the binary ran after the pressure ended). A concurrent `dd` or a cgroup memory limit during the binary would test that.
2. Truncation of the executable image or a `-shm` file during the run (not simulated).
3. The 336-test checkout of the lead's runs, and `cargo test --workspace` ordering itself (the binary was run by hand, not by cargo).

## Proposed next steps (none applied; each is cheap and separable)

1. **Capture, so the next occurrence is diagnosable**: in the lead's checkout run `ulimit -c unlimited` and set `core_pattern` to a writable directory; or run the full workspace with `dmesg -w` in another shell. Without the fault address nothing here can be settled.
2. **Cheapest discriminating experiment**: run the lib binary while `dd` writes 5 GB (memory plus IO pressure concurrent), and a second variant while `touch`-truncating nothing but running `cargo build -p serat-app --tests` against the same target dir. If either reproduces, the cause is concurrency on the target dir or pressure, not the code.
3. **Mitigation if reproduced**: CI job order puts `-p serat-app` before `-p budget` (budget needs the large vault; app tests do not); `cargo test --workspace` and the budget jobs never share a target dir with a concurrent build; add a disk headroom check that measures a real write (not `df` available) before the budget job.
4. **Disk**: `df` available overstates headroom on this host by about 1.2 GB per 5 GB written, so any "keep 4 GB free" gate should be set at 8 GB or measured by writing.

## Experiment 2 (coordinator follow-up): relink under a running binary, and dd concurrent with the binary

Script (scratchpad, not committed). The target dir was rebuilt (`CARGO_PROFILE_DEV_DEBUG=0` to fit the disk; the 9 GB debug-info target no longer fitted in about 8 GB free). The binary dir is the same as before, `serat_app-e6366411e7419a68`. For round i in 1..5: start a loop that runs the lib binary (`--test-threads=1`, so a run lasts well over 9 s) back to back, wait 3 s, append `// relink probe i` to `crates/serat-app/src/lib.rs`, run `cargo test -p serat-app --lib --no-run` against the SAME target dir (this relinks the binary at the same path), stop the loop after the build, `git checkout` the file. Then twice: run the binary (default threads) while `dd` writes 3 GB concurrently.

| Round | inode before | inode after | build exit | binary runs |
|---|---|---|---|---|
| 1 | 2200796 | 2195518 | 0 | run 1 exit 0 (353 pass) |
| 2 | 2195518 | 2195550 | 0 | run 1 **exit 135 = SIGBUS** (core dumped; 244 tests had passed); run 2 exit 0 |
| 3 | 2195550 | 2195567 | 0 | exit 0 |
| 4 | 2195567 | 2195592 | 0 | exit 0 |
| 5 | 2195592 | 2195610 | 0 | exit 0 |
| dd-concurrent 1 | n/a | n/a | n/a | exit 0 (page cache 11.3 GB) |
| dd-concurrent 2 | n/a | n/a | n/a | exit 0 (page cache 10.8 GB) |

**Inode:** every relink produced a new inode, so the path is replaced, not rewritten in place. A process already running keeps its old, unlinked inode, which is why the executable image itself was never the faulting mapping.

**Core analysis (gdb on the core; the core is 75 MB, deleted after analysis):**
- `Program terminated with signal SIGBUS`, `si_code` 2 (`BUS_ADRERR`, no physical page behind the address), `si_addr` 0x7f2994dce000.
- Faulting frame: `__memset_avx512_unaligned_erms` called from `walIndexAppend` <- `walFrames` <- `sqlite3WalFrames`, i.e. SQLite appending to the WAL index.
- That address lies in the mapping `0x7f2994dcd000-0x7f2994dd5000 (0x8000)` of `/tmp/serat-app-state-benchfallback-24315/.serat/index.db-shm`, at offset 0x1000. The test fixture `benchfallback` is `crates/serat-app/src/state.rs:2653`.
- `/tmp` here is the same ext4 filesystem as everything else (`/dev/vda`, mounted `without journal`, "mounting unchecked fs" in dmesg), thin-provisioned. So the `-shm` is an ordinary file mapping on that disk, and the fault means the kernel could not supply a page for a mapped file at that instant. `dmesg` shows nothing further.
- Timing: the crash (02:08:18) fell while the concurrent `cargo` was building (build log finished 02:08:36), not at the relink instant itself.

### Revised verdict on the hypothesis
The mechanism in the hypothesis (a later page fault on a mapped file fails) is confirmed, and the mapped file is the WAL `-shm`, not the executable image. The trigger is a concurrent cargo build in the same checkout, which matches the lead's history (the TaskCompleted hook runs `cargo test --workspace` in the root during gates), not page-cache volume (dd alone: 0 of 7). One reproduction in 7 trials; the rate is low and a rerun is needed before quoting a probability.

### Fix (process rule first, mitigation second)
1. **Process rule:** no second cargo in a checkout whose test binaries are running, and CI jobs never share a target dir or a checkout. Finding **F-234** is where this belongs; the **TaskCompleted hook** that runs `cargo test --workspace` in the root must either be serialised with gate runs (a lock file around both) or run against its own `CARGO_TARGET_DIR` and worktree.
2. **Mitigation in code (owner: rust-core-dev / test fixtures):** point the state-dir fixtures (`serat-app-state-*`, `std::env::temp_dir()`) at tmpfs (`/dev/shm`) or give test connections `PRAGMA locking_mode=EXCLUSIVE` (SQLite then keeps the WAL index in heap memory and creates no `-shm` mapping). Either removes the file-backed mapping from the failing path. This changes test fixtures only; the shipped app keeps its WAL and `-shm` behaviour unless the architect decides otherwise.
3. **Still open:** a control run (the same loop with a CPU/memory-heavy but unrelated build in a different target dir) would tell whether the relink matters or any heavy neighbour will do. Not run; the disk could not hold a second target dir.
