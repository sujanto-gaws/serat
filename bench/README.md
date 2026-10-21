# bench/

The verification harness (architecture §10.3). Owned by test-engineer.

```
vaultgen/    the deterministic benchmark vault generator — committed
budget/      one asserting job per NFR-PRF budget — committed
vaults/      generated vaults and their manifests — gitignored, never committed
out/         budget results — gitignored
```

## Generate the benchmark vault

```
cargo run --release -p vaultgen -- --preset 20k
```

`--preset smoke` (400 notes) for a fast local check, `--preset 50k` for the scale run, `--help` for everything else. Regenerating over an existing vault needs `--force`; `--force` refuses to delete any directory that does not carry the `.vaultgen` stamp this generator writes.

One seed produces byte-identical output — that is NFR-MNT-02, and it is what makes a benchmark regression attributable to a code change rather than to a different vault. Each run writes `bench/vaults/<name>.manifest.json` beside the vault, carrying the seed, the SHA-256 tree digest, and the expected note, link, field and task counts a test can assert against.

**Full documentation — the vault's shape, the canonical digests, the asset caveats and the traceability table — is `docs/05-testing/benchmark-vault.md`.** The machine the budgets are measured on is `docs/05-testing/reference-machine.md`.

## Assert the budgets

```
cargo run --release -p budget --bin prf04-full-rebuild -- --profile reference
cargo run --release -p budget --bin prf03-save-to-index -- --profile reference
cargo run --release -p budget --bin prf01-cold-start   -- --profile reference
```

The P1 read paths (search, backlinks, property lookup) have latency *expectations* in architecture §4.5 and no budget, so the `p1-read-paths` job in `.github/workflows/budgets.yml` reports (the text lands in the `p1-read-paths` artifact) and never fails the build on a miss. It also times a save of the highest in-degree note and of namesake-heavy notes, the link re-resolution cost NFR-PRF-03 will assert once `prf03-save-to-index` can run:

```
cargo run --release -p budget --bin p1-read-paths -- --vault 20k   # and --vault 50k
```

Two diagnostic jobs, which measure and apply no remedy (task #53), attribute what §3.1.1 and §4.5 leave open. Neither gates. `p1-save-tail` runs 1,000 consecutive saves of the highest in-degree note at `--gap-ms` (1,000, the editor's debounce) after the previous `flush()`, timing read, parse, the transaction's statements, `COMMIT`, the auto-checkpoint and the post-commit work separately, with the WAL frame count at every commit, the FTS5 segments per level, and a `concurrent_build_processes` sample (taken from `/proc`, since `env.rs` reports none on Linux). A run on a machine with build processes running says so in its header. `--wal-autocheckpoint 0 --checkpoint-between` and `--fts-automerge 0` are the bisections, and `--bare` measures the instrumentation's own cost. `p1-search-gap` compares the search connection with pool readers under equal warm-up, swapped order and a second-pool-reader control, reads the pragmas back from each and shows from `SQLITE_STMTSTATUS_RUN` that `prepare_cached` hits. A reporting run of either needs a quiet machine and a disk with room for the index: the host's disk filled under one run.

```
cargo run --release -p budget --bin p1-save-tail  -- --vault 20k --iterations 1000 --gap-ms 1000
cargo run --release -p budget --bin p1-search-gap -- --vault 20k --iterations 60
```

Record the machine conditions either side of a series with `cargo run --release -p budget --bin environment`; on Windows a run with the on-access virus scanner disabled is marked not gate eligible, because the gate figure has to be the one a user would see.

All three are **red today, on purpose**: `serat-index` has no rebuild entry point (I4) or incremental save path (I5), and there is no application to start (D2, S1). Each prints the entry point it needs and the task that unblocks it. Documentation: `docs/05-testing/budget-harness.md`. CI: `.github/workflows/budgets.yml`, one job per budget.

## Release builds are per package, never `--workspace`

`budget` links `serat-index` with its `test-hooks` feature; a workspace-wide build unifies that feature into `serat-app`, whose const assert refuses any release build that carries it. So `cargo build/test --workspace --release` does not compile (debug is unaffected). Build release one package at a time (`-p budget`, `-p vaultgen`, `-p serat-app`); every command above and in `.github/workflows/budgets.yml` already does. The `serat-app-release-guard` job runs `cargo check -p serat-app --release` on every PR so the guard itself is exercised.

## Repository root

`budget`, `vaultgen` and `doclint` share one root rule (`bench/reporoot`): `--root <dir>`, else `SERAT_REPO_ROOT`, else the `[workspace]` manifest above the working directory, else the checkout the binary was built in. The doclint bins print `root: <path> (from <rule>)` as their first line, so the last (wrong whenever a binary is reused across worktrees) is never silent. A bare `--root` (or one followed by another flag) is an error. `vaultgen` and `budget` print the same line to stderr; `vaultgen` refuses to write under the compiled-in fallback (pass `--root`, `SERAT_REPO_ROOT` or `--out`). `p1-read-paths` and `p1-tree-path` have their own argument parsers and do **not** take `--root`; they pick the root up from `SERAT_REPO_ROOT` or the working directory only.

## Serves

`vaultgen`: NFR-MNT-02 · US-35 · C-06. `budget`: NFR-PRF-08 · US-35 · C-06, asserting NFR-PRF-01, NFR-PRF-03 and NFR-PRF-04.
