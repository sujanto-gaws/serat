# bench/

The verification harness (architecture §10.3). Owned by test-engineer.

```
vaultgen/    the deterministic benchmark vault generator — committed
vaults/      generated vaults and their manifests — gitignored, never committed
```

## Generate the benchmark vault

```
cargo run --release -p vaultgen -- --preset 20k
```

`--preset smoke` (400 notes) for a fast local check, `--preset 50k` for the scale run, `--help` for everything else. Regenerating over an existing vault needs `--force`; `--force` refuses to delete any directory that does not carry the `.vaultgen` stamp this generator writes.

One seed produces byte-identical output — that is NFR-MNT-02, and it is what makes a benchmark regression attributable to a code change rather than to a different vault. Each run writes `bench/vaults/<name>.manifest.json` beside the vault, carrying the seed, the SHA-256 tree digest, and the expected note, link, field and task counts a test can assert against.

**Full documentation — the vault's shape, the canonical digests, the asset caveats and the traceability table — is `docs/05-testing/benchmark-vault.md`.** The machine the budgets are measured on is `docs/05-testing/reference-machine.md`.

## Serves

NFR-MNT-02 · US-35 · C-06, and it is the artefact NFR-PRF-01, NFR-PRF-03 and NFR-PRF-04 are measured against.
