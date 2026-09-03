---
project: Serat
artifact: Developer setup
phase: 04-implementation
owner: rust-core-dev
---

# Developer Setup

Everything needed to build, run and test Serat's Rust core. The frontend (`app/`)
and the Tauri shell (`crates/serat-app`) are not scaffolded yet; this document grows
when they arrive.

## Prerequisites

| Tool | Version | Why |
|---|---|---|
| Rust | 1.92 | Pinned in `rust-toolchain.toml`, with `rustfmt` and `clippy`. `rustup` installs it automatically on the first cargo command in this directory. |
| A C compiler | any | `rusqlite` is built with the `bundled` feature, which compiles SQLite from source. Windows: "Desktop development with C++" from the Visual Studio Build Tools. macOS: `xcode-select --install`. Linux: `build-essential`. |
| Git | 2.5+ | Worktrees. Each teammate builds in their own worktree under `.worktrees/`; see `coding-standards.md`. |

Node and the Tauri CLI are not needed yet and are deliberately not listed. They
arrive with tasks S1 and S2.

Nothing else is required: the workspace vendors no tooling, and every dependency
comes from crates.io on the first build.

### Why the toolchain is pinned

`NFR-PRF-08` puts performance budgets in CI, and a budget number is only comparable
between two runs if the compiler is the same in both. Bumping `rust-toolchain.toml`
is therefore a deliberate act, and the P0 measurement report records the toolchain
the numbers were taken with.

## Install

```sh
git clone <repo>
cd AIResearch
cargo build --workspace     # ~45 s cold, most of it compiling SQLite
```

Teammates work in a worktree rather than in the root working copy, which the lead
owns as the integration copy:

```sh
cd .worktrees/rust-core
git switch -c task/I2-vault-walk-watcher main
cargo build --workspace     # cold again: each worktree has its own target/
```

Every worktree contains a full copy of the workspace, which is why the root
manifest carries `exclude = [".worktrees"]` — without it cargo discovers those
copies as nested workspaces and every command in the root fails.

The first build compiles SQLite from source. If it fails with a linker or `cc`
error, the C compiler above is missing; nothing else in the tree needs one.

## Run locally

There is no application to run yet — P0 is a spike and the shell is task S1. What
exists is a library workspace plus the benchmark vault generator:

```sh
cargo run -p vaultgen -- --help      # deterministic benchmark vault (NFR-MNT-02)
```

`docs/05-testing/benchmark-vault.md` carries the documented invocation and is the
authority on it; test-engineer owns both it and the generator.

## Run tests

```sh
cargo test --workspace                              # everything
cargo test -p serat-core                            # one crate
cargo test -p serat-core --test dependency_rule     # the dependency rule alone
```

Before asking for review, all three of these must be clean:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

`--all-targets` matters: without it clippy skips test code, which is where most of
the code in this workspace currently lives.

### The dependency rule is a test, not a convention

Architecture §2.1 says the crate graph is "strict enough to be checked
mechanically". `crates/serat-core/tests/dependency_rule.rs` is what makes that
true. It runs `cargo metadata --no-deps`, and fails when:

- a crate takes an internal dependency other than `serat-core` (dev- and
  build-dependencies included — "it is only for tests" still couples the two
  crates);
- anything depends on `serat-app`;
- `serat-core` grows a dependency outside its allow-list, which is how "performs no
  I/O whatsoever" is enforced rather than remembered;
- a new workspace member appears that nobody has placed in the graph. It fails
  until someone adds it to the `POLICY` table, so growth is a decision rather than
  a default;
- anything in the product depends on a crate under `bench/`. The arrow only runs
  the other way: a bench crate may link the product to measure it, and the budget
  harness (task B2) must, but a benchmark tool has no business in a shipped binary.

It also asserts that no manifest inside a teammate's `.worktrees/` copy was picked
up. It asks cargo rather than walking the tree, and the root manifest excludes
`.worktrees`; a checker that tripped over the repo layout would not be a checker.

The check is itself tested, against graphs that violate the rule on purpose — a
guard nobody has ever seen fail is not yet a guard. Crossing the rule deliberately
needs an ADR first (CLAUDE.md, working agreement 3); the `POLICY` table is where
that ADR's outcome gets recorded.

## Workspace layout

```
Cargo.toml              virtual manifest; all dependency versions pinned here
rust-toolchain.toml     1.92 + rustfmt + clippy
crates/serat-core/      domain types, traits, error taxonomy. No I/O.
crates/serat-vault/     walk, watch, hash, atomic write. No Markdown.
crates/serat-parse/     bytes in, structures out. No file access.
crates/serat-index/     SQLite schema, single-writer actor, rebuild.
bench/vaultgen/         deterministic benchmark vault generator (test-engineer)
schema/ grammar/ contracts/   repo-ready design files (architect)
docs/                   SDLC artifacts by phase
```

`serat-sq`, `serat-extract`, `serat-ai` and `serat-mcp` are named in the
architecture but deliberately not scaffolded: P0 is a spike, and an empty crate for
a P2 capability is noise. `serat-app` is added with task S1.

Dependency versions are pinned once in the root `[workspace.dependencies]` and
referenced by crates with `workspace = true`, so no crate can quietly resolve a
different major than the rest of the workspace.

| Crate | Version | Chosen by |
|---|---|---|
| comrak | 0.54.0, `default-features = false` | Charter (Markdown). Default features pull in `syntect` and `clap` — a syntax highlighter and a CLI parser a library has no use for. Turning them off takes the graph from 134 crates to 60 and a cold debug build from ~45 s to ~5 s. |
| notify | 8.2.0 | Charter (watcher). |
| rusqlite | 0.40.2, `bundled` | Charter (SQLite). Bundled so the index does not depend on whatever `libsqlite3` the host ships; `serat-index` asserts the bundled version is ≥ 3.40, which FTS5 and later `sqlite-vec` need. |
| blake3 | 1.8.7 | Charter (hashing). |
| rayon | 1.12.0 | Charter (parse pool). Pinned at workspace level; added to the crate that owns the pool in task I5. |
| serde / serde_json / thiserror | 1.0.229 / 1.0.151 / 2.0.20 | Supporting. |

## Environment variables

None. Serat is local-first and configured by a TOML file in the platform config
directory (FR-PLT-05); there is no environment-variable configuration to document,
and there are no secrets in the tree.

Two cargo variables are worth knowing during development:

| Name | Purpose | Example |
|---|---|---|
| `RUST_BACKTRACE` | Backtrace on a test failure. | `RUST_BACKTRACE=1 cargo test -p serat-core` |
| `CARGO_TARGET_DIR` | Build somewhere other than the worktree — a faster disk, or off a synced folder. Do not point two worktrees at one directory: they will queue on the same build lock. | `CARGO_TARGET_DIR=D:\build\serat` |

## What is deliberately not here yet

- **No CI configuration.** The budget jobs are test-engineer's task B2.
- **No schema bootstrap.** `schema/001_init.sql` exists (architect's task D1), but
  `serat-index` does not read it yet; that is task I4.
- **No frontend or Tauri instructions.** Tasks S1 and S2.
