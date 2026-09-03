# Coding Standards

- Language/framework: Rust (workspace of crates under `crates/`) + Vue 3 / Vite / TypeScript in `app/`; Tauri v2 shell. Fixed by C-07.
- Formatting: `cargo fmt` and the frontend formatter before committing. `cargo clippy --workspace -- -D warnings` must be clean.
- Branch naming: `task/<task-id>-<slug>`. One task per branch — see "Worktrees" below for how this works with several teammates at once.
- Commit messages: imperative subject, body references the task ID and the FR-/NFR-/US-/ADR- IDs it serves.
- Every public function has a docstring; every bug fix has a regression test.
- No secrets in code; use environment variables and document them in dev-setup.md.

## Worktrees — one per teammate

Several agents work this repository at the same time, so per-task branches in a single working copy would mean one teammate checking out over another's uncommitted work. Each teammate therefore owns a git worktree:

| Teammate | Worktree | Owns |
|---|---|---|
| architect | `.worktrees/architect` | `docs/03-design/`, `schema/`, `contracts/` |
| rust-core-dev | `.worktrees/rust-core` | `crates/serat-{core,vault,parse,index}` |
| tauri-app-dev | `.worktrees/tauri-app` | `crates/serat-app`, `contracts/ipc.rs` |
| vue-frontend-dev | `.worktrees/vue-frontend` | `app/` |
| test-engineer | `.worktrees/test` | `bench/`, `docs/05-testing/` |
| code-reviewer | `.worktrees/review` | nothing — checks out others' branches read-only |

`.worktrees/` is gitignored and is derived state: every branch inside it also lives in the primary repository at the repo root. Deleting the directory loses nothing that has been committed.

**Per task:**

1. `cd .worktrees/<yours>` — stay inside it. Never edit files in the repo root working copy; that is the integration copy and the lead owns it.
2. `git switch -c task/<id>-<slug> main` — branch from `main`, which is checked out at the root. (Creating a branch from `main` is allowed from a worktree; *checking out* `main` there is not, and you do not need to.)
3. Work, then commit on that branch with the task ID and its FR/NFR/US/ADR IDs in the body. Stage only files you own.
4. Tell the lead the branch name. code-reviewer reviews it; the lead merges to `main` after approval, and the lead alone merges.
5. Start the next task from step 2, branching from the updated `main`.

**Consequences to know about:**

- Each worktree has its own `target/` and `node_modules/`, so the first build in a worktree is a cold one. That is the price of not blocking each other.
- Exclude `.worktrees/` from every recursive filesystem walk — the benchmark vault generator, the dependency-rule check, and anything else that walks the tree. Otherwise a worktree's copy of the workspace is discovered as if it were part of it.
- The root workspace `Cargo.toml` carries `exclude = [".worktrees"]` for the same reason.
- Another teammate's unmerged work is invisible to you. If you are blocked on it, message them and ask for the branch, or ask the lead to merge — do not reimplement it.
