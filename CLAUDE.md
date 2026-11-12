# Serat — Agent Team Conventions

Serat is a local-first, Obsidian-compatible knowledge app: **Tauri v2 shell, Rust core (workspace of 8 crates), Vue 3 / Vite / TypeScript frontend, plain `.md` files + rebuildable SQLite index**. Read this file before doing anything; then read the artifact for your phase.

## Canonical documents (read before acting)
| Need | Read |
|---|---|
| Vision, principles, milestones P0–P5, gates | docs/01-planning/project-charter.md, docs/01-planning/risk-register.md |
| Requirements (FR-/NFR-), acceptance criteria, traceability, glossary, assumptions A-nn, constraints C-nn | docs/02-requirements/prd.md |
| User stories US-01..42 with Given/When/Then | docs/02-requirements/user-stories.md |
| Crate graph, threading, data model, key flows, verification harness | docs/03-design/architecture.md |
| The ten expensive-to-reverse decisions | docs/03-design/adr/0001..0010 |
| Security mechanisms | docs/03-design/threat-model.md |
| Brand: name, colours, type | docs/03-design/ui/serat-identity.md |
| Index of everything | docs/SERAT-INDEX.md |

## Non-negotiables (constraints C-01..C-07 — requirements may not reopen these)
- **C-01** Files stay plain Markdown; no feature may require proprietary syntax.
- **C-02** The index (`.serat/`) is disposable; nothing may depend on state that lives only there.
- **C-03** No JavaScript in the query path; SQ is statically analysable, compiled to exactly one SQL statement (ADR-0010).
- **C-04** Writes patch byte ranges; never re-serialise a note (ADR-0008).
- **C-05** No AI write lands without diff review; there is no config that turns this off.
- **C-06** Performance budgets (NFR-PRF-01..09) are binding and enforced in CI against the synthetic 20k/50k vault.
- **C-07** Tauri v2 + Vue 3 are fixed decisions.

Design goals in priority order (higher wins a conflict): 1 the vault survives the application · 2 nothing blocks the keystroke · 3 correctness is provable, not argued · 4 the cost of a query is knowable before it runs · 5 capability degrades, nothing breaks.

## Identifier scheme (permanent — never reuse a dropped ID)
`FR-AREA-nn` functional · `NFR-AREA-nn` non-functional · `US-nn` story · `A-nn` assumption · `C-nn` constraint · `ADR-nnnn` decision · `RISK-nnn` risk · `T-nn` threat. Areas: VLT EDT NAV STR VIW BIN LNS AIX MCP HST PLT / PRF SEC REL CMP USE MNT.

## Roles (see .claude/agents/)
- **lead** (main session): plans phases, splits work into tasks, merges. Writes no code.
- **product-analyst**: owns docs/02-requirements. Re-baselines the PRD when P0 numbers land; never edits an ID.
- **architect**: owns docs/03-design; writes ADRs; produces the repo-ready design files (`schema/`, `grammar/`, `contracts/`).
- **rust-core-dev**: crates serat-core, -vault, -parse, -index, -sq, -extract, -ai, -mcp.
- **tauri-app-dev**: crate serat-app (Tauri commands/events, review queue) and the IPC contract, `ts-rs` generated types.
- **vue-frontend-dev**: `app/` — Vue 3, Pinia, CodeMirror 6 editor, views, lens UI.
- **frontend-designer**: screen specs, tokens, component inventory in docs/03-design/ui.
- **test-engineer**: benchmark vault generator, CI budget jobs, conformance suite, property tests; docs/05-testing.
- **code-reviewer**: read-only; checks constraints, ADRs, budgets, security mechanisms.
- **release-engineer**: signing/notarisation, bundler targets, auto-update, release notes; docs/06-release.

## Working agreement
1. Every unit of work is a task on the shared task list, subject `[Pn/phase] imperative`, e.g. `[P0/impl] Index 20k-note benchmark vault in serat-index`. Claim (set owner) before starting.
2. Every task description names the FR/NFR/US IDs it serves and the ADR(s) it must honour. A task with no ID is not ready.
3. Rust work lives in the crate that owns it (docs/03-design/architecture.md §2.1 table). Crossing the dependency rule (anything but serat-core as an internal dep, or depending on serat-app) needs an ADR first.
4. Nothing merges that breaches a budget; a PR that adds an index read path, a parse step or a query shape adds or updates the benchmark that measures it.
5. Teammates message each other directly on cross-role decisions; escalate to lead only after one exchange fails.
6. Each phase ends on its gate (charter §Milestones), not a date. P0 numbers replace blueprint targets before P1 starts.

## Definition of done
- Artifact in the right docs/ folder, or code in the right crate, citing its IDs.
- `cargo test --workspace` and `npm test` (in `app/`) pass; `cargo clippy -- -D warnings` clean; budget jobs green when touched.
- For write-back, patcher, extraction or reconciliation changes: the relevant property test / hostile test passes (concurrent edit, undo/redo, crash mid-write).
- Reviewed by code-reviewer with no BLOCKING findings; task description links artifact/branch.

## Repo layout (target)
```
crates/serat-{core,vault,parse,index,sq,extract,ai,mcp,app}/
app/            Vue 3 frontend
schema/ grammar/ contracts/   repo-ready design files (architect)
bench/          benchmark vault generator + budget jobs
docs/           SDLC artifacts by phase
```
