# bench/

The verification harness (architecture §10.3). Owned by test-engineer.

```
vaultgen/        the deterministic benchmark vault generator — committed
budget/          one asserting job per NFR-PRF budget — committed
editor-latency/  NFR-PRF-02 keystroke-to-paint harness (Playwright) — committed, scaffolding only until P1 #5/#21 land
vaults/          generated vaults and their manifests — gitignored, never committed
out/             budget results — gitignored
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

Record the machine conditions either side of a series with `cargo run --release -p budget --bin environment`; on Windows a run with the on-access virus scanner disabled is marked not gate eligible, because the gate figure has to be the one a user would see.

All three are **red today, on purpose**: `serat-index` has no rebuild entry point (I4) or incremental save path (I5), and there is no application to start (D2, S1). Each prints the entry point it needs and the task that unblocks it. Documentation: `docs/05-testing/budget-harness.md`. CI: `.github/workflows/budgets.yml`, one job per budget.

## Editor-latency harness (NFR-PRF-02)

```
cd bench/editor-latency
npm install
npm test
```

Scaffolding only today — see `bench/editor-latency/README.md`. The p95 statistic is green; the subject (opening a real note in a real Serat window) is blocked on P1 tasks #5 and #21 and fails loudly rather than silently.

## Serves

`vaultgen`: NFR-MNT-02 · US-35 · C-06. `budget`: NFR-PRF-08 · US-35 · C-06, asserting NFR-PRF-01, NFR-PRF-03 and NFR-PRF-04. `editor-latency`: NFR-PRF-02, asserting once wired.
