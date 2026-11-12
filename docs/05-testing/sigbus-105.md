# SIGBUS in the serat-app lib test binary: pressure hypothesis (task #105b), November 2026

Serves NFR-REL-01 (no crash path in the shipped test gate) · NFR-PRF-08 (budget jobs are part of the gate and must not destabilise it) · NFR-MNT-01 (the suite is trustworthy). Owner: test-engineer.

## Verdict: NOT REPRODUCED (10 of 10 sequenced runs exit without a signal). The hypothesis is neither confirmed nor refuted.

Absence of a repro on this host is weak evidence: the original crashes were on the lead's checkout, and 50 loaded runs of the binary alone also never reproduced it. What this experiment does show is that **budget generation followed by the binary, and 5 GB of dirty page cache followed by the binary, are not sufficient on a 16 GB / thin-provisioned VM**. If the fault needs memory pressure at the same time as the page-fault, the sequencing used here (binary runs after the pressure has ended) would not produce it. See "What this does not test".

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
